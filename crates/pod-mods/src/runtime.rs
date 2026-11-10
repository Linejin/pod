//! One mod's QuickJS sandbox: its own `Runtime` (memory limit, stack limit,
//! interrupt handler) and `Context` with the prelude installed (docs/16 §4).
//!
//! The sandbox has only ECMAScript built-ins plus what the prelude defines:
//! quickjs-libc (`std`, `os`) is not compiled into rquickjs and no module
//! loader is set, so `import`/`require` cannot reach anything.

use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Instant;

use rquickjs::context::EvalOptions;
use rquickjs::{Coerced, Context, Ctx, Exception, Function, Runtime, Value};

const PRELUDE: &str = include_str!("prelude.js");

/// Resource limits for one mod.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// QuickJS allocator limit in bytes.
    pub memory: usize,
    pub stack: usize,
    /// CPU budget for loading (prelude + top-level code).
    pub load_ms: u64,
    /// CPU budget for one callback, pending Promise jobs included.
    pub call_ms: u64,
    /// Uncaught errors in a row before the mod is stopped.
    pub max_errors_in_row: u32,
    /// Side-effect actions (notify, send, check, gate, status) per callback.
    pub actions_per_call: u32,
    /// Notify/send/check/gate per minute.
    pub actions_per_minute: u32,
    /// Pending `setTimeout`/`setInterval` timers per mod.
    pub max_timers: u32,
    /// Shorter `setTimeout` delays are raised to this.
    pub min_timeout_ms: u64,
    /// Shorter `setInterval` periods are raised to this.
    pub min_interval_ms: u64,
}

impl Default for Limits {
    fn default() -> Self {
        Limits {
            memory: 16 * 1024 * 1024,
            stack: 256 * 1024,
            load_ms: 200,
            call_ms: 50,
            max_errors_in_row: 3,
            actions_per_call: 20,
            actions_per_minute: 60,
            max_timers: 32,
            min_timeout_ms: 10,
            min_interval_ms: 100,
        }
    }
}

/// Why a call into the mod failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Failure {
    /// The CPU budget ran out (interrupted; JS cannot catch this).
    Timeout,
    /// Out of memory, or memory use stayed near the limit.
    Memory(String),
    /// An uncaught JS exception (message and stack).
    Exception(String),
}

impl Failure {
    /// Limit violations stop the mod at once; exceptions are counted.
    pub fn is_limit(&self) -> bool {
        !matches!(self, Failure::Exception(_))
    }
}

impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Failure::Timeout => write!(f, "CPU time limit exceeded"),
            Failure::Memory(m) => write!(f, "memory limit exceeded ({m})"),
            Failure::Exception(m) => write!(f, "{m}"),
        }
    }
}

/// The host call behind the prelude's `host(op, json)`: returns the JSON
/// result, or an error message that becomes a JS exception.
pub type HostFn = Rc<dyn Fn(&str, &str) -> Result<Option<String>, String>>;

pub struct ModRuntime {
    // Field order: the context goes before the runtime.
    ctx: Context,
    rt: Runtime,
    epoch: Instant,
    /// Deadline in ms since `epoch` (+1, 0 = none) checked by the interrupt handler.
    deadline: Arc<AtomicU64>,
    interrupted: Arc<AtomicBool>,
    /// Unhandled Promise rejections since the call started (an OOM or
    /// interrupt inside a Promise job ends up here, not as a job error).
    rejections: Rc<std::cell::RefCell<Vec<String>>>,
    limits: Limits,
}

/// `Name: message` plus the stack of a thrown value.
fn describe(v: Value<'_>) -> String {
    if let Some(obj) = v.as_object()
        && let Some(ex) = Exception::from_object(obj.clone())
    {
        let name: String = obj.get("name").unwrap_or_else(|_| "Error".into());
        let msg = ex.message().unwrap_or_default();
        // Host errors already carry a category ("PermissionDenied: ...").
        let categorized = msg
            .split_once(": ")
            .is_some_and(|(p, _)| !p.is_empty() && p.chars().all(|c| c.is_ascii_alphabetic()));
        let head = if categorized {
            msg
        } else {
            format!("{name}: {msg}")
        };
        let stack = ex.stack().unwrap_or_default();
        return if stack.trim().is_empty() {
            head
        } else {
            format!("{head}\n{}", stack.trim_end())
        };
    }
    v.get::<Coerced<String>>()
        .map(|c| c.0)
        .unwrap_or_else(|_| "exception".into())
}

fn error_text(ctx: &Ctx<'_>, e: rquickjs::Error) -> String {
    match e {
        rquickjs::Error::Exception => describe(ctx.catch()),
        other => other.to_string(),
    }
}

impl ModRuntime {
    /// Create the sandbox, install the prelude and run the mod's top-level code.
    pub fn load(
        name: &str,
        version: &str,
        source: &str,
        filename: &str,
        limits: Limits,
        host: HostFn,
    ) -> Result<ModRuntime, Failure> {
        let rt = Runtime::new().map_err(|e| Failure::Exception(format!("runtime: {e}")))?;
        rt.set_memory_limit(limits.memory);
        rt.set_max_stack_size(limits.stack);
        let epoch = Instant::now();
        let deadline = Arc::new(AtomicU64::new(0));
        let interrupted = Arc::new(AtomicBool::new(false));
        {
            let (deadline, interrupted) = (deadline.clone(), interrupted.clone());
            rt.set_interrupt_handler(Some(Box::new(move || {
                let d = deadline.load(Ordering::Relaxed);
                if d != 0 && epoch.elapsed().as_millis() as u64 + 1 > d {
                    interrupted.store(true, Ordering::Relaxed);
                    return true;
                }
                false
            })));
        }
        let rejections: Rc<std::cell::RefCell<Vec<String>>> = Rc::default();
        {
            let rejections = rejections.clone();
            rt.set_host_promise_rejection_tracker(Some(Box::new(
                move |_ctx, _promise, reason, handled| {
                    if !handled {
                        rejections.borrow_mut().push(describe(reason));
                    }
                },
            )));
        }
        let ctx = Context::full(&rt).map_err(|e| Failure::Memory(format!("context: {e}")))?;
        let me = ModRuntime {
            ctx,
            rt,
            epoch,
            deadline,
            interrupted,
            rejections,
            limits,
        };
        let (name, version, source, filename) = (
            name.to_string(),
            version.to_string(),
            source.to_string(),
            filename.to_string(),
        );
        me.run(limits.load_ms, move |ctx| {
            let prelude: Function = ctx.eval(PRELUDE)?;
            let hostf = Function::new(
                ctx.clone(),
                move |ctx: Ctx<'_>, op: String, args: String| -> rquickjs::Result<Option<String>> {
                    host(&op, &args).map_err(|msg| Exception::throw_message(&ctx, &msg))
                },
            )?;
            prelude.call::<_, ()>((hostf, name, version))?;
            let mut opts = EvalOptions::default();
            opts.filename = Some(filename);
            ctx.eval_with_options::<(), _>(source, opts)?;
            Ok(None)
        })?;
        Ok(me)
    }

    /// Call the prelude's dispatcher: `kind` = `event` | `command` | `screen` | `title` | `timer`.
    pub fn call(&self, kind: &str, key: &str, payload: &str) -> Result<Option<String>, Failure> {
        let (kind, key, payload) = (kind.to_string(), key.to_string(), payload.to_string());
        self.run(self.limits.call_ms, move |ctx| {
            let f: Function = ctx.globals().get("__pod_dispatch")?;
            f.call::<_, Option<String>>((kind, key, payload))
        })
    }

    pub fn memory_used(&self) -> usize {
        self.rt.memory_usage().malloc_size.max(0) as usize
    }

    /// Run `f` and pending jobs under a CPU deadline, then classify failures.
    fn run<F>(&self, budget_ms: u64, f: F) -> Result<Option<String>, Failure>
    where
        F: for<'js> FnOnce(Ctx<'js>) -> rquickjs::Result<Option<String>>,
    {
        self.interrupted.store(false, Ordering::Relaxed);
        self.rejections.borrow_mut().clear();
        let now = self.epoch.elapsed().as_millis() as u64;
        self.deadline.store(now + budget_ms + 1, Ordering::Relaxed);
        let mut result = self
            .ctx
            .with(|ctx| f(ctx.clone()).map_err(|e| error_text(&ctx, e)));
        // Promise jobs count against the same budget. Memory is sampled now
        // and then: an OOM inside a job only shows up as a `null` rejection.
        let (mut jobs, mut near_oom) = (0u64, false);
        while result.is_ok() && !self.interrupted.load(Ordering::Relaxed) {
            match self.rt.execute_pending_job() {
                Ok(true) => {
                    jobs += 1;
                    if jobs % 1024 == 0
                        && self.memory_used() as u64 * 10 > self.limits.memory as u64 * 9
                    {
                        near_oom = true;
                    }
                    continue;
                }
                Ok(false) => break,
                Err(job) => {
                    result = Err(job.0.with(|ctx| describe(ctx.catch())));
                }
            }
        }
        self.deadline.store(0, Ordering::Relaxed);
        let rejected = std::mem::take(&mut *self.rejections.borrow_mut());
        // QuickJS rejects with `null` when it ran out of memory creating the
        // error itself; the heap may be freed again by the time jobs end.
        if !rejected.is_empty()
            && (near_oom || self.memory_used() as u64 * 10 > self.limits.memory as u64 * 9)
        {
            self.rt.run_gc();
            return Err(Failure::Memory("out of memory in a Promise job".into()));
        }
        if result.is_ok()
            && let Some(first) = rejected.first()
        {
            // An unhandled rejection is the mod's uncaught error.
            result = Err(format!("Uncaught (in promise) {first}"));
        }
        if self.interrupted.swap(false, Ordering::Relaxed)
            || rejected.iter().any(|r| r.contains("interrupted"))
        {
            return Err(Failure::Timeout);
        }
        let used = self.memory_used();
        let near_limit = used as u64 * 10 > self.limits.memory as u64 * 9;
        match result {
            Err(msg) if msg.contains("out of memory") || msg.contains("Allocation failed") => Err(
                Failure::Memory(msg.lines().next().unwrap_or_default().to_string()),
            ),
            _ if near_limit => {
                self.rt.run_gc();
                if self.memory_used() as u64 * 10 > self.limits.memory as u64 * 9 {
                    Err(Failure::Memory(format!(
                        "{} KiB in use",
                        self.memory_used() / 1024
                    )))
                } else {
                    result.map_err(Failure::Exception)
                }
            }
            Err(msg) => Err(Failure::Exception(msg)),
            Ok(v) => Ok(v),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    /// Host calls seen by the test host: `(op, json)`.
    type Calls = Rc<RefCell<Vec<(String, String)>>>;

    fn load(src: &str) -> (Result<ModRuntime, Failure>, Calls) {
        let calls = Rc::new(RefCell::new(Vec::new()));
        let c = calls.clone();
        let host: HostFn = Rc::new(move |op: &str, args: &str| {
            c.borrow_mut().push((op.to_string(), args.to_string()));
            if op == "fail" {
                Err("PermissionDenied: nope".into())
            } else {
                Ok(None)
            }
        });
        let limits = Limits {
            load_ms: 300,
            call_ms: 100,
            ..Limits::default()
        };
        (
            ModRuntime::load("t", "1", src, "main.js", limits, host),
            calls,
        )
    }

    #[test]
    fn events_reach_handlers_and_logs_reach_the_host() {
        let (rt, calls) = load(r#"pod.on("x", (e) => console.log("got", e.n));"#);
        let rt = rt.unwrap();
        rt.call("event", "x", r#"{"n":7}"#).unwrap();
        let c = calls.borrow();
        assert_eq!(c[0].0, "register.event");
        assert_eq!(c[1], ("log".into(), r#"{"msg":"got 7"}"#.into()));
    }

    #[test]
    fn exceptions_have_messages() {
        let (rt, _) = load(r#"pod.on("x", () => { throw new TypeError("bad thing"); });"#);
        let err = rt.unwrap().call("event", "x", "{}").unwrap_err();
        match err {
            Failure::Exception(m) => assert!(m.starts_with("TypeError: bad thing"), "{m}"),
            other => panic!("{other:?}"),
        }
        let (rt, _) = load("syntax error here(");
        assert!(matches!(rt.err().unwrap(), Failure::Exception(m) if m.contains("SyntaxError")));
    }

    #[test]
    fn loops_are_interrupted_and_uncatchable() {
        let (rt, _) = load("for(;;){}");
        assert_eq!(rt.err().unwrap(), Failure::Timeout);
        let (rt, _) = load(r#"pod.on("x", () => { for(;;) { try { for(;;){} } catch (e) {} } });"#);
        let rt = rt.unwrap();
        let t = Instant::now();
        assert_eq!(rt.call("event", "x", "{}").unwrap_err(), Failure::Timeout);
        assert!(t.elapsed().as_millis() < 2000);
        // A stuck Promise chain counts too.
        let (rt, _) = load(
            r#"let n = 0; pod.on("x", () => { const spin = () => { n++; return Promise.resolve().then(spin); }; spin(); });"#,
        );
        let rt = rt.unwrap();
        let r = rt.call("event", "x", "{}").unwrap_err();
        // Interrupted, or out of memory first (the chain keeps promises alive).
        assert!(r.is_limit(), "{r:?}");
        let n: i32 = rt.ctx.with(|ctx| ctx.eval("n").unwrap());
        assert!(n > 100, "the chain ran {n} times");
        // A plain rejected promise is an ordinary (counted) error.
        let (rt, _) = load(r#"pod.on("x", async () => { throw new Error("later"); });"#);
        assert!(
            matches!(rt.unwrap().call("event", "x", "{}").unwrap_err(), Failure::Exception(m) if m.contains("in promise") && m.contains("later"))
        );
    }

    #[test]
    fn memory_bombs_hit_the_limit() {
        let (rt, _) = load(
            r#"pod.on("x", () => { const a = []; for(;;) a.push("x".repeat(1 << 16) + a.length); });"#,
        );
        match rt.unwrap().call("event", "x", "{}").unwrap_err() {
            Failure::Memory(_) => {}
            other => panic!("{other:?}"),
        }
        let (rt, _) = load(
            r#"pod.on("x", () => { globalThis.keep = []; try { for(;;) keep.push(new Array(1e5).fill(1)); } catch (e) {} });"#,
        );
        match rt.unwrap().call("event", "x", "{}").unwrap_err() {
            Failure::Memory(_) => {}
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn no_filesystem_network_process_or_modules() {
        let probe = r#"
            const out = {};
            for (const n of ["require","process","std","os","fetch","XMLHttpRequest","WebSocket","Deno","Bun","setImmediate","__host","host","print","scriptArgs","module","exports"]) out[n] = typeof globalThis[n];
            out.importRejected = false;
            import("os").then(() => { out.importResolved = true; }, () => { out.importRejected = true; });
            pod.on("probe", () => console.log(JSON.stringify(out)));
            let evalOk; try { evalOk = typeof eval("typeof std"); } catch (e) { evalOk = "threw"; }
            out.evalStd = eval("typeof std");
            out.fnCtor = new Function("return typeof os")();
            try { pod.notify = null; out.podWritable = pod.notify === null; } catch (e) { out.podWritable = false; }
            try { globalThis.__pod_dispatch = () => 1; out.dispatchWritable = true; } catch (e) { out.dispatchWritable = false; }
            // Timers exist (host-driven) but cannot be replaced or fed strings.
            out.timers = [typeof setTimeout, typeof setInterval, typeof clearTimeout, typeof clearInterval].join();
            try { globalThis.setTimeout = null; out.timerWritable = setTimeout === null; } catch (e) { out.timerWritable = false; }
            try { setTimeout("pod.log(1)", 1); out.stringTimer = "accepted"; } catch (e) { out.stringTimer = e.message; }
        "#;
        let (rt, calls) = load(probe);
        let rt = rt.unwrap();
        rt.call("event", "probe", "{}").unwrap();
        let c = calls.borrow();
        let line = &c.iter().find(|(op, _)| op == "log").unwrap().1;
        let v: serde_json::Value = serde_json::from_str(line).unwrap();
        let out: serde_json::Value = serde_json::from_str(v["msg"].as_str().unwrap()).unwrap();
        for n in [
            "require",
            "process",
            "std",
            "os",
            "fetch",
            "XMLHttpRequest",
            "WebSocket",
            "Deno",
            "Bun",
            "setImmediate",
            "__host",
            "host",
            "print",
            "scriptArgs",
            "module",
            "exports",
        ] {
            assert_eq!(out[n], "undefined", "{n} must not exist: {out}");
        }
        assert_eq!(out["importRejected"], true, "{out}");
        assert!(out.get("importResolved").is_none());
        assert_eq!(out["evalStd"], "undefined");
        assert_eq!(out["fnCtor"], "undefined");
        assert_eq!(out["podWritable"], false);
        assert_eq!(out["dispatchWritable"], false);
        assert_eq!(out["timers"], "function,function,function,function");
        assert_eq!(out["timerWritable"], false);
        assert!(
            out["stringTimer"]
                .as_str()
                .unwrap()
                .contains("needs a function"),
            "{out}"
        );
        assert!(
            !c.iter().any(|(op, _)| op == "timer.set"),
            "a string timer must not reach the host"
        );
    }

    #[test]
    fn host_errors_become_catchable_exceptions() {
        let (rt, calls) = load(
            r#"
            pod.on("x", () => {
              try { pod.log("a"); __never(); } catch (e) { pod.log("caught " + e.name); }
            });
        "#,
        );
        rt.unwrap().call("event", "x", "{}").unwrap();
        assert!(
            calls
                .borrow()
                .iter()
                .any(|(_, a)| a.contains("caught ReferenceError"))
        );
    }
}

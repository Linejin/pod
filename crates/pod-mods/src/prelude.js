// Pod mod prelude (docs/16 §8). Evaluated before the mod's entry script.
// Receives the native `host(name, json) -> json` call and the mod's name and
// version; defines the frozen `pod` and `console` globals, the timer
// functions, and the hidden, read-only `__pod_dispatch` the host calls for
// events, commands, detectors and due timers. Permissions, scope and timer
// limits are checked on the Rust side; the mod thread decides when a timer
// is due.
(function (host, name, version) {
  "use strict";
  const call = (op, args) => {
    const r = host(op, JSON.stringify(args === undefined ? null : args));
    return r === undefined || r === null ? undefined : JSON.parse(r);
  };
  const str = (v) => (typeof v === "string" ? v : v === undefined || v === null ? "" : String(v));
  const fmt = (args) =>
    Array.prototype.map
      .call(args, (a) => {
        if (typeof a === "string") return a;
        if (a instanceof Error) return a.stack ? `${a.name}: ${a.message}\n${a.stack}` : `${a.name}: ${a.message}`;
        try {
          return JSON.stringify(a);
        } catch (_) {
          return String(a);
        }
      })
      .join(" ");

  const handlers = new Map();
  const commands = new Map();
  const detectors = [];
  // Timer id (allocated by the host) -> {fn, args, repeat}.
  const timers = new Map();

  const pod = {
    name,
    version,
    on(event, fn) {
      if (typeof fn !== "function") throw new TypeError("InvalidArgument: pod.on needs a function");
      call("register.event", { event: str(event) });
      const list = handlers.get(event) || [];
      list.push(fn);
      handlers.set(event, list);
    },
    command(spec, fn) {
      if (!spec || typeof fn !== "function") throw new TypeError("InvalidArgument: pod.command({id, title}, fn)");
      call("register.command", { id: str(spec.id), title: str(spec.title), detail: spec.detail === undefined ? null : str(spec.detail) });
      commands.set(spec.id, fn);
    },
    detector(spec) {
      if (!spec || (typeof spec.onScreen !== "function" && typeof spec.onTitle !== "function"))
        throw new TypeError("InvalidArgument: pod.detector needs onScreen or onTitle");
      const match = Array.isArray(spec.match) ? spec.match.map(str) : [str(spec.match)];
      call("register.detector", { match });
      detectors.push(spec);
    },
    notify(title, body, level) {
      call("notify", { title: str(title), body: str(body), level: level === undefined ? "info" : str(level) });
    },
    log() {
      call("log", { msg: fmt(arguments) });
    },
    terminals() {
      return call("query.terminals");
    },
    tasks() {
      return call("query.tasks");
    },
    groups() {
      return call("query.groups");
    },
    terminal: Object.freeze({
      send(id, text, opts) {
        call("terminal.send", { terminal: str(id), text: str(text), enter: !!(opts && opts.enter) });
      },
      setStatus(id, state, opts) {
        call("terminal.status", {
          terminal: str(id),
          state: str(state),
          kind: opts && opts.kind !== undefined ? str(opts.kind) : null,
          msg: opts && opts.msg !== undefined ? str(opts.msg) : null,
        });
      },
    }),
    task: Object.freeze({
      runCheck(task, check) {
        call("task.check", { task: str(task), check: str(check) });
      },
    }),
    gate: Object.freeze({
      request(task, title, message) {
        return call("gate.request", { task: str(task), title: str(title), message: str(message) });
      },
    }),
  };
  const log = function () {
    call("log", { msg: fmt(arguments) });
  };
  const ro = (v) => ({ value: v, writable: false, configurable: false, enumerable: false });
  Object.defineProperty(globalThis, "pod", ro(Object.freeze(pod)));
  Object.defineProperty(globalThis, "console", ro(Object.freeze({ log, info: log, warn: log, error: log, debug: log })));

  const setTimer = (repeat, what) =>
    function (fn, delay) {
      if (typeof fn !== "function") throw new TypeError(`InvalidArgument: ${what} needs a function (strings are not evaluated)`);
      const d = Number(delay);
      const id = call("timer.set", { delay: Number.isFinite(d) && d > 0 ? d : 0, repeat });
      timers.set(id, { fn, args: Array.prototype.slice.call(arguments, 2), repeat });
      return id;
    };
  const clearTimer = function (id) {
    const n = Number(id);
    if (!timers.delete(n)) return;
    call("timer.clear", { id: n });
  };
  Object.defineProperty(globalThis, "setTimeout", ro(setTimer(false, "setTimeout")));
  Object.defineProperty(globalThis, "setInterval", ro(setTimer(true, "setInterval")));
  Object.defineProperty(globalThis, "clearTimeout", ro(clearTimer));
  Object.defineProperty(globalThis, "clearInterval", ro(clearTimer));

  const hint = (r) => {
    if (r === undefined || r === null) return null;
    if (typeof r !== "object") throw new TypeError("InvalidArgument: a detector returns null or {state, kind?, msg?}");
    return JSON.stringify({ state: str(r.state), kind: r.kind === undefined ? null : str(r.kind), msg: r.msg === undefined ? null : str(r.msg) });
  };
  const dispatch = function (kind, key, json) {
    const payload = JSON.parse(json);
    if (kind === "event") {
      for (const fn of handlers.get(key) || []) fn(payload);
      for (const fn of handlers.get("*") || []) fn(payload, key);
      return null;
    }
    if (kind === "command") {
      const fn = commands.get(key);
      if (fn) fn(payload);
      return null;
    }
    if (kind === "screen" || kind === "title") {
      const d = detectors[Number(key)];
      const fn = d && (kind === "screen" ? d.onScreen : d.onTitle);
      return typeof fn === "function" ? hint(fn.call(d, payload)) : null;
    }
    if (kind === "timer") {
      const id = Number(key);
      const t = timers.get(id);
      if (!t) return null;
      if (!t.repeat) timers.delete(id);
      t.fn.apply(undefined, t.args);
      return null;
    }
    return null;
  };
  Object.defineProperty(globalThis, "__pod_dispatch", ro(dispatch));
});

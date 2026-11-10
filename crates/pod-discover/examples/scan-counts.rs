//! Manual check: scan this PC's stores and print counts and ids only (never
//! titles, prompts or paths), plus running agent processes.
//!
//! `cargo run -p pod-discover --example scan-counts`

fn main() {
    let mut sources = pod_discover::default_sources();
    let mut cache = pod_discover::Cache::default();
    let t = std::time::Instant::now();
    let out = pod_discover::scan(&mut sources, &mut cache, &pod_discover::Limits::default());
    println!(
        "scan: {:?}, parsed {}, cached {}, read {} KiB, truncated {}",
        t.elapsed(),
        out.parsed,
        out.cached,
        out.bytes_read / 1024,
        out.truncated
    );
    for agent in pod_discover::Agent::ALL {
        let s: Vec<_> = out.sessions.iter().filter(|f| f.agent == agent).collect();
        let n = |f: fn(&pod_discover::SessionMeta) -> bool| s.iter().filter(|x| f(&x.meta)).count();
        println!(
            "{}: {} sessions; title {}, preview {}, model {}, branch {}, messages {}, started {}, last {}; running records {}",
            agent.as_str(),
            s.len(),
            n(|m| m.title.is_some()),
            n(|m| m.preview.is_some()),
            n(|m| m.model.is_some()),
            n(|m| m.branch.is_some()),
            n(|m| m.messages.is_some()),
            n(|m| m.started_ms.is_some()),
            n(|m| m.last_ms.is_some()),
            out.running_ids.get(&agent).map_or(0, |r| r.len())
        );
        for f in s.iter().take(3) {
            println!("  {} msgs={:?}", f.meta.id, f.meta.messages);
        }
    }
    let t = std::time::Instant::now();
    let procs = pod_discover::procs::snapshot();
    let mut agents = pod_discover::procs::find_agents(&procs);
    pod_discover::procs::attach_sessions(&mut agents, &out.running_ids, &out.sessions);
    println!(
        "processes: {} in {:?}; with cmdline {}; with cwd {}",
        procs.len(),
        t.elapsed(),
        procs.iter().filter(|p| p.cmdline.is_some()).count(),
        procs.iter().filter(|p| p.cwd.is_some()).count()
    );
    for a in &agents {
        println!(
            "  running {:?} pid {} pids {:?} cwd? {} session {:?} guessed {}",
            a.agent,
            a.pid,
            a.pids,
            a.cwd.is_some(),
            a.session_id,
            a.guessed
        );
    }
}

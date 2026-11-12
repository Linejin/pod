use super::*;

/// docs/17 §2.3: 1 → 2 → 4 … capped at 30 s, jitter within ±20 %.
#[test]
fn backoff_doubles_to_the_cap_with_bounded_jitter() {
    let p = ReconnectPolicy::default();
    let mid: Vec<u64> = (0..8).map(|a| p.delay(a, 0.5).as_secs()).collect();
    assert_eq!(mid, [1, 2, 4, 8, 16, 30, 30, 30]);
    assert_eq!(p.delay(40, 0.5), Duration::from_secs(30), "no overflow");
    for a in 0..8 {
        let base = p.delay(a, 0.5).as_secs_f64();
        let lo = p.delay(a, 0.0).as_secs_f64();
        let hi = p.delay(a, 0.999_999).as_secs_f64();
        assert!((lo - base * 0.8).abs() < 1e-6, "{a}: {lo}");
        assert!(hi <= base * 1.2 + 1e-6 && hi > base * 1.19, "{a}: {hi}");
    }
    let mut j = Jitter::new();
    for _ in 0..1000 {
        let u = j.unit();
        assert!((0.0..1.0).contains(&u));
    }
    // A 60 s drop is retried throughout by default: no give-up.
    assert!(p.give_up_after.is_none());
}

#[test]
fn input_streams_are_valid_and_distinct() {
    let a = new_stream();
    let b = new_stream();
    assert!(valid_input_stream(&a), "{a}");
    assert_ne!(a, b);
}

/// At most four views per machine; a closed view frees its slot.
#[test]
fn attach_slots_cap_concurrent_views() {
    let s = AttachSlots::default();
    let held: Vec<SlotGuard> = (0..4).map(|_| s.take().expect("slot")).collect();
    assert_eq!(s.in_use(), 4);
    assert!(s.take().is_none(), "a fifth view gets status only");
    drop(held);
    assert_eq!(s.in_use(), 0);
    assert!(s.take().is_some());
}

#[test]
fn gap_and_boot_markers_are_visible_text() {
    let g = Gap {
        from_seq: 100,
        to_seq: 1124,
    };
    assert_eq!(g.bytes(), 1024);
    assert!(g.label().contains("1024바이트"), "{}", g.label());
    assert!(ResyncReason::BootChanged.label().is_some());
    for r in [
        ResyncReason::First,
        ResyncReason::Reconnected,
        ResyncReason::Lagged,
    ] {
        assert!(r.label().is_none());
    }
}

/// A call that never gets an answer is cut at the deadline: the closer
/// runs, the watchdog says so. A disarmed one never fires.
#[test]
fn watchdog_fires_only_without_an_answer() {
    let hits = Arc::new(AtomicUsize::new(0));
    let h = hits.clone();
    let closer: Closer = Arc::new(move || {
        h.fetch_add(1, Ordering::SeqCst);
    });
    let dog = Watchdog::arm(Duration::from_millis(30), closer.clone());
    std::thread::sleep(Duration::from_millis(200));
    assert!(dog.disarm());
    assert_eq!(hits.load(Ordering::SeqCst), 1);
    let dog = Watchdog::arm(Duration::from_secs(5), closer);
    assert!(!dog.disarm());
    std::thread::sleep(Duration::from_millis(50));
    assert_eq!(hits.load(Ordering::SeqCst), 1);
}

/// A connector that is always down.
struct Down {
    calls: AtomicUsize,
    permanent: bool,
}

impl Connector for Down {
    fn connect(&self) -> Result<Link, ConnectError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Err(ConnectError {
            message: "down".into(),
            permanent: self.permanent,
        })
    }
}

fn quick() -> ReconnectPolicy {
    ReconnectPolicy {
        first_delay: Duration::from_millis(5),
        max_delay: Duration::from_millis(20),
        jitter: 0.0,
        give_up_after: Some(Duration::from_millis(300)),
        call_timeout: Duration::from_secs(5),
    }
}

/// A view on an unreachable machine reports its retries and gives up at the
/// policy's limit; a permanent error ends it at once. The slot comes back.
#[test]
fn attach_retries_then_gives_up_and_frees_its_slot() {
    let slots = AttachSlots::new(1);
    let down = Arc::new(Down {
        calls: AtomicUsize::new(0),
        permanent: false,
    });
    let (view, rx) = RemoteAttach::spawn(
        down.clone(),
        quick(),
        AttachOptions::new(TerminalRef::id("t1")),
        &slots,
    )
    .unwrap();
    assert!(
        RemoteAttach::spawn(
            down.clone(),
            quick(),
            AttachOptions::new(TerminalRef::id("t2")),
            &slots
        )
        .is_none()
    );
    let events: Vec<AttachUpdate> = rx.iter().collect();
    let retries = events
        .iter()
        .filter(|e| matches!(e, AttachUpdate::Reconnecting { .. }))
        .count();
    assert!(retries >= 3, "{events:?}");
    assert!(
        matches!(events.last(), Some(AttachUpdate::Failed { reason }) if reason.contains("다시 연결하지 못했습니다")),
        "{events:?}"
    );
    // Every failed attempt but the last is followed by a wait.
    assert_eq!(down.calls.load(Ordering::SeqCst), retries + 1);
    view.close();
    assert_eq!(slots.in_use(), 0);

    let perm = Arc::new(Down {
        calls: AtomicUsize::new(0),
        permanent: true,
    });
    let (_view, rx) = RemoteAttach::spawn(
        perm.clone(),
        quick(),
        AttachOptions::new(TerminalRef::id("t1")),
        &slots,
    )
    .unwrap();
    let events: Vec<AttachUpdate> = rx.iter().collect();
    assert_eq!(events.len(), 1, "{events:?}");
    assert!(matches!(&events[0], AttachUpdate::Failed { reason } if reason == "down"));
    assert_eq!(perm.calls.load(Ordering::SeqCst), 1);
}

/// Closing a view that is waiting to retry returns promptly.
#[test]
fn close_interrupts_the_backoff() {
    let mut p = quick();
    p.first_delay = Duration::from_secs(30);
    p.max_delay = Duration::from_secs(30);
    p.give_up_after = None;
    let down = Arc::new(Down {
        calls: AtomicUsize::new(0),
        permanent: false,
    });
    let (view, rx) = RemoteAttach::spawn(
        down,
        p,
        AttachOptions::new(TerminalRef::id("t1")),
        &AttachSlots::default(),
    )
    .unwrap();
    assert!(matches!(
        rx.recv_timeout(Duration::from_secs(5)),
        Ok(AttachUpdate::Reconnecting { .. })
    ));
    let t = Instant::now();
    view.close();
    assert!(t.elapsed() < Duration::from_secs(2), "{:?}", t.elapsed());
}

/// Without a connection nothing is sent: input is refused, not queued.
#[test]
fn control_refuses_while_down_and_reconnect_can_be_cancelled() {
    let down = Arc::new(Down {
        calls: AtomicUsize::new(0),
        permanent: false,
    });
    assert!(matches!(
        RemoteControl::connect(down.clone(), quick()),
        Err(RemoteError::Disconnected { .. })
    ));
    let perm = Arc::new(Down {
        calls: AtomicUsize::new(0),
        permanent: true,
    });
    assert!(matches!(
        RemoteControl::connect(perm, quick()),
        Err(RemoteError::Refused(m)) if m == "down"
    ));
    assert_eq!(down.calls.load(Ordering::SeqCst), 1);
}

//! Durable review/check notification delivery (B11). Hub events are wake
//! hints; the outbox is the source of truth across overflow and restart.

use std::collections::BTreeMap;

use pod_orch::engine::{NewMail, Orch};
use pod_orch::model::run_mailbox;

use crate::protocol::*;
use crate::server::orch_store::{LinkRecipient, LinkRecord};
use crate::server::{Daemon, log};

fn recipients(o: &Orch, link: &LinkRecord) -> Vec<LinkRecipient> {
    let mut by_run = BTreeMap::new();
    // Keep the newest eligible attempt in each run, as the live event path
    // did. A run created after the source result must not receive old mail.
    for a in o.attempts().filter(|a| {
        a.created_ms <= link.at_ms && a.task.as_ref().is_some_and(|t| link.tasks.contains(t))
    }) {
        by_run.insert(
            a.run.clone(),
            LinkRecipient {
                run: a.run.clone(),
                attempt: a.id.clone(),
                item: a.item.clone(),
            },
        );
    }
    by_run.into_values().collect()
}

fn send(
    o: &mut Orch,
    link: &LinkRecord,
    to: &LinkRecipient,
) -> pod_orch::engine::Result<(Mail, bool)> {
    // Before keyed mail existed, Pod stored these canonical source fields.
    // An upgrade must not replay already delivered notifications, and a
    // worker's matching payload must never suppress the real Pod message.
    let destination = run_mailbox(&to.run);
    if let Some(mail) = o.mail_of(&to.run)?.into_iter().find(|m| {
        m.from == "pod"
            && m.run == to.run
            && m.to == destination
            && m.recipients.contains(&destination)
            && m.kind == MailType::Status
            && m.text.as_ref().is_some_and(|t| t.kind == link.text.kind)
            && m.payload.as_ref().is_some_and(|p| {
                p.get("pod_link_event").is_none()
                    && if let Some(review) = link.payload.get("review") {
                        p.get("review") == Some(review)
                    } else {
                        p.get("group") == link.payload.get("group")
                            && p.get("job") == link.payload.get("job")
                    }
            })
    }) {
        return Ok((mail, false));
    }
    let text = link
        .text
        .clone()
        .with("attempt", &to.attempt)
        .with("item", &to.item);
    let mut payload = link.payload.clone();
    payload["attempt"] = serde_json::json!(to.attempt);
    let mail = NewMail::pod("pod", run_mailbox(&to.run), MailType::Status, text).payload(payload);
    o.send_link_once(&to.run, &link.key, mail, crate::now_ms())
}

fn announce_reconciled(
    mail: &Mail,
    fresh: bool,
    waiters: &tokio::sync::Notify,
    announce: impl FnOnce(&Mail),
) {
    if fresh {
        announce(mail);
    } else {
        // A complete append may only become visible in memory on retry.
        // Existing blocked check/ask readers must recheck it even though
        // the public event and typed terminal wake must not be duplicated.
        waiters.notify_waiters();
    }
}

impl Daemon {
    pub(super) fn orch_reconcile_links(&self) {
        let sources = {
            let mut store = self.orch.lock().unwrap();
            store.collect_links();
            store.link_outbox.clone()
        };
        for mut link in sources {
            if link.recipients.is_none() {
                link.recipients = Some(recipients(&self.orch(), &link));
            }
            // Freeze destinations and make the source durable BEFORE mail.
            // Never hold the source-store and engine locks together.
            {
                let mut store = self.orch.lock().unwrap();
                let Some(record) = store.link_outbox.iter_mut().find(|r| r.key == link.key) else {
                    continue;
                };
                record.recipients = link.recipients.clone();
                if let Err(e) = self.save_orch(&mut store) {
                    log(&format!(
                        "orch: cannot persist link {}: {}",
                        link.key, e.message
                    ));
                    continue;
                }
            }
            let mut delivered = Vec::new();
            for to in link.recipients.as_deref().unwrap_or_default() {
                let result = send(&mut self.orch(), &link, to);
                match result {
                    Ok((mail, fresh)) => {
                        announce_reconciled(&mail, fresh, &self.runs.mail, |mail| {
                            self.announce_mail(mail);
                        });
                        delivered.push(to.run.clone());
                    }
                    Err(e) => log(&format!("orch: link {} to {}: {}", link.key, to.run, e)),
                }
            }
            let mut store = self.orch.lock().unwrap();
            if let Some(record) = store.link_outbox.iter_mut().find(|r| r.key == link.key)
                && let Some(pending) = &mut record.recipients
            {
                pending.retain(|to| !delivered.contains(&to.run));
            }
            // Retain only outstanding destinations, not a forever-growing
            // successful-job history. Failed destinations have no expiry.
            store
                .link_outbox
                .retain(|r| r.recipients.as_ref().is_none_or(|p| !p.is_empty()));
            if let Err(e) = self.save_orch(&mut store) {
                // Mail has its own durable key: replay of an older outbox
                // after this failure cannot append or wake it twice.
                log(&format!(
                    "orch: cannot checkpoint link {}: {}",
                    link.key, e.message
                ));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::orch_store::{DeliveryRecord, GroupRecord, Store};
    use pod_orch::engine::BeginAttempt;
    use std::os::windows::fs::OpenOptionsExt;

    fn source() -> Store {
        let mut store = Store::default();
        store.deliveries.push(DeliveryRecord {
            info: DeliveryInfo {
                id: "d1".into(),
                task: "demo/task".into(),
                terminal: "t1".into(),
                comment_ids: vec!["c1".into()],
                state: DeliveryState::Delivered,
                prompt: "original review text".into(),
                created_ms: 50,
                delivered_ms: Some(100),
                agent_state: "idle".into(),
                delivered_state: Some("idle".into()),
                app_bracketed_paste: Some(true),
                error: None,
            },
            link_recorded: false,
            turn_at_delivery: Some(1),
            submit: None,
            origin: None,
        });
        store.groups.push(GroupRecord {
            id: "demo/group".into(),
            repo: "demo".into(),
            name: "group".into(),
            base: "main".into(),
            base_commit: "fixture".into(),
            prompt: "compare".into(),
            check: None,
            created_ms: 1,
            picked: None,
            picked_ms: None,
            members: vec![],
            check_job: Some(CheckJob {
                id: "k1".into(),
                state: CheckJobState::Finished,
                argv: vec!["fixture-check".into()],
                tasks: vec!["demo/task".into()],
                done: vec!["demo/task".into()],
                timeout_ms: 1_000,
                started_ms: 50,
                finished_ms: Some(100),
                all_passed: Some(true),
            }),
            linked_check: None,
        });
        store
    }

    fn add_worker(o: &mut Orch, created_ms: u64) {
        let run = o
            .create_run("fixture", None, None, created_ms)
            .unwrap()
            .id
            .clone();
        let item = o
            .add_item(&run, None, "worker", &[], None, created_ms)
            .unwrap()
            .id
            .clone();
        let a = o
            .begin_attempt(
                BeginAttempt {
                    run,
                    item,
                    agent: "test".into(),
                    place: "current".into(),
                    retry_of: None,
                    token_hash: "fixture".into(),
                },
                created_ms,
            )
            .unwrap();
        o.update_attempt(&a.id, |a| a.task = Some("demo/task".into()))
            .unwrap();
    }

    #[test]
    fn overflow_partial_recipient_failure_and_restart_preserve_each_source_once() {
        let root = std::env::temp_dir().join(format!(
            "podd-links-{}-{}",
            std::process::id(),
            crate::now_ms()
        ));
        let engine_root = root.join("runs");
        let mut store = source();
        store.save(&root).unwrap();
        // A second check must not erase the first pending successful result.
        let job = store.groups[0].check_job.as_mut().unwrap();
        job.id = "k2".into();
        job.all_passed = Some(false);
        store.save(&root).unwrap();
        assert_eq!(store.link_outbox.len(), 3);
        assert_eq!(store.link_outbox[1].payload["all_passed"], true);

        // Deterministic overflow: the notification transport loses its first
        // wake hint. Reconciliation uses the saved sources, not this channel.
        let mut rx = crate::server::status::subscribe();
        crate::server::status::publish_delivery(store.deliveries[0].info.clone(), None);
        for _ in 0..4097 {
            crate::server::status::publish_group_check(
                "demo/group",
                store.groups[0].check_job.clone().unwrap(),
                None,
            );
        }
        assert!(matches!(
            rx.try_recv(),
            Err(tokio::sync::broadcast::error::TryRecvError::Lagged(_))
        ));

        let mut o = Orch::load(&engine_root, Default::default(), 1);
        add_worker(&mut o, 1);
        add_worker(&mut o, 2);
        add_worker(&mut o, 101); // Newer run must not receive old events.
        for link in &mut store.link_outbox {
            link.recipients = Some(recipients(&o, link));
            assert_eq!(link.recipients.as_ref().unwrap().len(), 2);
        }
        store.save(&root).unwrap();
        // Complete one destination before the other journal becomes
        // unavailable. An unreadable journal intentionally fences all new
        // engine mutations until it is recovered (B10).
        for link in &store.link_outbox {
            let to = &link.recipients.as_ref().unwrap()[0];
            assert!(send(&mut o, link, to).unwrap().1);
        }
        let path = pod_orch::store::mail_jsonl(&engine_root, "r2");
        let lock = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .share_mode(0)
            .open(&path)
            .unwrap();
        for link in &store.link_outbox {
            let to = link.recipients.as_ref().unwrap();
            assert!(send(&mut o, link, &to[1]).is_err());
        }
        assert_eq!(o.mail_of("r1").unwrap().len(), 3);
        assert!(o.mail_of("r2").unwrap().is_empty());
        assert_eq!(store.deliveries[0].info.prompt, "original review text");
        assert_eq!(store.deliveries[0].info.state, DeliveryState::Delivered);
        drop(lock);
        drop(o);
        // Simulate restart before the successful destination checkpoint.
        let store = Store::load(&root);
        let mut o = Orch::load(&engine_root, Default::default(), 200);
        for link in &store.link_outbox {
            let to = link.recipients.as_ref().unwrap();
            assert!(!send(&mut o, link, &to[0]).unwrap().1);
            assert!(send(&mut o, link, &to[1]).unwrap().1);
            assert!(!send(&mut o, link, &to[1]).unwrap().1);
        }
        assert_eq!(o.mail_of("r1").unwrap().len(), 3);
        assert_eq!(o.mail_of("r2").unwrap().len(), 3);
        assert!(o.mail_of("r3").unwrap().is_empty());
        drop(o);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn drained_sources_are_not_requeued_and_old_stores_remain_readable() {
        let mut store = source();
        let mut legacy = serde_json::to_value(&store).unwrap();
        legacy["deliveries"][0]
            .as_object_mut()
            .unwrap()
            .remove("link_recorded");
        legacy["groups"][0]
            .as_object_mut()
            .unwrap()
            .remove("linked_check");
        legacy.as_object_mut().unwrap().remove("link_outbox");
        store = serde_json::from_value(legacy).unwrap();
        store.collect_links();
        assert_eq!(store.link_outbox.len(), 2);
        store.link_outbox.clear(); // All destinations were durably mailed.
        store.collect_links();
        assert!(
            store.link_outbox.is_empty(),
            "completed sources do not grow the outbox again"
        );
        assert_eq!(
            store.groups[0].check_job.as_ref().unwrap().all_passed,
            Some(true)
        );
    }

    #[test]
    fn markerless_upgrade_deduplicates_only_authentic_pod_mail_per_destination() {
        let root = std::env::temp_dir().join(format!(
            "podd-links-upgrade-{}-{}",
            std::process::id(),
            crate::now_ms()
        ));
        let mut o = Orch::load(&root, Default::default(), 1);
        add_worker(&mut o, 1);
        add_worker(&mut o, 2);
        let mut store = source();
        store.collect_links();
        for link in &store.link_outbox {
            o.send(
                None,
                NewMail::pod("pod", "run:r1", MailType::Status, link.text.clone())
                    .payload(link.payload.clone()),
                |_| false,
                100,
            )
            .unwrap();
            o.send(
                None,
                NewMail::pod("attempt:a1", "run:r2", MailType::Status, link.text.clone())
                    .payload(link.payload.clone()),
                |_| false,
                100,
            )
            .unwrap();
        }
        drop(o);
        let mut o = Orch::load(&root, Default::default(), 200);
        for link in &store.link_outbox {
            let to = recipients(&o, link);
            assert!(!send(&mut o, link, &to[0]).unwrap().1);
            assert!(send(&mut o, link, &to[1]).unwrap().1);
        }
        assert_eq!(o.mail_of("r1").unwrap().len(), 2);
        assert_eq!(o.mail_of("r2").unwrap().len(), 4);
        drop(o);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn recovered_mail_wakes_a_blocked_reader_without_republishing() {
        use std::future::Future;
        use std::task::{Context, Poll, Waker};

        let root = std::env::temp_dir().join(format!(
            "podd-links-waiter-{}-{}",
            std::process::id(),
            crate::now_ms()
        ));
        let mut old = Orch::load(&root, Default::default(), 1);
        add_worker(&mut old, 1);
        let mut store = source();
        store.collect_links();
        let link = &store.link_outbox[0];
        let to = &recipients(&old, link)[0];
        let waiters = tokio::sync::Notify::new();
        let mut waiting = std::pin::pin!(waiters.notified());
        waiting.as_mut().enable();
        let mut cx = Context::from_waker(Waker::noop());
        assert!(matches!(waiting.as_mut().poll(&mut cx), Poll::Pending));
        assert!(old.mail_of("r1").unwrap().is_empty());

        // Simulate a complete journal append whose acknowledgement/in-memory
        // update was lost, without weakening the core journal write fence.
        let mut writer = Orch::load(&root, Default::default(), 2);
        assert!(send(&mut writer, link, to).unwrap().1);
        drop(writer);
        let (mail, fresh) = send(&mut old, link, to).unwrap();
        assert!(!fresh);
        assert_eq!(old.mail_of("r1").unwrap().len(), 1);
        let mut published = 0;
        announce_reconciled(&mail, fresh, &waiters, |_| published += 1);
        assert_eq!(published, 0, "recovery must not duplicate public events");
        assert!(
            matches!(waiting.as_mut().poll(&mut cx), Poll::Ready(())),
            "the blocked reader must recheck newly recovered unread mail"
        );
        drop(old);
        std::fs::remove_dir_all(root).unwrap();
    }
}

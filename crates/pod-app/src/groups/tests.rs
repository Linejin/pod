use super::{AgentCounts, check_argv, check_line, check_summary, error_text, tail_lines, validate};
use crate::source::SourceError;
use podd::protocol::{AgentSpec, CheckResult, orch_codes};

#[test]
fn specs_and_member_names_follow_podd_rules() {
    let c = AgentCounts {
        claude: 1,
        codex: 2,
        shell: 1,
        custom: r#"py -c "print(1)""#.into(),
        custom_count: 1,
    };
    let specs = c.specs();
    assert_eq!(specs.len(), 5);
    assert_eq!(
        specs[4],
        AgentSpec::Argv {
            argv: vec!["py".into(), "-c".into(), "print(1)".into()]
        }
    );
    assert_eq!(
        c.member_names("fx"),
        [
            "fx-claude-1",
            "fx-codex-1",
            "fx-codex-2",
            "fx-shell-1",
            "fx-argv-1"
        ]
    );
    // A custom count without a command line adds nothing.
    let none = AgentCounts {
        custom_count: 2,
        ..Default::default()
    };
    assert!(none.specs().is_empty());
}

#[test]
fn validation_messages() {
    let one = AgentCounts {
        shell: 1,
        ..Default::default()
    };
    assert!(
        validate("fx", "do it", &one, false)
            .unwrap()
            .contains("저장소")
    );
    assert!(validate("", "do it", &one, true).unwrap().contains("이름"));
    assert!(
        validate("a b", "do it", &one, true)
            .unwrap()
            .contains("영문")
    );
    assert!(validate("fx", "  ", &one, true).unwrap().contains("요청"));
    assert!(
        validate("fx", "x", &AgentCounts::default(), true)
            .unwrap()
            .contains("에이전트")
    );
    assert!(
        validate(
            "fx",
            "x",
            &AgentCounts {
                shell: 17,
                ..Default::default()
            },
            true
        )
        .unwrap()
        .contains("16")
    );
    assert!(
        validate(
            "fx",
            "x",
            &AgentCounts {
                custom_count: 1,
                ..Default::default()
            },
            true
        )
        .is_some()
    );
    assert_eq!(validate("fx.1", "x", &one, true), None);
}

#[test]
fn check_command_round_trip() {
    assert_eq!(
        check_argv("  cargo test  ").unwrap(),
        ["cmd", "/c", "cargo test"]
    );
    assert!(check_argv("  ").is_none());
    assert_eq!(
        check_line(&check_argv("npm test -- --ci").unwrap()),
        "npm test -- --ci"
    );
    assert_eq!(check_line(&["cargo".into(), "test".into()]), "cargo test");
    // `pod group new ... -- cmd /c findstr /c:PASS result.txt` stores separate args.
    let cli: Vec<String> = ["cmd", "/c", "findstr", "/c:PASS", "result.txt"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    assert_eq!(check_line(&cli), "findstr /c:PASS result.txt");
}

#[test]
fn check_summaries_report_failure_as_failure() {
    let mut c = CheckResult {
        argv: vec![],
        passed: false,
        exit_code: Some(1),
        timed_out: false,
        duration_ms: 1500,
        finished_ms: 0,
        output_tail: "a\n\nb\r\nc\n".into(),
        error: None,
    };
    assert!(check_summary(&c).0.contains("검사 실패 · 종료 1 · 1.5초"));
    c.timed_out = true;
    assert!(check_summary(&c).0.contains("시간 초과"));
    c.timed_out = false;
    c.passed = true;
    c.exit_code = Some(0);
    assert!(
        check_summary(&c).0.starts_with("검사 통과"),
        "no text glyph: the row draws an icon"
    );
    assert_eq!(tail_lines(&c.output_tail, 2), ["b", "c"]);
}

#[test]
fn error_names() {
    let e = SourceError {
        code: Some(orch_codes::GROUP_EXISTS),
        message: "group demo/fx exists".into(),
    };
    assert_eq!(error_text(&e), "GROUP_EXISTS: group demo/fx exists");
}

/// The display plans and transitions A-P4 moved out of the panel (model.rs).
mod plans {
    use podd::protocol::{
        AgentKind, AgentSpec, CheckJob, CheckJobState, CheckResult, CompareMember, DiffFile,
        GroupCheckEvent, GroupCompareResult, GroupInfo, GroupMember, GroupPickResult, MemberCheck,
        MemberError, TaskRemoveResult,
    };
    use serde_json::json;

    use super::super::AgentCounts;
    use super::super::model::{
        MemberLive, PickBanner, PickOutcome, apply_check_event, card_plan, changed_check_text,
        check_job_ended, check_params, check_running, file_counts, header_plan, job_line,
        member_ids, member_preview, pick_banner, pick_others, pick_toast_body, removed_terminals,
    };
    use crate::orch::tokens;
    use crate::source::TestBackend;

    fn member(task: &str) -> GroupMember {
        GroupMember {
            task: task.into(),
            agent: AgentSpec::Kind(AgentKind::Claude),
            info: None,
            removed: false,
            picked: false,
            check: None,
        }
    }

    fn result(passed: bool) -> CheckResult {
        CheckResult {
            argv: vec!["cmd".into(), "/c".into(), "cargo test".into()],
            passed,
            exit_code: Some(if passed { 0 } else { 1 }),
            timed_out: false,
            duration_ms: 10,
            finished_ms: 1,
            output_tail: String::new(),
            error: None,
        }
    }

    fn job(state: CheckJobState, done: &[&str]) -> CheckJob {
        CheckJob {
            id: "k1".into(),
            state,
            argv: vec!["cmd".into(), "/c".into(), "cargo test".into()],
            tasks: vec!["r/a".into(), "r/b".into()],
            done: done.iter().map(|s| s.to_string()).collect(),
            timeout_ms: 1,
            started_ms: 1,
            finished_ms: None,
            all_passed: None,
        }
    }

    fn group() -> GroupInfo {
        let mut gone = member("r/c");
        gone.removed = true;
        GroupInfo {
            id: "r/fx".into(),
            repo: "r".into(),
            name: "fx".into(),
            base: "main".into(),
            base_commit: "0123456789abcdef".into(),
            prompt: "p".into(),
            check: Some(vec!["cmd".into(), "/c".into(), "cargo test".into()]),
            created_ms: 1,
            picked: None,
            picked_ms: None,
            members: vec![member("r/a"), member("r/b"), gone],
            check_job: None,
            gates: Vec::new(),
        }
    }

    fn compared(task: &str) -> CompareMember {
        CompareMember {
            task: task.into(),
            agent: AgentSpec::Kind(AgentKind::Claude),
            removed: false,
            picked: false,
            status: None,
            files: Vec::new(),
            files_changed: 0,
            added: 0,
            deleted: 0,
            ahead: None,
            dirty: None,
            check: None,
            error: None,
        }
    }

    #[test]
    fn member_preview_and_ids() {
        let two = AgentCounts {
            claude: 1,
            codex: 1,
            ..Default::default()
        };
        assert_eq!(
            member_preview(&two, ""),
            crate::tr!(MEMBER_TASK_S_ONE_WORKTREE_EACH; ; 2, "<name>-claude-1, <name>-codex-1")
        );
        assert!(member_preview(&two, "fx").contains("fx-codex-1"));
        assert_eq!(
            member_preview(&AgentCounts::default(), "fx"),
            crate::strings::NO_MEMBERS_YET
        );
        assert_eq!(member_ids(Some(&group())), ["r/a", "r/b", "r/c"]);
        assert!(member_ids(None).is_empty());
    }

    #[test]
    fn member_live_falls_back_to_the_group_record() {
        let mut g = group();
        // r/b is not in the task poll yet: its terminals come from the group record.
        g.members[1].info = Some(Box::new(
            serde_json::from_value(json!({
                "id": "r/b", "repo": "r", "name": "b", "branch": "pod/b", "base": "main",
                "base_commit": "0", "worktree": "C:/w/b", "agent": "codex", "created_ms": 1,
                "terminals": [{"id": "tb"}], "worktree_exists": true
            }))
            .expect("task info"),
        ));
        let tasks = [TestBackend::task("r/a", "a", "r", &["ta"])];
        let terminals = [
            TestBackend::terminal("ta", "a", Some("r/a"), "working"),
            TestBackend::terminal("tb", "b", Some("r/b"), "blocked"),
        ];
        let live = MemberLive::gather(&g.members, Some((&tasks[..], &terminals[..])));
        assert_eq!(live.tasks.keys().collect::<Vec<_>>(), ["r/a"]);
        let mut badged: Vec<&String> = live.badges.keys().collect();
        badged.sort();
        assert_eq!(badged, ["r/a", "r/b"]);
        assert_eq!((live.summary.working, live.summary.blocked), (1, 1));
        let gone = MemberLive::gather(&g.members, None);
        assert!(gone.tasks.is_empty() && gone.badges.is_empty());
    }

    #[test]
    fn check_requests_store_only_an_edited_command() {
        let stored = vec!["cmd".to_string(), "/c".into(), "cargo test".into()];
        assert_eq!(check_params("r/fx", "  ", None), None);
        let same = check_params("r/fx", "cargo test", Some(&stored)).unwrap();
        assert_eq!(same.check, None);
        assert_eq!(
            serde_json::to_value(&same).unwrap(),
            json!({"group": "r/fx", "wait": false})
        );
        assert_eq!(check_params("r/fx", "", Some(&stored)).unwrap().check, None);
        assert_eq!(
            check_params("r/fx", " cargo test --all ", Some(&stored))
                .unwrap()
                .check,
            Some(vec!["cmd".into(), "/c".into(), "cargo test --all".into()])
        );
        assert_eq!(
            check_params("r/fx", "npm test", None).unwrap().check,
            Some(vec!["cmd".into(), "/c".into(), "npm test".into()])
        );
    }

    #[test]
    fn check_events_reach_both_records() {
        let mut g = group();
        let mut c = GroupCompareResult {
            group: "r/fx".into(),
            base: "main".into(),
            check: None,
            members: vec![compared("r/a"), compared("r/b")],
        };
        let one = GroupCheckEvent {
            seq: 1,
            group: "r/fx".into(),
            job: job(CheckJobState::Running, &["r/b"]),
            member: Some(MemberCheck {
                task: "r/b".into(),
                result: result(false),
            }),
        };
        apply_check_event(Some(&mut g), Some(&mut c), &one);
        assert_eq!(g.check_job, Some(one.job.clone()));
        assert!(check_running(&g));
        assert_eq!(g.members[1].check, Some(result(false)));
        assert_eq!(g.members[0].check, None);
        assert_eq!(c.members[1].check, Some(result(false)));
        assert!(!check_job_ended(&one), "a member's result is not the end");
        // Without a group record the comparison still takes the result.
        let mut c2 = c.clone();
        c2.members[1].check = None;
        apply_check_event(None, Some(&mut c2), &one);
        assert_eq!(c2.members[1].check, Some(result(false)));

        let end = GroupCheckEvent {
            member: None,
            job: job(CheckJobState::Finished, &["r/a", "r/b"]),
            ..one.clone()
        };
        apply_check_event(Some(&mut g), Some(&mut c), &end);
        assert!(check_job_ended(&end));
        assert!(!check_running(&g));
        let still = GroupCheckEvent {
            member: None,
            ..one
        };
        assert!(!check_job_ended(&still), "running without a member");
    }

    #[test]
    fn a_reread_shows_the_check_command_only_when_it_changed() {
        let g = group();
        assert_eq!(changed_check_text(None, &g).as_deref(), Some("cargo test"));
        assert_eq!(changed_check_text(Some(&g), &g), None);
        let mut cleared = g.clone();
        cleared.check = None;
        assert_eq!(changed_check_text(Some(&g), &cleared), None);
        let mut other = g.clone();
        other.check = Some(vec!["npm".into(), "test".into()]);
        assert_eq!(
            changed_check_text(Some(&g), &other).as_deref(),
            Some("npm test")
        );
    }

    #[test]
    fn job_line_counts_finished_members_only() {
        let t = tokens();
        let mut g = group();
        g.members[0].check = Some(result(true));
        g.members[1].check = Some(result(false));
        let (passed, failed, line) = (1, 0, "cargo test");
        // r/b has an old result but is not done in this job: not counted.
        let running = job(CheckJobState::Running, &["r/a"]);
        assert_eq!(
            job_line(None, Some(&g), Some(&running)),
            (
                crate::tr!(CHECK_PROGRESS_REPORT; line,passed,failed; "k1", 1, 2),
                t.info_fg
            )
        );
        let mut done = job(CheckJobState::Finished, &["r/a", "r/b"]);
        let (passed, failed) = (1, 1);
        assert_eq!(
            job_line(None, Some(&g), Some(&done)),
            (
                crate::tr!(CHECK_FAILED_REPORT; line,failed,passed; "k1"),
                t.danger_fg
            )
        );
        done.all_passed = Some(true);
        assert_eq!(job_line(None, Some(&g), Some(&done)).1, t.success_fg);
        assert_eq!(
            job_line(None, Some(&g), Some(&job(CheckJobState::Cancelled, &[]))).1,
            t.warning_fg
        );
        assert_eq!(
            job_line(None, None, Some(&job(CheckJobState::Interrupted, &[]))).1,
            t.warning_fg
        );
        assert_eq!(
            job_line(None, Some(&g), None),
            (crate::strings::NO_CHECK_RUN_YET.to_string(), t.text_muted)
        );
        let error = "INVALID_STATE: busy".to_string();
        assert_eq!(
            job_line(Some(&error), Some(&g), Some(&running)),
            (error.clone(), t.danger_fg),
            "the last request error wins"
        );
    }

    #[test]
    fn pick_plans() {
        let removed = |id: &str, terms: &[&str]| TaskRemoveResult {
            id: id.into(),
            terminals: terms.iter().map(|s| s.to_string()).collect(),
            worktree_removed: true,
            branch_deleted: false,
            branch_kept: None,
        };
        let res = GroupPickResult {
            group: "r/fx".into(),
            picked: "r/a".into(),
            removed: vec![removed("r/b", &["tb", "tb2"])],
            kept: vec![MemberError {
                task: "r/c".into(),
                code: 1,
                message: "dirty".into(),
            }],
        };
        assert_eq!(removed_terminals(&res), ["tb", "tb2"]);
        assert_eq!(pick_toast_body(&res), crate::tr!(REMOVED; ; "r/b"));
        let none = GroupPickResult {
            removed: Vec::new(),
            kept: Vec::new(),
            ..res.clone()
        };
        assert_eq!(pick_toast_body(&none), crate::strings::NO_MEMBERS_REMOVED);

        assert_eq!(pick_banner(None), PickBanner::None);
        let m = "TASK_DIRTY: dirty".to_string();
        assert_eq!(
            pick_banner(Some(&PickOutcome::Failed(m.clone()))),
            PickBanner::Refused(crate::tr!(PICK_REFUSED_M; m;))
        );
        assert_eq!(
            pick_banner(Some(&PickOutcome::Done(res))),
            PickBanner::Done {
                line: crate::tr!(PICK_RESULT; ; "r/a", crate::tr!(REMOVED_269; ; "r/b")),
                kept: vec![crate::tr!(KEPT; ; "r/c", "dirty")],
            }
        );
        assert_eq!(
            pick_banner(Some(&PickOutcome::Done(none))),
            PickBanner::Done {
                line: crate::tr!(PICK_RESULT; ; "r/a", ""),
                kept: Vec::new(),
            }
        );

        // The pick dialog offers every other member still there.
        assert_eq!(pick_others(Some(&group()), "r/a"), ["r/b"]);
        assert!(pick_others(None, "r/a").is_empty());
    }

    #[test]
    fn header_and_card_plans() {
        let t = tokens();
        let mut g = group();
        g.picked = Some("r/b".into());
        g.check_job = Some(job(CheckJobState::Running, &["r/a"]));
        let live = MemberLive::gather(&g.members, None);
        let h = header_plan("r/fx", Some(&g), &live.summary);
        assert_eq!(h.title, crate::tr!(GROUP; ; "r/fx"));
        let (base, members) = (crate::tr!(BASE_253; ; "main", "012345678"), 2);
        assert_eq!(
            h.subtitle,
            crate::tr!(BASE_MEMBERS_MEMBER_S; base,members; crate::orch::summary_short(&live.summary))
        );
        assert_eq!(h.winner.as_deref(), Some("r/b"));
        assert_eq!(h.prompt, crate::tr!(PROMPT_257; ; "p"));
        assert!(h.running);
        let empty = header_plan("r/fx", None, &live.summary);
        assert!(empty.winner.is_none() && !empty.running);

        // r/a: done in the running job and failed → red; r/b: the winner → green;
        // a member the job still runs on → plain, whatever its old result.
        g.members[0].check = Some(result(false));
        let a = card_plan(&g.members[0], Some(&g), None, &live);
        assert_eq!(
            (a.short.as_str(), a.checking, a.picked),
            ("a", false, false)
        );
        assert_eq!(a.border, t.danger_fg);
        assert_eq!(a.branch, "—");
        let b = card_plan(&g.members[1], Some(&g), None, &live);
        assert!(b.picked && b.checking);
        assert_eq!(b.border, t.success_fg);
        g.picked = None;
        g.members[1].check = Some(result(false));
        let b = card_plan(&g.members[1], Some(&g), None, &live);
        assert!(b.checking);
        assert_eq!(b.border, t.border_subtle);
        // The comparison's result wins over the group record's.
        let mut cmp = compared("r/b");
        cmp.check = Some(result(true));
        g.check_job = None;
        let b = card_plan(&g.members[1], Some(&g), Some(&cmp), &live);
        assert_eq!(b.check, Some(&result(true)));
        assert_eq!(b.border, t.border_subtle);
        // Branch: the task poll, else the group record.
        let tasks = [TestBackend::task("r/a", "a", "r", &[])];
        let live = MemberLive::gather(&g.members, Some((&tasks[..], &[])));
        assert_eq!(
            card_plan(&g.members[0], Some(&g), None, &live).branch,
            "pod/a"
        );
    }

    #[test]
    fn file_counts_mark_binary_files() {
        let mut f = DiffFile {
            path: "a".into(),
            old_path: None,
            status: "M".into(),
            added: Some(3),
            deleted: None,
            binary: false,
        };
        assert_eq!(file_counts(&f), "+3 −0");
        f.binary = true;
        assert_eq!(file_counts(&f), "bin");
    }
}

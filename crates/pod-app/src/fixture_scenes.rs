//! Editor QA scenes for `--ui-fixture --fixture-scene <name>` (docs/13 §8,
//! tools/visual). The read-only fixture has no daemon, so task
//! `sample/task-000` gets a small generated git repository as its worktree
//! (or `POD_APP_FIXTURE_WORKTREE`, used by the restore check to reuse one).
//!
//! Visual-baseline scenes (`editor`, …) run without language servers so the
//! capture is deterministic; `editor-lsp`, `editor-hover` and
//! `editor-completion` start rust-analyzer against that small repository only.

use std::path::Path;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use gpui_kit::component::input::{RopeExt as _, ShowCompletions};
use gpui_kit::*;
use pod_editor::EditorWorkspace;
use pod_editor::lsp::ServerState;

use podd::protocol::{GateInfo, GateState};

use crate::vm::{AgentState, Liveness};
use crate::workspace::{ToastKind, Workspace};

pub const TASK: &str = "sample/task-000";
static WORKTREE: OnceLock<Option<String>> = OnceLock::new();
static NO_LSP: AtomicBool = AtomicBool::new(false);
static LSP_MAX: AtomicUsize = AtomicUsize::new(0);

/// Server cap override for the `editor-lsp-cap` scene (0: none).
pub fn lsp_max() -> usize {
    LSP_MAX.load(Ordering::Relaxed)
}

pub fn is_editor_scene(s: &str) -> bool {
    s.starts_with("editor") || s.starts_with("review-editor") || s.starts_with("pr-")
}

/// Worktree of the fixture's task-000 (editor scenes / restore check only).
pub fn worktree() -> Option<String> {
    WORKTREE.get().cloned().flatten()
}

/// Fixture scenes that must not start language servers.
pub fn no_lsp() -> bool {
    NO_LSP.load(Ordering::Relaxed)
}

/// Runs before the fixture source starts.
pub fn prepare(scene: Option<&str>) {
    // The dense layout's log prints wall-clock times: one fixed "now" in every fixture run.
    crate::layouts::dense::freeze_fixture_clock();
    if let Some(s) = scene.filter(|s| crate::pr_panel::SCENES.contains(s)) {
        crate::pr_panel::set_fixture_scene(s);
    }
    let given = std::env::var("POD_APP_FIXTURE_WORKTREE")
        .ok()
        .filter(|w| Path::new(w).is_dir());
    let wt = given.or_else(|| {
        let scene = scene.filter(|s| is_editor_scene(s))?;
        let _ = scene;
        let dir = crate::layout::config_dir().join("editor-fixture");
        let _ = std::fs::remove_dir_all(&dir);
        pod_editor::fixture::create(&dir, true)
            .ok()
            .map(|p| p.to_string_lossy().into_owned())
    });
    // Cap check without rust-analyzer: the test-only fake server (built next
    // to pod-app) stands in for Rust and Python, and only one may run.
    if scene == Some("editor-lsp-cap")
        && let Some(wt) = &wt
        && let Some(fake) = std::env::current_exe()
            .ok()
            .and_then(|e| e.parent().map(|d| d.join("pod-editor-fake-lsp.exe")))
            .filter(|f| f.is_file())
    {
        let cmd = fake.to_string_lossy().replace('\\', "/");
        let cfg = serde_json::json!({"servers": [
            // Same ids replace the defaults: no real rust-analyzer/pyright here.
            {"id": "rust-analyzer", "languages": ["rust"], "command": cmd, "root_markers": ["Cargo.toml"], "root_strategy": "outermost"},
            {"id": "pyright", "languages": ["python"], "command": cmd},
        ]});
        let dir = Path::new(wt).join(".pod");
        let _ = std::fs::create_dir_all(&dir);
        let _ = std::fs::write(dir.join("lsp.json"), cfg.to_string());
        // A repository lsp.json runs only once approved (editor-01): approve
        // the fixture's in a fixture-only editor config folder.
        let user = crate::layout::config_dir().join("editor-config");
        // SAFETY: fixture setup runs before any editor (or other thread) reads it.
        unsafe { std::env::set_var("POD_EDITOR_CONFIG_DIR", &user) };
        if let Some(repo) = pod_editor::lsp::registry::ServerRegistry::load_with(
            &pod_editor::uri::simplify(&Path::new(wt).canonicalize().unwrap_or_else(|_| wt.into())),
            Some(&user),
        )
        .2
        {
            let _ = repo.trust(&user);
        }
        LSP_MAX.store(1, Ordering::Relaxed);
    }
    let _ = WORKTREE.set(wt);
    let lsp_scene = matches!(
        scene,
        Some("editor-lsp" | "editor-hover" | "editor-completion" | "editor-lsp-cap")
    );
    NO_LSP.store(
        !lsp_scene && std::env::var_os("POD_APP_FIXTURE_LSP").is_none(),
        Ordering::Relaxed,
    );
}

fn editor(w: &Workspace, cx: &App) -> Option<Entity<EditorWorkspace>> {
    w.live_editors()
        .into_iter()
        .find(|e| e.read(cx).task_id == TASK)
        .map(|e| e.read(cx).editor.clone())
}

fn open(w: &mut Workspace, rel: &str, line: u32, window: &mut Window, cx: &mut Context<Workspace>) {
    w.open_editor(TASK, Some((rel.to_string(), line, 1)), false, window, cx);
}

/// Moves the cursor to the first `needle` (+`delta` chars); returns its screen point.
fn cursor_to(
    ed: &Entity<EditorWorkspace>,
    needle: &str,
    delta: usize,
    window: &mut Window,
    cx: &mut App,
) -> Option<Point<Pixels>> {
    let st = ed.read(cx).active_editor()?;
    let text = st.read(cx).value().to_string();
    let off = text.find(needle)? + delta;
    let pos = st.read(cx).text().offset_to_position(off);
    st.update(cx, |s, cx| s.set_cursor_position(pos, window, cx));
    st.read(cx)
        .range_to_bounds(&(off..off + 1))
        .map(|b| b.center())
}

fn server_ready(ed: &Entity<EditorWorkspace>, cx: &App) -> bool {
    let e = ed.read(cx);
    let Some(p) = e.active_path() else {
        return false;
    };
    match e.lsp().read(cx).status_for(p) {
        Some(s) => {
            (s.state.is_running() && s.progress.is_none())
                || matches!(
                    s.state,
                    ServerState::NotInstalled(..) | ServerState::Failed(_)
                )
        }
        None => false,
    }
}

pub fn run(scene: &str, w: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    match scene {
        "editor" => {
            open(w, "src/lib.rs", 1, window, cx);
            open(w, "src/main.rs", 5, window, cx);
        }
        "editor-tree" => {
            open(w, "scripts/tool.py", 1, window, cx);
            open(w, "src/main.rs", 1, window, cx);
            window.dispatch_action(Box::new(pod_editor::FocusTree), cx);
        }
        "editor-quickopen" => {
            open(w, "src/lib.rs", 1, window, cx);
            spawn_after(window, cx, 1200, |w, window, cx| {
                if let Some(ed) = editor(w, cx) {
                    ed.update(cx, |e, cx| e.quick_open_query("geo", window, cx));
                }
            });
        }
        // Tree file operation prompt (right-click > 새 파일… on src/).
        "editor-new-file" => {
            open(w, "src/lib.rs", 1, window, cx);
            if let Some(ed) = editor(w, cx) {
                let src = ed.read(cx).root().join("src");
                ed.update(cx, |e, cx| e.start_new_entry(src, false, window, cx));
            }
        }
        // Unsaved edits; the QA script then closes the window (prompt).
        "editor-dirty" => {
            open(w, "src/lib.rs", 1, window, cx);
            if let Some(st) = editor(w, cx).and_then(|e| e.read(cx).active_editor()) {
                st.update(cx, |s, cx| {
                    s.insert(
                        "// 저장하지 않은 변경
",
                        window,
                        cx,
                    )
                });
            }
        }
        "editor-close" => {
            open(w, "src/lib.rs", 1, window, cx);
            if let Some(st) = editor(w, cx).and_then(|e| e.read(cx).active_editor()) {
                st.update(cx, |s, cx| s.insert("// 저장하지 않은 변경\n", window, cx));
            }
            spawn_after(window, cx, 300, |_, window, cx| {
                window.dispatch_action(Box::new(pod_editor::CloseTab), cx)
            });
        }
        "editor-agent-edit" => {
            // An agent rewrites a clean file (README: reloaded silently) and
            // a file with unsaved edits (lib.rs: prompt banner).
            open(w, "README.md", 1, window, cx);
            open(w, "src/lib.rs", 1, window, cx);
            if let Some(st) = editor(w, cx).and_then(|e| e.read(cx).active_editor()) {
                st.update(cx, |s, cx| {
                    s.insert("// 내 변경 (저장 안 함)\n", window, cx)
                });
            }
            spawn_after(window, cx, 800, |w, _, cx| {
                if let Some(root) = editor(w, cx).map(|e| e.read(cx).root().to_path_buf()) {
                    for (rel, line) in [
                        ("README.md", "\n에이전트가 추가한 줄\n"),
                        (
                            "src/lib.rs",
                            "\n// agent: 에이전트가 추가한 함수\npub fn added_by_agent() {}\n",
                        ),
                    ] {
                        let p = root.join(rel);
                        let mut s = std::fs::read_to_string(&p).unwrap_or_default();
                        s.push_str(line);
                        let _ = std::fs::write(&p, s);
                    }
                }
            });
        }
        // Tab strip (docs/13 §9): pinned README, lib.rs with unsaved edits
        // that an agent then changes on disk (warning mark), and main.rs
        // opened as the preview tab (italic, active).
        "editor-tabs" => {
            open(w, "README.md", 1, window, cx);
            open(w, "src/lib.rs", 1, window, cx);
            if let Some(st) = editor(w, cx).and_then(|e| e.read(cx).active_editor()) {
                st.update(cx, |s, cx| s.insert("// 저장하지 않은 변경\n", window, cx));
            }
            spawn_after(window, cx, 500, |w, _, cx| {
                if let Some(root) = editor(w, cx).map(|e| e.read(cx).root().to_path_buf()) {
                    let p = root.join("src/lib.rs");
                    let mut s = std::fs::read_to_string(&p).unwrap_or_default();
                    s.push_str("\n// agent: 에이전트가 추가한 줄\n");
                    let _ = std::fs::write(&p, s);
                }
            });
            spawn_after(window, cx, 1300, |w, window, cx| {
                if let Some(ed) = editor(w, cx) {
                    let p = ed.read(cx).root().join("src/main.rs");
                    let _ = ed.update(cx, |e, cx| e.preview_file(&p, window, cx));
                }
            });
        }
        // Tab strip follow-up (docs/13 §9): many tabs overflow the strip
        // (scrolled to the active one, open-files button), two `mod.rs`
        // tell their folders apart; `-menu` opens the active tab's menu as
        // Shift+F10 does, `-close` asks once for two unsaved files
        // ("모두 닫기").
        "editor-tabs-overflow" | "editor-tabs-menu" | "editor-tabs-close" => {
            if let Some(root) = worktree() {
                for d in ["crates/podd/src", "crates/pod-app/src"] {
                    let dir = Path::new(&root).join(d);
                    let _ = std::fs::create_dir_all(&dir);
                    let _ = std::fs::write(dir.join("mod.rs"), "//! fixture\n");
                }
            }
            for rel in [
                "README.md",
                "Cargo.toml",
                "src/lib.rs",
                "src/geometry.rs",
                "crates/podd/src/mod.rs",
                "crates/pod-app/src/mod.rs",
                "scripts/tool.py",
                "config/settings.json",
                "notes/todo.md",
            ] {
                open(w, rel, 1, window, cx);
            }
            if let Some(ed) = editor(w, cx) {
                // Back to a tab in the middle: the strip scrolls to it.
                let p = ed.read(cx).root().join("crates/podd/src/mod.rs");
                let _ = ed.update(cx, |e, cx| e.open_file(&p, window, cx));
            }
            let scene = scene.to_string();
            if scene == "editor-tabs-close" {
                for rel in ["src/lib.rs", "crates/podd/src/mod.rs"] {
                    open(w, rel, 1, window, cx);
                    if let Some(st) = editor(w, cx).and_then(|e| e.read(cx).active_editor()) {
                        st.update(cx, |s, cx| s.insert("// 저장하지 않은 변경\n", window, cx));
                    }
                }
            }
            spawn_after(window, cx, 900, move |w, window, cx| {
                let Some(ed) = editor(w, cx) else { return };
                ed.update(cx, |e, cx| match scene.as_str() {
                    "editor-tabs-menu" => e.open_tab_menu(None, window, cx),
                    "editor-tabs-close" => {
                        if let Some(ix) = e.tab_states().iter().position(|t| t.active) {
                            e.run_tab_command(ix, pod_editor::TabCommand::CloseAll, window, cx);
                        }
                    }
                    _ => {}
                });
            });
        }
        "editor-lsp-cap" => {
            // main.rs starts the fake "rust-analyzer"; tool.py starts the fake
            // "pyright", which suspends the first (cap 1); back on main.rs it
            // resumes and "pyright" is suspended.
            open(w, "src/main.rs", 1, window, cx);
            spawn_after(window, cx, 700, |w, window, cx| {
                open(w, "scripts/tool.py", 1, window, cx)
            });
            spawn_after(window, cx, 1400, |w, window, cx| {
                open(w, "src/main.rs", 1, window, cx)
            });
        }
        "editor-lsp" | "editor-hover" | "editor-completion" => {
            open(w, "src/main.rs", 1, window, cx);
            let scene = scene.to_string();
            cx.spawn_in(window, async move |this, cx| {
                let t = Instant::now();
                while t.elapsed() < Duration::from_secs(120) {
                    let ready = this
                        .update(cx, |w, cx| {
                            editor(w, cx).is_some_and(|e| server_ready(&e, cx))
                        })
                        .unwrap_or(true);
                    if ready {
                        break;
                    }
                    cx.background_executor()
                        .timer(Duration::from_millis(500))
                        .await;
                }
                cx.background_executor()
                    .timer(Duration::from_millis(3000))
                    .await;
                let target = this
                    .update_in(cx, |w, window, cx| {
                        let ed = editor(w, cx)?;
                        match scene.as_str() {
                            "editor-lsp" => {
                                ed.update(cx, |e, cx| e.show_panel(false, cx));
                                cursor_to(&ed, "word_count(&message, 2)", 0, window, cx);
                                None
                            }
                            "editor-hover" => cursor_to(&ed, "greet(\"Pod\")", 1, window, cx),
                            _ => {
                                let st = ed.read(cx).active_editor()?;
                                let text = st.read(cx).value().to_string();
                                let off = text.find("    task.finish();")? + 4;
                                st.update(cx, |s, cx| {
                                    let pos = s.text().offset_to_position(off);
                                    s.set_cursor_position(pos, window, cx);
                                    s.insert("task.fi\n    ", window, cx);
                                    let p2 = s.text().offset_to_position(off + "task.fi".len());
                                    s.set_cursor_position(p2, window, cx);
                                });
                                None
                            }
                        }
                    })
                    .ok()
                    .flatten();
                if let Some(p) = target {
                    for dx in [0., 1., 2., 3., 2., 1., 0.] {
                        let _ = cx.update(|window, cx| {
                            window.dispatch_event(
                                PlatformInput::MouseMove(MouseMoveEvent {
                                    position: point(p.x + px(dx), p.y),
                                    pressed_button: None,
                                    modifiers: Modifiers::default(),
                                }),
                                cx,
                            );
                        });
                        cx.background_executor()
                            .timer(Duration::from_millis(700))
                            .await;
                    }
                }
                if scene == "editor-completion" {
                    cx.background_executor()
                        .timer(Duration::from_millis(600))
                        .await;
                    let _ = cx
                        .update(|window, cx| window.dispatch_action(Box::new(ShowCompletions), cx));
                }
            })
            .detach();
        }
        // Terminal links (docs/09 §12): compiler output in task-000's
        // terminal; `-links` opens the keyboard picker, `-hover` shows the
        // Ctrl-hover underline on the first file link.
        "editor-terminal-links" | "editor-terminal-hover" => {
            w.open_terminal("fixture-0", false, window, cx);
            let hover = scene == "editor-terminal-hover";
            spawn_after(window, cx, 400, move |w, window, cx| {
                let Some(p) = w
                    .live_panels()
                    .into_iter()
                    .find(|p| p.read(cx).terminal_id == "fixture-0")
                else {
                    return;
                };
                let view = p.read(cx).view.clone();
                view.update(cx, |v, cx| v.feed(LINK_OUTPUT.as_bytes(), cx));
                if hover {
                    spawn_after(window, cx, 300, move |_, _, cx| {
                        view.update(cx, |v, cx| {
                            if let Some(l) = v.visible_links().into_iter().next() {
                                let row = (l.start.line - v.abs_line_of_row(0)).max(0) as u16;
                                v.hover_link_at(l.start.col + 2, row, cx);
                            }
                        });
                    });
                } else {
                    spawn_after(window, cx, 300, |w, window, cx| {
                        w.open_link_picker(window, cx)
                    });
                }
            });
        }
        // Wide characters (Korean, CJK, emoji) in a terminal: glyph spacing,
        // a selection ending on a wide char, the block cursor on one.
        "editor-terminal-wide" => {
            w.open_terminal("fixture-0", false, window, cx);
            spawn_after(window, cx, 400, move |w, _, cx| {
                let Some(p) = w
                    .live_panels()
                    .into_iter()
                    .find(|p| p.read(cx).terminal_id == "fixture-0")
                else {
                    return;
                };
                let view = p.read(cx).view.clone();
                view.update(cx, |v, cx| {
                    v.feed(WIDE_OUTPUT.as_bytes(), cx);
                    // "선택 영역" row: drag from 영 to the first half of 역.
                    let line = v.abs_line_of_row(2);
                    let sel = pod_terminal_view::selection::Selection {
                        mode: pod_terminal_view::selection::SelMode::Simple,
                        anchor: pod_terminal_view::selection::SelPoint::new(line, 5),
                        head: pod_terminal_view::selection::SelPoint::new(line, 7),
                    };
                    v.set_selection(Some(sel), cx);
                });
            });
        }
        // Japanese/Korean/Chinese, half-width kana, emoji and box drawing in a
        // terminal: wide glyph size and placement against an ASCII ruler.
        "editor-terminal-cjk" => {
            w.open_terminal("fixture-0", false, window, cx);
            spawn_after(window, cx, 400, move |w, _, cx| {
                let Some(p) = w
                    .live_panels()
                    .into_iter()
                    .find(|p| p.read(cx).terminal_id == "fixture-0")
                else {
                    return;
                };
                let view = p.read(cx).view.clone();
                view.update(cx, |v, cx| v.feed(CJK_OUTPUT.as_bytes(), cx));
            });
        }
        // Status-bar language server menu: turned off, then 로그 보기.
        "editor-lsp-log" => {
            open(w, "src/main.rs", 5, window, cx);
            spawn_after(window, cx, 500, |w, window, cx| {
                w.lsp_turn_off(cx);
                w.lsp_show_log(window, cx);
            });
        }
        // Review with the "편집기에서 열기" header buttons; `-open` then
        // follows the first file's button (src/main.rs at its first change).
        "review-editor" | "review-editor-open" => {
            w.open_review(TASK, window, cx);
            if scene == "review-editor-open" {
                spawn_after(window, cx, 900, |w, window, cx| {
                    if let Some(r) = w
                        .live_reviews()
                        .into_iter()
                        .find(|r| r.read(cx).task_id == TASK)
                    {
                        r.update(cx, |r, cx| {
                            r.open_in_editor("src/main.rs".into(), None, window, cx)
                        });
                    }
                });
            }
        }
        // P6 PR section states from the fake forge (docs/15).
        s if s.starts_with("pr-") => crate::pr_panel::run_scene(s, w, window, cx),
        _ => {}
    }
}

/// Compiler-style output with file references (fixture repository) and a URL.
const LINK_OUTPUT: &str = concat!(
    "PS C:\\fixtures\\sample> cargo build\r\n",
    "\x1b[1;31merror[E0425]\x1b[0m: cannot find value `widht` in this scope\r\n",
    "  \x1b[34m-->\x1b[0m src/geometry.rs:4:9\r\n",
    "warning: unused variable: `x`\r\n",
    " --> src/main.rs:16:5\r\n",
    "src\\lib.rs(3,1): note: MSVC 형식 경로\r\n",
    "missing/file.rs:9 (없는 파일은 링크가 아님)\r\n",
    "문서: https://doc.rust-lang.org/error_codes/E0425.html\r\n",
    "PS C:\\fixtures\\sample> ",
);

/// Wide characters for `editor-terminal-wide`; the caret ends on 다 (a wide cell).
const WIDE_OUTPUT: &str = concat!(
    "PS C:\\fixtures\\sample> type 문서.txt\r\n",
    "문서: 한글 가나다라마바사 ABC abc 123\r\n",
    "선택 영역 확인 (마우스)\r\n",
    "漢字 かな カナ 全角ＡＢＣ\r\n",
    "emoji 😀👍🎉 end\r\n",
    "src\\lib.rs(3,1): note: MSVC 형식 경로\r\n",
    "PS C:\\fixtures\\sample> 가나다\x1b[2D",
);

/// Lines for `editor-terminal-cjk`; `tools/visual/cjk-compare.ps1` prints the
/// same lines in Windows Terminal for a side-by-side check.
const CJK_OUTPUT: &str = concat!(
    "0123456789012345678901234567890123456789\r\n",
    "ひらがな あいうえお かきくけこ さしすせそ\r\n",
    "カタカナ アイウエオ カキクケコ サシスセソ\r\n",
    "半角 ｶﾀｶﾅ ｱｲｳｴｵ ｶｷｸｹｺ ﾊﾟﾋﾟ\r\n",
    "漢字 日本語 東京 中文 简体字 繁體字\r\n",
    "한글 가나다라마바사 문서 확인\r\n",
    "emoji 😀👍🎉 end\r\n",
    "┌──┬──┬──┐ |あ|ア|가|漢|\r\n",
    "│あ│가│漢│ |い|イ|나|字|\r\n",
    "└──┴──┴──┘ 0123456789\r\n",
    "PS C:\\fixtures\\sample> ",
);

fn spawn_after(
    window: &mut Window,
    cx: &mut Context<Workspace>,
    ms: u64,
    f: impl FnOnce(&mut Workspace, &mut Window, &mut Context<Workspace>) + 'static,
) {
    cx.spawn_in(window, async move |this, cx| {
        cx.background_executor()
            .timer(Duration::from_millis(ms))
            .await;
        let _ = this.update_in(cx, f);
    })
    .detach();
}

// ───────────────────────── layout scenes (visual QA) ─────────────────────────
//
// `rail…`, `queue` and `dense` set fixture data up in the workspace (a gate
// snapshot, terminal statuses, a toast, the dock), which no layout reaches
// through the kit, and then hand the layout's own part a `LayoutActions` (and
// a frame) so the layout files never name the workspace (card T2).

/// The pending approval on the blocked fixture-1 that the queue and dense
/// scenes show, asked `now - 12 s`.
fn fixture_gate(now: u64) -> GateInfo {
    GateInfo {
        id: "g1".into(),
        title: "명령 실행 승인".into(),
        message: "$ cargo clippy -p pod-ui -- -D warnings".into(),
        task: Some("sample/task-000".into()),
        group: None,
        terminal: Some("fixture-1".into()),
        state: GateState::Pending,
        created_ms: now.saturating_sub(12_000),
        resolved_ms: None,
        note: None,
        expires_ms: None,
        run: None,
        item: None,
        options: Vec::new(),
        choice: None,
        requested_by: None,
        decided_by: None,
    }
}

/// `--fixture-scene rail[-attention|-switcher|-workspace]`: task-000's three
/// terminals laid out like the gallery mockup, two more task tabs, and the
/// named drawer open.
pub fn rail(scene: &str, w: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    for task in ["sample/task-002", "sample/task-001", "sample/task-000"] {
        let ids: Vec<String> = w
            .task(task)
            .map(|t| t.terminals.clone())
            .unwrap_or_default();
        for id in ids.iter().rev() {
            w.open_terminal(id, false, window, cx);
        }
    }
    crate::layouts::rail::fixture_scene(scene, w.layout_actions(cx), window, cx);
}

/// `--fixture-scene queue`: a pending gate on the blocked fixture-1, a
/// question on fixture-3, four panels in a grid, the gate card selected.
pub fn queue(w: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    w.orch
        .merge_snapshot(vec![fixture_gate(crate::orch::now_ms())]);
    if let Some(s) = w
        .terminals
        .iter_mut()
        .find(|t| t.id == "fixture-3")
        .and_then(|t| t.status.as_mut())
    {
        s.kind = Some("question".into());
    }
    for id in ["fixture-0", "fixture-1", "fixture-2", "fixture-3"] {
        w.open_terminal(id, false, window, cx);
    }
    w.arrange_grid(window, cx);
    w.touch(cx);
    crate::layouts::queue::fixture_scene(&w.layout_actions(cx), window, cx);
}

/// `--fixture-scene dense`: a pending gate on the blocked fixture-1, fixture-3
/// asks a question, four panels in a grid, fixture-1 in the details column,
/// a few status changes and a notification in the log.
pub fn dense(w: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    for id in ["fixture-0", "fixture-1", "fixture-2", "fixture-3"] {
        w.open_terminal(id, false, window, cx);
    }
    w.arrange_grid(window, cx);
    // The details column follows the focused terminal: end on fixture-1.
    w.open_terminal("fixture-1", false, window, cx);
    let actions = w.layout_actions(cx);
    let (view, now) = {
        let pending = w.orch.pending();
        let frame = w.layout_frame(&pending, cx);
        crate::layouts::dense::fixture_view(&actions, &frame, window, cx)
    };
    // Live changes after the log started.
    w.orch.merge_snapshot(vec![fixture_gate(now)]);
    if let Some(s) = w
        .terminals
        .iter_mut()
        .find(|t| t.id == "fixture-3")
        .and_then(|t| t.status.as_mut())
    {
        s.state = AgentState::Blocked;
        s.kind = Some("question".into());
    }
    if let Some(t) = w.terminals.iter_mut().find(|t| t.id == "fixture-2") {
        t.live = Liveness::Exited {
            code: Some(1),
            killed: false,
        };
    }
    w.raise_toast(
        ToastKind::Success,
        "Worker 2 · 작업 완료",
        "1분 12초 · 토큰 18.2k",
        cx,
    );
    {
        let pending = w.orch.pending();
        let frame = w.layout_frame(&pending, cx);
        crate::layouts::dense::fixture_select(&view, &frame, cx);
    }
    w.touch(cx);
}

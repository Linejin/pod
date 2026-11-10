//! `pod-editor-demo <path>`: the editor view alone in a window on the
//! secondary monitor.
//!
//! Options: `--light`, `--primary` (main monitor), `--duration N` (quit
//! after N s), `--fixture DIR` (create the demo repository in DIR and open
//! it), `--open REL`, `--scenario NAME` (scripted state for screenshots:
//! tree | quickopen | editor | notinstalled | diagnostics | hover | completion | definition).
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::path::PathBuf;
use std::time::{Duration, Instant};

use gpui_kit::component::input::{GoToDefinition, RopeExt as _, ShowCompletions};
use gpui_kit::*;
use pod_editor::EditorWorkspace;
use pod_editor::lsp::ServerState;
use pod_ui::{Mode, apply_theme};

struct Args {
    root: Option<PathBuf>,
    fixture: Option<PathBuf>,
    light: bool,
    primary: bool,
    duration: Option<u64>,
    open: Option<String>,
    scenario: Option<String>,
}

fn args() -> Args {
    let v: Vec<String> = std::env::args().skip(1).collect();
    let val = |k: &str| v.windows(2).find(|w| w[0] == k).map(|w| w[1].clone());
    let flags = ["--duration", "--fixture", "--open", "--scenario"];
    let root = v
        .iter()
        .enumerate()
        .find(|(i, a)| !a.starts_with("--") && (*i == 0 || !flags.contains(&v[i - 1].as_str())))
        .map(|(_, a)| PathBuf::from(a));
    Args {
        root,
        fixture: val("--fixture").map(PathBuf::from),
        light: v.iter().any(|a| a == "--light"),
        primary: v.iter().any(|a| a == "--primary"),
        duration: val("--duration").and_then(|d| d.parse().ok()),
        open: val("--open"),
        scenario: val("--scenario"),
    }
}

fn main() {
    let a = args();
    let root = match (&a.fixture, &a.root) {
        (Some(dir), _) => {
            if dir.exists() {
                let _ = std::fs::remove_dir_all(dir);
            }
            pod_editor::fixture::create(dir, true).expect("create fixture")
        }
        (None, Some(r)) => r.clone(),
        (None, None) => std::env::current_dir().expect("cwd"),
    };
    let mode = if a.light { Mode::Light } else { Mode::Dark };
    gpui_kit::application().run(move |cx| {
        gpui_kit::init(cx);
        apply_theme(mode, true, cx);
        pod_editor::init(cx);
        let main = cx.primary_display().map(|d| d.id());
        let display_id = if a.primary {
            main
        } else {
            cx.displays()
                .iter()
                .map(|d| d.id())
                .find(|id| Some(*id) != main)
                .or(main)
        };
        let bounds = Bounds::centered(display_id, size(px(1440.), px(900.)), cx);
        let opts = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            display_id,
            titlebar: Some(TitlebarOptions {
                title: Some("Pod 편집기 데모".into()),
                ..Default::default()
            }),
            ..Default::default()
        };
        let open = a.open.clone();
        let scenario = a.scenario.clone();
        let (window, ws) = gpui_kit::open_window(opts, cx, move |window, cx| {
            cx.new(|cx| EditorWorkspace::new(root, mode, window, cx))
        })
        .expect("open window");
        let _ = window.update(cx, |_, window, cx| {
            ws.update(cx, |w, cx| {
                if let Some(rel) = &open {
                    let _ = w.open_file(std::path::Path::new(rel), window, cx);
                }
            });
            if let Some(s) = scenario {
                run_scenario(s, ws.clone(), window, cx);
            }
        });
        // Stop language servers before quitting.
        let ws2 = ws.clone();
        cx.on_app_quit(move |cx| ws2.update(cx, |w, cx| w.shutdown(cx)))
            .detach();
        if let Some(secs) = a.duration {
            cx.spawn(async move |cx| {
                cx.background_executor()
                    .timer(Duration::from_secs(secs))
                    .await;
                cx.update(|cx| cx.quit());
            })
            .detach();
        }
        cx.activate(true);
    });
}

fn wait_for_server(ws: &Entity<EditorWorkspace>, cx: &App) -> bool {
    let w = ws.read(cx);
    let Some(p) = w.active_path() else {
        return false;
    };
    match w.lsp().read(cx).status_for(p) {
        Some(s) => {
            matches!(s.state, ServerState::Running) && s.progress.is_none()
                || matches!(
                    s.state,
                    ServerState::NotInstalled(..) | ServerState::Failed(_)
                )
        }
        None => true,
    }
}

/// Moves the cursor to the first occurrence of `needle` (+ `delta` chars).
fn cursor_to(
    ws: &Entity<EditorWorkspace>,
    needle: &str,
    delta: usize,
    window: &mut Window,
    cx: &mut App,
) -> Option<Point<Pixels>> {
    let ed = ws.read(cx).active_editor()?;
    let text = ed.read(cx).value().to_string();
    let off = text.find(needle)? + delta;
    let pos = ed.read(cx).text().offset_to_position(off);
    ed.update(cx, |s, cx| s.set_cursor_position(pos, window, cx));
    let b = ed.read(cx).range_to_bounds(&(off..off + 1))?;
    Some(b.center())
}

fn run_scenario(name: String, ws: Entity<EditorWorkspace>, window: &mut Window, cx: &mut App) {
    window
        .spawn(cx, async move |cx| {
            let ex = cx.background_executor().clone();
            let sleep = |ms| ex.timer(Duration::from_millis(ms));
            sleep(800).await;
            let open = |rel: &'static str, cx: &mut AsyncWindowContext| {
                let ws = ws.clone();
                let _ = cx.update(|window, cx| {
                    ws.update(cx, |w, cx| {
                        let _ = w.open_file(std::path::Path::new(rel), window, cx);
                    })
                });
            };
            let wait_lsp = async |cx: &mut AsyncWindowContext| {
                let t = Instant::now();
                while t.elapsed() < Duration::from_secs(90) {
                    if cx.update(|_, cx| wait_for_server(&ws, cx)).unwrap_or(true) {
                        break;
                    }
                    cx.background_executor()
                        .timer(Duration::from_millis(500))
                        .await;
                }
            };
            match name.as_str() {
                "tree" => {
                    open("src/main.rs", cx);
                    let _ = cx.update(|window, cx| {
                        ws.update(cx, |w, cx| {
                            let _ =
                                w.open_file(std::path::Path::new("scripts/tool.py"), window, cx);
                            let _ = w.open_file(std::path::Path::new("src/main.rs"), window, cx);
                        });
                        window.dispatch_action(Box::new(pod_editor_actions::FocusTree), cx);
                    });
                }
                "quickopen" => {
                    open("src/lib.rs", cx);
                    sleep(1500).await;
                    let _ = cx.update(|window, cx| {
                        ws.update(cx, |w, cx| w.quick_open_query("geo", window, cx))
                    });
                }
                "editor" => open("src/lib.rs", cx),
                "notinstalled" => {
                    open("scripts/tool.py", cx);
                    wait_lsp(cx).await;
                }
                "diagnostics" => {
                    open("src/main.rs", cx);
                    wait_lsp(cx).await;
                    sleep(4000).await;
                    let _ = cx.update(|window, cx| {
                        ws.update(cx, |w, cx| w.show_panel(false, cx));
                        let _ = cursor_to(&ws, "word_count(&message, 2)", 0, window, cx);
                    });
                }
                "hover" => {
                    open("src/main.rs", cx);
                    wait_lsp(cx).await;
                    sleep(2000).await;
                    let p = cx
                        .update(|window, cx| cursor_to(&ws, "greet(\"Pod\")", 1, window, cx))
                        .ok()
                        .flatten();
                    if let Some(p) = p {
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
                            sleep(700).await;
                        }
                    }
                }
                "completion" => {
                    open("src/main.rs", cx);
                    wait_lsp(cx).await;
                    sleep(1500).await;
                    let _ = cx.update(|window, cx| {
                        let ed = ws.read(cx).active_editor();
                        if let Some(ed) = ed {
                            let text = ed.read(cx).value().to_string();
                            if let Some(off) = text.find("    task.finish();") {
                                let pos = ed.read(cx).text().offset_to_position(off + 4);
                                ed.update(cx, |s, cx| {
                                    s.set_cursor_position(pos, window, cx);
                                    s.insert("task.fi\n    ", window, cx);
                                    let p2 = s.text().offset_to_position(off + 4 + "task.fi".len());
                                    s.set_cursor_position(p2, window, cx);
                                });
                            }
                        }
                    });
                    sleep(600).await;
                    let _ = cx
                        .update(|window, cx| window.dispatch_action(Box::new(ShowCompletions), cx));
                }
                "definition" => {
                    open("src/main.rs", cx);
                    wait_lsp(cx).await;
                    sleep(1500).await;
                    let _ = cx.update(|window, cx| {
                        let _ = cursor_to(&ws, "perimeter(&square)", 2, window, cx);
                        window.dispatch_action(Box::new(GoToDefinition), cx);
                    });
                }
                other => eprintln!("unknown scenario {other}"),
            }
        })
        .detach();
}

/// Re-export of the workspace's actions for scripting.
mod pod_editor_actions {
    pub use pod_editor::FocusTree;
}

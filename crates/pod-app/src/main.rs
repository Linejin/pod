//! pod-app: the Pod desktop app (P2 UI MVP), a GPUI client of the podd daemon.
//!
//! Usage:
//!   pod-app [--display primary|secondary|N] [--pipe NAME] [--open-all]
//!           [--stats] [--no-throttle] [--duration SECS]
//!
//! --display     monitor to open on (default: secondary, else primary)
//! --pipe        podd pipe (default `\\.\pipe\podd-<user>` / `POD_SOCKET`)
//! --open-all    open every running terminal and arrange a grid at start
//! --stats       print `[stats]` once per second and show fps in the status bar
//! --no-sound    no beep when a terminal turns blocked
//! --no-throttle do not cap the unfocused window at 30 fps (measurements)
//! --duration    quit after SECS, printing a `[summary]` line (automation)
//! --review TASK open the Review panel of TASK at start
//! --extensions  open the 확장 (extensions) settings page at start
//! --command ID  run a palette command once connected (repeatable; e.g.
//!               `orch-board`, `orch-inbox`: automation without key input)

// Release builds are GUI programs: no console window pops up next to the app.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

#[cfg(windows)]
mod agent_kind;
#[cfg(test)]
mod agent_marks_qa;
mod app_icon;
mod attach;
mod auto_open;
mod chrome;
mod editor_panel;
mod fixture_scenes;
mod gates;
mod groups;
mod hooks_prompt;
mod layout;
mod layout_mode;
mod layouts;
mod mods_ui;
#[cfg(test)]
mod native_qa;
mod orch;
mod orch_ui;
mod os_notify;
mod panel;
mod pr_panel;
#[cfg(test)]
mod pr_qa;
mod qa_marks;
mod review;
mod sessions_ui;
mod source;
mod stats;
mod strings;
mod term_links;
mod theme;
mod tree;
#[cfg(test)]
mod ui_review_qa;
mod vm;
mod workspace;

use std::time::{Duration, Instant};

use gpui_kit::component::TitleBar;
use gpui_kit::component::dock::register_panel;
use gpui_kit::*;

use crate::source::PoddSource;
use crate::workspace::{
    ArrangeGrid, NewTerminal, OpenReview, QuickOpenFiles, StartOptions, Workspace,
};

#[derive(Clone, Debug)]
struct Args {
    fixture: bool,
    display: String,
    pipe: Option<String>,
    open_all: bool,
    stats: bool,
    no_throttle: bool,
    duration: Option<u64>,
    no_sound: bool,
    review: Option<String>,
    settings: bool,
    fixture_scene: Option<String>,
    commands: Vec<String>,
}

fn parse_args() -> Args {
    let mut a = Args {
        fixture: false,
        display: "secondary".into(),
        pipe: None,
        open_all: false,
        stats: false,
        no_throttle: false,
        duration: None,
        no_sound: false,
        review: None,
        settings: false,
        fixture_scene: None,
        commands: Vec::new(),
    };
    let mut it = std::env::args().skip(1);
    while let Some(x) = it.next() {
        match x.as_str() {
            "--ui-fixture" => a.fixture = true,
            "--fixture-scene" => a.fixture_scene = it.next(),
            "--display" => a.display = it.next().unwrap_or_else(|| "secondary".into()),
            "--pipe" => a.pipe = it.next(),
            "--open-all" => a.open_all = true,
            "--stats" => a.stats = true,
            "--no-throttle" => a.no_throttle = true,
            "--no-sound" => a.no_sound = true,
            "--review" => a.review = it.next(),
            "--extensions" => a.settings = true,
            "--command" => a.commands.extend(it.next()),
            "--duration" => a.duration = it.next().and_then(|v| v.parse().ok()),
            _ => eprintln!("unknown arg: {x}"),
        }
    }
    a
}

/// Choose the display to open on.
fn pick_display(choice: &str, cx: &App) -> Option<DisplayId> {
    let displays = cx.displays();
    let primary = cx.primary_display().map(|d| d.id());
    match choice {
        "primary" => primary,
        "secondary" => displays
            .iter()
            .map(|d| d.id())
            .find(|id| Some(*id) != primary)
            .or(primary),
        n => n
            .parse::<usize>()
            .ok()
            .and_then(|i| displays.get(i))
            .map(|d| d.id())
            .or(primary),
    }
}

/// A GUI-subsystem exe has no console; attach to the parent's when started
/// from a terminal so `[stats]` lines still show up.
#[cfg(windows)]
fn attach_parent_console() {
    unsafe extern "system" {
        fn GetStdHandle(n: u32) -> *mut core::ffi::c_void;
        fn AttachConsole(pid: u32) -> i32;
    }
    const STD_OUTPUT_HANDLE: u32 = -11i32 as u32;
    const ATTACH_PARENT_PROCESS: u32 = u32::MAX;
    unsafe {
        if GetStdHandle(STD_OUTPUT_HANDLE).is_null() {
            AttachConsole(ATTACH_PARENT_PROCESS);
        }
    }
}

fn summarize(samples: &[stats::Window]) {
    if samples.is_empty() {
        println!("[summary] no samples");
        return;
    }
    let n = samples.len() as f64;
    let avg = |f: fn(&stats::Window) -> f64| samples.iter().map(f).sum::<f64>() / n;
    let min_fps = samples.iter().map(|s| s.fps).fold(f64::MAX, f64::min);
    let max_frame = samples
        .iter()
        .map(|s| s.max_frame_cpu_ms)
        .fold(0.0, f64::max);
    let slow: u64 = samples.iter().map(|s| s.slow_frames).sum();
    println!(
        "[summary] samples={} fps avg={:.1} min={:.1} | terminal paints/frame={:.2} paint ms/frame={:.3} | frame cpu avg={:.2}ms max={:.2}ms slow(>16.6ms)={} | secs<55fps={}",
        samples.len(),
        avg(|s| s.fps),
        min_fps,
        avg(|s| s.paints_per_frame),
        avg(|s| s.paint_ms_per_frame),
        avg(|s| s.avg_frame_cpu_ms),
        max_frame,
        slow,
        samples.iter().filter(|s| s.fps < 55.0).count()
    );
}

/// The one startup registration, shared by `main` and the headless native-QA
/// tests so both run the same: the layout modules, the key bindings and the
/// dock panel registry. A layout mode binds keys that only matter once it is
/// shown (kanban, blocks, review, queue) on its first render, once per app;
/// its per-window state lives in the kit's state slot, released with the
/// window (`layouts::kit`).
fn install(cx: &mut App) {
    cx.set_global(layout_mode::LayoutRegistry::new());
    chrome::bind_keys(cx);
    cx.bind_keys([
        KeyBinding::new("ctrl-shift-t", NewTerminal, None),
        KeyBinding::new("ctrl-shift-g", ArrangeGrid, None),
        KeyBinding::new("ctrl-shift-r", OpenReview, None),
    ]);
    review::bind_keys(cx);
    // Editor keys live in the `PodEditor` context; Ctrl+P outside it opens
    // quick open for the current task (inside, the editor's own wins).
    pod_editor::init(cx);
    cx.bind_keys([KeyBinding::new("ctrl-p", QuickOpenFiles, None)]);
    orch::bind_keys(cx);
    orch_ui::bind_keys(cx);
    layouts::rail::bind_keys(cx);
    layouts::dense::bind_keys(cx);
    register_panel(cx, panel::PANEL_NAME, workspace::build_panel);
    register_panel(cx, review::REVIEW_PANEL_NAME, workspace::build_review_panel);
    register_panel(cx, groups::GROUP_PANEL_NAME, workspace::build_group_panel);
    register_panel(cx, gates::GATES_PANEL_NAME, workspace::build_gates_panel);
    register_panel(cx, tree::TREE_PANEL_NAME, workspace::build_tree_panel);
    register_panel(
        cx,
        editor_panel::EDITOR_PANEL_NAME,
        workspace::build_editor_panel,
    );
    register_panel(cx, "PodSettings", chrome::build_chrome_panel);
    for name in orch_ui::PANEL_NAMES {
        register_panel(cx, name, orch_ui::build_panel);
    }
    register_panel(cx, "PodInspector", chrome::build_chrome_panel);
}

fn main() {
    #[cfg(windows)]
    attach_parent_console();
    pod_terminal_view::perf::set_paint_hook(stats::record_paint);
    let args = parse_args();
    let pipe = args
        .pipe
        .as_deref()
        .map(podd::endpoint::pipe_path)
        .unwrap_or_else(podd::endpoint::default_pipe);
    println!("pod-app: podd pipe {pipe}");

    gpui_kit::application().with_assets(gpui_kit::assets::Assets::new("")).run(move |cx| {
        gpui_kit::init(cx);
        // Same AppUserModelID as the installer's shortcut: taskbar grouping
        // and Windows toasts for mod notices (os_notify.rs). Sets the process
        // id only; registration is the installer's job.
        cx.set_app_identity(&os_notify::aumid(), "Pod");
        for (path, reason) in theme::reload_themes() {
            eprintln!("pod-app: theme {} skipped: {reason}", path.display());
        }
        let (settings, settings_notice) = theme::Settings::load();
        settings.apply(cx);
        install(cx);

        let display_id = pick_display(&args.display, cx);
        let bounds = Bounds::centered(display_id, size(px(1600.), px(1000.)), cx);
        let options = WindowOptions {
            focus: !args.fixture,
            // Maximized on the chosen display (fixed sizes overflow the 150%
            // secondary monitor, docs/02 §5).
            window_bounds: Some(WindowBounds::Maximized(bounds)),
            display_id,
            window_min_size: Some(size(px(800.),px(600.))),
            titlebar: Some(TitleBar::title_bar_options()),
            app_owns_titlebar_drag: true,
            inactive_frame_interval: if args.no_throttle { None } else { Some(Duration::from_micros(33_333)) },
            ..Default::default()
        };
        if args.fixture {
            fixture_scenes::prepare(args.fixture_scene.as_deref());
            orch_ui::fixture::set_scene(args.fixture_scene.as_deref());
        }
        let (source, events) = if args.fixture { PoddSource::fixture() } else { PoddSource::start(pipe.clone()) };
        // Hook check: the real `pod.exe` beside the app; the fixture's
        // `hooks-prompt` scene uses a scripted CLI (never the real install).
        let hook_cli: Option<std::sync::Arc<dyn hooks_prompt::HookCli>> = if args.fixture {
            args.fixture_scene.as_deref().is_some_and(|s| s.starts_with("hooks-")).then(|| std::sync::Arc::new(hooks_prompt::MockHookCli::missing()) as _)
        } else {
            hooks_prompt::PodCli::next_to_app().map(|c| std::sync::Arc::new(c) as _)
        };
        let start = StartOptions { open_all: args.open_all, sound: !args.no_sound, review: args.review.clone(), settings: args.settings, hook_cli: hook_cli.map(hooks_prompt::HookCliHandle) };
        let (stats_on, duration) = (args.stats || args.duration.is_some(), args.duration);
        let (window_handle, ws) = gpui_kit::open_window(options, cx, move |window, cx| cx.new(|cx| Workspace::new(source, events, start, window, cx)))
            .expect("failed to open window");
        if !args.fixture { cx.activate(true); }
        if let Some(notice) = settings_notice {
            eprintln!("pod-app: {notice}");
            ws.update(cx, |w, cx| w.raise_toast(workspace::ToastKind::Warning, strings::SETTINGS_DAMAGED, notice, cx));
        }
        // Unsaved editor files: ask before the window closes. On quit, save
        // the layout (editor tabs/cursors) and stop language servers.
        let close_ws = ws.downgrade();
        let _ = window_handle.update(cx, move |_, window, cx| {
            #[cfg(windows)]
            app_icon::install(window);
            window.on_window_should_close(cx, move |window, cx| close_ws.update(cx, |w, cx| w.confirm_quit(window, cx)).unwrap_or(true));
        });
        let quit_ws = ws.downgrade();
        cx.on_app_quit(move |cx| {
            let t = quit_ws.update(cx, |w, cx| w.on_quit(cx)).ok();
            async move {
                if let Some(t) = t {
                    t.await;
                }
            }
        })
        .detach();

        // `--command`: palette commands for automation (after the first data arrived).
        if !args.commands.is_empty() {
            let (workspace, commands) = (ws.downgrade(), args.commands.clone());
            cx.spawn(async move |cx| {
                // After the saved layout was restored (it replaces the dock).
                for _ in 0..100 {
                    cx.background_executor().timer(Duration::from_millis(200)).await;
                    if workspace.update(cx, |w, _| w.restored).unwrap_or(true) {
                        break;
                    }
                }
                cx.background_executor().timer(Duration::from_millis(500)).await;
                for id in commands {
                    let _ = workspace.update_in(cx, |w, window, cx| w.command(&id, window, cx));
                }
            })
            .detach();
        }
        // Deterministic visual QA only: never dispatch commands into a live workspace.
        if args.fixture && let Some(scene) = args.fixture_scene {
                let workspace = ws.downgrade();
                cx.spawn(async move |cx| {
                    cx.background_executor().timer(Duration::from_millis(600)).await;
                    let _ = workspace.update_in(cx, |w, window, cx| {
                        if scene == "palette" { w.open_jump(window, cx); }
                        else if scene == "inspect" { w.focused_terminal = Some("fixture-1".into()); w.open_inspector(window, cx); }
                        else if scene == "review" { w.open_review("sample/task-000", window, cx); }
                        else if scene == "hooks-preview" { w.request_hook_install(window, cx); }
                        else if scene.starts_with("rail") { fixture_scenes::rail(&scene, w, window, cx); }
                        else if fixture_scenes::is_editor_scene(&scene) { fixture_scenes::run(&scene, w, window, cx); }
                        else if scene == layouts::queue::FIXTURE_SCENE { fixture_scenes::queue(w, window, cx); }
                        else if scene == layouts::dense::FIXTURE_SCENE { fixture_scenes::dense(w, window, cx); }
                        else { w.command(&scene, window, cx); }
                    });
                }).detach();
        }

        if stats_on {
            let ws = ws.downgrade();
            cx.spawn(async move |cx| {
                let t0 = Instant::now();
                let mut last = Instant::now();
                let mut samples = Vec::new();
                loop {
                    cx.background_executor().timer(Duration::from_secs(1)).await;
                    let s = stats::take(last.elapsed());
                    last = Instant::now();
                    println!(
                        "[stats] t={:>3}s fps={:>5.1} paints/s={:>6.1} paints/frame={:>5.2} paint ms/frame={:.2} paint avg={:.2}ms max={:.2}ms | frame cpu avg={:.2}ms max={:.2}ms slow={}",
                        t0.elapsed().as_secs(),
                        s.fps,
                        s.paints_per_sec,
                        s.paints_per_frame,
                        s.paint_ms_per_frame,
                        s.avg_paint_ms,
                        s.max_paint_ms,
                        s.avg_frame_cpu_ms,
                        s.max_frame_cpu_ms,
                        s.slow_frames
                    );
                    if t0.elapsed() > Duration::from_secs(5) {
                        samples.push(s);
                    }
                    let line: SharedString = format!("{:.0} fps · frame cpu {:.1} ms", s.fps, s.avg_frame_cpu_ms).into();
                    if ws.update(cx, |w, cx| {
                        w.stats_line = Some(line);
                        cx.notify();
                    })
                    .is_err()
                    {
                        break;
                    }
                    if let Some(d) = duration
                        && t0.elapsed() >= Duration::from_secs(d)
                    {
                        summarize(&samples);
                        cx.update(|cx| cx.quit());
                        break;
                    }
                }
            })
            .detach();
        }
    });
}

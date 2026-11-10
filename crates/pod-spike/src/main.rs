//! pod-spike: GPUI terminal grid + dock, multi-terminal FPS check.
//!
//! Usage:
//!   pod-spike [--terminals N] [--stress] [--duration SECS] [--cmd "<program> <args...>"]
//!
//! --stress   every terminal runs a colored output flood (PowerShell)
//!            --batch N (lines per burst, default 50) --sleep-ms M (default 16)
//! --no-throttle  measure capacity: do not cap unfocused windows at 30fps
//! --display  primary | secondary (default) | <index>
//! --stress-count K   only the first K terminals flood, the rest run an idle shell
//! --idle-cmd "<cmd>" program for the idle terminals of --stress-count (default: a shell)
//! --no-view-cache    baseline: no view caching, every output repaints all terminals (for comparison)
//! --duration quit after SECS and print a summary (for automated runs)
//! Stats are printed to stdout once per second and shown in the status bar.

// Release builds are GUI programs: no console window pops up next to the app.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod panel;
mod pty;
mod stats;

use std::time::{Duration, Instant};

use std::sync::Arc;

use gpui_kit::base::dock::{DockArea, DockLayout};
use gpui_kit::component::dock::{DockSkin, PanelHandle};
use gpui_kit::component::theme::{Theme, ThemeMode};
use gpui_kit::*;

use panel::TerminalPanel;

#[derive(Clone, Debug)]
struct Args {
    terminals: usize,
    stress: bool,
    /// Lines per batch and sleep between batches for --stress.
    batch: u32,
    sleep_ms: u32,
    duration: Option<u64>,
    /// Disable GPUI inactive-window throttling (default caps unfocused windows at 30fps).
    no_throttle: bool,
    /// "secondary" (default: first non-primary display, else primary), "primary", or an index.
    display: String,
    cmd: Option<(String, Vec<String>)>,
    /// With --stress: how many terminals flood (default all).
    stress_count: Option<usize>,
    idle_cmd: Option<(String, Vec<String>)>,
    view_cache: bool,
}

fn parse_args() -> Args {
    let mut a = Args {
        terminals: 4,
        stress: false,
        batch: 50,
        sleep_ms: 16,
        duration: None,
        no_throttle: false,
        display: "secondary".into(),
        cmd: None,
        stress_count: None,
        idle_cmd: None,
        view_cache: true,
    };
    let mut it = std::env::args().skip(1);
    while let Some(x) = it.next() {
        match x.as_str() {
            "--terminals" | "-n" => {
                a.terminals = it
                    .next()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(4)
                    .clamp(1, 64)
            }
            "--stress" => a.stress = true,
            "--no-throttle" => a.no_throttle = true,
            "--display" => a.display = it.next().unwrap_or_else(|| "secondary".into()),
            "--batch" => a.batch = it.next().and_then(|v| v.parse().ok()).unwrap_or(50),
            "--sleep-ms" => a.sleep_ms = it.next().and_then(|v| v.parse().ok()).unwrap_or(16),
            "--duration" => a.duration = it.next().and_then(|v| v.parse().ok()),
            "--stress-count" => a.stress_count = it.next().and_then(|v| v.parse().ok()),
            "--no-view-cache" => a.view_cache = false,
            "--idle-cmd" => {
                let mut parts = split_command_line(&it.next().unwrap_or_default()).into_iter();
                a.idle_cmd = parts.next().map(|p| (p, parts.collect()));
            }
            "--cmd" => {
                let line = it.next().unwrap_or_default();
                let mut parts = split_command_line(&line).into_iter();
                if let Some(p) = parts.next() {
                    a.cmd = Some((p, parts.collect()));
                }
            }
            _ => eprintln!("unknown arg: {x}"),
        }
    }
    a
}

/// PowerShell script that floods colored lines in batches (agent-like log spam).
const STRESS_PS: &str = r#"$e=[char]27;$i=0;while($true){$b=[System.Text.StringBuilder]::new();for($k=0;$k -lt __BATCH__;$k++){$i++;[void]$b.Append("$e[3$($i%7+1)m$($i.ToString().PadLeft(8))$e[0m $e[1mstep$e[0m tool_call read_file src/lib.rs:$($i%997) $e[2m$([guid]::NewGuid())$e[0m`n")};[Console]::Out.Write($b.ToString());Start-Sleep -Milliseconds __SLEEP__}"#;

/// Split on whitespace, honoring double quotes: `vim -c "set mouse=a"`.
fn split_command_line(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let (mut quoted, mut any) = (false, false);
    for c in line.chars() {
        match c {
            '"' => {
                quoted = !quoted;
                any = true;
            }
            c if c.is_whitespace() && !quoted => {
                if any {
                    out.push(std::mem::take(&mut cur));
                    any = false;
                }
            }
            c => {
                cur.push(c);
                any = true;
            }
        }
    }
    if any {
        out.push(cur);
    }
    out
}

fn program_for(args: &Args, index: usize) -> Option<(String, Vec<String>)> {
    if args.stress && args.stress_count.is_none_or(|k| index < k) {
        return Some((
            "powershell.exe".into(),
            vec![
                "-NoLogo".into(),
                "-NoProfile".into(),
                "-Command".into(),
                STRESS_PS
                    .replace("__BATCH__", &args.batch.to_string())
                    .replace("__SLEEP__", &args.sleep_ms.to_string()),
            ],
        ));
    }
    args.cmd.clone()
}

struct Workspace {
    dock: Entity<DockArea>,
    status: SharedString,
    terminals: usize,
    _stats: Task<()>,
}

impl Workspace {
    fn new(args: &Args, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let n = args.terminals;
        let panels: Vec<Entity<TerminalPanel>> = (0..n)
            .map(|i| {
                cx.new(|cx| {
                    let program = if args.stress && args.stress_count.is_some_and(|k| i >= k) {
                        // Idle terminal next to the flooding ones.
                        args.idle_cmd.clone().or_else(|| {
                            Some((
                                "powershell.exe".into(),
                                vec!["-NoLogo".into(), "-NoProfile".into()],
                            ))
                        })
                    } else {
                        program_for(args, i)
                    };
                    TerminalPanel::new(i, program, args.view_cache, window, cx)
                        .expect("failed to spawn terminal")
                })
            })
            .collect();

        // Near-square grid: rows of h-splits stacked in a v-split.
        let cols = (n as f64).sqrt().ceil() as usize;
        let mut layout = DockLayout::v_split();
        for row in panels.chunks(cols) {
            let mut h = DockLayout::h_split();
            for p in row {
                // Wrap in the component PanelHandle so the skin shows our tab_name.
                let view = Arc::new(PanelHandle::new(p.clone()));
                h = h.child(DockLayout::tabs().panel_view(view, cx), None);
            }
            layout = layout.child(h, None);
        }

        let (dock, _skin) = DockSkin::dock_area("pod-spike", None, window, cx);
        dock.update(cx, |area, cx| area.set_center(layout, window, cx));
        if let Some(first) = panels.first() {
            first.read(cx).focus_handle(cx).focus(window, cx);
        }

        let duration = args.duration;
        let stats = cx.spawn(async move |this, cx| {
            let start = Instant::now();
            let mut last = Instant::now();
            let mut samples: Vec<stats::Window> = Vec::new();
            loop {
                cx.background_executor().timer(Duration::from_secs(1)).await;
                let s = stats::take(last.elapsed());
                last = Instant::now();
                println!(
                    "[stats] t={:>3}s fps={:>5.1} paints/s={:>6.1} paints/frame={:>5.2} paint ms/frame={:.2} paint avg={:.2}ms max={:.2}ms | frame cpu avg={:.2}ms max={:.2}ms slow={}",
                    start.elapsed().as_secs(),
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
                // Ignore the first 2s (startup, shell spawn).
                if start.elapsed() > Duration::from_secs(2) {
                    samples.push(s);
                }
                let ok = this
                    .update(cx, |w, cx| {
                        w.status = format!(
                            "{} terminals · {:.0} fps · paint avg {:.2}ms / max {:.2}ms per terminal",
                            w.terminals, s.fps, s.avg_paint_ms, s.max_paint_ms
                        )
                        .into();
                        cx.notify();
                    })
                    .is_ok();
                if !ok {
                    break;
                }
                if let Some(d) = duration
                    && start.elapsed() >= Duration::from_secs(d)
                {
                    summarize(&samples);
                    let _ = cx.update(|cx| cx.quit());
                    break;
                }
            }
        });

        Self {
            dock,
            status: "starting…".into(),
            terminals: n,
            _stats: stats,
        }
    }
}

fn summarize(samples: &[stats::Window]) {
    if samples.is_empty() {
        println!("[summary] no samples");
        return;
    }
    let n = samples.len() as f64;
    let avg_fps = samples.iter().map(|s| s.fps).sum::<f64>() / n;
    let min_fps = samples.iter().map(|s| s.fps).fold(f64::MAX, f64::min);
    let avg_paint = samples.iter().map(|s| s.avg_paint_ms).sum::<f64>() / n;
    let max_paint = samples.iter().map(|s| s.max_paint_ms).fold(0.0, f64::max);
    let avg_frame = samples.iter().map(|s| s.avg_frame_cpu_ms).sum::<f64>() / n;
    let max_frame = samples
        .iter()
        .map(|s| s.max_frame_cpu_ms)
        .fold(0.0, f64::max);
    let slow: u64 = samples.iter().map(|s| s.slow_frames).sum();
    let ppf = samples.iter().map(|s| s.paints_per_frame).sum::<f64>() / n;
    let pmf = samples.iter().map(|s| s.paint_ms_per_frame).sum::<f64>() / n;
    let below60 = samples.iter().filter(|s| s.fps < 55.0).count();
    println!(
        "[summary] samples={} fps avg={:.1} min={:.1} | terminal paints/frame={:.2} paint ms/frame={:.3} | per-terminal paint avg={:.2}ms max={:.2}ms | frame cpu avg={:.2}ms max={:.2}ms slow(>16.6ms)={} | secs<55fps={}",
        samples.len(),
        avg_fps,
        min_fps,
        ppf,
        pmf,
        avg_paint,
        max_paint,
        avg_frame,
        max_frame,
        slow,
        below60
    );
}

impl Render for Workspace {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        stats::record_frame();
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(rgb(0x16171a))
            .text_color(rgb(0xd6d8db))
            .child(div().flex_1().min_h_0().child(self.dock.clone()))
            .child(
                div()
                    .h(px(24.))
                    .px_2()
                    .flex()
                    .items_center()
                    .text_xs()
                    .bg(rgb(0x202227))
                    .child(self.status.clone()),
            )
    }
}

/// Choose the display to open on. Prints the display list so `--display N` is discoverable.
fn pick_display(choice: &str, cx: &App) -> Option<DisplayId> {
    let displays = cx.displays();
    let primary = cx.primary_display().map(|d| d.id());
    for (i, d) in displays.iter().enumerate() {
        let b = d.bounds();
        println!(
            "display {i}: {:?} at ({:.0},{:.0}) {:.0}x{:.0}{}",
            d.id(),
            b.origin.x.as_f32(),
            b.origin.y.as_f32(),
            b.size.width.as_f32(),
            b.size.height.as_f32(),
            if Some(d.id()) == primary {
                " (primary)"
            } else {
                ""
            }
        );
    }
    let picked = match choice {
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
    };
    println!("opening on display {picked:?} ({choice})");
    picked
}

/// A GUI-subsystem exe has no console. When launched from a terminal (stdout not
/// redirected), attach to the parent's console so `[stats]` lines still show up.
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

fn main() {
    // Must run before any thread exists (GPUI, PTY readers).
    pty::sanitize_child_env();
    pod_terminal_view::perf::set_paint_hook(stats::record_paint);
    #[cfg(windows)]
    attach_parent_console();
    let args = parse_args();
    println!("pod-spike args: {args:?}");
    gpui_kit::application().run(move |cx| {
        gpui_kit::init(cx);
        Theme::change(ThemeMode::Dark, None, cx);
        let display_id = pick_display(&args.display, cx);
        let bounds = Bounds::centered(display_id, size(px(1600.), px(1000.)), cx);
        let options = WindowOptions {
            // Maximized on the chosen display: a fixed size overflowed the
            // secondary monitor because GPUI mixes DPI-scaled and raw display
            // coordinates across monitors (observed: 2560,235 reported as 1707,157).
            window_bounds: Some(WindowBounds::Maximized(bounds)),
            display_id,
            inactive_frame_interval: if args.no_throttle {
                None
            } else {
                Some(Duration::from_micros(33_333))
            },
            ..Default::default()
        };
        let args = args.clone();
        gpui_kit::open_window(options, cx, move |window, cx| {
            cx.new(|cx| Workspace::new(&args, window, cx))
        })
        .expect("failed to open window");
        cx.activate(true);
    });
}

#[cfg(test)]
mod tests {
    use super::split_command_line;

    #[test]
    fn command_line_quotes() {
        assert_eq!(
            split_command_line(r#"vim -c "set mouse=a" f.txt"#),
            ["vim", "-c", "set mouse=a", "f.txt"]
        );
        assert_eq!(
            split_command_line("  cmd.exe   /c dir "),
            ["cmd.exe", "/c", "dir"]
        );
        assert_eq!(split_command_line(r#"a "" b"#), ["a", "", "b"]);
    }
}

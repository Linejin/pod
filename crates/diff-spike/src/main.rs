//! diff-spike: P4 risk spike — a native GPUI code-review diff view.
//!
//! Usage:
//!   diff-spike --old A --new B
//!   diff-spike --git <repo> [--rev <ref>]          (working tree vs HEAD / ref)
//!   diff-spike --gen-bench <dir> [--lines N] [--changes M]   (write + open a synthetic pair)
//!
//! View:     --mode split|unified (default split)   --expand-all (no folding)   --wrap|--no-wrap
//! Comments: --export <file.json> (default ./diff-comments.json)   --demo-comments
//! Measure:  --bench-scroll <px/frame> [--bench-delay S] --duration S --no-throttle
//! Window:   --display primary|secondary|N (default secondary)

// Release builds are GUI programs: no console window pops up next to the app.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod review;
mod stats;

use std::path::PathBuf;
use std::time::Duration;

use gpui_kit::component::theme::{Theme, ThemeMode};
use gpui_kit::*;
use pod_diff::model;

#[derive(Clone, Debug)]
pub struct Args {
    pub old: Option<PathBuf>,
    pub new: Option<PathBuf>,
    pub git: Option<PathBuf>,
    pub rev: String,
    pub gen_bench: Option<PathBuf>,
    pub lines: usize,
    pub changes: usize,
    pub split: bool,
    pub expand_all: bool,
    /// None = on in split mode, off in unified.
    pub wrap: Option<bool>,
    pub export: PathBuf,
    pub demo_comments: bool,
    pub bench_scroll: Option<f32>,
    pub bench_delay: u64,
    pub duration: Option<u64>,
    pub no_throttle: bool,
    pub display: String,
}

fn parse_args() -> Args {
    let mut a = Args {
        old: None,
        new: None,
        git: None,
        rev: "HEAD".into(),
        gen_bench: None,
        lines: 10_000,
        changes: 1_000,
        split: true,
        expand_all: false,
        wrap: None,
        export: PathBuf::from("diff-comments.json"),
        demo_comments: false,
        bench_scroll: None,
        bench_delay: 3,
        duration: None,
        no_throttle: false,
        display: "secondary".into(),
    };
    let mut it = std::env::args().skip(1);
    while let Some(x) = it.next() {
        match x.as_str() {
            "--old" => a.old = it.next().map(PathBuf::from),
            "--new" => a.new = it.next().map(PathBuf::from),
            "--git" => a.git = it.next().map(PathBuf::from),
            "--rev" => a.rev = it.next().unwrap_or_else(|| "HEAD".into()),
            "--gen-bench" => a.gen_bench = it.next().map(PathBuf::from),
            "--lines" => a.lines = it.next().and_then(|v| v.parse().ok()).unwrap_or(10_000),
            "--changes" => a.changes = it.next().and_then(|v| v.parse().ok()).unwrap_or(1_000),
            "--mode" => a.split = it.next().as_deref() != Some("unified"),
            "--expand-all" => a.expand_all = true,
            "--wrap" => a.wrap = Some(true),
            "--no-wrap" => a.wrap = Some(false),
            "--export" => a.export = it.next().map(PathBuf::from).unwrap_or(a.export),
            "--demo-comments" => a.demo_comments = true,
            "--bench-scroll" => a.bench_scroll = it.next().and_then(|v| v.parse().ok()),
            "--bench-delay" => a.bench_delay = it.next().and_then(|v| v.parse().ok()).unwrap_or(3),
            "--duration" => a.duration = it.next().and_then(|v| v.parse().ok()),
            "--no-throttle" => a.no_throttle = true,
            "--display" => a.display = it.next().unwrap_or_else(|| "secondary".into()),
            _ => eprintln!("unknown arg: {x}"),
        }
    }
    a
}

/// Choose the display to open on (copied from pod-spike: secondary by default).
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

/// GUI-subsystem exe: attach to the parent console when stdout is not redirected.
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
    stats::process_start();
    #[cfg(windows)]
    attach_parent_console();
    let mut args = parse_args();
    println!("diff-spike args: {args:?}");

    if let Some(dir) = args.gen_bench.clone() {
        match model::generate_bench(&dir, args.lines, args.changes) {
            Ok((o, n)) => {
                println!("[gen] wrote {} and {}", o.display(), n.display());
                args.old = Some(o);
                args.new = Some(n);
            }
            Err(e) => {
                eprintln!("[gen] failed: {e:#}");
                std::process::exit(2);
            }
        }
    }

    let t = std::time::Instant::now();
    let loaded = match (&args.git, &args.old, &args.new) {
        (Some(repo), _, _) => model::from_git(repo, &args.rev),
        (None, Some(o), Some(n)) => model::from_two_files(o, n),
        _ => Err(anyhow::anyhow!(
            "need --old A --new B, --git <repo> [--rev REF], or --gen-bench <dir>"
        )),
    };
    let sources = match loaded {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error: {e:#}");
            std::process::exit(2);
        }
    };
    let load_ms = t.elapsed().as_secs_f64() * 1e3;
    let t = std::time::Instant::now();
    let patch: String = sources.iter().map(model::full_context_patch).collect();
    let diff_ms = t.elapsed().as_secs_f64() * 1e3;
    let total_lines: usize = sources
        .iter()
        .map(|f| {
            f.text(false)
                .lines()
                .count()
                .max(f.text(true).lines().count())
        })
        .sum();
    println!(
        "[open] {} files, {} lines: load {load_ms:.1}ms, line diff+patch {diff_ms:.1}ms ({} KB patch)",
        sources.len(),
        total_lines,
        patch.len() / 1024
    );

    // Bundled Lucide icons (fold expand chevrons, collapse arrows); without this they render blank.
    gpui_kit::application()
        .with_assets(gpui_kit::assets::Assets)
        .run(move |cx| {
            gpui_kit::init(cx);
            Theme::change(ThemeMode::Dark, None, cx);
            let display_id = pick_display(&args.display, cx);
            let bounds = Bounds::centered(display_id, size(px(1600.), px(1000.)), cx);
            let options = WindowOptions {
                // Maximized: a fixed size overflows the 150% secondary monitor (docs/02 §5).
                window_bounds: Some(WindowBounds::Maximized(bounds)),
                display_id,
                titlebar: Some(TitlebarOptions {
                    title: Some("diff-spike".into()),
                    ..Default::default()
                }),
                inactive_frame_interval: if args.no_throttle {
                    None
                } else {
                    Some(Duration::from_micros(33_333))
                },
                ..Default::default()
            };
            let args = args.clone();
            gpui_kit::open_window(options, cx, move |window, cx| {
                cx.new(|cx| review::Review::new(&args, sources, patch, window, cx))
            })
            .expect("failed to open window");
            cx.activate(true);
        });
}

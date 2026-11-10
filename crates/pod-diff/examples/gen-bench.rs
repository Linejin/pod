//! Writes the synthetic benchmark pair (`bench_old.rs`, `bench_new.rs`):
//! `gen-bench <dir> [lines] [changes]` (default 10000 lines, 1000 changes).

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let dir = std::path::PathBuf::from(args.next().unwrap_or_else(|| ".".into()));
    let lines = args.next().and_then(|v| v.parse().ok()).unwrap_or(10_000);
    let changes = args.next().and_then(|v| v.parse().ok()).unwrap_or(1_000);
    let (o, n) = pod_diff::model::generate_bench(&dir, lines, changes)?;
    println!("{} {}", o.display(), n.display());
    Ok(())
}

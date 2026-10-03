//! Benchmark: compiling the memory catalog, which every agent turn does.
//!
//! Ignored by default; uses only `compile_global_snapshot`, so the file runs
//! on any revision (see `crates/db/tests/bench_history.rs` for the
//! worktree recipe):
//!
//!   cargo test --release -p appv3-memory --test bench_catalog -- --ignored --nocapture
//!
//! BENCH_PAGES (default 400) pages of ~12 kB in 20 folders; times are the
//! median of BENCH_RUNS (default 50) compiles of an unchanged tree.

use std::collections::BTreeMap;
use std::time::Instant;

fn env_num(name: &str, default: usize) -> usize {
    std::env::var(name).ok().and_then(|v| v.parse().ok()).unwrap_or(default)
}

#[test]
#[ignore = "benchmark; run with --ignored --nocapture"]
fn bench_memory_catalog() {
    let pages = env_num("BENCH_PAGES", 400);
    let runs = env_num("BENCH_RUNS", 50);
    let d = tempfile::tempdir().unwrap();
    let root = d.path();
    std::fs::write(root.join("preferences.md"), "Prefer small diffs.\n").unwrap();
    for i in 0..pages {
        let dir = root.join(format!("area{}", i % 20));
        std::fs::create_dir_all(&dir).unwrap();
        let body = format!("---\ntitle: Topic {i}\n---\n# Topic {i}\n\nA short summary line for topic {i}.\n\n{}", "Details and notes. ".repeat(600));
        std::fs::write(dir.join(format!("t{i:04}.md")), body).unwrap();
    }
    appv3_memory::compile_global_snapshot(root); // warm the page cache
    let mut times: Vec<f64> = (0..runs)
        .map(|_| {
            let t = Instant::now();
            std::hint::black_box(appv3_memory::compile_global_snapshot(root));
            t.elapsed().as_secs_f64() * 1e6
        })
        .collect();
    times.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let mut results = BTreeMap::new();
    results.insert("compile catalog (µs)".to_string(), times[runs / 2]);

    if let Ok(path) = std::env::var("BENCH_JSON") {
        std::fs::write(&path, serde_json::to_string_pretty(&results).unwrap()).unwrap();
    }
    let baseline: Option<BTreeMap<String, f64>> = std::env::var("BENCH_BASELINE").ok().map(|p| serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap());
    println!("\nmemory catalog — {pages} pages, median of {runs} compiles{}\n", if baseline.is_some() { " — baseline → this revision" } else { "" });
    for (name, v) in &results {
        match baseline.as_ref().and_then(|b| b.get(name)) {
            Some(before) => println!("  {name:<24} {before:>10.1} → {v:>10.1}  ({:.1}×)", before / v),
            None => println!("  {name:<24} {v:>10.1}"),
        }
    }
    println!();
}

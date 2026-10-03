//! Benchmark: the per-request session queries on a long session.
//!
//! Ignored by default. Drives public queries only, so the same file runs on
//! any revision:
//!
//!   cargo test --release -p appv3-db --test bench_history -- --ignored --nocapture
//!
//!   # compare with an older revision
//!   git worktree add --detach /tmp/before <ref>
//!   cp crates/db/tests/bench_history.rs /tmp/before/appv3/crates/db/tests/
//!   (cd /tmp/before/appv3 && BENCH_JSON=/tmp/db-before.json cargo test --release -p appv3-db --test bench_history -- --ignored --nocapture)
//!   BENCH_BASELINE=/tmp/db-before.json cargo test --release -p appv3-db --test bench_history -- --ignored --nocapture
//!
//! The session is BENCH_EXCHANGES (default 2000) exchanges of a prompt, three
//! tool calls with ~8 kB results (every tenth carries a ~200 kB image part),
//! and an answer whose `extra` carries usage, like a long coding session.
//! Times are medians of BENCH_RUNS (default 30) calls.

use std::collections::BTreeMap;
use std::time::Instant;

use appv3_db::create_pool;
use appv3_db::queries::*;
use serde_json::{json, Map, Value};

fn env_num(name: &str, default: usize) -> usize {
    std::env::var(name).ok().and_then(|v| v.parse().ok()).unwrap_or(default)
}

fn obj(v: Value) -> Map<String, Value> {
    match v {
        Value::Object(m) => m,
        _ => unreachable!(),
    }
}

async fn seed(pool: &appv3_db::DbPool, sid: &str, exchanges: usize) {
    let result = "x".repeat(8_000);
    let image = "A".repeat(200_000);
    for n in 0..exchanges {
        save_message(pool, sid, NewMessage::user(format!("Prompt {n}: update module {n} and add a test."))).await.unwrap();
        for k in 0..3 {
            let call = format!("call-{n}-{k}");
            let mut asst = NewMessage::assistant(None);
            asst.tool_calls = Some(json!([{ "id": call, "type": "function", "function": { "name": "read", "arguments": "{\"path\":\"src/a.rs\"}" } }]));
            asst.extra = Some(obj(json!({ "usage": { "input": 12000, "output": 80, "cost": { "estimated_usd": 0.0012 } } })));
            save_message(pool, sid, asst).await.unwrap();
            let mut tool = NewMessage::tool(call, "read", result.clone());
            let mut extra = json!({ "duration_ms": 40 });
            if (n * 3 + k) % 10 == 0 {
                extra["parts"] = json!([{ "type": "image", "media_type": "image/png", "data": image }]);
            }
            tool.extra = Some(obj(extra));
            save_message(pool, sid, tool).await.unwrap();
        }
        let mut answer = NewMessage::assistant(Some(format!("Done with step {n}.")));
        answer.extra = Some(obj(json!({ "usage": { "input": 14000, "output": 240, "cost": { "estimated_usd": 0.0031 } } })));
        save_message(pool, sid, answer).await.unwrap();
    }
}

async fn median_us<F, Fut>(runs: usize, mut f: F) -> f64
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    f().await; // warm the page cache and statement cache
    let mut times: Vec<f64> = Vec::with_capacity(runs);
    for _ in 0..runs {
        let start = Instant::now();
        f().await;
        times.push(start.elapsed().as_secs_f64() * 1e6);
    }
    times.sort_by(|a, b| a.partial_cmp(b).unwrap());
    times[runs / 2]
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "benchmark; run with --ignored --nocapture"]
async fn bench_session_queries() {
    let exchanges = env_num("BENCH_EXCHANGES", 2000);
    let runs = env_num("BENCH_RUNS", 30);
    let dir = tempfile::tempdir().unwrap();
    let pool = create_pool(dir.path().join("oad.db")).await.unwrap();
    let s = create_session(&pool, NewSession { workspace: "/tmp/ws".into(), ..Default::default() }).await.unwrap();
    let seed_start = Instant::now();
    seed(&pool, &s.id, exchanges).await;
    let seed_ms = seed_start.elapsed().as_secs_f64() * 1e3;
    let rows = exchanges * 8;

    let mut results: BTreeMap<String, f64> = BTreeMap::new();
    let sid = s.id.clone();
    results.insert(
        "usage totals (history load)".into(),
        median_us(runs, || async {
            session_usage_totals(&pool, &sid).await.unwrap();
        })
        .await,
    );
    results.insert(
        "usage totals many (history load)".into(),
        median_us(runs, || async {
            session_usage_totals_many(&pool, &[sid.as_str()]).await.unwrap();
        })
        .await,
    );
    results.insert(
        "has queued (every model call)".into(),
        median_us(runs, || async {
            has_queued_user_messages(&pool, &sid).await.unwrap();
        })
        .await,
    );
    results.insert(
        "list queued".into(),
        median_us(runs, || async {
            list_queued_messages(&pool, &sid).await.unwrap();
        })
        .await,
    );
    results.insert(
        "newest history page".into(),
        median_us(runs, || async {
            history_page(&pool, &sid, None).await.unwrap();
        })
        .await,
    );
    results.insert(
        "save message (8 kB tool row)".into(),
        median_us(runs, || async {
            let mut tool = NewMessage::tool("bench-call", "read", "y".repeat(8_000));
            tool.extra = Some(obj(json!({ "duration_ms": 40 })));
            save_message(&pool, &sid, tool).await.unwrap();
        })
        .await,
    );

    if let Ok(path) = std::env::var("BENCH_JSON") {
        std::fs::write(&path, serde_json::to_string_pretty(&results).unwrap()).unwrap();
    }
    let baseline: Option<BTreeMap<String, f64>> = std::env::var("BENCH_BASELINE").ok().map(|p| serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap());
    println!("\nsession queries — {rows} rows (seeded in {seed_ms:.0} ms), median of {runs} runs, µs{}\n", if baseline.is_some() { " — baseline → this revision" } else { "" });
    for (name, us) in &results {
        match baseline.as_ref().and_then(|b| b.get(name)) {
            Some(before) => println!("  {name:<34} {before:>10.1} → {us:>10.1}  ({:.1}×)", before / us),
            None => println!("  {name:<34} {us:>10.1}"),
        }
    }
    println!();
}

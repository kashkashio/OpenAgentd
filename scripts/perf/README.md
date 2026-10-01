# Performance benchmarks

Before/after measurements for the performance plan. Each script runs
unchanged against an older checkout and the current one, so the numbers
compare the same scenario on both.

```bash
scripts/perf/run-all.sh            # baseline fad35386 vs this checkout
scripts/perf/run-all.sh <ref>      # any other baseline
```

`run-all.sh` keeps a detached worktree of the baseline in `/tmp/oadperf/base`
(`OAD_PERF_DIR` overrides). Remove it with `git worktree remove /tmp/oadperf/base`.

| Script | Measures |
|---|---|
| `backend_bench.rs` | Stream-store fan-out, `save_message`, turn-start heal, request-body build, OpenAI/Anthropic stream parsing, Codex provider build. Copied into `appv3/crates/agent/tests/` for the run, release mode. |
| `seed_reopen.rs` + `reopen.py` | Opening a long plan-mode session the way the web client does (history page walk): requests, bytes, server time, repeated member rows. Also 60 s of idle server CPU. |
| `web_store.bench.ts` | Replaying one long streamed turn into the agent store: time and Immer drafts. |
| `web_render.bench.tsx` | Rendering 80 finished turns, then 300 streaming flushes, under happy-dom. Use the ratio, not the absolute time. |

Run on an idle machine. The scripts run one at a time, and each reports a
median over several runs.

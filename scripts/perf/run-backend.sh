#!/usr/bin/env bash
# Run scripts/perf/backend_bench.rs against one checkout and print BENCH lines.
#   scripts/perf/run-backend.sh <repo-root> [label]
# The bench file is copied into the tree only for the run, then removed.
set -euo pipefail
tree="$(cd "$1" && pwd)"
label="${2:-$(git -C "$tree" rev-parse --short HEAD)}"
here="$(cd "$(dirname "$0")" && pwd)"
dest="$tree/appv3/crates/agent/tests/zz_perf_bench.rs"
cp "$here/backend_bench.rs" "$dest"
trap 'rm -f "$dest"' EXIT
cd "$tree/appv3"
cargo ${OAD_PERF_TOOLCHAIN-+1.98.1} test -q --release -p appv3-agent --test zz_perf_bench -- --ignored --nocapture --test-threads=1 2>&1 \
  | grep '^BENCH' | sed "s/^BENCH /BENCH [$label] /"

#!/usr/bin/env bash
# Run the web benchmarks against one checkout and print BENCH lines.
#   scripts/perf/run-web.sh <repo-root> [label]
set -euo pipefail
tree="$(cd "$1" && pwd)"
label="${2:-$(git -C "$tree" rev-parse --short HEAD)}"
here="$(cd "$(dirname "$0")" && pwd)"
dir="$tree/web/src/__tests__"
cp "$here/web_store.bench.ts" "$dir/zz_perf_store.bench.test.ts"
cp "$here/web_render.bench.tsx" "$dir/zz_perf_render.bench.test.tsx"
trap 'rm -f "$dir/zz_perf_store.bench.test.ts" "$dir/zz_perf_render.bench.test.tsx"' EXIT
cd "$tree/web"
for f in zz_perf_store.bench.test.ts zz_perf_render.bench.test.tsx; do
  OAD_LABEL="$label" bun test --timeout 600000 "src/__tests__/$f" 2>&1 | grep '^BENCH' || { echo "bench $f failed" >&2; exit 1; }
done

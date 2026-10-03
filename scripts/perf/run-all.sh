#!/usr/bin/env bash
# Before/after performance run: a baseline ref (default fad35386, the commit
# before the performance plan) against the current checkout.
#   scripts/perf/run-all.sh [base-ref]
# Results go to $OAD_PERF_DIR/results-<ts>.txt (default /tmp/oadperf).
# Run on an otherwise idle machine; every step runs one at a time.
set -euo pipefail
repo="$(git rev-parse --show-toplevel)"
here="$repo/scripts/perf"
base_ref="${1:-fad35386}"
work="${OAD_PERF_DIR:-/tmp/oadperf}"
tc="${OAD_PERF_TOOLCHAIN-+1.98.1}"
out="$work/results-$(date +%Y%m%d-%H%M%S).txt"
mkdir -p "$work"

if [ ! -d "$work/base" ]; then git -C "$repo" worktree add --detach "$work/base" "$base_ref"; fi
git -C "$work/base" checkout -q --detach "$base_ref"
for t in "$work/base" "$repo"; do
  (cd "$t/appv3" && cargo ${tc:+"$tc"} build -q --release -p appv3-cli)
  (cd "$t/web" && bun install --frozen-lockfile >/dev/null)
done

# One seeded database, shared by both servers (the schema is unchanged).
mkdir -p "$work/ws"
rm -f "$work/seed.db" "$work/seed.db-wal" "$work/seed.db-shm"
seed="$repo/appv3/crates/agent/tests/zz_seed_reopen.rs"
cp "$here/seed_reopen.rs" "$seed"
lead="$(cd "$repo/appv3" && OAD_SEED_DB="$work/seed.db" OAD_SEED_WS="$work/ws" cargo ${tc:+"$tc"} test -q -p appv3-agent --test zz_seed_reopen -- --ignored --nocapture 2>/dev/null | sed -n 's/^SEEDED lead=//p')"
rm -f "$seed"
sqlite3 "$work/seed.db" "PRAGMA wal_checkpoint(TRUNCATE);" >/dev/null
export OAD_LEAD="$lead"
export OAD_PERF_TOOLCHAIN="$tc"
older_base="${OAD_PERF_BASE_OLDER_PAGES:-10}"
older_head="${OAD_PERF_HEAD_OLDER_PAGES:-2}"
{
  "$here/run-backend.sh" "$work/base" base
  "$here/run-backend.sh" "$repo" head
  python3 "$here/reopen.py" "$work/base/appv3/target/release/openagentd" "$work/seed.db" base "$older_base"
  python3 "$here/reopen.py" "$repo/appv3/target/release/openagentd" "$work/seed.db" head "$older_head"
  "$here/run-web.sh" "$work/base" base
  "$here/run-web.sh" "$repo" head
} | tee "$out"
echo "results: $out"

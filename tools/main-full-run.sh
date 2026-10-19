#!/usr/bin/env bash
# Run the full suite (tools/gate --full) on demand on a pinned, fresh origin/main. Logs, the lock and
# latest.json go to $HIRSEL_MAIN_RUNS; each run's detached worktree goes under
# $HIRSEL_MAIN_WORKTREES; Cargo reuses $CARGO_TARGET_DIR between runs.
set -Eeuo pipefail
if (( $# > 0 )); then echo "usage: tools/main-full-run.sh" >&2; exit 2; fi
repo=$(git -C "$(dirname "$0")/.." rev-parse --show-toplevel)
runs=${HIRSEL_MAIN_RUNS:-${XDG_STATE_HOME:-$HOME/.local/state}/hirsel/main-runs}
trees=${HIRSEL_MAIN_WORKTREES:-$runs/worktrees}
mkdir -p "$runs"
# Keep the lock inode: unlinking it would let another runner lock a different file.
exec 9>"$runs/main-full.lock"
if ! flock -n 9; then
  echo "main-full-run: a run is already active (see $runs/main-full.pid)" >&2
  exit 75
fi
printf '%s\n' "$$" > "$runs/main-full.pid"
worktree=""
log=""
sha=""
start=$(date -u +%Y-%m-%dT%H:%M:%SZ)
started=$SECONDS
stage="fetch origin/main"
finish() {
  local status=$? end
  trap - EXIT
  end=$(date -u +%Y-%m-%dT%H:%M:%SZ)
  if [[ -n "$log" ]]; then
    echo "main-full-run: finished (exit $status, $((SECONDS - started)) s)" >> "$log"
  fi
  if [[ -n "$worktree" ]]; then
    cd "$repo"
    if ! git -C "$repo" worktree remove --force "$worktree" >> "${log:-/dev/null}" 2>&1; then
      echo "main-full-run: worktree cleanup failed: $worktree" >&2
      if (( status == 0 )); then
        status=1
        stage="worktree cleanup"
      fi
    fi
  fi
  # Write latest.json (one JSON line, atomic rename) while still owning the lock.
  python3 "$repo/tools/main_full_result.py" "$runs" "$sha" "$start" "$end" "$status" "$stage" \
    "$log" "$((SECONDS - started))"
  rm -f "$runs/main-full.pid"
  echo "main-full-run: $sha $([[ $status == 0 ]] && echo pass || echo fail), $((SECONDS - started)) s; ${log:-no gate log}"
  exit "$status"
}
trap finish EXIT
trap 'exit 130' INT
trap 'exit 143' TERM
# The pidfile is diagnostic; flock handles stale PIDs and the check/start race.
git -C "$repo" fetch origin main
sha=$(git -C "$repo" rev-parse refs/remotes/origin/main)
log="$runs/$(date -u +%Y%m%dT%H%M%SZ)-$sha.log"
: > "$log"
stage="fresh main worktree"
mkdir -p "$trees"
worktree=$(mktemp -d "$trees/main-full-XXXXXXXX")
rmdir "$worktree"
git -C "$repo" worktree add --detach "$worktree" "$sha" >> "$log" 2>&1
export CARGO_BUILD_JOBS=4 HIRSEL_JOBS=4 RUST_TEST_THREADS=4
# The owner's on-demand run queues for no lane's gate slot unless told to.
export HIRSEL_GATE_SLOTS="${HIRSEL_GATE_SLOTS:-none}"
# Reuse build artifacts between scheduled runs; source inputs always come from the fresh tree.
unset CARGO_TARGET_DIR_AUTO
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-${XDG_CACHE_HOME:-$HOME/.cache}/hirsel/main-full-target}"
export XDG_RUNTIME_DIR="${XDG_RUNTIME_DIR:-/run/user/$(id -u)}"
export DBUS_SESSION_BUS_ADDRESS="${DBUS_SESSION_BUS_ADDRESS:-unix:path=$XDG_RUNTIME_DIR/bus}"
stage="resource scope"
echo "main-full-run: sha=$sha start=$start worktree=$worktree" >> "$log"
# Foreground scope: it owns the complete process tree and releases it before cleanup.
cd "$worktree"
systemd-run --user --scope -p MemoryMax=8G -p CPUQuota=400% \
  nice -n 10 tools/gate --full >> "$log" 2>&1
stage="completed"

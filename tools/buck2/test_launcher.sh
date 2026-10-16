#!/bin/sh
# Run one libtest harness (or gate command) as `cargo test` would: from its
# package directory (HIRSEL_TEST_CWD), with an existing CARGO_TARGET_TMPDIR,
# the pinned LLVM as LLVM_SYS_211_PREFIX (its bin first on PATH, for tests that
# run `clang` by name, and its lib on the loader path), the runtime archives as
# absolute HIRSEL_RUNTIME_ARCHIVES, insta's snapshots under the project root,
# and test threads and campaign workers bounded by the action's CPU share (at
# most 4).
#
# TEST_SHARD_INDEX=<i> of TEST_TOTAL_SHARDS=<n> (Bazel's variables, i from 0)
# runs the i-th of n cost-balanced parts of the listed cases, weighted by
# HIRSEL_TEST_COSTS when it names a cost table. Given a filter or a value
# argument other than `--skip`, every shard but the first runs nothing, and
# the first runs the whole selection.
set -eu
root="$(pwd)"
if [ -n "${HIRSEL_TEST_POOL:-}" ]; then
  # A bless rewrites the files the test reads. A pool action reads a copy of
  # the checkout that is discarded after the run, so a bless there would
  # change nothing and could still pass. tools/bless.sh runs blesses on this host.
  bless=""
  [ -z "${HIRSEL_BLESS:-}" ] || bless="HIRSEL_BLESS=$HIRSEL_BLESS"
  case "${INSTA_UPDATE:-}" in always | 1 | force | new | unseen) bless="INSTA_UPDATE=$INSTA_UPDATE" ;; esac
  for argument in "$@"; do
    case "$argument" in --bless | --update-passing) bless="$argument" ;; esac
  done
  if [ -n "$bless" ]; then
    echo "test_launcher.sh: $bless on the pool would rewrite a discarded copy of the checkout; run it with tools/bless.sh" >&2
    exit 2
  fi
  # A pool action's work directory is a bind mount of a subdirectory, whose
  # source ancestry the runtime cannot establish, so it refuses recordings
  # there (crates/hirsel-rt/src/host_paths.rs). The action's private /dev
  # tmpfs is mounted at its root: scratch directories and TMPDIR live there.
  scratch=/dev/shm/hirsel-test
  mkdir -p "$scratch/target-tmpdir" "$scratch/tmp" "$(dirname "$HIRSEL_TEST_TMPDIR")"
  rm -rf "$HIRSEL_TEST_TMPDIR"
  ln -s "$scratch/target-tmpdir" "$HIRSEL_TEST_TMPDIR"
  export TMPDIR="$scratch/tmp"
else
  # On this host, scratch lives outside the project: Buck2 watches the project
  # tree and stops at a file name that is not UTF-8, which native launch tests
  # create on purpose. One directory per checkout, linked in atomically, since
  # host tests run concurrently: a directory an older run left is renamed away,
  # never deleted in place, and nothing recreates it, so a placed link stays.
  scratch="${TMPDIR:-/tmp}/hirsel-test-$(printf %s "$root" | cksum | cut -d' ' -f1)"
  mkdir -p "$scratch" "$(dirname "$HIRSEL_TEST_TMPDIR")"
  until [ "$(readlink "$HIRSEL_TEST_TMPDIR" || true)" = "$scratch" ]; do
    if [ -d "$HIRSEL_TEST_TMPDIR" ] && [ ! -L "$HIRSEL_TEST_TMPDIR" ] &&
      mv -T "$HIRSEL_TEST_TMPDIR" "$HIRSEL_TEST_TMPDIR.stale.$$" 2>/dev/null; then
      rm -rf "$HIRSEL_TEST_TMPDIR.stale.$$"
    fi
    ln -sfn "$scratch" "$HIRSEL_TEST_TMPDIR.$$"
    mv -T "$HIRSEL_TEST_TMPDIR.$$" "$HIRSEL_TEST_TMPDIR" 2>/dev/null || rm -f "$HIRSEL_TEST_TMPDIR.$$"
  done
fi
LLVM_SYS_211_PREFIX="$(cd "$LLVM_SYS_211_PREFIX" && pwd)"
export LLVM_SYS_211_PREFIX
if [ -n "${HIRSEL_AARCH64_TOOLCHAIN:-}" ]; then
  arm="$(cd "$HIRSEL_AARCH64_TOOLCHAIN" && pwd)"
  qemu="$(cd "$HIRSEL_QEMU_TOOLCHAIN" && pwd)"
  valgrind="$(cd "$HIRSEL_VALGRIND_TOOLCHAIN" && pwd)"
  export HIRSEL_AARCH64_CC="$arm/bin/aarch64-none-linux-gnu-gcc"
  export HIRSEL_AARCH64_SYSROOT="$arm/aarch64-none-linux-gnu/libc"
  export HIRSEL_QEMU_AARCH64="$qemu/qemu-aarch64"
  export HIRSEL_VALGRIND="$valgrind/bin/valgrind"
  export VALGRIND_LIB="$valgrind/libexec/valgrind"
  export HIRSEL_VALGRIND_RUNTIME="$valgrind/lib/x86_64-linux-gnu"
  unset HIRSEL_AARCH64_TOOLCHAIN HIRSEL_QEMU_TOOLCHAIN HIRSEL_VALGRIND_TOOLCHAIN
fi
export PATH="$LLVM_SYS_211_PREFIX/bin:${PATH:-/usr/bin:/bin}"
export LD_LIBRARY_PATH="$LLVM_SYS_211_PREFIX/lib${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
if [ -n "${HIRSEL_RUNTIME_ARCHIVES:-}" ]; then
  HIRSEL_RUNTIME_ARCHIVES="$(cd "$HIRSEL_RUNTIME_ARCHIVES" && pwd)"
  export HIRSEL_RUNTIME_ARCHIVES
fi
if [ -n "${HIRSEL_NATIVE_SELECTION:-}" ]; then
  HIRSEL_NATIVE_SELECTION="$(realpath "$HIRSEL_NATIVE_SELECTION")"
  export HIRSEL_NATIVE_SELECTION
fi
export INSTA_WORKSPACE_ROOT="$root"
# A pool action's reservation is its cgroup's cpu.max (the worker's action
# supervisor), which nproc does not read; on the host, a scope's CPUQuota is.
# The tightest quota on the way to the root bounds the threads, as in Rust's
# available_parallelism, and so does a caller's own setting.
jobs="$(nproc 2>/dev/null || echo 4)"
[ "$jobs" -le 4 ] || jobs=4
cgroup="$(sed -n 's/^0:://p' /proc/self/cgroup 2>/dev/null)" || cgroup=""
while :; do
  if read -r quota period 2>/dev/null < "/sys/fs/cgroup${cgroup%/}/cpu.max" && [ "$quota" != max ]; then
    cpus=$((quota / period))
    [ "$cpus" -ge 1 ] || cpus=1
    [ "$cpus" -ge "$jobs" ] || jobs=$cpus
  fi
  [ -n "$cgroup" ] && [ "$cgroup" != / ] || break
  cgroup="${cgroup%/*}"
done
RUST_TEST_THREADS="${RUST_TEST_THREADS:-$jobs}" HIRSEL_JOBS="${HIRSEL_JOBS:-$jobs}"
[ "$RUST_TEST_THREADS" -le "$jobs" ] || RUST_TEST_THREADS=$jobs
[ "$HIRSEL_JOBS" -le "$jobs" ] || HIRSEL_JOBS=$jobs
export RUST_TEST_THREADS HIRSEL_JOBS
harness="$root/$1"
if [ -n "${HIRSEL_TEST_RELOCATE:-}" ] && [ -n "${HIRSEL_TEST_POOL:-}" ]; then
  # The conformance tools keep their scratch beside their executable
  # (crates/hirsel-conformance/src/native/scratch.rs): run a copy on /dev/shm, at
  # the same repository path, so its RUNPATH still reaches tools/.
  moved="$scratch/root"
  mkdir -p "$moved/$(dirname "$1")"
  cp "$1" "$moved/$1"
  ln -sfn "$root/tools" "$moved/tools"
  harness="$moved/$1"
fi
shift
if [ -n "${HIRSEL_TEST_BIN_EXES:-}" ]; then
  # The harness names the package's binaries (CARGO_BIN_EXE_<name>) under
  # HIRSEL_TEST_BIN_EXE_DIR, so its compile never waits for them
  # (tools/buck2/defs.bzl). Link those names to this run's binaries. Shards on
  # the host share the directory: each replaces a link by an atomic rename.
  mkdir -p "$HIRSEL_TEST_BIN_EXE_DIR"
  for entry in $HIRSEL_TEST_BIN_EXES; do
    link="$HIRSEL_TEST_BIN_EXE_DIR/${entry%%=*}"
    ln -s "$root/${entry#*=}" "$link.$$"
    mv -f "$link.$$" "$link"
  done
fi
cd "$HIRSEL_TEST_CWD"
shard=""
if [ -n "${TEST_TOTAL_SHARDS:-}" ]; then shard="$((TEST_SHARD_INDEX + 1))/$TEST_TOTAL_SHARDS"; fi
costs="${HIRSEL_TEST_COSTS:-}"
case "$costs" in "" | /*) ;; *) costs="$root/$costs" ;; esac
test_binary="${HIRSEL_TEST_BINARY:-}"
unset HIRSEL_TEST_TMPDIR HIRSEL_TEST_CWD HIRSEL_TEST_BIN_EXES HIRSEL_TEST_BIN_EXE_DIR HIRSEL_TEST_COSTS HIRSEL_TEST_POOL HIRSEL_TEST_RELOCATE
unset HIRSEL_TEST_BINARY
full_gate=false
ignored=false
for argument in "$@"; do
  [ "$argument" != full_gate_ ] || full_gate=true
  [ "$argument" != --ignored ] || ignored=true
done
inventory=false
if "$full_gate" && "$ignored" && [ -n "$test_binary" ]; then inventory=true; fi
if [ -z "$shard" ] && ! "$inventory"; then exec "$harness" "$@"; fi
set -f # Rust case names and declared patterns are words, never shell globs.
# Value-taking options from the pinned libtest --help. Keep values quoted,
# even when they contain spaces or look like flags.
takes_value() {
  case "$1" in
    --logfile | --test-threads | --skip | --color | --format | --shuffle-seed | -Z) return 0 ;;
    *) return 1 ;;
  esac
}
list_cases() (
  remaining=$#
  set -- "$@" --list --format terse --color never
  literal=false
  while [ "$remaining" -gt 0 ]; do
    argument=$1; shift; remaining=$((remaining - 1))
    if "$literal"; then set -- "$@" "$argument"; continue; fi
    case "$argument" in
      --) literal=true; set -- "$@" -- ;;
      --list | --quiet | -q | --format=* | --color=* | --logfile=*) ;;
      --format | --color | --logfile)
        [ "$remaining" -gt 0 ] || { echo "$argument requires a value" >&2; exit 2; }
        shift; remaining=$((remaining - 1)) ;;
      *)
        set -- "$@" "$argument"
        if takes_value "$argument"; then
          [ "$remaining" -gt 0 ] || { echo "$argument requires a value" >&2; exit 2; }
          value=$1; shift; remaining=$((remaining - 1)); set -- "$@" "$value"
        fi ;;
    esac
  done
  listing="$("$harness" "$@")" || exit $?
  printf '%s\n' "$listing" | sed -n 's/: test$//p' | LC_ALL=C sort
)
cases="$(list_cases "$@")"
if "$inventory"; then
  printf 'HIRSEL_FULL_GATE_BINARY %s\n' "$test_binary"
  if [ -n "$cases" ]; then
    for test_case in $cases; do
      printf 'HIRSEL_FULL_GATE_EXPECT %s %s\n' "$test_binary" "$test_case"
    done
  fi
fi
if [ -n "$shard" ]; then
  index="${shard%/*}"
  count="${shard#*/}"
  # Positional arguments select cases; flags and their values never do, so
  # e.g. `-Z unstable-options --report-time` keeps every shard running.
  filtered=false
  skip=false
  literal=false
  for argument in "$@"; do
    if "$skip"; then skip=false; continue; fi
    if "$literal"; then filtered=true; continue; fi
    case "$argument" in
      --) literal=true ;;
      -*) if takes_value "$argument"; then skip=true; fi ;;
      *) filtered=true ;;
    esac
  done
  if "$filtered"; then
    [ "$index" -eq 1 ] || { echo "shard $shard: arguments select the cases; shard 1 runs them"; exit 0; }
  else
    # Longest-first greedy assignment by measured seconds (HIRSEL_TEST_COSTS:
    # `case<TAB>seconds` rows, tools/buck2/test_costs.py). A case without a
    # row costs the mean of the measured ones (1 without any), so equal costs
    # deal cases out in name order. Every shard computes the same assignment.
    cases="$(printf '%s\n' "$cases" | awk -F '\t' -v costs="${costs:-/dev/null}" '
      BEGIN { while ((getline row < costs) > 0) { split(row, f, "\t"); cost[f[1]] = f[2] + 0; sum += f[2]; n++ } }
      NF { name[++m] = $0 }
      END { mean = n ? sum / n : 1; for (i = 1; i <= m; i++) printf "%.3f\t%s\n", (name[i] in cost) ? cost[name[i]] : mean, name[i] }' |
      LC_ALL=C sort -t "$(printf '\t')" -k1,1nr -k2,2 |
      awk -F '\t' -v index_="$index" -v count="$count" '{
        best = 1; for (s = 2; s <= count; s++) if (load[s] < load[best]) best = s
        load[best] += $1; if (best == index_) print $2 }' | LC_ALL=C sort)"
  fi
fi
# The caller's positive filters already shaped the listing. Replace them with
# the exact selected names, preserving flags and paired values. In particular,
# an empty partition must use an empty exact filter: no filters would run all.
remaining=$#
if [ -n "$cases" ]; then
  # shellcheck disable=SC2086 # Rust case names contain no spaces; globbing is off.
  set -- "$@" --exact $cases
else
  set -- "$@" --exact ""
fi
literal=false
while [ "$remaining" -gt 0 ]; do
  argument=$1; shift; remaining=$((remaining - 1))
  if "$literal"; then continue; fi
  case "$argument" in
    --) literal=true ;;
    --exact) ;;
    -*)
      set -- "$@" "$argument"
      if takes_value "$argument"; then
        [ "$remaining" -gt 0 ] || { echo "$argument requires a value" >&2; exit 2; }
        value=$1; shift; remaining=$((remaining - 1)); set -- "$@" "$value"
      fi ;;
  esac
done
exec "$harness" "$@"

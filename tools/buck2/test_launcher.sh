#!/bin/sh
# Run one libtest harness (or gate command) as `cargo test` would: from its
# package directory (HIRSEL_TEST_CWD), with an existing CARGO_TARGET_TMPDIR,
# the pinned LLVM as LLVM_SYS_231_PREFIX (its bin first on PATH, for tests that
# run `clang` by name, and its lib on the loader path), the runtime archives as
# absolute HIRSEL_RUNTIME_ARCHIVES, insta's snapshots under the project root,
# and test threads and campaign workers bounded by the action's CPU share (at
# most 4 unless the target declares an exact CPU requirement).
#
# TEST_SHARD_INDEX=<i> of TEST_TOTAL_SHARDS=<n> (Bazel's variables, i from 0)
# runs the i-th of n cost-balanced parts of the listed cases, weighted by
# HIRSEL_TEST_COSTS when it names a cost table. Given a filter or a value
# argument other than `--skip`, every shard but the first runs nothing, and
# the first runs the whole selection.
set -eu
if [ -n "${HIRSEL_TEST_REPORTER:-}" ] && [ -z "${HIRSEL_TEST_REPORT_ACTIVE:-}" ]; then
  exec python3 "$HIRSEL_TEST_REPORTER" sh "$0" "$@"
fi
root="$(pwd)"
# Keep SDK copies on the executor's original scratch filesystem, before the
# recording tests select their ancestry-safe /dev/shm TMPDIR.
native_inputs_tmp="${TMPDIR:-/tmp}"
# A test runs the CLI in its own process: never on, or by starting, the
# workspace's resident compiler (crates/hirsel-cli/src/serve.rs).
export HIRSEL_DAEMON=off
export HIRSEL_RESULTS=none@1
if [ -n "${HIRSEL_TEST_POOL:-}" ]; then
  # A bless rewrites the files the test reads. A pool action reads a copy of
  # the checkout that is discarded after the run, so a bless there would
  # change nothing and could still pass. tools/bless.sh runs blesses on this host.
  bless=""
  [ -z "${HIRSEL_BLESS:-}" ] || bless="HIRSEL_BLESS=$HIRSEL_BLESS"
  case "${INSTA_UPDATE:-}" in always | 1 | force | new | unseen) bless="INSTA_UPDATE=$INSTA_UPDATE" ;; esac
  for argument in "$@"; do
    case "$argument" in --bless) bless="$argument" ;; esac
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
# Every action owns its writable caches, including on a host with a populated
# home. Share them among its test threads/CLI children, never between actions.
# mktemp creates a unique mode-0700 directory (Go os.MkdirTemp's ownership
# pattern); the launcher stays alive so its exit trap also runs on test failure.
action_cache="$(mktemp -d "$scratch/action-XXXXXXXXXX")"
action_cache="$(cd "$action_cache" && pwd)"
private_native_inputs=""
native_input_materializer=""
trap 'rm -rf "$action_cache"; [ -z "$private_native_inputs" ] || rm -rf "$private_native_inputs"' 0
trap 'exit 129' HUP
trap 'exit 130' INT
trap 'exit 143' TERM
export XDG_CACHE_HOME="$action_cache/cache"
export HIRSEL_RESULTS_DIR="$action_cache/results"
export HIRSEL_NATIVE_CACHE="$action_cache/runtime"
mkdir -m 700 "$XDG_CACHE_HOME"
LLVM_SYS_231_PREFIX="$(cd "$LLVM_SYS_231_PREFIX" && pwd)"
export LLVM_SYS_231_PREFIX
if [ -n "${HIRSEL_NATIVE_ENVIRONMENT:-}" ]; then
  # The shared CAS inode's ctime may change when another action creates or
  # removes a hard link. Detach every tool/shared-library file before strict
  # native hashing; preserve private aliases and the existing input bytes.
  native_input_materializer="$(realpath "${HIRSEL_PRIVATE_NATIVE_INPUTS:?missing native input materializer}")"
  unset HIRSEL_PRIVATE_NATIVE_INPUTS
  # Each row of tools/buck2/native-toolchain/environment.tsv is a variable, the
  # toolchain whose tree HIRSEL_TOOLCHAIN_<TARGET> names, and a path inside it.
  trees=""
  while IFS="$(printf '\t')" read -r variable target path; do
    case "$variable" in "" | "#"*) continue ;; esac
    name="HIRSEL_TOOLCHAIN_$(printf '%s' "$target" | tr '[:lower:]-' '[:upper:]_')"
    eval "tree=\${$name}"
    export "$variable=$(cd "$tree" && pwd)/$path"
    trees="$trees $name"
  done < "$HIRSEL_NATIVE_ENVIRONMENT"
  unset HIRSEL_NATIVE_ENVIRONMENT $trees
fi
# Listing and empty shards never execute native work, so do not copy an SDK.
materialize_native_inputs() {
  [ -n "$native_input_materializer" ] || return 0
  for argument in "$@"; do
    case "$argument" in --list | --help | -h) return 0 ;; esac
  done
  private_native_inputs="$(mktemp -d "$native_inputs_tmp/hirsel-native-inputs-XXXXXXXXXX")"
  LLVM_SYS_231_PREFIX="$(python3 "$native_input_materializer" "$LLVM_SYS_231_PREFIX" "$private_native_inputs/llvm")"
  export LLVM_SYS_231_PREFIX
  export PATH="$LLVM_SYS_231_PREFIX/bin:$PATH"
  export LD_LIBRARY_PATH="$LLVM_SYS_231_PREFIX/lib${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
}
export PATH="$LLVM_SYS_231_PREFIX/bin:${PATH:-/usr/bin:/bin}"
export LD_LIBRARY_PATH="$LLVM_SYS_231_PREFIX/lib${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
if [ -n "${HIRSEL_RUNTIME_ARCHIVES:-}" ]; then
  HIRSEL_RUNTIME_ARCHIVES="$(cd "$HIRSEL_RUNTIME_ARCHIVES" && pwd)"
  export HIRSEL_RUNTIME_ARCHIVES
fi
if [ -n "${HIRSEL_TEST_PRODUCTS:-}" ]; then
  HIRSEL_TEST_PRODUCTS="$(realpath "$HIRSEL_TEST_PRODUCTS")"
  export HIRSEL_TEST_PRODUCTS
fi
if [ -n "${HIRSEL_PRODUCTS:-}" ]; then
  HIRSEL_PRODUCTS="$(realpath "$HIRSEL_PRODUCTS")"
  export HIRSEL_PRODUCTS
fi

if [ -n "${HIRSEL_NATIVE_SELECTION:-}" ]; then
  HIRSEL_NATIVE_SELECTION="$(realpath "$HIRSEL_NATIVE_SELECTION")"
  export HIRSEL_NATIVE_SELECTION
fi
export INSTA_WORKSPACE_ROOT="$root"
# defs.bzl passes the executor's effective CPU reservation explicitly: a
# worker's cgroup files need not be visible inside its action sandbox. Never
# infer a pool reservation from the worker host's CPU count.
jobs="${HIRSEL_TEST_CPUS:-}"
case "$jobs" in
  "")
    if [ -n "${HIRSEL_TEST_POOL:-}" ]; then
      echo "test_launcher.sh: pool action requires HIRSEL_TEST_CPUS" >&2
      exit 2
    fi
    jobs="$(nproc 2>/dev/null || echo 1)" ;;
  *[!0-9]* | 0) echo "test_launcher.sh: invalid HIRSEL_TEST_CPUS=$jobs" >&2; exit 2 ;;
esac
# A visible, tighter cgroup quota and caller settings can only reduce it.
exact_cpus="${HIRSEL_TEST_EXACT_CPUS:-}"
if [ -n "$exact_cpus" ]; then
  if [ "$exact_cpus" != "$jobs" ]; then
    echo "test_launcher.sh: exact CPU request differs from reservation" >&2
    exit 2
  fi
  # Validate inherited visibility, never expand it. The worker owns affinity.
  visible=$(awk '/^Cpus_allowed_list:/ {
    n=split($2, parts, ","); total=0
    for (i=1; i<=n; i++) {
      k=split(parts[i], ends, "-")
      total += k == 1 ? 1 : ends[2]-ends[1]+1
    }
    print total
  }' /proc/self/status)
  if [ "$visible" != "$exact_cpus" ]; then
    echo "test_launcher.sh: exact CPU request $exact_cpus has affinity width $visible" >&2
    exit 2
  fi
else
  [ "$jobs" -le 4 ] || jobs=4
fi
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
if [ -n "$exact_cpus" ] && [ "$jobs" != "$exact_cpus" ]; then
  echo "test_launcher.sh: exact CPU request $exact_cpus exceeds inherited quota ($jobs)" >&2
  exit 2
fi
# The run's memory grant (defs.bzl) caps its data before anything runs, so
# scripts and the native programs a test launches are capped too; hirsel
# binaries set the same limit again and name it when they reach it. The limit
# is soft: only a sanitizer's child lifts it (AddressSanitizer maps terabytes
# of shadow address space), and the pool's reservation still bounds that run.
if [ -n "${HIRSEL_MEMORY_LIMIT:-}" ]; then
  case "$HIRSEL_MEMORY_LIMIT" in
    *K) limit_kb="${HIRSEL_MEMORY_LIMIT%K}" ;;
    *M) limit_kb="$((${HIRSEL_MEMORY_LIMIT%M} * 1024))" ;;
    *G) limit_kb="$((${HIRSEL_MEMORY_LIMIT%G} * 1024 * 1024))" ;;
    *) limit_kb="$((HIRSEL_MEMORY_LIMIT / 1024))" ;;
  esac
  ulimit -S -d "$limit_kb"
fi
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
# Value-taking options from the pinned libtest --help. Keep values quoted,
# even when they contain spaces or look like flags.
takes_value() {
  case "$1" in
    --logfile | --test-threads | --skip | --color | --format | --shuffle-seed | -Z) return 0 ;;
    *) return 1 ;;
  esac
}
# libtest's command-line setting overrides RUST_TEST_THREADS. Bound both
# spellings too, preserving all other arguments and literal filters after --.
remaining=$#
literal=false
while [ "$remaining" -gt 0 ]; do
  argument=$1; shift; remaining=$((remaining - 1))
  if "$literal"; then set -- "$@" "$argument"; continue; fi
  case "$argument" in
    --) literal=true ;;
    --test-threads | --test-threads=*)
      case "$argument" in
        --test-threads)
          [ "$remaining" -gt 0 ] || { echo "$argument requires a value" >&2; exit 2; }
          threads=$1; shift; remaining=$((remaining - 1)) ;;
        *) threads=${argument#*=} ;;
      esac
      case "$threads" in
        "" | *[!0-9]* | 0) echo "test_launcher.sh: invalid --test-threads=$threads" >&2; exit 2 ;;
      esac
      [ "$threads" -le "$jobs" ] || threads=$jobs
      set -- "$@" --test-threads "$threads"
      continue ;;
  esac
  set -- "$@" "$argument"
  if takes_value "$argument" && [ "$remaining" -gt 0 ]; then
    value=$1; shift; remaining=$((remaining - 1)); set -- "$@" "$value"
  fi
done
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
# Only Rust harness runs use the libtest event protocol. Discovery and help
# retain their own formats, and scripts keep their original argument surface.
run_harness() (
  if [ -n "$test_binary" ] && [ -n "${HIRSEL_TEST_REPORT_ACTIVE:-}" ]; then
    for argument in "$@"; do
      case "$argument" in --list | --help | -h) "$harness" "$@"; exit $? ;; esac
    done
    remaining=$#
    literal=false
    set -- "$@" -Zunstable-options --format=json
    while [ "$remaining" -gt 0 ]; do
      argument=$1; shift; remaining=$((remaining - 1))
      if "$literal"; then set -- "$@" "$argument"; continue; fi
      case "$argument" in
        --) literal=true; set -- "$@" -- ;;
        --format=* | --quiet | -q | -Zunstable-options) ;;
        -Z)
          [ "$remaining" -gt 0 ] || { echo "$argument requires a value" >&2; exit 2; }
          value=$1; shift; remaining=$((remaining - 1))
          [ "$value" = unstable-options ] || set -- "$@" -Z "$value" ;;
        --format)
          [ "$remaining" -gt 0 ] || { echo "$argument requires a value" >&2; exit 2; }
          shift; remaining=$((remaining - 1)) ;;
        *)
          set -- "$@" "$argument"
          if takes_value "$argument" && [ "$remaining" -gt 0 ]; then
            value=$1; shift; remaining=$((remaining - 1)); set -- "$@" "$value"
          fi ;;
      esac
    done
    export RUSTC_BOOTSTRAP=1
  fi
  "$harness" "$@"
)
shard=""
if [ -n "${TEST_TOTAL_SHARDS:-}" ]; then shard="$((TEST_SHARD_INDEX + 1))/$TEST_TOTAL_SHARDS"; fi
costs="${HIRSEL_TEST_COSTS:-}"
case "$costs" in "" | /*) ;; *) costs="$root/$costs" ;; esac
# Keep per-case costs in ordinary gate output, including an unsharded binary
# that has a cost table. Libtest times each case, not the surrounding launcher.
if [ -n "$costs" ]; then
  export RUSTC_BOOTSTRAP=1
  report_time=false
  for argument in "$@"; do
    case "$argument" in --report-time | --report-time=*) report_time=true ;; esac
  done
  if ! "$report_time"; then set -- "$@" -Zunstable-options --report-time; fi
fi
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
set -f # Rust case names and declared patterns are words, never shell globs.
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
# --ignored can legitimately select only host-disabled bodies. Distinguish
# that skip from a name absent from the binary, using libtest's full listing.
selection_exists() (
  remaining=$#
  literal=false
  while [ "$remaining" -gt 0 ]; do
    argument=$1; shift; remaining=$((remaining - 1))
    if "$literal"; then set -- "$@" "$argument"; continue; fi
    case "$argument" in
      --) literal=true ;;
      --ignored) continue ;;
    esac
    set -- "$@" "$argument"
    if takes_value "$argument" && [ "$remaining" -gt 0 ]; then
      value=$1; shift; remaining=$((remaining - 1)); set -- "$@" "$value"
    fi
  done
  [ -n "$(list_cases "$@")" ]
)
# Discover positive selectors before deciding which shard owns them. A stale
# selector must fail on shard 1, rather than becoming an empty exact filter.
filtered=false
restricted=false
skip=false
literal=false
discovery=false
for argument in "$@"; do
  if "$skip"; then skip=false; continue; fi
  if "$literal"; then filtered=true; continue; fi
  case "$argument" in
    --) literal=true ;;
    --list | --help | -h) discovery=true ;;
    --skip) restricted=true; skip=true ;;
    --skip=*) restricted=true ;;
    -*) if takes_value "$argument"; then skip=true; fi ;;
    *) filtered=true ;;
  esac
done
if "$discovery" && ! "$inventory" && [ -z "$shard" ] && [ -z "${HIRSEL_TEST_DEDICATED_CASE:-}" ]; then run_harness "$@"; exit $?; fi
if [ -z "$shard" ] && ! "$inventory" && [ -z "${HIRSEL_TEST_DEDICATED_CASE:-}" ]; then
  if [ -n "$test_binary" ] && { "$filtered" || "$restricted"; }; then
    cases="$(list_cases "$@")"
    if [ -z "$cases" ] && ! selection_exists "$@"; then
      printf 'test_launcher.sh: selector [%s] matched no test bodies\n' "$*" >&2
      exit 2
    fi
  fi
  materialize_native_inputs "$@"; run_harness "$@"; exit $?
fi
cases="$(list_cases "$@")"
if [ -z "$cases" ] && { "$filtered" || "$restricted"; } && ! "$discovery" && ! selection_exists "$@"; then
  printf 'test_launcher.sh: selector [%s] matched no test bodies\n' "$*" >&2
  exit 2
fi
if [ -n "${HIRSEL_TEST_DEDICATED_CASE:-}" ]; then
  if ! "$discovery" && [ -z "$(list_cases --exact "$HIRSEL_TEST_DEDICATED_CASE")" ]; then
    printf 'test_launcher.sh: selector [%s] matched no test bodies\n' "$HIRSEL_TEST_DEDICATED_CASE" >&2
    exit 2
  fi
  cases="$(printf '%s\n' "$cases" | awk -v selected="$HIRSEL_TEST_DEDICATED_CASE" '$0 == selected')"
fi
unset HIRSEL_TEST_DEDICATED_CASE
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
# Deal every original case before removing isolated bodies, so unrelated
# partitions keep their cases. Explicit selections still run the named body.
if [ -n "${HIRSEL_TEST_ISOLATED_CASES:-}" ] && [ -n "$shard" ] && ! "$filtered"; then
  cases="$(printf '%s\n' "$cases" | awk -v isolated="$HIRSEL_TEST_ISOLATED_CASES" '
    BEGIN { n = split(isolated, names, " "); for (i = 1; i <= n; i++) excluded[names[i]] = 1 }
    !($0 in excluded)')"
fi
unset HIRSEL_TEST_ISOLATED_CASES
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
[ -z "$cases" ] || materialize_native_inputs "$@"
run_harness "$@"

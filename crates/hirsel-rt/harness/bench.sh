#!/usr/bin/env bash
# Time the binarytrees C port against every allocator:
#   glibc, mimalloc, jemalloc   the malloc build (16-byte nodes), the latter two by LD_PRELOAD;
#   mimalloc-48B                the malloc build with nodes padded to 48 bytes on mimalloc, a
#                               control at the prior block size (32-byte header, 16-byte niche
#                               payload) that separates bytes per node from allocator code;
#   mimalloc-32B                the same at 32 bytes, another retained byte-size control;
#   hirsel-walk                 the hirsel_rt_v2 build on the size-class allocator, a tree freed
#                               by the runtime's walk of the descriptor's step program;
#   hirsel                      the same, a tree freed by the drop glue hirsel-llvm compiles for
#                               the node layout (the object hirsel-llvm's tests/glue.rs writes), which
#                               is what compiled Hirsel runs.
# C is built with the pinned LLVM's clang -O3 -ffp-contract=off. The runtime is the default
# Buck2 runtime archive (`//:hirsel-rt-archives`: release, aborting panics), the one compiled
# Hirsel links. There is no cross-language LTO.
# Each depth runs `rounds` rounds; a round runs every variant once in shuffled order, so the samples
# are paired on a shared host. It retains every sample's cycles, instructions, L1 and demand
# L2 misses, wall time and peak RSS. L2 uses the AMD host event 0x64/0x9. Not a gate.
# HIRSEL_BENCH_DATA selects the evidence directory; HIRSEL_BENCH_CPU pins the processes.
# HIRSEL_BENCH_BASELINE adds a frozen prior compiled executable; paired ratios use whole rounds.
# HIRSEL_BENCH_PREBUILT=1 measures existing binaries without rebuilding them.
# Usage: bench.sh [rounds=5] [depth...]   (depths default to the design's 8 12 16, and 21)
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
root="$(cd "$here/../../.." && pwd)"
rounds="${1:-5}"
shift || true
depths=("$@")
((${#depths[@]})) || depths=(8 12 16 21)
llvm="$root/tools/buck2/toolchains/llvm-files"
export PATH="$llvm/bin:$PATH" LD_LIBRARY_PATH="$llvm/lib${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
buck2="$root/.buck2/bin/buck2"
build="$root/buck-out/tmp/rt-bench"
mkdir -p "$build"
if [[ "${HIRSEL_BENCH_PREBUILT:-0}" != 1 ]]; then
  python3 "$root/tools/buck2/sync.py" --if-stale
  archives="$(cd "$root" && "$buck2" build //:hirsel-rt-archives --show-full-simple-output)"
  cflags=(-std=c11 -O3 -ffp-contract=off -Wall -Wextra -Werror)
  clang "${cflags[@]}" "$here/binarytrees.c" -o "$build/bt-malloc"
  clang "${cflags[@]}" -DNODE_BYTES=48 "$here/binarytrees.c" -o "$build/bt-malloc-48"
  clang "${cflags[@]}" -DNODE_BYTES=32 "$here/binarytrees.c" -o "$build/bt-malloc-32"
  libs=("$archives/x86_64-unknown-linux-gnu/default/libhirsel_rt.a" -lgcc_s -lutil -lrt -lpthread -lm -ldl)
  clang "${cflags[@]}" -DHIRSEL_RT -I "$here/../include" "$here/binarytrees.c" "${libs[@]}" -o "$build/bt-hirsel-walk"
  if [[ -n "${HIRSEL_BENCH_GLUE_OBJECT:-}" ]]; then
    cp "$HIRSEL_BENCH_GLUE_OBJECT" "$build/glue.o"
    cp "${HIRSEL_BENCH_GLUE_OBJECT%.o}.binding.h" "$build/glue.binding.h"
  else
    # On this host, so the test writes the object here.
    (cd "$root" && "$buck2" test -c hirsel.tests=host //:hirsel-llvm-test-it -- \
      --env HIRSEL_GLUE_OBJECT="$build/glue.o" --test-arg=glue::the_tree_root_is_the_binarytrees_node --test-arg=--exact >/dev/null)
  fi
  clang "${cflags[@]}" -DHIRSEL_RT "-DHIRSEL_GLUE_BINDING_HEADER=\"$build/glue.binding.h\"" -I "$here/../include" "$here/binarytrees.c" \
    "$build/glue.o" "${libs[@]}" -o "$build/bt-hirsel"
fi
python3 "$here/bench.py" "$build" "$rounds" "${depths[@]}"

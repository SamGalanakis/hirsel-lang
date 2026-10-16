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
# C is built with clang -O3 -ffp-contract=off. The runtime is a release static library
# with one codegen unit, fat LTO and aborting panics: a whole-library build, as mimalloc's
# single-unit build is. There is no cross-language LTO.
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
target_dir="$(cargo metadata --format-version 1 --no-deps --manifest-path "$root/Cargo.toml" |
  sed -n 's/.*"target_directory":"\([^"]*\)".*/\1/p')"
build="$target_dir/rt-bench"
mkdir -p "$build"
if [[ "${HIRSEL_BENCH_PREBUILT:-0}" != 1 ]]; then
  CARGO_PROFILE_RELEASE_PANIC=abort CARGO_PROFILE_RELEASE_CODEGEN_UNITS=1 CARGO_PROFILE_RELEASE_LTO=fat \
    cargo build --quiet --release -p hirsel-rt --lib --manifest-path "$root/Cargo.toml" \
    --target-dir "$build/cargo"
  cflags=(-std=c11 -O3 -ffp-contract=off -Wall -Wextra -Werror)
  clang "${cflags[@]}" "$here/binarytrees.c" -o "$build/bt-malloc"
  clang "${cflags[@]}" -DNODE_BYTES=48 "$here/binarytrees.c" -o "$build/bt-malloc-48"
  clang "${cflags[@]}" -DNODE_BYTES=32 "$here/binarytrees.c" -o "$build/bt-malloc-32"
  libs=("$build/cargo/release/libhirsel_rt.a" -lgcc_s -lutil -lrt -lpthread -lm -ldl)
  clang "${cflags[@]}" -DHIRSEL_RT -I "$here/../include" "$here/binarytrees.c" "${libs[@]}" -o "$build/bt-hirsel-walk"
  # From the workspace root, whose .cargo/config.toml names the pinned LLVM.
  if [[ -n "${HIRSEL_BENCH_GLUE_OBJECT:-}" ]]; then
    cp "$HIRSEL_BENCH_GLUE_OBJECT" "$build/glue.o"
    cp "${HIRSEL_BENCH_GLUE_OBJECT%.o}.binding.h" "$build/glue.binding.h"
  else
    (cd "$root" && HIRSEL_GLUE_OBJECT="$build/glue.o" cargo test --quiet -p hirsel-llvm --test it glue:: \
      the_tree_root_is_the_binarytrees_node >/dev/null)
  fi
  clang "${cflags[@]}" -DHIRSEL_RT "-DHIRSEL_GLUE_BINDING_HEADER=\"$build/glue.binding.h\"" -I "$here/../include" "$here/binarytrees.c" \
    "$build/glue.o" "${libs[@]}" -o "$build/bt-hirsel"
fi
python3 "$here/bench.py" "$build" "$rounds" "${depths[@]}"

#!/bin/bash
# Run the tools with positive output checks and a deliberately leaking twin.
set -euo pipefail
llvm=$(realpath "$1")
arm=$(realpath "$2")
qemu=$(realpath "$3")
valgrind=$(realpath "$4")
export LD_LIBRARY_PATH="$llvm/lib"
export VALGRIND_LIB="$valgrind/libexec/valgrind"
build=$(mktemp -d)
trap 'rm -rf "$build"' EXIT
fail() { echo "native toolchain proof: $*" >&2; exit 1; }
version=$("$llvm/bin/llvm-config" --version)
clang_version=$("$llvm/bin/clang" --version)
[[ "$clang_version" == *"clang version $version "* ]] || fail "clang version differs from the pinned LLVM"
printf '%s\n' "$clang_version"
lld_version=$("$llvm/bin/ld.lld" --version)
[[ "$lld_version" == *"LLD $version "* ]] || fail "lld version differs from the pinned LLVM"
printf '%s\n' "$lld_version"
cat > "$build/tiny.c" <<'C'
#include <stdio.h>
#include <stdlib.h>
static void *volatile allocation;
int main(void) {
  allocation = malloc(32);
  if (!allocation) return 2;
  *(volatile unsigned char *)allocation = 7;
#ifndef PROOF_LEAK
  free(allocation);
#endif
  allocation = NULL;
  puts("native-toolchain-smoke");
  return 0;
}
C
link_native() {
  "$llvm/bin/clang" --ld-path="$llvm/bin/ld.lld" -O0 -g "$build/tiny.c" \
    -Xlinker --dynamic-linker -Xlinker "$valgrind/lib/x86_64-linux-gnu/ld-linux-x86-64.so.2" \
    -Xlinker -rpath -Xlinker "$valgrind/lib/x86_64-linux-gnu" "$@"
}
link_native -o "$build/tiny"
[[ "$("$build/tiny")" == native-toolchain-smoke ]] || fail "the lld-linked executable did not print its marker"
echo 'x86-64: linked with input lld and printed native-toolchain-smoke'
"$arm/bin/aarch64-none-linux-gnu-gcc" -static -O0 -g "$build/tiny.c" -o "$build/arm"
[[ "$("$qemu/qemu-aarch64" "$build/arm")" == native-toolchain-smoke ]] || fail "the AArch64 qemu run did not print its marker"
echo 'AArch64: cross-compiled executable printed native-toolchain-smoke under input qemu'
memcheck=("$valgrind/bin/valgrind" --error-exitcode=97 --leak-check=full --errors-for-leak-kinds=definite)
"${memcheck[@]}" "$build/tiny" > "$build/good.out" 2> "$build/good.err"
[[ "$(cat "$build/good.out")" == native-toolchain-smoke ]] || fail "Valgrind did not run the accepted executable"
grep -Fq 'ERROR SUMMARY: 0 errors from 0 contexts' "$build/good.err" || { cat "$build/good.err" >&2; fail "Valgrind did not report zero errors"; }
grep -Fq 'in use at exit: 0 bytes in 0 blocks' "$build/good.err" || fail "the accepted executable leaked"
cat "$build/good.err"
link_native -DPROOF_LEAK -o "$build/leak"
status=0
"${memcheck[@]}" "$build/leak" > "$build/leak.out" 2> "$build/leak.err" || status=$?
[[ "$status" == 97 ]] || { cat "$build/leak.err" >&2; fail "the planted leak exited $status instead of 97"; }
[[ "$(cat "$build/leak.out")" == native-toolchain-smoke ]] || fail "Valgrind did not run the leaking twin"
grep -Fq 'definitely lost: 32 bytes in 1 blocks' "$build/leak.err" || { cat "$build/leak.err" >&2; fail "Valgrind did not find the planted 32-byte leak"; }
grep -Eq 'ERROR SUMMARY: [1-9][0-9]* errors' "$build/leak.err" || fail "Valgrind did not count the planted leak as an error"
cat "$build/leak.err"
echo 'native toolchain proof: clang, lld, cross gcc, qemu and Valgrind passed; planted leak refused with exit 97'

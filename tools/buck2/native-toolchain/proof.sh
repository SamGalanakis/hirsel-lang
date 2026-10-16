#!/bin/bash
# The pool supplies its ordinary C runtime; all four native tools are inputs.
set -euo pipefail
llvm=$(realpath "$1")
arm=$(realpath "$2")
qemu=$(realpath "$3")
valgrind=$(realpath "$4")
export LD_LIBRARY_PATH="$llvm/lib"
export VALGRIND_LIB="$valgrind/libexec/valgrind"
build=$(mktemp -d)
trap 'rm -rf "$build"' EXIT
printf '#include <stdlib.h>\nint main(void) { void *p = malloc(32); if (!p) return 1; free(p); return 0; }\n' > "$build/tiny.c"
"$llvm/bin/clang" --version
"$llvm/bin/clang" --ld-path="$llvm/bin/ld.lld" "$build/tiny.c" \
  -Xlinker --dynamic-linker -Xlinker "$valgrind/lib/x86_64-linux-gnu/ld-linux-x86-64.so.2" \
  -Xlinker -rpath -Xlinker "$valgrind/lib/x86_64-linux-gnu" -o "$build/tiny"
"$build/tiny"
"$arm/bin/aarch64-none-linux-gnu-gcc" -static "$build/tiny.c" -o "$build/arm"
"$qemu/qemu-aarch64" "$build/arm"
"$valgrind/bin/valgrind" --extra-debuginfo-path="$valgrind/lib/debug" --error-exitcode=97 --leak-check=full "$build/tiny"
echo 'native toolchain proof: 4 tools passed'

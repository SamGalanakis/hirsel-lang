#!/usr/bin/env bash
# The library C probes against the diagnostic runtime archive: x86-64 under ASan/UBSan and
# AArch64 under qemu-user. The archives are Buck2's (`//:hirsel-rt-archives`):
# HIRSEL_RUNTIME_ARCHIVES names their directory, or this script builds them. C compiles with
# the pinned LLVM's clang.
set -euo pipefail
root="$(cd "$(dirname "$0")/../.." && pwd)"
llvm="$root/tools/buck2/toolchains/llvm-files"
export PATH="$llvm/bin:$PATH" LD_LIBRARY_PATH="$llvm/lib${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
archives="${HIRSEL_RUNTIME_ARCHIVES:-$(cd "$root" && .buck2/bin/buck2 build //:hirsel-rt-archives --show-full-simple-output)}"
x86_archive="$archives/x86_64-unknown-linux-gnu/diagnostic/libhirsel_rt.a"
aarch64_archive="$archives/aarch64-unknown-linux-gnu/diagnostic/libhirsel_rt.a"
build="$root/buck-out/tmp/library-c"
mkdir -p "$build"
libs=(-lgcc_s -lutil -lrt -lpthread -lm -ldl)
sanitize=(-std=c11 -DHIRSEL_RT_ENABLE_STATS -O1 -g -Wall -Wextra -Werror -fsanitize=address,undefined -fno-sanitize-recover=all)
cross=(-std=c11 -DHIRSEL_RT_ENABLE_STATS -O2 -Wall -Wextra -Werror)
library="$root/native-tests/library"
clang "${sanitize[@]}" "$library/views.c" "$x86_archive" "${libs[@]}" -o "$build/views"
ASAN_OPTIONS=detect_leaks=1 UBSAN_OPTIONS=halt_on_error=1 "$build/views" "$library/GraphemeBreakTest.txt"
aarch64-linux-gnu-gcc "${cross[@]}" "$library/views.c" "$aarch64_archive" "${libs[@]}" -o "$build/views-aarch64"
qemu-aarch64-static -L /usr/aarch64-linux-gnu "$build/views-aarch64" "$library/GraphemeBreakTest.txt"
clang "${sanitize[@]}" "$library/text.c" "$x86_archive" "${libs[@]}" -o "$build/text"
ASAN_OPTIONS=detect_leaks=1 UBSAN_OPTIONS=halt_on_error=1 "$build/text"
aarch64-linux-gnu-gcc "${cross[@]}" "$library/text.c" "$aarch64_archive" "${libs[@]}" -o "$build/text-aarch64"
qemu-aarch64-static -L /usr/aarch64-linux-gnu "$build/text-aarch64"
echo "library C probes: x86-64 ASan/UBSan and AArch64 passed"

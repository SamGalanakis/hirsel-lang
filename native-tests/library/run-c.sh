#!/usr/bin/env bash
# The library C probes against the diagnostic runtime archive: x86-64 under ASan/UBSan and
# AArch64 under qemu-user. The archives are Buck2's (`//:hirsel-rt-archives`):
# The Buck2 launcher supplies HIRSEL_RUNTIME_ARCHIVES and all native tool paths.
set -euo pipefail
root="$(cd "$(dirname "$0")/../.." && pwd)"
llvm="${LLVM_SYS_231_PREFIX:?LLVM_SYS_231_PREFIX must name the pinned input}"
: "${HIRSEL_AARCH64_CC:?}" "${HIRSEL_QEMU_AARCH64:?}" "${HIRSEL_AARCH64_SYSROOT:?}"
archives="${HIRSEL_RUNTIME_ARCHIVES:?HIRSEL_RUNTIME_ARCHIVES must name the Buck2 input}"
x86_archive="$archives/x86_64-unknown-linux-gnu/diagnostic/libhirsel_rt.a"
aarch64_archive="$archives/aarch64-unknown-linux-gnu/diagnostic/libhirsel_rt.a"
build="$(mktemp -d)"
trap 'rm -rf "$build"' EXIT
mkdir -p "$build"
libs=(-lgcc_s -lutil -lrt -lpthread -lm -ldl)
sanitize=(-std=c11 -DHIRSEL_RT_ENABLE_STATS -O1 -g -Wall -Wextra -Werror -fsanitize=address,undefined -fno-sanitize-recover=all)
cross=(-std=c11 -DHIRSEL_RT_ENABLE_STATS -O2 -Wall -Wextra -Werror)
library="$root/native-tests/library"
"$llvm/bin/clang" --ld-path="$llvm/bin/ld.lld" "${sanitize[@]}" "$library/views.c" "$x86_archive" "${libs[@]}" -o "$build/views"
ASAN_OPTIONS=detect_leaks=1 UBSAN_OPTIONS=halt_on_error=1 "$build/views" "$library/GraphemeBreakTest.txt"
"$HIRSEL_AARCH64_CC" "${cross[@]}" "$library/views.c" "$aarch64_archive" "${libs[@]}" -o "$build/views-aarch64"
"$HIRSEL_QEMU_AARCH64" -L "$HIRSEL_AARCH64_SYSROOT" "$build/views-aarch64" "$library/GraphemeBreakTest.txt"
"$llvm/bin/clang" --ld-path="$llvm/bin/ld.lld" "${sanitize[@]}" "$library/text.c" "$x86_archive" "${libs[@]}" -o "$build/text"
ASAN_OPTIONS=detect_leaks=1 UBSAN_OPTIONS=halt_on_error=1 "$build/text"
"$HIRSEL_AARCH64_CC" "${cross[@]}" "$library/text.c" "$aarch64_archive" "${libs[@]}" -o "$build/text-aarch64"
"$HIRSEL_QEMU_AARCH64" -L "$HIRSEL_AARCH64_SYSROOT" "$build/text-aarch64"
echo "library C probes: x86-64 ASan/UBSan and AArch64 passed"

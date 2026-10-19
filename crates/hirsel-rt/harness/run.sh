#!/usr/bin/env bash
# Link the standalone C harness against the hirsel_rt_v2 diagnostic runtime archive and run it
# under AddressSanitizer/UndefinedBehaviorSanitizer (LeakSanitizer included) and on AArch64 under the input qemu. The archives are Buck2's
# (`//:hirsel-rt-archives`, `<dir>/<triple>/<features>/libhirsel_rt.a`, aborting panics):
# The Buck2 launcher supplies HIRSEL_RUNTIME_ARCHIVES and all native tool paths.
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
llvm="${LLVM_SYS_211_PREFIX:?LLVM_SYS_211_PREFIX must name the pinned input}"
: "${HIRSEL_AARCH64_CC:?}" "${HIRSEL_QEMU_AARCH64:?}" "${HIRSEL_AARCH64_SYSROOT:?}"
libs=(-lgcc_s -lutil -lrt -lpthread -lm -ldl)
archives="${HIRSEL_RUNTIME_ARCHIVES:?HIRSEL_RUNTIME_ARCHIVES must name the Buck2 input}"
build="$(mktemp -d)"
trap 'rm -rf "$build"' EXIT
x86_archive="$archives/x86_64-unknown-linux-gnu/diagnostic/libhirsel_rt.a"
aarch64_archive="$archives/aarch64-unknown-linux-gnu/diagnostic/libhirsel_rt.a"
mkdir -p "$build"

echo "harness: x86-64 with ASan and UBSan"
"$llvm/bin/clang" --ld-path="$llvm/bin/ld.lld" -std=c11 -DHIRSEL_RT_ENABLE_STATS -O1 -g -Wall -Wextra -Werror -fsanitize=address,undefined -fno-sanitize-recover=all \
-fno-omit-frame-pointer -I "$here/../include" "$here/harness.c" \
"$x86_archive" "${libs[@]}" -o "$build/harness-asan"
# AddressSanitizer maps terabytes of shadow address space, which the run's soft
# data limit (tools/buck2/test_launcher.sh) refuses: lift it for this child.
(ulimit -S -d "$(ulimit -H -d)"
ASAN_OPTIONS=detect_leaks=1:abort_on_error=0 UBSAN_OPTIONS=halt_on_error=1:print_stacktrace=1 \
"$build/harness-asan")

echo "harness: AArch64 under qemu-user"
"$HIRSEL_AARCH64_CC" -std=c11 -DHIRSEL_RT_ENABLE_STATS -O2 -Wall -Wextra -Werror -I "$here/../include" "$here/harness.c" \
    "$aarch64_archive" "${libs[@]}" \
    -o "$build/harness-aarch64"
"$HIRSEL_QEMU_AARCH64" -L "$HIRSEL_AARCH64_SYSROOT" "$build/harness-aarch64"

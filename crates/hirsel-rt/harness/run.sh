#!/usr/bin/env bash
# Link the standalone C harness against the hirsel_rt_v2 diagnostic runtime archive and run it
# under AddressSanitizer/UndefinedBehaviorSanitizer (LeakSanitizer included) and, when the
# cross toolchain is installed, on AArch64 under qemu-user. The archives are Buck2's
# (`//:hirsel-rt-archives`, `<dir>/<triple>/<features>/libhirsel_rt.a`, aborting panics):
# HIRSEL_RUNTIME_ARCHIVES names their directory, or this script builds them. C compiles with
# the pinned LLVM's clang (`python3 tools/buck2/prepare.py` installs it).
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
root="$(cd "$here/../../.." && pwd)"
llvm="$root/tools/buck2/toolchains/llvm-files"
export PATH="$llvm/bin:$PATH" LD_LIBRARY_PATH="$llvm/lib${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
libs=(-lgcc_s -lutil -lrt -lpthread -lm -ldl)
archives="${HIRSEL_RUNTIME_ARCHIVES:-$(cd "$root" && .buck2/bin/buck2 build //:hirsel-rt-archives --show-full-simple-output)}"
build="$root/buck-out/tmp/rt-harness"
x86_archive="$archives/x86_64-unknown-linux-gnu/diagnostic/libhirsel_rt.a"
aarch64_archive="$archives/aarch64-unknown-linux-gnu/diagnostic/libhirsel_rt.a"
mkdir -p "$build"

echo "harness: x86-64 with ASan and UBSan"
clang -std=c11 -DHIRSEL_RT_ENABLE_STATS -O1 -g -Wall -Wextra -Werror -fsanitize=address,undefined -fno-sanitize-recover=all \
  -fno-omit-frame-pointer -I "$here/../include" "$here/harness.c" \
  "$x86_archive" "${libs[@]}" -o "$build/harness-asan"
ASAN_OPTIONS=detect_leaks=1:abort_on_error=0 UBSAN_OPTIONS=halt_on_error=1:print_stacktrace=1 \
  "$build/harness-asan"

if command -v aarch64-linux-gnu-gcc >/dev/null && command -v qemu-aarch64-static >/dev/null; then
  echo "harness: AArch64 under qemu-user"
  aarch64-linux-gnu-gcc -std=c11 -DHIRSEL_RT_ENABLE_STATS -O2 -Wall -Wextra -Werror -I "$here/../include" "$here/harness.c" \
    "$aarch64_archive" "${libs[@]}" \
    -o "$build/harness-aarch64"
  qemu-aarch64-static -L /usr/aarch64-linux-gnu "$build/harness-aarch64"
else
  echo "harness: AArch64 skipped (aarch64-linux-gnu-gcc or qemu-aarch64-static missing)"
fi

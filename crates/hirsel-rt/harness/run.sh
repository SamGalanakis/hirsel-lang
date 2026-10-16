#!/usr/bin/env bash
# Build the hirsel_rt_v2 static library with aborting panics, then run the standalone C harness
# under AddressSanitizer/UndefinedBehaviorSanitizer (LeakSanitizer included) and, when the
# cross toolchain is installed, on AArch64 under qemu-user. With HIRSEL_RUNTIME_ARCHIVES (the
# Buck2 runtime archives, `<dir>/<triple>/<features>/libhirsel_rt.a`) it links those instead.
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
root="$(cd "$here/../../.." && pwd)"
libs=(-lgcc_s -lutil -lrt -lpthread -lm -ldl)
archives="${HIRSEL_RUNTIME_ARCHIVES:-}"
if [[ -n "$archives" ]]; then
  build="$root/buck-out/tmp/rt-harness"
  x86_archive="$archives/x86_64-unknown-linux-gnu/diagnostic/libhirsel_rt.a"
  aarch64_archive="$archives/aarch64-unknown-linux-gnu/diagnostic/libhirsel_rt.a"
else
  target_dir="$(cargo metadata --format-version 1 --no-deps --manifest-path "$root/Cargo.toml" |
    sed -n 's/.*"target_directory":"\([^"]*\)".*/\1/p')"
  build="$target_dir/rt-harness"
  x86_archive="$build/cargo/release/libhirsel_rt.a"
  aarch64_archive="$build/cargo/aarch64-unknown-linux-gnu/release/libhirsel_rt.a"
  export CARGO_PROFILE_RELEASE_PANIC=abort
  cargo build --quiet --release -p hirsel-rt --lib --features diagnostic --manifest-path "$root/Cargo.toml" --target-dir "$build/cargo"
fi
mkdir -p "$build"

echo "harness: x86-64 with ASan and UBSan"
clang -std=c11 -DHIRSEL_RT_ENABLE_STATS -O1 -g -Wall -Wextra -Werror -fsanitize=address,undefined -fno-sanitize-recover=all \
  -fno-omit-frame-pointer -I "$here/../include" "$here/harness.c" \
  "$x86_archive" "${libs[@]}" -o "$build/harness-asan"
ASAN_OPTIONS=detect_leaks=1:abort_on_error=0 UBSAN_OPTIONS=halt_on_error=1:print_stacktrace=1 \
  "$build/harness-asan"

if command -v aarch64-linux-gnu-gcc >/dev/null && command -v qemu-aarch64-static >/dev/null; then
  echo "harness: AArch64 under qemu-user"
  [[ -n "$archives" ]] || CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER=aarch64-linux-gnu-gcc \
    cargo build --quiet --release -p hirsel-rt --lib --features diagnostic --target aarch64-unknown-linux-gnu \
    --manifest-path "$root/Cargo.toml" --target-dir "$build/cargo"
  aarch64-linux-gnu-gcc -std=c11 -DHIRSEL_RT_ENABLE_STATS -O2 -Wall -Wextra -Werror -I "$here/../include" "$here/harness.c" \
    "$aarch64_archive" "${libs[@]}" \
    -o "$build/harness-aarch64"
  qemu-aarch64-static -L /usr/aarch64-linux-gnu "$build/harness-aarch64"
else
  echo "harness: AArch64 skipped (aarch64-linux-gnu-gcc or qemu-aarch64-static missing)"
fi

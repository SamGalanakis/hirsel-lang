# native-toolchain

The pinned LLVM for native code.

- `llvm-21.lock`: the pin, the only place it is written. LLVM 21.1.8 from the apt.llvm.org
  `llvm-toolchain-noble-21` packages at one exact version, with each `.deb`'s SHA-256 and
  size; the release tag, its commit and the source tarball's SHA-256 for a source build; and
  llvm-sys 211.0.1 with its crates.io checksum.
- `install`: prints the pinned apt commands; `--run` runs them as root; `--unpack DIR`
  verifies every `.deb` and unpacks it under DIR without root, then prints the
  `LLVM_SYS_211_PREFIX` and loader path that select it; `--source` prints the pinned source
  build with a shared libLLVM.
- `check [--prefix DIR]`: passes only on the exact pin. It rejects a wrong `llvm-config`
  major, minor, patch or suffix, a static-only install (no `libLLVM-21.so`), missing
  headers, a missing X86 or AArch64 target, a clang or lld of another version, an installed
  package at another version (system prefix only), and a Cargo.lock llvm-sys other than the
  pinned version and checksum. It then links `fixtures/version.c` against the shared
  libLLVM and requires it to report 21.1.8 at run time. It never searches PATH or falls back.
- `fixtures/`: `version.c` for `check`; `smoke.ll`, `smoke_harness.c` and `smoke.expected`,
  the FFI smoke that `crates/hirsel-llvm/tests/it/native.rs` compiles, links and runs.

The GitHub `LLVM-21.1.8-Linux-X64.tar.xz` is static-only and its `llvm-config
--system-libs` output breaks llvm-sys's build script; `check` rejects it.

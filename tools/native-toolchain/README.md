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

## Pool execution inputs

`BUCK` downloads LLVM's locked Debian packages and their runtime libraries,
Arm GNU 14.3.Rel1 (including its Linux sysroot), Ubuntu's static AArch64 qemu
and Valgrind 3.22.0. `native.lock.json` pins the additional archives by SHA-256
and byte size. `python3 tools/buck2/sync.py` generates `pins.bzl` from these
locks; `--check` rejects stale pins. Nothing is installed on the host or in the
worker image. Only unpacking runs locally, using `dpkg-deb`; it resolves
symlinks before Buck2 hashes the output directories. Execution uses read-only
action inputs inside the pool's networkless sandbox, with its private `/tmp`.

```sh
.buck2/bin/buck2 test //tools/native-toolchain:proof
```

The proof runs clang's version command, links and runs C with the pinned lld,
builds a static AArch64 executable and runs it directly with the input qemu
(no binfmt), then checks the C executable with the input Valgrind and
`VALGRIND_LIB`. It fails on any missing tool, link, launch or memcheck error.
The per-push native smoke stage includes this proof, so a pin or unpacking
change cannot land without exercising the tools on a worker.

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
and Valgrind 3.22.0 with a matching libc 2.39 loader and debug symbols. `native.lock.json` pins the additional archives by SHA-256
and byte size. `python3 tools/buck2/sync.py` generates `pins.bzl` from these
locks; `--check` rejects stale pins. Nothing is installed on the host or in the
worker image. Only unpacking runs locally, using `dpkg-deb`; it resolves
symlinks before Buck2 hashes the output directories. Execution uses read-only
action inputs inside the pool's networkless sandbox, with its private `/tmp`.

## Link kit

`native.lock.json`'s `link_kit` pins the inputs of the link kit
(`//tools/buck2/native-toolchain:kit`, `../buck2/native-toolchain/link_kit.py`),
which every executable links with instead of a compiler driver:

- Debian's glibc 2.28-10+deb10u1 `libc6` and `libc6-dev` and Linux 4.19
  `linux-libc-dev` for amd64 and arm64 (archive.debian.org);
- LLVM 21.1.8's `libunwind` and `compiler-rt` source releases;
- Zig's `lib/libc/glibc/abilists` at `bff490a2`.

Per target the kit holds `Scrt1.o`, `crti.o`, `crtn.o` and `libc_nonshared.a`
from glibc 2.28 unchanged; stub DSOs for libc, libm, libpthread, libdl, librt,
libutil, libresolv and the loader generated (Zig's `.symver` technique) from
the 2.28 libraries' public versioned symbols with their kinds, sizes, bindings
and default versions, cross-checked against abilists; `crtbeginS.o`/`crtendS.o`
from compiler-rt's `crtbegin.c`/`crtend.c`; and a static `libunwind.a` built with
LLVM's own static configuration against the 2.28 headers. `manifest.json`
records each file's SHA-256, source and flags, the clang and lld versions and
the SHA-256 of `ld.lld` and libLLVM; `digest` (the manifest's SHA-256) is the
kit's identity and enters every build ID. The runtime archive's own
`compiler_builtins` provides every compiler builtin: linking every archive
variant whole against the kit leaves no symbol undefined (the driver unit test
`native::kit::tests::whole_runtime_archives_need_no_builtins_beyond_their_own`),
so the supplemental builtin list is empty and the kit carries no compiler-rt
builtins. `:glibc_floor` unpacks the same `libc6` packages as
`<triple>/` loader and library directories for running executables at the
floor through that loader (never a link input).

```sh
.buck2/bin/buck2 test //tools/buck2/native-toolchain:proof
```

The proof runs clang's version command, links and runs C with the pinned lld,
builds a static AArch64 executable and runs it directly with the input qemu
(no binfmt), then checks the C executable with the input Valgrind and
`VALGRIND_LIB`. It checks the pinned clang/lld versions, requires output markers from both
executables and an explicit zero-error Valgrind summary, then requires
Valgrind to refuse a planted 32-byte leak with exit 97. A skipped step fails.
The per-push native smoke stage includes this proof, so a pin or unpacking
change cannot land without exercising the tools on a worker.

Valgrind's declared libc/debug dependency is included in its input tree.
The proof's C executable selects that loader and library directory, and
Valgrind finds the matching symbols beside each ELF in its standard `.debug/`
directory, by the original debuglink and build ID. Absolute
Debian usrmerge links are resolved inside the archive namespace before copying;
the unpack host's loader can never satisfy one of those links.

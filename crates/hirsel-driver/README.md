# hirsel-driver

The compiler's wiring. `FrontPhases` implements hirsel-engine's `Phases` over
hirsel-signature, hirsel-check, hirsel-modes and hirsel-lower (`pipeline.rs`), and
`COMPILER` is the engine's `Compiler` over them; `compiler_adapters()` and the
path entry points (`check_path`, `run_path`, `format_path`, `load_program*`)
start sessions from it. `native/` compiles the engine's `ValidatedNativeInput`
through hirsel-lir, hirsel-rc, hirsel-heapcheck, hirsel-llvm and hirsel-rt.

This crate exports only that wiring. Callers import queries, sessions and their
data from hirsel-engine directly, so each caller's dependency on the backends
and front phases is explicit.

Native callable-target specializations retain their original instance's signature
and placement proofs. `ReadyProgram.inline_instances` lets `native::link` include
known callable bodies and their transitive specializations in each calling
partition; the backend emits them as linkonce entries. Singleton boundaries
therefore do not erase a proven closure target or prevent its inlining. Ordinary
source entries, failure anchors and dynamic callable dispatch keep their ABI.
At O3, `native::partition_objects` also copies each small source unit
(`SMALL_UNIT_INSTRUCTIONS` LIR instructions or fewer), transitively, into every
object whose units call it, as an `available_externally` body. Its own object
still exports it, so the copy only lets LLVM inline it there; whether a call
inlines does not depend on the partition. An object's code therefore depends on
the small callee bodies it copies.

Allowed dependencies: hirsel-engine, the shared data crates, check-protocol, signature, check, modes, lower, run-config, core-contract and the native backends. Only cli, conformance and pipeline-tests (and the registry-backed harnesses of check and modes, as dev-dependencies) depend on this crate.

Native tool paths are explicit absolute inputs: `LLVM_SYS_211_PREFIX`,
`HIRSEL_AARCH64_CC`, `HIRSEL_AARCH64_SYSROOT`, `HIRSEL_QEMU_AARCH64`,
`HIRSEL_VALGRIND`, `VALGRIND_LIB`, `HIRSEL_VALGRIND_RUNTIME` and
`HIRSEL_VALGRIND_DEBUG`. The Buck2 test launcher supplies them
from the pinned toolchains. Missing or relative paths refuse construction;
there is no PATH search or system LLVM fallback. The input clang launcher
selects the input lld, and memcheck launches retain their input library path
when the child's environment is cleared.

Memcheck executables select the input libc loader and library directory at
link time and the matching debug-symbol directory at launch, so mandatory
Valgrind redirects do not depend on the worker image's stripped loader.

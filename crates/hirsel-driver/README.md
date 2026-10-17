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

`native::backend_features` inventories LIR opcodes with operand representations,
arithmetic and conversion modes, projections, layout shapes, validation stage
outcomes and runtime calls in emitted O0 LLVM IR across both targets and all
lowering choices. It never links or executes the source. A stage failure is
returned separately; it is not evidence that the failed stage was covered.
Individual LIR-validator and heap-checker check kinds are not instrumented.

`native::pairwise_matrix` covers every feasible pair of dimension values in
its input matrix, including the matrix's constraints on runtime archives and
pass choices. A per-program rotating anchor visits every input configuration
within one matrix-length cycle of consecutive seeds. Pair coverage is not a
proof of equivalent bug-finding power: higher-order interactions are covered
only by rotation over time and the explicit exhaustive campaign.

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
`HIRSEL_VALGRIND`, `VALGRIND_LIB`, `HIRSEL_VALGRIND_RUNTIME`. The Buck2 test
launcher supplies them
from the pinned toolchains. Missing or relative paths refuse construction;
there is no PATH search or system LLVM fallback. The input clang launcher
selects the input lld, and memcheck launches retain their input library path
when the child's environment is cleared.

Memcheck executables select the input libc loader and library directory at
link time; matching debug symbols sit in each object's adjacent `.debug/`
directory, so mandatory
Valgrind redirects do not depend on the worker image's stripped loader.

Production native import uses `hirsel-layout` for type-to-layout and ABI derivation.

`run_path_with_config` with an explicit record is the production interpreter launch: it validates the record,
checks the semantic revision and required roots with `admit_run`, then executes TIR with
`network::HostLibrary`. This adapter translates explicit filesystem, clock and network
calls to the runtime's typed Request/Completion protocol, which owns grant enforcement;
the interpreter has no host effects. Pure calls and recorded Args/Env snapshots use the
ordinary Library. `run_path`, `run_path_with_simulation` and `run_file` are the fake-provider
entry points for semantic fixtures. Production native runs pass the same validated record
to linking and launch; explicit host runs have streaming stdout and no harness wall-time limit.


`native::build_path_native` checks a source or manifest and writes a standalone
executable. Its entry adapter forwards nominal roots in source parameter order,
derived from the engine's shared `entry_roots` validation. The executable embeds
semantic program revision and entry identity; fresh launch grants and argv are
validated and admitted by the runtime, independently of compiler profile.

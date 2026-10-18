# hirsel-driver

The compiler's wiring. `FrontPhases` implements hirsel-engine's `Phases` over
hirsel-signature, hirsel-check, hirsel-modes and hirsel-lower (`pipeline.rs`), and
`COMPILER` is the engine's `Compiler` over them; `compiler_adapters()` and the
path entry points (`check_path`, `run_path`, `format_path`, `load_program*`)
start sessions from it. `native/` compiles the engine's `ValidatedNativeInput`
through hirsel-lir, hirsel-rc, hirsel-heapcheck, hirsel-llvm and hirsel-rt.

Source launch boundaries flush stdout before writing the interpreter's PROG-3 return-payload report. Ready programs retain a checked, layout-derived diagnostic view for the native entry wrapper. Host reports are separate from semantic observations and Io tapes. Native CLI launches inherit the OS output streams, preserving the host report and finalizer order; custom-writer launches retain per-stream forwarding.

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

`native::Proofs` (`native/proofs.rs`) is the `--proofs` consumer. It builds
this compiler's own export of the entry's closure, assesses the sidecar with
`hirsel_proof_export::sidecar`, and maps each accepted obligation's TIR sites to
a removal form in a `Discharge` plan carried by `Options::discharge`. Before it
plans, it requires the native snapshot's TIR of each planned definition to be
the exported TIR. `build_program` lowers with the plan and runs
`validate_discharge` on every discharged unit before the Value validator. Any
difference from the plan is an import validator error. `ReadyProgram.discharged`
lists the removed sites, which the proof report reads. Integer operators import
as unchecked `IntArith` (Neg as subtraction from zero); projection addresses
retain their path and mark only the licensed bounds/tag guards unchecked.
Append fusion carries source-site and canonical-place mappings through caller
rewrites and derived builders, so reports and evidence keep their original ids.

Extern calls (PROG-4): `ReadyProgram.foreign_links` are the root's `unsafe ffi` link
inputs, passed after the runtime archive (`-l<name>` or the file); the link accepts
`Foreign` imports beside runtime ones. `foreign_unreplayable` sets hirsel-rt's
`UNREPLAYABLE` bit beside the root mask, so the executable refuses `--hirsel-record` and
`--hirsel-replay`; H refuses an extern call as `Unsupported`.
`ReadyProgram.callbacks` names the definitions a unit references as C callbacks, whose
objects export their callback thunks.

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

`run_path_on_host` is the production interpreter launch. `bind_host_run` either validates
a record or template, checks the semantic revision and required roots with `admit_run` and,
with `HostLaunch::Live { record: Some(tape) }`, streams the runtime tape
(`hirsel_rt::host_launch::record`); or binds a `HostLaunch::Replay` tape's own run to the
checked revision, entry and roots and runs on `hirsel_rt::host_launch::replay`'s SimIo. It
then executes TIR with `network::HostLibrary`. This adapter translates every Io
projection, stdout and stderr writes, stdin and Env reads, and filesystem, clock and
network calls to the runtime's typed Request/Completion protocol, which owns grant
enforcement, in the order native code issues them, so one tape replays in either engine.
The interpreter has no host effects: live stdout and stderr are written by the runtime
adapter, and a replay copies its recorded output to the process's streams. A replay never
parks on readiness; the order in which ready sockets resume cooperative tasks is not yet
recorded, so a run whose tasks interleave host requests by readiness may refuse its own
replay as a divergence (never replay differently). Pure calls and
recorded Args/Env snapshots use the ordinary Library. `run_path`, `run_path_with_simulation` and `run_file` are the fake-provider
entry points for semantic fixtures. Production native runs pass the same validated record
to linking and launch; explicit host runs have streaming stdout and no harness wall-time limit.


`native::build_path_native` checks a source or manifest and writes a standalone
executable. Its entry adapter forwards nominal roots in source parameter order,
derived from the engine's shared `entry_roots` validation. The executable embeds
semantic program revision and entry identity; fresh launch grants and argv are
validated and admitted by the runtime, independently of compiler profile.

`LaunchConfig` selects either a bound record or a grants template. Both source
engines resolve it against the checked semantic revision before host admission;
templates never pass through a temporary exported record. Native host launches pass
`--hirsel-record <tape>` or `--hirsel-replay <tape>` to the executable after the driver
has bound the run, so a recording of other code refuses before linking; a replay inherits
no stdin.

Native live launches inherit stdin only under the admitted stdin projection.
Their two output pipes drain concurrently into separate caller-owned writers,
while retaining the same bytes for native observations and launch diagnostics.

`run_path_with_step_budget` selects an optional simulation execution budget;
`run_path_on_host` receives the selected optional step bound for live and replayed
host runs. `None` means unbounded. CLI callers supply one billion for simulation
and replay, and no bound for admitted live host runs. `run_path`,
`run_path_with_simulation`, and interpreter
`Limits::default` retain their existing test/conformance budgets.

HostLibrary implements OperationsAdapter, whose forwarding implementation is generated
from the same inventory as Operations. Scheduler and cleanup hooks therefore cannot
silently inherit no-op defaults. Host sockets use CooperativeNet: parked operations
retain typed snapshots while the evaluator retains owners and mutable loans. A
`stdin.read` with no available byte parks its task on the same Readiness and resumes
through `Library::read_stdin`. Socket and stdin readiness and relative timeouts are
polled at scheduling points; the scheduler waits only when no task is runnable. Effect-free waits observe level-triggered cancellation.

`LaunchConfig::Grants` can supply an explicit Args snapshot override, validated
and included in the completed record before admission. An absent override retains
the template's snapshot; an empty override grants empty Args.

`api_index` enumerates public core registry entries and std manifest declarations
with the compiler parser, retaining source signatures and constructor payloads.
`hirsel doc --index` renders the deterministic full signature index in `docs/api.md`.

# hirsel-cranelift

The Cranelift backend, for fast debug builds: emission of validated Ready LIR
units through Cranelift 0.136.3 (`cranelift-codegen`, `-frontend`, `-module`,
`-object`, pinned `=0.136.3`) into an owned `hirsel_lir::ObjectArtifact`. It
links with the same runtime archive and calls at the same ABI as `hirsel-llvm`.
`hirsel build --backend cranelift` and `hirsel run --native --backend cranelift`
select it. The Cranelift parity gate holds it to LLVM's observations.

Allowed dependencies: the pure `hirsel-graph` utility, the shared data crates
`hirsel-lir`, `hirsel-core-contract` and `hirsel-syntax`, and the four Cranelift
crates. No other implementation crate.

Interface:

- `emit_object(&[&ValidatedUnit<Ready>], &EmitInputs) -> Result<ObjectArtifact, CodegenError>`,
  with the same `EmitInputs` (`hirsel_lir::emission`) as `hirsel-llvm`. `OptLevel::None`
  is Cranelift's `none`, every other level its `speed`. The artifact names `TOOLCHAIN`
  (`cranelift 0.136.3`) in its reproducibility record, and the driver links the
  objects of that toolchain with a launcher this crate emits too.
- `emit_clif`: the same emission, returning every function's Cranelift IR as text.
- `launch_object(target, entry, root_words, report, launcher, arguments)`: the launcher
  object, `main` and the `hirsel.entry` adapter with its PROG-3 exit report
  (`ExitReport`), as the driver's LLVM shim builds them.
- x86-64 and AArch64 (`isa::lookup_by_name` of the target triple); the parity gate runs
  both, AArch64 under qemu.

Invariants:

- A unit is emitted only if its receipt binds this unit, the Ready stage, this LIR and
  validator version, and exactly the target, layout table, manifest and input identity.
- An instruction, layout or step this backend does not lower is `CodegenError::Unsupported`
  naming it and its definition, never a miscompile. A panic in Cranelift or here is
  `CodegenError::Panic`. The Cranelift verifier runs on every function.
- Values: scalars are SSA values (Bool an `i8` 0 or 1, i128 a Cranelift `i128` passed in
  register pairs as LLVM passes it); callables a code and environment pair; every other
  value lives in an immutable stack copy, so a load is always a copy. Block parameters
  holding bytes are stack slots per block and parameter, and edges copy into them,
  through temporaries when one parameter's source is another's slot.
- Counts, allocations and releases always go through the runtime: no inlined fast paths,
  no deferred count stores, no uniqueness facts, no specialized release glue
  (`hirsel_rt_v2_release` with the header's descriptor), no adopted builder blocks.
  Arena blocks always call `hirsel_rt_v2_arena_alloc` after the bounded-arena charge.
  In a program that opens a bounded arena, a List or Array an arena holds is a tracked
  block (MEM-6, runtime ABI 11): every `ArenaPush` calls `hirsel_rt_v2_arena_push`,
  `IsWritable` reads the count, and `Share` marks the tracked blocks a copy holds shared.
- Library and inline-instance bodies are weak (`Preemptible`) in every object that
  references them, as are layout descriptors and the descriptor registry; source units
  are exported once. No `available_externally` copies: Cranelift does not inline.
- Checked arithmetic and conversions follow `hirsel-llvm`: overflow, division by zero,
  shifts and float-to-int range checks are guarded before the instruction. i128
  division and i128-to-float conversion call compiler-rt (`__divti3`, `__floattidf`, ...),
  float remainder calls `fmod`/`fmodf`.
- Comparators and hashers are generated per layout as `hirsel-llvm` generates them
  (`hirsel.compare.<id>`, `hirsel.hash.<id>`), defined as soon as a unit asks for one so
  an error names that unit.

- Capacity-based SoA storage retains row coordinates through places; projection,
  gather and scatter use the stored column capacity. Reserve moves credits from
  private blocks and duplicates holders when detaching shared blocks.
- Foreign calls use the platform C ABI and establish the thread's callback context.
  Callback thunks restore the Hirsel status protocol and end the process on failure.
  AAPCS64 narrow scalar registers are explicitly truncated/extended at the C boundary.
- Inline releases use the runtime's inline continuation (`hirsel_rt_v2_finalize`),
  sharing finalizer shielding, field-ordinal live masks, ordered drops and failure
  accumulation with block releases. Sequence/column copies walk their scan programs.

Debug line tables and profile sources are not emitted. Cranelift records source
locations and machine-code ranges but does not write DWARF line sections; doing so
requires a separate DWARF writer and object-section relocations.

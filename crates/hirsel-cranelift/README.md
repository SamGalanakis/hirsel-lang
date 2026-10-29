# hirsel-cranelift

The Cranelift backend, for fast debug builds: emission of validated Ready LIR
units through Cranelift 0.136.3 (`cranelift-codegen`, `-frontend`, `-module`,
`-object`, pinned `=0.136.3`) into an owned `hirsel_lir::ObjectArtifact`. It
links with the same runtime archive and calls at the same ABI as `hirsel-llvm`.
`hirsel build --backend cranelift` and `hirsel run --native --backend cranelift`
select it. The Cranelift parity gate holds it to LLVM's observations.

Allowed dependencies: the pure `hirsel-graph` utility, the shared data crates
`hirsel-lir`, `hirsel-core-contract` and `hirsel-syntax`, the four Cranelift
crates and `gimli` 0.33 (DWARF writing). No other implementation crate.

Interface:

- `emit_object(&[&ValidatedUnit<Ready>], &EmitInputs) -> Result<ObjectArtifact, CodegenError>`,
  with the same `EmitInputs` (`hirsel_lir::emission`) as `hirsel-llvm`. `OptLevel::None`
  is Cranelift's `none`, every other level its `speed`. The artifact names `TOOLCHAIN`
  (`cranelift 0.136.3`) in its reproducibility record, and the driver links the
  objects of that toolchain with a launcher this crate emits too.
- `test_dispatcher_object(target, entries)` emits the package-test selector with
  Cranelift, including decimal-index validation and exit 3 for invalid arguments.
- Hirsel unit and glue calls use direct PC-relative references across objects.
  This requires image symbols to remain non-preemptible (`local:` units and
  `-Bsymbolic`). Runtime, toolchain and foreign imports retain GOT indirection.
- The one canonical ELF stream validates x86 RIP-relative GOT load and indirect
  call/jump encodings before marking them `REX_GOTPCRELX`/`GOTPCRELX`. lld can
  relax references that bind locally; all artifact and launcher consumers receive
  those same bytes. Other references retain their original relocation.
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
  `CodegenError::Panic`. The Cranelift verifier runs on every function in debug builds
  (`debug_assertions`, as every test builds); release builds skip it.
- Values: scalars are SSA values (Bool an `i8` 0 or 1, i128 a Cranelift `i128` passed in
  register pairs as LLVM passes it); callables a code and environment pair; every other
  value lives in an immutable stack copy, so a load is always a copy. Block parameters
  holding bytes are stack slots per block and parameter, and edges copy into them,
  through temporaries when one parameter's source is another's slot.
- Counts, allocations and releases always go through the runtime: no inlined fast paths,
  no deferred count stores, no ordinary uniqueness facts, no specialized release glue
  (`hirsel_rt_v2_release` with the header's descriptor), no adopted builder blocks.
  Arena blocks always call `hirsel_rt_v2_arena_alloc` after the bounded-arena charge.
  In a program that opens a bounded arena, a List or Array an arena holds is a tracked
  block (MEM-6, runtime ABI 11): every `ArenaPush` calls `hirsel_rt_v2_arena_push`,
  `IsWritable` reads the count, and `Share` marks the tracked blocks a copy holds shared.
- Library and inline-instance bodies are weak (`Preemptible`) in every object that
  references them, as are layout descriptors and the descriptor registry; source units
  are exported once. Private ahead objects inline their validated spare-push bodies
  with Cranelift's inliner; ordinary unit calls remain out of line.
  Checked CPU list helpers additionally carry one must-unique bit for their first
  mutable root. A completed reserve establishes it; copying, sharing, replacing
  or exposing an owner invalidates it. Borrowed reads keep separate facts that
  calls also invalidate. A valid bit skips the header test in the ordinary
  lowered body. No runtime ABI or externally callable signature changes.
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

`emit_object_profile` accepts the shared LIR `ProfileSources` and emits DWARF 5
line tables, a compile unit and subprogram entries through gimli. Source locations
use the same operation/cleanup/block anchor selection as LLVM. Rows cover only
Cranelift's final machine source ranges; generated thunks/helpers have no source
rows. Deferred failure code retains its initiating operation's location. Code
addresses and section offsets relocate through cranelift-object's object writer
on both x86-64 and AArch64. Without sources, no debug sections are emitted.

`named_launch_object` uses an explicit exported dispatcher symbol, allowing a package test executable to contain one launch adapter per test.

Foreign pointer boundaries match LLVM: Ptr/FnPtr results, callback arguments and
writable pointer words are guarded before entering Hirsel, including unused results.
Nullable pointer options use one C pointer word and ordinary niche storage inside
Hirsel. Foreign contract violations terminate with PROG-4 at the active extern call's
site; nested calls restore the enclosing callback context and site.

ABI v3 has the same optional caller/operation u64 carrier and source attribution
policy as LLVM. Function-value boundaries survive Direct/Bound specialization;
root and C callback adapters provide zero when there is no Hirsel caller. Runtime
cleanup and task startup pass the pair explicitly.

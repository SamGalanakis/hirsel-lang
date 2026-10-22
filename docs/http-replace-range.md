# Initialized byte replacement

`core@1/bytes.replace_range(mut builder: List[u8], start: u64, data: Bytes) -> ()`
is a checked, fixed-length write. It preserves destination uniqueness and does
not retain source provenance. It may allocate, requires no capability and has no
recoverable error result.

After ordinary left-to-right argument evaluation and mutable-place preparation,
read original list length L and source length N. Check start ≤ L before computing
L − start; then check N ≤ L − start. Either violation raises MEM-14 through normal
failure cleanup, without an operation-owned copy, arena charge or partial write.
A valid empty interval returns immediately, including shared storage in an arena
with no allowance left. This guarantee does not suppress ordinary evaluation or
preparation of an enclosing shared record.

For a valid nonempty interval, keep the immutable source snapshot alive, obtain
ordinary writable destination storage once, then copy with overlap-safe semantics.
Every external alias keeps its old bytes. A same-owner source requires COW even
when the intervals are disjoint or the bytes identical. No equality elision changes
accounting. Length and capacity stay unchanged; only [start, start + N) changes.
Spare capacity is neither a legal destination nor a readable source.

A writable tracked arena charges zero. Shared tracked storage charges exactly L
logical bytes for the existing COW copy, with no N-byte append charge or spare
capacity charge. Failed preflight publishes no owner or byte change. Physical
allocation failure retains ordinary environmental termination behavior. Once the
bulk copy begins there is no language failure point. Completion/writeback, source
temporary cleanup, cancellation and FAIL-3 shielding follow existing rules.

The compiled wrapper serves direct calls and function values. Its ordered checks
and empty return precede a newly prepared list-slot address. P4 classifies only
this runtime leaf as writing the list held in that slot, inserting existing
IsWritable/shallow UniqueCopy and ordinary release. There is no new instruction,
length publication, ArenaPush or hand-coded library RC. The runtime leaf repeats
range checks but assumes the wrapper has made nonempty destination backing
writable; it does not allocate or return arena exhaustion. H writes that same
prepared allocation, preserving owner, initialized length, loans and capacity.

The additive ABI export takes TaskCtx followed by list-slot pointer, start u64,
source owner pointer, source origin u64 and source length u64; it returns status
u8 and has no result storage. Source origin uses the existing Bytes representation.
No noalias assumption is made. Runtime ABI revision 39 adds this export; existing
fields, layouts and signatures are intact. Language rules and TIR/LIR constructs
are unchanged.

Prior art: Rust f6c1107f8be13e48465e12c0fd26e6a39fe18e4f,
`library/core/src/slice/mod.rs:4280–4388` and
`library/coretests/tests/slice.rs:2030–2088`, MIT/Apache-2.0, The Rust Project
Contributors. Attribution and license terms are in `third-party/NOTICES.md`.
The check-before-copy structure and overlap/empty/invalid vectors follow
copy_from_slice/copy_within. Six Hirsel deviations are deliberate:

1. Immutable Bytes aliases survive writes through ordinary destination COW.
2. Invalid destinations raise MEM-14 with cleanup rather than Rust panic.
3. Sources are valid Bytes intervals; invalid source construction raises SEM-3
   before the operation, rather than accepting generic RangeBounds here.
4. Shared arena copies obey MEM-6 logical charging; physical exhaustion remains
   environmental.
5. The operation is byte-only and performs no generic element finalization.
6. Unlike Vec::splice, length and capacity never change. History append's forward
   recursive copying is a separate contract.

A correct primitive does not establish the HTTP product target. The unchanged
1024-request persistent static GET server still requires same-session instruction
and native timing evidence against the Rust library participant at ≤3×. HTTP
integration is conditional on primitive parity and the scoped measurement gates.

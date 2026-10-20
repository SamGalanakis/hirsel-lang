# Exclusive mut-place extent and alignment

LLVM 21.1.8 can hoist a sort partition's root-word load when its exclusive
address carries `dereferenceable(8) align 8` beside the existing `noalias`.
These attributes describe the place holding the pointer, not the block it
points to. The soundness argument is in the [implementation spec](../../../docs/spec/implementation.md#aliasing-facts).

Fixed places use their layout size and alignment: an inline 24-byte tuple
promises 24 bytes, not eight. `Packed[T]` uses the smallest variant extent,
since a sequential element has only its active extent. Zero-size places
omit `dereferenceable`, whose LLVM operand must be positive. Private library
Block/Box roots travel by value and receive none of these address attributes.
Packed field calls use aligned temporary storage and write back before either
status continuation; an independently compiled C callee checks actual alignment
for every offset at widths 2, 4, 8 and 16, both outcomes, targets and O0/O3.

## Controlled IR comparison

The alloc-perf sort experiment's unoptimized module is the input. The control
adds the already-landed `noalias` to the partition definition's root parameter.
The treatment adds only `dereferenceable(8) align 8` to that same parameter.
Both run through pinned `opt -S -passes='default<O3>'`.

| Partition IR | Before | After |
|---|---:|---:|
| Static instructions | 452 | 442 |
| Root-word loads | 8 | 6 |
| Root-word loads in cyclic basic blocks | 5 | 0 |

The five loop loads move to preheaders. Root loads outside loops remain.
No invariant or pointee metadata is added; this change promises nothing about
the list's length word through other holders.

## Executed microbenchmark

`bench/micro/programs/sort.hir`, 1,000,000 generated u64 values, one epoch,
LLVM O3, baseline a49b172f versus this change on the same base. The Rust pair
participates as a checksum and workload control. `bench/micro/run` uses ten
rounds, seed 42, shuffled order, CPU 30, total and user instructions/cycles,
and before/after SMT sibling snapshots. CPU 31's timing lock was occupied.
No samples were excluded; all 30 stdout SHA-256 hashes match.

| Median hardware instructions | Before | After | After / before |
|---|---:|---:|---:|
| Total | 561,202,272.5 | 539,775,682.5 | 0.961820 |
| User | 529,938,454 | 508,564,002 | 0.959666 |

The verdict is 3.82% fewer total instructions and 4.03% fewer user instructions.
The within-session task-clock ratio is 0.9224, with bootstrap 95% CI
[0.8300, 1.0060]; this does not establish a timing improvement. Median sibling
busy share is 0.2917 for both Hirsel binaries (ranges 0–0.8571 before and
0.1667–0.6667 after).

Session `fe309e35-b6de-4ff4-a849-33f1b33cdbe8` and the binaries, participants,
raw counters, sibling snapshots, checksums, complete IR inputs/outputs and
hashes are archived on the measurement host at
`/workspace/code/hirsel-briefs/llvm-deref-2026-10-20/`.
Binary SHA-256: before
`08975bb60d44c21c645f6d1a897ffa77665bdcd3145558d1202055066d9f6957`, after
`60e0c48327746fa09ce8980fbace956d25ca732f3d168922357cd7e07a8d93f2`.

The optional source-level Packed mutation probe exposed a pre-existing native
import `MutExit`, reproduced with the baseline compiler. Its minimal program
is archived as `gaps/gap10-packed-mutexit.hir` and assigned to native-bugs-5;
the task owner approved retaining the validated Ready-IR minimum-extent golden
and omitting that source probe from this change.

## Validation

The exhaustive matrix (version 4, seed 1) passed all 150 configuration runs:
packed-field success, packed-field failure, and the sort-vector control under
all 50 configurations, both targets, no exclusions. The focused LLVM suite
passed all three address-attribute tests. Its independent C alignment witness
covers 240 calls: (2 + 4 + 8 + 16) offsets × two outcomes × two targets × O0/O3.

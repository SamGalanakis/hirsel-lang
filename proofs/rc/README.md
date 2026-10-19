# Lean RC and region model

A machine-checked model of the native sequential storage protocol: counted
heap blocks with finite and immortal headers, holder credits, borrow guards, partial
initialization, the dead worklist, raw reuse tokens, compiler-local uncounted regions,
explicit forget and static exemptions, and finalizers as ordered events. One executable
function, `Rc.step`, is both the transition system the theorems are about and the evaluator
behind `rc-trace`, so the vectors in `native-tests/model/` run the proved machine itself.

## Toolchain and commands

Lean **4.34.1** (released 2026-09-24), pinned in `lean-toolchain`. The project has no
dependencies: `lake-manifest.json` lists no packages, and nothing comes from Mathlib or
Batteries. Install elan in user space, without a default toolchain; the pin selects the
release:

```
curl -sSf https://raw.githubusercontent.com/leanprover/elan/master/elan-init.sh | sh -s -- -y --default-toolchain none
```

| command (in `proofs/rc`) | what it checks |
|---|---|
| `lake build` | the model, every proof and the `rc-trace` executable (about 45 s from clean) |
| `./audit.sh` | no `sorry`, `admit`, `axiom`, `native_decide` or other escape hatch in the sources; a clean build; every theorem in `Audit.lean` depends only on `propext`, `Classical.choice` and `Quot.sound`. Prints the published theorem set |
| `./check-vectors.sh` | every vector in `native-tests/model/` prints exactly its checked `.expected` output |
| `lake exe rc-trace <vector>` | prints the exact final (or stuck) model state of one vector |

`tools/check-native.sh` runs `audit.sh` and `check-vectors.sh` as its Lean stage.
`tools/gate` runs both on this host when a change touches `proofs/`, and `check-vectors.sh`
when it touches `native-tests/model/`; `tools/gate --full` always runs them.

## The model

`Rc/Basic.lean` and `Rc/Machine.lean` define the state and the machine.

- **Allocation identities.** An address plus a generation. Every value birth at an address
  bumps its generation, so a stale handle never matches a reused slot. Slots are `free`,
  `live`, `dying` (zero holders, on the dead worklist) or `raw` (owned by one reuse token).
- **Counts.** A header is `Dead | Finite n | Immortal`. The *ideal count* of an address is the
  number of credits in the ledger that target it. A live counted block's finite header equals
  its ideal count and lies in `1 .. max-1`. A share that would reach `max` saturates to
  `Immortal`, which is sticky. `Config.max` is a parameter, so a vector can use a tiny one. ABI v2 instantiates it with `Config.production.max = 4294967295`; `production_incr_bounded` and `production_sentinel_sticky` pin the full u32 boundary without changing rc-trace v1.
- **Holder credits.** Owning credits are explicit: `root` (a binding or temporary), `field`
  (an initialized pointer field `k` of a block; an uninitialized field holds none, which is
  partial initialization) and `forgot` (the permanent ledger of an explicit forget).
- **Borrow guards.** A borrow is guarded by the credit it was obtained through and by its
  parent (a borrow, or a region local). The guarding credit cannot be released or moved while
  the borrow is live, and a borrow cannot end while a sub-borrow is live.
- **Dead worklist (MEM-3).** A release that reaches zero pushes the block onto the worklist.
  Each `drain` step releases one field credit, the highest initialized index first (reverse
  declaration order), or retires the block. A child that dies is drained before its parent's
  next field, with no recursion.
- **Finalizers.** A finalizable (resource) block runs one `finalize ok|fail` event before
  its fields are released. A failure still releases the fields. Resources are noncopyable,
  so they never saturate.
- **Reuse tokens.** `reset` consumes a holder. On a unique donor it produces a `pending` token,
  which becomes `raw` storage of exact capacity once the donor has drained. A failing donor
  finalizer turns the token into `none`. On a shared or immortal donor it releases normally
  and produces `none`. `reuse` and `dispose` each consume exactly one token.
- **Regions.** Region blocks are uncounted and referenced only by locals. They may hold
  outgoing heap credits, stored while the block is unpublished. Regions close innermost
  first, never while a local into them is live. Closing releases each outgoing credit once
  and frees the region's storage.
- **Exemptions.** Static constants (`static`) and saturated blocks are `Immortal` and never
  freed. A forgotten subgraph is held by the forget ledger and never drains.

### Transitions

Runtime primitives cite these IDs. The `hirsel-rt` comments currently use provisional
`D:<name>` labels, mapped in the last column.

| ID | op (vector syntax) | effect | runtime entry, `hirsel-rt` label |
|---|---|---|---|
| T01 | `alloc rN ARITY [fin]` | fresh counted heap block, count one, new root | `alloc`; Alloc |
| T02 | `static rN` | immortal static block, new root | static constant |
| T03 | `dup rA rB` | share: count+1, or saturate to Immortal, or stay Immortal | `dup`; Dup, DupSaturate, DupImmortal |
| T04 | `drop rN` | release: count−1, or die onto the worklist, or stay Immortal | `release`; Drop, DropLast, DropImmortal |
| T05 | `write rB K rN` | in-place store into a unique block, releasing a displaced child | generated store after the count test |
| T06 | `take rB K rN` | move a field out of a unique block (partial move) | generated |
| T07 | `load bP K rN` | owned read of a field through a borrow (counted projection) | `dup` |
| T08–T11 | `borrow`, `borrow-field`, `borrow-local`, `end-borrow` | start or end a guarded view | none (static loans) |
| T12 | `reset rB tN` | consume a holder; pending token if unique, else none | `reset`; Reset |
| T13 | `reuse tN rR ARITY [fin]` | consume a token; raw storage gets the next generation | `reuse`; Reuse |
| T14 | `dispose tN` | consume a token; shallow-free raw storage | `dispose`; Dispose |
| T15 | `finalize ok\|fail` | the worklist head's finalizer event | `finalize`; FinalizeBegin, FinalizeEnd |
| T16 | `drain` | release the head's last field credit, or retire it (free, or raw for its token) | release worklist; DropEdge, Free |
| T17 | `forget rN` | move a root credit to the forget ledger | `forget` |
| T18–T20 | `region-open`, `region-alloc`, `region-store` | open, allocate uncounted, store an outgoing heap credit | `region_enter`, `region_reserve`, `region_hold`; RegionEdgeAdd |
| T21, T22 | `local-copy` (publishes), `local-drop` | copy or end a local reference | none |
| T23 | `region-close gN` | release outgoing credits once each, free the region | `region_exit`; RegionClose |

An illegal operation is an error naming its transition, never a silent no-op. The runtime
must reject it too, or prove it unreachable.

## Theorems

Each theorem below is in the published set (`Audit.lean`) that `audit.sh` checks.

| claim | theorem |
|---|---|
| every legal step of every operation preserves the invariant | `step_inv`, `inv_init`, `reachable_inv` |
| holder and borrower safety | `holder_safety`, `borrower_safety`, `step_safety` |
| no step frees or reuses a reachable allocation (surviving holders keep a live block of the same generation) | `step_no_reuse_of_reachable` |
| counts track the ideal count and never wrap | `counts_track`, `step_no_wrap`, `incr_bounded`, `unique_header_exact` |
| immortality is sticky: never freed, never unique again | `immortal_sticky`, `immortal_never_unique`, `resource_never_immortal` |
| alloc, dup, drop, reset, reuse, dispose and region close each simulate an ideal step or a silent storage step | `simulation` (with the decision correspondence `Inv.abs_release`) |
| token linearity | `token_linearity`, `reset_produces`, `reuse_consumes`, `dispose_consumes`, `token_used_once` |
| region-exit credit conservation | `region_close_conserves` |
| each finalizer obligation is consumed once, before the fields are released | `finalizer_at_most_once`, `drain_after_finalizer` |
| a finite zero-holder DAG drains to dead storage, except the explicit exemptions | `drain_terminates`, `no_garbage`, `drains_to_dead` |
| last-use release is safe, and is what lets MODE-2 end uniqueness | `last_use_release`, `last_use_enables_unique` |

The **ideal machine** (`Rc/Ideal.lean`) has no headers and no tokens. A value dies when no
credit remains on it, unless it is *pinned*: conservatively retained, which is how the ideal
machine sees an immortal block. `abs` erases headers, tokens, pending-reset marks and the
contents of raw storage. `simulation` states that each listed concrete step maps to an ideal
`alloc`, `dup` (possibly pinning), `release` or `regionClose` step, or leaves the abstraction
unchanged. `dispose` always leaves it unchanged, which makes it the silent storage step.

**Draining** assumes that a scheduled user finalizer terminates: the model makes each
finalizer one atomic event whose outcome the caller supplies. With that, `drains_to_dead`
shows that, for any finalizer outcomes, legal steps reach an empty worklist. In that state
every live counted block is `Held`: reachable through field credits from a root, the forget
ledger, a region or an immortal block. Every other block is free or raw storage. The
acyclicity witness is a ghost birth stamp: each edge points from a younger block to an older
one, and an in-place write restamps the unique block it mutates.

**Last-use release.** `drop` has no scope-end precondition. It needs only that no live borrow
is guarded by the holder, which holds once the holder's last loan has ended. `last_use_release`
shows that such a release is legal and safe, keeps every other holder, borrow and local, and is
the ideal release step. `last_use_enables_unique` shows that after the last-use release of one
of two shares, the survivor's header is exactly one, so its in-place update test succeeds.
M1b changes no source rule. Relaxing MODE-2 is a separate spec decision that can cite these.

## Vector format `rc-trace v1`

`native-tests/model/*.trace` are the protocol fixtures. The J and H lanes consume them as a
contract. Each has a checked `.expected` output. `Rc/Trace.lean` documents the syntax:
a `rc-trace v1` line, a `max N` line, then one operation per line with sort-prefixed
identifiers (`r` roots, `b` borrows, `l` locals, `t` tokens, `g` regions) and `#` comments.
The output is the exact final model state, or the state before the first illegal operation
with that operation's step, line and error. The ghost `born` stamps are omitted. Credits,
borrows, locals and tokens print sorted; the worklist prints head first, regions innermost
first and events oldest first.

**Versioning.** Any change to the syntax, an operation's semantics or the output form bumps
the version (`rc-trace v2`). It also regenerates and re-checks every `.expected` file in the
same change. A consumer rejects a version it does not know.

## What the model does not cover

These claims are outside the proof domain. Nothing here should be read as evidence for them.

- **Perceus insertion.** Nothing proves where the compiler places dup, release, reset, reuse
  or dispose, nor that it emits only legal operations. P4/P5 and the C validator own that.
  This model proves that the operations are safe when legal.
- **Normalization.** Instantiation, mandatory loop normalization, SSA import, placement and
  cleanup lowering (P1–P3, P6).
- **Rust unsafe code.** `hirsel-rt`'s pointer manipulation, headers, size classes, free
  lists, slabs and the idle pool cap. Runtime correspondence is tested against these vectors,
  not proved.
- **LLVM** and everything P7 and P8 do.
- **The C ABI** (`hirsel_rt_v2`): status codes, record layouts, u64 conversion, `RunCtx`.
- **OS behaviour.** Backing allocation, out-of-memory, stack depth, signals, process exit, Io.
- **Weak memory and concurrency.** The model is sequential. Shared roots, atomic counts,
  freeze and thaw, task transfer, channels, mutexes and explicit arenas are M2, and no
  theorem here covers them.
- **User finalizer bodies.** A finalizer is one atomic event with an outcome. Its
  termination is assumed, and its effects are not modelled.
- **Physical layout.** Fields are abstract indices, with no byte offsets, scan or drop
  programs, tags, flat or packed values or alignment. Reuse is keyed on exact field count.
- **Region internals.** Uncounted pointers between region blocks, unpublished builder
  relocation, growth by replacement and the 8 MiB idle pool cap (MEM-10) are not modelled.
  Region blocks hold only outgoing heap credits.
- **Copy and COW.** Abstract mutation requires uniqueness. Deep copy, COW detachment and
  `unique x` are expressed by a trace as allocations, loads and writes. Nothing proves that
  the compiler chooses them correctly.
- **Source rules.** Loans, modes, MODE-1/MODE-2, brands and the language's failure
  propagation are not modelled. Failure appears only as a finalizer outcome.
- **Logical accounting.** No frame or Depth charge exists here (#16).

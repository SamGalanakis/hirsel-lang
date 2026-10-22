# NetPeer ownership repair lab

[Run the example](../examples/ownership-repairs/main.hir) to compare three ways
of putting `Bytes` into the real core `NetPeer` representation. These are fixture
records only: the program acquires no capabilities and performs no network I/O.
Its `Result` is `Ok` only when the payload and lifetime controls pass.

| Need | Signature and construction | Caller consequence | Cost consequence |
| --- | --- | --- | --- |
| Read a peer while keeping its source alive | `peer_view(data: Bytes) -> NetPeer in data`, with `Send(data)` | Source may be read, but cannot be replaced, mutated or taken until the peer's last use. A non-global source keeps its bounds. | No payload deep copy; constructing the enclosing fixture can still allocate. |
| Give the peer the caller's last use of the input | `peer_taken(take data: Bytes) -> NetPeer`, with `Send(take data)` | Call `peer_taken(take data)`; the input binding is unavailable afterwards. The result can outlive that binding. | Ownership transfer adds no payload copy; fixture construction still has its own cost. |
| Keep using the source independently of the peer | `peer_copied(data: Bytes) -> NetPeer`, calling `peer_taken(unique data)` | Source and result can be replaced and used independently; the result can escape the source binding. | A borrowed `Bytes` needs an explicit deep unique copy, O(payload size), potentially allocating. This is not equivalent in cost to transfer. |

The authority is [MODE-1](spec/ownership.md#mode-1-locality),
[MODE-2](spec/ownership.md#mode-2-uniqueness), and
[copy and take](spec/ownership.md#on-copy-and-take).
`copy data` shares counted storage and retains a borrowed parameter's bounds;
it is not the third repair. `unique` preserves arena/region placement, so this
heap-input recipe does not promise to move arena data onto the heap.

The preserved [original F07 reject](../crates/hirsel-driver/tests/fixtures/ownership-repairs-6/original.hir)
returns an unrelated `NetPeer` containing borrowed bytes and must still fail
MODE-1. The [existing diagnostic repair table](../crates/hirsel-modes/src/explain.rs)
already recommends a taken parameter/caller move or a deep unique copy, and warns
that `copy` retains the borrow. This lab does not change that diagnostic.

From the repository root, with `hirsel` installed:

```sh
hirsel check examples/ownership-repairs/hirsel.json
hirsel run examples/ownership-repairs/hirsel.json
hirsel run --native --backend llvm examples/ownership-repairs/hirsel.json
hirsel run --native --backend cranelift examples/ownership-repairs/hirsel.json
hirsel check --json crates/hirsel-driver/tests/fixtures/ownership-repairs-6/original.hir
```

The last command intentionally exits nonzero. Each reject has a local manifest;
run the same `check --json` command on each of these files:

| Fixture under `crates/hirsel-driver/tests/fixtures/ownership-repairs-6/` | Intended rule | Misuse |
| --- | --- | --- |
| `original.hir` | MODE-1 | Borrowed payload returned without `in data` |
| `view-live.hir` | MODE-1 | Replace source before a returned view's last use |
| `view-escape.hir` | MODE-1 | Return a view of a borrowed parameter as global |
| `take-reuse.hir` | OWN-2 | Read an input after moving it |
| `copy-still-borrows.hir` | MODE-1 | Pass `copy data` from a borrowed parameter to a global taken parameter |
| `take-borrowed.hir` | OWN-1 | Try to move from a borrowed parameter |

`view_last_use` is the accepted twin of `view-live`: the same reassignment is
allowed after the view's last use. A view of an owned heap argument can also
leave that argument binding's scope:
the result owns counted shares and the global argument lends no bound loan
(MODE-1). `heap_view_outlives_input` checks this separately. This does not allow
reassigning the source while the view is still used. A borrowed parameter
remains local to its own root, which is what `view-escape` rejects.

The transferred and copied results each leave a function-local input's scope. `copy_independence` replaces the source while the
result is live, then replaces the result and reads the source again. Separately,
`equal_payloads` compares independently built owned and borrowed binary inputs
against the same expected bytes; equal contents alone say nothing about loans.

The bounded Rust harness checks the package under checked interpretation, then H,
LLVM and Cranelift with diagnostic runtime leak/ownership checks. It requires all
three configuration results and all six intended rejects, checking rules and
source spans without a rendering snapshot:

```sh
python3 tools/buck2/prepare.py
.buck2/bin/buck2 targets 'root//:'
.buck2/bin/buck2 test //:hirsel-driver-test-it-shard-1 -- --test-arg=ownership_repairs_6
```

With a positional filter, shard 1 runs the complete selection (two Rust tests),
including the six reject fixtures and three execution configurations. These
lane-private fixtures are not source-corpus/native-campaign registrations.
No corpus selector is needed; a zero-test run is not evidence.

Prior art: Rust's `Cow` borrowed/owned distinction at
[`alloc/src/borrow.rs:167–179`](https://github.com/rust-lang/rust/blob/f6c1107f8be13e48465e12c0fd26e6a39fe18e4f/library/alloc/src/borrow.rs#L167)
and independent equal-payload controls at
[`alloctests/tests/borrow.rs:7–28`](https://github.com/rust-lang/rust/blob/f6c1107f8be13e48465e12c0fd26e6a39fe18e4f/library/alloctests/tests/borrow.rs#L7)
(MIT/Apache-2.0). Only that test structure is adapted. Hirsel deviations: use
`Bytes` including NUL/0xff/CRLF and the original NetPeer fields; spell three
repairs as separate functions rather than a Cow enum; use Hirsel relations,
transfer and deep uniqueness rather than Rust references, lifetimes or lazy
clone-on-write. The numeric `[1, 2, 3]` control is also retained for lifetime twins.

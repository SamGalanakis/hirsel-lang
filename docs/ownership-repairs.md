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

## Record, binding and literal repairs

The [compact executable twins](../examples/ownership-repairs/authoring.hir)
extend the [NetPeer choices above](#netpeer-ownership-repair-lab). The pure
[entry](../examples/ownership-repairs/authoring-main.hir) checks payloads,
last-use borrowing, independent replacements, literal values and typed error
arms. The [I/O entry](../examples/ownership-repairs/authoring-io.hir) exercises
an actual file after rebinding and both capability lookups. Neither entry
compares diagnostic text to decide success.

Each rejected file below is under
[`tests/fixtures/authoring-repairs-11/`](../crates/hirsel-driver/tests/fixtures/authoring-repairs-11/).
Run `hirsel check --json` on it: each isolates one error. The harness checks
its rule and exact primary span, including UTF-8 byte offsets.

| Friction | Rejected file / minimal operation | Accepted twin in `authoring.hir` | Rule |
| --- | --- | --- | --- |
| dogfood7 F02: borrowed record path | `path-copy.hir`: `fn name(asset: Asset) -> String { copy asset.path }` | `path_view` returns `String in asset`; `path_unique` returns `unique asset.path` as an independent `String` | MODE-1 |
| dogfood7 F02: partial record move | `partial-move.hir`: `take value.assets` | `paths` consumes/destructures Manifest, iterates `take assets`, then `path_taken` consumes/destructures each Asset | OWN-8 |
| dogfood7 F02: later use after consumption | `record-reuse.hir`: `asset.path` after `let Asset(path: name) = take asset`; `field-reuse.hir`: `name` after `take name` | `path_taken` returns the owned field at its last use; `records` reads the returned path | OWN-2 |
| dogfood7 F03: mutable match binding | `mutable-pattern.hir`: `let Ok(mut f) = ... else { ... }` | `read`: `let Ok(f) = ... else { ... }`, then `let mut file = take f` and `file.read(d, mut file, ...)` | LANG-13 |
| dogfood7 F04: sequence context | `empty-sequence-context.hir`: `for name in []` | `sequence-context.hir`: `for name in ["a", "b"]`; `bindings_and_literals` supplies an explicit `List[String]` context | LANG-2 |
| dogfood7 F05: capability error domains | `file-error.hir`: plain `?` on `io.dir` with only `Grant(GrantError)`; `grant-error.hir`: stdout with only `File(FileError)` | `directory` uses `? File`, `output` uses `? Grant`; `AppError = File(FileError) \| Grant(GrantError)` | LANG-8 |
| dogfood8 F02: take versus unique | `unique-take.hir`: `Ok(take config)` returned as `Result[unique V2, ()]` | `migrate`: `Ok(unique config)` | MODE-2 |
| dogfood8 F03: UTF-8 byte spelling | `byte-literal.hir`: `b"雪"` | `bindings_and_literals`: `b"\xE9\x9B\xAA" == string.as_bytes("雪")` | SEM-3 |
| dogfood8 F04: integer receiver context | `numeric-context.hir`: `65537 - list.len(data)` | `bindings_and_literals`: `65537u64 - list.len(data)` gives `65534` for three bytes | LANG-2 |

Since 81c2fd39, [LANG-2](spec/language.md#lang-2-prefix-law) defaults a
context-free nonempty sequence to core `List` from its first element. The original
`sequence-context.hir` is therefore an accepted control on H, checked heap, LLVM
and Cranelift; the empty literal retains the missing-context rejection and exact
span check.

`copy asset.path` shares counted path storage and retains the borrowed
parameter's region. A result relation keeps that borrow explicit; `records`
replaces the asset only after the view's last use. Consuming the entire owned
record transfers its fields to bindings that can be moved separately. It does
not permit a partial `take` from the still-bound record or reuse of the consumed
record/field. The transfer adds no path deep copy; building output lists can
still allocate. `unique asset.path` instead pays O(path size) when a deep copy is
needed. As with NetPeer, uniqueness retains arena/region placement; it is no
promise to promote an arena value to the heap.

`take` transfers an owned value that may still share storage. It cannot prove
the stronger result mode `unique V2`. `migrate` explicitly constructs that mode
with `unique config`; this may deep-copy the DTO and its counted fields,
O(value size), while preserving placement. Its runtime control supplies a shared
heap-backed name, then replaces the source while reading the result. Equal
payloads alone do not prove the borrow or uniqueness rules.

Pattern matching and mutable storage are separate steps. The file repair first
binds `f`, then moves it into a mutable local. This also works for an owned value
inside a `Result`; the pure control increments a number after the same rebind.
A sequence in a `for` receiver has no expected element type, so provide
`List[String]` first. Similarly, the arithmetic receiver literal needs `u64`
even though `list.len` returns `u64`. Byte literals contain ASCII and explicit
byte escapes; `string.as_bytes` supplies a borrowed UTF-8 view of text instead.

`io.dir` returns `FileError`, while `io.stdout` returns `GrantError`.
`NotGranted()` can occur in both domains without making their types equal.
The two explicit mappings preserve that distinction in `AppError`; the pure
controls check both typed `NotGranted` arms, and the I/O entry checks successful
lookups. These recipes use the current APIs and preserve every existing rule.

Run the pure entry without grants:

```sh
hirsel run examples/ownership-repairs/authoring.hir.json
hirsel check --json crates/hirsel-driver/tests/fixtures/authoring-repairs-11/path-copy.hir
.buck2/bin/buck2 test //:hirsel-driver-test-it-shard-1 -- --test-arg=authoring_repairs_11
```

The new selection runs two Rust tests: eleven isolated rejects and two accepted
entries under checked interpretation, H, LLVM and Cranelift (six configuration
agreements). The I/O harness grants its own private directory and creates
`input` there. For a manual I/O run, mount a private writable directory at the
empty virtual root and grant stdout:

```sh
lab_dir=$(mktemp -d)
printf '%s\n' '{"io":{"stdout":"stdout.bytes@1","dirs":[{"path":"","host":".","access":"read_write"}]}}' > "$lab_dir/grants.json"
hirsel run examples/ownership-repairs/authoring-io.hir.json --grants "$lab_dir/grants.json"
```

The I/O entry writes only `input` in that mount; relative `host` resolves beside
the grants file. Build each entry once per native backend and reuse it as in the
[native verification recipe](native-verification.md).
The original `ownership_repairs_6` selection and all six NetPeer rejects remain.

This extends the Rust `Cow` borrowed/owned and equal-payload control structure
cited above (Rust Project Contributors, MIT/Apache-2.0; existing notice in
[third-party notices](../third-party/NOTICES.md)). Adaptations: Asset/Manifest
path fields and a `Result[unique V2]` replace Cow; Hirsel whole-record
consumption, relation and uniqueness rules replace Rust reference/lifetime and
clone-on-write machinery. `[1, 2, 3]` and `"hello"` controls are retained; binding,
UTF-8 and capability-domain cases come from the concrete Hirsel authoring
failures, not a port of Rust policy.

## Input boundaries

The [executable boundary recipes](../examples/ownership-repairs/input_boundaries.hir)
compose the missing dogfood9/10 repairs. `signed_field` accepts 1–20 UTF-8 bytes
including at most one initial ASCII `+` or `-`, followed by one or more ASCII
digits. It validates the entire field before stripping a plus. Whitespace,
embedded/doubled signs, bare signs, non-ASCII digits, fractions and prefixes
are `scalar.Invalid`; a valid bounded decimal outside i64 is `OutOfRange`.
The byte cap is application policy: even zero spelled with 21 bytes is invalid.
Negative minimum, positive maximum and signed zero are accepted. This extends
[tabfold's validated plus adapter](../examples/tabfold/model.hir) to the full
i64 range; tabfold retains its own narrower value bounds. The
[strict std parser](../stdlib/std@1/i64.hir) still rejects a leading plus.

`bytes_view` borrows with `in data`; `bytes_success` consumes a counted `Bytes`
and explicitly returns `Ok(unique data)`. The independent rejected snippet is:

```hirsel
fn make_bytes(take data: Bytes) -> Result[unique Bytes, ()] {
  Ok(take data)
}
```

`hirsel check --json` rejects `Ok(take data)` with MODE-2. Repair that
expression to `unique data`, as in the existing
[record uniqueness recipe](#record-binding-and-literal-repairs). Taking counted
bytes alone does not establish uniqueness; making a borrowed/shared input unique
can copy O(payload size) and preserves placement. The runtime control releases a
view at last use, then replaces a shared source while its unique result stays live.

`input` composes Directory lookup, File open and a 64-byte bounded read. Its
`directory_result`, `grant_result` and `file_result` seams preserve
`Directory(FileError)`, `Grant(GrantError)`, `File(FileError)` and `Limit`.
Reuse the [existing explicit error mappings](#record-binding-and-literal-repairs)
and [typed List[cli.Flag] recipe](cli-errors.md); their definitions need no new
variant here. Synthetic controls distinguish both domains' `NotGranted`, file
`NotFound`, `FileFault(1)` and `FileFault(73)`. The codes are provider-defined:
tabfold's observed path escape `FileFault(1)` is an observation, not a promise
for all malformed paths. There is no `InvalidPath` arm.

```sh
hirsel test examples/ownership-repairs/hirsel.json
hirsel test --native --backend llvm examples/ownership-repairs/hirsel.json
hirsel test --native --backend cranelift examples/ownership-repairs/hirsel.json
.buck2/bin/buck2 test //:hirsel-cli-test-cli-shard-1 -- --test-arg=authoring_input_recipes_13
```

Each package run selects three tests. The CLI filter selects three Rust tests:
the package matrix, one isolated reject with its accepted constructor repair,
and a composed I/O entry with success, missing file, read limit, denied Directory
and denied Grant controls on H/LLVM/Cranelift. The I/O check builds once per
native backend and reuses the executables. For launcher help and record-prefix
ordering, use `hirsel run --help` and the shipped
[native verification](native-verification.md) and [build/launch](building.md#build-and-launch-a-program)
recipes. Native record prefixes precede grants; flags after the launch input
belong to application Args. The dogfood9 inner-object binding collision remains
the deliberate LANG-4 no-shadow rule; rename that binding. These dispositions
require no compiler, std grammar, filesystem mapping or launcher change.

Local authority: tabfold/model.hir:112–146, std i64.hir:34–42 and
authoring.hir:30–42 at `8cd5f338c97f9afc2cadcf08fd49b724e91d3d21`.
Adaptations are the explicit 20-byte wire cap, full i64 range, preserved
Invalid/OutOfRange errors and Hirsel borrow/unique success modes. These are local
Hirsel recipes, not an external implementation port; the Rust control-structure
license and notices above remain applicable.

# metricfold authoring friction

Launch baseline: origin/main `f36e8f4b04d616e24f63e1b74fa4ba97bd46f466`.
The first complete source attempt was saved before any corrections. Distinct
external attempt directories retain sources, manifests and JSON diagnostics;
raw logs and TIR exports are outside this repository. The current example
inventory had no qualified metrics consumer; logq/logsummary/logdigest count
messages rather than bounded declared metric instruments. This is bounded
novelty evidence, not a universal search claim.

No compiler or std change was needed for the application. The following records
include intentional recipes and extra plumbing that remain in the final code.
Times are approximate investigation time, not measurements of performance.

## Integer context (about 1 minute; intentional LANG-2)

First source, `main.hir:35:29`, one-byte primary span:

```hirsel
let end = if n == 0 { 0 } else { i + 1 }
```

Actual JSON diagnostic: `LANG-2`, "literal needs an expected type or numeric
suffix". The first branch's literal has no numeric context. Working repair:
`let end: u64 = if n == 0 { 0 } else { i + 1 }`. Other numeric counters and the
empty `List[u8]`/`List[telemetry.Attribute]` builders also have written types;
those documented forms were used directly, without a separate failed attempt.

## Reused local name (about 1 minute; intentional LANG-4)

First source, `model.hir:100:17-21`, with the earlier `text` at `64:10`:

```hirsel
let Ok(text) = string.from_bytes(copy data) else { /* return error */ }
// Later, inside the same function:
Some(Number(text)) => /* parse the number */
```

Actual diagnostic: `LANG-4`, "a local cannot shadow another local". Working
repair: call the numeric spelling `lexeme`, including its parse uses. No parser
or name-resolution change was made; those seams remain owned by other lanes.

## Stream bytes to owned text (about 3 minutes; intentional MODE-1)

Second source, `model.hir:64:18-46`:

```hirsel
fn measurement(data: Bytes, number: u64) -> Result[metrics.CheckedMeasurement, InvalidInput] {
  let Ok(text) = string.from_bytes(copy data) else { /* return error */ }
  // ...
}
```

Actual diagnostic: `MODE-1`, "this operation requires a global value: `data` is
local to the caller, but `string.from_bytes` requires a global argument".
The diagnostic explicitly says that `copy data` keeps locality and offers
`take data` or `unique data`. Working repair: declare `take data` in both
`accept` and `measurement`, pass `take data` onward, and finally call
`string.from_bytes(take data)`. Scanner output is already owned, so this path
needs no additional deep copy of the line. This is the documented ownership
recipe, not a new std gap or language-rule proposal.

## Borrowed iteration (about 2 minutes; intentional OWN-1)

Third source, `main.hir:47:11-44`:

```hirsel
for line in lines {
  model.accept(mut fold, take line)?
}
```

Actual diagnostic: `OWN-1`, "a borrowed value cannot be taken: `lines` is local
to this function, but this needs an owned value". Working repair:
`for line in take lines`. This consumes the scanner's owned list and allows
moving each line to the typed validator. A matching consuming-loop recipe was
read in std before applying it. No ownership implementation was changed.

## First-error ordering with line_scan (about 3 minutes; application composition)

`line_scan.feed` returns all complete lines from one chunk, but a later
`LineTooLong` error discards that call's output. Feeding an entire read before
validating its first line could therefore report a later long line instead of
an earlier malformed record. No failed compiler source or diagnostic applies.
Working recipe: split each bounded 256-byte read at LF, feed one such segment
at a time, consume its owned lines immediately, then advance. A dedicated
`{\n` followed by a 1,025-byte line checks that line 1 wins. The scanner retains
only the current bounded partial line. This follows the shipped feed contract;
it does not require a scanner change or a speculative shared helper.

## Owned fields and snapshots (about 2 minutes; documented mode plumbing)

These forms were used directly; no failing diagnostic was observed:

```hirsel
Some(Text(value)) => Ok(unique value)
Some(Object(labels)) => copy labels
telemetry.Attribute(name: unique key, value: Text(unique label))
metrics.Descriptor(name: unique name, /* other fields */ unit: unique unit)
```

Map lookups and iteration provide views. The returned text, stored label fields
and descriptor fields need independent ownership, so `unique` detaches their
small bounded values. `copy` shares the owned parsed object's map while it is
inspected. `metrics.snapshot` supplies its own independent projection. The
model keeps the opaque Registry inside a typed Fold record and mutates it
through `metrics.record(mut fold.registry, checked)`; no representation access
or capability grant is introduced.

## Typed errors, output and grants (about 4 minutes; application policy)

No failed diagnostic applies. Metric errors are explicitly matched into the
app's Reason variants, then paired with the physical line number in
`InvalidInput`. The helper preserves different dispositions for descriptor,
label, value and arithmetic failures. `AppError.Output(OutputError)` supplies
the nominal injection arm for stdout operations. `process.exit_code` permits a
typed JSON rejection on stderr with status 2 and missing-output status 3,
without terminating before cleanup.

The report uses a small application-specific writer and
`json.write_text_into` for label/name/unit escaping. It projects only the
counter/histogram fields this job tool needs, and spells out the overflow
policy. There was no attempted derive of foreign std records and no invented
serialization defect. Fixed registration order and canonical library labels
avoid an additional sort or hash-order dependency. The stdout/stderr handles
and read-only input directory are passed explicitly; there is no ambient
filesystem, clock, process or exporter authority.

## Harness setup (about 2 minutes; local authoring correction)

The first Rust test build failed with `E0425` at
`metricfold_23.rs:79:20`: `hirsel_driver::with_compiler_stack` does not exist.
Working repair: use `hirsel_engine::with_compiler_stack`, as the existing
harness imports do. That build ran zero tests and is recorded as setup failure.
A draft pure control also guessed `string.into_bytes`; API inspection found
only `string.as_bytes`, so it was changed before execution to
`unique string.as_bytes(line)`. That bounded test-only copy creates independent
bytes from an f-string. No compiler diagnostic or acceptance is claimed for the
unexecuted draft. The existing checked-H runner was consumed without edits.

## Reserved test local (about 1 minute; intentional LANG-14)

The first pure control used `let before = model.report(fold)`. Exported JSON
from the frozen compiler reports `LANG-14` at `control.hir:13:7-13`:
"`before` is reserved for a pre-state snapshot in a postcondition; choose
another name". Working repair: `saved_report`, including its two later uses.
The CLI engine matrix passed independently; its success did not count as a
checked-H pass when this control failed before execution. No parser change or
new diagnostic issue was proposed.

## Packaged grants path (about 2 minutes; documented launch policy)

The first checked-in template used host `examples/metricfold/fixtures`.
The documented root-level command rejected it before entry with exit 3:
`{"kind":"invalid","field":"io.dirs.host","detail":".../examples/metricfold/examples/metricfold/fixtures: No such file or directory (os error 2)"}`.
Relative host paths resolve beside the grants file, not the shell working
directory. Working repair: host `fixtures`, and correct that explanation in
README. The real filesystem paths are private fixtures in tests. The realistic
case now consumes the checked-in grants template unchanged on all three
engines, so dynamic absolute-path test grants cannot hide this packaging error.
The original refused launch and template are retained externally. This is an
application setup correction, not a compiler diagnostic or an API change.

## Prior art and bounded coverage

OpenTelemetry Rust SDK `histogram.rs:120-159,317-342` at
`fb6b53566815062f6be838d49cbea77019669d6f` supplies the fixed-bucket construction
shape and the 1-through-10 observation vector. Its Apache-2.0 license is retained
in `fixtures/LICENSE`. This application reuses the shipped metrics reducer,
keeps checked u64 state and the library's overflow-series policy, and selects
explicit integer milliseconds rather than SDK f64 state or std's conventional
nanosecond telemetry spelling. It has no threads, time sampling or exporter.

The scenarios enumerated in README and the focused harness cover these
recipes. A passing matrix establishes those bounded scenarios; it does not
claim universal ownership ergonomics, arbitrary schema support or all possible
compiler/runtime paths. No issue was routed to a shared owner or FOR SAM.


## Relative-host diagnostic guidance resolved (round 24)

Landed in `231d4880e4e6acd59b4da5d1f95ea7c02156f363`. The historical relative-host refusal above is retained.
A missing relative host now keeps `kind:"invalid"`, `field:"io.dirs.host"` and
the OS cause while naming the original host, grants-file directory and attempted
resolved candidate. It explains that relative host paths resolve relative to
the grants file. The checked-in `host:"fixtures"` and all grant/confinement
semantics stay unchanged. See the
[shared grants-base recipe](../../docs/native-verification.md#resolve-directory-hosts-beside-the-grants-file)
for a private bad/correct pair launched from another working directory.

Focused shared-parser/CLI tests cover the diagnostic, cwd independence,
relocation, nested runner mounts, escaped paths and authority controls. This
resolution does not claim that this example's native application matrix was
rerun.

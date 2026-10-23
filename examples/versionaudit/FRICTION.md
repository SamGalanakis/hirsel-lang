# versionaudit authoring friction

Frozen main: `f36e8f4b` (2026-10-23). Scope: the 35 bounded CLI scenarios,
44 range vectors, reachable TIR export, and the two pure checked-heap assertions.
No compiler, language rule, shared runner or std change was needed. Timings below
are approximate investigation time, not performance measurements. The external
lane evidence retains the first source/manifest, JSON diagnostics, failed test
receipts, minimized copies and toolchain/source hashes.

## Deep uniqueness across the name map and mismatch list (about 5 minutes)

The first form returned `Result[Row, InputError]` from `row` and inserted
`copy item.name` into a `Map[String, u64]` before moving `item` into the mismatch
list. `list.sort(mut mismatches, by_name)` failed MODE-2 at model.hir:80:3–37:
“`mismatches` is not proved deeply unique on every path”. The diagnostic traced
the push's potentially shared value and suggested ending shares or deep copying.
Changing only the helper to return `Result[unique Row, InputError]` still failed
at the same sort: the map retained a share of the name.

The minimized negative on the same frozen compiler is:

```hirsel
type Row { name: String }
fn row() -> unique Row uses {} { Row(name: "a") }
@noalloc
fn by_name(a: Row, b: Row) -> i32 uses {} { string.compare(a.name, b.name) }
pub fn main() -> Result[(), ()] uses {} {
  let item = row()
  let mut names: Map[String, u64] = map.empty()
  map.insert(mut names, copy item.name, 1u64)
  let mut rows: List[Row] = []
  list.push(mut rows, take item)
  list.sort(mut rows, by_name)
  Ok(())
}
```

Actual reduced diagnostic: MODE-2 at negative.hir:11:3–31, byte span 383–411,
“`rows` lost its deep uniqueness”. Replacing `copy item.name` with
`unique item.name` passes. The final application keeps that bounded deep copy and
the helper's unique return contract. Disposition: intentional ownership/sort API
rule; no routed issue or diagnostic repair. The copy is at most 128 UTF-8 bytes
per row. Avoiding the copy with a different collection/order design was not needed.

## Grants paths resolve relative to the grants file (about 1 minute)

Initial grants: `"host":"examples/versionaudit/fixtures"`. Launch exited 3
before entry: `field=io.dirs.host`, detail ended in
`examples/versionaudit/examples/versionaudit/fixtures: No such file or directory`.
There is no source rule/span because this is a launcher refusal. Repair:
`"host":"fixtures"`. Tests use an absolute private input directory.
Disposition: intentional grants path rule; documented in README.

## Literal braces in formatted JSON (about 1 minute)

The first successful H run exposed a missing final root brace in the summary:
the tail `...{count}}}\n` renders only one literal `}` after the interpolation.
The final tail is `...{count}}}}}\n`, rendering both closing object braces.
There was no compiler diagnostic; this was an application output bug caught by
inspection and then by exact independent JSON output comparisons. Disposition:
intentional formatting escape rule. JSON string values use `json.write_text_into`
so names, Unicode, quotes and version spellings remain escaped and preserved.

## Checked-heap entry receipt (about 2 minutes)

The pure test's first source used `pub fn main() -> () uses {}`. Source checking
accepted the definition, but execution setup refused it with
`InvalidEntry("entry must return Result[(), E], std@1/process.ExitCode or Result[std@1/process.ExitCode, E]")`.
This is an entry-contract refusal, with no source diagnostic/rule/span. Repair:
`Result[(), ()]`, `Ok(())` on success and `return Error(())` on defensive failure
branches. Disposition: intentional program entry contract, already documented
under program exit statuses. The application entry already used `process.ExitCode`.

## Harness count receipt and repeated selection (about 1 minute)

The first application verifier completed 35 cases × 3 engines = 105 successful
cells, including cleanup. Its Rust receipt incorrectly expected 38 cases and
114 observations. Correcting those literals to 35/105 changes no oracle answer.
The first focused run therefore reported 0 passed / 2 failed Rust tests (the
second failure was the checked-heap entry above). No engine fixture failed.
The full focused retry had already started when the coordinator requested a
separate heap-only retry. It was allowed to finish with frozen consumed inputs;
its 105 repeated cells are disclosed in the external report. No third unchanged
application selection was launched. Disposition: harness bookkeeping error.

## Explicit forms retained in the final code

These forms were chosen from documented APIs and passed without a prior failed
attempt; they remain part of the ergonomic inventory rather than defects:

- Empty collections/options and initially absent string slots have explicit
  `List`, `Map`, `Set` and `Option` annotations. Counters use `0u64` and limits
  use context/suffixes under LANG-2. Sorting needs a named `@noalloc` comparator.
- The row reader owns decoded fields through `json.read_owned_text`; the input
  reader and cursor stay borrowed. The owned helper above prevents repeated
  record-level deep copies before sorting. Record and tuple constructors have
  explicit field/type context where needed.
- `InputError.Json(json.JsonError)` lets ordinary `?` inject JSON errors. Two
  semver arms represent different roles, so explicit matches wrap the row index
  as `Requirement(index, error)` or `Version(index, error)`; plain `?` could not
  choose either role. A helper maps the four semver error arms to stable report
  detail/offset fields. No payload is silently discarded.
- Small `append`, `encode` and `encode_error` helpers produce fixed field order,
  following archiveaudit's output structure. They avoid depending on hash-map
  iteration order and keep all transformation/rendering pure. No generic helper
  library was introduced.
- `Io`, `Args`, `Dir` and stdout are explicitly passed/projected. File/grant
  errors are mapped to application variants; the entry returns `exit_code(2u8)`
  for invalid input and output failures. It does not return `Error`, which would
  impose the runner's status-1/error-rendering policy. Only stdout and read-only
  fixture directory authority are granted.
- The Rust harness embeds both modules, manifest, fixtures, verifier and capped
  launcher with `include_str!`, rebasing only the std manifest in its private
  package. This is required remote-test input plumbing, not a compiler repair.
  tools/capped's own report is separated from child stderr through a small
  verifier child mode, so application stderr is compared exactly.

No other workaround was observed in these enumerated scenarios. This does not
claim universal ergonomic or semantic-version coverage. Std's parsed `Req` and
`Version` are transparent types on this pin; this application treats them only
through parse/matches APIs and makes no opaque-representation coverage claim.

## Contextual-error guidance resolution

The two semver matches described above now use the shipped `results.map_error`.
Both callbacks take the semver cause and capture an immutable snapshot of the
current row. They return exactly `InputError`, choosing `Requirement(index,
take error)` or `Version(index, take error)` explicitly, so bare `?` propagates
the already contextual error. No Result API or unique `Row` contract changed.
Affected success/order and requirement/version failures at rows 1 and 2 retain
exact report bytes, semantic role, row and cause offset through H, LLVM and
Cranelift. The original authoring errors above remain preserved; the closure
recipe resolves the manual-context plumbing, not the intentional error-arm rule.

Contextual-error guidance and these application rewrites landed in `ea29a8642f1a465689e71ebea18e6abfa740fd34`.

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

## Bounded decoded JSON fields (round 36)

Row fields now use `json.read_owned_text_bounded` with 128 bytes for names and 256 for requirements/versions.
The earlier read-then-measure recipe could allocate a complete rejected escaped
field. The shared reader admits decoded UTF-8 spans/scalars before appending,
then validates overflow without retaining the suffix. `Limit` maps to the
existing `FieldBytes(index)`; malformed strings retain their JSON cause and
offset even after overflow. The 64 KiB whole-input cap remains necessary to
bound scanning work. This removes the duplicated post-allocation field check.

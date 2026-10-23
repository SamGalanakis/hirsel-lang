# invoicecheck authoring friction

Scope: bounded CSV → typed Line → exact Decimal multiplication/HalfEven rescale →
invoice accumulation → deterministic JSON; actual CLI H/LLVM/Cranelift, pure
checked H, grants and rejected inputs. These findings do not claim universal
coverage. No compiler, language rule, std implementation or shared runner changed.
Initial source was preserved before corrections; the frozen launch base was
`61f42cb717b0cde64cc2afb2fc9074bac5bd0048`. Raw source snapshots and JSON diagnostics
are external to the repository.

1. **Inline error mapper** (about 3 minutes; intentional LANG-8 syntax).
   Attempt:
   ```hirsel
   let header = csv.next_record(mut reader) ? fn(e: csv.CsvError) -> InvalidInput {
     InvalidInput(record: 0, reason: Csv(e))
   }
   ```
   Actual first diagnostic: LANG-1, `expected newline or semicolon after complete
   statement/item`, initial `model.hir:98:55–57` on `fn`; the same form recurred at
   99:46, 107:46 and 111:33. Language LANG-8 explicitly requires a path after `?`.
   Repair retained in reconcile: explicit `match`, wrapping `Csv(e)` with the
   current data-record index. This also avoids capturing a changing counter in a
   mapper. No diagnostic repair requested: this is an unsupported authoring form.

2. **Module name beats field access** (about 2 minutes; intentional name-resolution
   rule, existing diagnostic gives a legal repair).
   ```hirsel
   for group in mut groups { group.lines += 1 }
   ```
   Actual LANG-4: `before '.', 'group' names a module; rename the local 'group' to
   access its fields`, second attempt `model.hir:131:10–26` and subsequent group
   projections. Repaired every loop local to `acc`, including report projection.
   No checker edit or new issue: the explicit diagnostic already explains this
   module-head restriction. This is a retained local-name workaround.

3. **JSON variant spelling** (about 1 minute; intentional LANG-4, author API mistake).
   ```hirsel
   json.stringify(json.String(unique reason))
   ```
   Actual LANG-4 `unresolved module member 'json.String'; looked for
   std@1/json/String`, second attempt `main.hir:19:29–40`. The declared value
   variant is `Text(String)` and variants are bare/contextual. Repair:
   `json.stringify(Text(unique reason))`. No std gap.

4. **Nested JSON derivation convention** (about 3 minutes; documented library
   convention, extra helper retained).
   ```hirsel
   fn report_json(report: Report) -> json.Value uses {} = comptime derive.json_value(shape(Report))
   ```
   Report contains `List[Invoice]`. Actual LANG-4: `unresolved name
   'invoicecheck@0/model/invoice_to_json'` at generated source line 7:29–65,
   with generator label at second-attempt `model.hir:154:56–97`.
   Repair: define `fn invoice_to_json(invoice: Invoice) -> json.Value uses {} =
   comptime derive.json_value(shape(Invoice))`. The derive is intentionally
   compositional rather than recursively synthesizing domain converters.

5. **Relative grant host** (about 1 minute; launch configuration semantics).
   Initial host was `examples/invoicecheck/fixtures` in a grants file already
   under `examples/invoicecheck`. Actual launch rejection:
   `{"kind":"invalid","field":"io.dirs.host","detail":
   "examples/invoicecheck/examples/invoicecheck/fixtures: No such file or directory (os error 2)"}`,
   exit 3, before program entry. Repair: host `fixtures`, relative to the grants
   file. Harness grant hosts are private absolute paths. README records the rule.

Other explicit plumbing retained, without a failed diagnostic:

- Empty collections and numeric control counters have type annotations/suffixes
  under LANG-2. Result payloads are extracted with `take` from matches; returned
  lists use `take`. No inferred generic-context or ownership repair was needed.
- CSV exposes strings/field views. `decode` constructs a typed `Line` and validates
  quantity and amount policies; this is application validation, not a new CSV API.
  `unique id`, `unique line.invoice_id` and `unique acc.invoice_id` make stored
  identifier strings independent of the borrowed row/accumulator; the error JSON
  uses `unique reason`. These copies are explicit and retained.
- `let Some(...) else` guards remain even after width validation because field
  access returns `Option`; no unchecked field extractor was added.
- Decimal quantities use `decimal.new(bigint.from_u64(quantity), 0)` because there
  is no implicit integer-to-opaque-decimal conversion. Decimal report fields use
  `to_string`, preserving scale for totals and normalization for expected sums.
- I/O errors are translated into stable structured reasons; CSV errors retain their
  typed payload inside the model but the public error report intentionally groups
  syntax/width errors as `csv`. Explicit ExitCode distinguishes invalid input from
  reconciliation mismatches. Output/JSON/Grant errors use nominal arm injection.
- The harness captures application streams inside `tools/capped` so resource
  accounting does not contaminate stderr assertions. Native compilation remains
  setup; individual fixture processes have a 60 CPU-second / 2 GiB bound.

## API discovery resolution

The [tested std API recipes](../../docs/std-api-recipes.md) address the API-search
detour recorded above: `json_access.member`/`element`, contextual `Text(String)`,
core `string.from_bytes(take data)` and manifest-registered `time.format`.
The original attempts remain historical evidence. The production calls, required
annotations, ownership copies and application adapters remain valid. This is
documentation navigation for existing APIs, with no alias or resolver change.

Addressed by the executable recipe documentation landed in `36879d8e307bb032944a618df036327bff837fe5`.

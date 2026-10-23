# jsonpatchctl authoring record

Frozen launch main: `f36e8f4b04d616e24f63e1b74fa4ba97bd46f466`.
The nearest examples were configctl (bounded configuration files) and docsession
(line sessions with merge patch). An inventory of qualified `json_patch` calls
and RFC6902 references found no existing complete example at launch. This is a
bounded inventory, not a claim about every possible equivalent program.
Live ownership was checked before implementation. Only the new application,
its focused tests, the example index and test cost rows are changed; no std,
compiler, language rule or owned round21/22 seam is changed.

Attempts and exact JSON diagnostics are retained externally. All source checks
below ran on the frozen main compiler and matching std image. Times are rough
active investigation time, not benchmark results. No rejected form was repaired
by relaxing a check or changing the language.

| Attempt / minimal form | Actual diagnostic and original span | Working form | Time / disposition |
| --- | --- | --- | --- |
| 01: `let result = model.apply(document, patch_text)?` | LANG-14, main.hir:37:7: `` `result` is reserved for the returned value in a postcondition; choose another name `` | Rename binding and use to `patched`. | ~1 min; intentional reserved-name rule. |
| 02: `string.from_utf8(data)` | LANG-4, main.hir:15:18: `unresolved module member string.from_utf8; looked for core@1/string/from_utf8, std@1/string/from_utf8` | Use declared `string.from_bytes(take data)`. | ~1 min; author used the wrong API name. |
| 03: `text.from_bytes(data)` | LANG-4, main.hir:15:18: `unresolved module member text.from_bytes; looked for std@1/text/from_bytes` | Inspect the actual UTF-8 constructor and use `string.from_bytes`. | ~1 min; second incorrect API name, not a std gap. |
| 02: `type InvalidInput = Arguments; arg.text(args, 0) ? Arguments` | LANG-8 at main.hir:23:8, :30:23, :31:20: `variant mapper needs exactly one matching error payload`; required `fn(ArgError) -> InvalidInput`. | `Arguments(ArgError)` retains the typed payload; rendering deliberately groups it as `Arguments`. | ~1 min; intentional mapper rule, no nullary payload-discard shorthand. |
| 02: `fn apply(document: String, ...) { json.parse(copy document) }` | MODE-1, model.hir:36:18: ``this operation requires a global value: `document` is local to the caller, but `json.parse` requires a global argument``. | `json.parse(unique document)`; keep the public text snapshot borrowed and immutable. | ~2 min with following copies; intentional locality rule. |
| 02: `json_patch.parse_text(copy patch_text, limits())` with borrowed `patch_text` | MODE-1, model.hir:37:15: same global-argument requirement, naming `patch_text` and `json_patch.parse_text`. | `unique patch_text` creates an independent bounded parser input. | Included above; intentional locality rule. |
| 02: `fn simple(stage: String, ...) -> Rejection { Rejection(stage: copy stage, ...) }` | MODE-1, model.hir:53:12: ``this operation requires a global value: `stage` is local to the caller, but the result must be global``. | `unique stage`, `unique kind`, `unique detail` in the returned record. The latter two repairs were made with the first; no separate failure is claimed. | ~1 min; intentional locality rule; small bounded copies remain. |
| 02: returned rejection with `Field(name) => copy name` and `path: Some(copy cause.path)` | MODE-1, model.hir:109:20, on `copy cause.path`: `` `cause` is local to this function, but the result must be global ``. | Deep-copy the selected field name and original encoded pointer with `unique`. | ~1 min; intentional lifetime rule; both copies remain. |
| 02: `fn valid_name(name: String) { string.split(copy name, "/") }` | MODE-1, model.hir:152:15: `name` is local to caller, `string.split` requires a global argument. | `string.split(unique name, "/")`; filename is already limited to 256 bytes. | <1 min; intentional global input requirement. |
| 05: `let raw = r#"[...]"#; json_patch.parse_text(raw, limits())` | OWN-2, model.hir:168:20: `a take parameter requires an owned argument: the argument is a place passed without take, but the parameter is take`. JSON fix inserts `take ` at column 42. | `json.parse(copy raw)` for the DOM parse vector, then `json_patch.parse_text(take raw, limits())` at its last use. | <1 min; intentional explicit move. |
| First real fixture launch: grants file with `host: "examples/jsonpatchctl/fixtures"` | Tool status 3, field `io.dirs.host`: doubled example-relative path does not exist. No language rule/span. | `host: "fixtures"`: host paths resolve relative to the grants file. | ~1 min; capability setup correction. |
| First Rust harness compile: captured `manifest` plus `use hirsel_driver::native::*` inside closure | Rust E0308 at harness.rs:105:54: expected `&Path`, found `&fn(&NativeInput) -> ... {manifest}`. Zero tests executed. | Rename the captured path to `selection` to avoid the imported helper name. | <1 min; host harness name collision. |
| First executed integration run: expect replacement to retain Python dictionary member position | `copy`, interpreter: actual document has `b` before `a`; independent expected document has `a` before `b`. Checked heap passes; integration fails. | Compare fixed semantic answers independent of object member order; separately require compact encoding and identical bytes across engines and repeated promotion. No case removed or reduced. | ~2 min; oracle correction for documented JSON object semantics. |
| First standalone local verifier native build without launcher environment | Build status 2: `LLVM_SYS_231_PREFIX must name an absolute toolchain input`. No runtime engine cell executed. | Build the pinned native input targets and export absolute LLVM, runtime archive and link-kit paths. Pool harness already receives them through the documented test launcher. | ~2 min; local setup correction. |

Remaining explicit authoring costs in the final code:

- `InputError`, `InvalidInput`, `AppError` and `Rejection` separate parser/patch,
  input and output failures. `Model(model.InputError)` and `Read(file.ReadBoundedError)`
  preserve nested Results through `?`; the report deliberately groups file host
  errors while retaining every patch kind, index, pointer and nested JSON offset.
  `Ok(model.success(patched)?)` and `Ok(json_patch.apply(...)?)` explicitly wrap
  the successful payload. These are documented LANG-8 recipes; no additional
  diagnostic was observed for these final forms.
- `simple`, `json_detail`, `patch_kind`, `append` and `describe` provide explicit
  error context/JSON encoding without a new shared abstraction. Borrowed report
  strings require the bounded `unique` copies listed above. User strings always
  go through `json.write_text_into`; f-strings interpolate only encoded JSON or
  numeric offsets. This is application policy, not a missing std helper finding.
- `Some(copy original)` makes the document snapshot explicit. The pure test
  retains both `original` and `copy original`, checks unchanged contents after
  success and after a failure following root removal, and reinspects the parsed
  patch. This counted share is deliberate; std does its private working copy.
- `List[u8]` builder annotations, `Option[u64]`/`Option[String]` report fields,
  the `0u64..` convention in consumed APIs, and explicitly typed numeric limits
  follow prefix typing. No later inference or expression type arguments were
  attempted as a repair. Pure helpers declare `uses {}`. Output/grant plumbing
  remains in main; `process.ExitCode` permits status 2 without a second launcher
  error line. Help obtains no directory grant.
- Checked-heap testing uses the existing public load/check/export/import/build/
  `run_heap` APIs with an alternate entry reaching `model.verify`; the shared
  runner is untouched. Test packaging uses literal `include_str!` inputs and
  private temporary sources, fixtures and launch files. `tools/capped` metadata
  is captured separately from application stdout/stderr by a tiny verifier
  subprocess; it cannot be mistaken for application diagnostics.

No new compiler/std defect was demonstrated in these scenarios, and no issue was
routed. This does not claim universal ergonomic coverage. Resource ceilings were
not raised. Original failed attempts remain failures; passing later forms do not
retroactively validate them. The standalone acceptance matrix executed 42 cases
through all three engines (126 runtime observations), plus one pure source test. All passed with source
check and reachable TIR export. Runtime caps stayed at 60 seconds / 2 GiB; the
observed maximum was 163 MiB and 3.6 seconds wall time per cell. These are
bounded acceptance observations, not performance claims. The final focused
Buck2 run executed both Rust tests: 2 passed, 0 failed,
including all 126 engine observations and the checked heap with zero live heap
bytes. The new-test baseline was 0 tests. Earlier setup failure ran 0 tests;
the first executed harness ran 2 tests (1 passed, 1 failed due to the object-order
oracle). The successful final measured case costs, 4.076 s and 117.547 s, were
merged into the existing test-cost table without a wider campaign.

## API discovery resolution

The [tested std API recipes](../../docs/std-api-recipes.md) address the API-search
detour recorded above: `json_access.member`/`element`, contextual `Text(String)`,
core `string.from_bytes(take data)` and manifest-registered `time.format`.
The original attempts remain historical evidence. The production calls, required
annotations, ownership copies and application adapters remain valid. This is
documentation navigation for existing APIs, with no alias or resolver change.

Addressed by the executable recipe documentation landed in `36879d8e307bb032944a618df036327bff837fe5`.

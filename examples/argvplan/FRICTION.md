# argvplan authoring friction

The initial package was checked on frozen main `f36e8f4b04d616e24f63e1b74fa4ba97bd46f466`.
Distinct original attempts and JSON diagnostics were retained outside the repo.
No compiler, language rule or std implementation was changed for this package.
Times below are approximate active investigation time, excluding builds.

| Attempt / minimal form | Actual observation | Working repair | Time / disposition |
|---|---|---|---|
| `let mut result = Plan(schema: 1, commands: [])` | LANG-14, `result` reserved for returned value in a postcondition; model.hir line 48, bytes 1469–1475 in the saved first source | Rename the local and all its uses to `plan` | <1 min; intentional rule, LANG-14. Final code retains the renamed local. |
| `pub fn plan_json(value: Plan) -> json.Value uses {} = comptime derive.json_value(shape(Plan))`, with `commands: List[CommandPlan]` | LANG-4, unresolved `argvplan@0/model/command_plan_to_json`; generated plan_json line 7, bytes 351–388; secondary on model.hir line 95, bytes 2919–2958 in saved attempt 2 | Add `command_plan_to_json(value: CommandPlan)` with the same generator before the outer converter | <2 min; intentional documented nested-deriver naming convention in std-library.md, JSON derivation. Final code retains the helper. |

Additional explicit forms present in the first source (not failed experiments):

- Empty list builders have `List[u8]` annotations; counters start with `u64`
  suffixes, and `arg.text`'s conditional index has suffixed branches. These
  satisfy prefix typing, LANG-2; no later text is relied upon for inference.
- `model.invalid` copies its borrowed kind with `unique` into the returned
  record, preserving a global error payload. It is a deliberate helper/copy,
  rather than returning a borrowed String as global. Under MODE-1 this is the
  documented owned-result recipe; no rejected alternate form was tried.
- `words` matches the taken shlex result to preserve its unique list and maps
  offset payloads into the app's `InvalidInput` with a line number. Explicit
  match is needed to include per-call line context; a `?` mapper is a named
  unary path and cannot capture that context (LANG-8). No closure-mapper
  experiment or diagnostic claim is made.
- `read_plan` uses explicit matches for File/Grant failures and maps the
  bounded-read `Limit` arm to the application cap report. This capability
  plumbing keeps effects in main.hir and pure validation in model.hir.
- `AppError` explicitly wraps Grant, Output and Json errors so plain `?`
  injection has a unique payload arm. Expected input errors instead become
  typed stderr JSON with `process.exit_code(2u8)`; missing output authority
  becomes status 3. This follows LANG-8 and the existing process API.
- `text.finish_valid` is used only after file UTF-8 validation and LF-boundary
  scanning. `unique` copies input bytes in the checked-H harness because
  decode takes ownership; the production file reader already returns unique
  bytes. No ad hoc shared helper or parser was introduced.
- Formatting uses the existing formatter. Multi-line function headers and
  generator declarations are formatter output, with no semantic edits.

These choices are recorded even where documented syntax made the first form
legal. The enumerated application and upstream-vector checks do not establish
universal ergonomic coverage. Engine results and any additional harness
repairs are recorded below after verification.

Verification: source check and reachable TIR export pass. Both focused tests
executed and passed: 43 application cases × interpreter/LLVM/Cranelift = 129
runtime observations, plus 31 upstream vectors and model controls through the
existing checked-H helper. Fixture hashes, missing grants, deterministic
output, status, stderr and private input/grant/tempdir cleanup passed.

Setup friction: an initial standalone native build lacked
`LLVM_SYS_231_PREFIX` and exited 2 with `native link: LLVM_SYS_231_PREFIX must
name an absolute toolchain input` (<1 min investigation). The original setup
failure was preserved. Repair: use the documented Buck test launcher, which
supplies the pinned LLVM/link kit/runtime inputs. This is host setup, not a
source diagnostic or an application engine failure. No cap was raised and no
fixture was shrunk. No additional application workaround or compiler/std
change was needed in the enumerated engine scenarios. No open routed issue.

## Nested JSON diagnostic resolution (2026-10-23)

Landed in `66a5b0288e76496d525e829f2db8fefc0cc614a8`: ordinary JSON encoding now labels
the enclosing `Plan.commands` source field when `command_plan_to_json` is missing
or has the wrong contract. The label gives the canonical helper path and
directional borrowed-argument signature, alongside the original rule, generated
failure span and comptime origin. The explicit `command_plan_to_json` helper remains
necessary and is retained. See the [complete nested JSON helper recipe](../../docs/spec/std-library.md#nested-json-helpers).
The earlier failed attempt above records the historical diagnostic unchanged.

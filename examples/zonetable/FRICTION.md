# Observed authoring friction

Scope: this application's JSON input/typed rows, opaque TZif zones, gap/fold
policies, formatting, bounded reads, capability acquisition, source check,
reachable TIR export, checked-heap model execution and H/LLVM/Cranelift workflows.
No compiler, language-rule, std or shared-runner change is included. Observations
below are bounded to these scenarios; they do not establish universal ergonomic
coverage. Times are approximate investigation effort, not performance results.

The first source and manifest were saved before correction. The diagnostic runs
used the CLI/image built from frozen main
`e4f48f7a58ba6df14ef25b6b2079784770d8c6c2`. Raw attempts, JSON diagnostics,
source/manifest/toolchain hashes and setup failures are retained externally in
the lane evidence, not checked into the repository.

| Attempt / minimal source | Actual observation | Repair and disposition | Effort |
| --- | --- | --- | --- |
| `let result = model.convert(...)` | LANG-14, `main.hir:27:9`, bytes 1466..1472: “`result` is reserved for the returned value in a postcondition; choose another name”; suggested `value_2`. | Rename the local and its use to `conversion`. Intentional reserved-name rule, no compiler issue. | ~1 min |
| `pub type Row { local: String, ... }` | LANG-14, `model.hir:9:3`, bytes 638..643: “`local` is reserved for a type qualifier; choose another name”; suggested `value_2`. | Use `source_input` for record fields and the parser local; retain the external JSON key `local`. This adds explicit key/field translation. Intentional reserved-name rule. | ~1 min |
| `time_format.FormatError`, `time_format.broken_down(...)`, `time_format.format(...)` | LANG-4, e.g. `model.hir:6:181`, bytes 335..358: unresolved `time_format.FormatError`; `model.hir:83:19`, bytes 3498..3521: unresolved `time_format.broken_down`. The complete invalid error signature also caused dependent variant errors and an initial PROG-2 at `main.hir:4:45`, bytes 360..364. | The physical `time_format.hir` source is registered as module `time` in std's manifest. Use `time.FormatError`, `time.broken_down` and `time.format`. The manifest and std-library spec document this; filename-to-module assumption was wrong. Intentional module registration, no std gap or diagnostic repair claimed. | ~3 min |
| Manual native build without `LLVM_SYS_231_PREFIX` | CLI exit 2: `native link: LLVM_SYS_231_PREFIX must name an absolute toolchain input`. No source diagnostic/span. | Set the absolute pinned LLVM directory; do not infer it from PATH. Intentional build requirement. Original failed setup retained. | ~1 min |
| Manual native build with LLVM but without prebuilt runtime/link-kit inputs | CLI exit 2: `native link: runtime cache: No such file or directory (os error 2)`. No source diagnostic/span. | Run the focused Buck2 CLI harness, whose launcher supplies the declared LLVM, runtime archives and link kit. README links the explicit manual setup. Setup issue; no cache/link implementation changes. The failed build is not a passed engine cell. | ~3 min |

Final-code adaptations made proactively, including those that produced no failed
diagnostic:

- Empty collections, `Option[String]`, policy values and the heterogeneous
  diagnostic tuple carry annotations. `[]`/`None()` and unsuffixed numbers need
  context under LANG-2; `0u64` and `2u8` supply it elsewhere. These were used on
  the first applicable attempt, so no failing span was observed. Intentional
  prefix typing; ~2 min.
- `model.convert` borrows its input row and returns three detached strings using
  `unique item.source_input`, `unique item.source`, `unique item.destination`.
  The output table outlives that borrow. `pattern = unique value` likewise
  detaches an argument snapshot. These copies remain; no failed witness was
  observed and no implicit ownership relaxation is claimed. Intentional modes;
  ~1 min.
- `ConversionError` wraps parse errors separately for local/source/destination,
  then `InputError.Row(index, cause)` adds row context with an explicit match.
  Explicit `? Local`, `? SourceTzif`, `? DestinationTzif`, `? Resolve` and
  `? OutputFormat` select roles; one-payload `Range`, `Format`, `Json` and `Read`
  use ordinary one-level injection. A two-argument row wrapper cannot itself be
  a unary `?` mapper. No failing form was needed. Intentional LANG-8 recipe;
  ~3 min.
- `parse_detail` and `format_detail` translate typed nested causes to stable
  report fields; `append` writes string bytes into a JSON builder. Manual
  encoding retains the public JSON key `local` and stable field order. These
  helpers remain; no speculative shared helper or derive hook was added.
  Application reporting policy, no failed span; ~3 min.
- `validate_format` calls the shipped formatter on an explicitly constructed
  epoch/offset value so even an empty table rejects an invalid pattern. The
  epoch's impossible range error is handled explicitly. There is no separate
  pattern-validation API consumed here. Small application recipe, not evidence
  of a blocking std gap; ~1 min.
- A first Reject resolution classifies `single`/`gap`/`fold`; a second call
  applies the requested policy. Returning the selected source civil time makes
  the intentional gap shift visible. This uses opaque zone operations without
  inspecting their representation. Application policy, no failed span; ~2 min.
- `main` obtains `io.stdout` and `io.dir("input")` explicitly, opens each
  basename using the granted Dir and calls `file.read_all_bounded`; native
  launches take `--hirsel-grants`. Missing stdout exits 2 silently. File causes
  remain typed internally and are intentionally summarized in the report.
  Capability plumbing remains; no write/process/clock/socket grants were added.
  Intentional authority recipe, ~2 min.
- Test orchestration copies the sources and fixture bytes into a private package
  via literal Rust `include_str!`/`include_bytes!`, rewrites only the std manifest
  location, and supplies a copied `tools/capped`. Pure model checks use the
  existing checked-heap runner without editing it. This is required remote-input
  plumbing, not a new runtime facility; ~3 min.

No diagnostic repair or language-rule question was needed. Upstream deviations
and fixture provenance are listed in [README.md](README.md).

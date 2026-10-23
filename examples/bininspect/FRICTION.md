# bininspect authoring friction

Frozen launch main: `726c686c94443495a489c5bcbdeaa7030dd0c406`.
The initial attempt and JSON diagnostics were saved outside the repository before
repair. No compiler or std changes are part of this application.

| Attempt / minimal source | Observed diagnostic / span | Repair retained | Investigation | Disposition |
| --- | --- | --- | --- | --- |
| `let mut result = Report(error: "invalid_input", kind: "", offset: None())` | LANG-14: "`result` is reserved for the returned value in a postcondition; choose another name", `model.hir:86:11`, bytes 2202–2208 in the first attempt | Rename the binding and all references to `report_value` | ~1 minute | Intentional reserved-word rule; no diagnostic repair |
| Tail `match selected { Cbor() => cbor.encode(value, deterministic)? Wire, Msgpack() => msgpack.encode(value)? Wire }` in a Result-returning function | Corrected while reading LANG-8 before another check; no diagnostic was observed for this form | Wrap the match in `Ok(...)`: propagation produces the payload, not a Result | ~1 minute | Intentional explicit Result construction; not claimed as a reproduced compiler error |

Final-code accommodations, including forms chosen from documented recipes without
a failed attempt:

- `Format = Cbor | Msgpack` makes dispatch explicit. Typed `Wire(binary.DecodeError)`
  and `Json(json.JsonError)` wrappers plus `? Wire` / `? Json` preserve codec errors.
  These are application error roles, not a new shared helper library.
- `Report` and `report_to_json` add a typed output record and one compile-time
  derived converter; explicit matches preserve every error kind and position.
  `Option[u64]` distinguishes absent offsets from offset zero. `unique kind` copies
  a borrowed payload into the returned global record. Approximately 5 minutes;
  intentional ownership/output-conversion recipe, no rejecting diagnostic.
- `read_local` repeats the existing bounded bufio recipe because there is no
  application-level file reader here: own the File and Reader, request at most
  4096 bytes or remaining+1, reject the extra byte before appending, then convert
  `List[u8]` into `unique Bytes`. `unique bytes.from_list(take data)` follows the
  nearest example and makes the owned handoff explicit. Approximately 3 minutes;
  capability/ownership plumbing, no demonstrated std gap.
- Empty builders and counters use explicit `List[u8]` / `u64` annotations.
  Numeric entry statuses use `u8` suffixes; format constructors receive their
  expected enum type from signatures/comparisons. These are intentional prefix
  typing rules, no additional inference repair observed.
- JSON input requires `take data: unique Bytes`, a UTF-8 conversion, then a taken
  String for `json.parse`; decode only borrows Bytes. No input copy is needed
  between the I/O reader and model encode. Approximately 2 minutes; documented
  consuming conversion recipe.
- Error reporting obtains stderr explicitly and handles each output Result before
  returning `process.exit_code(2u8)`. Missing stdout/stderr cannot always produce a
  report. This is required capability/exit plumbing, no engine workaround.
- The 64-item check runs after the byte-bounded DOM decode. Std exposes a whole
  value API, so the initial DOM is bounded by the 65,536-byte input rather than
  the smaller presentation policy. The README states this distinction. No second
  binary parser or speculative streaming API was added. Approximately 3 minutes;
  intentional application policy within shipped APIs.

The enumerated forms do not establish universal ergonomic coverage. Engine
failures or harness corrections found during acceptance are recorded below.

- Initial standalone native setup lacked `LLVM_SYS_231_PREFIX`; actual CLI error:
  `hirsel: native link: LLVM_SYS_231_PREFIX must name an absolute toolchain input`
  (exit 2, no source span). Check and reachable TIR export had succeeded, but no
  engine fixture ran in that attempt. Repair: run the focused Buck2 harness,
  whose existing launcher supplies pinned LLVM/link-kit/runtime inputs. Original
  setup logs retained externally; no app/compiler change. ~2 minutes; documented
  native toolchain setup requirement.

- The first focused engine matrix reached the interpreter `missing-stdout`
  control with `{"io":{"stderr":"stderr.bytes@1"},"args":[]}`. The launcher
  rejected that test grant before entry, status 3, with
  `{"kind":"malformed","detail":"io.dirs: missing field `dirs` at line 1 column 35"}`.
  Repair: retain `"dirs":[]` when removing stdout. The expected application status
  remains 2 with empty output. This is a harness-schema repair (~2 minutes), not
  an engine failure; the original cell is retained in the first focused log and
  its source copy. Checked H passed in that run; LLVM/Cranelift builds passed,
  but their runtime cells had not run and were not marked passed.
- Before the final run, added exact 64-entry maps and 31/32/255/256-byte strings
  to cover collection-policy and string-header boundaries in both formats.

- A second full focused run had already started when the live lane instruction
  required resuming only failed/unexecuted cells. Its exact Buck client was
  canceled (exit 141); it provides no additional acceptance claim. The accepted
  checked-H result remains the first run's 7.185-second case. The repaired
  grant and new boundaries run in an external, explicitly narrowed copy of the
  unchanged verifier. The first failure's remote TempDir was automatically
  removed, so individual successful-prefix receipts were unavailable; the
  retained raw assertion and ordered verifier establish the 82 accepted
  interpreter cells before `missing-stdout`. Native runtime cells were all
  unexecuted. Two interpreter encode calls are repeated only to recreate actual
  output for the previously unexecuted roundtrip checks. Fresh-checkout tests
  retain the complete matrix; no skip or resume path is committed.

Final coverage: 94 cases on interpreter/LLVM/Cranelift plus one deterministic
repeat and two actual-output roundtrips per engine (291 accepted runtime cells).
82 accepted interpreter prefix cells were retained from the first focused test;
the resumed run executed 211 calls, including two encode setup calls needed
because worker-local artifacts had been cleaned. Checked-H passed with zero live
heap bytes. Source check, reachable TIR export, both native builds, exact streams
and statuses, input snapshots, path confinement and temporary input cleanup all
passed in the relevant scope. No compiler/std defect or owner-routed issue was
found. No engine workaround beyond the enumerated application plumbing was
observed; this is bounded coverage of these scenarios.

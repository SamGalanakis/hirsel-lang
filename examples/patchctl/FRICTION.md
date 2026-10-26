# patchctl authoring record

The first complete source attempt checked successfully on frozen main
`f36e8f4b04d616e24f63e1b74fa4ba97bd46f466`. No checker, std, language rule,
runner or ABI change was needed. This records the enumerated forms below,
not universal ergonomic coverage. Raw sources, commands, JSON checks, hashes
and failed setup logs are retained outside the repository in the lane evidence.

| Attempt / minimal form | Observed diagnostic or rule/span | Repair or final recipe | Investigation / disposition |
| --- | --- | --- | --- |
| `let command: model.Command = match arg.text(args, 0)? { "diff" => Diff(), "apply" => Apply(), _ => return Error(InvalidInput(Usage())) }` | Accepted; no diagnostic/span | Keep the expected enum type explicit and use borrowed strings in `model.execute` | ~2 min; intentional LANG-2 prefix context, annotation retained |
| `string.split(unique value, ",")`, `string.split(unique line, " ")` | Accepted; no diagnostic/span | `split` takes an owned input; deep-copy the borrowed parameter/line with `unique` | ~2 min; intentional ownership contract, two copies retained |
| `string.from_bytes(unique start_text)` | Accepted; no diagnostic/span | Byte slices borrow their source; make an owned copy before UTF-8 conversion | ~1 min; intentional ownership contract, copy retained |
| `Error(Rejected(unique patched.rejected))` and `Ok(Report(text: unique patched.text))` | Accepted; no diagnostic/span | Copy record fields into global result payloads; retain std's record while inspecting its rejection list | ~2 min; intentional locality recipe, two copies retained |
| `diff.apply(source, other) ? MalformedPatch` with `fn MalformedPatch(error: diff.PatchError) -> InvalidInput { match error { Malformed(line) => Malformed(line) } }` | Accepted; no diagnostic/span | Named unary mapper preserves std's one-based line in the application's typed channel | ~2 min; intentional LANG-8 wrapper/helper retained |
| `AppError = Grant(GrantError) \| Arguments(ArgError) \| Files(FileError) \| InvalidInput(model.InvalidInput) \| Output(OutputError)` | Accepted; no diagnostic/span | Typed wrappers permit ordinary `?`; bounded-read `Limit` is mapped explicitly to `BytesLimit`, while `File(error)` retains the original error | ~3 min; intentional error-channel plumbing retained |
| `file.read_all_bounded(d, mut f, model.INPUT_BYTES)` | Accepted; no diagnostic/span | Thread both the `Dir` and its branded file through the existing API; no new capability helper | ~1 min; intentional CAP-2/brand plumbing retained |
| `process.exit_code(code)` after explicit stderr rendering | Accepted; no diagnostic/span | Return `Ok(ExitCode)` to distinguish rejected hunks (1) from invalid input (2); stderr is optional on failure | ~2 min; intentional process API recipe retained |
| `bounded(value, limit)` before `diff.split_lines`; count commas/spaces before `string.split` | Accepted; no diagnostic/span | Extra pure admission helpers bound collections and hunk coordinates. Fixed header labels avoid filename/header injection. Empty patches are explicit identity operations | ~4 min; intentional bounded application policy/helpers retained |
| First host native build without `LLVM_SYS_231_PREFIX` | Tooling error: `hirsel: native link: LLVM_SYS_231_PREFIX must name an absolute toolchain input`; no source rule/span | Set the absolute pinned LLVM prefix | ~1 min; setup issue, no engine execution claimed |
| Second host native build with LLVM but without prebuilt runtime inputs | Tooling error: `hirsel: native link: runtime cache: No such file or directory (os error 2)`; no source rule/span | Build `hirsel-rt-archives`, native `kit` and `glibc_floor`; set their documented environment inputs | ~2 min; setup issue, no engine execution claimed |
| Rust checked-H harness: `let manifest = package(...); ... { use hirsel_driver::native::*; load_program(&manifest) }` | Rust E0308 at `patchctl_23.rs:102:54`: expected `&Path`, found native `manifest` function | Rename the enclosing local to `package_manifest` | ~2 min; host-language import shadowing, renamed local retained |
| Missing directory grant: `Files(_) => "file_error"` after `io.dir(rt, path.of("fixtures"))` | Runtime assertion: status 2 / empty stdout / `patchctl: file_error`, expected `patchctl: grant`; no compiler diagnostic/span. Checked H passed; original full matrix failed at this cell | Match `Files(NotGranted()) => "grant"` explicitly. The directory projection returns `FileError`, whereas stdout/stderr projections return `GrantError` | ~3 min; intentional capability API behavior, extra error arm retained; original evidence and distinct minimal source preserved |
| First reduced grant witness used `{}` as launch policy | Launch refusal: `not_granted`, root `Io`, exit 3; no source rule/span | Keep an `io` object with stdout/stderr and empty `dirs` to enter the program, then propagate `io.dir` directly as `FileError` | ~1 min; intentional launch/root boundary, first and corrected witnesses preserved |
| First landing: blank line before `mod patchctl_23;` in `modules.rs` | `modules.rs:56: expected one independent mod declaration`; no language rule/span | Remove the blank line; keep one module declaration per line for the union merge | <1 min; repository hot-file format, no application/test input change |
| Attempted Buck target `//:hirsel-cli-test-cli` | `Unknown target hirsel-cli-test-cli`; zero tests ran | Use the required `//:hirsel-cli-test-cli-shard-1` with `patchctl_23` filter | <1 min; harness setup, not a passing test |

The application deliberately uses shipped Myers, fixed context and std's offset
search. It does not duplicate a diff algorithm. The reference's replace and
identical inputs are retained, but the oracle checks valid application instead
of requiring a unique edit script; fixed labels and three context lines are the
application policy. The stricter single-file envelope and bounded coordinates
are admission policy around std's permissive header skipping.

The integration harness also uses explicit manifest dependency paths in private
temporary packages, `include_str!` for both modules and all fixture inputs, a
Python subprocess oracle, normal native-build setup limits, and sequential
runtime processes with 60 CPU-second / 2 GiB allocation limits. Host evidence
runs additionally put each runtime process under `tools/capped`. These are test
plumbing choices, with no shared-runner changes. Cleanup and unchanged input
bytes are assertions, including after rejection.

Focused acceptance executed two Rust tests: the first executable matrix had one
passing checked-H test and one failing application test at missing-directory
authority. After the reporting repair, both tests passed (61 application cases,
198 runtime observations, plus the pure checked-H audit). The successful
unchanged cells were repeated by focus03 and the already-started external05;
these are disclosed repeats, not additional feature coverage. The original
failure remains a failed run. No open compiler/std issue was inferred from it.
# treepatch-29 authoring record (append to prior patchctl history)

| Attempt | Actual behavior | Contract / recipe / cost | Effort / disposition / owner / witness |
|---|---|---|---|
| `let result = Envelope(...)`, `let before = ...` | LANG-14 reserved postcondition names | Rename to envelope/old_bytes; no allocation | 2m, intended; treepatch-29; seal01/02 and check01/02 logs |
| `Ready(Some(data)) => total += ...` | LANG-1 expected comma/newline | Block around assignment arm | 1m, intended; treepatch-29; check02 |
| `for file in ... { file.name }`, `for child in ... { child.path }`, old `let report = model.execute...` after adding report module | LANG-4 module head wins before dot | Rename record locals item/entry/single; no runtime cost | 3m, intended; treepatch-29; seal03/check03 |
| nested `fields` and source names; `let tree` under function tree; plan fn names and local names | LANG-4 no shadowing | Distinct record_fields/input_text/snapshot/output_paths names | 2m, intended; treepatch-29; check03 |
| `fn problem(...) -> Problem` in `unique Outcome[...]` | MODE-2 result not proved unique | Private helper case fixed by private summaries (b552ff23d839 / f71ba1242e00); current exported problem contract stays declared | 2m; treepatch-29; check03 |
| source scan using `file.read_dir` | Shipped API materializes a sorted page before return; no bounded-page parameter | Check entry count immediately before accepting/descending, bound 8192 admitted entries; provider page allocation cannot be application-bounded. Strict preallocation obligation INCOMPLETE | 5m, tooling/API limitation; treepatch-29 witness plan.scan; no new std API introduced |
| separate Dir grants | Launcher permits physical host aliases; Dir has no identity query | Grant caller must supply physically distinct roots and keep them stable. Empty output checked. Source requires read only. Host alias-proof obligation INCOMPLETE | 3m, explicit capability precondition; treepatch-29; design.md |
| `buck2 build //:hirsel` | Unknown target; no compilation | Inspect generated BUCK; correct name hirsel-cli-bin-hirsel | 1m, setup; cli-build.log |
| seal01/03 initially stored stamped file reads/outputs only | Shepherd identified missing live probes, glob memberships and followed directory symlinks | Full evaluated union added externally and initial retained hashes compared unchanged; late union on build02 is honestly late, no green rerun | 4m, tooling/evidence; seal03-post-full |
| `hirsel-toolchain` bundle build | std_objects seed-object action held app test behind setup for ~22m; owner client interrupted with SIGINT (141) | Build exact CLI harness/std image/runtime archives/native kit only; original setup is superseded, not passed | 3m active, tooling; superseded-setup.json, toolchain.log; shepherd 640347 |
| `files-32` complete app | Both public reference and checked-H adapters return ToolingLimit Steps 1000000 at manifest.find (line41); no semantic observation | Preserve original vector, fixed 1M budget, mark each cell INCOMPLETE. Native vector retained unchanged | 3m, tooling; scripted01/03 logs and seal07/09 |
| Buck --env VAR1=... VAR2=... | Test runner argument parse error unexpected VAR2; zero tests | Repeat --env for each variable; select only missing cells | 1m, setup; scripted02.log |
| Cancellation SimulationConfig only added monotonic projection | App returned status2 / `patchctl: grant`, expected canceled; matches provider contract | Add both MonotonicClock root grant and monotonic_clock projection, as prior batchrun adapter does. No app/runtime repair | 2m, intended test setup; scripted03.log |

The original single-file acceptance history above is retained verbatim. The tree
extension's external evidence lives under `hirsel-briefs/treepatch-29`.
Static checking is not an engine run. A required cell stopped by the fixed 1M
step budget remains INCOMPLETE, even when native execution accepts its fixture.
Checked H has no supported host-tape replay adapter in this lane; that replay
obligation is explicitly unsupported/INCOMPLETE. No incidental runtime adapter
was introduced.

Source scan retains regular-file bytes, empty directory names, and an additional
bounded worklist. `shared.freeze(unique snapshot)` copies the source snapshot
and envelope once; output buffers for untouched files make one independent byte
copy. Changed files make an owned UTF-8 copy and copy std's result field into an
owned byte buffer. These are ordinary ownership costs, not compiler defects.

Original native01 stopped on `llvm-files-32-w1-replay-reference` with status 3,
empty stdout and `hirsel: replay refused: UnconsumedInput(8)`. This is a distinct
unresolved reference-replay-29 witness, not proof of Steps. The first failed
remote test's TempDir auto-dropped its raw tape; the original log and sealed
source remain, and this evidence gap is disclosed. Native02 temporarily accepted
an overly broad boundary/refusal substring; its 12 incomplete aggregate rows do
not establish exact refusal identities. The permanent harness now recognizes
only the exact original identity/status/stdout/stderr tuple and fails every new
refusal. Failed or incomplete scratch is retained. No engine matching rule was
relaxed. Effort ~5m; owner reference-replay-29; external native01/native02 logs.

Production small workflows (success, two stale conflicts, pre-expired deadline)
recorded 18 native launches across LLVM/Cranelift requested widths 1/2/4 and
replayed all tapes through reference/LLVM/Cranelift with their source/grants
removed: 72 passed observations. Metrics files were empty for these short runs;
physical effective-width/control qualification remains INCOMPLETE. No actual
placement is inferred from requested width. Separate production replicas were
used to retain immutable host tapes and capped receipts missing from pool tests;
this repeat is disclosed rather than counted as new functional coverage.

| Additional attempt | Actual behavior | Recipe / cost | Effort, owner, witness |
| --- | --- | --- | --- |
| Task captures a unique path only borrowed inside shared.with | CLO-3 path not proved sendable at task boundary | Worker helper takes `unique String`; inner Shared callback borrows it synchronously; no extra copy | ~3 min, intended, treepatch-29, check04/seal04 |
| Existing pure-model generated `let report` after report module registration | LANG-4 module head wins before dot | Rename generated local `single`; original full compatibility oracle passed, rerun only the failed pure-model identity | ~2 min, intended, compat01/compat02 |
| Initial native03 selection included production-qualified mixed/stale | Shepherd identified repeat before queued consumer began | Withdraw exact owner client; narrow native04 to missing 17 Cranelift workflows; no semantic cell ran in withdrawn attempt | ~1 min, setup, native03-withdrawn.json |
| Complete accepted raw envelope, 524288 bytes | Public reference/checked H return ToolingLimit Steps 1000000 in json whitespace scanning | Keep vector and budget, explicitly INCOMPLETE; one-over returns application bytes_limit | ~2 min, tooling, scripted05 |
| Reference replay of accepted 524288-byte envelopes from each native backend | Exact status3, empty stdout, UnconsumedInput(133) | Retain tapes, argv/grants/tool/source hashes and original failure, route to reference-replay-29; no inferred Steps | ~3 min, open/owned, native-caps and native-caps-next |

Final known reference replay refusals are exact identity/status/stdout/stderr
pairs, for each native producer: files-32 cursor 8, file-bytes-65536 cursor 32,
source-bytes-524288 cursor 172, hunks-256 cursor 7, and envelope-bytes-524288
cursor 133. All are INCOMPLETE under reference-replay-29. Fresh failing host
witnesses bind raw tapes and tools; the earlier pool artifact loss remains in
the history. This is a failure disposition, never a replay pass. Other errors
fail the harness.

The production tape audit compared complete semantic recordings for mixed and
stale workflows across requested widths 1/2/4 on each backend. Only placement
and admitted_at were normalized; task IDs, errors, clocks and event order were
retained. It passed. Changed input/config tapes and corrupted tapes were refused
by the supported reference/LLVM/Cranelift paths tested; changed-program control
also refused. Checked-H host-tape replay remains unsupported.

The legacy broad `patchctl_23` selection also matched two unrelated jsonpatchctl
names: both passed, and were not repeated. Original patchctl end-to-end oracle
passed; its pure-model checked-H test passed after the generated-local rename.
New treepatch tests add two libtest identities (384 to 386 total at the frozen
base). Forty-six independent tree workflow/boundary fixtures are retained;
large public-adapter cells stopped by the fixed budget remain INCOMPLETE, not
replaced by smaller vectors. Exact selection/started/completed receipts, including
passing tests whose per-cell libtest output was captured, are external.

Owner disposition after the first application landing: reference-replay-29
landed `d78c17e17b33ff0e781817072b01e11323120fcd`. It preserves the
`evaluator tooling limit: Steps` outcome instead of masking a stopped evaluator
with replay finalization's UnconsumedInput. Fresh full files32 recordings from
both native backends confirm Steps at the unchanged 1M limit and exact native
replays with providers removed. Diagnostic copies of the other retained boundary
tapes also stop as Steps, but binding transplants are nonauthoritative and are
not acceptance runs. Original failures remain immutable.

The harness recognizes the exact new Steps tuple only for the ten already-owned
identities above, retaining their historical tuples too. All remain INCOMPLETE;
no request matching, event consumption or budget was relaxed. Step-cost work
belongs to interp-step-budget-29. The owner also added the public
`replay_observations_checked` host-tape adapter. Thus the historical checked-H
adapter limitation is now fixed upstream; this lane's prior tapes bind the old
runtime revision and its checked-H host replay cells remain UNRUN/INCOMPLETE.
No unchanged green campaign was repeated because main moved. Public full-app
checked-H SimulationConfig cells and the prior replay receipts remain as stated.

## Bounded directory iterator update

The directory-listing limitation above is resolved by the read_dir iterator cutover: the provider retains fixed128-entry pages and plan.scan collects at most its remaining8192 admitted entries, probing one extra before rejecting. Empty-output admission pulls one fallible entry. Other INCOMPLETE obligations in this authoring record remain separate.

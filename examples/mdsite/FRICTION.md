# First-contact friction

Minutes are approximate elapsed investigation time; overlapping waits are not a benchmark.
Copied diagnostics normalize checkout paths and strip trailing display whitespace.
Both incidents below are historical and fixed; their captured output is unchanged.

| Tried | Historical diagnostic | Current disposition / repair | Minutes lost | Class |
| --- | --- | --- | ---: | --- |
| Expected raw HTML from the spec overview, unlike the API index. | [Exact output](repros/incident-1.txt) | The API correctly documents seven options and safe rendering. Fixed the stale overview wording and asserted safe default output. No Markdown rules changed. | 1 | doc gap |
| Compiled `markdown.to_html` natively. | [Exact output](repros/incident-2.txt) | [Minimal program](repros/markdown-native.hir) and adjacent manifest now pass native LLVM. Fixed by [native importer storage/default handling, 0a32703c](https://github.com/SamGalanakis/hirsel-lang/commit/0a32703ca4b6bc31776a024e01d5b2c10d356383); [import regression](../../crates/hirsel-pipeline-tests/tests/rc/rc_import.rs) and [application registration](../../crates/hirsel-cli/tests/examples.rs) retain coverage. | 24 | bug (fixed) |

The native importer incident includes about 2 minutes of direct investigation plus
22 minutes waiting after the other example work was verified. At that time only
patched-toolchain controls passed the minimal native program, all 3 LLVM tests,
the full site run and LLVM replay. The fix has since landed; later dogfood
verification also ran the unchanged minimal program successfully on main's LLVM
backend. The original panic is not a current blocker.

The application retains borrowed views while traversing paths and rendering text,
`unique raw` for UTF-8 ownership, `take source`/`take index` to transfer owners,
explicit Result arms and `Ok(...)` completion. Safe Markdown rendering and HTML
escaping are intentional output contracts. No native workaround remains.

## sitebuild-29

The new TOML commands use six explicit modules; the original no-argument command
and its three test identities remain. Evidence is external in
`/workspace/code/hirsel-briefs/sitebuild-29/`; that path is a local receipt location,
not a runtime dependency. Minutes below are approximate active effort.

| Attempt / minimal source | Actual diagnostic or behavior | Expected contract and disposition | Workaround / cost | Minutes | Owner / witness |
| --- | --- | --- | --- | ---: | --- |
| `fn result[T,E](...)`, `fn copy(...)`, `jobs as u64` | LANG-14 reserved identifiers; LANG-7 casts excluded | Intended authoring rejections, not defects | `resolve`, `load`, bounded 1..4 value selection; no runtime allocation cost | 3 | sitebuild-29; draft-01, check-02/03, scripted-01 logs |
| `input.source`, shadowed page/title/rows/outcome names | LANG-4 resolves module heads before field access and rejects shadowing | Intended explicit naming contract | Distinct local names; no allocation cost | 3 | sitebuild-29; check-04.log, draft-06 |
| Moving several fields with `take asset.field`; foreign record pattern | OWN-8 consumes whole owner; unqualified foreign Asset pattern unresolved | Intended whole-value ownership and constructor visibility | Destructure inside model owner; no extra copies | 2 | sitebuild-29; check-05/06.log |
| `TaskSet[unique WorkResult[Page,Problem]]` / borrowed nested callback capture | MODE-2/4 nested uniqueness and OWN-1; CONC-2 on early set returns | Intended nested ownership/task obligation contracts | `WorkResult[unique Page,unique Problem]`, owner-taking helper before Shared callback; explicit cancel/join on all early exits; bounded deep copies of input descriptors, configs and render outputs | 6 | sitebuild-29; check-05/06.log; no ownership relaxation |
| `list.sort(mut inputs, ...)` after ordinary mut helper | MODE-2 loses deep uniqueness at mut handback | Fixed by private summaries (b552ff23d839 / f71ba1242e00) | Consume the owned inputs directly before sort; bounded deep copy removed | 1 | sitebuild-29; check-05.log |
| `f"{problem}"` | SEM-3 no automatic custom formatting | Intended formatting contract | Explicit domain error description; no new std API | 1 | sitebuild-29; check-05.log |
| Full three-page/asset build through public reference adapter | Original 1M Steps limit inside std.sha256, no semantic observation | Tooling limit, not resource/domain refusal; INCOMPLETE. Same unchanged budget on checked H | No budget increase, no substitute input. Six current-generation nested/unsafe/CommonMark reference+checked-H cells remain incomplete; empty/plan/domain workflows complete | 2 | interp-step-budget-29 / @agentlang-24; original scripted-02.log and exact per-ID scripted-04.log |
| `file.read_dir(d,p)` before checking child count | Provider materializes direct-child list before app sees its length | Strict before-allocation admission unavailable through current API; OPEN/INCOMPLETE | Bound returned count, visited entries, pending stack and admitted inputs. Unbounded provider listing allocation remains; no new core API added | 2 | @agentlang-24, message639947; catalog.discover |
| `markdown.to_html` before exact byte count | Historical renderer built its String before the 2MiB check (message639947) | Resolved by bounded shared walker: emission/escape admission precedes sink writes; no rendered heap fragments | Input/AST memory remains separately capped. Coordinator reserves manifest/assets/index bytes, renders one page task at a time with the remaining publication budget, and preserves output_bytes errors and pre-publication checks. Payload cap excludes allocator bookkeeping/size-class slack | 1 | markdown-bounded-output-33; render.finish_page/publish.remaining/publish.write |
| Generic scripted wrapper accepted any ToolingLimit and incremented completed; successful libtest logs captured | scripted-03 wrapper EXIT0 lacked per-ID counts | Harness defect, fixed forward; no all-workflow credit from that wrapper | Separate attempted/observed/pass/incomplete counters, visible receipts and explicit demonstrated limit allowlist; unchanged compatibility green retained | 2 | sitebuild-29; harness-dispositions.md, scripted-04.log |
| Native support invoked Python `tools/capped` via bash | Static review found shebang mismatch before first native launch | Setup bug fixed before execution | Invoke python3, keep CPU60/2GiB caps; zero native cells had run | 1 | sitebuild-29; shepherd639940 |
| Full toolchain bundle materialization | std_objects setup still running after ~13 minutes | Overbroad setup, interrupted own client with EXIT141; zero application runs | Materialize only harness/runtime archives; reuse matching CLI and image | 1 | sitebuild-29; tools-materialized.log, tools-focused.log |
| Initial seal tracked source plus BUCK/local config | Evaluated stamp/probe/glob/nested tool symlink union omitted | Evidence limitation; late supplements labeled honestly, not retroactive passes | Full preconsumer source/fixture/generated/config/tool union from full-seal-02 onward; full-seal-02/03 comparison had zero differences | 3 | sitebuild-29; seal-01, full-seal files, boundary-03-comparison.json |

Current qualification is deliberately not an all-engine/replay completion claim.
Host-tape checked-H replay has no public adapter and is unsupported/INCOMPLETE.
Native scripted file-fault cancellation, cancellation cleanup at physical widths,
and effective-width/control handshakes remain incomplete unless a later receipt
explicitly supplies them. Real filesystem recordings compare each tape to its own
run; live metadata timestamps are not silently normalized into cross-width
semantic-event equality. Requested widths 1/2/4 alone prove no actual placement.

Discovery ports Go's sorted-child traversal/error propagation with a bounded
explicit stack, rejects symlinks, then sorts admitted destinations globally.
Markdown rendering and HTML escaping remain in shipped std implementations;
CommonMark fixture vectors are limited to the renderer's documented subset.

The config bytes count toward the 1MiB aggregate admission bound. The legacy
command projects no Tasks capability; only plan/build need it. This preserves
old stdout+Dir grants. Native oracle correction retained at native-01.log:
`# <name>&` has an escaped title but raw tag omission in its Markdown body.
The renderer was correct; the fixed literal was corrected, not the source.
The changed-tape refusal control returned the documented tool refusal exit3;
its first expected-exit1 assertion was a harness error (native-02.log).

Final bound tightening checks the full output-root/destination path, including
index and manifest, for 8 components/256 bytes. The accepted depth fixture uses
seven source-path components and the output-root prefix as its eighth; its
one-over twin is refused before render/publication. Generation-2 native boundary
receipts remain preserved; only these affected depth cells need new binaries.
Native boundary receipt native-bounds-01.log completed82 command checks and9
refusal controls, with10 large reference replays explicitly unrun. Its exact
2MiB output and one-over twin pass LLVM and Cranelift. Atomic cancellation/failure
and jobs1 scripted additions pass6 observations. Larger reference/checked-H
boundary cells retain exact1M Steps dispositions in additional-01.log and
bounds-remaining-01.log. No configured budget was raised.

The full legacy entry is additionally exercised without a Tasks projection,
using its fixed original page/index literals. All failures and unsupported cells
remain external; production tests never turn a tooling refusal into an observed
semantic pass. The full-seal supplements hash actual generated read/probe/glob
sets and source/tool symlinks, but the checked-platform harness artifact was
omitted from early overlays; late-artifact-overlay.json labels that limitation.

The initial Rust negative-control wrapper inherited Buck's ~8GiB memory setting,
although its observed RSS was ~107MiB. This was a harness cap error, not a raised
application allowance. The helper now explicitly fixes2GiB/four wrapper CPUs;
only the nine affected refusal controls were requalified under the required
bound (native-controls-final-02.json). All Python application runs already fixed
2GiB/CPU60/four wrapper CPUs. Original control receipts remain unchanged.

A retained CLI copy initially lacked its libLLVM loader path: the first fixed-cap
control attempt ended127 before source execution (controls-final.log). The
corrected LD_LIBRARY_PATH attempt passed9/9 refusals at2GiB/four wrapper CPUs;
raw originals remain. Final no-Tasks legacy entry passes reference, checked H,
LLVM and Cranelift, plus all supported replay adapters (legacy-entry-final.log).

## Bounded directory iterator update

The directory-listing row above is resolved by the read_dir iterator cutover: provider storage is a fixed128-entry page, and catalog.discover collects at most ITEMS children before rejecting the next. Tree-entry and pending-stack limits remain in force. The renderer output-allocation limitation is unchanged.

## Bounded Markdown output (markdown-bounded-output-33)

The shared fallible renderer and layout byte sink now check output admission
before append/escape expansion. Exact 2MiB success and 2MiB+1 refusal retain the
original vectors and atomic publication policy. Native workflow receipts and
record/replay controls passed on LLVM and Cranelift. Scripted application cells
observed 14 passes of 18 attempts; unsafe-Markdown and CommonMark cells on
reference/checked H still stop at the existing 1M Steps cap in SHA-256. Both
original publication-boundary cases on both interpreters stop there as well
(0 observed of 4); these are INCOMPLETE, not output-limit successes.

An initial renderer draft caused the existing Markdown safety aggregate to
exceed 1M steps. The baseline passed, and checking complete literal spans once
restored its unchanged cap without changing HTML, sink-prefix accounting or
input vectors. Initial aggregate bounded fixtures also exceeded 1M; every full
parsed vector now runs as an independent Rust partition at the unchanged cap.
The complete AST aggregate passes reference, checked H, LLVM and Cranelift.
Historical failures remain in the lane receipts; no language rule was weakened.

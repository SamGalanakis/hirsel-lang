# pnmtiles authoring and qualification

Evidence, failed drafts, original logs, fixed vectors and tool/source seals are
external under `/workspace/code/hirsel-briefs/pnm-tiles-30/`. The initial worktree
base is `d49af62dc69a2237e2ebd35550638f68cc3abb46`. No compiler/runtime/std or
unrelated application change is made by this lane.

## Intended authoring rules

About 6 active minutes; owner application; disposition intended/fixed in source.
Attempt01 declared `type Raster { shape: image.Shape, ... }`; check rejected
LANG-14, “`shape` is reserved for a type shape expression.” Rename the field and
locals `layout`; no allocation cost. Attempt02 wrote qualified variant patterns
`report.Invert() => ...`; the parser reported LANG-7 “expected `=>`”. Use
context-resolved `Invert()` patterns/constructors. Attempt03 used `view`, `layout`,
`interruptible`, `name`, and `jobs` for both module functions and local bindings;
LANG-4 forbids those shadows and reported the conflicting declarations. Renamed
functions to `borrow_view`, `checked_layout`, `run_interruptible`, `problem_name`
and `parse_jobs`. Failed complete sources and diagnostics are in `attempt-01/`
through `attempt-03/`; no language defect is inferred from intended rejections.

Attempt04 returned `Result[Raster, Problem]` with a `unique Bytes` field, then
moved its destructured pixels into `tiles.run` requiring unique input. MODE-2
reported that the destructured `pixels` was not proved unique. The complete
producer contract is `Result[unique Raster, Problem]` and construction explicitly
uses `unique Raster(...)`; attempt05 source-checks. About 2 active minutes,
private helper case fixed by private summaries (b552ff23d839 / f71ba1242e00); the current cross-module read contract stays declared. The constructor already owns all
counted input; this boundary requires no additional deep byte copy. Tile outputs
have payload-qualified `Result[unique Bytes, Problem]` contracts at both producer
and TaskSet. Shared freezing moves the input buffer; assembly appends every output
byte once. There is one output raster buffer plus at most four unique tile buffers
(each ≤12288 bytes); no halo copy. These are static source costs, not measured
allocation counts. No ownership rule is relaxed.

## Setup and evidence

Initial prepare01 launched a shell background without retaining its producer;
the shell ended and no EXIT marker appeared. This is setup failure, not successful
preparation. Prepare02 retained the shell and finished exit0. Build01 built the
matching CLI/std image from the owned cwd. Source checks each have before/after
seals and preserved original diagnostics. Formatting expanded625 handwritten
lines to940; there is no padding or generated application code.

Test01 ran on the pool; its ephemeral scratch is unavailable, but per-identity
receipts and original diagnostics remain in its retained log and extracted JSON.
Later runs explicitly select host execution and an external evidence directory.
The first sealer recorded generated stamp reads/probes/globs/outputs but omitted
direct hashes for some materialized native tools and misspelled three generated
output paths. Test02's inventory grew17853→18145. The292 exact paths are classified
in `test-02-seal-delta.json`: newly enumerated native dependency/tool artifacts
and the three generated inventories. No byte invariance is asserted where the
before inventory has no direct hash. Reference/checked cap execution did not use
native linker/runtime archives. Tool paths first directly hashed there are late
coverage, not generated data. Native consumers additionally seal their actual
compiler, std image, runtime archives, linker kit, tools, sources and fixtures
before execution. Prospective seal/retention improvements do not rerun prior greens.

## Original semantic outcomes

Test01 selected one Rust test and executed136 full-app cells:132 completed/pass
(66 cases×reference/checked H), and4 header boundary Steps cells.
Header-cap and header-over each consumed their unchanged1M steps without an
observation. Test02 selected one test: cap-info completed/pass twice (15348 steps
each), cap-invert consumed its predeclared16M steps twice without an observation.
Both original Rust tests failed, and their exits remain retained. Steps is a
tooling disposition, never a language failure or semantic pass. Required boundary
cells remain INCOMPLETE; owner application/tooling budget policy. Their source,
argv and unchanged byte vectors are the minimal witnesses in support/cases.json.

Test03 built both native backends, recorded138 workflows and completed276
cross-backend native replays (all exact stdout, stderr, exit, output bytes and
unchanged inputs). Original mounts were renamed unavailable and grants removed.
The host replay partition then panicked on its first completed replay by assuming
`Observable.final_state` is populated: public host replay deliberately returns
none. This is a harness error (about3 minutes), not a language defect. Subsequent
selection reuses the immutable recordings; no native recording/replay green is
repeated to correct it. Host replay validates all recorded requests and exhaustion;
stderr is asserted against the exact tape writes, whose byte payloads replay must
match, and reference/checked observations are compared. Test04 was setup-only:
Buck's test executor requires a separate `--env` for each variable; the original
unexpected-argument exit1 remains retained. No application ran in that attempt.

## Replay tooling diagnostic gap

At base main, `replay_observations`/`replay_observations_checked` discard the
returned host RunOutcome and report only `Runtime("replay produced no semantic
observation")` on tooling stop (`hirsel-driver/src/lib.rs:684–715`). This cannot
prove Steps rather than another tooling outcome. Routed with original full-app
witnesses to agentlang-24 (message643137); no runtime adapter or parallel repair.
Known Steps reference replays use the public ordinary host launcher to assert
exact `ToolingLimit {kind:"Steps", limit:original_bound}`. Checked-host exact
classification remains INCOMPLETE until an outcome-preserving public seam exists;
no broad `is_err()` assertion is counted as a Steps pass. No original failing
vector is shrunk or given a larger bound.

## Completed replay and production qualification

Test05 reused the138 immutable native tapes and passed264 complete host replay
observations (132 on each reference/checked H). Six ordinary host replays retain
exact Steps outcomes and six checked boundary replays remain unrun because the
public helper loses the outcome. The passing test's libtest output suppresses
per-cell lines; `test-05-derived-receipts.json` explicitly labels its per-identity
receipts as derived afterwards from the frozen loop, tapes and exit0, not
contemporaneously streamed evidence. Later partitions write per-cell receipts.

Qualification01 passed12 success/domain cells: LLVM/Cranelift × requested1/2/4.
Every one retains a `worker_phase:running`, effectiveN, pending0 control reply
before completion. Full canonical traces, including all task identities,
ordinals, errors and payload bytes, agree exactly across all six success runs
and separately all six domain-error runs (no normalization, sorting or dropped
fields). Six initial cancellation probes exited by SIGINT (-2): inotify observed
the admission snapshot opening input before the app installed its signal handler.
This is a harness handshake mistake, not a runtime defect. About4 active minutes.

Qualification02 retried only those six failed cancellation identities after the
handshake additionally checked the child's armed SIGINT handler bit in
`/proc/<pid>/status`. All six now return exit2 and exact `canceled` stderr,
preserve the existing output, close the input, and remove their control socket;
each retains settled effective1/2/4 and pending0. No sleeps or worker hooks were
used. These prove executor capacity, never simultaneous CPU execution. Each
independently delivered signal has its own immutable tape; differences in the
signal's logical arrival are not normalized away. Original -2 runs remain red.

All20 additional tapes (18 qualification runs and2 provider-boundary witnesses)
passed40 native replays with their original mounts/grants unavailable. Test08
passed28 additional complete reference/checked host replays; its6 header-boundary
reference replays retained exact Steps and6 checked counterparts remain unrun.
`extra-host-replay-receipts.json` retains every identity/status and original bound.
Test06 passed12 refusal cells (changed directory request, extra event and changed
header/config seed × four replay adapters). The original tapes are unchanged.
Test07 passed6 full-app reference/checked fault cells: partial atomic write,
commit failure and input read failure. Each asserts exact stderr/status and the
whole initial tree, proving temporary removal and output preservation.

## Native fixture gap and remaining obligations

The public native Fixture.file_reads supports `test.dir` trees, while production
host mounts intentionally ignore that hook. The exact full-app witness in
`pending-native-witness/` sets `HIRSEL_RT_FILE_READS` to Pending and a scripted
Interrupt on a real mounted1×1 P5 file. Both production backends read/filter it
successfully rather than pend/cancel. This current witness demonstrates the
boundary; it is not a passed Pending cell. The actual full app's reference and
checked-H Pending/cancel cells passed in Test01. Native full-entry scripted Pending
and injected file-fault cells, their tapes and required all-engine replays remain
INCOMPLETE, as do the retained boundary Steps cells. The helper/outcome and
native-provider gaps are owned by native-harness-gaps-30 (routed message643419);
no alternate adapter or compiler repair was added. Useful application landing
does not claim all-engine/replay completion.

The exact input and output vectors were compacted from2.4MiB to87KiB using a
prefix+constant-repeat fixture recipe for the two maximum rasters. Static expanded
JSON equality is retained in `compact-fixture-equivalence.txt`; no byte, bound,
workflow or expected result changed and no green cell was repeated for this.
The oracle itself and all raw evidence stay external. No performance campaign,
optimizer change, std duplication or language extension was attempted.

## Final validation accounting

The focused module adds4 named tests; the CLI harness lists436 pre-existing
identities and440 after this change. This is discovery, not440 executed tests.
Executed app observations are140 reference/checked scripted completions,
138 native matrix completions,18 production qualification completions,2 native
provider-boundary witness completions,316 native replay completions, and292
reference/checked host replay completions, plus12 refusal controls. The original
six reference Steps cells,24 replay-boundary incomplete dispositions, native
Pending and native fault-injection obligations remain separate. A Rust harness
pass that asserts an expected tooling stop never makes that cell a semantic pass.
The four original boundary vectors and native witness files remain unchanged.

Raw launch grant files were removed at their original argv paths for host-free
replay. Their canonical validated grants remain in each immutable tape. Raw JSON
was later reconstructed under `.archived.json` names from the frozen producer;
all18 qualification reconstructions match their original pre-consumer hashes.
The138 matrix JSON reconstructions have late hashes only and are labelled as
such in `archived-grants.json`; no pre-execution raw-JSON hash is invented. The
bound tape and frozen producer retain their exact semantic grant provenance.

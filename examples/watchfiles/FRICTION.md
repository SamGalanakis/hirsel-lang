# watchfiles findings

No runtime, compiler or std changes are included. Immutable failed drafts and
receipts are external under `hirsel-briefs/watchfiles-27/`.

- `attempt-01`: `string.split(copy name, "/")` on borrowed `name: String`
  failed MODE-1 (“requires a global value”). `unique name` copies at most 1024
  bytes during validation. Intended ownership rule; about two minutes.
- `attempt-01`: `model.Present(...)`, `model.Missing()` and the other qualified
  variant arms failed LANG-4 (“unresolved name”). Contextual bare constructors
  select `model.Snapshot`. Intended naming rule; about two minutes, no copies.
- `attempt-01/02`: capturing mutable `tick` failed CLO-3 (“is mutable”);
  capturing a provider borrowed through scoped callbacks failed CLO-3 (“not
  proved sendable”). The loop now snapshots the scalar tick index and takes
  the sendable provider into each scope. Intended crossing rules; about five
  minutes. Each worker receives an explicitly owned callable and path.
- `attempt-02`: a report closure parameter `index` shadowed the enclosing
  argument cursor, LANG-4 (“a local cannot shadow another local”). Renamed
  `input_index`. Intended rule; one minute, no runtime cost.
- `attempt-03`: repeated `unique fetch_inner` was moved at its syntactic last
  use, then failed OWN-2 on a loop backedge (“not initialised on every path”).
  Explicit `unique copy fetch_inner` (and the per-tick provider copy) preserves
  the loop's source callable. Intended explicit-copy plumbing; two minutes.
  Cost: bounded callable-environment copies, once per tick and per file read.
- `script-check-01`: a mutex observer written `fn(state: unique Stats)` failed
  LANG-7 (“expected fn(mut T0) -> T1”). `mutex.with` requires a mutable callback
  even for an observation; `fn(mut state: unique Stats)` returns a small unique
  copy. Intended API contract; one minute.

Application plumbing: bounded complete reads own their returned bytes; the
comparison baseline moves into the next tick without copying contents.
Reporting callbacks stay scoped; read callbacks are explicitly sendable. A
nested tick scope intentionally turns an outer deadline into `Parent` for its
workers; the idle timer in the deadline scope sees `DeadlineExceeded`. The
private driver returns that cause to fixtures; the CLI prints `deadline`.

Reference deviations and public bounds are recorded in README.md. Scripted
blocked-read fixtures park an unanswered provider response on a channel; the
real file-provider cancellation cell must be accounted separately in the
external report. Source checks alone are never runtime passes.

- First execution preparation: a borrowed-list `for` inside a scoped callback
  passed `check` but failed TIR validation `InvalidOrigin` before execution.
  Minimal source and a control are external in `origin-witness/`; indexed
  baseline initialization avoids the failing item projection. Reported to
  @agentlang-24 for the existing origin-validation owner (message 636703).
  Open compiler defect; about ten minutes; no added content copies. Both
  initial Rust tests failed at preparation, so no workflow was a runtime pass.

- Second preparation/execution attempt: native launch required all three seed
  fields in an explicit `inputs` record; supplying only `test_seed` was refused
  as malformed before entry. The harness now supplies zero hash/random seeds.
  Intended configuration schema; one minute.
- The scripted interpreter then failed `release of a dead Mutex root`. A
  minimized `invoke(take f: sendable fn() -> ()) { let g = unique copy f;
  g() }` with a captured Mutex makes a later mutex access fail `expired mutex`.
  External `dead-mutex/main.hir`; routed as message 636764 to @agentlang-24.
  The application now accepts a scoped factory that constructs each owned
  task closure afresh, eliminating copied provider environments. This
  supersedes the earlier copy workaround; per-operation environments and
  unique path copies remain bounded. Open interpreter issue; ten minutes.
- Factory drafts `factory-01/02` returned a closure under `unique fn` and
  failed MODE-4/CLO-1/OWN-1: that expression retained a scoped context. An
  explicitly typed `unique sendable once fn()` local supplies the escaping
  context, then `take job` returns it. Intended closure-context rules; three
  minutes. No extra payload copy.

The scoped-loop defect is owned by `scope-closure-origin-27`, assigned by
@agentlang-24 in reply to message 636703. Indexed initialization remains until
the owner lands its producer repair. No validation rule is weakened here.

- Factory return experiments are retained externally. Returning a qualified
  callable directly from a scoped lambda reached a TIR `TypeMismatch` for
  `closure result` (`factory-03/05`); no shared producer repair is included.
  An application-private `ReadJob` record makes the return envelope explicit.
  A callback returning a Task branded in its own scope parameter was rejected
  PROV-2/MODE-6/CONC-2 (`factory-04`), an intended restriction; that design
  was abandoned. `factory-06` then rejected `take reader.call` with OWN-8;
  whole-value `let ReadJob(call: job) = take reader` is the documented repair.
  About ten minutes total; one small record per observation, no content copies.
  The fixture uses a named constructor taking a copied Mutex handle so its
  escaping callable owns the handle; borrowing it through a scoped factory
  had correctly failed CLO-4 (`script-check-03`).

The final `ReadJob` is an owned record with a sendable once callable, not a
unique record: a Mutex root may be shared safely. Requiring unique on a job
capturing that root caused MODE-2/OWN-5 (`script-check-06/07`); removing that
unneeded promise keeps sendability and avoids copying a noncopyable once
function. This supersedes the experimental unique job return above. The
copied-closure defect is owned by `closure-copy-resource-27` per @agentlang-24.

- First real executable launch supplied arguments only in the grants template.
  Standalone launch snapshots its trailing argv and replaces template args,
  even when empty; the tool therefore returned Invalid. Supplying explicit
  trailing arguments fixes the invocation. `hirsel run` still honors template
  args. Intended launcher interface; five minutes. The scripted native cells
  completed before this real-file setup failure; their green prefix is kept.

- The live qualification fixture originally mixed buffered `readline` with
  `communicate`, losing buffered stdout rows on two width-one I/O captures.
  Fourteen apparent replay output mismatches were retained. An unbuffered
  fixture pipe repaired the capture; only those two recordings and their
  fourteen replays were repeated, and passed. Harness bug; five minutes;
  no application or runtime repair.
- Production recording plus trace export aborts at `io_home.rs:48` on width
  one, or refuses with `incomplete canonical event evidence` at widths two
  and four. Same source and recording without trace export passes. Owned by
  `e1-trace-record-27` (routed with exact argv and retained receipts). No trace
  override or shared repair is included. Traced cells remain incomplete.

Validation: twelve workflows pass through the actual loop on four engines;
72 native scripted recordings preserve full semantic tapes across both
backends and widths 1/2/4, normalizing only admitted_at and placement. The
complete CLI path has separate H/checked-H argument/cap/clock/overflow tests;
real files run on LLVM/Cranelift, and handshake observations on H too.
H/checked-H replay parity for actual host tapes is not claimed: the public
host replay adapter lacks a checked-H selector. The minimal retained
higher-order factory `TypeMismatch` draft has not been isolated beyond the
full application; simple scalar and Bytes factory controls pass, so it is
not reported as a new minimized compiler defect.

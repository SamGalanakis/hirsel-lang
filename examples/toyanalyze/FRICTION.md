# Toy analyzer friction

Base: d1ecb020468b787a29a96abdbab118d237f2262a. Original drafts, minimal
witnesses, source/tool seals and logs live in the external toy-analyzer-31 lane
archive. No compiler, runtime, std or ABI change is included. Effort estimates
below describe active diagnosis, excluding builds and queued qualification.

`Located[T]` represents tokens, expressions and names (Kind/Expr/String). The
indexed expression arena bounds depth without recursive ownership. Scoped
callbacks receive explicit mutable context; frozen snapshots cross task boundaries;
results aggregate in declaration order. Taking a parsed replacement displaces its
old AST only after successful parsing. This is ordinary application plumbing,
about 30 minutes initially, owned by this application.

Attempt 01: `let Ok(mut file) = file.create_atomic(...)` rejected LANG-13:
"nested `mut` is unsupported; put `mut` on a local binding after `let`".
Repair: `let mut Ok(file) = ...`. Intended syntax, under one minute, no copy cost.
Draft01/reference-01 retain the admission failure: one started test, no semantic
observation. Guessing an absent hirsel-cli Buck target was setup failure; the
actual generated target is hirsel-cli-bin-hirsel.

Attempts 02–04: `local`/`before` are reserved (LANG-14); same-module values cannot
be shadowed, and `env` before a dot names the exposed core module (LANG-4).
Renamed application locals/helpers. `problem(mut p, p.tokens[p.at].span)` rejected
OWN-3; capture the plain span before lending p. `take p.tree` rejected OWN-8;
destructure the Parser owner instead. Intended rules; about five minutes, no
additional span copies. Originals: drafts02..04/check-02..05.

Attempt 05: mutating captured output rejected CLO-1. `scoped[S,R]` now receives
mutable context explicitly. Intended borrowing, about three minutes, no context
copy. Task errors must be sendable: ordinary String rejected MODE-4. The first
repair used unique String task/mapper errors, exposing the separate producer bug
below. Final workers return plain u64 error codes and the parent maps them to
ordinary String. This costs a small application-specific error-code mapping.
Originals: drafts05..06/check-06 and export logs.

The original unique String mapper returned via `?` into Result[(),String]
passes source checking but fails reachable TIR export with call-result
TypeMismatch. The nine-line error-mapper.hir and ordinary-result twin isolate it;
the twin exports. Initial loop-name/helper theories were discarded: source `check`
was not full reachable export, and splitting loops alone did not fix the original
application (reference-02 still failed body0/block40). Explicit Result matches,
plain worker codes and ordinary mapper results let the final application export
and execute. About 30 minutes of reduction/repair; no extra AST allocation from
this workaround. Owner: generic-loop-matrix-31, open exact original full-app
qualification obligation. Its candidate compiler repair and minimal acceptance
cannot close that obligation. Original application/config/identity and minimal
controls remain in witnesses/. No second compiler repair is included.

Ownership costs: token_text materializes a unique String from bounded source
bytes. Each dependency wave freezes a copied module-state snapshot for task
callbacks, and summary/diagnostic aggregation copies plain records. These are
explicit bounded application costs, not measured optimizations; no throughput
claim. Names and scope declaration records are copied where their lifetimes
outlast parser views. Cache schema1 is advisory: validate shape/version and
recompute before atomic publication. It provides no cross-launch acceleration;
in-memory session invalidation/reuse is separately exercised. Intended policy,
owned by this application; roughly ten minutes of ownership plumbing.

Qualification plumbing: export-01's replacement missed formatted whitespace;
export-03 left a callback annotation malformed (LANG-7/8). extras-01 was Rust
compilation failure (undefined steps, nonexistent ObservedSimulation.files);
zero tests ran. The repaired harness uses the actual trees map and scoped bound.
These are application/setup errors, not language defects. About five minutes;
original logs are retained. Early source seals01..05 were partial; later seals
import generated.BUILD_INPUTS and evaluate stamp reads/probes/globs/outputs.
Runtime tools are hashed before consumers. Host-audit01's fuller tool seal was
in-flight and its later exit seal includes subsequent support edits; neither is
presented as an original prelaunch seal. No green rerun repaired provenance.

Generation1 (1407 application lines) qualified the Python scope/type oracle,
reference/checked sessions and cap pairs, then native workflows/replay. all-01
started four tests: three passed, production width4 failed with effective2 on a
constrained focus worker. LLVM widths1/2 had passed. Capacity was legitimate;
ten missing success/domain cells and six cancellation cells passed on the host
with unchanged four-CPU wrapper, 60 CPU-second/2GiB limits. Settled running,
effective-N/pending-zero control receipts prove admitted capacity only. The host
audit passed over16 local tapes. extras-02 passed34 recordings/136 replays and12
new reference/checked cache/edit observations. Original all-01 remote raw tapes
were lost with disposable worker tmpfs; assertions/logs survive. Later tapes are
archived. This retention gap remains open; no unchanged rerecord was performed.

Generation2 (1456 lines) adds optional manifest readiness under an explicit
writable output Dir. It announces first-batch admission, never semantic success.
Seven affected readiness workflows passed reference/checked and14 native
recordings/56 replays, including actual Blocked/AfterWrite release jobs1/2/4,
default, denied write with pending cancellation and invalid path/type. Five
additional workflows passed10 native recordings/40 replays: incremental edits,
denied input, parser recovery, unknown import and128 diagnostics. Eighteen live
success/domain/cancel cells and six separately named default-cancel companions
passed both native backends at widths1/2/4 with capacity receipts. Old qualified
source/tapes remain generation1; they are not relabeled as final-source coverage.

Final scripted width qualification passed12 recordings/96 replay executions for
AfterWrite release and denied-write pending cancellation at requested1/2/4 on
both backends. Streams match exactly after only placement/admission normalization;
this fixture makes no physical capacity claim. Exact reference/checked negative
controls passed: changed source -> program binding; changed config -> header/run
record disagreement; extra event -> UnconsumedInput; altered event -> ReplayMismatch.
Generic is_err assertions alone are not credited as these proofs.

Remaining obligations are explicitly INCOMPLETE. Original generation2
ready-plus8192-byte cancellation tape cranelift-cancel-w1 stops at Steps1M in
SHA256, with no observation; the bound was not raised. Default cancellation is a
separate companion, not a substitute. host-audit-final completed cranelift-domain-w1
and the reference/checked prefix for domain-w2, then cross-width comparison failed.
The structured control identifies event16: identical Metadata(a.toy) request,
size/kind/permissions, different modified_unix_ns from independently rewritten
live files. Metadata is not normalized. Each live tape must be judged against
its own recording; remaining per-live offline adapters/control suffixes are
unrun. Deterministic scripted stream equality above is a separate completed proof.
The original mapper/full-app witness and missing all-01 raw archive also remain
open. Detailed cell dispositions live in the external report, never an all-green
engine/replay claim. These are tooling/owner/retention limitations, not repaired
runtime defects. No performance campaign was undertaken.

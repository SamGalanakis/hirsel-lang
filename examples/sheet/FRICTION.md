# sheet-eval-31 friction

Additive stdin-line-bound-35 resolution: `std.input.read_line_bounded` now owns
the byte-at-a-time admission loop. Sheet uses max512, strips one returned LF,
keeps CR, and preserves line_limit/input/utf8 mapping and the original small
session and 512/513-byte command oracles. The earlier private-loop adaptation
below remains historical. Dense boundaries and historical replay/Steps
dispositions are unchanged and are not requalified by this reader migration.

Starting main: ff3886a3. Application and focused fixture implementation complete. No compiler/runtime/std
change. Exact bounded/native/replay dispositions below remain separate.

Intentional application adaptations (approximately 10 minutes): use bounded core
stdin.read instead of unbounded std input.read_line; ASCII grammar keeps accepted spellings byte-sized; Unicode length failures are
counted by scalar, separately from syntax; cache snapshots use Shared and copied edited AST nodes
for task ownership; Kahn's blocked remainder includes downstream cells, represented
as #CYCLE. These are explicit costs/policies, not language defects.

Failed drafts and exact run receipts belong externally under hirsel-briefs/sheet-eval-31.

First source check (test-01.log; 0 semantic cells started): module-qualified enum
constructors are invalid; use contextual constructors selected by the declared
Result/Map payload. Local names cannot shadow top-level module values: rename
formula.name to coordinate, graph dependencies parameter to prerequisites and
evaluate's lookup to read_cell. Intentional rules, approximately 3 minutes.
Borrowed source slices cannot cross a take/global boundary: spelling copies at
most 256 bytes with unique; command takes its owned line; path splitting copies
at most 128 bytes. Args-based settings explicitly uses Args authority. The
interruptible task takes a uniquely owned Settings result. MODE-1/CLO-3/CAP-1
rejections were intended; no compiler repair. Draft 01 retained externally.

Source-accepted / validator-rejected callback (open, capture-contract-matrix-31):
`match parsed { Ok(tree) => shared.with(prior, fn(values) { expression(tree,
fn(index) { read_cell(values,index) }) }), ... }` rejects reachable export with
`TypeMismatch { entity: "closure capture" }`, job Block1 instruction4. Taking
parsed instead keeps the failure at instruction7. Source check passes; a 19-line
standalone witness reproduces on unchanged V. Exact minimal/full source, log,
tool hashes and original test02 retained in external witness-capture/ and seals.
App-local equivalent moves the match inside shared.with; exports successfully.
No additional data copy; approximately 8 minutes. Original obligation remains
open, independently of the passing twin. No compiler changes or weakened V.

Seal timing: test01/test02 had subset prelaunch seals; evaluated generator
read/probe/glob memberships and actual materialized tool/image/archive union
were captured after test02. Later consumers use the complete union prelaunch.
No semantic green was repeated for sealing alone.

Dense boundary disposition: exact full-entry loads with 4096 and 4097 edges
both exhaust their predeclared 16,000,000 TIR steps in reference and checked H
(graph.rewire, graph.hir:44–45), with consumed=16,000,000 and no observation.
Original log test-10-extra.log is retained unchanged; these four cells remain
INCOMPLETE. No budget increase or algorithm optimization is assigned. A later, separately
named reduced twin was authorized; it does not replace these identities. The permanent dense semantic test is explicitly ignored with this
reason; native boundary cells stay active and their host replays retain Steps.
All other additional reference/checked cells passed. An earlier harness build
used the wrong Rust Fault import (test07); it ran zero tests and is setup failure.
The corrected import is library::simulation::Fault, not a language workaround.

Application-bound corrections (intended application policy, ~5 minutes): AST
height is now checked before allocating the parent Binary/Negate node. The
original parser computed height inside node after construction; cap/one-over
fixtures still expect identical values/errors. Unicode formula lengths count
scalars when the byte length exceeds256; a 256-character invalid Unicode formula
is #SYNTAX, while257 characters refuse admission. Accepted grammar stays ASCII.
These are application fixes, no shared language/std changes.

Harness plumbing (~8 minutes): std JSON parse takes String; initial caller used
Bytes. The public Rust fault type lives in library::simulation::Fault. The native
launcher clears its child environment, so a helper invoked as python3 may see
empty sys.executable. Embed capped/launch.py into a retained private fixture
root, use /usr/bin/python3 and retain the explicit fallback; scope reports go to
separate files so app streams remain exact. Original test07/11/12 setup failures
are retained. A test14 reader was stopped on its exact PID after recording its
completed identities, to avoid repeated already-stopped dense host replay cells.
A queued test18 was canceled before execution to set test threads1. Neither
wrapper exit nor canceled test counts as a semantic pass. The supplemental
cancellation draft used reserved identifier local; LANG-14 is intended; rename
worker_book. Failed draft retained externally, ~1 minute.

Dense-twin resolution: a distinct32-cell/512-edge full-entry load/show/clear
workflow passes reference and checked H at2,411,585 steps each under its frozen
16M bound. Original4096/4097 reference/checked cells remain INCOMPLETE at16M.
Both original boundaries pass natively; their stopped/unrun host replay rows
are separately INCOMPLETE, never accepted by an expected-Error assertion.

Production replay: success/domain12 tapes replay through reference and checked
H (24 host passes), both native backends replay all18 tapes (36 native passes).
Host-signal cancel6 tapes refuse with interpretation-is-not-implemented in both
host adapters:12 INCOMPLETE cells owned by reference-cancel-replay-31. Every
native cancellation replay passes. This is a tooling refusal, not an application
semantic mismatch. Original source/artifact/tape hashes are retained externally.

Provider release: an explicit supplemental root calls complete app.main in a
child with real Io+Args and all six app modules registered; a scripted signal at
global idle releases a test-host writer. AfterWrite then readies the actual app
batch read. Live reference/checked, LLVM/Cranelift and four-engine replay pass,
including Blocked read and cleanup evidence. It is disclosed as a provider host
wrapper, not a new stock CLI command. Pending cancellation uses stock main.

Ordered:18 deterministic scripted success/domain/cancel recordings across both
backends and requested1/2/4 match byte-for-byte semantic event order after only
placement/admitted_at normalization. Their72 replays pass. Separate18 ordinary
production launches retain an application stdin handshake and control reply
worker_phase=running, effective=requested, pending=0. Capacity is established;
no claim of simultaneous hardware utilization or speedup. Free stays disabled.

Scope: only the six app modules, app docs/fixtures/grants, focused Rust/Python
support and one modules.rs registration change. No compiler/runtime/std patch,
optimization, public API removal or root README edit.

Capture resolution: landed in86ddbca1aa1819b36c737f5bb6a4057ebc24f976
(coverage8e161529). The owner verified both exact small originals through four
engines and LLVM O0, hostile Borrow-to-Take validation guards, and the unchanged
six-module original through source/V export. This resolves the reported source/V
producer defect, not an additional full-app runtime campaign. The app-local
match-inside-callback arrangement remains. No repair is included in this app commit.

Host-cancel owner disposition: the original6tapes×2host-adapter cells are
runtime-revision incompatible with the required forthcoming repair. They remain
INCOMPLETE; no compatibility migration or retrospective green is authorized.
reference-cancel-replay-31 retains portable original CLI/std image/libLLVM/std
sources and the exact tapes externally. A fresh recording after the owner's
repair would be a new identity, never recovery of these originals.

Evidence accounting: test22 repeated the four Unicode256/257 reference/checked
observations from test20; they are repeated receipts, not new coverage. Tests
were partitioned by missing/affected identities; no unchanged broad matrix was
rerun for sealing or movement of main. The base native06 matrix passed40live
and160replay cells remotely; its log is retained, but its40raw sandbox tapes
were not archived locally. Subsequent required representative success, domain,
cancellation, provider and width tapes are retained externally. The native06 raw
archival obligation remains explicitly incomplete, independently of its passed
observations. Complete source/tool/stamp seals are prelaunch from later phases;
test01/02 retain their honestly labelled subset/late captures.

Final focused receipts (separate selections, no single all-green campaign):

| Selection | Disposition |
| --- | --- |
| base20workflows |40reference/checked,40native and160four-engine replay observations passed |
| independent oracle regeneration | passed |
| additional bounds/provider rows | non-dense reference/checked passed; original4dense cells Steps at16M |
| original4096/4097 native boundaries | both backends passed; all native replay cells completed, remaining2in test23 |
| Pending file cancellation and output denial | stock main passed both live simulators, both natives and four-engine replay; parked-read/Close verified |
| AfterWrite release | supplemental full-main provider host passed2live simulators,2native and8replay cells |
| canceled recalculation | actual evaluator preserved dirty/cache state;2live simulators,2native and8replay cells passed |
| AST cap/one-over, Unicode256/257, logical jobs1/2/3 | affected parser/job cells passed reference/checked and native/replay; jobs4 covered by base/width rows |
| reduced512edges |2live simulator passes,2,411,585steps each at16M; original full cells unchanged |
| scripted Ordered widths |18live and72replay cells passed; full semantic tapes equal after documented metadata normalization |
| ordinary production widths |18live capacity-qualified launches;36native replays and24success/domain host replays passed;12originalcancel host rows INCOMPLETE |
| refusal controls | request/config/trailing tape in four engines and changed-source host controls passed |
| real stdout ENOSPC |2native recordings and8four-engine replays passed |
| production REPL/batch transcript |6process observations passed; checked REPL/batch covered by public scripted adapters |

The CLI harness had492tests before this lane and509after:17new identities,
12ordinary tests and5explicit manual/tooling selections. Tests were executed
in focused partitions; retained original failure and ignored identities prevent
an all-green suite claim. No zero-test selection is a pass. Original dense host
replay rows and historical host-signal tapes remain INCOMPLETE even where new
small or scripted cells pass.

Handwritten application source:1207lines added,0removed: main361, formula286,
cells73, graph107, evaluate185, store195. Tests/fixtures/licenses are excluded.

## Fresh host-signal qualification after the owner repair

The interpreter scope repair landed in6993069cc6e9d5d72be5df1c46683d1b1dff4c99.
Six new ordinary production cancellation recordings use that repaired runtime,
the stock sheet entry and the same logical jobs4 at LLVM/Cranelift widths1/2/4.
Every launch retained its stdin readiness handshake and a running/effective=N/
pending=0 control receipt. The original work mounts were made unavailable.
Twelve native cross-replays passed complete stdout, exit3 and immutable tape
checks. All twelve public reference/checked host replays still refused with
`interpretation is not implemented yet` on the matching repaired runtime. These
fresh full-app residual cells are INCOMPLETE, distinct from historical binding
incompatibility. The exact fresh source/std/compiler/image/libs/tapes and errors
were routed to @agentlang-24 after the original owner step had settled. No
parallel central repair or semantic acceptance of that refusal is made.
The fresh host adapter selection uses SHEET_EVAL_31_FRESH_CANCEL=1, expects
exactly6tapes×2engines, and keeps the historical18tape checked selection separate.

These are new identities. The twelve historical host rows remain runtime-revision
incompatible under the owner's no-migration ruling; none was relabeled recovered.
The original dense16M cells and native06 raw-tape archival gap also remain.
No success/domain/dense or unchanged engine matrix was repeated for this follow-up.

Fresh setup retained its current CLI/image/std sources and ldd-selected libraries.
The first relocated compiler attempts lacked libLLVM.so.23.1 and stopped at
loader exit127 before compiling or running the app. Capturing ldd with the pinned
LLVM library directory supplied the exact portable dependency; the original
setup logs remain. This was test-tool packaging, not a language/runtime defect
or a semantic pass; approximately2minutes active effort. Current source/tool/
generator seals precede subsequent consumers; fresh tapes and receipts are kept
externally under sheet-eval-31/fresh-cancel-6993069c.

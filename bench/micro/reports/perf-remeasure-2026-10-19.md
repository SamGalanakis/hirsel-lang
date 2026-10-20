# Run-time gaps after the five performance lanes

Measured October 20, 2026, on post-perf revision
`2b7ae36bb454e62a2e26324d4de5ad4b4b6949bf`, including text-perf,
codegen-mem, collections-perf, llvm-noalias (`805a7845`) and layout-niche
(`ca58c3fa`). **14 of the 17 large native workloads are at or below 1.2×
Rust in total instructions.** This is an instruction verdict, not a claim
about isolated-core speed. All 64 sessions completed: 3,330 fresh-process
samples, ten paired rounds per workload, with identical output hashes within
each session and native outputs checked against the sealed checksum oracles.
No compiler, runtime, spec, conformance expectation, TIR, LIR or ABI changed.
Remaining architectural changes are findings for follow-up, not fixes claimed
by this measurement-only lane.

## Protocol and scope

`bench/micro/run` supplied seeded interleaving (seed 20261019), medians,
CPU31 affinity, the CPU31 session flock, a separate `tools/capped` invocation
for each sample (2560 MiB), and `perf stat` counters
`task-clock,cycles,instructions,cycles:u,instructions:u`. CPU15 tick snapshots
bracket every sample. The task-clock confidence interval is a paired-round
bootstrap (5,000 draws, 95%). No samples were excluded. Profiling is separate
from the verdict samples; profiles retain their own binaries and inputs.

The 63 ordinary sessions comprise all 17 native workloads at small, medium
and large inputs (51), explicit-region binarytrees at those sizes (3), and
nine micros. HTTP adds the 64th session. All eligible sealed Rust allocator
variants participate, along with an independently repeated Rust baseline.
The table chooses the **lowest median instruction count** among eligible
Rust variants, excluding the duplicate A/A participant; its cycles, IPC and
time denominator is that same binary. This avoids selecting a denominator
by shared-host timing noise. The heap binarytrees row deliberately includes
Rust's arena baseline, as the originating audit did; the matched allocation
comparison is stated below.

The old native controls are the October 17 sealed archive, compiler
`e1f095ab2a1117b04b4a9080183824709bed1eaa`, not the deleted October 19
perf-smash worktree. Retained October 19 micro executables supply the old
micro controls. Old and current executables run in the same session. No
cross-session time comparison is used. The JSON native source includes the
previous lane's approved bulk-copy helper edits; its old/current comparison
therefore measures source plus compiler/runtime changes, not compiler alone.

Native code is LLVM O3, regions-on-whole, production runtime; Rust uses the
sealed x86-64 references and their registered allocator variants. Micro Rust
uses `rustc -O`; JSON uses the pinned serde_json 1.0.151 default-feature
`Value` implementation in a separate Buck2 release target. Sizes are 2M
string appends, SHA over 32 MiB, 1M printed lines, 1M sorted u64s, 1M integer
map inserts/gets, 2M string-counter updates over 4,096 keys, 20k map removals,
and 20k JSON records with ten parse/stringify/drop passes. Native epoch
counts are shared by every participant in a session and retained in
`plan.json`; small cells are capped at 10,000 epochs. Startup and teardown
are included. Print output goes to a file and is hashed, not `/dev/null`.

The std JSON checksum is total serialized byte length, matching the existing
micro; this is not a semantic equality proof. Hirsel retains decimal
spellings and insertion order and rejects duplicate keys; serde_json defaults
to numeric values, sorted object keys and last-key-wins. The generated inputs
exercise neither duplicate-key nor unusual-number differences. This row is
separate from the lossless native JSON algorithm, whose byte oracle is exact.

## Large workloads and micros

Ratios are Hirsel/Rust. IPC is total instructions per total cycle, H/R.
Task-clock is CPU time, not elapsed time; its CI describes session jitter,
not host drift. The all-size table follows the findings.

| Case | Rust reference | H/R instructions | H/R user instructions | H/R cycles | IPC H / R | H/R task-clock [95% CI] | H ms / R ms |
|---|---|---:|---:|---:|---:|---:|---:|
| batch-validation-large | rust-arena | 0.485 | 0.484 | 0.472 | 4.18 / 4.06 | 0.467 [0.447, 0.492] | 52.73 / 112.80 |
| binarytrees-large | rust-arena | 1.418 | 1.439 | 1.047 | 2.88 / 2.13 | 1.064 [0.912, 1.297] | 170.44 / 160.24 |
| binarytrees-region-large | rust-arena | 1.130 | 1.116 | 0.985 | 2.19 / 1.91 | 1.006 [0.867, 1.132] | 180.95 / 179.79 |
| closure-iter-erased-large | rust-system | 1.502 | 1.503 | 1.259 | 2.19 / 1.84 | 1.252 [1.145, 1.347] | 151.30 / 120.88 |
| closure-iter-scoped-large | rust-system | 1.054 | 1.052 | 1.239 | 3.29 / 3.85 | 1.221 [0.925, 1.453] | 33.93 / 27.79 |
| expr-compiler-large | rust-arena | 0.857 | 0.867 | 0.780 | 3.77 / 3.43 | 0.782 [0.754, 0.816] | 97.82 / 125.03 |
| http-keepalive | rust | 2.080 | 75.797 | 1.927 | 0.61 / 0.57 | 1.705 [1.628, 1.811] | 47.17 / 27.66 |
| json-roundtrip-large | rust-arena | 0.991 | 0.995 | 0.862 | 3.13 / 2.72 | 0.870 [0.752, 1.043] | 125.91 / 144.80 |
| micro-json | rust | 1.585 | 1.583 | 1.242 | 2.60 / 2.04 | 1.274 [1.123, 1.338] | 395.61 / 310.60 |
| micro-map_remove | rust | 1.345 | 1.232 | 1.763 | 1.21 / 1.57 | 1.802 [1.368, 1.976] | 4.92 / 2.73 |
| micro-map_str | rust | 4.055 | 4.066 | 3.543 | 1.84 / 1.61 | 3.628 [3.145, 3.913] | 327.48 / 90.27 |
| micro-map_str_upsert | rust | 1.329 | 1.327 | 1.455 | 1.59 / 1.74 | 1.446 [1.275, 1.713] | 118.72 / 82.12 |
| micro-map_u64 | rust | 1.087 | 1.039 | 1.128 | 0.53 / 0.55 | 1.162 [0.899, 1.277] | 456.80 / 393.17 |
| micro-print | rust | 1.173 | 1.751 | 1.132 | 0.70 / 0.67 | 1.143 [1.072, 1.189] | 1664.76 / 1456.45 |
| micro-sha | rust | 1.180 | 1.184 | 1.272 | 2.80 / 3.01 | 1.272 [1.242, 1.336] | 257.95 / 202.84 |
| micro-sort | rust | 2.554 | 2.574 | 2.380 | 2.13 / 1.97 | 2.366 [2.115, 2.640] | 65.95 / 27.87 |
| micro-strbuild | rust | 1.037 | 0.991 | 1.188 | 1.65 / 1.89 | 1.185 [1.136, 1.260] | 115.40 / 97.42 |
| persistent-list-map-shared-large | rust-mimalloc | 0.879 | 0.878 | 0.915 | 2.33 / 2.43 | 0.891 [0.793, 0.999] | 151.53 / 170.14 |
| persistent-list-map-unique-large | rust-mimalloc | 0.897 | 0.896 | 0.854 | 3.13 / 2.98 | 0.862 [0.777, 0.975] | 112.01 / 129.89 |
| records-messages-flat-large | rust-system | 0.576 | 0.572 | 0.841 | 2.38 / 3.47 | 0.827 [0.703, 0.864] | 92.82 / 112.30 |
| records-messages-scalar-large | rust-system | 1.050 | 1.049 | 0.929 | 4.45 / 3.95 | 0.939 [0.839, 1.068] | 130.74 / 139.31 |
| resource-failure-heavy-large | rust-mimalloc | 0.298 | 0.297 | 0.346 | 2.13 / 2.47 | 0.342 [0.332, 0.358] | 56.34 / 164.64 |
| resource-failure-throughput-large | rust-mimalloc | 0.355 | 0.353 | 0.426 | 1.89 / 2.27 | 0.433 [0.415, 0.461] | 69.69 / 160.82 |
| saxpy-checked-f32-large | rust-mimalloc | 0.631 | 0.631 | 0.660 | 3.19 / 3.34 | 0.654 [0.622, 0.695] | 120.56 / 184.42 |
| saxpy-checked-f64-large | rust-mimalloc | 0.658 | 0.657 | 0.847 | 2.95 / 3.79 | 0.829 [0.708, 0.902] | 126.63 / 152.74 |
| saxpy-checked-integer-large | rust-mimalloc | 0.499 | 0.498 | 0.585 | 3.07 / 3.59 | 0.574 [0.506, 0.636] | 91.84 / 159.99 |
| slots-churn-large | rust-system | 0.919 | 0.918 | 0.896 | 2.99 / 2.92 | 0.889 [0.831, 0.992] | 160.53 / 180.60 |
| wordfreq-large | rust-arena | 1.340 | 1.340 | 1.083 | 3.30 / 2.66 | 1.086 [0.941, 1.249] | 143.34 / 131.98 |

## Host noise and before/after controls

The independently repeated identical Rust executable gives A/A total-instruction
ratios **0.99909–1.00636** (median 1.0000005), but task-clock ratios
**0.8733–1.2930** (median 0.99884). Thus even within these ten-round sessions,
the Rust baseline can move roughly -13% to +29% in CPU time. For the hash-map
micros, A/A variation also includes Rust's per-process randomized hash layout.
CPU15's median busy fraction ranges from 0 to 1 across Hirsel cells; for SHA
it is 0.944. All snapshots, null zero-tick intervals and perf running percentages
are retained. No observation was filtered or time-adjusted. A CI crossing a
target does not establish that target, and no shared-host timing number is an
isolated-core performance guarantee.

The following instruction-only old/current controls are from the same sessions.
Each old column names its actual retained date. Old/current times are available
in those raw sessions, but no earlier report's times enter this table.

| Workload | Old instructions/Rust | Current instructions/Rust | Current/old instructions | Old control |
|---|---:|---:|---:|---|
| persistent-list-map-unique-large | 4.794 | 0.897 | 0.1871 | Oct 17 |
| persistent-list-map-shared-large | 3.825 | 0.879 | 0.2298 | Oct 17 |
| binarytrees-large | 1.944 | 1.418 | 0.7291 | Oct 17 |
| binarytrees-region-large | 1.087 | 1.130 | 1.0400 | Oct 17 |
| json-roundtrip-large | 1.406 | 0.991 | 0.7051 | Oct 17 |
| closure-iter-erased-large | 1.271 | 1.502 | 1.1817 | Oct 17 |
| micro-strbuild | 3.101 | 1.037 | 0.3344 | Oct 19 |
| micro-sha | 1.483 | 1.180 | 0.7959 | Oct 19 |
| micro-print | 1.511 | 1.173 | 0.7759 | Oct 19 |
| micro-sort | 3.333 | 2.554 | 0.7665 | Oct 19 |
| micro-map_u64 | 1.172 | 1.087 | 0.9280 | Oct 19 |
| micro-map_str | 7.369 | 4.055 | 0.5503 | Oct 19 |
| micro-map_remove | 1681.110 | 1.345 | 0.0008 | Oct 19 |
| micro-json | 1.465 | 1.585 | 1.0822 | Oct 19 |

## Causes, architecture classification and minimal triggers

Profiles below are current-binary profiles in `profiles/`, with matching Rust
profiles. Percentages are leaf sample shares, not independent causal speedup
estimates. Each row states the narrowest demonstrated cause and its limit.
There is no controlled implementation change in this lane.

| Gap | Classification first | Profile-backed cause | Minimal trigger / follow-up seam |
|---|---|---|---|
| Heap binarytrees, 1.41–1.44× across sizes | Architectural: allocation/lifetime strategy | `binarytrees-large-hirsel.txt`: tree construction 35.54%, traversal 34.80%, remaining hot addresses in generated release glue. Disassembly shows per-branch free-list/header management; leaves already use an immortal constant, so the old per-leaf-allocation diagnosis is obsolete. Rust's fastest reference resets an arena. | `type Tree = Leaf | Branch(box Tree, box Tree); tree(d); check(t)`. Compare like allocation strategies: H heap is 0.458× Rust/mimalloc instructions; H explicit-region is 1.130× Rust/arena. The headline gap compares heap reclamation with bulk reset; it does not establish a universal ownership penalty. |
| Wordfreq, medium 1.322× / large 1.340× | Local: benchmark copy helpers and borrowed box reads | `wordfreq-large-hirsel.txt`: workload 54.02%, `seq_grow` 20.21%, `owned` 9.71%, dup 4.74%, release block 4.57%, byte slice 4.21%. The frozen `owned` and `append` helpers still push one byte at a time, while Rust bulk-copies. Repeated boxed-entry reads retain RC traffic. | `owned(bytes.slice(data,start,end))`, then chain through `nodes[index].key`. Existing bulk append and borrowed-read lowering are the relevant seams; the table preserves this original source. |
| Erased closure, small 1.260× / medium 1.453× / large 1.502× | Architectural: indirect-call/status ABI and closure representation | `closure-iter-erased-large-hirsel.txt`: workload 62.55%, add thunk 25.11%, multiply thunk 11.43%. `erased-workload.asm` shows context/storage register setup, indirect calls, AL status tests and result moves for each stage, plus repeated list bounds tests. Rust also uses genuinely indirect calls (`black_box`), but its function values use a plain two-scalar ABI. | `type Stage { call: fn(u64,u64)->u64, environment:u64 }; stages[0].call(i,env)`. Investigate typed indirect-call ABI/callable facts and bounds proofs together; do not devirtualize this intentionally erased workload to claim a fix. The combined 1.502× gap is not an isolated measurement of failure propagation alone. |
| Scoped closure, small only 1.224× | Local: fixed per-epoch formatting/builder work | The longer N=64 profile `scoped-small-long.txt` has workload 51.75%, decimal append 15.38%, main 9.09%, sequence growth 7.69%, release block 6.99%, `number` 5.59%. At large N the instruction ratio is 1.054×; the loss is not closure erasure. | Run `closure-iter/scoped` on the 64-element input and serialize the result every epoch through the frozen `number` helper. Optimize the helper/builder path separately from the scoped loop. |
| Sort, 2.554× | Local: checked partition code and algorithm/code-generation constants | `micro-sort-hirsel.txt`: partition 66.48%, quicksort 22.60%; Rust's profile is in unstable quicksort/small-sort networks. `sort-partition.asm` retains repeated root/length loads and bounds branches, a 0x468-byte frame, and two memset calls for 64-element offset arrays. The prior noalias and dereferenceability work is present; heapsort is no longer the primary algorithm. | `list.sort` on 1M seeded random u64s. Improve the existing partition implementation and proof-preserving bounds/load elimination; changing the IR is not demonstrated necessary by this profile. The profile does not apportion the 2.554× loss among these costs. |
| String counter with get+insert, 4.055× | Local: caller API choice, with runtime copy cost exposed | `micro-map_str-hirsel.txt`: probe 32.30%, SipHash 14.76%, unique-copy entry 7.22%, graph copy 6.05%, alloc 5.04%, holder walks 4.64%. Every update hashes/probes twice and constructs a unique key copy, including hits. | `match map.get(m,w) { Some(c)=>map.insert(mut m,unique w,c+1), ... }`. In the upsert session, the same-compiler two-probe control costs 4.054× Rust; upsert costs 1.329×, a **67.2% instruction reduction**, measured in that one session. Keep both API shapes visible. |
| String counter with upsert, 1.329× | Architectural: current ordered Map index/probe structure | `micro-map_str_upsert-hirsel.txt`: native probe 62.78%, SipHash 16.73%; copying is largely gone. Rust uses hashbrown lookup and vectorized memcmp. Hirsel's insertion-ordered entry store still adds dependent index/tag/key loads; the nested String niche fix is already present. | A single upsert per selection from 4,096 prebuilt String keys. Work at the core Map index/probe and comparison seam. Ordered iteration is a semantic difference from Rust HashMap, but no irreducible ordering cost is established: an equally ordered optimized reference/control was not measured. |
| Insert then remove 20k u64 keys, 1.345× total / 1.232× user | Architectural: current ordered Map index maintenance; startup also matters at this size | The longer `micro-map-remove-long-hirsel.txt` has probe 42.99%, index 17.78%, SipHash 10.76%, insert 6.81%, remove 3.86%, prepare 3.35%. The old rehash-and-shift quadratic path is not the hot path. Total/user disagreement and the 4.92 ms H process include startup/kernel work; do not assign the entire 1.345× to remove itself. | Insert then remove all keys, N=20,000, one epoch. Improve the index/probe representation; retain deterministic iteration. The retained old binary costs 1681× Rust total instructions here, versus 1.345× now. This one size alone is not an asymptotic proof. |
| Std JSON vs serde_json, 1.585× | Architectural: DOM/storage representation; local buffer building, with semantic differences | `micro-json-hirsel.txt`: seq growth 10.74%, parse node 10.21%, quoted string 8.66%, write value 6.76%, allocator growth 5.15%, map insert 5.07%, write text 4.52%, probe 3.90%, UTF-8 conversion 3.87%. Repeated buffer growth, per-string construction and ordered-map objects dominate the extra work. This is distinct from native lossless JSON at 0.991×. | `json.stringify(json.parse(copy doc))` ten times over the 20k-record generated document. Improve builders and object/string construction within std's existing contracts; do not replace lossless numbers or duplicate-key rejection with serde semantics to claim parity. This comparison does not isolate the price of either guarantee. |
| HTTP keep-alive, 2.080× total / 75.797× user | Architectural: HTTP exchange transfer and runtime capability structure | `http-keepalive-hirsel-flat.txt`: seq growth 4.59%, allocator growth 4.21%, alloc 3.35%, next_stream 3.04%; transfer/from-transfer, graph copy, holders, shared recount, release and capability dispatch are spread across many functions. `http_h2.hir` freezes/thaws connection data and uses a new return channel for each HTTP/1.1 request. | Repeated `http.next_stream`/`http.respond` on one connection. Retain owned ServerStream semantics while avoiding whole-connection snapshot/transfer work on the serial path. The Rust fixture is a deliberately narrow request recognizer; the full ratio is not a quantified price of capabilities or a like-for-like HTTP library comparison. |
| Print, 1.751× user, although total is 1.173× | Local: temporary formatting plus output adapter work | `micro-print-user-hirsel.txt` uses cpu-clock:u to avoid attributing skid at syscall boundaries as user work. It shows libc write 50.32%, decimal append 5.54%, release block 5.06%, alloc 4.11%, output write 3.01%, byte append 2.85%, sequence growth 2.22%, UTF-8 conversion 1.90%. Per-line f-strings still allocate, validate and release; provider dispatch adds work. | `out.print(output, f"line {i}\n")` for 1M lines. Borrowed output is present; remaining work is formatting/adapter policy. Do not hide user overhead by reporting only syscall-dominated totals. |

Two time-only rows cross 1.2× without a >1.2× instruction verdict:
scoped/large (1.221× time, CI [0.925,1.453]) and region trees/small
(1.211×, CI [0.876,1.353]) have intervals including parity and substantial
sibling activity; they do not establish code regressions. SHA has 1.180×
instructions (CI [1.1801,1.1815]) and 1.272× time, CI [1.242,1.336]. Its
profile is rounds 74.87%, input generation 12.91%, direct-block compression
8.46%; lower IPC contributes to time beyond the instruction ratio. The
old constant-K stack-copy and byte-by-byte block staging are gone. Disassembly
still copies the dynamic 256-byte `take initial` schedule into local stack
storage in `rounds`; this is a local transfer/copy-elision opportunity, not
proof that the whole residual SHA cost is copying or irreducible ownership.

**No irreducible guarantee cost was proved.** V runs during compilation, not
inside these measured native processes. Deterministic ordering, lossless JSON,
capability admission, checked arithmetic and failure propagation are retained,
but these profiles do not isolate a lower bound for any guarantee. Future
changes to callable ABI or ownership transfer must address those architectural
seams and be measured against equivalent contracts. No TIR/LIR, ABI or durable
encoding change is proposed as already validated here.

## HTTP latency and comparison limits

Server CPU31, independent open-loop client CPU7, one server thread each, one
persistent connection, 1,024 responses per fresh process, offered load
2,048 requests/second. Every offered request is sent on its scheduled cadence;
latency is measured from scheduled send time, including queueing. Ten H,
ten Rust and ten repeated-Rust process samples all completed with no response
errors. Median per-sample latency percentiles: H p50 **612 µs**, p99 **2,260 µs**;
Rust (including its repeated baseline) p50 **617 µs**, p99 **2,560 µs**.
There is no old 40 ms per-response keep-alive stall in this workload. This is
not a saturation, tail-at-knee, concurrency or idle-connection scaling result.

H uses about **110,532 total / 47,647 user instructions per request**, versus
Rust **53,150 total / 629 user**. These totals include process startup and the
final EOF exchange divided over 1,024 requests. The large total/user difference
comes from kernel networking work. The Rust fixture follows std's TcpListener
accept/read/write structure and recognizes only the fixed benchmark request;
it does not validate the full HTTP grammar, provide HTTP/2 or transport
capabilities. That deliberate reduced grammar is a **cost-floor reference**,
not a production protocol implementation or a guarantee-equivalent comparator.
The next HTTP measurement needs a full HTTP-library Rust pair and the parked
connection sweep; neither is inferred from this fixture.

## Verification, artifacts and limits

The new JSON and upsert programs produced matching independent Rust, LLVM
and Cranelift checksums at N=1000 (six executions). HTTP completed all counted
LLVM/Rust samples and an additional Cranelift 1,024-response correctness run.
No existing test or corpus case was added, removed or weakened; the enforced
count budget still contains the original 25 cases (17 native + eight micros).
The two extended micros are explicit measurements, not silently new budgets.

The targeted gate ran once: 157 workspace build targets, 132 Clippy twins,
formatting, documentation, tool tests, native harness tests and all **25 count
cases actually executed with valid outputs**. Its 29 other checks passed;
the historical symmetric count comparison failed because every case improved
8–42% against the stale table. Main subsequently supplied its reblessed,
one-sided ratchet. Re-comparing the **already executed counts** with main
`7f2b5da9`'s current baseline and comparator gives **25/25 passing, zero errors**
(`gate-counts-current-baseline.json`); no test was repeated and no count budget
was changed in this lane. The old failed gate log is retained, not called green.
Current lane policy no longer requires a gate receipt for landing.

A Cranelift compile probe using the old compiler/precheck against a moving
stdlib hit the unchanged 2560 MiB guard (reported resident peak 2263 MiB;
scoped peak 2211 MiB). Matching the source to the sealed compiler snapshot
completed at 521 MiB. No cap was raised. Build-file cache later reached the
session's 6 GiB envelope; per-invocation cache reclaim reduced it below 2.2 GiB.
These are recorded resource findings, not runtime benchmark exclusions.

Raw evidence is retained at
`/workspace/code/hirsel-briefs/perf-remeasure-2026-10-19/`:
`plan.json`, `measurement-scope.json`, `participants/`, all 64 `sessions/*.json`,
`table.json`, `profiles/`, matching std `snapshot/`, `inputs/`, the sealed current
`native/`, `micro/`, `http/` executables, and their build/validation logs.
Sessions include executable/input/output hashes, counters, IPC, perf running
percentages, round order and sibling snapshots. `artifact-sha256.json` seals
the primary inputs and executables. Native Rust sources and all input/oracle
identities match the sealed archive. Failed launch setup attempts are retained
under `failed-launch-sessions/` and never enter a verdict.

Replay a retained ordinary participant manifest by invoking the repository's
`bench/micro/run` from the artifact directory with `--seed 20261019 --rounds 10`.
For HTTP, start the independent CPU7 client first, with 30 connections for the
three-participant manifest. `bench/micro/build json --size 20000 --output OUT`
and `bench/micro/build map_str_upsert --size 2000000 --output OUT` reproduce
the added reference pairs. Native source/compiler revisions and the old-control
dates must remain explicit; this report is not a claim about changes that
landed after the measured revision.

## Every registered input size

| Case | Rust reference | H/R instructions | H/R user instructions | H/R cycles | IPC H / R | H/R task-clock [95% CI] | H ms / R ms |
|---|---|---:|---:|---:|---:|---:|---:|
| batch-validation-large | rust-arena | 0.485 | 0.484 | 0.472 | 4.18 / 4.06 | 0.467 [0.447, 0.492] | 52.73 / 112.80 |
| batch-validation-medium | rust-arena | 0.490 | 0.489 | 0.440 | 4.11 / 3.71 | 0.432 [0.358, 0.492] | 55.02 / 127.50 |
| batch-validation-small | rust-arena | 0.513 | 0.506 | 0.545 | 2.87 / 3.05 | 0.546 [0.460, 0.627] | 11.97 / 21.94 |
| binarytrees-large | rust-arena | 1.418 | 1.439 | 1.047 | 2.88 / 2.13 | 1.064 [0.912, 1.297] | 170.44 / 160.24 |
| binarytrees-medium | rust-arena | 1.435 | 1.436 | 1.452 | 3.35 / 3.39 | 1.427 [1.409, 1.471] | 160.94 / 112.77 |
| binarytrees-region-large | rust-arena | 1.130 | 1.116 | 0.985 | 2.19 / 1.91 | 1.006 [0.867, 1.132] | 180.95 / 179.79 |
| binarytrees-region-medium | rust-arena | 1.120 | 1.120 | 1.081 | 3.13 / 3.04 | 1.073 [0.930, 1.273] | 136.66 / 127.34 |
| binarytrees-region-small | rust-arena | 1.112 | 1.111 | 1.205 | 3.20 / 3.47 | 1.211 [0.876, 1.353] | 133.05 / 109.87 |
| binarytrees-small | rust-arena | 1.407 | 1.406 | 1.426 | 3.38 / 3.43 | 1.408 [1.268, 1.563] | 158.61 / 112.68 |
| closure-iter-erased-large | rust-system | 1.502 | 1.503 | 1.259 | 2.19 / 1.84 | 1.252 [1.145, 1.347] | 151.30 / 120.88 |
| closure-iter-erased-medium | rust-system | 1.453 | 1.450 | 1.296 | 2.06 / 1.83 | 1.306 [1.092, 1.560] | 21.55 / 16.50 |
| closure-iter-erased-small | rust-system | 1.260 | 1.224 | 1.237 | 1.51 / 1.48 | 1.211 [0.958, 1.534] | 5.01 / 4.14 |
| closure-iter-scoped-large | rust-system | 1.054 | 1.052 | 1.239 | 3.29 / 3.85 | 1.221 [0.925, 1.453] | 33.93 / 27.79 |
| closure-iter-scoped-medium | rust-system | 1.085 | 1.068 | 1.149 | 2.56 / 2.69 | 1.146 [1.012, 1.404] | 6.67 / 5.82 |
| closure-iter-scoped-small | rust-system | 1.224 | 1.159 | 1.336 | 1.46 / 1.59 | 1.305 [1.189, 1.547] | 3.12 / 2.40 |
| expr-compiler-large | rust-arena | 0.857 | 0.867 | 0.780 | 3.77 / 3.43 | 0.782 [0.754, 0.816] | 97.82 / 125.03 |
| expr-compiler-medium | rust-arena | 0.867 | 0.867 | 0.830 | 3.90 / 3.74 | 0.838 [0.804, 0.884] | 95.88 / 114.37 |
| expr-compiler-small | rust-arena | 0.870 | 0.870 | 0.872 | 3.82 / 3.83 | 0.876 [0.843, 0.906] | 98.03 / 111.96 |
| http-keepalive | rust | 2.080 | 75.797 | 1.927 | 0.61 / 0.57 | 1.705 [1.628, 1.811] | 47.17 / 27.66 |
| json-roundtrip-large | rust-arena | 0.991 | 0.995 | 0.862 | 3.13 / 2.72 | 0.870 [0.752, 1.043] | 125.91 / 144.80 |
| json-roundtrip-medium | rust-arena | 1.000 | 1.000 | 1.001 | 3.40 / 3.40 | 1.016 [0.942, 1.086] | 114.53 / 112.77 |
| json-roundtrip-small | rust-arena | 1.007 | 1.007 | 0.993 | 3.32 / 3.28 | 1.000 [0.847, 1.072] | 117.28 / 117.22 |
| micro-json | rust | 1.585 | 1.583 | 1.242 | 2.60 / 2.04 | 1.274 [1.123, 1.338] | 395.61 / 310.60 |
| micro-map_remove | rust | 1.345 | 1.232 | 1.763 | 1.21 / 1.57 | 1.802 [1.368, 1.976] | 4.92 / 2.73 |
| micro-map_str | rust | 4.055 | 4.066 | 3.543 | 1.84 / 1.61 | 3.628 [3.145, 3.913] | 327.48 / 90.27 |
| micro-map_str_upsert | rust | 1.329 | 1.327 | 1.455 | 1.59 / 1.74 | 1.446 [1.275, 1.713] | 118.72 / 82.12 |
| micro-map_u64 | rust | 1.087 | 1.039 | 1.128 | 0.53 / 0.55 | 1.162 [0.899, 1.277] | 456.80 / 393.17 |
| micro-print | rust | 1.173 | 1.751 | 1.132 | 0.70 / 0.67 | 1.143 [1.072, 1.189] | 1664.76 / 1456.45 |
| micro-sha | rust | 1.180 | 1.184 | 1.272 | 2.80 / 3.01 | 1.272 [1.242, 1.336] | 257.95 / 202.84 |
| micro-sort | rust | 2.554 | 2.574 | 2.380 | 2.13 / 1.97 | 2.366 [2.115, 2.640] | 65.95 / 27.87 |
| micro-strbuild | rust | 1.037 | 0.991 | 1.188 | 1.65 / 1.89 | 1.185 [1.136, 1.260] | 115.40 / 97.42 |
| persistent-list-map-shared-large | rust-mimalloc | 0.879 | 0.878 | 0.915 | 2.33 / 2.43 | 0.891 [0.793, 0.999] | 151.53 / 170.14 |
| persistent-list-map-shared-medium | rust-mimalloc | 0.945 | 0.945 | 1.035 | 2.75 / 3.02 | 1.018 [0.902, 1.181] | 139.20 / 136.69 |
| persistent-list-map-shared-small | rust-mimalloc | 0.972 | 0.968 | 0.973 | 2.84 / 2.83 | 0.939 [0.769, 1.120] | 26.93 / 28.67 |
| persistent-list-map-unique-large | rust-mimalloc | 0.897 | 0.896 | 0.854 | 3.13 / 2.98 | 0.862 [0.777, 0.975] | 112.01 / 129.89 |
| persistent-list-map-unique-medium | rust-mimalloc | 0.960 | 0.959 | 0.980 | 3.48 / 3.55 | 0.978 [0.926, 1.004] | 98.41 / 100.65 |
| persistent-list-map-unique-small | rust-mimalloc | 0.979 | 0.973 | 1.022 | 3.14 / 3.28 | 1.028 [0.986, 1.085] | 12.91 / 12.55 |
| records-messages-flat-large | rust-system | 0.576 | 0.572 | 0.841 | 2.38 / 3.47 | 0.827 [0.703, 0.864] | 92.82 / 112.30 |
| records-messages-flat-medium | rust-system | 0.575 | 0.574 | 0.788 | 2.54 / 3.48 | 0.787 [0.759, 0.834] | 87.56 / 111.20 |
| records-messages-flat-small | rust-system | 0.563 | 0.561 | 0.803 | 2.40 / 3.41 | 0.803 [0.726, 0.857] | 96.05 / 119.64 |
| records-messages-scalar-large | rust-system | 1.050 | 1.049 | 0.929 | 4.45 / 3.95 | 0.939 [0.839, 1.068] | 130.74 / 139.31 |
| records-messages-scalar-medium | rust-system | 1.050 | 1.050 | 1.047 | 4.15 / 4.14 | 1.047 [0.924, 1.178] | 131.51 / 125.65 |
| records-messages-scalar-small | rust-system | 1.041 | 1.037 | 1.046 | 3.69 / 3.72 | 1.036 [0.885, 1.276] | 18.73 / 18.08 |
| resource-failure-heavy-large | rust-mimalloc | 0.298 | 0.297 | 0.346 | 2.13 / 2.47 | 0.342 [0.332, 0.358] | 56.34 / 164.64 |
| resource-failure-heavy-medium | rust-mimalloc | 0.303 | 0.301 | 0.357 | 2.11 / 2.49 | 0.356 [0.339, 0.379] | 43.55 / 122.24 |
| resource-failure-heavy-small | rust-mimalloc | 0.348 | 0.336 | 0.454 | 1.71 / 2.23 | 0.443 [0.432, 0.474] | 8.19 / 18.50 |
| resource-failure-throughput-large | rust-mimalloc | 0.355 | 0.353 | 0.426 | 1.89 / 2.27 | 0.433 [0.415, 0.461] | 69.69 / 160.82 |
| resource-failure-throughput-medium | rust-mimalloc | 0.357 | 0.356 | 0.444 | 1.88 / 2.34 | 0.429 [0.411, 0.450] | 65.47 / 152.46 |
| resource-failure-throughput-small | rust-mimalloc | 0.392 | 0.383 | 0.505 | 1.67 / 2.15 | 0.506 [0.486, 0.527] | 11.16 / 22.04 |
| saxpy-checked-f32-large | rust-mimalloc | 0.631 | 0.631 | 0.660 | 3.19 / 3.34 | 0.654 [0.622, 0.695] | 120.56 / 184.42 |
| saxpy-checked-f32-medium | rust-mimalloc | 0.636 | 0.631 | 0.754 | 2.62 / 3.10 | 0.754 [0.718, 0.817] | 24.07 / 31.91 |
| saxpy-checked-f32-small | rust-system | 0.720 | 0.695 | 0.899 | 1.93 / 2.41 | 0.906 [0.879, 1.051] | 5.90 / 6.52 |
| saxpy-checked-f64-large | rust-mimalloc | 0.658 | 0.657 | 0.847 | 2.95 / 3.79 | 0.829 [0.708, 0.902] | 126.63 / 152.74 |
| saxpy-checked-f64-medium | rust-mimalloc | 0.694 | 0.690 | 0.783 | 2.64 / 2.98 | 0.786 [0.705, 0.900] | 26.21 / 33.36 |
| saxpy-checked-f64-small | rust-system | 0.732 | 0.707 | 0.939 | 1.90 / 2.44 | 0.915 [0.881, 0.995] | 6.23 / 6.81 |
| saxpy-checked-integer-large | rust-mimalloc | 0.499 | 0.498 | 0.585 | 3.07 / 3.59 | 0.574 [0.506, 0.636] | 91.84 / 159.99 |
| saxpy-checked-integer-medium | rust-mimalloc | 0.507 | 0.503 | 0.693 | 2.55 / 3.49 | 0.677 [0.598, 0.729] | 19.30 / 28.54 |
| saxpy-checked-integer-small | rust-system | 0.613 | 0.584 | 0.851 | 1.82 / 2.53 | 0.827 [0.654, 0.888] | 5.17 / 6.26 |
| slots-churn-large | rust-system | 0.919 | 0.918 | 0.896 | 2.99 / 2.92 | 0.889 [0.831, 0.992] | 160.53 / 180.60 |
| slots-churn-medium | rust-system | 0.750 | 0.747 | 0.833 | 2.57 / 2.85 | 0.806 [0.695, 0.919] | 37.54 / 46.55 |
| slots-churn-small | rust-system | 0.776 | 0.756 | 1.132 | 2.17 / 3.17 | 1.136 [0.955, 1.217] | 7.03 / 6.19 |
| wordfreq-large | rust-arena | 1.340 | 1.340 | 1.083 | 3.30 / 2.66 | 1.086 [0.941, 1.249] | 143.34 / 131.98 |
| wordfreq-medium | rust-arena | 1.322 | 1.322 | 1.017 | 3.86 / 2.96 | 1.009 [0.895, 1.132] | 114.97 / 113.97 |
| wordfreq-small | rust-arena | 1.166 | 1.163 | 0.887 | 3.79 / 2.88 | 0.914 [0.813, 1.079] | 27.85 / 30.46 |

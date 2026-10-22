Current matched follow-up: [FAST-2.md](/workspace/code/hirsel-briefs/http-fast-2/repo-evidence/bench/micro/http/FAST-2.md). The ≤3× target remains **INCOMPLETE**.

This is a historical report. New measurement campaigns, raw sessions and
reproduction scripts belong in `/workspace/code/hirsel-briefs/<lane>/`.
Keep only a short result row or archive link in an existing results page.

# HTTP serial measurements — 2026-10-21

The serial HTTP/1 path improves, but the ≤3× hello-web target is **not met**.
The final candidate costs 31,556.9 user instructions/request for hello-web
(6.0349× Tokio + httparse), down 23.69% from 41,353.7 (7.9084×).
Keep-alive costs 18,237.5 (7.7872×), down 36.88% from 28,893.9 (12.3374×).
The symbol attribution below records the remaining work; these numbers do not
establish parity with a general Rust HTTP server.

## Method

One session (`c4da72ed-d98a-47f7-90a4-b0c4fa27e886`), ten rounds, seed
20261021, server CPU31 and client CPU7. The existing micro runner interleaves
all participants in seeded order while holding the shared CPU31 timing lock.
Each fresh process serves 1024 requests and prints 1024. Counters include startup;
reported values are medians of process counts divided by 1024. Both all-mode
cycles/instructions and user events were recorded. [measurements.json](/workspace/code/hirsel-briefs/http-fast/repo-evidence/bench/micro/http/measurements.json)
contains the counters and executable hashes. A single fixed release compiler
and runtime were used for every Hirsel variant; only the saved std/example
sources changed. Base source: `49b74942`.

Hello uses the production hello-web example bounded to 1024 accepts, with
2000 new connections/second. Keep-alive uses one connection at 2048 requests/s.
The keep-alive fixture retains the http-serial-path harness request/response
bytes, 1024-request loop, port and open-loop client, adapted to `serve_next`.
Clients schedule sends independently of response arrival. `wrk2` is unavailable,
so this finite client replaces it; startup and this reference's ~5.2k hello user
instructions differ from the historical ~2.6k reference. These are within-session
ratios, not a reproduction of the old wrk2 absolute count. The pinned reference
is Tokio 1.53.0 + httparse 1.10.1, one current-thread runtime without a per-request
task. It parses GET heads generally but asserts this fixture's method/target
and emits fixed responses; it omits Hirsel's generic body/framing/capability
machinery. The original std TCP fixture is included only as a lower cost bound.

## Each change

Each row includes all earlier changes, except the two alternative writer rows.
The `input` row retains the old fresh response writer. `final` is the first
reused writer; its per-byte helper regressed both workloads. `batched` replaces
that helper with bulk appends for growth and direct indexed overwrites for reuse.
It improves keep-alive over `input` by 3.28%, but costs 2.95% more for hello,
where each connection emits only one response and cannot amortize its storage.

| Variant | Workload | User instructions/request | / Tokio | All instructions/request | All cycles/request |
|---|---|---:|---:|---:|---:|
| Rust TCP floor | keep | 628.4 | 0.2683× | 51,189.0 | 80,592.4 |
| Tokio + httparse | hello | 5,229.1 | 1.0000× | 149,194.2 | 276,911.3 |
| Tokio + httparse | keep | 2,342.0 | 1.0000× | 26,972.5 | 54,531.9 |
| Before | hello | 41,353.7 | 7.9084× | 182,279.8 | 339,234.6 |
| Before | keep | 28,893.9 | 12.3374× | 85,557.6 | 138,206.5 |
| Local inline dispatch | hello | 33,350.8 | 6.3779× | 175,277.2 | 328,435.2 |
| Local inline dispatch | keep | 22,148.0 | 9.4570× | 78,952.6 | 125,026.8 |
| Header spans | hello | 31,637.8 | 6.0504× | 172,292.6 | 329,740.3 |
| Header spans | keep | 20,413.5 | 8.7164× | 76,903.4 | 125,870.4 |
| Shared input buffer | hello | 30,653.3 | 5.8621× | 168,863.1 | 320,623.4 |
| Shared input buffer | keep | 18,854.4 | 8.0507× | 75,084.8 | 127,627.2 |
| First reused writer (rejected) | hello | 33,591.6 | 6.4240× | 174,193.8 | 323,547.9 |
| First reused writer (rejected) | keep | 20,747.7 | 8.8590× | 76,783.3 | 127,893.2 |
| Batched reused writer (final) | hello | 31,556.9 | 6.0349× | 172,208.1 | 318,260.6 |
| Batched reused writer (final) | keep | 18,237.5 | 7.7872× | 73,945.6 | 124,346.4 |

Times are not used as the verdict. All-cycle values include kernel/network work
and should not be interpreted as HTTP parser costs. The final keep user ratio
to the minimal Rust TCP floor is 29.02× (before: 45.98×).

## Prior art and semantic deviations

* httparse `a0fa552e4e0fa9a5914a736ef3a556748fef709b`,
  `src/lib.rs:458–520`: borrow input and parse request/header spans in one scalar
  pass. The existing `http_wire` grammar is retained. Sixteen scratch entries
  are batched under the configured header limit, not a new protocol limit.
* hyper `b07337878fe57b34b2065335ae8cae9ce5a780a0`,
  `src/proto/h1/dispatch.rs:185–203,320–372,375–482`,
  `conn.rs:216–289,614–654`, `role.rs:390–520,662`:
  drive read/handler/write on one connection and retain output storage.
  `tests/server.rs:691–728,1072–1113` supplies keep-alive/pipeline structure.
* Ownership: `serve_next` is an additive callback API. Existing `next_stream`
  still yields owned, sendable handles for task handoff, streaming and detachment.
  Automatic detection of moving a stream is not implemented: preserving public
  owned request fields without compiler/runtime changes requires explicit API
  selection. Its channel is allocated lazily on the first owned checkout.
* Ownership: byte spans retain one immutable Bytes owner instead of Rust
  references. The existing take conversion shares input; COW preserves retained
  views. Only the owned transfer seal deep-copies input into unique storage.
* Policy: existing request-field UTF-8 validation remains. The completed head
  is checked once, and method/target are read for target-policy validation;
  application field accessors materialize individual Strings lazily. Thus not
  every String conversion waits for application access. httparse permits opaque
  obs-text where the existing Hirsel public String API does not.
* Storage: output uses an initialized List prefix and a cursor because std has
  no borrowed List byte view or truncate API. A take adapter shares that prefix
  for the write, then returns the capacity. Small bodies join the write; large
  bodies remain separate. Framing stays in HTTP and no f-string head is built.
* Capabilities/replay: explicit Net/Tasks and scripted-peer IO remain in place;
  no unmanaged socket, compiler optimization or runtime ABI change is included.

## Remaining symbols and causes

Profiles were collected after each stage with user-instruction sampling (period
10,000). The later stages use DWARF stacks; the original before/inline keep
frame-pointer callers were unreliable, so their flat samples were used only.
The final profiles contain 1,707 keep and 2,327 hello samples. These short
profiles guide attribution; they are not substitutes for the ten-round verdict.
All final symbols with ≥0.3% self samples and their causal classification are in
[profile-hello.tsv](/workspace/code/hirsel-briefs/http-fast/repo-evidence/bench/micro/http/profile-hello.tsv) and [profile-keep.tsv](/workspace/code/hirsel-briefs/http-fast/repo-evidence/bench/micro/http/profile-keep.tsv).

| Final symbol (self) | Hello | Keep | Cause / next seam |
|---|---:|---:|---|
| `http_output_append` | 1.72% | 3.93% | Reused initialized prefix still overwrites bytes with indexed List stores; a bulk overwrite/truncate interface would avoid repeated checks. Growth is already batched. |
| `alloc::grow` | 3.78% | 1.05% | Managed buffers/records and IO results; per-connection hello cannot amortize storage. |
| `seq_grow` | 1.63% | 1.87% | Remaining input/span/response sequence growth; output reuse removed only one source. |
| `memset` / `memmove` families | 1.16% / 2.02% | 1.70% / 0.94% | Initialization/copy leaves shared by generated structures and IO results; callers are needed to split these further. |
| `_int_malloc` | 1.93% | 0.29% | Allocator bookkeeping for managed storage. |
| `State::net_error` | 1.85% | 2.34% | Validates/maps each Net operation's outcome. |
| `RealIo::perform` | 1.50% | 1.11% | Runtime IO operation dispatch. |
| `State::execute_net` | 1.59% | 1.23% | Net provider operation dispatch/result handling. |
| `Adapter::call` family | 1.55% | 2.16% | Capability adapter boundary per IO call. |
| `State::stream_grant` | 1.38% | 1.87% | Checks stream authority on operations. |
| `host_waits::issue` family | 2.06% | 1.29% | Capability call/wait completion path. |
| `capability_call` | 1.12% | 1.05% | Runtime capability bridge. |
| `CooperativeNet::set_timeout` | 0.77% | 1.05% | Read/write timeout setup; repeated settings are a future runtime/transport seam. |
| `tasks::poll_parked` | 0.56% | 0.70% | Cooperative readiness work. |
| `http_receive_view` | 1.46% | 1.87% | Head/body/deadline state and retained span-view construction. |
| `http_server_view` | 1.63% | 0.88% | Scalar parse, framing and whole-head UTF-8/target policy. |
| `serve_next` | 0.99% | 1.46% | Move exchange locally, handler call and result/state cleanup. |
| `validate_headers` | 0.95% | 0.82% | Response header syntax and framing ownership. |
| `http_output_head` | 1.29% | 1.46% | Serialize response fields into retained storage. |
| `hirsel_rt_v2_dup` | 0.60% | 0.47% | Runtime sharing credits for handles/values. |

The bridge/capability/runtime/socket rows are retained because they enforce
Hirsel's authority and replay semantics; changing the socket/runtime layer is
outside this lane. The highest output symbol has already been iterated once:
the initial per-byte writer regressed, and batched growth recovers that loss.
The scalar overwrite loop remains a std/core/compiler interface opportunity.

There is substantial unresolved sampling: about 27% of hello and 46% of keep
samples have kernel-looking unresolved addresses even though the event is
`instructions:u`. We retain those rows explicitly rather than assigning them to
user functions. The host PMU reports `max_precise=0`, so precise instruction sampling is not
available to remove skid here. Small user leaves below the cutoff and ambiguous libc callers
also remain. Thus this is symbol-by-symbol attribution of the resolved hot
symbols, **not a complete causal accounting of every excess instruction**.
The performance target and that unresolved residual stay open.

## Validation

* HTTP corpus: 56/56 selected `SEM-1-http*` paths passed (55 before, one new
  inline case). The original seven-shard selection timed out on one eight-case
  shard at the pool's 600-second action deadline; all eight individual slices
  passed. Corpus passing inventory: 6508 → 6509 at the lane base.
* Selected agreement: all 14 slices passed on the same 56 input paths, seed 1,
  limit 5000. There were 81 checked variants: 37 rejected, 44 accepted, zero
  bug clusters. All 44 accepted variants hit agreement's unchanged 10,000-step
  budget, so this establishes checking/no detected cluster, not completed
  runtime equivalence for those variants. The corpus and native checks supply
  execution coverage.
* stdlib HTTP/HTTP-wire: 24 passed, one existing failure:
  `stdlib_http::http2_adapter_scripts_record_and_replay` asserts `net.connect`
  for a server fixture that listens/accepts. This exact failure predates the lane
  and is recorded in `tools/known-failures.tsv` (owner `std-web-2`).
* Tasks end-to-end: 11 differential HTTP/HTML, four corrupt/save-failure,
  one SIGTERM and six storage-provider checks passed.
* New inline fixture: interpreter, LLVM and Cranelift each passed 1/1. It covers
  pipelined POST/body ownership, field access, HEAD metadata/body suppression,
  response reuse, HTTP/1.0 close and EOF. Existing corpus covers owned handoff.
* Standard precheck passed with the unchanged 2 GiB guard; final batched source
  used 1792 MiB. No compiler or runtime limit was raised.
* Native HTTP parity: 55 selected paths passed LLVM/Cranelift, including the new
  inline case. One existing HTTP/2 server case failed in the LLVM control with
  the `shared.freeze` multi-holder invariant. net-gaps independently reproduced
  it on unchanged baseline HTTP/2 transfer code and registered the known failure
  plus `native-tests/regressions/open/http2-session-freeze.hir`. One original
  shard timed out at 600 seconds; all eight remaining individual cases passed.

The hello example now isolates both read and response failures to the current
connection and continues accepting. Previously it ignored receive failures but
could return from the server on a response failure; this is an intentional
example-policy change needed by its one-call inline request/response handler.

## Archived campaign notes

Campaign data and reproduction scripts live in `/workspace/code/hirsel-briefs/<lane>/repo-evidence/`.

| Campaign | Archived report |
| --- | --- |
| http-replace-range-impl-7 | [RESULTS-http-replace-range-impl-7.md](/workspace/code/hirsel-briefs/http-replace-range-impl-7/repo-evidence/bench/micro/http/RESULTS-http-replace-range-impl-7.md) |

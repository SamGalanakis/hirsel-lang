# Serial HTTP/1 follow-up — 2026-10-21

**INCOMPLETE: the ≤3× user-instruction target is not met.** Initial response
reservation reduces hello user instructions by 2.18%; keep-alive increases
0.32%. The remaining ratios are
7.2371× hello and
9.4270× keep-alive against the retained
Tokio 1.53.0 + httparse 1.10.1 library participant. The Rust grammar is unchanged.

## Matched measurement

Frozen source base `0a0d9aed835ce1808f18342a139835935187df05`. Baseline remeasurement session
`74d72bb1-823e-491f-a609-00b36b5e6f3c` completed 70 samples before choosing
the change. The accepted before/after session is `c4d18d7b-93d4-4c7d-afda-11b9dab592cd`:
10 rounds, seed 20261021, 110 interleaved samples (including separate
allocation builds and the owned `next_stream` control), 1024 requests per
fresh server. No samples were excluded. Values below are medians of process
counters divided by 1024; the ratios divide those medians within this session.
The earlier http-fast results used a different compiler/runtime and are retained
as historical evidence, not a matched regression comparison.

Both sources use one frozen native builder, LLVM 21.1.8, O3, whole-program
partition, production regions/reuse and the same default runtime archive.
The Rust participant was built with rustc 1.99.0
`b940084d7eb6a299eb4bfeb8e34901bc051e7ac4`; its unchanged Cargo.lock pins
the library versions. Each process includes startup, grants/validation and
cleanup. Perf records task-clock, instructions, cycles and their user events,
including descendants. Servers run on CPU31; the existing finite open-loop
clients run on CPU7 (2000 new connections/s hello, 2048 requests/s keep).
All participants use one CPU and the unchanged 4 GiB capped budget. The shared
CPU7 lock is acquired before the micro runner's CPU31 lock. SMT sibling activity
and all perf running percentages are retained. No time ratio is used as a gate.

| Participant | User instructions/request | / library user | Total instructions/request | Total cycles/request |
| --- | ---: | ---: | ---: | ---: |
| rust-hello | 5,282.8 | 1.0000× | 149,543.2 | 299,529.7 |
| before-hello | 39,082.8 | 7.3981× | 189,532.1 | 435,671.4 |
| after-hello | 38,232.3 | 7.2371× | 188,747.5 | 380,199.6 |
| rust-keep | 2,372.8 | 1.0000× | 28,613.4 | 65,426.2 |
| before-keep | 22,297.7 | 9.3972× | 87,207.2 | 152,224.5 |
| after-keep | 22,368.6 | 9.4270× | 87,226.5 | 152,634.0 |
| owned-control-keep | 39,071.5 | 16.4664× | 105,098.0 | 203,308.2 |

The after/before user-instruction ratio is 0.978238 hello (paired-round
bootstrap 95% interval 0.951015–0.996481) and 1.003178 keep-alive
(0.989779–1.020527). This interval describes within-session variation only;
keep-alive has no demonstrated instruction improvement.

The owned control remains a compatibility participant. Its materialized request
fields and explicit transport handoff have additional costs and are not the
inline-path denominator.

## Attribution and bounded change

Before choosing the change, symbol self-sample groups were: parser/validation
6.43% hello /
6.95% keep; serializer
4.43% /
5.11%; buffer/allocation/copy
15.80% /
15.02%; capability/adapter
16.51% /
21.40%.
These are symbol classifications, not a complete accounting of excess
instructions. Shared libc leaves are grouped by mechanism. Unresolved samples
remain 29.33% /
26.15%; the hello capture lost
26 samples in seven chunks. No precision or causal claim is made for those rows.
The hello `alloc::grow` self samples were 2.83%, with its stack leading through
`bytes.append` to `http_output_append`; the keep overwrite helper was 1.89%.

The only production change reserves 128 List bytes after the existing response
header validator when the output buffer is still empty. This follows hyper
`b07337878fe57b34b2065335ae8cae9ce5a780a0`,
`src/proto/h1/role.rs:423–427`. The constant, instead of hyper's
`30 + header_count * 30`, covers an ordinary head and the coalesced small body
without new capacity arithmetic. Larger responses still grow; later responses
retain the existing initialized-prefix cursor. All wire grammar, validation,
capability operations, timeouts, ownership and replay interfaces are retained.
No compiler, runtime ABI, handler contract or automatic move detection changed.
See [HTTP/1 prior art](../../../docs/http1-prior-art.md#initial-response-storage).

## Allocations in the same ten rounds

Separate builds use the same compiler/options with the matching measurement
archive and emission hooks. Their instruction counters are not production
results. Each allocation build participated in all ten rounds of the accepted
session; all 40 per-round JSON profiles were captured and hashed immediately
after its process finished. These are Hirsel managed physical block operations,
including growth, not a census of Rust host/OS allocations. Frees equal
allocations at exit in every capture. Copied bytes below count runtime COW/deep
copy hooks; they do not count every serializer overwrite or buffer-growth copy.

| Allocation participant | Allocations/process | Frees/process | Copied blocks/process | Copied bytes/process |
| --- | ---: | ---: | ---: | ---: |
| before-alloc-hello | 9,216 | 9,216 | 2,048 | 32,768 |
| after-alloc-hello | 5,120 | 5,120 | 1,024 | 16,384 |
| before-alloc-keep | 3,990 | 3,990 | 2 | 32 |
| after-alloc-keep | 4,002.5 | 4,002.5 | 1 | 16 |

Hello allocations fall 44.44% (nine to five per request). Keep-alive
allocations vary with socket read coalescing: the medians increase 12.5
allocations/process (0.31%) despite eliminating one initial copy. Neither
that workload nor its measured instruction count is claimed as an improvement.

## Correctness and retained failures

* Before: 54 HTTP corpus cases, eight wire unit tests and 16 HTTP driver tests
  passed. The new five-response fixture also passed 1/1 against unchanged std.
  The existing inline native smoke passed one case, 11 runs under four
  configurations and real IO record/replay, plus a Cranelift executable.
* After: 55/55 HTTP corpus cases passed with formatting (20+20+14+1), and
  24/24 HTTP/wire driver tests passed. The replay driver now includes both
  the existing inline body fixture and the new output fixture, comparing the
  complete recorded and replayed observation.
* Three selected native smoke cases and three Cranelift parity cases passed:
  output growth/reuse; owned serial handoff, retained request and cancellation;
  and stream status/header errors. Native summaries are saved in the evidence.
* The new fixture adapts hyper `tests/server.rs:691–728,1072–1113`'s pipeline
  structure: small response, 300-byte header (beyond the reservation), smaller
  HEAD response with metadata, 204 and 304. Exact scripted wire bytes forbid
  leaked bytes from the retained suffix. Existing corpus/driver vectors cover
  malformed headers, limits, streamed bodies and cleanup independently.
* Independent physical-socket checks compare all response bytes for 1024 hello
  and 1024 keep requests before and after, plus 1024 owned-control requests:
  5120 requests, equal per-workload wire and stdout hashes, all exits zero.
  The timed clients additionally validate each response's status/body.
* Preserved setup failures: invalid target spelling `root//:*` (current Buck2
  uses `root//:`); a shadowed local in the scratch owned fixture; missing owned
  Buck outputs after a host cleanup; and an upload digest mismatch while the
  registration file was corrected during a queued invocation (zero tests).
  No failed run is counted as execution. Only the failed selections were
  retried after formatting/registration settled, with a frozen consumed-source
  manifest. The rematerialized compiler, CLI and both runtime archives matched
  their original SHA-256 values. No cap, threshold or comparator was relaxed.
* HTTP/2 freeze/replay work is separately owned by http2-freeze-2; its two
  driver tests were not part of the 24 HTTP/1/wire tests.

## Evidence and remaining trigger

Raw manifests, before/after sources, binary/source identities, every sample,
40 allocation profiles, exact-wire checks, perf data and before/failure/test
logs are retained in the lane evidence archive.
The compact [evidence JSON](fast-2-evidence.json) retains identities and the
complete accepted counter samples without machine-specific tool environments.

Smallest retained measured trigger: `bench/micro/http/server.hir` calls
`serve_next` with one static Content-Type header and `Hello, world`, for
`GET / HTTP/1.1\r\nHost: localhost\r\n\r\n` on a persistent connection.
The finite hello fixture generated by `hello_fixture.py` uses the same response
with close=true, one request per accepted connection. No body streaming, worker
handoff or HTTP/2 is needed to reproduce the remaining instruction gap.
Remaining work is the repeated initialized-prefix overwrite and parser/buffer
work plus capability/adapter dispatch, cancellation/cleanup and unresolved
sampling. A performance/adoption gate is not passed by closing this lane.

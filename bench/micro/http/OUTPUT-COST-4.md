# HTTP/1 initialized-prefix experiment — 2026-10-22

**NO-GO. The serializer remains unchanged.** Moving its retained byte list into
one local per append does not improve the small workloads and materially
regresses reused large output. The new correctness controls are retained.
The original ≤3× Rust-library performance goal remains unmet.

## Frozen experiment

The source base is `dd14443a155555e8af920a5d731d1a737cefc85d`, including the
landed HTTP/2 unique-copy fix and its replay-oracle correction. All participants
use the same frozen compiler, LLVM 21.1.8, O3 whole-program configuration and
production runtime. Before and candidate std differ only in private
`http1_parse.hir`. Request handlers, ownership, parsing, capability operations,
128-byte reservation and 65536-byte coalescing boundary are unchanged.
The retained Rust executable and its Tokio/httparse dependency lock are unchanged.
Frozen compiler SHA-256 is
`81c9670cabef936f9179e0126f434d7b6e62bc255694db468affef9ce797a28b`;
production runtime archive
`18b330dc022603617fc9d6fec5aa7ef1dad18ebebe14e5985e40fc2743f3aa2f`;
Rust executable
`59b303d9ea0506616aaabbcaec8d503f03edd2e468aa35b0eac3531e512a2a14`.
The consumed compiler/std/runtime/harness/fixture manifest seals 11,395 files.

All eleven historical FAST-2 participant entries still match their saved binary
hashes; those older results are not multiplied into this comparison.

The sole candidate moves `buffer_state.bytes` into a local using `replace`,
executes the original checked overwrite and suffix append, restores the list
with `take`, then advances `used`. It adds no unsafe operation, intrinsic, byte
bridge, truncate, runtime ABI or language rule. Validation and reserved-header
checks stay in their existing order, before any transport write.

Prior art is hyper `b07337878fe57b34b2065335ae8cae9ce5a780a0`,
[`src/proto/h1/role.rs:419–438`](https://github.com/hyperium/hyper/blob/b07337878fe57b34b2065335ae8cae9ce5a780a0/src/proto/h1/role.rs#L419)
(retained destination, initial reservation and status encoding), and
[`role.rs:1693–1695`](https://github.com/hyperium/hyper/blob/b07337878fe57b34b2065335ae8cae9ce5a780a0/src/proto/h1/role.rs#L1693)
(the span append helper). The upstream checkout's exact commit was verified.
The existing growth/pipeline fixture follows hyper `tests/server.rs:691–728`
and `1072–1113`. Hirsel preserves its established empty reason, header spelling
and ordering, owned handler result and synchronous capability handoff. Its
initialized-prefix cursor substitutes for the mutable byte operations available
to hyper; no upstream unchecked byte operation was introduced.

## Instruction verdict

Session `67eee03a-887d-42be-ae12-9ba83b79d31a` executes ten seeded interleaved
rounds, seed 20261022: 90 cells, nine participants, 1024 requests per process.
They comprise before/candidate hello and keep, unchanged owned keep, unchanged
Rust hello/keep, and before/candidate mixed-response controls. Every exit is
zero and every complete stdout is `1024\n`. Raw Cachegrind output, annotations,
stdout, stderr, client results, commands, statuses and hashes are retained per
cell; each accepted cell is checkpointed immediately. No cells are excluded.

The coordinator explicitly replaced the blocked CPU31 timing session with the
repository's `tools/perf-counts` Cachegrind convention: pinned Valgrind/loader/libc,
`--cache-sim=no --branch-sim=no`, counting `Ir`. Servers and open-loop clients
use available CPUs 13 and 14 under the same one-CPU/4-GiB budgets. The original
CPU7 hold was released. The finite clients retain their original rates and
request counts; the mixed client independently checks every response byte.
These are user-space instruction references, **not PMU `instructions:u`, kernel
instructions, cycles or native timings**. Socket read coalescing still varies;
the tool does not make an asynchronous workload's executed path constant.

| Workload | Before Ir/request | Candidate Ir/request | Candidate / before | Paired bootstrap 95% interval |
| --- | ---: | ---: | ---: | ---: |
| hello | 32,026.5 | 32,049.9 | 1.000729 | 1.000544–1.000729 |
| keep | 26,693.7 | 27,160.2 | 1.017477 | 0.999314–1.024338 |
| mixed large/small | 149,092.6 | 185,149.6 | 1.241843 | 1.236739–1.281916 |

Ratios divide within-session medians. Intervals resample paired rounds 20,000
times and describe this session only. Cachegrind candidate/library ratios are
7.4201× hello and 12.8619× keep. They do not replace the historical PMU ratios
in [FAST-2](FAST-2.md), and neither metric establishes the ≤3× goal.

The mixed workload repeatedly sends empty and three-byte bodies, a 512-byte body
with a 300-byte header, 65535/65536/65537-byte bodies, shorter responses, HEAD,
204 and 304. Its response selection and expected bytes are frozen identically
before and after. Both physical clients receive exactly 18,435,465 response
bytes per 1024 requests, SHA-256
`4ce3883f4796f85de8c2d36409f70207659a28ea52d8045c9fa03a056c27ab8c`.

## Mechanism and allocation evidence

Final binary disassembly removes one entry detach call site but retains a
uniqueness test, bounds check and scalar byte store inside the overwrite loop.
The append function grows from 1090 to 1286 bytes. Its exact self Ir is stable
across the ten rounds despite TCP coalescing elsewhere:

| Workload | Before append self Ir/process | Candidate append self Ir/process |
| --- | ---: | ---: |
| hello | 727,040 | 757,760 |
| keep | 1,453,370 | 1,782,806 |
| mixed large/small | 123,160,714 | 160,008,162 |

A separate fixed scripted peer removes socket coalescing: the same eleven
mixed responses execute three times before and three times after. Whole-process
Ir is 3,614,581–3,614,586 before versus 3,815,905–3,815,933 after; medians regress
5.5703%. All six executions validate every scripted response and return success.
This additional product control supports the rejection independently of live
socket read grouping. It is not a new implementation candidate.

The private restructuring increases the targeted loop's work. Changes in
request-buffer handling, parsing, callbacks or capability dispatch are not
credited to it. Historical FAST-2 unresolved sample shares (29.33% hello,
26.15% keep, with lost hello samples) remain unresolved historical evidence;
this experiment neither redistributes them nor repeats broad sampling.

Separate allocation builds use the same compiler and matching measurement
archive. One successful diagnostic execution per participant gives:

| Participant | Allocations = frees/process | Copied bytes | Count decrements | Peak managed live bytes | Process max RSS, KiB |
| --- | ---: | ---: | ---: | ---: | ---: |
| before hello | 5,120 | 16,384 | 84,993 | 272 | 6,656 |
| candidate hello | 5,120 | 16,384 | 95,233 | 272 | 6,656 |
| before keep | 3,851 | 16 | 68,019 | 640 | 6,656 |
| candidate keep | 3,828 | 16 | 78,336 | 944 | 6,912 |

These single diagnostic observations are not ten-round allocation estimates or
production timing/counter results. Keep allocation variation reflects input
coalescing, not demonstrated serializer savings. Managed live bytes return to
zero; reported region and region-pool high waters are zero, and arena high water
is explicitly unavailable. RSS is a process high water, not logical byte length
or reserved backing capacity. The mixed serializer's largest initialized prefix
is 65,615 bytes; later bodyless replies expose only `used`. Backing capacity was
not independently instrumented. No memory/performance cap was raised.

## Correctness and preserved attempts

Before: 24 focused HTTP/1/wire Rust tests pass. Candidate: the same 24 pass;
the new boundary/alias test agrees under ordinary and checked interpretation,
scripted replay, H, LLVM and Cranelift. It exercises empty/small/reused bodies
and headers, lengths 65535/65536/65537, HEAD, 204 and 304 while retaining input
aliases. The final Rust fixture grows each connection once at each boundary, then
reuses it for shorter replies. Repeated full large-body overwrite remains in the
compiled/socket experiment. Its fixed Rust-test budget avoids changing the corpus
budget.

The new inline-error fixture rejects nine exchanges: low/high status, malformed
header name/value, all three reserved framing headers, and validation precedence
for 204/304. The replay oracle additionally requires zero writes, nine closes
and no shutdown. Existing output-growth, serial handoff/cancellation and streamed
error controls pass three native smoke selections (33 image runs under four
configurations, real IO/replay and Cranelift) and three explicit parity selections.
The new error fixture additionally passes corpus execution/formatting, H fault
injection, native smoke and LLVM/Cranelift parity. No HTTP/2 campaign is rerun.

Independent physical sockets compare all bytes of 1024 requests for before and
candidate hello/keep plus the unchanged owned control (5120 requests), and both
mixed sequences (2048 requests): 7168 exact responses, all exits zero.

Retained setup failures include a zero-test CLI invocation corrected with test
registration, an unmatched Rust format brace, a captured-local ownership error
corrected with owned copies, and a missing corpus rule marker. The temporary
registration reorder was corrected to one exact added path. Early concurrent
builders shared an output directory; affected owned/sequence builds were repeated
in separate directories and matched their saved hashes. One allocation client
collided with a physical client; its failed cell is retained and the exact keep
control was repeated after serializing fixed-port consumers. No such attempt is
counted as a semantic or performance pass. A final combined driver action
timed out at its existing 600-second limit without an assertion result. The
frozen replay-only test passes in 12.92 seconds. A bounded, owned debugger
capture of the original boundary-only test identifies checked-interpreter
`Frame::displaced_shadow`/`shadow_of` and shadow destruction, before H execution.
The fixture was corrected to grow separate connections before shorter reuse;
the two exact tests pass separately in 26.38 and 23.22 seconds without
increasing limits. The original
600-second remote timeout, harness/image hashes and debugger stack are preserved.

Both CPU31 PMU producers were canceled before taking a sample. Their zero-sample
checkpoints and cancellation statuses remain available. The second already
preserved raw cells/checkpoints and required 100% running counters, but no PMU
cell executed. This is not a successful ten-round PMU session.

## Separate proposal and follow-up

The next seam is a **specified checked replacement of an initialized byte range**,
for example `replace_range(mut List[u8], start, Bytes)`: check start/length and
initialized bounds, preserve length/capacity, handle an aliased immutable source,
perform ordinary COW/arena accounting, and specify failure/cleanup ordering on
all executors. This proposal adds no operation here and promises no numerical
result. `bytes.append` cannot overwrite the retained prefix; appending after a
shorter response would retain stale bytes. A fresh builder changes reservation
and allocation behavior, and a scalar pop loop supplies no cheap truncate.

The smallest remaining product trigger is `bench/micro/http/server.hir`: 1024
persistent `GET / HTTP/1.1\r\nHost: localhost\r\n\r\n` requests, returning
status 200, Content-Type text/plain and `Hello, world`. Its serializer still
performs checked byte-at-a-time prefix replacement. A future primitive proposal
and a fresh CPU31/CPU7 native instruction/cycle and wall-clock session remain
separate work. Closing this no-go does not waive the performance target.

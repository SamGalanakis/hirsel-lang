# Frozen service and compiler scoreboard — 2026-10-22

**INCOMPLETE / NO-GO overall.** Recorded list exceeded the unchanged 4 GiB cap in round 7; seven valid rounds are retained without an accepted recorded-list ratio. Restart misses the historical 1.5× instruction goal; cold is also a material startup residual. Wall-time and tail-latency acceptance remain unmeasured because CPU31’s timing lock belonged to LangArena. No production change or performance/adoption gate is claimed.

All tables use compiler/runtime/std/app main `ab44bbb731be91d79289eb3df88aeb139e3dfb75`. Rust is the source and locked dependencies at that same revision, built with `rustc 1.99.0 (b940084d7 2026-09-28)`. The release bundle is the developer release profile, without PGO distribution tuning. This is a fresh snapshot, not a combination of earlier ratios.

Compiler SHA-256 `f868f49a58acad45db370946169d8473fa7ea0c3e864a1e6b1e857a2a434341a`; std image `ca49370f7cfe959fa6a3fa76ef221f454f2a2243728512da7e922b976cf731f8`. Complete identities and streaming source/native-artifact manifests are retained in the evidence directory described below.

Servers use CPU31; the existing open-loop Rust client uses CPU7. Rust uses Tokio multi_thread with one worker. Ten seeded interleaved rounds (`20261022`), 10,000 tasks except empty cold, 16 requests/s for three seconds, SSE 1/100. The client consumes complete responses; latency starts at scheduled arrival. Both participants use files/volatile durability. Each sample starts a fresh process and must stop with exit 0. Default Hirsel recording is a participant using exactly the plain binary. Service and fresh compiler commands retain the existing 4 GiB capped budget; resident budget limitations are called out separately.

The live host rule allowed instruction-only measurement while the timing lock was occupied; no timing locks were held. Total and user instructions are reported separately. Cycles, task-clock and latency remain raw observations, with no accepted timing ratios. Cold/restart counters include startup, first selective GET, validation and shutdown plus descendants. Operation counters bracket the established client workload after startup/subscription setup and include server worker threads; they exclude the client and initial setup. Total service lifecycle work for these operation cells is not measured. RSS is a resident snapshot after the selective warmup and after load, not peak RSS or an additive memory breakdown.

## Service instructions

Ratios are ratios of medians, normalized by completed requests. Cold/restart are per process; SSE is per mutation, not per subscriber. Each plain/Rust group has ten rounds. Recorded list’s seven-round values appear only in the raw samples/observations.

| Cell | Plain / Rust total I | Plain / Rust user I | Recorded / plain total I | Recorded / plain user I |
|---|---:|---:|---:|---:|
| cold/0 | 2.443425 | 2.114133 | 2.118000 | 3.170561 |
| restart/0 | 2.415355 | 2.424182 | 1.256022 | 1.080073 |
| create/16 | 1.082748 | 0.941033 | 1.065359 | 1.010077 |
| list/16 | 0.823088 | 0.591156 | INCOMPLETE (7/10) | INCOMPLETE (7/10) |
| search/16 | 1.151309 | 1.117345 | 1.002916 | 1.000598 |
| update/16 | 1.083697 | 0.940780 | 1.064028 | 1.010061 |
| sse/1 | 0.961291 | 0.979999 | 1.047411 | 1.007307 |
| sse/100 | 1.108183 | 1.107174 | 1.003306 | 1.000872 |

## Service observations

Latency below is descriptive under shared CPU contention; it cannot establish a tail-latency or capacity gate. Completed counts are sums across valid samples. I is median million instructions per process/request/mutation as defined above. Cold/restart latency is first response, and their completed count is not a load-generator count. Incomplete recorded list has seven valid samples, excluding the failed cell entirely.

| Cell | Participant (n) | Completed | Total / user I (M) | p99 / p99.9 / max (ms), or first response | Idle / load RSS (MiB) |
|---|---|---:|---:|---:|---:|
| cold/0 | hirsel (10) | — | 18.969 / 2.756 | first 5.927 | 8.43 / 8.43 |
| cold/0 | hirsel-recorded (10) | — | 40.176 / 8.738 | first 10.135 | 8.58 / 8.58 |
| cold/0 | rust-one-worker (10) | — | 7.763 / 1.304 | first 3.010 | 3.88 / 3.88 |
| restart/0 | hirsel (10) | — | 233.131 / 200.643 | first 26.190 | 12.79 / 12.79 |
| restart/0 | hirsel-recorded (10) | — | 292.818 / 216.709 | first 39.411 | 14.67 / 14.67 |
| restart/0 | rust-one-worker (10) | — | 96.520 / 82.767 | first 16.277 | 7.23 / 7.23 |
| create/16 | hirsel (10) | 480 | 50.289 / 39.390 | 18.027 / 18.027 / 18.027 | 12.79 / 16.91 |
| create/16 | hirsel-recorded (10) | 480 | 53.576 / 39.787 | 17.863 / 17.863 / 17.863 | 14.65 / 18.90 |
| create/16 | rust-one-worker (10) | 480 | 46.446 / 41.858 | 12.275 / 12.275 / 12.275 | 7.23 / 11.62 |
| list/16 | hirsel (10) | 480 | 186.269 / 126.433 | 69.471 / 69.471 / 69.471 | 12.76 / 15.48 |
| list/16 | hirsel-recorded (7) | 336 | 311.630 / 136.597 | 1864.703 / 1864.703 / 1864.703 | 14.66 / 32.88 |
| list/16 | rust-one-worker (10) | 480 | 226.305 / 213.874 | 71.967 / 71.967 / 71.967 | 7.22 / 39.42 |
| search/16 | hirsel (10) | 480 | 18.594 / 17.911 | 10.243 / 10.243 / 10.243 | 12.77 / 13.40 |
| search/16 | hirsel-recorded (10) | 480 | 18.649 / 17.922 | 7.659 / 7.659 / 7.659 | 14.60 / 15.64 |
| search/16 | rust-one-worker (10) | 480 | 16.151 / 16.030 | 7.843 / 7.843 / 7.843 | 7.22 / 7.25 |
| update/16 | hirsel (10) | 480 | 50.239 / 39.301 | 17.075 / 17.075 / 17.075 | 12.75 / 16.41 |
| update/16 | hirsel-recorded (10) | 480 | 53.456 / 39.697 | 17.711 / 17.711 / 17.711 | 14.57 / 18.56 |
| update/16 | rust-one-worker (10) | 480 | 46.359 / 41.775 | 13.563 / 13.563 / 13.563 | 7.22 / 11.58 |
| sse/1 | hirsel (10) | 10 | 68.239 / 57.064 | 11.123 / 11.123 / 11.123 | 12.75 / 14.30 |
| sse/1 | hirsel-recorded (10) | 10 | 71.474 / 57.481 | 11.275 / 11.275 / 11.275 | 14.62 / 16.17 |
| sse/1 | rust-one-worker (10) | 10 | 70.987 / 58.228 | 10.699 / 10.699 / 10.699 | 7.23 / 11.22 |
| sse/100 | hirsel (10) | 1000 | 1846.506 / 1815.587 | 137.663 / 137.727 / 137.727 | 12.72 / 17.78 |
| sse/100 | hirsel-recorded (10) | 1000 | 1852.611 / 1817.171 | 127.455 / 127.519 / 127.519 | 14.66 / 19.65 |
| sse/100 | rust-one-worker (10) | 1000 | 1666.247 / 1639.839 | 124.063 / 125.247 / 125.247 | 7.22 / 13.35 |

## Compiler cold and unchanged

32 successful cells, 320 measured commands: hello/csvjson/tasks/standard generated syn10k × check/debug × Hirsel/Rust × cold/unchanged. Every cell has ten rounds; zero refused, timed-out or failed cells. Standard syn10k was executed successfully before measurement, not replaced by a rejection workload. All counters ran at 100%. Fresh CLI processes explicitly disable the daemon; unchanged retains disk definitions and objects. Cold removes only runner-owned caches and the Rust package’s artifacts/incremental state. The published std image, std objects and Rust dependency artifacts remain. Neither page cache nor host caches are flushed. This is command-level comparison: Rust unchanged can be Cargo’s no-op fast path. Hirsel debug is its default LLVM debug backend; Rust uses Cargo dev. One job/CPU31 on both sides.

| Input / mode / state | Hirsel total / user I (M) | Rust total / user I (M) | H/R total / user I | H/R peak RSS (MiB) |
|---|---:|---:|---:|---:|
| hello/check/cold | 783.784 / 691.636 | 479.212 / 256.918 | 1.635567 / 2.692045 | 116.5 / 70.2 |
| hello/check/warm | 477.976 / 405.624 | 415.586 / 236.834 | 1.150125 / 1.712694 | 95.2 / 30.0 |
| hello/debug/cold | 2546.354 / 2112.459 | 670.435 / 371.341 | 3.798063 / 5.688737 | 215.8 / 95.5 |
| hello/debug/warm | 2532.933 / 2102.975 | 415.905 / 237.185 | 6.090168 / 8.866386 | 208.5 / 30.0 |
| csvjson/check/cold | 1053.339 / 947.263 | 674.376 / 424.992 | 1.561945 / 2.228897 | 123.5 / 114.5 |
| csvjson/check/warm | 581.936 / 505.243 | 415.105 / 237.704 | 1.401901 / 2.125514 | 99.5 / 30.0 |
| csvjson/debug/cold | 17619.770 / 16893.717 | 2952.855 / 2481.735 | 5.967030 / 6.807220 | 278.7 / 170.0 |
| csvjson/debug/warm | 9769.831 / 9128.423 | 415.948 / 238.051 | 23.488106 / 38.346547 | 274.3 / 30.0 |
| tasks/check/cold | 6036.382 / 5762.334 | 2334.575 / 1780.532 | 2.585645 / 3.236298 | 190.7 / 188.3 |
| tasks/check/warm | 4680.368 / 4504.519 | 643.879 / 410.596 | 7.269019 / 10.970682 | 172.0 / 45.9 |
| tasks/debug/cold | 88005.633 / 85757.160 | 14054.813 / 12796.693 | 6.261601 / 6.701509 | 622.0 / 294.8 |
| tasks/debug/warm | 56168.405 / 54405.727 | 646.137 / 412.117 | 86.929552 / 132.015333 | 606.4 / 46.0 |
| syn10k/check/cold | 12743.408 / 12207.040 | 3569.774 / 3185.440 | 3.569808 / 3.832136 | 269.3 / 189.0 |
| syn10k/check/warm | 8382.041 / 8114.815 | 432.274 / 249.361 | 19.390559 / 32.542402 | 218.9 / 30.0 |
| syn10k/debug/cold | 224187.932 / 219866.662 | 19687.050 / 18688.712 | 11.387584 / 11.764677 | 926.8 / 291.9 |
| syn10k/debug/warm | 87380.741 / 84639.372 | 434.380 / 249.956 | 201.162105 / 338.617201 | 904.6 / 30.1 |

Compiler peak RSS comes from GNU time over the counted process tree; capped scope peaks are retained separately. A fresh CLI unchanged command is not a resident session or an edit benchmark. The historical compiler table used different compiler/cache inputs and is not a baseline for current time ratios.

## Named body edit (fresh CLI)

`refresh_body_probe`: append an uncalled private u64 function, seed check, then change only its result literal from 1u64 to round+2. Hirsel and Rust use equivalent probes; this deliberately measures invalidation of an uncalled body, not a reached production edit. Each measured command includes startup and uses CPU31, one job and the same 4 GiB limit. Rust retains incremental state; Hirsel retains definition stores. Ten seeded language-interleaved rounds per input, separate from cold/unchanged, except tasks Rust: its original zero-sample relocation error is preserved and only that unexecuted subgroup was completed separately. Its build.rs path points to the exact frozen Unicode tables; no comparator logic changed. Tasks body has no accepted interleaved comparison.

| Input | H/R samples | H total / user I (M) | R total / user I (M) | H/R total / user I |
|---|---:|---:|---:|---:|
| hello | 10/10 | 798.931 / 692.262 | 476.849 / 258.400 | 1.675439 / 2.679036 |
| csvjson | 10/10 | 901.161 / 789.489 | 542.227 / 302.822 | 1.661961 / 2.607109 |
| tasks | 10/10 | 4705.001 / 4513.410 | 1061.682 / 707.876 | separate subgroup; no accepted ratio |
| syn10k | 10/10 | 8408.135 / 8130.913 | 1281.866 / 935.121 | 6.559291 / 8.695043 |

## Resident check: tasks

The existing resident harness selects unchanged and `resident_probe` body edits (1u64 → round+2), CPU31 server/CPU7 client, ten interleaved rounds across resident/disk/cold. Every reported instruction total charges server plus client. Disk starts a fresh CLI loading stores; cold disables definition/object stores. Daemon startup and initial seeding are outside per-request counts. These are check requests; resident debug-build costs are unmeasured. No Rust speed ratio is formed from a different session.

Natural daemon exit status: verified exit 0 after idle 60 seconds. Combined daemon/client 4 GiB capped scope: MISSING — diagnostic only, budget acceptance INCOMPLETE; no rerun.

| Mode / edit | n | Total / user instructions (M), server + client |
|---|---:|---:|
| resident/unchanged | 10 | 51.762 / 26.018 |
| resident/body | 10 | 107.804 / 80.837 |
| disk/unchanged | 10 | 4687.275 / 4510.100 |
| disk/body | 10 | 4693.241 / 4511.984 |
| cold/unchanged | 10 | 5458.174 / 5266.455 |
| cold/body | 10 | 5457.923 / 5266.500 |

## Resident check: syn10k

The existing resident harness selects unchanged and `resident_probe` body edits (1u64 → round+2), CPU31 server/CPU7 client, ten interleaved rounds across resident/disk/cold. Every reported instruction total charges server plus client. Disk starts a fresh CLI loading stores; cold disables definition/object stores. Daemon startup and initial seeding are outside per-request counts. These are check requests; resident debug-build costs are unmeasured. No Rust speed ratio is formed from a different session.

Natural daemon exit status: verified exit 0 after idle 60 seconds. Combined daemon/client 4 GiB capped scope: verified.

| Mode / edit | n | Total / user instructions (M), server + client |
|---|---:|---:|
| resident/unchanged | 10 | 52.071 / 26.107 |
| resident/body | 10 | 116.909 / 89.739 |
| disk/unchanged | 10 | 8379.400 / 8119.186 |
| disk/body | 10 | 8391.247 / 8122.844 |
| cold/unchanged | 10 | 10992.040 / 10693.151 |
| cold/body | 10 | 10992.888 / 10694.992 |

## Separate allocation diagnostic

The unchanged heaptrack workload is startup + selective GET + one full list + 100 SSE subscriptions + one mutation + graceful stop. This is one diagnostic process per participant, not ten-round timing evidence or a per-operation allocation decomposition. Heaptrack counts libc allocation calls: Hirsel’s logical allocations inside managed slabs are absent, so these counts are not equivalent allocation units across languages. Recorded mode was omitted after its memory defect; no recorded-list retry was made.

- hirsel-recorded: not-run
- hirsel: ok
  calls to allocation functions: 3562 (7124/s)
  temporary memory allocations: 999 (1998/s)
  peak heap memory consumption: 6.76M
  peak RSS (including heaptrack overhead): 45.51M
  total memory leaked: 1.42K
- rust-one-worker: ok
  calls to allocation functions: 4480048 (2786099/s)
  temporary memory allocations: 4064499 (2527673/s)
  peak heap memory consumption: 63.99M
  peak RSS (including heaptrack overhead): 35.85M
  total memory leaked: 35.15K

## Failed cell and remaining triggers

1. **Recorded memory, NO-GO; owner `recorded-memory-8`.** Recreate `fixture(data, 10000)`, start the frozen tasks binary with default `--hirsel-record`, run the existing `load PORT list 16 3`, then close stdin. In round 7, capped reported 4096 MiB / 4 GiB, 17.2 s, exit 137. The failed tape is 2.2 GiB and retained with counters, status and hashes. This is a charged-memory failure; its split into live anonymous RSS versus charged file cache has not been measured. Seven accepted recorded-list rounds do not make a ten-round result. The failed cell’s client completion/latency JSON is unavailable because the existing sample function raises at server shutdown before returning it. No retry, cap increase or local fix.

2. **Compiler unchanged debug cost.** Minimal input: the accepted generated syn10k (no arguments, output `start\n`); repeat frozen `hirsel build --debug --timings=json MANIFEST -o PROGRAM` with daemon off and unchanged definition/object stores. The check/debug table retains the large unchanged gap versus Cargo’s no-op path. Compiler self-phase observations retain import/plan/verify/store lookup; these are candidate investigation boundaries, not instruction-attribution proof. The smaller tasks manifest is also sufficient to expose substantial repeated import work.

3. **Restart instruction goal, NO-GO; cold startup residual.** For restart, generate the 10k Board fixture, launch, drain `GET /?q=Task+9999`, close stdin. For cold, use the empty fixture and the same route/shutdown. Restart remains over the historical 1.5× Rust goal; cold is over 2× Rust in this snapshot. No earlier JSON or tape-fast ratio is substituted. The next reduced diagnostic is process-lifetime instructions split at readiness/first GET/close without changing the fixture or startup charge.

4. **RSS retention.** Start over the 10k fixture, perform the selective first GET, then `GET /` at 16/s for three seconds and idle. Inspect idle/load snapshots and a separate allocation profile. Cold plain idle RSS is 8.43 MiB versus 3.88 MiB (about 2.17×), above the historical 2× goal; seeded restart idle is 12.79 versus 7.23 MiB (about 1.77×). These are snapshots, not peak or total charged memory. The public table gives actual RSS rather than assigning old RSS savings to this compiler. libc retention, live board data and tape/cache charges require separate accounting; no allocator-policy change is made.

5. **Exclusive timing and full lifecycle accounting, INCOMPLETE.** Repeat precisely the frozen selection with both convention timing locks available, acquired without holding one while waiting for another (maximum wait ten minutes), for accepted wall/tail comparisons. Operation counters currently omit startup/key setup/subscription setup and final teardown, which cold/restart cover separately. Resident check excludes daemon launch; resident debug and reached-body edit comparisons remain absent. The tasks resident diagnostic also needs its omitted combined 4 GiB scope before budget acceptance; syn10k used the correct scope. Tasks fresh-body Rust completed in a separate subgroup, so no accepted interleaved ratio is published for that row. No multicore or full acceptance matrix was run.

## Correctness and evidence

Before measurement: 2 × 23 differential HTML/Unicode, corrupt/save-failure, SIGTERM and storage checks; 3 × 117 original HTTP/SSE/restart/snapshot checks across plain, recorded and Rust one-worker (397 checks total). All passed, including graceful stdin/SIGTERM handling. Native debug hello/csvjson/standard syn10k each ran once per language with identical stdout hashes and exit 0 (six executions). All 24 selected service preflight cells passed. These are executed checks, not compilation or empty filters. No harness or production code changed, so no harness unit tests were run and no before/after optimization test count is invented.

Raw JSON, hashes and exact command records: [evidence index](app-perf-refresh-2/index.json). The complete private local archive is `/workspace/code/hirsel-briefs/app-perf-refresh-2/`, including all logs, original sample directories, failed tape, bundle/native hashes and source manifests. Public compressed raw JSON contains every valid service sample, failure record and compiler cell; gzip changes storage only. Ratios in this page are independently recomputed from those JSON values, with sample-count, exit-status and 100% counter-running checks. Run `python3 bench/apps/tasks/app-perf-refresh-2/verify.py` to verify the public archive without the original worktree. The archive includes exact selector drivers, raw counter audits and setup errors. The tasks Rust body relocation generated byte-identical Unicode16 tables. A shell orchestration edit failed after resident measurements and before allocations; only the unexecuted allocation stage was then run separately. Both setup failures remain in the local logs; no successful measured group was rerun.

Prior art is the unchanged repository measurement machinery at frozen main: `bench/apps/tasks/bench:125–212` (sample boundaries), `rust/src/bin/load.rs:1–139` (scheduled arrivals/HdrHistogram and complete SSE), `support.py:57–104` (one-worker launch and graceful stop), `bench/micro/measure.py:17–44,79–89,125–140` (perf parsing/counters and seeded ordering), and `tools/compiler-bench/run:284–403` (isolated cold/unchanged caches). Its upstream rustc-perf and rustc cache references remain documented in [compiler README](../../../tools/compiler-bench/README.md). Session-only selectors omit SSE1000/stress/release/native, bypass timing locks under the live fallback rule, add raw exit/counter auditing, and let resident daemons idle out rather than counting forced termination as success. No new protocol, algorithm or library surface was implemented.

Historical [tasks results](RESULTS.md), [performance narrative](PERFORMANCE.md), [compiler results](../../../tools/compiler-bench/RESULTS.md), tape-fast/text-build/JSON/RSS sessions keep their dates and original inputs. None is silently overwritten or multiplied into this scoreboard.

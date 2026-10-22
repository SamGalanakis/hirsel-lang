# Cell 7: bounded recording memory

Only `hirsel-recorded-list-16`, round 7, was rerun. The frozen campaign used
`ab44bbb731be91d79289eb3df88aeb139e3dfb75`. The fixed runtime is based on
`090cf460`; the fixture, frozen compiler, standard library and load client are
unchanged. The server runs on CPU 31, client on CPU 7: 10,000 seeded tasks,
one initial search, then 48 open-loop list requests at 16 arrivals/second for
three seconds. `HIRSEL_MEMORY_LIMIT=4G`, swap disabled, throughout.

| Measurement | Before | Fixed |
| --- | ---: | ---: |
| Original campaign scope peak | 4096 MiB, killed at 17.2 s | — |
| Isolated scope peak | 4096 MiB, exit 137 | 1431 MiB, exit 0 |
| Separate process-RSS baseline / fixed peak | 2428.4 MiB | 252.0 MiB |
| Completed requests / errors in successful samples | 48 / 0 | 48 / 0 |
| Tape bytes in successful samples | 839,982,156 | 1,358,911,266 |
| Replay | exit 0, identical stdout and observation | exit 0, identical stdout and observation |

The scope includes file cache; it is not process RSS. The separate successful
baseline's scope peak was 3258 MiB. Process high-water RSS comes from
`/proc/PID/status` (`VmHWM`), sampled through shutdown. Tape sizes vary with
socket partial writes and readiness events; these are exact recorded inputs.
No wall-time ratio is claimed: this session held no exclusive timing lock.

Heaptrack pinpoints `TimingJournal::save`, rather than the streaming tape writer
or retained Hirsel lists. For a 957,162,009-byte tape, it measured a 2.91 GB heap
peak and 2.90 GB RSS peak (decimal, profiler overhead included). Three tape-sized
allocations coexisted: `std::fs::read` (957.16 MB), the parser's tape copy
(957.16 MB), and decoded event payloads (957.13 MB). The observer decoded every
payload merely to count events. The frame writer's reusable buffer was 33.76 MB.

Timing samples now stream through `BufWriter<File>`. The observer keeps only
scalar offsets/counts, remembers the last successful durability barrier, and
truncates unconfirmed samples at teardown. It hashes the persisted tape through
a 64 KiB buffer, patches its fixed-width digest and completes the existing v1
JSON. It never reloads or decodes event payloads. Journal failures leave timing
incomplete; diagnostic failures cannot change application results or barriers.
Tape bytes, replay identity and terminal controls are unchanged.

The fixed tape's SHA-256 is
`699cc03cbd4cc962c53520442f001d0fca644728d2a60ba6b8081057cf76f88f`.
The sidecar digest matches. Replay consumes the full tape and checks every
request, including response write bytes, against its recorded request; stdout
and the terminal observation are also compared byte for byte. Replay's scope
peak was 1558 MiB, exit 0.

The regression spawns fresh processes and measures `getrusage(RUSAGE_SELF)`
peak RSS, covering both payload accumulation and retained timestamp growth:

| Request payload | N | Fixed peak RSS |
| --- | ---: | ---: |
| 1 MiB | 16 | 9724 KiB |
| 1 MiB | 256 | 9384 KiB |
| empty | 1024 | 7912 KiB |
| empty | 2,097,152 | 7912 KiB |

Before the fix, the same 1 MiB regression failed: 58,364 KiB at N=16 versus
795,580 KiB at N=256. The assertion permits at most 8 MiB growth. Timing tests
expand from two to six, including ordinary/durable timing-on/off tape byte
invariance, interrupted durable prefixes, JSON counts/digests, diagnostic write
failure, and short/interrupted hash reads.

The hash read loop follows Rust `1a2c4797ef25`,
`library/alloc/src/io/copy/generic.rs:232-254`; test vectors adapt
`library/alloctests/tests/io/copy.rs:6-14,16-30`. Differences: SHA-256 consumes
each chunk instead of a writer, and stable `Read` uses an initialized 64 KiB
buffer instead of `read_buf`. Hashing itself uses the existing `sha2` dependency.

Focused Buck2 validation passed 34 distinct cases: six observer tests, five
journal tests, and 23 recording/recorder-overlap integration tests. The stronger
N-scaling case was rerun after increasing its small-event count to 2,097,152.
The full benchmark campaign was not rerun.

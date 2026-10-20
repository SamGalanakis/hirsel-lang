# Tasks application comparison

`./bench/apps/tasks/bench` builds native LLVM Hirsel and the locked Rust release
project, then runs one seeded, interleaved session on the CPU31 timing lock.
Servers use CPU31, clients CPU7; CPU15 busy share is sampled around each cell.
Requires the repository Buck2 setup, `perf`, Linux `/proc`, Python 3 and Cargo.
No system packages or load tools are installed. The load generator is the
`load` binary in the same pinned Cargo project (reqwest + HdrHistogram).

Defaults: ten rounds, three-second open-loop cells at 1, 4, 16, 64, 256 and 1024 requests/s,
10,000 persisted tasks, three participants (Hirsel, Rust current_thread, Rust
multi_thread with **one** worker), plus 1/100/1000-client SSE cells. Each
participant’s sweep stops at its first capacity knee. Rates can be
selected with `--rates 1,4,16,64`; cells that cannot keep up retain their corrected
latency and completion rate. The knee is the first rate whose achieved rate is
below 90% of offered rate. If no knee is reached the report says so; extend the
sweep before drawing a capacity conclusion. `--rounds` cannot be below ten.
`--skip-build` reuses existing binaries; it is only for iterative harness checks.
`--only` selects comma-separated operations for a focused run. A partial run is
labelled partial in its generated report. All raw results are saved before
rendering `RESULTS.md`. Never compare wall times from separate invocations.

The server is a fresh capped process for every sample. A native pre-exec marker
excludes perf startup from first-response latency. Cold-start cells use an
empty board; restart and operation cells start with the same 10k snapshot.
Open-loop latency includes queueing from the scheduled arrival (no coordinated
omission). Each response body is fully consumed, keeping HTTP connections alive.
SSE clients use a selective search over the 10k board, consume their initial
patch, and await one complete patch each after a status mutation. Their histogram
measures mutation-to-patch arrival per client; CPU/request is per mutation, not
per subscriber. RSS is sampled at readiness and after the measured load (the
latter is a resident snapshot, not a high-water mark).

Each cell has server `perf stat` task-clock, cycles, instructions and user-only
cycles/instructions. Operation counters attach with `-p` and explicit enable/disable
acknowledgements, excluding startup and initial subscriptions. Cold/restart
counters cover process lifetime through shutdown. The instruction ratio is the
verdict. Ratios, IPC, cycles and task-clock have paired bootstrap confidence
intervals; Rust baseline variation is reported. Binary size is on-disk ELF bytes,
including the matching release profiles' debug sections. File durability is
volatile (the common cheap default); every acknowledged mutation has atomically
published a compact snapshot and keeps the last two revisions.

`./bench/apps/tasks/e2e --kind rust` drives the original Hirsel HTTP e2e suite
(first, watch/shutdown, second/restart) against Rust. `--kind rust-one-worker`
checks the one-worker scheduler; `--kind hirsel` checks the benchmarked Hirsel
binary. The original fixture controls clocks and reads persisted revisions.

`./bench/apps/tasks/verify` adds byte-for-byte differential HTML/Unicode checks,
corrupt-snapshot recovery, save failure and SIGTERM with stdin open.
`./bench/apps/tasks/bench --report-only` regenerates the Markdown from raw samples.
`--heap-only` appends heap evidence to an earlier session without new timings.
CPU and heap profiles are separate diagnostic runs, not comparison samples.

The saved session reports a 4 GiB cap failure while Hirsel opens 1000 SSE clients.
It does not retry that cell or increase the cap; the row stays unavailable and the
other cells continue. The exact memory-limit evidence is retained in the report.
Allocation profiles require the already installed `heaptrack` tools; no package
installation is attempted. `/proc` resident mapping snapshots accompany them.

Measurement builds use the repository's pinned Rust 1.99.0 and LLVM 21.1.8.
The initial seed is written directly as the common on-disk snapshot, rather than
spending measurement time creating 10,000 tasks through HTTP. A create cell grows
that board; an update cell repeatedly edits task 1, and every successful edit
bumps the revision and saves even when the submitted fields match their old values.

`attribution.json` records the reviewed profile interpretations for the measured
binary hashes. The generator applies them only to matching binaries; a new
binary requires fresh profile review. Full profiles remain in `results.json`,
with compact excerpts in `RESULTS.md`. `--startup-profiles-only` refreshes separate
startup profiles and never changes the ten-round measurement samples.

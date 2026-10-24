# Landing a lane

Run the tests your change can break, commit it, then run:

```sh
tools/land > /tmp/<lane>-land.log 2>&1; echo EXIT $? >> /tmp/<lane>-land.log
```

Land first joins a FIFO ticket queue shared by every worktree under
`$(git rev-parse --git-common-dir)/land-queue/`. Only its head fetches,
rebases, checks and pushes, and it lands the waiting tickets with its own
change as one batch (see [Batches](#batches)). A short `flock` protects an atomic sequence
counter and atomic JSON ticket publication; increasing numeric ticket
filenames define arrival order. A ticket keeps its place across rejected
push retries and re-execution into an updated `tools/land`. Exiting releases
it, including red checks and rebase conflicts. No speculative builds run.
Waiting lanes print their position and the head's lane name when either
changes, and at least every 30 seconds.

Each ticket records its lane, worktree, PID, wall-clock start time, Linux
process start ticks, boot ID and heartbeat. A background thread renews the
heartbeat every 10 seconds, including during blocking checks; waiting lanes
renew too, so a long queue does not expire them. Re-execution preserves the
PID and ticket and restarts the heartbeat thread. The next waiter removes
and logs a head whose process is dead (including zombies, reboot or PID
reuse), or whose heartbeat is older than 15 minutes. Process identity uses
`/proc/<pid>/stat` and boot ID, never command-line matching. It does not
signal or kill another lane. A resumed holder verifies ticket ownership
before starting an attempt and immediately before any push; a revoked or
expired holder refuses to push.

The ordering follows the increasing-ticket/acquire-head/release structure
of Linux's [generic ticket lock](https://github.com/torvalds/linux/blob/7be4bc52fd9e0e55dc9a2e0232b89427d2c4b614/include/asm-generic/ticket_spinlock.h#L33),
lines 33–49 and 63–69. Files and a short OS lock replace in-memory atomics;
sleeping waiters replace spinning; process identity and heartbeats add
recovery for abandoned tickets. These adaptations serve independent
processes and survive a lane crash on this single Linux host.

Land fetches and rebases onto a captured `origin/main` commit, regenerates
stale build inventories, and checks the workspace build, Clippy and fmt.
The build covers workspace compilation, including test harness compilation;
it excludes test execution, campaigns, the two-host link check, std images
and native std precompilation/packaging. The latter are integration artifacts
covered by main-runs, and never replace the workspace build. At this
change, the build query selects 200 targets, including all 84 Rust test
harness build targets, `hirsel-cli-bin-hirsel` and `hirsel-rt`. Compared with
the old query on the same inventory, exactly `hirsel-std-objects` is removed;
packaging targets were already excluded through the std image.
Clippy and fmt retain their full first-attempt aggregates.
It pushes the exact checked commit SHA, after verifying that HEAD and the
worktree still match. A red check refuses the push. Source conflicts stop
and abort the rebase; resolve them yourself and run land again. Land does
not run tests or commit generated files. Main's scheduled run owns the API
index; the generated-file and independent-line-list rules still apply.

Every retry checks its rebased tree. There is no third-attempt exception,
no inherited lint pass and no attempt limit. Losing a push race waits a
random duration in `[0, min(30, 2**attempt)]` seconds, then fetches and
rebases again. Non-race push errors stop with the error. This is the full
jitter algorithm from
[aws-arch-backoff-simulator](https://github.com/aws-samples/aws-arch-backoff-simulator/blob/master/src/backoff_simulator.py#L40),
lines 24–25 and 40–43, with a one-second base and a 30-second cap. The
exponent is bounded before calculation because the number of retries is
unbounded. The lane retains its FIFO place during this wait; the short metadata
lock is released. External writers that do not use the queue can still
cause a rejected push.

## Batches

A ticket records the commit it lands (`head`). When the head has rebased
its own change, it claims up to 15 live tickets behind it, oldest first, so
a batch holds at most 16. It replays each ticket's commits after the
captured `origin/main`, in ticket order, onto the tree of the tickets ahead
of it (`git rebase --onto <batch> <main> <head>`), detached in the head's
own worktree. It reads other lanes' commits by SHA from the shared object
store and never touches their worktrees or branches. The checks (hot files,
regenerated build files, build, Clippy and fmt) run once on the combined
tree, and one push carries every commit. Each lane's commits stay separate,
with their authors; nothing is squashed. Each landed ticket gets its own row
in `hirsel-landings.jsonl`.

The head writes each waiting ticket's outcome into its ticket file. The
waiting `tools/land` prints it and exits with its code: 0 with its own
landed SHAs, after which its branch points at its last landed commit; 1
with its conflict or its first error. Commits made after queueing do not
land; land says so, and you run it again for them.

A ticket whose commits do not replay leaves the batch with the conflict
message a single land prints; the rest stay on the tree before it. A
conflict with origin/main is final at once. A conflict with batched changes
ahead of the ticket stands only if every one of them lands; otherwise the
ticket stays queued for the next head. A change that touches generated
files is refused at once. A red batch
splits into halves, each checked on the green tree before it. A half that
is already known red is split without another check. A ticket is blamed
only when a check of its change alone, on a green tree, is red; the culprit
exits with that check's first error and the rest land. A red that does not
repeat blames nobody. A batch of one is a single land, as before.

Every pushed tree is one this attempt checked green. Before the push, the
head confirms that it still owns the queue head and that every batched
ticket is still held by its live process; a dropped ticket's commits never
land, and the head rebuilds the batch without it. A lost push race retries
the whole batch with the usual jitter. A dead or expired waiting ticket is
dropped from the batch and removed. If the head dies, its waiting tickets
keep their places, and the next head batches them. A revoked head neither
pushes nor delivers outcomes. If the head's own change is a culprit, it
still pushes the green rest; after a lost race it exits instead, and the
next head lands the rest.

A ticket queued by an older `tools/land` has no `head`. The batch takes the
tip of the branch that its worktree has checked out, as `git worktree list`
reports it. That waiting process never reads its outcome. When it reaches
the head, its rebase drops the commits that already landed, and it exits 0
with "nothing to land".

The design follows four merge trains:

- [bors-ng](https://github.com/bors-ng/bors-ng/blob/ca725797e53a88e954998de0bbb14a8a5acb13ab/lib/worker/batcher.ex#L10)
  builds every waiting change as one batch and bisects a red batch into
  halves (`lib/worker/batcher.ex` lines 10–22, `lib/worker/batcher/divider.ex`
  lines 9–27 and 89–94). Its `max_batch_size` is the cap.
- [Zuul's dependent pipeline](https://opendev.org/zuul/zuul/src/commit/3daef6c4f92672347ba9a2dcabef4415e35f7ac1/zuul/manager/__init__.py#L1955)
  tests each change on the changes ahead of it. A failing change leaves,
  and the rest move onto the nearest non-failing item (lines 1955–1972).
- [GitHub's merge queue](https://docs.github.com/en/repositories/configuring-branches-and-merges-in-your-repository/configuring-pull-request-merges/managing-a-merge-queue)
  groups up to a maximum number of pull requests per merge group, and
  removes only the failing one.
- [LUCI CV](https://chromium.googlesource.com/infra/luci/luci-go/+/f46b764fe10a5595c02d8779d520bbfec9705fe4/cv/internal/tryjob/execute/retry.go#43),
  the Chromium CQ, gives each verdict to one change, and retries failed
  tryjobs before it blames that change (lines 43–60).

Unlike bors-ng, land never blames by elimination, and it does not queue a
separate batch for each half: one process settles the whole batch, so the
lanes behind it keep waiting.

## When a retry can check fewer crates

The first attempt always checks everything, and so does every bisection
check. A subsequent attempt can reuse
an unaffected part of this process's preceding successful check only if:

- The previous base is an ancestor of the new base.
- All incoming paths are existing regular `.rs` files under
  `crates/<crate>/src/`, with a crate manifest present. Added, removed,
  renamed or executable files, manifests, build scripts, documentation,
  tool/configuration changes and other inputs require full checks.
- Incoming paths and the lane's own paths are disjoint. Every lane path has
  exactly the checked blob and mode after rebase. The entire difference
  between checked and rebased heads is exactly the incoming paths, and
  each incoming path exactly matches the new base. A clean rebase by
  itself is insufficient evidence.
- After regeneration, every generated build inventory has the same bytes
  as at the preceding check. This keeps the dependency and source-input
  graph fixed, including compile-time includes. A changed or missing
  inventory requires full checks.

With those proofs, Buck2 selects all targets in incoming crates, all
owners of the incoming sources (including cross-crate compile-time
includes), and their transitive reverse dependencies. Each of the build,
Clippy and fmt aggregates is intersected with that closure. Only the
complement's prior pass is reused: its sources, graph and transitive
inputs are unchanged. The lane's own crate is checked again whenever it
imports a changed crate. Unknown paths, failed comparisons, unavailable
graphs and empty selections never produce a green unchecked push; land
uses full checks or refuses the push if checking fails. The proof is kept
only in process memory. Re-executing a changed tools/land starts with full
checks.

Unchanged actions retain their usual Buck2 action keys and remote cache;
land does not change cache or pool configuration. Stage durations are
printed to millisecond precision, and `tools/buck2/event_logs.py` retains
the build and Clippy event logs. These logs distinguish cache reuse from
new execution. Workflow latency depends on pool contention; it is not a
compiler benchmark.

## Retry measurements

Observed workflow latency on the shared pool, 2026-10-22:

| Check | Build | Clippy | Fmt | Aggregate |
| --- | ---: | ---: | ---: | ---: |
| Historical first attempt, checked-shadow-cost-7 | 1,918 s | 24 s | 11 s | — |
| Historical lost-race retry | 2,531 s | skipped | skipped | — |
| Final full aggregate, std objects excluded | 0.696 s | 0.658 s | 8.206 s | 9.912 s |
| Immediate unchanged retry | 0.300 s | 0.454 s | 7.937 s | 9.064 s |

The historical retry recorded 167 cache hits and 66 misses (71.67% of
remote action records). Its longest action was native std precompilation,
2,358.411 seconds; the 63 Rust misses totaled 1,194.950 execution seconds
across parallel actions, with a maximum of 93.666 seconds. Clippy was not
the long pole. The first build in a fresh worktree in this session recorded
1,598 remote hits and zero misses (100%); its first Clippy pass recorded
278 hits and 51 misses. Subsequent unchanged checks executed no remote
actions: Buck2 reused its existing graph/results, so a remote hit percentage
is inapplicable to those checks. This supports cross-worktree cache reuse;
it does not suggest that unrelated worktree paths invalidate all actions.

Ten rounds interleaved full and representative incoming-crate checks in a
seeded order (595675). Medians were 8.898 seconds full and 3.783 seconds
scoped, a 2.35× latency ratio within that session. The representative scope
was `hirsel-object-storage`, its source owners and reverse dependencies.
Both checks used warm unchanged action results; the full variant still
included the cached std-objects target, before its explicit exclusion.
The final aggregate above measures the approved exclusion separately.
These are operational retry latencies, without an exclusive CPU lock;
there is no compiler instruction-count or CPU-performance claim, and the
historical and current runs are not a controlled before/after speed ratio.

## Regression test

```sh
python3 tools/buck2/sync.py --if-stale
.buck2/bin/buck2 test -c hirsel.tests=host //:test-land
```

The host setting supplies Git, which pool workers need not have.
The test uses a bare local Git remote, real rebases and pushes, and fake
build tools. Three concurrent worktrees queue behind a deliberately slow
first check.
The remote's receive hook requires build, Clippy and fmt records for the
exact proposed SHA and base; all three land in ticket order, the first
alone and the other two as one batch. Killed
and expired live holders are reaped; a resumed holder refuses to push
after losing its ticket. Re-execution retains the ticket and PID identity.
Batch cases: three clean changes
land with one check run; one red change in a batch of four is isolated in
five checks, and the other three land; a conflict in the middle ticket
leaves only that ticket; a conflict with a red ticket ahead stays queued
and lands next; a dead lane's ticket is dropped; a batch that
loses a push race is checked again and lands whole; and a ticket queued
by an older land is batched and later finds nothing to land.
Further cases cover
22 rejected attempts, a red retry in each aggregate, worktree/HEAD edits,
legacy lint-pass environment variables and conservative scope selection.
The slow historical retry included native std precompilation: one
`hirsel-std-objects` cache miss ran for 2,358 seconds of a 2,531-second build.
Excluding that integration artifact removes that cost from every retry,
including conservative full checks.

Only the test harness removes the randomized wait. Barrier waits and child
processes have bounded deadlines so a broken retry cannot hang the suite.

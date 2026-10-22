# Landing a lane

Run the tests your change can break, commit it, then run:

```sh
tools/land > /tmp/<lane>-land.log 2>&1; echo EXIT $? >> /tmp/<lane>-land.log
```

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
unbounded. No shared resource is held during the wait.

## When a retry can check fewer crates

The first attempt always checks everything. A subsequent attempt can reuse
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
build tools. Two concurrent lands rendezvous after checking. The remote's
receive hook requires build, Clippy and fmt records for the exact proposed
SHA and base; both lanes must eventually reach main. Further cases cover
22 rejected attempts, a red retry in each aggregate, worktree/HEAD edits,
legacy lint-pass environment variables and conservative scope selection.
The slow historical retry included native std precompilation: one
`hirsel-std-objects` cache miss ran for 2,358 seconds of a 2,531-second build.
Excluding that integration artifact removes that cost from every retry,
including conservative full checks.

Only the test harness removes the randomized wait. Barrier waits and child
processes have bounded deadlines so a broken retry cannot hang the suite.

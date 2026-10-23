# cronstate

A bounded UTC cron scheduler with durable admission records and joined children.
`check` validates a schedule, `next` prints each job's next occurrence in calendar
order, and `run` admits due occurrences until its UTC stop or admission count.

From the repository root, with a matching Hirsel CLI:

```sh
hirsel run examples/cronstate/hirsel.json --grants examples/cronstate/grants-check.json
hirsel run examples/cronstate/hirsel.json --grants examples/cronstate/grants-check.json -- next schedule.json 2026-10-23T12:00:00Z
mkdir -p examples/cronstate/state
until=$(date -u -d '+10 seconds' +%Y-%m-%dT%H:%M:%SZ)
hirsel run examples/cronstate/hirsel.json --grants examples/cronstate/grants-run.json -- run schedule.json "$until" 1
```

The example runs literal `/usr/bin/true` once for the current minute. Config is
read-only; state is a private writable directory. Templates resolve `fixtures`
and `state` relative to their own directory. Every executable requires an exact
process rule. Children receive an empty environment, start in `/`, and use no shell.

Build standalone with `hirsel build examples/cronstate/hirsel.json --backend llvm -o /tmp/cronstate`.
Run `/tmp/cronstate --hirsel-grants examples/cronstate/grants-run.json run schedule.json "$until" 1`.
Observer options (`--hirsel-record /absolute/outside-mounts/run.tape`) precede
`--hirsel-grants`; application arguments follow its operand. Select
`--backend cranelift` to build the other native backend.

Schedules have exactly `version`, `parallel`, and `jobs`. Each job has exactly
`id`, `cron`, `program`, `argv`, and `deadline_ms`; see fixtures/schedule.json.
IDs are unique ASCII letters/digits/underscore/hyphen, 1..64 bytes.
Five numeric fields are minute/hour/day-of-month/month/day-of-week (Sunday0).
Stars, comma lists, ascending ranges, positive steps and `N/step` are supported.
DOM/DOW use robfig's star-bit AND/OR rule, including stepped stars. Seconds,
month/day names, `?`, descriptors, timezones and DST are rejected. Search uses
month/day/hour/minute jumps with a five-calendar-year horizon, reusing std date
and timestamp operations. Dates are Gregorian UTC, years0000..9999. `next` is
strictly after the supplied time; scheduled occurrences have minute precision.

Limits are checked as application refusals: config64KiB, state128KiB, jobs16,
parallel children1..4, admissions1..32 per invocation, argv4096 bytes/job
including executable and terminating bytes, argv64 entries, cron256 bytes,
deadline1..3000ms, run-until at most24h ahead. Test sessions are at most15s.
Children's two output pipes are drained and discarded without accumulating output.
Deadlines have the provider's bounded termination grace; task cancellation kills
and reaps direct children. Descendant termination is not promised.

The first run starts with the current UTC minute eligible. Afterward the latest
scheduled minute per job is its admission cursor. Downtime up to24h coalesces
missed occurrences to the most recent per job and reports the skipped count.
A backward wall-clock sample never decreases high-water time or an admission
cursor. A larger gap writes a persistent refusal flag and requires explicit reset.
The same decoded configuration is bound in state: changing a job requires reset.

One invocation owns the entire state mount. It creates `owner.lock` exclusively;
there is no time-based stealing. A normal scope exit joins children before the
lease removes that directory. After a killed owner, first establish that it has
stopped, then run
`hirsel run examples/cronstate/hirsel.json --grants examples/cronstate/grants-run.json -- unlock`. This is an explicit operator assertion, not a
PID-liveness heuristic. A running owner must never be unlocked.

`hirsel run examples/cronstate/hirsel.json --grants examples/cronstate/grants-run.json -- reset schedule.json` acquires the lease and durably replaces state,
discarding admission history and clearing the catch-up refusal. It can deliberately
permit a previously executed occurrence to run again. Preserve the old private state
externally before reset if its outcome matters.

Admission stores `(job ID, Unix minute, admitted)` before spawning. Publication
syncs staged bytes and listing, renames within one directory, then syncs the published
file and directory chain. Completion is published only after the child is joined.
Restart converts any prior `admitted` row to `uncertain`, persists it, reports it,
and continues future due occurrences. Uncertain occurrences never rerun automatically.
A crash after admission but before spawn is conservatively uncertain. A failure
or crash after an external effect and before completion can lose that completion.
There is no transaction spanning disk and process launch and no exactly-once claim.
A failed write prevents new jobs, cancels/joins active children and leaves durable
admissions available for restart reconciliation. `pending.json` is staging only.

Output is JSON lines; next/admission order is time then configuration declaration
order. Completion order is the scheduler's order. Exit0 is success,1 child failure
or run-until cancellation,2 invalid input/state/horizon/reset-required,3 resource,
file or capability refusal. Language failures and tooling stops retain engine policy.
An already-past run-until time cancels before reads/admission and leaves state untouched.
A run-until stop reports `run_until`; durable unfinished admissions remain uncertain
until the next restart. See FRICTION.md for exact engine/replay dispositions.

Modules are explicit manifest entries: main owns CLI/capabilities/deadline, spec
owns cron/calendar, plan owns validation and generic stable scheduled ordering,
state owns leases/format/durability/recovery, run owns bounded children and joins,
report owns JSON summaries. Generic scheduled payloads are job indices for admission
and job names for `next`, with a shared stable-order implementation.

Only the focused harness is registered. Run:

```sh
python3 tools/buck2/prepare.py
python3 tools/buck2/sync.py --if-stale
.buck2/bin/buck2 test -c hirsel.tests=focus //:hirsel-cli-test-cli-shard-1 -- --test-arg=cron_state_30
```

External matrices, pinned oracle results and immutable tapes are outside the source
repository. Tests never wait for a real cron minute. The 16-job reference/checked-H
boundary retains its original10M Steps result as INCOMPLETE.

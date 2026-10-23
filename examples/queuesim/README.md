# queuesim

A bounded discrete-event service network. Run from the repository root:

```
hirsel run examples/queuesim/hirsel.json --grants examples/queuesim/grants-run.json
```

The real `main(Io, Args)` takes `INPUT JOBS [--interruptible]`. INPUT is a single
filename beneath the read-only `input` grant; JOBS is 1–4 independent scenario
workers. The report goes to stdout only after the entire batch succeeds.
Diagnostics go to stderr with status 2; missing output capabilities return 3.
The optional interrupt flag requires an explicitly granted signal source and
races the whole workflow against Interrupt, canceling and joining the loser.
Simulation itself uses no wall clock, timer, randomness or channel.

The version 1 JSON schema is illustrated by `fixtures/simpy-fifo.json`:

```
{"version":1,"replicas":1,"stations":[{"capacity":1}],
 "jobs":[{"arrival":0,"cancel":1000001,
          "route":[{"station":0,"duration":1}]}],"horizons":[]}
```

Job and station IDs are zero-based declaration indices. `cancel:1000001` means
no cancellation. Routes must be nonempty and finite. Duration is positive.
Cancellation removes a future or waiting job; service is non-preemptive and
cancellation during service is ignored. Each next route visit requests a new
slot. A release admits the oldest waiter before requesting its own next visit.

Events order by `(tick, priority, insertion ordinal)`: cancellation 0, release 1,
arrival 2. Equal initial events follow job declaration order. Canceled arrivals
remain in the event queue as no-ops. The worst possible event count includes
all arrivals, cancellations and route completions, even events beyond the
horizon or belonging to jobs canceled before arrival. It is checked before
scheduling any event. Release addition is checked before taking a resource slot.

An empty `horizons` drains the schedule. Otherwise each strictly increasing
horizon pauses **before** events at that tick, and the next resumes the same
owned state. This follows SimPy's numeric-until behavior. Empty schedules stop
at tick 0 unless an explicit horizon advances them. Reports contain every
job's phase, completed route visits, accumulated wait and completion/cancel tick
(1000001 if absent); stations report current users, queue length, exact user-tick
and queue-tick integrals. Current waits are included at checkpoints. Replicas
publish in declaration order regardless of completion order.

Bounds: 16 stations, 64 jobs, 512 total generated events, four replicas, four
workers, ticks at most 1000000, 16 horizons, 128KiB input, 64KiB output.
Capacity is 1–64. A batch that exceeds the report cap returns `output_bytes`
without publishing a prefix. File input uses the standard bounded reader;
JSON parsing and heap storage use shipped standard modules.

`model` validates typed input; `events` owns a generic event payload in the
standard heap; `resources` owns FIFO waiting membership and integral counters;
`simulate` applies scoped mutations, checks conservation after every event,
and runs independent replicas with immutable sharing and owned results;
`report` bounds deterministic publication. Every module is declared in the
manifest and reached by main.

The focused tests are `discrete_simulation_33` in the CLI harness. Archival tests
require `-c hirsel.tests=host` so the explicitly named evidence directory
`/workspace/code/hirsel-briefs/discrete-simulation-33/` is reachable. Oracle bytes
are frozen in the support directory; `oracle.py` is a separate tick-by-tick
model. `simpy_vectors.py` runs the pinned upstream FIFO/capacity vectors.
See FRICTION.md for exact qualification results and open obligations.

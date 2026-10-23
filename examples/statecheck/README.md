# statecheck

An explicit finite directed graph verifier. The actual `main(Io, Args)` accepts
`INPUT OUTPUT JOBS [--interruptible]`: single portable filenames beneath the
`input` read grant and `output` write grant, and 1–4 independent model jobs.
All reports are built and checked before atomic replacement of OUTPUT. The same
bytes go to stdout. Status 0 means analysis completed (including counterexamples),
2 means domain/resource/I/O refusal, and 3 means stdout/stderr is unavailable.
The optional flag races the workflow against a granted Interrupt; scope exit
cancels and joins the loser. There are no timers, channels or fairness assumptions.

Frozen schema, version 1 (all fields required, no unknown fields):

```json
{"version":1,"models":[{"id":"example","states":[{"id":"start"},{"id":"done"}],"edges":[{"source":0,"target":1,"label":"finish"}],"initial":0,"terminal":[1],"forbidden":[]}]}
```

State and model IDs and edge labels use 1–24 ASCII letters, digits, `_` or `-`.
State IDs and model IDs must be unique. Edges have unique `(source,target)` pairs;
parallel edges are refused even if labels differ. State references are declaration
indices. Terminal/forbidden lists are sets: duplicates and out-of-range members
are refused. The two sets may overlap. Edges and equal-length BFS ties follow
edge declaration order; model results publish in model declaration order.

Safety and deadlock inspect the original graph, including outgoing terminal
edges. Safety reports the first forbidden state in BFS order. Deadlock reports
the first reachable nonterminal with no outgoing edge. Both prefixes are shortest.
Termination uses a separate reachable graph with terminal states absorbing:
only reachable nonterminal SCCs count, and singleton SCCs require a self-loop.
A nonterminal deadlock is a finite maximal path; it violates the separate
deadlock property, not absence of infinite paths. No implicit stutter is added.
A lasso chooses the first cyclic state in absorbing BFS order, its first internal
outgoing edge, then a shortest internal return path to that state.

Report grammar (decimal indices and declaration edge ordinals):

```
model example
reachable 0 1
components 0 64
safety safe
deadlock clear
termination terminates
end
```

`reachable` lists original BFS order. `components` gives each state's minimum
SCC member, or sentinel 64 for a terminal or an absorbing-unreachable state.
Counterexamples replace verdicts with `safety unsafe INITIAL EDGE...`,
`deadlock stuck INITIAL EDGE...`, or
`termination loop INITIAL PREFIX_EDGE... | CYCLE_EDGE...`.
An empty prefix is legal; a cycle must contain an edge and close. The encoder
checks endpoints, forbidden/deadlock policy, absorption, closure and trace caps
against the input before any publication. Edge labels remain in the input so
indices unambiguously reconstruct labeled transitions.

Fixed limits: 1–64 states and 0–128 edges per model, 1–4 models, 128 total
edges per individual prefix or lasso, 32768 total input bytes, 16384 output
bytes and 1–4 jobs. Cap/one-over vectors are frozen in the focused support
fixtures before implementation. Oversized reads probe one byte beyond the cap.
JSON, collections, bounded I/O, task scopes and immutable sharing use std.

The six modules separate lifecycle, schema, generic visitors/BFS/owned parents,
iterative SCC/cycles, property/task orchestration and checked atomic reports.
See REFERENCES.md for pinned prior art and FRICTION.md for qualification.

The example grants resolve host paths relative to this grants file and write
`report.txt` next to it. Run from the repository root with:

```sh
hirsel run examples/statecheck/hirsel.json --grants examples/statecheck/grants-run.json
```

The focused CLI module has seven tests: five active and two explicitly ignored
physical-capacity obligations. Archival runs require `-c hirsel.tests=host` so
`/workspace/code/hirsel-briefs/state-model-checker-34/` is reachable. To select
cases, pass `-- --env STATECHECK_CASE=ID,...` to Buck's test executor; shell
environment variables are not forwarded. Full-main source, input, native tapes,
raw streams, actual tool bytes and per-cell receipts are retained there.

Original qualification: 84/92 interpreter cells and 92/92 native cells passed;
eight original interpreter cap cells and sixteen corresponding tape replay cells
stop at Steps1M. Five additional provider-error workflows passed on all four
engines. Raw stderr recovery and strict replay refusal controls pass on the
landed public fixture repair. Width 1/2/4 capacity remains deployment-blocked;
see FRICTION.md for exact cells, owners, resource limits and recovery history.

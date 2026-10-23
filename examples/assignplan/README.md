# Bounded assignment planner

Run `hirsel run examples/assignplan/hirsel.json --grants examples/assignplan/grants.json`.
Application arguments are `ROOT INPUT OUTPUT JOBS [--interruptible]`; jobs is 1–4.
The example grants select `data`, `batch.json`, `assignments.json`, and four jobs.
Free scheduling stays off. Each invocation loads a fresh immutable batch; an
invalid replacement input leaves the prior report intact.

Input is `{ "version": 1, "revision": 7, "manifests": [...] }`. Each manifest has
`resources` and `jobs` arrays of `{ "id": "name" }`, `allowed` pairs of zero-based
`{ "resource": 0, "job": 0 }`, and `disabled` resource ordinals. All fields are
required. Names are nonempty, at most 64 UTF8 bytes, and unique within their
partition. The same name may occur in the other partition. Duplicate pairs and
disabled ordinals are errors; disabled resources remain declared but unavailable.

Limits are 32 resources, 32 jobs, 128 eligibility pairs, 32 disabled ordinals,
four manifests, 32 KiB total input, and 16 KiB output. Counts and decoded name
sizes are checked before derived typed collections are allocated. The initial
std JSON DOM is input-byte bounded. Every job has equal priority; no weights.

The planner ports NetworkX Hopcroft–Karp layering and augmentation. It scans jobs
and edges in declaration order. Ties are deterministic, with no promise of a
lexicographically minimal result. Independent manifests use scoped `task.map_all`;
all requested results join before one atomic report replacement. No scheduler
runtime is added. Each report contains matching pairs, both inverse maps,
unmatched jobs, reached alternating sets, and a minimum vertex cover. The
separate verifier checks eligibility, inverse consistency, cardinality, exact
reachability and equal matching/cover sizes, proving maximum coverage.

Success prints `assignplan: N manifests` and exits 0. Domain, capability and
cancellation errors print `assignplan: ERROR` to stderr and exit 2. Failed
publication drops its atomic temporary. `--interruptible` selects the workflow
against a granted Interrupt and cancels/joins the losing member.

Focused tests live in `assignment_planner_34.rs`. Frozen fixtures include
NetworkX vectors, a greedy-defeating augmenting chain, exhaustive independent
oracles on graphs of at most 7+7 vertices, every cap/one-over, and full-main
failure/cancellation fixtures. Four additional public provider-fault workflows
check open, atomic creation, partial write and commit errors with preserved output. Qualification evidence and incomplete original
cells are retained outside the repository; see FRICTION.md for limitations.

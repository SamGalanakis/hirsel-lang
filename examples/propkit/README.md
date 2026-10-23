# Propkit

A private reusable property-testing example with a bounded CLI suite. All six source
modules are declared in `hirsel.json`; importing a module grants no host authority.

Create a grants file and run the shipped properties from the repository root:

```sh
printf '%s\n' '{"args":[],"io":{"stdout":"stdout.bytes@1","tasks":"tasks.cooperative@1","dirs":[]}}' > /tmp/propkit-grants.json
hirsel run examples/propkit/hirsel.json --grants /tmp/propkit-grants.json -- run reverse 0 8 4 64
hirsel run examples/propkit/hirsel.json --grants /tmp/propkit-grants.json -- run reverse-nonempty 0 8 4 64
hirsel run examples/propkit/hirsel.json --grants /tmp/propkit-grants.json -- run sort 0 8 4 64
hirsel run examples/propkit/hirsel.json --grants /tmp/propkit-grants.json -- run codec 1 8 4 64
hirsel run examples/propkit/hirsel.json --grants /tmp/propkit-grants.json -- run deliberate-faulty 0 8 4 64 > /tmp/propkit-failure.json
```

The deliberately faulty predicate claims integers are below 7. Seed 0 first fails
at index 3 with 11; shrinking retains category 7 and reaches 7. Zero fails with a
different category (99), so accepting it would be a framework bug. The last
command intentionally exits 1 and prints a complete replay record.

For replay, make two private directories and instantiate the grants template:

```sh
mkdir -p /tmp/propkit-records /tmp/propkit-output
cp /tmp/propkit-failure.json /tmp/propkit-records/failure.json
python3 - <<'PY'
import json
from pathlib import Path
value = json.loads(Path('examples/propkit/grants.template.json').read_text())
value['args'] = ['replay']
value['io']['dirs'][0]['host'] = '/tmp/propkit-records'
value['io']['dirs'][1]['host'] = '/tmp/propkit-output'
Path('/tmp/propkit-replay-grants.json').write_text(json.dumps(value))
PY
hirsel run examples/propkit/hirsel.json --grants /tmp/propkit-replay-grants.json
hirsel build examples/propkit/hirsel.json --backend llvm -o /tmp/propkit-llvm
/tmp/propkit-llvm --hirsel-record /tmp/propkit.tape --hirsel-grants /tmp/propkit-replay-grants.json replay
/tmp/propkit-llvm --hirsel-replay /tmp/propkit.tape
```

`replay` exits 1 and prints `replayed category=7 minimum=7`. App replay regenerates
the indexed original and checks the recorded minimum's failure category; runtime
tape replay consumes the recorded capability inputs without host file access.
Use distinct new tape paths, outside all writable grants. Cranelift uses the same
build/launch arguments with `--backend cranelift`.

`replay-cancel` and `replay-release` demonstrate interruptible loading using the
second `output` mount. Scripted file providers can park `records/failure.json`
until cancellation or an `output/release` write. Ordinary files ignore these
readiness scripts. `cancel` instead cancels a pending stdin read and writes
`output/joined` from its resource guard. Keep stdin open with no data to exercise
the pending case. Every task is joined before cleanup is inspected.

An optional final `--ready` prints `ready` and waits for one stdin byte. A host can
inspect the ordinary production control socket at that application handshake
before releasing the workload. Capacity evidence requires phase `running`,
effective requested N, pending zero; it makes no simultaneous CPU-use claim.

The modules have separate responsibilities:

| Module | Responsibility |
|---|---|
| strategy | Owned generic draw/reduce functions, int/list sources, map and bounded filter |
| shrink | Owned rollback/frontiers and signed simplify/complicate |
| runner | Outcome/error composition, case budgets and ordered task admission |
| replay | Schema validation, bounded files, cancellation and cleanup |
| properties | Real list/text/codec callbacks and explicit faulty/reject/error controls |
| main | Args, capability projections, output and exit status |

`Strategy[T]` callbacks own their environments. Draw results and each shrink
candidate have explicit unique ownership. Scoped callbacks only borrow for their
call. `Shared` roots transport inert callable environments across tasks. The
runner instantiates record strategies; supplemental vectors instantiate integer
and list strategies and trees too. Pure decisions require no capabilities.

Each case starts std Xoshiro256++ from `seed + index * 0x9e3779b97f4a7c15` modulo
2^64. List draws take one length draw followed by a fresh seed per element. The
split is independent of logical jobs and completion order; it is not proptest RNG
compatibility. Batches admit at most jobs cases, join in index order, then cancel
and join higher cases before deterministic shrinking. Counts cover the selected
prefix; later admitted work is discarded. The first matching Fail category is
retained; Pass, Reject and another category roll the tree back.

Limits are cases1..64, jobs1..4, candidates0..64, depth1..4, filter attempts1..32,
list length0..8, integers-32..32, replay bytes0..65536. The CLI checks admission
limits before cases or reads. Replay checks bytes before parsing and list bounds
before building elements. Pure callbacks cannot be interrupted within a call.
A shrink budget may stop before a global minimum; the report preserves failure.

| Exit | Meaning |
|---|---|
| 0 | Passed (or help) |
| 1 | Falsified/replayed failure |
| 2 | Invalid input or resource refusal |
| 3 | Property or I/O error |
| 4 | Rejected cases or exhausted filters, with separate counts |
| 5 | Canceled and joined |

The focused Rust harness is `property_kit_31` in CLI shard1. It runs the complete
entry with Args and scripted providers on reference/checked-H/LLVM/Cranelift;
independent vectors supplement the full app. Production qualification is ignored
in routine pool tests because a four-CPU host allowance is required. Its Python
helper accepts `PROPERTY_KIT_31_PRODUCTION_CELLS` (comma-separated exact cells,
e.g. `llvm-w4-success`) to run only missing cells and flushes each readiness and
completion receipt. See FRICTION.md for precise dispositions and archival gaps.

References: proptest `70cf37d46b7d83393b6ed94c611f8c52209f585d`,
`strategy/traits.rs:580–621`, `num.rs:280–345,995–1069` (LICENSE.proptest); Hirsel
history runner `8530c5a0`, `tests/support/history.rs:333–358`. The port uses typed
Pass/Fail/Reject and Result errors instead of panic unwinding. Bounded owned
frontiers replace upstream's unbounded value trees; integer candidates are zero,
binary midpoint, and one step toward zero. List deletion/restart precedes element
reductions. Shipped std reversal, sort, JSON and canonical codec implement the
useful properties; the expected seed and minimum vectors are independent.

#!/usr/bin/env python3
"""S1 acceptance: the final multicore design's S-1..S-14 text is in its target spec.

Run with python3 tools/tests/multicore_spec.py. Whitespace wrapping is ignored;
wording and punctuation are exact. The gate also discovers this tool test.
"""
from pathlib import Path
import unittest

ROOT = Path(__file__).resolve().parents[2]
SPEC = ROOT / 'docs/spec'

# Literal acceptance text, independent of the external design brief at test time.
DELTAS = {'S-1': [('determinism.md',
          'Its observations are those of one cooperative scheduler at every execution width. '
          "An implementation may run several tasks' code between scheduling points at once on "
          'several threads. Scheduling points, synchronising operations, host requests, '
          '`shared.thaw` tests, failures, stops and budget charges take effect in the order '
          'the selected scheduler gives. Execution width is a placement member and never part '
          'of identity.')],
 'S-2': [('concurrency.md', '`Shared` root counts'),
         ('concurrency.md',
          'and the releases of a root that a `shared.thaw` of it observes, before that '
          'thaw.')],
 'S-3': [('determinism.md',
          'task interleaving, atomic order, mutex acquisition order, the acquisition order of '
          "each `serial` package's foreign gate (S-13), the sole-holder outcome of each "
          "`shared.thaw`, each task's observed cancellation at each check, and each deadline "
          "scope's expiry-versus-exit outcome")],
 'S-4': [('memory.md',
          'updated without data races at every execution width. A root count is exact: it '
          "never saturates. `thaw`'s sole-holder test is a synchronising event (CONC-4).")],
 'S-5': [('memory.md',
          'A count about to overflow makes the block immortal, except a `Shared` root count, '
          'which is exact (MEM-11).')],
 'S-6': [('ownership.md',
          "A block's storage may return to another thread's allocator; this is unobservable "
          '(DET-1).')],
 'S-7': [('determinism.md', '`budgets.limits@2`, which retires `budgets.limits@1`'),
         ('determinism.md',
          "**Memory** is the run's logical storage: for each live counted block, its size "
          'class; for each live large block, its request rounded up to 4 KiB; for each open '
          'region, the canonical segment sequence its own requests would take from an empty '
          'pool. A charge happens when a task allocates and a credit when a logical lifetime '
          "ends, in the order the run's scheduler gives. The first charge, in that order, "
          'that exceeds its budget decides the stop and its kind. The interpreter charges the '
          'same quantities from its value model.'),
         ('determinism.md',
          'Fuel is apportioned to running tasks in chunks, returned when a task stops '
          "running. The run stops when a charge finds no fuel left in its chunk or the run's "
          'pool, so up to one chunk per running task may remain unspent. A replay does not '
          're-enforce budgets; it stops at the recorded cut.')],
 'S-8': [('memory.md', '8 MiB of idle backing storage per region pool (one per worker)'),
         ('memory.md', "the run's end releases everything retained.")],
 'S-9': [('determinism.md',
          'A **placement member** (`placement`) chooses where and how fast a run executes and '
          'nothing it observes. Validation checks it, then removes it from the canonical '
          'bytes whatever its value. The validated header states the resolved placement as '
          'diagnostic provenance. Replay ignores it and never opens a path it names.')],
 'S-10': [('runners.md',
           '| Execution width | `placement.cores` | 1 | simulators run on one thread |')],
 'S-11': [('concurrency.md',
           '`channel.send` is not a cancellation point: it succeeds or returns `Closed` with '
           "its value whatever its scope's cancellation; `task.check_canceled` marks producer "
           'loops.'),
          ('core-library.md',
           '`send` never checks cancellation; `Canceled` is an outcome of `receive` only (the '
           'error type is shared).')],
 'S-12': [('determinism.md',
           'Suspension at a point CONC-1 permits is allowed, not required; a task yields at '
           'least every 128 operations, and a spawned child may run at once. A `Shared` '
           'release that leaves other holders takes effect when the releasing task next '
           "enters an operation, or at its end; both engines release a value's roots at its "
           'cleanup point (S-14), in DROP-2 order. Runs record the per-object order of '
           "synchronising operations (each `serial` package's foreign gate included), atomic "
           'load values and the scheduling inputs of DET-1 (S-3), keyed by (task spawn path, '
           'operation ordinal). Host requests are ordered per logical host resource in shared '
           'and exclusive epochs, and host handles are named by (creating task path, its '
           'handle ordinal). A replay re-executes the recorded cut, which is causally closed. '
           'A record naming both `tasks.free@1` and `inputs.scheduler` refuses.')],
 'S-13': [('programs.md',
           'The `unsafe_ffi` object gains the trusted member `threads`: `"home"` (default; '
           'the run\'s admission thread), `"serial"` (any thread, one call chain at a time '
           'per package; a callback\'s nested call to the same package re-enters) or `"any"` '
           '(any thread, concurrently).'),
          ('programs.md',
           '**Dispatch precedence:** a call inside an active foreign interval runs on that '
           "interval's thread; otherwise a call to a `home` package runs on the admission "
           'thread, `@blocking` or not, and occupies it until it returns; otherwise a '
           '`@blocking` call runs on the bounded pool; otherwise it runs on the calling '
           'thread. A callback that can reach a `home` package may be handed only to an '
           'extern whose effective thread is the admission thread, that is, a `home` '
           "package's extern; otherwise the build refuses, naming both."),
          ('programs.md',
           "The outermost entry of a `serial` package's gate is a synchronising operation "
           '(CONC-4) and its order a scheduling input (S-3); nested re-entry is not.'),
          ('programs.md', 'Callbacks still run on the calling thread during the call.'),
          ('concurrency.md',
           'Foreign functions declared `@blocking` run on a bounded synchronous pool, except '
           'a call to a `home` package, which runs on the admission thread, and a nested '
           "call, which runs on its enclosing call's thread.")],
 'S-14': [('failure.md',
           'A value whose destruction releases a `Shared` root, directly or through any '
           "component, holds that root until its cleanup point (its scope's cleanup or its "
           'consumption, on normal and failure paths). No implementation releases such a '
           'value earlier, so the holders of every root at every synchronising operation are '
           'the same in every engine.'),
          ('memory.md',
           "A value's roots are held until its cleanup point, as specified by "
           '[DROP-2](failure.md#drop-2-resources) (S-14).')]}


def normalized(text):
    return ' '.join(text.split())


class MulticoreSpec(unittest.TestCase):
    def test_all_fourteen_deltas_are_in_their_target_files(self):
        self.assertEqual(set(DELTAS), {f'S-{n}' for n in range(1, 15)})
        for delta, clauses in DELTAS.items():
            for file, text in clauses:
                with self.subTest(delta=delta, file=file, clause=text):
                    self.assertIn(normalized(text), normalized((SPEC / file).read_text()))

    def test_conformance_index_names_the_four_new_contracts(self):
        index = (ROOT / 'conformance/README.md').read_text()
        for delta, targets in {
            'S-11': ('concurrency.md#conc-3-cancellation', 'core-library.md#concurrency', 'S2'),
            'S-12': ('determinism.md#run-configuration', 'F0/F1'),
            'S-13': ('programs.md#prog-4-interop', 'concurrency.md#conc-2-tasks', 'R8/F1'),
            'S-14': ('failure.md#drop-2-resources', 'memory.md#mem-11-shared', 'R7/F1'),
        }.items():
            with self.subTest(delta=delta):
                row, = (line for line in index.splitlines() if line.startswith(f'| {delta} |'))
                for target in targets:
                    self.assertIn(target, row)

    def test_superseded_contracts_are_removed(self):
        for file, obsolete in (
            ('concurrency.md', '`Shared` roots (atomic)'),
            ('memory.md', 'The current single-threaded scheduler uses plain counts'),
            ('memory.md', '8 MiB of idle backing storage per task'),
            ('core-library.md', 'A nonblocking send does not check cancellation'),
            ('determinism.md', 'with idle slabs returned before a stop'),
            ('determinism.md', '16 bytes per value node'),
            ('determinism.md', 'so a replay applies them again'),
        ):
            with self.subTest(file=file, obsolete=obsolete):
                self.assertNotIn(obsolete, normalized((SPEC / file).read_text()))


if __name__ == '__main__':
    unittest.main()

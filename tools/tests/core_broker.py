#!/usr/bin/env python3
"""K2's toy broker and quota, congestion and cooperative resize regressions."""
from copy import deepcopy
import json
from pathlib import Path
import runpy
import socket
import subprocess
import sys
import tempfile
import threading
import unittest

ROOT = Path(__file__).resolve().parents[2]
BROKER = ROOT / 'tools/core-broker'
api = runpy.run_path(str(BROKER))


def metrics(width, delay=0, runnable=0, busy=0.1, pending=0):
    return dict(width=dict(requested=width, effective=width, pending=pending),
                segment_ready_delay_ns=dict(p50=delay, p99=delay),
                runnable=runnable, role_busy=busy, lost_permits=0,
                reclamation_pending=0, stop_requested=False)


class ToyRun:
    """The K1 JSON-line socket protocol with an injectable safe-boundary delay."""
    def __init__(self, path, sample):
        self.path, self.sample = path, sample
        self.requests = []
        self.errors = []
        self.delayed = False
        self.lock = threading.Lock()
        self.listener = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        self.listener.bind(str(path))
        self.listener.listen()
        self.thread = threading.Thread(target=self.serve)
        self.thread.start()

    def serve(self):
        try:
            while True:
                peer, _ = self.listener.accept()
                with peer, peer.makefile('rb') as reader:
                    request = json.loads(reader.readline())
                    if request == {'quit': True}:
                        return
                    with self.lock:
                        self.requests.append(request)
                        if 'placement' in request:
                            width = request['placement']['cores']
                            self.sample['width']['requested'] = width
                            if self.delayed:
                                self.sample['width']['pending'] = width
                            else:
                                self.sample['width'].update(effective=width, pending=0)
                        peer.sendall(json.dumps(self.sample).encode() + b'\n')
        except BaseException as error:
            self.errors.append(error)

    def close(self):
        with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as peer:
            peer.connect(str(self.path))
            peer.sendall(b'{"quit":true}\n')
        self.thread.join(timeout=2)
        self.listener.close()
        assert not self.thread.is_alive()
        assert not self.errors, self.errors


class BrokerTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory(prefix='hirsel-broker-')
        self.addCleanup(self.directory.cleanup)
        self.root = Path(self.directory.name)
        self.settings = dict(cores=4, cgroup_root=str(self.root), interval_ms=100,
                             delay_ns=100, runs=[])
        for name in ('a', 'b'):
            group = self.root / name
            group.mkdir()
            (group / 'cpu.max').write_text('max 100000\n')
            self.settings['runs'].append(dict(name=name, control=str(self.root / (name + '.sock')),
                                             cgroup=str(group), guaranteed=1, burst=3))
        self.samples = {'a': metrics(3), 'b': metrics(1, delay=200, runnable=10)}
        self.requests = []

    def transport(self, path, request):
        name = Path(path).stem
        sample = self.samples[name]
        self.requests.append((name, request))
        if 'placement' in request:
            sample['width'].update(requested=request['placement']['cores'],
                                   effective=request['placement']['cores'], pending=0)
        return deepcopy(sample)

    def broker(self):
        return api['Broker'](self.settings, self.transport)

    def widths(self):
        return [s['width']['effective'] for s in self.samples.values()]

    def test_toy_broker_rebalances_two_runs_and_both_widths_change(self):
        # Caladan simple.c:35-40: reclaim a donor's burst, retain its guarantee.
        toys = []
        for run in self.settings['runs']:
            toy = ToyRun(Path(run['control']), self.samples[run['name']])
            self.addCleanup(toy.close)
            toys.append(toy)
        path = self.root / 'broker.json'
        path.write_text(json.dumps(self.settings))
        before = self.widths()
        for delay in (200, 300):
            with toys[1].lock:
                toys[1].sample['segment_ready_delay_ns']['p99'] = delay
            result = subprocess.run([sys.executable, str(BROKER), str(path), '--once'],
                                    capture_output=True, text=True, timeout=5)
            self.assertEqual(result.returncode, 0, result.stderr + result.stdout)
            self.assertEqual(len(json.loads(result.stdout)['runs']), 2)
        self.assertEqual(before, [3, 1])
        self.assertEqual(self.widths(), [1, 3])
        for old, new in zip(before, self.widths()):
            self.assertNotEqual(old, new)

    def test_shrink_is_acknowledged_before_any_growth(self):
        self.broker().tick()
        mutations = [(name, r['placement']['cores']) for name, r in self.requests if 'placement' in r]
        self.assertEqual(mutations, [('a', 1), ('b', 2)])

    def test_pending_shrink_does_not_lend_occupied_cores(self):
        toys = []
        for run in self.settings['runs']:
            toy = ToyRun(Path(run['control']), self.samples[run['name']])
            self.addCleanup(toy.close)
            toys.append(toy)
        toys[0].delayed = True
        broker = api['Broker'](self.settings)
        broker.tick()
        self.assertEqual(self.widths(), [3, 1])
        self.assertEqual(self.samples['a']['width']['pending'], 1)
        with toys[0].lock:
            self.samples['a']['width'].update(effective=1, pending=0)
        # Demand persists while the donor releases workers, even if the next
        # sampled queue delay is unchanged.
        broker.tick()
        self.assertEqual(self.widths(), [1, 2])

    def test_pending_growth_remains_reserved(self):
        self.settings['runs'][0]['guaranteed'] = 3
        self.samples['a'] = metrics(1, delay=1000, runnable=8, pending=3)
        self.samples['a']['width']['requested'] = 3
        self.broker().tick()
        self.assertEqual(self.samples['b']['width']['effective'], 1)

    def test_auto_and_range_requests_become_broker_owned_integers_without_unprompted_growth(self):
        for request in ('auto', dict(min=1, max=3)):
            with self.subTest(request=request):
                self.samples['a'] = metrics(1)
                self.samples['a']['width']['requested'] = request
                self.samples['b'] = metrics(1)
                self.broker().tick()
                self.assertEqual(self.samples['a']['width']['requested'], 1)
                self.assertEqual(self.widths(), [1, 1])

    def test_socket_refusal_aborts_before_any_growth(self):
        toys = []
        for run in self.settings['runs']:
            toy = ToyRun(Path(run['control']), self.samples[run['name']])
            self.addCleanup(toy.close)
            toys.append(toy)
        with toys[1].lock:
            toys[1].sample = {'error': 'run stopped'}
        with self.assertRaisesRegex(ValueError, 'run stopped'):
            api['Broker'](self.settings).tick()
        self.assertFalse(any('placement' in r for toy in toys for r in toy.requests))

    def test_failed_shrink_never_grants_another_run(self):
        failing = True
        def refuse(path, request):
            if failing and 'placement' in request:
                raise OSError('shrink connection closed')
            return self.transport(path, request)
        broker = api['Broker'](self.settings, refuse)
        with self.assertRaises(OSError):
            broker.tick()
        self.assertEqual(self.widths(), [3, 1])
        failing = False
        broker.tick()
        self.assertEqual(self.widths(), [1, 2])

    def test_quota_contraction_limits_a_guarantee_and_reports_it(self):
        self.settings['runs'][0]['guaranteed'] = 3
        (self.root / 'a/cpu.max').write_text('100000 100000\n')
        report = self.broker().tick()
        self.assertEqual(self.samples['a']['width']['effective'], 1)
        self.assertTrue(report['runs'][0]['guarantee_limited'])

    def test_never_sets_integer_above_ceil_of_ancestor_quota(self):
        (self.root / 'cpu.max').write_text('100001 100000\n')
        broker = self.broker()
        for delay in (200, 300, 400):
            self.samples['b']['segment_ready_delay_ns']['p99'] = delay
            broker.tick()
        self.assertEqual(self.widths(), [1, 2])
        self.assertTrue(all(r['placement']['cores'] <= 2 for _, r in self.requests if 'placement' in r))

    def test_exact_quota_rounding_and_minimum_of_all_ancestors(self):
        run = self.settings['runs'][0]
        run['burst'] = 100
        (self.root / 'cpu.max').write_text('9007199254740993 9007199254740992\n')
        self.assertEqual(api['quota_ceiling'](run, self.root), 2)
        (Path(run['cgroup']) / 'cpu.max').write_text('50000 100000\n')
        self.assertEqual(api['quota_ceiling'](run, self.root), 1)

    def test_saturated_role_is_flagged_without_growth(self):
        self.samples['b']['role_busy'] = 0.8
        report = self.broker().tick()
        self.assertEqual(self.widths(), [1, 1])
        self.assertEqual(report['runs'][1]['suggest'], 'tasks.free@1 or runner sharding')

    def test_only_rising_delay_grows_and_burst_is_a_hard_maximum(self):
        broker = self.broker()
        broker.tick()
        self.assertEqual(self.widths(), [1, 2])
        broker.tick()
        self.assertEqual(self.widths(), [1, 2])
        for delay in (300, 400, 500):
            self.samples['b']['segment_ready_delay_ns']['p99'] = delay
            broker.tick()
        self.assertEqual(self.widths(), [1, 3])

    def test_missing_signals_hold_width_and_stopped_runs_never_grow(self):
        for sample in self.samples.values():
            sample.update(segment_ready_delay_ns=None, runnable=None, role_busy=None)
        self.broker().tick()
        self.assertEqual(self.widths(), [3, 1])
        self.samples['b'] = metrics(1, delay=200, runnable=10)
        self.samples['b']['stop_requested'] = True
        self.broker().tick()
        self.assertEqual(self.widths(), [3, 1])

    def test_missing_queue_depth_does_not_trigger_burst_growth(self):
        self.samples['b']['runnable'] = None
        self.broker().tick()
        self.assertEqual(self.widths(), [1, 1])

    def test_lost_and_reclamation_permits_are_not_replaced_or_counted_twice(self):
        self.samples['a'].update(lost_permits=1, reclamation_pending=1)
        broker = self.broker()
        for delay in (200, 300, 400):
            self.samples['b']['segment_ready_delay_ns']['p99'] = delay
            broker.tick()
        self.assertEqual(self.widths(), [1, 2])

    def test_guarantees_precede_burst_even_with_rotated_priority(self):
        self.settings['runs'][0]['guaranteed'] = 2
        self.broker().tick()
        self.assertEqual(self.widths(), [2, 2])

    def test_unknown_limits_or_dead_peer_freeze_tick_before_mutations(self):
        (self.root / 'b/cpu.max').write_text('broken\n')
        with self.assertRaises(ValueError):
            self.broker().tick()
        self.assertFalse(any('placement' in r for _, r in self.requests))

    def test_configuration_refuses_invalid_or_overcommitted_grants(self):
        for mutate in (lambda c: c.update(cores=1),
                       lambda c: c.update(interval_ms=99),
                       lambda c: c.update(interval_ms=1001),
                       lambda c: c.update(cores=True),
                       lambda c: c.update(unrecognized=1),
                       lambda c: c['runs'][1].update(name='a'),
                       lambda c: c['runs'][1].update(control=c['runs'][0]['control']),
                       lambda c: c['runs'][1].update(cgroup='/'),
                       lambda c: c['runs'][1].update(guaranteed=4),
                       lambda c: c['runs'][1].update(burst=0)):
            with self.subTest(mutate=mutate):
                settings = deepcopy(self.settings)
                mutate(settings)
                with self.assertRaises(ValueError):
                    api['config'](settings)

    def test_malformed_snapshots_and_refusals_never_become_metrics(self):
        for change in (dict(role_busy=float('nan')), dict(role_busy=1.1),
                       dict(runnable=-1), dict(lost_permits=True), dict(error='refused'),
                       dict(stop_requested=1), dict(width=dict(effective=1, pending=-1, requested=1))):
            with self.subTest(change=change), self.assertRaises(ValueError):
                api['snapshot'](metrics(1) | change)
        api['snapshot'](metrics(1) | dict(role_busy=None, runnable=None))

    def test_equally_congested_runs_share_burst_and_total_never_exceeds_pool(self):
        self.settings['cores'] = 5
        self.samples = {'a': metrics(1, 200, 10), 'b': metrics(1, 200, 10)}
        broker = self.broker()
        for delay in range(200, 1000, 100):
            for sample in self.samples.values():
                sample['segment_ready_delay_ns']['p99'] = delay
            broker.tick()
            self.assertLessEqual(sum(self.widths()), 5)
            self.assertLessEqual(abs(self.widths()[0] - self.widths()[1]), 1)
        self.assertEqual(sum(self.widths()), 5)


if __name__ == '__main__':
    unittest.main()

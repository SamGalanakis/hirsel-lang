#!/usr/bin/env python3
"""Consumer selection follows real launch bodies, source uses and shard assignment."""
import importlib.machinery
import importlib.util
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[2]
loader = importlib.machinery.SourceFileLoader('consumers', str(ROOT / 'tools/consumers'))
consumers = importlib.util.module_from_spec(importlib.util.spec_from_loader('consumers', loader))
loader.exec_module(consumers)


def graph_for(inventories):
    return {f'root//:{binary}-shard-{index + 1}':
            dict(test_binary=binary, shard_index=index, shards=3, costs=None)
            for binary in inventories for index in range(3)}


class Consumers(unittest.TestCase):
    def test_cpu_pool_change_selects_native_launches_and_replay_before_other_shards(self):
        path = 'crates/hirsel-rt/src/pool.rs'
        inventories = {binary: set(cases) for binary, cases in consumers.MUST.items()}
        inventories['hirsel-cli-test-free'] = {
            'native_returned_rest_completion_replays_across_engines',
            'every_f0_trace_replays_to_its_live_projection_at_every_cut_gap_interpreter',
        }
        graph = graph_for(inventories)
        cpu_targets = {f'root//:hirsel-cli-test-cpu_pool_{width}' for width in (1, 4, 8, 16)}
        graph |= {target: dict(test_binary=target.split(':')[1]) for target in cpu_targets}
        selected = consumers.select(ROOT, [path], graph, inventories)
        assignments = consumers.assignments(ROOT, graph, inventories)
        expected = cpu_targets | {assignments[binary, case] for binary, cases in consumers.MUST.items() for case in cases}
        expected.add(assignments['hirsel-cli-test-free', 'native_returned_rest_completion_replays_across_engines'])
        expected |= {'root//:hirsel-cli-test-cli-consumer-native-http',
                     'root//:hirsel-cli-test-cli-consumer-native-http-concurrent'}
        self.assertLessEqual(expected, set(selected[:consumers.LIMIT]))

    def test_std_module_selects_the_conformance_shard_that_uses_it(self):
        path = 'stdlib/std@1/toml.hir'
        corpus = 'conformance/SEM/SEM-3-toml-edit-cargo-run.hir'
        inventories = {'hirsel-driver-test-it-stdlib': {'toml_upstream_25::generated_00'}}
        graph = graph_for(inventories)
        selected = consumers.select(ROOT, [path], graph, inventories)
        mapped = consumers.assignments(ROOT, graph, inventories)
        self.assertIn(mapped['hirsel-driver-test-it-stdlib', 'toml_upstream_25::generated_00'], selected)
        from sync import CONFORMANCE_SHARDS
        target = f'root//:gate-conformance-shard-{consumers.corpus_shard(corpus, CONFORMANCE_SHARDS)}'
        self.assertIn(target, selected)
        self.assertNotIn('root//:hirsel-llvm-unittest', selected)

    def test_module_uses_propagate_through_other_std_modules(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            std = root / 'stdlib/std@1'
            std.mkdir(parents=True)
            (std / 'a.hir').write_text('pub fn a() -> () {}')
            (std / 'b.hir').write_text('pub fn b() -> () { a.a() }')
            (std / 'c.hir').write_text('pub fn c() -> () { b.b() }')
            self.assertEqual(consumers.module_users(root, ['stdlib/std@1/a.hir']), {'a', 'b', 'c'})

    def test_case_source_follows_helpers_and_loaded_corpus_without_string_braces(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / 'loaded.hir').write_text('toml.document("a=1")')
            (root / 'main.rs').write_text('''
fn unrelated() { let text = "fn regression() { }"; }
fn fixture() { let text = r###" } { "###; include_str!("loaded.hir"); }
#[test]
fn regression() { fixture(); }
''')
            source = consumers.Sources(root, 'main.rs')
            selected = source.case('regression')
            self.assertIn('toml.document', selected)
            self.assertNotIn('fn unrelated', selected)
            self.assertNotIn('toml.document', source.case('unrelated'))

    def test_real_libtest_listing_format_resolves_cases_to_their_binary(self):
        binary = 'hirsel-cli-test-cli'
        graph = graph_for({binary: set()})
        output = ('[2026-10-29T16:00:00] ✓ Pass: root//:hirsel-cli-test-cli-shard-1 (0.5s)\n'
                  '[2026-10-29T16:00:00] ---- STDOUT ----\n'
                  '[2026-10-29T16:00:00] https::verifies_replay: test\n')
        with patch.object(consumers, 'buck', return_value=output):
            self.assertEqual(consumers.listings(ROOT, graph), {binary: {'https::verifies_replay'}})
        with patch.object(consumers, 'buck', return_value=''):
            with self.assertRaisesRegex(ValueError, 'no case listing for hirsel-cli-test-cli'):
                consumers.listings(ROOT, graph)

    def test_isolated_body_selects_its_dedicated_target_after_balancing(self):
        binary = 'hirsel-cli-test-free'
        inventories = {binary: {'isolated', 'ordinary'}}
        graph = graph_for(inventories)
        before = consumers.assignments(ROOT, graph, inventories)
        graph['dedicated'] = dict(test_binary=binary, extra_env={'HIRSEL_TEST_DEDICATED_CASE': 'isolated'})
        after = consumers.assignments(ROOT, graph, inventories)
        self.assertEqual(after[binary, 'isolated'], 'dedicated')
        self.assertEqual(after[binary, 'ordinary'], before[binary, 'ordinary'])

    def test_cost_assignment_matches_the_existing_launcher_vector(self):
        # Same measured/unmeasured vector as buck2_launcher's behavioral test.
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / 'costs').write_text('heap\t9\nprefix\t1\nsmoke\t2\n')
            binary = 'fixture'
            graph = {f'shard-{index + 1}': dict(test_binary=binary, shard_index=index,
                                              shards=2, costs='root//:costs') for index in range(2)}
            result = consumers.assignments(root, graph, {binary: {'heap', 'prefix', 'smoke', 'other', 'host'}})
            self.assertEqual({case: target for (_, case), target in result.items()},
                             dict(heap='shard-1', prefix='shard-1', smoke='shard-2',
                                  other='shard-2', host='shard-2'))


if __name__ == '__main__':
    unittest.main()

"""C arena registration, exact byte/counter oracles, and pool lifetime controls."""
import importlib.machinery
import importlib.util
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))
from suite import controlled_environment, lift_data_limit, load_manifest, participants, sha


class CArenaTests(unittest.TestCase):
    def test_registration_is_binarytrees_only_and_archives_keep_their_contract(self):
        manifest = load_manifest()
        for case in manifest['cases']:
            self.assertEqual('c/arena' in participants(manifest, case), case['case'] == 'binarytrees')
        del manifest['c_arena_cases']
        self.assertTrue(all('c/arena' not in participants(manifest, c) for c in manifest['cases']))

    def test_arena_source_is_sealed_and_changed_source_refuses(self):
        manifest = load_manifest()
        source = 'c/binarytrees-arena.h'
        self.assertEqual(manifest['source_sha256'][source], sha(ROOT / source))
        manifest['source_sha256'][source] = '0' * 64
        with self.assertRaisesRegex(AssertionError, 'source changed'):
            load_manifest(manifest)

    def test_all_inputs_have_exact_timing_and_counter_oracles(self):
        loader = importlib.machinery.SourceFileLoader('c_arena_collector', str(ROOT / 'run'))
        spec = importlib.util.spec_from_loader(loader.name, loader)
        collector = importlib.util.module_from_spec(spec)
        loader.exec_module(collector)
        manifest = load_manifest()
        manifest['cases'] = [c for c in manifest['cases'] if c['case'] == 'binarytrees']
        with tempfile.TemporaryDirectory() as tmp:
            binaries = {}
            for suffix in ('', '-stats'):
                output = str(Path(tmp) / ('c-arena' + suffix))
                subprocess.run(['clang', '-std=c17', '-O3', '-ffp-contract=off', '-fno-lto',
                                '-Wall', '-Wextra', '-Werror', '-DARENA'] +
                               (['-DSTATS'] if suffix else []) +
                               [str(ROOT / 'c/main.c'), '-o', output], check=True,
                               env=controlled_environment())
                binaries['c/arena' + suffix] = {'path': output}
            # Exercise the production oracle/counter path rather than duplicating it.
            from unittest.mock import patch
            with patch.object(collector, 'participants', return_value=['c/arena']):
                result = collector.verify(manifest, {'executables': binaries})
            self.assertEqual(result['checks'], 12)
            self.assertEqual(result['failures'], [])
            self.assertTrue(result['passed'])

    def test_clear_reuses_chunks_without_overwriting_retained_tree(self):
        source = r'''
#define ARENA
#define STATS
#define main reference_main
#include "main.c"
#undef main
int main(void) {
    TreeArena retained = {0}, temporary = {0};
    Tree *long_tree = arena_tree(13, &retained);
    Tree *first = arena_tree(14, &temporary);
    assert(check_arena_tree(first) == 32767);
    size_t capacity = temporary.reserved;
    TreeChunk *second = temporary.first->next;
    assert(second);
    clear_tree_arena(&temporary);
    assert(arena_tree(0, &temporary) == first);
    clear_tree_arena(&temporary);
    assert(arena_tree(14, &temporary) == first);
    assert(temporary.reserved == capacity && temporary.first->next == second);
    assert(check_arena_tree(long_tree) == 16383);
    assert(check_arena_tree(first) == 32767);
    drop_tree_arena(&temporary);
    assert(check_arena_tree(long_tree) == 16383);
    drop_tree_arena(&retained);
    assert(tree_reserved == 0);
    assert(!temporary.first && !retained.first);
    return 0;
}
'''
        with tempfile.TemporaryDirectory() as tmp:
            fixture = Path(tmp) / 'lifetime.c'
            fixture.write_text(source)
            binary = Path(tmp) / 'lifetime'
            subprocess.run(['clang', '-std=c17', '-O1', '-g', '-fsanitize=address,undefined',
                            '-Wall', '-Wextra', '-Werror', '-I', str(ROOT / 'c'),
                            str(fixture), '-o', str(binary)], check=True)
            # AddressSanitizer's shadow mapping needs the soft data limit lifted.
            subprocess.run([str(binary)], check=True, env=controlled_environment(), preexec_fn=lift_data_limit)

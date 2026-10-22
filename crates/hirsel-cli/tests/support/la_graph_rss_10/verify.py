"""Run private Graph controls against vectors from pinned Rust graph_path.rs.

The diagnostic copies add a pop trace; the production searches remain intact.
LangArena 3c9f84dd, MIT: bench/langarena/LICENSE.
"""
import argparse
import json
from pathlib import Path
import re
import subprocess


def hir_list(values, suffix):
    return '[' + ', '.join(f'{v}{suffix}' for v in values) + ']'


def fixtures(source, vectors):
    width = re.search(r'adj: List\[List\[(\w+)\]\]', source)[1]
    generation = source[source.index('  let mut adj:'):source.index('  harness.measure(')]
    # Count exactly the existing RNG call sites, without adding draws.
    generation = generation.replace('  let mut seed = 42u64', '  let mut seed = 42u64\n  let mut draws = 0u64')
    generation = generation.replace('    let count = random', '    draws += 1\n    let count = random')
    generation = generation.replace('      let offset:', '      draws += 1\n      let offset:')
    extra = f'\ntype LaGeneration {{ adj: List[List[{width}]], seed: u64, draws: u64 }}\n'
    extra += 'fn la_generate(n: u64, jumps: u64, jump_len: u64) -> LaGeneration {\n'
    extra += generation + '  LaGeneration(adj: take adj, seed: seed, draws: draws)\n}\n'
    traced = source[source.index('fn astar('):source.index('\nfn step(')]
    traced = traced.replace('fn astar(s: State)', 'fn la_astar(s: State, mut trace: List[u64])')
    traced = traced.replace('fn search(s: State)', 'fn la_search(s: State, mut trace: List[u64])')
    traced = traced.replace('return astar(s)', 'return la_astar(s, mut trace)')
    traced = traced.replace('    if v == target {', '    list.push(mut trace, v)\n    if v == target {')
    traced = traced.replace('    let distance = current.score', '    list.push(mut trace, v)\n    let distance = current.score')
    extra += traced
    for i, vector in enumerate(vectors):
        rows = '[' + ', '.join(hir_list(row, width) for row in vector['adj']) + ']'
        extra += f'\n@test\nfn la_graph_rss_10_case_{i}(t: Test) {{\n'
        if vector['generated']:
            n, jumps, jump_len = vector['generated']
            # Independent draw count from the same published recurrence, not from the port.
            seed, draws = 42, 0
            for _ in range(n):
                seed = (seed * 3877 + 29573) % 139968
                count = int(seed / 139968.0 * jumps)
                draws += 1
                for _ in range(count):
                    seed = (seed * 3877 + 29573) % 139968
                    draws += 1
            assert seed == vector['seed']
            extra += f'  let generated = la_generate({n}, {jumps}, {jump_len})\n'
            extra += f'  test.check(t, generated.adj == {rows})\n'
            extra += f'  test.check(t, generated.seed == {seed} && generated.draws == {draws})\n'
        for mode, result in enumerate(vector['results']):
            extra += f'  let s{mode} = State(adj: {rows}, mode: {mode}, value_out: 0)\n'
            extra += f'  test.check(t, search(s{mode}) == {result}i64)\n'
            extra += f'  let mut trace{mode}: List[u64] = []\n'
            extra += f'  test.check(t, la_search(s{mode}, mut trace{mode}) == {result}i64)\n'
            extra += f'  test.check(t, trace{mode} == {hir_list(vector["traces"][mode], "u64")})\n'
        extra += '}\n'
    extra += '\n@test\nfn la_graph_rss_10_heap_ties(t: Test) {\n  let mut q: unique List[Entry] = []\n'
    entries = [(3, 9), (1, 8), (1, 2), (3, 0), (1, 2), (0, 7), (2, 1)]
    for score, vertex in entries:
        extra += f'  push(mut q, Entry(score: {score}, vertex: {vertex}))\n'
    for i, (score, vertex) in enumerate(sorted(entries)):
        extra += f'  let e{i} = pop(mut q)\n  test.check(t, e{i}.score == {score} && e{i}.vertex == {vertex})\n'
    extra += '  test.check(t, list.len(q) == 0)\n}\n'
    # test.js has one warmup plus three measured runs; state checksum is retained.
    extra += '''\n@test
fn la_graph_rss_10_test_js(t: Test) {
  let adj = la_generate(100, 3, 100).adj
  for mode in 0u64..3u64 {
    let mut s = State(adj: copy adj, mode: mode, value_out: 0)
    for i in 0u64..4u64 { step(mut s, i) }
    let expected = if mode == 0 { 20u32 } else if mode == 1 { 28u32 } else { 36u32 }
    test.check(t, checksum(s) == expected)
  }
}
'''
    return extra


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--cli', required=True)
    parser.add_argument('--stdlib', required=True)
    parser.add_argument('--backend', required=True)
    parser.add_argument('--directory', type=Path, required=True)
    args = parser.parse_args()
    d = args.directory
    source = (d/'graph.hir').read_text()
    vectors = json.loads((d/'vectors.json').read_text())
    (d/'graph.hir').write_text(source + fixtures(source, vectors))
    (d/'main.hir').write_text('pub fn main() {}\n')
    manifest = dict(schema=1, package=dict(name='la_graph_rss_10', version='1'),
        entry=dict(module=[], name='main'),
        sources=[dict(path=m+'.hir', module=[] if m == 'main' else [m]) for m in ['main', 'graph', 'harness']],
        dependencies=[dict(manifest=args.stdlib, package=dict(name='std', version='1'), expose=[])])
    (d/'hirsel.json').write_text(json.dumps(manifest, indent=2)+'\n')
    command = [args.cli, 'test']
    command += ['--max-steps', 'unbounded'] if args.backend == 'h' else ['--native', '--backend', args.backend]
    result = subprocess.run(command+[str(d/'hirsel.json'), 'la_graph_rss_10_'], capture_output=True, text=True)
    output = result.stdout + result.stderr
    print(output, end='', flush=True)
    assert result.returncode == 0, result.returncode
    assert f'{len(vectors) + 2} passed' in output, output


if __name__ == '__main__':
    main()

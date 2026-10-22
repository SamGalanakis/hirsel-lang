#!/usr/bin/env python3
"""Private A* fixtures against a sorted-frontier model; H, LLVM and Cranelift.

The temporary modules append tests to the actual port, keeping private helpers
private. Reference: LangArena 3c9f84d graph_path.rs:249-295, maze.rs:383-444
(MIT; see LICENSE). Graph's target-v heuristic is deliberately not a BFS oracle.
"""
import heapq
import json
import os
from pathlib import Path
import random
import subprocess
import tempfile
import unittest

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[1]
INF = 2147483647


def model(adj, start, target, heuristic, sorted_frontier=False):
    g, best, came = [INF] * len(adj), [INF] * len(adj), [-1] * len(adj)
    g[start] = 0
    best[start] = heuristic(start)
    frontier, trace, relaxations = [(best[start], start)], [], []
    while frontier:
        if sorted_frontier:
            frontier.sort(reverse=True)
            _, at = frontier.pop()
        else:
            _, at = heapq.heappop(frontier)
        trace.append(at)
        if at == target:
            path, cur = [], at
            while cur != -1:
                path.append(cur)
                cur = came[cur]
            return g[at], path[::-1], trace, relaxations
        for neighbor in adj[at]:
            tentative = g[at] + 1
            if tentative < g[neighbor]:
                relaxations.append((neighbor, g[neighbor], tentative))
                came[neighbor], g[neighbor] = at, tentative
                score = tentative + heuristic(neighbor)
                if score < best[neighbor]:
                    best[neighbor] = score
                    if sorted_frontier:
                        frontier.append((score, neighbor))
                    else:
                        heapq.heappush(frontier, (score, neighbor))
    return -1, [], trace, relaxations


def graph_cases():
    cases = [[[]], [[], []], [[1], [0, 2], [1]],
             [[1, 2], [3], [3], []],
             # Nonadmissible target-v: the first target can be a longer route.
             [[1, 8], [9], [], [], [], [], [9], [6], [7], []]]
    rng = random.Random(9009)
    for _ in range(40):
        n = rng.randrange(3, 14)
        cases.append([[j for j in range(n) if j != i and rng.random() < .24]
                      for i in range(n)])
    return cases


def maze_cases():
    # Rectangular grids: Manhattan is admissible. Opposite neighbor orders
    # retain the same priority/row-major tie ordering and exact expected path.
    cases = [(3, 3, {4}, False), (5, 5, {6, 18}, False)]
    for reverse in [False, True]:
        cases.append((5, 5, {y * 5 + x for y in range(1, 4) for x in range(1, 4)}, reverse))
    cases += [(6, 5, {7, 8, 9, 10, 13, 16, 19, 20, 21, 22}, False),
              (7, 6, {y * 7 + x for y in range(1, 5) for x in range(1, 6)} - {17, 24}, True)]
    return cases


def maze_adj(w, h, walkable, reverse):
    adj, neighbors = [], []
    for at in range(w * h):
        y, x = divmod(at, w)
        row = [at-w, at+w, at+1, at-1] if 0 < x < w-1 and 0 < y < h-1 else [0]*4
        if reverse:
            row.reverse()
        neighbors.append(row)
        adj.append([n for n in row if n in walkable])
    return adj, neighbors


def hir_list(values, suffix='u64'):
    return '[' + ', '.join(f'{v}{suffix}' for v in values) + ']'


def fixture_sources():
    graph, maze = '', ''
    for i, adj in enumerate(graph_cases()):
        expected, _, _, _ = model(adj, 0, len(adj)-1, lambda v: len(adj)-1-v)
        rows = '[' + ', '.join(hir_list(row) for row in adj) + ']'
        graph += f'''\n@test
fn la_astar_9_graph_{i}(t: Test) {{
  let s = State(adj: {rows}, mode: 2, value_out: 0)
  test.check(t, search(s) == {expected}i64)
}}
'''
    for i, (w, h, walkable, reverse) in enumerate(maze_cases()):
        adj, neighbors = maze_adj(w, h, walkable, reverse)
        _, path, _, _ = model(adj, w+1, (h-2)*w+w-2,
                             lambda v: h-2-v//w+w-2-v%w)
        cells = '[' + ', '.join(f'Cell(kind: {1 if at in walkable else 4}, neighbors: {hir_list(row)})'
                                for at, row in enumerate(neighbors)) + ']'
        maze += f'''\n@test
fn la_astar_9_maze_{i}(t: Test) {{
  let s = State(w: {w}, h: {h}, cells: {cells}, mode: 2, path: [], value_out: 0)
  let path = astar(s)
  test.check(t, path == {hir_list(path)})
}}
'''
    # Heap payload/tie order and duplicate entries, independent of path lengths.
    for module in ['graph', 'maze']:
        entries = [(3, 9), (1, 8), (1, 2), (3, 0), (1, 2), (0, 7), (2, 1)]
        body = '\n@test\nfn la_astar_9_heap(t: Test) {\n  let mut q: unique List[Entry] = []\n'
        for score, vertex in entries:
            body += f'  push(mut q, Entry(score: {score}, vertex: {vertex}))\n'
        for i, (score, vertex) in enumerate(sorted(entries)):
            body += f'  let e{i} = pop(mut q)\n  test.check(t, e{i}.score == {score} && e{i}.vertex == {vertex})\n'
        body += '  test.check(t, list.len(q) == 0)\n}\n'
        if module == 'graph':
            graph += body
        else:
            maze += body
    return dict(graph=graph, maze=maze)


class Model(unittest.TestCase):
    def test_independent_sorted_frontier_and_repeated_relaxation(self):
        repeated = False
        for adj in graph_cases():
            h = lambda v: len(adj)-1-v
            result = model(adj, 0, len(adj)-1, h)
            self.assertEqual(result, model(adj, 0, len(adj)-1, h, True))
            repeated |= any(old != INF for _, old, _ in result[3])
        self.assertTrue(repeated)
        adj = graph_cases()[4]
        self.assertEqual(model(adj, 0, 9, lambda v: 9-v)[:2],
                         (4, [0, 8, 7, 6, 9]))

    def test_maze_path_tie_and_midpoint(self):
        for w, h, walkable, reverse in maze_cases():
            adj, _ = maze_adj(w, h, walkable, reverse)
            heuristic = lambda v: h-2-v//w+w-2-v%w
            self.assertEqual(model(adj, w+1, (h-2)*w+w-2, heuristic),
                             model(adj, w+1, (h-2)*w+w-2, heuristic, True))
        adj, _ = maze_adj(5, 5, set(range(25)), False)
        _, path, _, _ = model(adj, 6, 18, lambda v: 6-v//5-v%5)
        self.assertEqual(path, [6, 7, 8, 13, 18])
        self.assertEqual(path[len(path)//2], 8)


class Native(unittest.TestCase):
    def run_backend(self, backend):
        cli = os.environ.get('HIRSEL') or str(ROOT / 'buck-out/tmp/cargo-bin-exe/gate-langarena-astar-9/hirsel')
        with tempfile.TemporaryDirectory(prefix='la-astar-9-') as directory:
            d = Path(directory)
            for module, tests in fixture_sources().items():
                (d / f'{module}.hir').write_text((HERE / f'{module}.hir').read_text() + tests)
            (d / 'harness.hir').write_bytes((HERE / 'harness.hir').read_bytes())
            (d / 'main.hir').write_text('pub fn main() {}\n')
            manifest = dict(schema=1, package=dict(name='la_astar_9', version='1'),
                entry=dict(module=[], name='main'),
                sources=[dict(path=f'{m}.hir', module=[] if m == 'main' else [m])
                         for m in ['main', 'graph', 'maze', 'harness']],
                dependencies=[dict(manifest=str(ROOT/'stdlib/std@1/hirsel.json'),
                                  package=dict(name='std', version='1'), expose=[])])
            (d/'hirsel.json').write_text(json.dumps(manifest))
            command = [cli, 'test']
            if backend == 'h':
                command += ['--max-steps', 'unbounded']
            else:
                command += ['--native', '--backend', backend]
            result = subprocess.run(command + [str(d/'hirsel.json'), 'la_astar_9_'],
                                    capture_output=True, text=True)
            output = result.stdout + result.stderr
            print(output, flush=True)
            self.assertEqual(result.returncode, 0, output)
            self.assertIn('53 passed', output)

    def test_h(self):
        self.run_backend('h')

    def test_llvm(self):
        self.run_backend('llvm')

    def test_cranelift(self):
        self.run_backend('cranelift')


if __name__ == '__main__':
    unittest.main(verbosity=2)

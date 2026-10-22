#!/usr/bin/env python3
"""Regenerate independent Graph goldens using pinned LangArena Rust methods.

Only add pop observers and a wrapper; the algorithm bodies are upstream source.
MIT notice: bench/langarena/LICENSE. The first 45 fixtures retain tests_astar_9.
Run with --upstream /path/to/LangArena at the exact recorded revision.
"""
from pathlib import Path
import json,subprocess,sys,hashlib
import argparse, tempfile
parser=argparse.ArgumentParser(description=__doc__)
parser.add_argument('--upstream',type=Path,required=True)
args=parser.parse_args()
r=Path(__file__).resolve().parents[5]
work=tempfile.TemporaryDirectory(prefix='la-graph-rust-oracle-')
d=Path(work.name)
reference=args.upstream.resolve()
assert subprocess.check_output(['git','rev-parse','HEAD'],cwd=reference,text=True).strip()=='3c9f84ddaeec834c0631747cb789e8cfd602f993'
assert hashlib.sha256((reference/'rust/src/benchmarks/graph_path.rs').read_bytes()).hexdigest()=='fbf24fbda41d09a1848095c717d6e1e00c86ac52d67157b3b21d97d6b9a6f0ce'
assert hashlib.sha256((reference/'rust/src/helper.rs').read_bytes()).hexdigest()=='d264c471a03abd5cc4e1fcc96bd8ac31a2a1ff223c6d9d26e45f1f7d698eb7c9'
sys.path.insert(0,str(r/'bench/langarena'));import tests_astar_9
cases=tests_astar_9.graph_cases()+[[[0,1,1],[1,0,2,2],[2]],[[2,1],[3],[3],[]],[[1,1,0],[2,0,0],[1,3],[2,3]],[[1],[0],[],[]]]
generations=[(n,j,l) for n,j,l in [(1,3,100),(2,3,100),(5,3,10),(12,6,20),(100,3,100)]]
up=(reference/'rust/src/benchmarks/graph_path.rs').read_text()
up=up.replace('while let Some((v, dist)) = queue.pop_front() {','while let Some((v, dist)) = queue.pop_front() { trace(v);')
up=up.replace('while let Some((v, dist)) = stack.pop() {','while let Some((v, dist)) = stack.pop() { trace(v);')
up=up.replace('while let Some(Reverse(current)) = open_set.pop() {','while let Some(Reverse(current)) = open_set.pop() { trace(current.vertex);')
prefix='''#![allow(dead_code)]
fn config_i64(_: &str, _: &str) -> i64 { unreachable!() }
trait Benchmark { fn name(&self)->String; fn prepare(&mut self); fn run(&mut self,i:i64); fn checksum(&self)->u32; }
mod helper { include!("HELPER"); }
mod benchmarks { pub mod graph {
thread_local! { static TRACE: std::cell::RefCell<Vec<i32>> = const { std::cell::RefCell::new(Vec::new()) }; }
fn trace(v:i32) { TRACE.with(|t| t.borrow_mut().push(v)); }
'''.replace('HELPER',str(reference/'rust/src/helper.rs'))
post='''
pub fn run(adj:Vec<Vec<i32>>, generated:Option<(i32,i32,i32)>) {
 let seed=helper::last();
 let mut results=Vec::new(); let mut traces=Vec::new();
 for mode in 0..3 {
  TRACE.with(|t|t.borrow_mut().clear());
  let n=adj.len() as i32;
  let base=GraphPathBenchmark { graph:Graph{vertices:n,jumps:0,jump_len:0,adj:adj.clone()},result_val:0,prepared:true};
  let result=match mode {
   0=> GraphPathBFS{base}.bfs_shortest_path(0,n-1),
   1=> GraphPathDFS{base}.dfs_find_path(0,n-1),
   _=> GraphPathAStar{base}.a_star_shortest_path(0,n-1),
  };
  results.push(result); traces.push(TRACE.with(|t| t.borrow().clone()));
 }
 println!("{{\\\"adj\\\":{:?},\\\"results\\\":{:?},\\\"traces\\\":{:?},\\\"seed\\\":{},\\\"generated\\\":{:?}}}",adj,results,traces,seed,generated.map(|(n,j,l)|vec![n,j,l]).unwrap_or_default());
}
pub fn generated(n:i32,j:i32,l:i32){helper::reset();let mut g=Graph::new(n,j,l);g.generate_random();run(g.adj,Some((n,j,l)));}
}}
fn main() {
'''
for adj in cases:
 post+='benchmarks::graph::run(vec!['+','.join('vec!['+','.join(map(str,row))+']' for row in adj)+'],None);\n'
for n,j,l in generations:post+=f'benchmarks::graph::generated({n},{j},{l});\n'
post+='}\n'
(d/'oracle.rs').write_text(prefix+up+post)
subprocess.run(['rustup','run','1.99.0','rustc','--edition=2024','-O',str(d/'oracle.rs'),'-o',str(d/'oracle')],check=True)
result=subprocess.check_output([str(d/'oracle')],text=True)
(d/'oracle-output.jsonl').write_text(result)
vectors=[json.loads(line) for line in result.splitlines()]
(r/'crates/hirsel-cli/tests/support/la_graph_rss_10').mkdir(exist_ok=True)
(r/'crates/hirsel-cli/tests/support/la_graph_rss_10/vectors.json').write_text(json.dumps(vectors,indent=2)+'\n')
print(len(vectors),'cases',sum(len(v['traces'][0]) for v in vectors),'BFS pops')

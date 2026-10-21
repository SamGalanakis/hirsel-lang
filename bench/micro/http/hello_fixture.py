#!/usr/bin/env python3
"""Bound the production hello-web workload for instruction measurements."""
import argparse
import json
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]
parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('output', type=Path)
parser.add_argument('--source', type=Path, default=ROOT / 'examples/hello-web/main.hir')
parser.add_argument('--stdlib', type=Path, default=ROOT / 'stdlib/std@1/hirsel.json')
args = parser.parse_args()
source = args.source.read_text()
source = source.replace('Network(NetError) | Http(http.HttpError)',
                        'Network(NetError) | Http(http.HttpError) | Output(OutputError)')
source = source.replace('while true {', 'for _ in 0u64..1024u64 {')
source = source.replace('port: 8080u16', 'port: 18477u16')
source = source.replace('    serve(tasks, n, mut listener)?\n  }',
                        '    serve(tasks, n, mut listener)?\n  }\n'
                        '  out.print(io.stdout(rt)?, "1024\\n")?')
if 'port: 18477u16' not in source or '1024\\n' not in source:
    raise ValueError('hello-web shape changed; update this fixture generator')
args.output.parent.mkdir(parents=True, exist_ok=True)
args.output.write_text(source)
args.output.with_suffix('.hir.json').write_text(json.dumps({
    'schema': 1, 'package': {'name': 'hello_micro', 'version': '1'},
    'sources': [{'path': args.output.name, 'module': []}],
    'dependencies': [{'manifest': str(args.stdlib.resolve()),
                      'package': {'name': 'std', 'version': '1'}, 'expose': []}],
    'entry': {'module': [], 'name': 'main'},
}, indent=2) + '\n')

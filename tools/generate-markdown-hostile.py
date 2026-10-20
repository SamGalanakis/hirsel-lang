#!/usr/bin/env python3
"""Port pinned regression, pathological and safety vectors into bounded programs.

Run after generate-markdown.py with the same two checkouts and cmark shared build.
Repeated pathological families use 12/24 repetitions (title escapes 10/20/30);
cmark uses 24, lists 12, backtick runs 24, and 48 reference hash collisions.
The upstream sizes exceed the corpus interpreter's 1M-step limit. The recipes,
character patterns and expected-output assertions are retained, including both
sizes of each JS family; each resulting program has its own step/time bound.
"""
import argparse
import importlib.util
import itertools
import json
import re
import subprocess
from pathlib import Path

spec = importlib.util.spec_from_file_location('markdown_vectors', Path(__file__).with_name('generate-markdown.py'))
base = importlib.util.module_from_spec(spec)
spec.loader.exec_module(base)
ROOT = base.ROOT


def js_value(checkout, source, expression):
    script = "const vm=require('vm'); const c={repeat:(s,n)=>s.repeat(n)}; vm.runInNewContext(" + json.dumps(source) + ",c); process.stdout.write(JSON.stringify(" + expression + "));"
    return json.loads(subprocess.check_output(['node', '-e', script], cwd=checkout, text=True))


def renderer(checkout):
    script = "import fs from 'node:fs'; import {Parser,HtmlRenderer} from './lib/index.js'; const p=new Parser(),r=new HtmlRenderer(); process.stdout.write(JSON.stringify(JSON.parse(fs.readFileSync(0,'utf8')).map(s=>r.render(p.parse(s)))));"

    def render(sources):
        return json.loads(subprocess.check_output(['node', '--input-type=module', '-e', script], cwd=checkout, input=json.dumps(sources), text=True))
    return render


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('commonmark', type=Path)
    parser.add_argument('cmark_gfm', type=Path)
    parser.add_argument('--cmark-build', type=Path, required=True)
    parser.add_argument('--hirsel', type=Path, help='format the emitted corpus programs')
    args = parser.parse_args()
    for path, expected in ((args.commonmark, base.COMMONMARK), (args.cmark_gfm, base.CMARK_GFM)):
        assert subprocess.check_output(['git', '-C', str(path), 'rev-parse', 'HEAD'], text=True).strip() == expected
    js_render = renderer(args.commonmark)
    gfm_render = base.gfm_renderer(args.cmark_build)
    vectors = []
    origins = {}
    configs = {}

    def example_lines(path):
        return [i for i, line in enumerate(path.read_bytes().decode().split('\n'), 1)
                if re.match(r'^`{32} example', line)]
    # Each tuple records suite, ordinal, source, expected, GFM, safe rendering.
    for flavor, checkout, count in [('js', args.commonmark, 47), ('cmark', args.cmark_gfm, 26)]:
        cases = list(base.examples(checkout / 'test/regression.txt'))
        assert len(cases) == count
        starts = example_lines(checkout / 'test/regression.txt')
        flags = [line.split('example', 1)[1].split() for line in (checkout / 'test/regression.txt').read_text().splitlines() if re.match(r'^`{32} example', line)]
        for _, number, source, expected in cases:
            origins[(f'{flavor}-regression', number)] = f'{flavor} test/regression.txt:{starts[number - 1]}'
            configs[(f'{flavor}-regression', number)] = set(flags[number - 1])
            vectors.append((f'{flavor}-regression', number, source, expected, False, False))
    test = (args.commonmark / 'test/test.js').read_text()
    recipes = test[test.index('var cases = ['):test.index('var parse_and_render')]
    # Keep the two-size structure of all 14 repeated families, plus 2 literals.
    recipes = recipes.replace('x = 1000; x <= 10000; x *= 10', 'x = 12; x <= 24; x *= 2')
    recipes = recipes.replace('x = 10; x <= 1000; x *= 10', 'x = 10; x <= 30; x += 10')
    js_cases = js_value(args.commonmark, recipes, 'c.cases')
    assert len(js_cases) == 31
    for number, case in enumerate(js_cases, 1):
        origins[('js-pathological', number)] = f'commonmark.js test/test.js:178-300; {case["name"]}'
        vectors.append(('js-pathological', number, case['input'], case['expected'], False, False))
    # Execute the pinned Python recipes, including its actual collision generator.
    code = (args.cmark_gfm / 'test/pathological_tests.py').read_text()
    code = code[code.index('def hash_collisions'):code.index('whitespace_re =')]
    sizes = {65000: 24, 64999: 23, 50000: 24, 49999: 23, 80000: 24,
             160000: 24, 1000: 12, 999: 11, 5000: 24, 30000: 24,
             300000: 24, 29999: 23}
    code = re.sub(r'\b(' + '|'.join(map(str, sizes)) + r')\b', lambda m: str(sizes[int(m[0])]), code)
    code = code.replace('COUNT = 24', 'COUNT = 48')
    scope = {'re': re, 'itertools': itertools}
    exec(compile(code, str(args.cmark_gfm / 'test/pathological_tests.py'), 'exec'), scope)
    assert len(scope['pathological']) == 21
    for number, (name, (source, pattern)) in enumerate(scope['pathological'].items(), 1):
        origins[('cmark-pathological', number)] = f'cmark-gfm test/pathological_tests.py:14-121; {name}'
        expected = gfm_render(source)
        assert pattern.search(expected), f'upstream pathological oracle: {name}: {expected!r}'
        vectors.append(('cmark-pathological', number, source, expected, True, False))
    safe = test[test.index('var safeCases = ['):test.index('var safe_parse_and_render')]
    for number, case in enumerate(js_value(args.commonmark, safe, 'c.safeCases'), 1):
        origins[('safe', number)] = f'commonmark.js test/test.js:322-370; {case["name"]}'
        vectors.append(('safe', number, case['input'], case['expected'], False, True))
    for section, number, source, expected in base.examples(args.cmark_gfm / 'test/spec.txt'):
        if section == 'Disallowed Raw HTML (extension)':
            origins[('gfm-tagfilter', number)] = 'cmark-gfm test/spec.txt:9647'
            vectors.append(('gfm-tagfilter', number, source, expected, True, False))
    # Ordinary regression/pathological twins use independent reference output.
    paths = []
    for suite, number, source, expected, gfm, safe in vectors:
        twin = source[:len(source) // 2] + '\0[&unterminated'
        flags = configs.get((suite, number), set())
        if suite == 'cmark-regression':
            oracle = base.gfm_renderer(args.cmark_build, footnotes='footnotes' in flags, extensions=tuple(flag.encode() for flag in flags if flag != 'footnotes'))
            assert oracle(source) == expected, f'cmark regression oracle mismatch: {number}'
            twin_expected = oracle(twin)
        else:
            twin_expected = gfm_render(twin) if gfm else js_render([twin])[0]
        stem = f'SEM-3-markdown-{suite}-{number}-run'
        lines = ['// rule: SEM-3', '// expect: run', '// Pinned upstream vectors; tools/generate-markdown-hostile.py.',
                 '// ' + origins[(suite, number)],
                 '// Each program is bounded by the corpus step budget and runner timeout.',
                 'fn grant_error(take e: GrantError) -> () {}', '',
                 'pub fn main(rt: Io) -> Result[(), ()] {', '  let s = io.stdout(rt) ? grant_error',
                 '  let mut failed: u64 = 0',
                 '  let mut options = std@1/markdown.' + ('gfm_options()' if gfm else 'default_options()')]
        fields = {'footnotes': 'footnotes', 'table': 'tables', 'strikethrough': 'strikethrough', 'autolink': 'autolinks', 'tagfilter': 'tagfilter'}
        for flag in sorted(flags):
            lines.append(f'  options.{fields[flag]} = true')
        for label, inp, out, method in [(suite, source, expected, 'check_safe' if safe else 'check')]:
            lines.append(f'  failed += harness.{method}(s, {base.literal(label + "/" + str(number))}, {base.literal(inp)}, {base.literal(out)}, options)')
        # Safe twins have a different oracle; safety-specific edge cases are
        # covered separately, while every parser regression gets a malformed twin.
        if not safe:
            lines.append(f'  failed += harness.check(s, {base.literal(suite + "/" + str(number) + "/malformed")}, {base.literal(twin)}, {base.literal(twin_expected)}, options)')
        lines += ['  failure.check(failed == 0, TextInvariant())', '  Ok(())', '}', '']
        path = ROOT / 'conformance/SEM' / (stem + '.hir')
        path.write_text('\n'.join(lines))
        manifest = {'schema': 1, 'package': {'name': 'application', 'version': '1'}, 'entry': {'module': [], 'name': 'main'}, 'sources': [{'path': stem + '.hir', 'module': []}, {'path': '../../packages/fixtures/markdown_vectors@1/harness.hir', 'module': ['harness']}], 'dependencies': [{'manifest': '../../stdlib/std@1/hirsel.json', 'package': {'name': 'std', 'version': '1'}, 'expose': []}]}
        path.with_suffix('.hir.json').write_text(json.dumps(manifest, indent=2) + '\n')
        paths.append(path.relative_to(ROOT).as_posix())
    passing = ROOT / 'conformance/passing.txt'
    if passing.exists():
        entries = set(passing.read_text().splitlines())
        entries.update(paths)
        passing.write_text('\n'.join(sorted(entries)) + '\n')
    if args.hirsel:
        for path in paths:
            subprocess.run([str(args.hirsel.resolve()), 'fmt', path], cwd=ROOT, check=True)
    from collections import Counter
    print(dict(Counter(v[0] for v in vectors)))
    print(f'{len(vectors)} vectors in {len(paths)} bounded programs with {sum(not v[5] for v in vectors)} malformed twins')


if __name__ == '__main__':
    main()

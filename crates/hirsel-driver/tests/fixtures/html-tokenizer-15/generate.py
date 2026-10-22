#!/usr/bin/env python3
"""Reproduce embedded tables from the pinned golang.org/x/net checkout.
Usage: python3 generate.py /path/to/x-net
The hand-written scanner before the marker is preserved.
"""
import hashlib
from pathlib import Path
import re
import struct
import sys

reference = Path(sys.argv[1])
PIN = '7e562009b3884a0845019b5f886f0242a7c0b970'
import subprocess
assert subprocess.check_output(['git', '-C', str(reference), 'rev-parse', 'HEAD'], text=True).strip() == PIN
for relative in ['html/token.go', 'html/token_test.go', 'html/escape.go', 'html/entity.go', 'LICENSE']:
    assert (reference / relative).read_bytes() == subprocess.check_output(['git', '-C', str(reference), 'show', PIN + ':' + relative])
source = (reference / 'html/entity.go').read_bytes()
assert source == subprocess.check_output(['git', '-C', str(reference), 'show', PIN + ':html/entity.go'])
entries = []
for line in source.decode().splitlines():
    match = re.match(r'\s*"([A-Za-z0-9;]+)":\s*(.*),', line)
    if not match:
        continue
    scalars = [int(n, 16) for n in re.findall(r"'\\[uU]([0-9a-fA-F]+)'", match[2])]
    assert 1 <= len(scalars) <= 2, line
    entries.append((match[1].encode(), scalars + [0] * (2 - len(scalars))))
assert len(entries) == 2231
names = bytearray()
index = bytearray()
for name, scalars in sorted(entries):
    index.extend(struct.pack('<IIII', len(names), len(name), *scalars))
    names.extend(name)

def constant(name, value):
    # One literal per table, as used by other std Unicode tables.
    return 'const ' + name + ': Bytes = b"' + ''.join('\\x%02x' % b for b in value) + '"\n'

root = Path(__file__).resolve().parents[5]
target = root / 'stdlib/std@1/html_tokenizer.hir'
marker = '// BEGIN PINNED ENTITY TABLES\n'
text = target.read_text().split(marker)[0] + marker
text += '// entity.go sha256: ' + hashlib.sha256(source).hexdigest() + '\n'
text += constant('ENTITY_NAMES', names) + '\n' + constant('ENTITY_INDEX', index)
target.write_text(text)
(reference / 'LICENSE').read_bytes()
Path(__file__).with_name('GO-LICENSE.txt').write_bytes((reference / 'LICENSE').read_bytes())
print('Generated 2231 named references (including 93 two-scalar references).')

if len(sys.argv) > 2:
    import json
    import tempfile
    import shutil

    extras = []
    def case(name, source, cdata=False, suppress=False):
        extras.append(dict(Name=name, Source=source, CDATA=cdata, SuppressRaw=suppress))
    for source in [
        '<p a=/><p a=//><p a="/"/><p a=x a=/><p a=x a=""/>',
        '<P A="first" a="second" B=three></P ignored=1>',
        '<p a\0=1 a�=2 A\0=3>\0</p>',
        'é😀\r\n雪\r<İ Aİ=I></İ>',
        '&#0; &#xD800; &#1114112; &#x110000; &#99999999999999999999999; &#x80; &#x9F; &#13; &#x;',
        '&notit; &notin; &ampx &amp= &copy &NotEqualTilde; &acE; &nparsl; &CounterClockwiseContourIntegral;',
        '<a x="&notit; &notin; &ampx &amp= &amp;= &copy &copy! &copy= &#13;">',
        '<!--&amp;\0\r\n--><!DOCTYPE html PUBLIC "&amp;\0\r\n">',
        '<![CDATA[&amp;\0\r\n<tag>]]>after',
        '<script><!--<script>inner</script>still--></script><p>end',
        '<script><!--<ScRiPt/>inner</script >still--></script>',
        '<script><!--<x>--!< /script>--></script>',
        '<script/>x</script><style/>x</style>',
        '<style>\0\r\n&amp;<p></STYLE><textarea>\0\r\n&amp;<p></textarea>',
        '<title>&notit;<b></title><iframe>x</iframe><noembed>y</noembed><noframes>z</noframes>',
        '<noscript>&amp;<p></noscript><xmp>x</xmp><plaintext>\0&amp;<b></plaintext>',
    ]:
        for cdata in [False, True]:
            case('additional', source, cdata)
    case('foreign raw override', '<svg><title><b>x</b></title></svg>', True, True)
    # Every byte prefix observes EOF in each declaration, attribute and script
    # state, including escapes and double escapes. Selected from upstream's
    # named state structure, not generated from the Hirsel implementation.
    for source in [
        '<p abc="quoted" single=\'x\' unquoted=y/>',
        '<!DOCTYPE html>', '<![CDATA[x]]>', '<!--x--!>',
        '<script><!--<script >x</script >--></script>',
        '<script><!--<script>--<x>-x</script>--></script>',
        '<style>x</style>', '<textarea>&amp;</textarea>',
    ]:
        for end in range(len(source) + 1):
            case('EOF prefix', source[:end], True)
    # Exercise every entry of both pinned entity maps in one text token.
    case('all 2231 named references', '|'.join('&'+n.decode()+'|' for n, _ in sorted(entries)))
    with tempfile.TemporaryDirectory(prefix='html-tokenizer-oracle-', dir=Path(__file__).resolve().parents[6]) as tmp:
        tmp = Path(tmp)
        (tmp/'html').mkdir()
        for path in (reference/'html').glob('*.go'):
            if not path.name.endswith('_test.go'):
                shutil.copy(path, tmp/'html'/path.name)
        shutil.copytree(reference/'html/atom', tmp/'html/atom')
        (tmp/'go.mod').write_text('module golang.org/x/net\n\ngo 1.25.0\n')
        shutil.copy(Path(__file__).with_name('oracle.go'), tmp/'main.go')
        result = subprocess.run([sys.argv[2], 'run', '.', str(reference/'html/token_test.go')],
            cwd=tmp, input=json.dumps(extras), text=True, capture_output=True, check=True)
    cases = json.loads(result.stdout)
    def literal(value):
        return json.dumps(value, ensure_ascii=False).replace('\\u0000', '\\u{0}').replace('\\b', '\\u{8}').replace('\\f', '\\u{c}').replace('\u200e', '\\u{200e}').replace('\u200f', '\\u{200f}')
    body = ['// Reproduced by generate.py from pinned Go x/net tokenTests and oracle.go.',
            '// Copyright 2010 The Go Authors; BSD-3-Clause (GO-LICENSE.txt).',
            '@track_caller', 'fn check(take source: String, cdata: bool, suppress: bool, expected: List[std@1/html_tokenizer.Token]) -> () uses {} {',
            '  let limits = std@1/html_tokenizer.Limits(bytes: 100000, tokens: 10000, attributes: 10000, work: 10000000)',
            '  let mut Ok(z) = std@1/html_tokenizer.tokenizer(take source, limits) else {',
            '    core@1/failure.check(false, TextInvariant()); return', '  }',
            '  let mut actual: List[std@1/html_tokenizer.Token] = []',
            '  while true {',
            '    if suppress { std@1/html_tokenizer.next_is_not_raw_text(mut z) }',
            '    match std@1/html_tokenizer.next(mut z, cdata) {',
            '      Ok(Some(t)) => core@1/list.push(mut actual, take t),',
            '      Ok(None()) => break,',
            '      Error(_) => { core@1/failure.check(false, TextInvariant()); return },',
            '    }', '  }',
            '  core@1/failure.check(actual == expected, TextInvariant())',
            '  core@1/failure.check(std@1/html_tokenizer.next(mut z, cdata) == Ok(None()), TextInvariant())',
            '}', '', 'pub fn main(rt: Io, args: Args) -> Result[(), ()] {']
    for i, c in enumerate(cases):
        body.append('  // %d: %s' % (i, c['Name']))
        tokens = []
        for t in c['Tokens'] or []:
            attrs = ', '.join('std@1/html_tokenizer.Attribute(key: %s, value: %s)' % (literal(a['Key']), literal(a['Val'])) for a in t['Attr'] or [])
            tokens.append('std@1/html_tokenizer.Token(kind: %s(), span: std@1/html_tokenizer.Span(start: %d, end: %d), data: %s, attributes: [%s])' % (t['Kind'],t['Start'],t['End'],literal(t['Data']),attrs))
        body.append('  check(%s, %s, %s, [%s])' % (literal(c['Source']),str(c['CDATA']).lower(),str(c['SuppressRaw']).lower(), ', '.join(tokens)))
    body += ['  caps()', '  Ok(())', '}']
    Path(__file__).with_name('vectors.hir').write_text('\n'.join(body)+'\n'+Path(__file__).with_name('caps.hir').read_text())
    print('Generated %d exact token/value/span cases, including %d upstream vectors.' % (len(cases), sum(c['Name'].startswith('upstream/') for c in cases)))

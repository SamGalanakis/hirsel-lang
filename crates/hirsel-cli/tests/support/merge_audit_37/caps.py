"""Original boundary data; never substitute smaller companions for these."""
import json


def vectors():
    # Each entry retains exact bytes, with no whitespace tokenization.
    yield dict(id='source-exact', kind='source', text='x'*32768, accepted=True)
    yield dict(id='source-over', kind='source', text='x'*32769, accepted=False)
    yield dict(id='lines-exact', kind='source', text='x\n'*256, accepted=True)
    yield dict(id='lines-over', kind='source', text='x\n'*257, accepted=False)
    yield dict(id='unicode-byte-over', kind='source', text='羊'*10923, accepted=False)
    for n in (131072, 131073):
        yield dict(id=f'total-{n}', kind='total', sources=['x'*32768]*4+(['x'] if n>131072 else []), accepted=n==131072)
    for n in (4,5):
        yield dict(id=f'files-{n}', kind='manifest', files=[dict(name=f'{i}.txt',base='b',left='l',right='r') for i in range(n)],accepted=n==4)
    for n in (16384,16385):
        yield dict(id=f'decisions-{n}', kind='decisions-bytes', text=' '*n,accepted=n==16384)
    yield dict(id='conflicts-128',kind='merge',base=''.join('x\n'+('a\n' if i<127 else '') for i in range(128)),left=''.join('L\n'+('a\n' if i<127 else '') for i in range(128)),right=''.join('R\n'+('a\n' if i<127 else '') for i in range(128)),accepted=True)
    # Corrected reachability: 128 anchors + 128 changed base lines, with
    # alternating delete/modify sides and one different leading insertion.
    base = ''.join(f'a{i}\nx{i}\n' for i in range(128))
    left = 'start-L\n' + ''.join(f'a{i}\n' + (f'L{i}\n' if i % 2 == 0 else '') for i in range(128))
    right = 'start-R\n' + ''.join(f'a{i}\n' + (f'R{i}\n' if i % 2 else '') for i in range(128))
    yield dict(id='conflicts-129', kind='merge', base=base, left=left, right=right, accepted=False)
    # Retain the original helper vectors too. The resolved-text cap alone remains
    # unreachable; fully escaped plan reports can reach the output boundary.
    for name, extra in [('aa', 0), ('aaa', 1)]:
        yield dict(id='report-exact' if not extra else 'report-over', kind='report', name=name, source='\0'*14550+'xxx', source_bytes=14553, source_total=43659, expected_report_bytes=262144+extra, accepted=not extra)
    for n in (128,129):
        yield dict(id=f'conflict-helper-{n}',kind='conflicts',count=n,accepted=n==128)
    for n in (262144,262145):
        yield dict(id=f'output-helper-{n}',kind='output',bytes=n,accepted=n==262144)


if __name__ == '__main__':
    print(json.dumps(list(vectors()),ensure_ascii=False,indent=2))

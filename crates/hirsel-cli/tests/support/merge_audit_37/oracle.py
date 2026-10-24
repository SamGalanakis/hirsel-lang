"""Independent DP edit oracle and node-diff3 overlap port (MIT, see app NOTICE).

DP deliberately chooses its own valid LCS alignment. Sweep qualification feeds
actual canonical std.diff hunks separately; no assertion of unique alignment.
"""
from itertools import product


def lcs(a, b):
    d = [[0] * (len(b) + 1) for _ in range(len(a) + 1)]
    for i in range(len(a) - 1, -1, -1):
        for j in range(len(b) - 1, -1, -1):
            d[i][j] = 1 + d[i+1][j+1] if a[i] == b[j] else max(d[i+1][j], d[i][j+1])
    return d


def hunks(o, a, side):
    d = lcs(o, a)
    i = j = 0
    result = []
    while i < len(o) or j < len(a):
        if i < len(o) and j < len(a) and o[i] == a[j]:
            i += 1
            j += 1
            continue
        s, t = i, j
        while i < len(o) or j < len(a):
            if i < len(o) and j < len(a) and o[i] == a[j]:
                break
            if i < len(o) and (j == len(a) or d[i+1][j] >= d[i][j+1]):
                i += 1
            else:
                j += 1
        result.append([side, s, i-s, t, j-t])
    return result


def sweep(o, a, b, hs):
    # Independent direct port diff3.mjs:208-319, with full coverage spans.
    hs = sorted(enumerate(hs), key=lambda h: (h[1][1], h[1][0], h[0]))
    hs = [h for _, h in hs]
    result = []
    p = x = y = 0
    while hs:
        h = hs.pop(0)
        start, end = h[1], h[1] + h[2]
        group = [h]
        if start > p:
            n = start-p
            result.append(["base", p, n, x, n, y, n])
            x += n
            y += n
        while hs and hs[0][1] <= end:
            h = hs.pop(0)
            end = max(end, h[1]+h[2])
            group.append(h)
        spans = []
        for side, cursor in [(0, x), (1, y)]:
            own = [h for h in group if h[0] == side]
            if own:
                lo = min(h[3] for h in own) - (min(h[1] for h in own)-start)
                hi = max(h[3]+h[4] for h in own) + end-max(h[1]+h[2] for h in own)
                assert lo == cursor
                spans.extend([lo, hi-lo])
            else:
                spans.extend([cursor, end-start])
        x0, xn, y0, yn = spans
        sides = set(h[0] for h in group)
        kind = 'left' if sides == {0} else 'right' if sides == {1} else 'identical' if a[x0:x0+xn] == b[y0:y0+yn] else 'conflict'
        result.append([kind, start, end-start, *spans])
        p, x, y = end, x0+xn, y0+yn
    n = len(o)-p
    if n:
        result.append(['base', p, n, x, n, y, n])
        x += n
        y += n
    assert x == len(a) and y == len(b)
    return result


def vectors():
    rows = [
        ('identity', 'a\nb\n', 'a\nb\n', 'a\nb\n'),
        ('left', 'a\nb\n', 'a\nx\n', 'a\nb\n'),
        ('right', 'a\nb\n', 'a\nb\n', 'a\nx\n'),
        ('identical', 'a\nb\n', 'a\nx\n', 'a\nx\n'),
        ('empty', '', 'a\n', 'b\n'),
        ('insert-start', 'a\n', 'x\na\n', 'y\na\n'),
        ('insert-end', 'a\n', 'a\nx\n', 'a\ny\n'),
        ('adjacent', 'a\nb\nc\n', 'b\nc\n', 'a\nc\n'),
        ('overlap', 'a\nb\nc\n', 'c\n', 'a\n'),
        ('transitive', 'a\nb\nc\nd\ne\n', 'x\nb\ny\ne\n', 'a\nz\ne\n'),
        ('repeated', 'a\na\nb\na\n', 'a\nb\na\n', 'a\na\nx\na\n'),
        ('crlf', 'a\r\nb\r\n', 'a\r\nx\r\n', 'a\r\ny\r\n'),
        ('no-final-lf', 'a\nb', 'a\nx', 'a\ny'),
        ('unicode', '羊\nβ\n', '羊\n猫\n', '羊\n犬\n'),
        ('upstream-regions', 'AA\nZZ\n00\nM\n99\n', 'AA\na\nb\nc\nZZ\nnew\n00\na\na\nM\n99\n', 'AA\na\nd\nc\nZZ\n11\nM\nz\nz\n99\n'),
    ]
    for name, o, a, b in rows:
        tokens = [s.splitlines(keepends=True) for s in (o,a,b)]
        hs = hunks(tokens[0],tokens[1],0)+hunks(tokens[0],tokens[2],1)
        yield dict(id=name, base=o, left=a, right=b, hunks=hs, regions=sweep(*tokens,hs))


if __name__ == '__main__':
    import json
    print(json.dumps(list(vectors()), ensure_ascii=False, indent=2))

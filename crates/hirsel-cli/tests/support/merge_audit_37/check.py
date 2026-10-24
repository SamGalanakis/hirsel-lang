"""Verify actual std canonical hunks and sweep against independent algorithms."""
import json
import sys
from oracle import lcs, sweep

rows = json.load(open(sys.argv[1]))
for row in rows:
    o, a, b = [row[key].splitlines(keepends=True) for key in ('base','left','right')]
    hs = row['hunks']
    for side, text in enumerate((a,b)):
        own = [h for h in hs if h[0] == side]
        i = j = edits = 0
        rebuilt = []
        for _, old, removed, new, inserted in own:
            assert old >= i and new >= j
            assert old-i == new-j and o[i:old] == text[j:new]
            rebuilt.extend(o[i:old])
            rebuilt.extend(text[new:new+inserted])
            i, j = old+removed, new+inserted
            edits += removed+inserted
        assert o[i:] == text[j:]
        rebuilt.extend(o[i:])
        assert rebuilt == text
        assert edits == len(o)+len(text)-2*lcs(o,text)[0][0]
    expected = sweep(o,a,b,hs)
    actual = [[r['kind'],*r['base'],*r['left'],*r['right']] for r in row['report']['files'][0]['regions']]
    assert actual == expected, (row['id'],actual,expected,hs)
    # Full coverage with no double consumption, including zero-length insertions.
    for key,tokens in [('base',o),('left',a),('right',b)]:
        p = 0
        for r in row['report']['files'][0]['regions']:
            start,n = r[key]
            assert start == p
            assert r[key+'_text'] == ''.join(tokens[p:p+n])
            p += n
        assert p == len(tokens)
    if o == a or o == b or a == b:
        merged = ''.join(r[('left' if r['kind'] in ('left','identical') else 'right' if r['kind']=='right' else 'base')+'_text'] for r in row['report']['files'][0]['regions'])
        assert merged == ''.join(b if o == a else a)
print(json.dumps({'verified':len(rows),'dp_minimality':2*len(rows),'same_hunk_sweeps':len(rows)}))

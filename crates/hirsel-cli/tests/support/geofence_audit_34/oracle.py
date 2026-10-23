import json,pathlib,hashlib
from fractions import Fraction
def p(x,y):return dict(x=x,y=y)
def ring(coords):return [p(*xy) for xy in coords+[coords[0]]]
def box(a,b,c,d):return ring([(a,b),(c,b),(c,d),(a,d)])
def region(outer,holes=[]):return dict(outer=outer,holes=holes)
def batch(rs,ps):return dict(version=1,regions=rs,tracks=[dict(samples=ps)])
# Independent rational ray parity: no winding accumulator and no orientation kernel.
def classify_ring(r,q):
 x,y=q['x'],q['y'];inside=False
 for a,b in zip(r,r[1:]):
  ax,ay,bx,by=a['x'],a['y'],b['x'],b['y']
  if (bx-ax)*(y-ay)==(by-ay)*(x-ax) and min(ax,bx)<=x<=max(ax,bx) and min(ay,by)<=y<=max(ay,by):return 'boundary'
  if (ay>y)!=(by>y):
   hit=Fraction(ax)+Fraction((y-ay)*(bx-ax),by-ay)
   if hit>x:inside=not inside
 return 'inside' if inside else 'outside'
def classify(r,q):
 c=classify_ring(r['outer'],q)
 if c!='inside':return c
 for h in r['holes']:
  c=classify_ring(h,q)
  if c=='boundary':return c
  if c=='inside':return 'outside'
 return 'inside'
def oracle(b):
 results=[]
 for t in b['tracks']:
  rs=[]
  for r in b['regions']:
   cs=[classify(r,q) for q in t['samples']]
   rs.append(dict(classes=cs,transitions=[i for i in range(1,len(cs)) if cs[i]!=cs[i-1]]))
  results.append(dict(regions=rs))
 return dict(version=1,results=results)

if __name__=='__main__':
 import sys
 rows=json.loads(pathlib.Path(sys.argv[1]).read_text());count=0
 for row in rows:
  if 'expected' not in row:continue
  b=json.loads(row['input'])
  expected=json.dumps(oracle(b),sort_keys=True,separators=(',',':'))+'\n'
  assert row['expected']==expected,row['id'];count+=1
 print(f'rational parity: {count} full application expected reports checked')

"""Freeze independent Decimal Taylor DFT intervals. Python stdlib only.
RustFFT 3bfe1afa dft.rs:284-299,327-359 supplies the two exact vectors.
Algorithm tolerances are fixed here before any Hirsel execution.
"""
from decimal import Decimal as D, localcontext
import json, pathlib
PI=D('3.1415926535897932384626433832795028841971693993751058209749445923078164062862089986280348253421170679')

def trig(x):
    x=(x+PI)%(2*PI)-PI
    s=t=x; c=u=D(1)
    for k in range(1,160):
        t=-t*x*x/D((2*k)*(2*k+1));u=-u*x*x/D((2*k-1)*(2*k))
        s+=t;c+=u
        if abs(t)<D('1e-85') and abs(u)<D('1e-85'):return c,s
    raise AssertionError('Taylor failed fixed 160 term bound')

def dft(xs,inverse=False):
    n=len(xs);out=[]
    for k in range(n):
        re=im=D(0)
        for j,(r,i) in enumerate(xs):
            c,s=trig(D(2)*PI*j*k/n*(1 if inverse else -1))
            re+=r*c-i*s;im+=r*s+i*c
        out.append((re/n,im/n) if inverse else (re,im))
    return out

def interval(x):
    e=D('1e-9')*(1+abs(x))
    return [str(x-e),str(x+e)]

def doc(channels,remove=False,top=8,band=None):
    n=len(channels[0]) if channels else 1
    return dict(channels=[dict(samples=[dict(real=r,imaginary=i) for r,i in ch]) for ch in channels],remove_mean=remove,top=top,band=band or dict(first=0,end=n))

def row(id,channels,**kw):return dict(id=id,input=json.dumps(doc(channels,**kw),separators=(',',':')))

def main():
    z=(0,0)
    rows=[row('rustfft-2',[[(1,0),(-1,0)]]),row('rustfft-4',[[(0,1),(2.5,-3),(-1,-1),(4,0)]]),row('impulse',[[(1,0),z,z,z]]),row('dc',[[(3,-2)]*4]),row('alternating',[[(1,0),(-1,0)]*2]),row('phase',[[(1,0),(0,1),(-1,0),(0,-1)]]),row('zero',[[z]*4]),row('mean',[[(3,-2)]*4],remove=True),row('length-1',[[(2,-3)]],top=1),row('length-32',[[(j-16,(j*7)%11-5) for j in range(32)]]),row('channels-4',[[(i,0)] for i in range(4)]),row('component-cap',[[(1000,-1000)]]),row('round-below',[[(0.000000499999,0)]]),row('round-above',[[(0.000000500001,0)]]),row('negative-zero',[[(-0.0,-0.0)]]),row('band',[[(1,0),(-1,0)]*2],band=dict(first=1,end=3))]
    rows[0]['exact']=[[0,0],[2,0]];rows[1]['exact']=[[5.5,-3],[-2,3.5],[-7.5,3],[4,0.5]]
    for r in rows:
        inp=json.loads(r['input'],parse_float=D,parse_int=D);r['oracle']=[]
        for channel in inp['channels']:
            xs=[(D(s['real']),D(s['imaginary'])) for s in channel['samples']];n=len(xs)
            mean=tuple(sum(v[i] for v in xs)/n for i in range(2)) if inp['remove_mean'] else (D(0),D(0))
            xs=[(r-mean[0],i-mean[1]) for r,i in xs];bins=dft(xs);back=dft(bins,True)
            en=sum(r*r+i*i for r,i in xs)
            assert max(max(abs(a-b) for a,b in zip(x,y)) for x,y in zip(xs,back))<D('1e-75')
            r['oracle'].append(dict(bins=[[interval(a),interval(b)] for a,b in bins],energy=interval(en)))
    base=doc([[(1,0)]])
    bad=[('channels-empty',doc([]),'channels'),('channels-over',doc([[(1,0)]]*5),'channels'),('samples-empty',doc([[]]),'samples'),('samples-over',doc([[z]*33]),'samples'),('unequal',doc([[z],[z,z]]),'samples'),('component-over',doc([[(1000.000001,0)]]),'component'),('top-zero',doc([[z]],top=0),'top'),('top-over',doc([[z]],top=9),'top'),('band-over',doc([[z]],band=dict(first=0,end=2)),'band'),('unknown',dict(base,extra=0),'schema'),('missing',dict(channels=[]),'schema'),('nan-string',doc([[('NaN',0)]]),'schema'),('infinity-string',doc([[('Infinity',0)]]),'schema')]
    rows += [dict(id=id,input=json.dumps(v,separators=(',',':')),error=e) for id,v,e in bad]
    rows += [dict(id='nan-token',input='{"channels":[NaN]}',error='json'),dict(id='infinity-token',input='{"channels":[Infinity]}',error='json'),dict(id='overflow',input=json.dumps(base).replace('"real": 1','"real": 1e9999'),error='component'),dict(id='malformed',input='[',error='json')]
    padded=json.dumps(base,separators=(',',':'))
    rows += [dict(id='input-cap',input=padded+' '*(32768-len(padded))),dict(id='input-over',input=padded+' '*(32769-len(padded)),error='input_limit')]
    rows[-2]['oracle']=rows[8]['oracle'] # replaced below with independently computed unit input
    rows[-2]['oracle']=[dict(bins=[[[str(D(1)-D('2e-9')),str(D(1)+D('2e-9'))],['-1e-9','1e-9']]],energy=[str(D(1)-D('2e-9')),str(D(1)+D('2e-9'))])]
    for id,fields in [('jobs-zero',dict(jobs=0,error='jobs')),('jobs-over',dict(jobs=5,error='jobs')),('denial',dict(access='read',error='file_denied')),('pending-cancel',dict(pending=True,error='canceled'))]:rows.append(dict(id=id,input=padded,**fields))
    out=pathlib.Path(__file__).with_name('cases.json');out.write_text(json.dumps(rows,indent=2)+'\n')
    kernel=['pub fn main(rt: Io, args: Args) -> Result[(), GrantError] {', '  let _ = args', '  let output = io.stdout(rt)?']
    for index,case in enumerate(rows):
        if 'oracle' not in case or case.get('error') or len(case['input'])>4096:continue
        literal=json.dumps(case['input'])
        kernel += [f'  let Ok(manifest{index}) = samples.parse(unique b{literal}) else {{ failure.check(false, Arithmetic()); return Ok(()) }}', f'  let Ok(table{index}) = transform.new(manifest{index}.length) else {{ failure.check(false, Arithmetic()); return Ok(()) }}']
        for ch,oracle in enumerate(case['oracle']):
            kernel += [f'  let Ok(audit{index}_{ch}) = analysis.channel(table{index}, manifest{index}.channels[{ch}], manifest{index}.remove_mean, manifest{index}.top, manifest{index}.first, manifest{index}.end) else {{ failure.check(false, Arithmetic()); return Ok(()) }}']
            energy=(D(oracle['energy'][0])+D(oracle['energy'][1]))/2
            tolerance=abs(energy)*D('1e-9') if energy else D('1e-9')
            for field in ['time_energy','frequency_energy']:
                kernel += [f'  failure.check(audit{index}_{ch}.checks.{field} >= {energy-tolerance}f64 && audit{index}_{ch}.checks.{field} <= {energy+tolerance}f64, Arithmetic())']
            for k,parts in enumerate(oracle['bins']):
                for field,bounds in zip(['real','imaginary'],parts):
                    lo,hi=bounds
                    kernel += [f'  failure.check(audit{index}_{ch}.spectrum.values[{k}].{field} >= {lo}f64 && audit{index}_{ch}.spectrum.values[{k}].{field} <= {hi}f64, Arithmetic())']
    kernel += ['  let _ = out.print(output, "kernel oracle ok\\n")', '  Ok(())', '}']
    out.with_name('kernel.hir').write_text('\n'.join(kernel)+'\n')
if __name__=='__main__':
    with localcontext() as c:c.prec=95;main()

# Additional per-vector controls preserve, rather than replace, the original
# aggregate kernel and its stopped execution identity. Reuse its exact checks.
if __name__ == '__main__':
    original=pathlib.Path(__file__).with_name('kernel.hir').read_text().splitlines()
    blocks=[]
    for line in original[3:-3]:
        if line.strip().startswith('let Ok(manifest'):
            blocks.append([])
        blocks[-1].append(line)
    rows=json.loads(pathlib.Path(__file__).with_name('cases.json').read_text())
    names=[r['id'] for r in rows if 'oracle' in r and not r.get('error') and len(r['input'])<=4096]
    assert len(names)==len(blocks)
    lines=['pub fn main(rt: Io, args: Args) -> Result[(), GrantError] {',
           '  let output = io.stdout(rt)?',
           '  let Ok(name) = arg.text(args, 0) else { failure.check(false, Arithmetic()); return Ok(()) }']
    for name,block in zip(names,blocks):
        lines += ['  if name == '+json.dumps(name)+' {']+['  '+line for line in block]
        lines += ['    let _ = out.print(output, "kernel oracle ok\\n")','    return Ok(())','  }']
    lines += ['  failure.check(false, Arithmetic())','  Ok(())','}']
    pathlib.Path(__file__).with_name('kernel_cases.hir').write_text('\n'.join(lines)+'\n')

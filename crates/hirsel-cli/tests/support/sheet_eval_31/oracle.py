"""Independent full recomputation oracle; no incremental graph/cache is shared.

tinyexpr 7ab30db4 smoke.c:47-87 integer applicable vectors (zlib).
CPython 010bebbf test_graphlib.py:41-130 shapes (PSF-2.0).
"""
import ast
import json
import re
import sys

LOW, HIGH = -(2**63), 2**63-1
class CellFault(Exception):
    pass

def checked(n):
    if n < LOW or n > HIGH:
        raise CellFault('OVERFLOW')
    return n

def recompute(sources):
    def value(name, stack):
        if name in stack:
            raise CellFault('CYCLE')
        source = sources.get(name, '0')
        def replace_sum(m):
            a,b=m.groups()
            ra,rb=int(a[1:]),int(b[1:]);ca,cb=ord(a[0]),ord(b[0])
            return '('+'+'.join(chr(c)+str(r) for r in range(ra,rb+1) for c in range(ca,cb+1))+')'
        try:
            tree=ast.parse(re.sub(r'SUM\(([A-P](?:1[0-6]|[1-9])):([A-P](?:1[0-6]|[1-9]))\)',replace_sum,source),mode='eval').body
        except (SyntaxError,ValueError):
            raise CellFault('SYNTAX')
        def visit(node):
            if isinstance(node, ast.Constant) and type(node.value) is int:
                return checked(node.value)
            if isinstance(node, ast.Name):
                try: return value(node.id,stack+[name])
                except CellFault as e:
                    raise CellFault('CYCLE' if str(e)=='CYCLE' else 'DEPENDENCY')
            if isinstance(node, ast.UnaryOp):
                if isinstance(node.operand, ast.Constant) and type(node.operand.value) is int:
                    return checked(-node.operand.value if isinstance(node.op,ast.USub) else node.operand.value)
                n=visit(node.operand)
                return checked(-n if isinstance(node.op, ast.USub) else n)
            if isinstance(node, ast.BinOp):
                a,b=visit(node.left),visit(node.right)
                if isinstance(node.op,ast.Add): return checked(a+b)
                if isinstance(node.op,ast.Sub): return checked(a-b)
                if isinstance(node.op,ast.Mult): return checked(a*b)
                if isinstance(node.op,ast.Div):
                    if not b: raise CellFault('DIV_ZERO')
                    return checked((abs(a)//abs(b))*(-1 if (a<0)!=(b<0) else 1))
            raise CellFault('SYNTAX')
        return visit(tree)
    out={}
    for name in sorted(sources,key=lambda n:(int(n[1:]),n[0])):
        try: out[name]=str(value(name,[]))
        except CellFault as e: out[name]='#'+str(e)
    return out

def history(label, edits):
    sources={};commands=[];expected=[];failed=False
    for name,source in edits:
        commands.append(f'set {name} = {source}' if source is not None else f'clear {name}')
        expected.append('OK\n')
        if source is None: sources.pop(name,None)
        else: sources[name]=source
        commands.append('show')
        for key,value in recompute(sources).items():
            failed |= value.startswith('#')
            expected.append(f'{key} = {value}\n')
    return dict(id=label,script='\n'.join(commands)+'\n',stdout=''.join(expected),exit=int(failed),steps=8_000_000)

def cases():
    data=[dict(id='empty',script='show\n',stdout='',exit=0,steps=1_000_000)]
    data.append(history('diamond',[('A1','2'),('B1','A1+1'),('A2','A1*2'),('B2','B1+A2'),('A1','5'),('B1',None),('A1',None)]))
    data.append(history('cycles',[('A1','B1'),('B1','A1'),('C1','A1+1'),('D1','7'),('B1',None),('D1','D1'),('D1','9')]))
    data.append(history('ranges',[('A1','2'),('B1','3'),('A2','SUM(A1:B1)'),('B1','-7/2'),('C1','SUM(A1:B2)'),('A1',None)]))
    data.append(history('errors',[('A1','1/0'),('B1','A1+1'),('C1','9223372036854775807+1'),('D1','('),('E1','-9223372036854775808/-1'),('F1','--2'),('G1','-9223372036854775808')]))
    vectors=['1','1 ','(1)','2+1','(((2+(1))))','3+2','3+2+4','(3+2)+4','3+(2+4)','(3+2+4)','3*2*4','(3*2)*4','3*(2*4)','(3*2*4)','3-2-4','(3-2)-4','3-(2-4)','(3-2-4)','3/2/4','(3/2)/4','3/(2/4)','(3/2/4)','(3*2/4)','(3/2*4)','3*(2/4)']
    data.append(history('tinyexpr', [('A1',v) for v in vectors]))
    data.append(history('graphlib', [('A1','B1+C1'),('B1','D1'),('C1','F1+G1'),('D1','E1'),('E1','J1'),('F1','D1'),('G1','H1'),('H1','I1'),('I1','E1'),('J1','2'),('J1','3')]))
    data.append(dict(id='durable',script='set A1 = 9\nsave book.json\nset A1 = 1\nload book.json\nshow A1\n',stdout='OK\nOK\nOK\nOK\nA1 = 9\n',exit=0,steps=1_000_000))
    data.append(dict(id='duplicate',script='set A1 = 9\nload bad.json\nshow A1\n',files={'bad.json':'{"version":1,"cells":[{"name":"A1","formula":"2"},{"name":"A1","formula":"3"}]}'},stdout='OK\nerror: duplicate_cell\nA1 = 9\n',exit=1,steps=1_000_000))
    data.append(dict(id='bad-version',script='set A1 = 9\nload bad.json\nshow A1\n',files={'bad.json':'{"version":2,"cells":[]}'},stdout='OK\nerror: workbook_version\nA1 = 9\n',exit=1,steps=1_000_000))
    data.append(dict(id='path',script='set A1 = 9\nload ../bad\nshow A1\n',stdout='OK\nerror: path\nA1 = 9\n',exit=1,steps=1_000_000))
    # Independent cap/one-over vectors remain byte-identical when a tooling stop occurs.
    for label,expr,error in [('formula-cap','1'+' '*255,None),('formula-over','1'+' '*256,'formula_limit'),('depth-cap','('*16+'1'+')'*16,None),('depth-over','('*17+'1'+')'*17,'depth_limit')]:
        data.append(dict(id=label,script=f'set A1 = {expr}\nshow A1\n',stdout=('OK\nA1 = 1\n' if error is None else f'error: {error}\nA1 = 0\n'),exit=0 if error is None else 3,steps=1_000_000))
    for n in [128,129]:
        data.append(dict(id=f'commands-{n}',script='recalc\n'*n,stdout='RECALC 0\n'*128+('error: command_limit\n' if n==129 else ''),exit=0 if n==128 else 3,steps=8_000_000))
    for n in [65536,65537]:
        raw='{"version":1,"cells":[]}'
        data.append(dict(id=f'bytes-{n}',script='load big.json\n',files={'big.json':raw+' '*(n-len(raw))},stdout='OK\n' if n==65536 else 'error: bytes_limit\n',exit=0 if n==65536 else 3,steps=16_000_000))
    data.append(dict(id='grid',script='set P16 = 4\nshow P16\nset Q1 = 5\nset A17 = 6\nshow P16\n',stdout='OK\nP16 = 4\nerror: address\nerror: address\nP16 = 4\n',exit=1,steps=1_000_000))
    return data

if __name__=='__main__':
    data=cases()
    if len(sys.argv)>1:
        with open(sys.argv[1],'w') as f: json.dump(data,f,indent=2);f.write('\n')
    else: print(json.dumps(data))

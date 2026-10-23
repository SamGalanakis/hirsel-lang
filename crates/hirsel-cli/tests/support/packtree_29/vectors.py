"""Compact cap recipes expanded at test time, never large permanent fixtures."""
import json
from oracle import archive,compact,report


def cases():
    rows=[]
    def add(id,command,entries,kind=None,path='',payload=None,**extra):
        if command=='pack':
            files={'source/tree.json':compact(dict(schema=1,entries=[dict(path=p,type=k,size=len(b)) for p,k,b in entries])).encode()}
            files.update({'source/'+p:b for p,k,b in entries if k=='file'})
            args=['pack','tree.json','out.tar','4']
            dirs=['source/'+p for p,k,_ in entries if k=='directory']
        else:
            files={'archives/in.tar':archive(entries) if payload is None else payload}
            args=[command,'in.tar'];dirs=[]
        rows.append(dict(id=id,args=args,files={p:b.hex() for p,b in files.items()},directories=dirs,links={},
            stdout='' if kind else report(entries),stderr=compact(dict(kind=kind,path=path))+'\n' if kind else '',
            status=2 if kind else 0,entries=[(p,k,b.hex()) for p,k,b in sorted(entries)],**extra))
    for n in (64,65):
        es=[(f'f{i:02}','file',b'') for i in range(n)]
        add('members'+str(n),'unpack',es,kind='member_count_limit' if n==65 else None,path='f64' if n==65 else '')
    for n in (65536,65537):
        add('file'+str(n),'pack',[('a','file',b'x'*n)],kind='file_bytes_limit' if n==65537 else None,path='a' if n==65537 else '',large=n==65536)
    for n in (1048576,1048577):
        es=[(f'f{i:02}','file',b'x'*65536) for i in range(16)]
        if n==1048577:es.append(('z','file',b'x'))
        add('payload'+str(n),'unpack',es,kind='payload_limit' if n==1048577 else None,path='z' if n==1048577 else '',large=True)
    for n in (2097152,2097153):
        data=b'\0'*n
        add('archive'+str(n),'list',[],payload=data,kind='input_bytes_limit' if n==2097153 else None,path='in.tar' if n==2097153 else '',large=n==2097152)
    for n in (512,513):
        add('path'+str(n),'list',[('a'*n,'file',b'')],kind='path_bytes_limit' if n==513 else None,path='a'*n if n==513 else '')
    for n in (8,9):
        p='/'.join(['a']*n)
        add('depth'+str(n),'unpack',[(p,'file',b'')],kind='depth_limit' if n==9 else None,path=p if n==9 else '')
    add('tar-global','unpack',[],payload=archive([('global','global',b'')]),kind='unsupported_kind',path='global')
    add('directory-size','unpack',[('dir','directory',b'x')],kind='directory_size',path='dir/')
    add('extract-mkdir-fault','unpack',[('sub/a','file',b'x')],kind='target_mkdir',path='sub',fault='file.mkdir',incomplete=True)
    # Source metadata lies about stable length so the exact stream itself must decide.
    for n,kind in [(2,'short_body'),(4,'long_body')]:
        add(kind,'pack',[('a','file',b'abc')],kind=kind,path='a',metadata={'source/a':3})
        rows[-1]['files']['source/a']=(b'x'*n).hex()
    # Go filepath Node tree: a,b,c,d/x,d/y,d/z/u,d/z/v.
    es=[(p,'directory' if p in ('b','d','d/y','d/z') else 'file',b'')
        for p in ('a','b','c','d','d/x','d/y','d/z','d/z/u','d/z/v')]
    add('walk-tree','pack',es)
    for width in (1,):
        add('jobs'+str(width),'pack',[('a','file',b'abc')]);rows[-1]['args'][-1]=str(width)
    for row in rows: row["cap"]=True
    return rows


if __name__=='__main__':print(json.dumps(cases(),ensure_ascii=False))

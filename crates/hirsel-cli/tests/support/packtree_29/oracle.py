"""Independent Python tarfile/hashlib vectors. Go AddFS/walk BSD-3-Clause recipes."""
import hashlib
import io
import json
import tarfile
from pathlib import Path


def compact(value):
    return json.dumps(value, ensure_ascii=False, separators=(',', ':'))


def report(entries):
    return compact(dict(schema=1, entries=[dict(path=p.rstrip('/'), type=k, size=len(b),
        sha256=hashlib.sha256(b).hexdigest()) for p,k,b in sorted(entries)],
        bytes=sum(len(b) for _,_,b in entries)))+'\n'


def archive(entries):
    out=io.BytesIO()
    with tarfile.open(fileobj=out,mode='w',format=tarfile.PAX_FORMAT) as tar:
        for p,k,b in entries:
            info=tarfile.TarInfo(p)
            info.type={'directory':tarfile.DIRTYPE,'file':tarfile.REGTYPE,
                       'symlink':tarfile.SYMTYPE,'link':tarfile.LNKTYPE,
                       'fifo':tarfile.FIFOTYPE,'device':tarfile.CHRTYPE,
                       'global':tarfile.XGLTYPE,'sparse':tarfile.GNUTYPE_SPARSE}[k]
            if k in ('symlink','link'): info.linkname='file.go'
            info.size=len(b)
            tar.addfile(info,io.BytesIO(b))
        end=tar.offset
    return out.getvalue()[:end+1024]


def cases():
    rows=[]
    def add(id,args,files,stdout='',kind=None,path='',dirs=(),links=None,**extra):
        rows.append(dict(id=id,args=args,files={p:b.hex() for p,b in files.items()},
            directories=list(dirs),links=links or {},stdout=stdout,
            stderr=compact(dict(kind=kind,path=path))+'\n' if kind else '',
            status=2 if kind else 0,**extra))
    def decl(entries):
        return compact(dict(schema=1,entries=[dict(path=p,type=k,size=len(b)) for p,k,b in entries])).encode()
    def packcase(id,entries,**kwargs):
        files={'source/tree.json':decl(entries)}
        files.update({'source/'+p:b for p,k,b in entries if k=='file'})
        dirs=['source/'+p for p,k,_ in entries if k=='directory']
        add(id,['pack','tree.json','out.tar','4'],files,
            stdout=report(entries) if not kwargs else '',dirs=dirs,entries=[(p,k,b.hex()) for p,k,b in sorted(entries)],**kwargs)
    go=[('emptyfolder','directory',b''),('file.go','file',b'hello'),
        ('subfolder','directory',b''),('subfolder/another.go','file',b'world')]
    packcase('pack-empty',[])
    packcase('pack-go',list(reversed(go)))
    packcase('pack-utf8',[('café','file',b'hi')])
    packcase('pack-name100',[('a'*100,'file',b'x')])
    packcase('pack-name101',[('a'*101,'file',b'x')])
    for id,entries in [('empty',[]),('go',go),('implicit-parent',[('sub/a','file',b'abc')]),
                        ('utf8',[('café','file',b'hi')]),('name101',[('a'*101,'file',b'x')])]:
        data=archive(entries)
        for command in ('list','unpack'):
            add(command+'-'+id,[command,'in.tar'],{'archives/in.tar':data},report(entries),
                entries=[(p,k,b.hex()) for p,k,b in sorted(entries)])
    for id,entries,kind,p in [
        ('duplicate',[('a','file',b''),('a','file',b'')],'duplicate_path','a'),
        ('file-directory',[('a','file',b''),('a/','directory',b'')],'duplicate_path','a/'),
        ('ancestor',[('a/b','file',b''),('a','file',b'')],'path_collision','a'),
        ('traversal',[('../escape','file',b'')],'invalid_path','../escape'),
        ('absolute',[('/escape','file',b'')],'invalid_path','/escape'),
        ('backslash',[('a\\b','file',b'')],'invalid_path','a\\b'),
        ('colon',[('a:b','file',b'')],'invalid_path','a:b'),
        ('depth9',[('/'.join(['a']*9),'file',b'')],'depth_limit','/'.join(['a']*9)),
        ('reserved',[('.packtree-manifest.json','file',b'')],'reserved_path','.packtree-manifest.json'),
    ]:
        add(id,['unpack','in.tar'],{'archives/in.tar':archive(entries)},kind=kind,path=p)
    for kind in ('symlink','link','fifo','device','sparse'):
        add('tar-'+kind,['unpack','in.tar'],{'archives/in.tar':archive([('bad',kind,b'')])},
            kind='tar_malformed' if kind=='sparse' else 'unsupported_kind',path='' if kind=='sparse' else 'bad')
    data=bytearray(archive([('a','file',b'abc')]))
    data[0]=98
    add('corrupt',['unpack','in.tar'],{'archives/in.tar':bytes(data)},kind='tar_malformed')
    good=archive([('a','file',b'abc')])
    for id,data,kind,p in [('short-header',good[:511],'tar_framing',''),
        ('short-body',good[:513],'tar_framing',''),('no-footer',good[:1024],'tar_framing',''),
        ('trailing',good+b'x'*512,'tar_trailing','')]:
        add(id,['unpack','in.tar'],{'archives/in.tar':data},kind=kind,path=p)
    add('target-nonempty',['unpack','in.tar'],{'archives/in.tar':archive([]),'target/old':b'keep'},kind='target_not_empty')
    packcase('source-symlink',[('a','file',b'')],kind='symlink',path='a',links={'source/a':'elsewhere'})
    for width in ('0','5'):
        add('jobs'+width,['pack','tree.json','out.tar',width],{},kind='jobs_limit')
    add('unknown',['bad','in.tar'],{},kind='usage')
    add('denied',['list','in.tar'],{},kind='archives_grant',denied=True)
    # Faults occur in worker body, not the declaration read.
    packcase('canceled-declaration',[('a','file',b'x'),('b','file',b'y')],kind='canceled',path='tree.json',fault='canceled')
    for op,kind in [('file.write_all','publication_write'),('file.commit','publication_commit')]:
        add(op,['pack','tree.json','out.tar','4'],{'source/tree.json':decl([]),'archives/out.tar':b'old'},kind=kind,path='out.tar',fault=op)
    return rows

if __name__=='__main__':
    Path(__file__).with_name('cases.json').write_text(json.dumps(cases(),ensure_ascii=False,indent=2)+'\n')

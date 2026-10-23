import json,hashlib,copy,pathlib
V=json.loads(pathlib.Path(__file__).with_name('vectors.json').read_text())
F='chunkbackup1:rabin:3da3358b4dc173:64:256:1024:4096'
def sha(b):return hashlib.sha256(b).hexdigest()
def encode(x):return json.dumps(x,separators=(',',':'),ensure_ascii=False).encode()
def row(n,b,parts=None):return dict(path=n,size=len(b),id=sha(b),chunks=parts if parts is not None else ([] if not b else [dict(offset=0,length=len(b),id=sha(b))]))
def spec(rs):return encode([{k:r[k] for k in ['path','size','id']} for r in rs])
def snap(rs):return encode(dict(format=F,files=rs))
def success(rs):return encode(dict(files=len(rs),bytes=sum(r['size'] for r in rs)))+b'\n'
def err(tag,path='',completed=0):return encode(dict(error=tag,path=path,completed=completed))+b'\n'
C=[]
def add(id,args,files,rows=None,error=None,**kw):
 out=b'' if error else success(rows)
 expected=dict(files)
 if not error:
  if args[0]=='backup':expected['snapshots/daily']=snap(rows)
  if args[0]=='restore':
   for r in rows:
    expected['target/'+r['path']]=b''.join(files['chunks/'+p['id']] for p in r['chunks'])
 C.append(dict(id=id,args=args,files={k:v.hex() for k,v in files.items()},expected={k:v.hex() for k,v in expected.items()},stdout=out.hex(),stderr=(error or b'').hex(),exit=2 if error else 0,steps=40000000 if kw.get('boundary') else 20000000,**kw))
def backup(id,blobs,parts=None,**kw):
 rs=[row(n,b,parts[i] if parts else None) for i,(n,b) in enumerate(blobs)]
 fs={'source/input.json':spec(rs)};fs.update({'source/'+n:b for n,b in blobs})
 # Chunk contents expected from Go's vector, never from the app under test.
 for (n,b),r in zip(blobs,rs):
  for p in r['chunks']:fs.setdefault('chunks/'+p['id'],b[p['offset']:p['offset']+p['length']]) if kw.pop('preinsert',False) else None
 add(id,['backup','input.json','daily','4'],fs,rs,**kw)
 c=C[-1]
 if not c['stderr']:
  for (n,b),r in zip(blobs,rs):
   for p in r['chunks']:c['expected']['chunks/'+p['id']]=b[p['offset']:p['offset']+p['length']].hex()
backup('empty-snapshot',[])
for v in V:backup(v['name'],[('a',bytes.fromhex(v['hex']))],[v['parts']])
seed=next(v for v in V if v['name']=='seed8192')
shift=next(v for v in V if v['name']=='shift8192')
backup('shift-pair',[('a',bytes.fromhex(seed['hex'])),('b',bytes.fromhex(shift['hex']))],[seed['parts'],shift['parts']])
backup('duplicate',[('a',b'abc'),('b',b'abc')])
backup('depth8',[('a/b/c/d/e/f/g/h',b'abc')])
for n in ['', '../escape','/absolute','a//b','a/./b','a/../b','a\\b','c:x','a\x00b','a/b/c/d/e/f/g/h/i']:
 r=row(n,b'');tag='depth_limit' if n=='a/b/c/d/e/f/g/h/i' else 'invalid_path'
 add('path-'+str(len(C)-1),['backup','input.json','daily','4'],{'source/input.json':spec([r])},error=err(tag,n))
for names,tag in [(['a','a'],'duplicate_path'),(['a','a/b'],'path_collision')]:
 rs=[row(n,b'') for n in names];add(tag,['backup','input.json','daily','4'],{'source/input.json':spec(rs)},error=err(tag,names[1]))
r=row('a',b'abc');r['id']=sha(b'bad')
add('input-hash',['backup','input.json','daily','4'],{'source/input.json':spec([r]),'source/a':b'abc'},error=err('input_changed','a'))
r=row('a',b'abc');r['size']=2
add('input-size',['backup','input.json','daily','4'],{'source/input.json':spec([r]),'source/a':b'abc'},error=err('input_changed','a'))
# Restore and verify independent known snapshots.
for verb in ['verify','restore']:
 rs=[row('nested/a',b'abc'),row('empty',b'')]
 fs={'snapshots/daily':snap(rs),'chunks/'+sha(b'abc'):b'abc'}
 add(verb,[verb,'daily'],fs,rs)
 if verb=='restore':
  add('restore-equal',[verb,'daily'],dict(fs,**{'target/nested/a':b'abc','target/empty':b''}),rs)
  add('restore-different',[verb,'daily'],dict(fs,**{'target/nested/a':b'bad'}),error=err('target_exists','nested/a'))
 rs=[row('a',b'abc')];fs={'snapshots/daily':snap(rs)}
 add(verb+'-missing',[verb,'daily'],fs,error=err('missing',sha(b'abc')))
 fs['chunks/'+sha(b'abc')]=b'bad'
 add(verb+'-corrupt',[verb,'daily'],fs,error=err('chunk_corrupt',sha(b'abc')))
 rs=[row('a',b'abc'),row('b',b'def')];fs={'snapshots/daily':snap(rs),'chunks/'+sha(b'abc'):b'abc'}
 add(verb+'-partial',[verb,'daily'],fs,error=err('missing',sha(b'def'),1),completed_files={'target/a':b'abc'.hex()} if verb=='restore' else {})
 rs=[row('../escape',b'')];add(verb+'-traversal',[verb,'daily'],{'snapshots/daily':snap(rs)},error=err('invalid_path','../escape'))
 rs=[row('a',b'abc')];rs[0]['id']=sha(b'bad');add(verb+'-wholehash',[verb,'daily'],{'snapshots/daily':snap(rs),'chunks/'+sha(b'abc'):b'abc'},error=err('file_corrupt','a'))
 for field,value in [('offset',1),('length',4097),('length',0)]:
  rs=[row('a',b'abc')];rs[0]['chunks'][0][field]=value
  add(verb+'-layout-'+field+str(value),[verb,'daily'],{'snapshots/daily':snap(rs)},error=err('chunk_layout',sha(b'abc')))
 add(verb+'-config',[verb,'daily'],{'snapshots/daily':snap([]).replace(b':1024:',b':2048:')},error=err('configuration'))
# Fault/cancellation orchestration; pending a, sibling wrong whole hash b, queued c.
rs=[row('a',b'abc'),row('b',b'bad'),row('c',b'')]
fs={'source/input.json':spec(rs),'source/a':b'abc','source/b':b'xyz','source/c':b''}
add('pending-cancel',['backup','input.json','daily','2'],fs,error=err('input_changed','b'),pending='source/a')
rs=[row('a',b'abc'),row('b',b'def')]
fs={'source/input.json':spec(rs),'source/a':b'abc','source/b':b'def'}
add('afterwrite-release',['backup','input.json','daily','2'],fs,rs,afterwrite={'path':'source/a','release':'chunks/'+sha(b'def')})
for b in [b'abc',b'def']:C[-1]['expected']['chunks/'+sha(b)]=b.hex()
# Durable failures by grant refusal: same app/provider semantics, no partial file.
rs=[row('a',b'abc')]
for mount,tag in [('chunks','chunk_publish'),('snapshots','snapshot_publish')]:
 add('durable-'+mount,['backup','input.json','daily','4'],{'source/input.json':spec(rs),'source/a':b'abc'},error=err(tag,sha(b'abc') if mount=='chunks' else 'daily'),readonly=mount)
add('durable-target',['restore','daily'],{'snapshots/daily':snap(rs),'chunks/'+sha(b'abc'):b'abc'},error=err('target_publish','a'),readonly='target')
rs=[row('a',b'abc')]
add('durable-provider-fault',['backup','input.json','daily','4'],{'source/input.json':spec(rs),'source/a':b'abc'},error=err('chunk_publish',sha(b'abc')),fault='file.put_new')
rs=[row('empty',b'')]
add('snapshot-provider-fault',['backup','input.json','daily','4'],{'source/input.json':spec(rs),'source/empty':b''},error=err('snapshot_publish','daily'),fault='file.put_new')
rs=[row('a',b'abc')]
add('restore-unrelated',['restore','daily'],{'snapshots/daily':snap(rs),'chunks/'+sha(b'abc'):b'abc','target/unrelated':b'no'},error=err('target_not_empty','unrelated'))
# Large boundary identities retain 40M, never substitute smaller inputs on stop.
v=next(v for v in V if v['name']=='zero4096');bp=[dict(offset=i*256,length=256,id=sha(bytes(256))) for i in range(128)]
backup('file-cap',[('a',bytes(32768))],[bp],boundary=True)
r=row('a',b'');r['size']=32769
add('file-over',['backup','input.json','daily','4'],{'source/input.json':spec([r])},error=err('file_limit','a'),boundary=True)
backup('files-cap',[(str(i),b'') for i in range(16)],boundary=True)
add('files-over',['backup','input.json','daily','4'],{'source/input.json':spec([row(str(i),b'') for i in range(17)])},error=err('files_limit'),boundary=True)
backup('total-cap',[(str(i),bytes(32768)) for i in range(8)],[bp]*8,boundary=True)
rs=[row(str(i),bytes(32768)) for i in range(8)]+[row('extra',b'x')]
add('total-over',['backup','input.json','daily','4'],{'source/input.json':spec(rs)},error=err('total_limit','extra'),boundary=True)
for size in [262144,262145]:
 data=b'[]'+b' '*(size-2)
 add('manifest-'+str(size),['backup','input.json','daily','4'],{'source/input.json':data},[] if size==262144 else None,error=None if size==262144 else err('input_limit','input.json'),boundary=True)
# Max chunks applies to declared descriptors, including unusual but valid small chunks.
for n in [2048,2049]:
 ps=[dict(offset=i,length=1,id=sha(b'x')) for i in range(n)]
 rs=[row('a',b'x'*n,ps)]
 add('chunks-'+str(n),['verify','daily'],{'snapshots/daily':snap(rs),'chunks/'+sha(b'x'):b'x'},rs if n==2048 else None,error=None if n==2048 else err('chunks_limit','a'),boundary=True)
C.append(dict(id='deny-output',args=['verify','daily'],files={},expected={},stdout='',stderr='',exit=3,steps=20000000,deny_output=True))
print(json.dumps(C))

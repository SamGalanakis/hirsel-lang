import json,hashlib,html,pathlib
cfg='content = "content"\nassets = "assets"\noutput = "site"\ntitle = "Site & sheep"\njobs = 4\n'
sha=lambda b:hashlib.sha256(b).hexdigest()
esc=lambda s:html.escape(s).replace('&#x27;','&#39;').replace('&quot;','&#34;')
# Fixture-only literal model: paragraphs consisting exclusively of x or &.
def outputs(files):
 pages=sorted((n+'.html',n) for n in files if n.startswith('content/'))
 out={n[7:]:v for n,v in files.items() if n.startswith('assets/')}
 for name,source in pages:
  nav='<nav><ul>\n'+''.join('<li><a href="'+'../'*name.count('/')+n+'">'+s+'</a></li>\n' for n,s in pages)+'</ul></nav>\n'
  body=('<p>'+esc(files[source].decode())+'</p>\n') if files[source] else ''
  out[name]=('<!doctype html>\n<meta charset="utf-8">\n<title>'+source+' | Site &amp; sheep</title>\n'+nav+'<main>\n'+body+'</main>\n').encode()
 out['index.html']=('<!doctype html>\n<meta charset="utf-8">\n<title>Site &amp; sheep</title>\n<ul>\n'+''.join('<li><a href="'+n+'">'+s+'</a></li>\n' for n,s in pages)+'</ul>\n').encode()
 entries={}
 for n,b in sorted(out.items()):
  original=files[n[:-5]] if n.endswith('.md.html') else files.get('assets/'+n,cfg.encode())
  entries[n]={'input':sha(original),'output':sha(b)}
 manifest=json.dumps({'version':1,'config':sha(cfg.encode()),'files':entries},separators=(',',':')).encode()
 return out,manifest
cases=[]
def add(id,files,error=None):
 out,manifest=outputs(files)
 cases.append(dict(id=id,files=[dict(path=n,fill=chr(b[0]) if b else 'x',size=len(b)) for n,b in files.items()],error=error,output_hashes={n:sha(b) for n,b in out.items()} if not error else {},output_bytes=sum(map(len,out.values()))+len(manifest)))
add('items-64',{'assets/a%02d'%i:b'' for i in range(64)})
add('items-65',{'assets/a%02d'%i:b'' for i in range(65)},'directory_entries')
add('page-65536',{'content/a.md':b'x'*65536})
add('page-65537',{'content/a.md':b'x'*65537},'page_bytes')
for total in [1048576,1048577]:
 remaining=total-len(cfg.encode());files={}
 while remaining:
  n=min(65536,remaining);files['assets/a%02d'%len(files)]=b'x'*n;remaining-=n
 add('input-'+str(total),files,None if total==1048576 else 'input_bytes')
for depth in [8,9]:
 add('depth-'+str(depth),{'content/'+'a/'*(depth-3)+'x.md':b'x'},None if depth==8 else 'output_path')
files={'content/p%d.md'%i:b'&'*45000 for i in range(7)}
files.update({'assets/fill%d'%i:b'' for i in range(8)})
out,m=outputs(files);remaining=2097152-sum(map(len,out.values()))-len(m)
assert 0<remaining<=8*65536
for i in range(8):
 n=min(65536,remaining);files['assets/fill%d'%i]=b'x'*n;remaining-=n
add('output-2097152',files)
assert cases[-1]['output_bytes']==2097152
files=dict(files);files['assets/fill7']+=b'x'
add('output-2097153',files,'output_bytes')
assert cases[-1]['output_bytes']==2097153
pathlib.Path(__file__).with_name('caps.json').write_text(json.dumps(cases,indent=2)+'\n')
print([(c['id'],c['output_bytes']) for c in cases])

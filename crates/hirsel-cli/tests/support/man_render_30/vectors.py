"""Permanent independent inputs and literal man output (cmark no-wrap structure)."""
import json
TH = '.TH "TEST" "1" "" "" ""\n'
def page(id, source, body, **extra):
    return dict(id=id, args=['render','in.md','out.1','TEST','1'], files={'in.md':source}, exit=0, stdout='rendered 1 pages; published 1; failed 0\n', outputs={'out.1':TH+body}, steps=1000000, **extra)
def error(id, source, message, **extra):
    row=page(id,source,'',**extra); row.update(exit=2,stdout=message+'\n',outputs={});return row
cases=[
 page('empty','',''),
 page('cmark-nowrap','foo *bar*\n\n- Lorem ipsum dolor sit amet,\n  consectetur adipiscing elit,\n- sed do eiusmod tempor incididunt\n  ut labore et dolore magna aliqua.\n','.PP\nfoo \\f[I]bar\\f[]\n.IP \\[bu] 2\nLorem ipsum dolor sit amet,\nconsectetur adipiscing elit,\n.IP \\[bu] 2\nsed do eiusmod tempor incididunt\nut labore et dolore magna aliqua.\n'),
 page('controls','.SH injected\n\'apostrophe\n\\\\escape - x "q"\n','.PP\n\\&.SH injected\n\\&\'apostrophe\n\\eescape \\- x "q"\n'),
 page('unicode','λ “sheep” — ‘wool’ –\n','.PP\nλ \\[lq]sheep\\[rq] \\[em] \\[oq]wool\\[cq] \\[en]\n'),
 page('blocks','# NAME\n\n> quote\n\n```sh\n.rm unsafe\n\\escape - x\n```\n\n---\n','.SH\nNAME\n.RS\n.PP\nquote\n.RE\n.IP\n.nf\n\\f[C]\n\\&.rm unsafe\n\\eescape \\- x\n\\f[]\n.fi\n.PP\n  *  *  *  *  *\n'),
 page('nested','3. **bold *italic* bold**\n   - `code`\n\n[x](https://example/x-y) ![alt](image)\n','.IP "3." 4\n\\f[B]bold \\f[I]italic\\f[B] bold\\f[]\n.IP \\[bu] 2\n\\f[C]code\\f[]\n.PP\nx (https://example/x\\-y) [IMAGE: alt]\n'),
 page('hardbreak','a  \nb\n','.PP\na.PD 0\n.P\n.PD\nb\n'),
 error('html-block','<div>raw</div>','in.md:1:1–1:15: unsupported raw HTML (bytes [0,14), end exclusive)'),
 error('html-inline','x <i>y</i>','in.md:1:3–1:6: unsupported raw HTML (bytes [2,5), end exclusive)'),
 error('table','a | b\n--- | ---\nx | y\n','in.md:1:1–3:6: unsupported table (bytes [0,21), end exclusive)'),
 error('utf8',[255],'in.md: invalid UTF-8'),
]
for id,title,section,date in [('invalid-section','TEST','0',''),('metadata-control','bad\n.rm','1',''),('metadata65','x'*65,'1',''),('date33','TEST','1','x'*33)]:
 r=page(id,'','');r.update(args=['render','in.md','out.1',title,section,date],exit=2,stdout='in.md: invalid '+('section (1..9)' if section=='0' else 'metadata')+'\n',outputs={});cases.append(r)
for id,title,date in [('metadata64','x'*64,''),('date32','TEST','x'*32),('metadata-quotes','A"B\\C-D','')]:
 r=page(id,'','');r['args']=['render','in.md','out.1',title,'1',date]
 escaped=title.replace('\\','\\e').replace('-','\\-').replace('"','\\(dq')
 r['outputs']={'out.1':f'.TH "{escaped}" "1" "{date}" "" ""\n'};cases.append(r)
r=error('missing','','in.md: file not found');r['files']={};cases.append(r)
r=page('check','hi','');r.update(args=['check','in.md','TEST','1'],outputs={},stdout='checked 1 pages\n');cases.append(r)
r=error('write-refused','hello','out.1: publish not granted\nrendered 1 pages; published 0; failed 1');r['read_only']=True;cases.append(r)
for id,n in [('pages16',16),('pages17',17)]:
 r=page(id,'','');r.update(args=['render','--manifest','pages.tsv'],files={'pages.tsv':''.join(f'p{i}.md\tp{i}.1\tTEST\t1\t\t\t\n' for i in range(n)),**{f'p{i}.md':'' for i in range(n)}},steps=10000000)
 r.update(outputs={f'p{i}.1':TH for i in range(n)},stdout=f'rendered {n} pages; published {n}; failed 0\n')
 if n==17:r.update(exit=2,outputs={},stdout='manifest: resource limit pages\n')
 cases.append(r)
r=page('batch-order','','');r.update(args=['render','--manifest','pages.tsv'],files={'pages.tsv':'b.md\tz.1\tTEST\t1\t\t\t\na.md\ta.1\tTEST\t1\t\t\t\n','a.md':'a','b.md':'b'},outputs={'z.1':TH+'.PP\nb\n','a.1':TH+'.PP\na\n'},stdout='rendered 2 pages; published 2; failed 0\n');cases.append(r)
for id,blocked in [('batch-no-publish',False),('pending-cancel',True)]:
 r=error(id,'','b.md:1:1–1:15: unsupported raw HTML (bytes [0,14), end exclusive)');r.update(args=['render','--manifest','pages.tsv'],files={'pages.tsv':'a.md\ta.1\tTEST\t1\t\t\t\nb.md\tb.1\tTEST\t1\t\t\t\nc.md\tc.1\tTEST\t1\t\t\t\n','a.md':'a','b.md':'<div>bad</div>','c.md':'c'})
 if blocked:r['pending']='data/a.md'
 cases.append(r)
for n in [16384,16385]:
 r=page(f'source{n}','x'*n,'.PP\n'+'x'*n+'\n');r['steps']=10000000
 if n==16385:r.update(exit=2,stdout='in.md: resource limit source bytes\n',outputs={})
 cases.append(r)
# Historical fixture names use root depth zero; retain their source bytes.
# Root depth one now makes the original depth32 fixture refuse at AST depth32.
for depth in [32,33]:
 q=depth-2;r=page(f'depth{depth}','> '*q+'x\n','.RS\n'*q+'.PP\nx\n'+'.RE\n'*q);r['steps']=10000000
 r.update(exit=2,outputs={},stdout='in.md: resource limit AST depth\n')
 cases.append(r)
for n in [8192,8193]:
 r=page(f'manifest{n}','','');r.update(args=['check','--manifest','pages.tsv'],files={'pages.tsv':'\n'*n},outputs={},stdout='checked 0 pages\n')
 if n==8193:r.update(exit=2,stdout='pages.tsv: resource limit bytes\n')
 cases.append(r)
for count in [8192,8193]:
 source='x\n\n'*4095+('---\n' if count==8192 else 'x\n')
 body='.PP\nx\n'*4095+('.PP\n  *  *  *  *  *\n' if count==8192 else '.PP\nx\n')
 r=page(f'nodes{count}',source,body);r['steps']=10000000
 if count==8193:r.update(exit=2,outputs={},stdout='in.md: resource limit AST nodes\n')
 cases.append(r)
for size in [65536,65537]:
 # TH24 + PP4 + final newline; eight roff bytes per sanitized control.
 n=(size-29)//8;tail=size-29-n*8
 source='\x01'*n+'z'*tail
 body='.PP\n'+'\\[uFFFD]'*n+'z'*tail+'\n'
 r=page(f'output{size}',source,body);r['steps']=10000000
 assert len(r['outputs']['out.1'])==size
 if size==65537:r.update(exit=2,outputs={},stdout='in.md: resource limit output bytes\n')
 cases.append(r)
for size in [131072,131073]:
 sources=[16384]*8+([1] if size==131073 else [])
 files={'pages.tsv':''.join(f'p{i}.md\tp{i}.1\tTEST\t1\t\t\t\n' for i in range(len(sources)))}
 files.update({f'p{i}.md':'x'*n for i,n in enumerate(sources)})
 r=page(f'total{size}','','');r.update(args=['render','--manifest','pages.tsv'],files=files,steps=10000000,stdout=f'rendered {len(sources)} pages; published {len(sources)}; failed 0\n',outputs={f'p{i}.1':TH+'.PP\n'+'x'*n+'\n' for i,n in enumerate(sources)})
 if size==131073:r.update(exit=2,stdout='p8.md: resource limit total source bytes\n',outputs={})
 cases.append(r)
for length in [128,129]:
 name='p'*length;r=page(f'path{length}','','');r['args'][1]=name;r['files']={name:''}
 if length==129:r.update(exit=2,stdout=name+': invalid path\n',outputs={})
 cases.append(r)
r=error('pending-cancel-queued','','b.md:1:1–1:15: unsupported raw HTML (bytes [0,14), end exclusive)');r.update(args=['render','--manifest','pages.tsv'],files={'pages.tsv':''.join(f'{c}.md\t{c}.1\tTEST\t1\t\t\t\n' for c in 'abcde'),**{f'{c}.md':('<div>bad</div>' if c=='b' else c) for c in 'abcde'}},pending='data/a.md');cases.append(r)
r=page('reordered-batch','','',scheduler_seed=3);r.update(args=['render','--manifest','pages.tsv'],files={'pages.tsv':'a.md\ta.1\tTEST\t1\t\t\t\nb.md\tb.1\tTEST\t1\t\t\t\n','a.md':'a'*64,'b.md':'b'},outputs={'a.1':TH+'.PP\n'+'a'*64+'\n','b.1':TH+'.PP\nb\n'},stdout='rendered 2 pages; published 2; failed 0\n');cases.append(r)
r=page('publication-continues','','',commit_fault=True);r.update(args=['render','--manifest','pages.tsv'],files={'pages.tsv':'a.md\ta.1\tTEST\t1\t\t\t\nb.md\tz.1\tTEST\t1\t\t\t\n','a.md':'a','b.md':'b'},outputs={'z.1':TH+'.PP\nb\n'},stdout='a.1: publish permission denied\nrendered 2 pages; published 1; failed 1\n',exit=2);cases.append(r)
for field in ['origin','manual']:
 for size in [64,65]:
  origin=('o'*size if field=='origin' else 'source')
  manual=('m'*size if field=='manual' else 'Manual')
  r=page(f'{field}{size}','','');r.update(args=['render','--manifest','pages.tsv'],files={'pages.tsv':f'in.md\tout.9\tTEST\t9\t2026-10-23\t{origin}\t{manual}\n','in.md':''},outputs={'out.9':f'.TH "TEST" "9" "2026\\-10\\-23" "{origin}" "{manual}"\n'})
  if size==65:r.update(exit=2,outputs={},stdout='in.md: invalid metadata\n')
  cases.append(r)
if __name__=='__main__':print(json.dumps(cases,ensure_ascii=False))

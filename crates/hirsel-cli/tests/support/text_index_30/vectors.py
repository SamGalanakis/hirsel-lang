"""Independent whole-document scan oracle; no postings from Hirsel are inputs.
Tantivy tokenizer e988ecb7 simple_tokenizer.rs:73-80; MIT in app directory.
The shipped predicate is Unicode Letter | Decimal_Number (see README deviations).
"""
import collections
import copy
import json
from pathlib import Path
import unicodedata


def scan(text):
    tokens, start, begin, offset = [], None, 0, 0
    for c in text + '\0':
        alnum = unicodedata.category(c).startswith('L') or unicodedata.category(c) == 'Nd'
        if alnum and start is None:
            start, begin = offset, len(tokens)
        if not alnum and start is not None:
            raw = text.encode()[start:offset].decode()
            tokens.append((raw.lower(), dict(start=start, end=offset, position=len(tokens))))
            start = None
        offset += len(c.encode())
    return tokens


def snapshot(docs):
    terms = collections.defaultdict(list)
    table = []
    for id, text in docs:
        tokens = scan(text)
        table.append(dict(id=id, bytes=len(text.encode()), tokens=len(tokens)))
        grouped = collections.defaultdict(list)
        for term, span in tokens:
            grouped[term].append(span)
        for term, spans in grouped.items():
            terms[term].append(dict(id=id, spans=spans))
    return dict(version=1, documents=sorted(table, key=lambda x:x['id']), terms=[dict(term=t, postings=sorted(p,key=lambda x:x['id'])) for t,p in sorted(terms.items())])


def query(docs, mode, text):
    terms = list(dict.fromkeys(t for t,_ in scan(text)))
    rows=[]
    for id, source in sorted(docs):
        counts = collections.Counter(t for t,_ in scan(source))
        if terms and (all(counts[t] for t in terms) if mode == 'all' else any(counts[t] for t in terms)):
            rows.append(dict(id=id, frequencies={t:counts[t] for t in terms}))
    return rows


def render(value):
    return json.dumps(value, ensure_ascii=False, separators=(',', ':'))


def vectors():
    cases=[]
    def case(id, docs, error=None, query_text='hello tax', jobs=4, boundary=False):
        manifest=dict(version=1, documents=[dict(id=k,path=f'{i}.txt') for i,(k,v) in enumerate(docs)])
        files={'input/manifest.json':render(manifest),'index/search.json':'old snapshot'}
        files.update({f'input/{i}.txt':v for i,(k,v) in enumerate(docs)})
        stages=[dict(args=['build','manifest.json','search.json',str(jobs)],exit=2 if error else 0,stdout='' if error else 'built\n',stderr=error or '')]
        if not error:
            stages.append(dict(args=['inspect','manifest.json','search.json'],exit=0,index=snapshot(docs)))
            for mode in ['all','any']:
                stages.append(dict(args=['query','manifest.json','search.json',mode,query_text],exit=0,rows=query(docs,mode,query_text)))
        cases.append(dict(id=id,files=files,stages=stages,boundary=boundary))
        return cases[-1]
    for jobs in [1,2,4]:
        case(f'tantivy-j{jobs}', [('z','Hello, happy tax payer! Hello.'),('a','tax payer payer'),('empty','')], query_text='Hello tax',jobs=jobs)
    case('unicode', [('Café','Café CAFÉ cafe\u0301 İ i ΟΣ 你好 λ2'),('a','CAFÉ tax'),('none','!!!')], query_text='CAFÉ İ')
    case('empty', [], query_text='')
    case('empty-doc', [('a','')],query_text='hello')
    case('disjoint', [('a','left'),('b','right')],query_text='left right')
    case('overlap', [('1','left right'),('3','left right right'),('9','right left')],query_text='left right')
    case('missing-term', [('a','hello')],query_text='hello absent absent')
    case('reordered-completion', [('z','one '*20),('a','two'),('m','one two')],query_text='one two')
    case('duplicate-id',[('a','hello'),('a','tax')],error='duplicate_id:1\n')
    case('invalid-utf8',[('a','')],error='contents_utf8:0\n')['files']['input/0.txt']=[255]
    case('pending-cancel',[('bad',''),('park','x'),('queued','z'),('queued2','w'),('unadmitted','x')],error='contents_utf8:0\n')['files']['input/0.txt']=[255]
    cases[-1]['pending']={'1.txt':'pending'}
    case('token-cap',[('a','a'*256)],query_text='a'*256)
    case('token-over',[('a','a'*257)],error='token_bytes_limit:0\n')
    case('normalization-expansion',[('a','İ'*128)],query_text='İ'*128)
    case('query-token-cap',[('a','x')],query_text='x '*32)
    c=case('query-token-over',[('a','x')]);c['stages']=[dict(args=['query','manifest.json','search.json','all','x '*33],exit=2,stdout='',stderr='token_count_limit:0\n')];c['files']['index/search.json']=render(snapshot([('a','x')]))
    case('doc-cap',[(f'{i:02}','') for i in range(32)],boundary=True)
    case('doc-over',[(f'{i:02}','') for i in range(33)],error='document_limit:0\n',boundary=True)
    case('bytes-cap',[('a',' '*8192)],boundary=True)
    case('bytes-over',[('a',' '*8193)],error='input_bytes_limit:0\n',boundary=True)
    case('total-cap',[(f'{i:02}',' '*8192) for i in range(16)],boundary=True)
    case('total-over',[(f'{i:02}',' '*8192) for i in range(16)]+[('z',' ')],error='input_bytes_limit:16\n',boundary=True)
    case('terms-cap',[(str(n),' '.join(f't{i}' for i in range(n,min(n+1000,4096)))) for n in range(0,4096,1000)],boundary=True)
    case('terms-over',[(str(n),' '.join(f't{i}' for i in range(n,min(n+1000,4097)))) for n in range(0,4097,1000)],error='term_limit:0\n',boundary=True)
    case('postings-cap',[(str(i),'x '*4096) for i in range(4)],query_text='x',boundary=True)
    case('postings-over',[(str(i),'x '*4096) for i in range(4)]+[('z','x')],error='posting_limit:4\n',boundary=True)
    base=snapshot([('a','hello hello')])
    mutations=[('version',lambda s:s.update(version=2),'index_version'),('document-order',lambda s:s['documents'].append(copy.deepcopy(s['documents'][0])),'manifest_mismatch'),('term-order',lambda s:s['terms'].append(copy.deepcopy(s['terms'][0])),'term_order'),('reference',lambda s:s['terms'][0]['postings'][0].update(id='bad'),'posting_reference'),('span',lambda s:s['terms'][0]['postings'][0]['spans'][0].update(end=100),'span_bounds'),('position',lambda s:s['terms'][0]['postings'][0]['spans'][1].update(position=0),'span_bounds'),('missing-position',lambda s:s['terms'][0]['postings'][0]['spans'].pop(),'position_count'),('token-bounds',lambda s:s['documents'][0].update(tokens=100),'document_bounds')]
    for id,mut,error in mutations:
        c=case('corrupt-'+id,[('a','hello hello')]);s=copy.deepcopy(base);mut(s);c['files']['index/search.json']=render(s);c['stages']=[dict(args=['inspect','manifest.json','search.json'],exit=2,stdout='',stderr=error+':0\n')]
    for id,value,error in [('malformed','{','schema'),('trailing',render(base)+'x','json'),('index-over',' '*1048577,'input_bytes_limit')]:
        c=case(id,[('a','hello hello')],boundary=id=='index-over');c['files']['index/search.json']=value;c['stages']=[dict(args=['inspect','manifest.json','search.json'],exit=2,stdout='',stderr=error+':0\n')]
    for id,access,error in [('denied-read','denied','input_grant'),('denied-write','read','index_create')]:
        c=case(id,[('a','hello')],error=error+':0\n');c['access']=access
    # Additional independent acceptance/control vectors, frozen before execution.
    docs=[(str(n),' '.join(t for t,ns in [('left',[1,3,9]),('right',[3,4,9,18]),('third',[1,5,9,111])] if n in ns)) for n in [1,3,4,5,9,18,111]]
    c=case('tantivy-intersection',docs,query_text='left right')
    c['stages'].append(dict(args=['query','manifest.json','search.json','all','left right third'],exit=0,rows=query(docs,'all','left right third')))
    case('tantivy-zero',[('0','left right')],query_text='left right')
    case('id-cap',[('i'*128,'x')],query_text='x')
    case('id-over',[('i'*129,'x')],error='text_bytes_limit:0\n')
    for id,jobs in [('jobs-zero',0),('jobs-over',5)]:
        case(id,[('a','x')],error='jobs:0\n',jobs=jobs)
    for id,size,error in [('manifest-cap',16384,None),('manifest-over',16385,'input_bytes_limit:0\n')]:
        c=case(id,[],error=error,boundary=True)
        c['files']['input/manifest.json'] += ' '*(size-len(c['files']['input/manifest.json'].encode()))
    for id,size,error in [('index-cap',1048576,None)]:
        c=case(id,[],boundary=True);raw=render(snapshot([]));c['files']['index/search.json']=raw+' '*(size-len(raw))
        c['stages']=[dict(args=['inspect','manifest.json','search.json'],exit=0,index=snapshot([]))]
    for id,name,error in [('path-cap','p/'*127+'ab',None),('path-over','p/'*127+'abc','text_bytes_limit:0\n')]:
        c=case(id,[('a','x')],error=error)
        c['files']['input/manifest.json']=render(dict(version=1,documents=[dict(id='a',path=name)]))
        c['files']['input/'+name]=c['files'].pop('input/0.txt')
    for id,width,error in [('normal-term-cap',768,None),('normal-term-over',769,'text_bytes_limit:0\n')]:
        c=case(id,[('a','a'*256)])
        v=dict(version=1,documents=[dict(id='a',bytes=256,tokens=1)],terms=[dict(term='a'*width,postings=[dict(id='a',spans=[dict(start=0,end=256,position=0)])])])
        c['files']['index/search.json']=render(v)
        c['stages']=[dict(args=['inspect','manifest.json','search.json'],exit=2 if error else 0,**(dict(stdout='',stderr=error) if error else dict(index=v)))]
    for id,projection,code,error in [('denied-tasks','tasks',2,'tasks_grant:0\n'),('denied-stdout','stdout',3,''),('denied-stderr','stderr',3,'')]:
        c=case(id,[('a','x')],error=error)
        c['deny_projection']=projection
        c['stages']=[dict(args=['build','manifest.json','search.json','4'],exit=code,stdout='',stderr=error)]
    # Independent, literal port of Tantivy's half-open offsets.
    assert scan('Hello, happy tax payer!') == [('hello',dict(start=0,end=5,position=0)),('happy',dict(start=7,end=12,position=1)),('tax',dict(start=13,end=16,position=2)),('payer',dict(start=17,end=22,position=3))]
    return cases

if __name__=='__main__':
    print(json.dumps(vectors(),ensure_ascii=False,separators=(',', ':')))

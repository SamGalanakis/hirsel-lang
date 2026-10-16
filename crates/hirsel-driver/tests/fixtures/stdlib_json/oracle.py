"""Python json oracle; preserve number token text and enforce Hirsel's stricter policy."""
import json, pathlib, random, sys
class Lexeme(str): pass
def pairs(entries):
    result={}
    for key,value in entries:
        if key in result:raise ValueError('duplicate key')
        result[key]=value
    return result
def bad_constant(value):raise ValueError(value)
def compact(value):
    if isinstance(value,Lexeme):return str(value)
    if isinstance(value,str):
        value.encode('utf-8') # reject lone surrogates admitted by Python's decoder
        return json.dumps(value,ensure_ascii=False,separators=(',',':'))
    if isinstance(value,list):return '['+','.join(map(compact,value))+']'
    if isinstance(value,dict):return '{'+','.join(compact(k)+':'+compact(v) for k,v in value.items())+'}'
    return json.dumps(value,separators=(',',':'))
def oracle(text):
    return compact(json.loads(text,parse_int=Lexeme,parse_float=Lexeme,parse_constant=bad_constant,object_pairs_hook=pairs))
def literal(s):
    out='"'
    for c in s:
        if c=='"':out+='\\"'
        elif c=='\\':out+='\\\\'
        elif c=='\n':out+='\\n'
        elif c=='\r':out+='\\r'
        elif c=='\t':out+='\\t'
        elif ord(c)<32 or ord(c)==127 or ord(c) in [0x61c,0x200e,0x200f] or 0x202a<=ord(c)<=0x202e or 0x2066<=ord(c)<=0x2069:out+='\\u{'+format(ord(c),'x')+'}'
        else:out+=c
    return out+'"'
def valid_body(vectors):
    rows=[]
    for vector in vectors:
        expected=oracle(vector['input'])
        assert expected==vector['compact'],vector['name']
        # Decoding again must retain each exact token, including signs/exponent case.
        assert oracle(expected)==expected
        rows.append('('+literal(vector['input'])+', '+literal(expected)+')')
    return '''  let cases: List[(String, String)] = [\n    '''+',\n    '.join(rows)+'''
  ]
  for (input, expected) in cases {
    let value = match std@1/json.parse(copy input) { Ok(v) => v, Error(_) => { failure.check(false, TextInvariant()); let empty: std@1/json.Value = Null(); empty } }
    match std@1/json.stringify(value) { Ok(s) => failure.check(s == expected, TextInvariant()), Error(_) => failure.check(false, TextInvariant()) }
    let mut builder: List[u8] = []
    match std@1/json.write(value, mut builder) { Ok(_) => (), Error(_) => failure.check(false, TextInvariant()) }
    match std@1/text.finish(take builder) { Ok(s) => failure.check(s == expected, TextInvariant()), Error(_) => failure.check(false, TextInvariant()) }
  }'''
def fuzz_body():
    rng=random.Random(0x4a534f4e)
    numbers=['0','-0','9007199254740993','-123.4500E+007','1e999999','0.000001','1E-000','1e-999999']
    texts=['','é😀','\x00\x01\b\f\n\r\t','"\\/','\u2028','\U0010ffff']
    def tree(depth):
        choice=rng.randrange(6 if depth<4 else 4)
        if choice==0:return 'null'
        if choice==1:return rng.choice(['true','false'])
        if choice==2:return rng.choice(numbers)
        if choice==3:return json.dumps(rng.choice(texts),ensure_ascii=rng.choice([True,False]))
        if choice==4:return '['+','.join(tree(depth+1) for _ in range(rng.randrange(4)))+']'
        return '{'+','.join(json.dumps('key'+str(i))+':'+tree(depth+1) for i in range(rng.randrange(4)))+'}'
    valid=[{'name':'fuzz-'+str(i),'input':tree(0)} for i in range(32)]
    for v in valid:v['compact']=oracle(v['input'])
    mutants=[]
    for i,v in enumerate(valid):
        text=v['input']
        edit=i%4
        candidate=text+'#' if edit==0 else text[:rng.randrange(len(text))] if edit==1 else text[:1]+','+text[1:] if edit==2 else '"\\uD800"'+text
        try: expected=oracle(candidate);ok=True
        except (ValueError,UnicodeError):expected='';ok=False
        mutants.append('('+literal(candidate)+', '+str(ok).lower()+', '+literal(expected)+')')
    body=valid_body(valid)+'''
  let mutants: List[(String, bool, String)] = [\n    '''+',\n    '.join(mutants)+'''
  ]
  for (input, valid, expected) in mutants {
    match std@1/json.parse(copy input) {
      Ok(v) => {
        failure.check(valid, TextInvariant())
        match std@1/json.stringify(v) { Ok(s) => failure.check(s == expected, TextInvariant()), Error(_) => failure.check(false, TextInvariant()) }
      },
      Error(_) => failure.check(!valid, TextInvariant())
    }
  }'''
    return body
if __name__=='__main__':
    directory=pathlib.Path(__file__).parent
    corpus=json.loads((directory/'corpus.json').read_text())
    body=fuzz_body() if '--fuzz' in sys.argv else valid_body(corpus)
    print('// rule: SEM-3\n// expect: run\npub fn main(rt: Io, args: Args) -> Result[(), OutputError] {\n'+body+'\n  Ok(())\n}')

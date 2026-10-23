"""Independent JSON/framing oracle, never calls the Hirsel codec or hub."""
import json
from pathlib import Path

def frame(value):
    data = json.dumps(value, separators=(',', ':'), ensure_ascii=False).encode()
    return list(len(data).to_bytes(4, 'big') + data)

def send(value): return {'send':frame(value)}
def expect(value): return {'expect':frame(value)}
def sub(topic='a'): return {'op':'subscribe','topic':topic}
def subscribed(topic='a'): return {'op':'subscribed','topic':topic}
def pub(data='x', topic='a'): return {'op':'publish','topic':topic,'data':data}
def delivery(seq=1, data='x', topic='a'): return {'op':'delivery','seq':seq,'topic':topic,'data':data}
def queued(seq=1, n=1): return {'op':'queued','seq':seq,'subscribers':n}
stop=[send({'op':'stop'}), expect({'op':'stopping'}), 'stall']
cases=[]
def add(name, scripts, admitted=0, queued_count=0, reason='stop', args=None, exit=0):
    cases.append({'id':name,'args':args or ['serve','/fixture/pubsub.sock','64','250','4'],
      'scripts':scripts,'admitted':admitted,'queued':queued_count,'reason':reason,'exit':exit})
add('no-subscriber',[[send(pub()),expect(queued(n=0)),*stop]],1)
add('one-receiver',[[send(sub()),expect(subscribed()),send(pub()),expect(delivery()),expect(queued()),*stop]],1,1)
add('order-two',[[send(sub()),expect(subscribed()),send(pub('one')),expect(delivery(data='one')),expect(queued()),send(pub('two')),expect(delivery(2,'two')),expect(queued(2)),*stop]],2,2)
add('duplicate',[[send(sub()),expect(subscribed()),send(sub()),expect(subscribed()),send(pub()),expect(delivery()),expect(queued()),*stop]],1,1)
add('unsubscribe',[[send(sub()),expect(subscribed()),send({'op':'unsubscribe','topic':'a'}),expect({'op':'unsubscribed','topic':'a'}),send(pub()),expect(queued(n=0)),*stop]],1)
add('late-subscribe',[[send(pub()),expect(queued(n=0)),send(sub()),expect(subscribed()),send(pub()),expect(delivery(2)),expect(queued(2)),*stop]],2,1)
add('two-receivers',[[send(sub()),expect(subscribed()),expect(delivery()),expect(delivery(2,'y')),'stall'],[send(sub()),expect(subscribed()),send(pub()),expect(delivery()),expect(queued(n=2)),send(pub('y')),expect(delivery(2,'y')),expect(queued(2,2)),*stop]],2,4)
for name,raw in [('malformed',[0,0,0,1,123]),('truncated-header',[0,0]),('truncated-body',[0,0,0,5,123]),('oversize',[0,0,8,1])]:
    add(name,[[{'send':raw},'close']],reason='eof')
add('pending-read-cancel',[['stall']],reason='deadline')
add('frame-cap',[[{'send':list((2048).to_bytes(4,'big')+b' '*2048)},'close']],reason='eof')
add('admission-cap',[],args=['serve','/fixture/pubsub.sock','65','250','4'],exit=2)
add('jobs-zero',[],args=['serve','/fixture/pubsub.sock','1','250','0'],exit=2)
add('jobs-over',[],args=['serve','/fixture/pubsub.sock','1','250','5'],exit=2)
add('duration-over',[],args=['serve','/fixture/pubsub.sock','1','10001','4'],exit=2)
add('denied',[],exit=3)
Path(__file__).with_name('cases.json').write_text('[\n'+',\n'.join(json.dumps(case,separators=(',',':')) for case in cases)+'\n]\n')

# Additional boundary partition, declared independently of the original green set.
original=cases
cases=[]
add('topic-64',[[send(sub('t'*64)),expect(subscribed('t'*64)),*stop]])
add('topic-65',[[send(sub('t'*65)),'close']],reason='eof')
add('data-192',[[send(pub('x'*192)),expect(queued(n=0)),*stop]],1)
add('data-193',[[send(pub('x'*193)),'close']],reason='eof')
full=json.dumps({'op':'stop'},separators=(',',':')).encode()
add('frame-2048',[[{'send':list((2048).to_bytes(4,'big')+full+b' '*(2048-len(full)))},expect({'op':'stopping'}),'stall']])
add('frame-zero',[[{'send':[0,0,0,0]},'close']],reason='eof')
add('invalid-utf8',[[{'send':[0,0,0,1,255]},'close']],reason='eof')
script=[]
for i in range(8):script += [send(sub(str(i))),expect(subscribed(str(i)))]
script += [send(sub('ninth')),expect({'op':'refused','reason':'topic_limit'}),*stop]
add('topics-8-plus-1',[script])
script=[]
for seq in range(1,65):script += [send(pub(str(seq))),expect(queued(seq,n=0))]
script += ['stall']
add('publishes-64',[script],64,reason='admission_limit')
add('clients-8', [['close'] for _ in range(8)],reason='eof')
add('jobs-1',[[send(pub()),expect(queued(n=0)),*stop]],1,args=['serve','/fixture/pubsub.sock','64','250','1'])
Path(__file__).with_name('boundaries.json').write_text('[\n'+',\n'.join(json.dumps(case,separators=(',',':')) for case in cases)+'\n]\n')

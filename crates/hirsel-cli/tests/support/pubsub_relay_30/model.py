"""Independent bounded event oracle, based on Tokio permit/fanout vectors.

The oracle uses sets of owned permits and immutable subscriber snapshots. It does
not execute, import or translate the application's admission implementation.
"""
from collections import defaultdict

class Model:
    def __init__(self):
        self.topics = defaultdict(set)
        self.owners = set()
        self.queued = defaultdict(list)
        self.written = defaultdict(list)
        self.seq = 0
        self.created = self.returned = self.lost = 0
    def subscribe(self, client, topic): self.topics[topic].add(client)
    def unsubscribe(self, client, topic): self.topics[topic].discard(client)
    def reserve(self, client, token):
        if sum(c == client for c,t in self.owners) == 4: return False
        assert (client,token) not in self.owners
        self.owners.add((client,token)); self.created += 1
        return True
    def release(self, client, token):
        self.owners.remove((client,token)); self.returned += 1
    def publish(self, topic):
        targets = tuple(sorted(self.topics[topic]))
        held=[]
        for client in targets:
            if not self.reserve(client,self.seq+1):
                for c,t in held: self.release(c,t)
                return False
            held.append((client,self.seq+1))
        self.seq += 1
        for client in targets: self.queued[client].append(self.seq)
        return True
    def write(self, client):
        seq = self.queued[client].pop(0)
        self.written[client].append(seq); self.release(client,seq)
    def disconnect(self, client):
        for topic in self.topics.values(): topic.discard(client)
        self.lost += len(self.queued[client]); self.queued[client].clear()
        for c,t in tuple(self.owners):
            if c==client: self.release(c,t)
    def check(self):
        assert self.created == self.returned + len(self.owners)
        assert all(sum(c==client for c,t in self.owners) <= 4 for client,t in self.owners)

def vectors():
    # Tokio send_two_recv_bounded/order/close: both see hello then world.
    m=Model()
    m.subscribe(0,'a');m.subscribe(1,'a')
    assert m.publish('a');assert m.publish('a')
    for _ in range(2):m.write(0);m.write(1)
    assert m.written[0]==m.written[1]==[1,2]
    m.disconnect(0);m.disconnect(1);m.check()
    assert not m.owners
    # Subscribe is prospective and idempotent; unsubscribe removes future sends.
    m=Model();assert m.publish('a');m.subscribe(0,'a');m.subscribe(0,'a')
    assert m.publish('a');m.write(0);m.unsubscribe(0,'a');assert m.publish('a')
    assert m.written[0]==[2];m.check()
    # Full cap and one-over; one slow receiver cannot overwrite its prefix.
    m=Model();m.subscribe(0,'a');m.subscribe(1,'a')
    for _ in range(4):assert m.publish('a');m.write(0)
    assert not m.publish('a');assert m.queued[1]==[1,2,3,4]
    assert not any(c==0 for c,t in m.owners)  # partial reservation returned
    m.unsubscribe(1,'a');m.disconnect(1);assert m.publish('a');m.write(0)
    assert m.written[0]==[1,2,3,4,5];assert m.lost==4;m.check()
    # Tokio reserve_disarm: dropping an unused reservation frees one place.
    m=Model()
    for t in range(4): assert m.reserve(0,t)
    assert not m.reserve(0,4);m.release(0,1);assert m.reserve(0,4)
    m.disconnect(0);m.check();assert m.created==m.returned
    print('PUBSUB_30_MODEL_PASS two-receivers order close late duplicate unsubscribe slow-fast partial permit-drop cap one-over')
if __name__=='__main__': vectors()

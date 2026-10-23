"""Run with the pinned upstream SimPy checkout on PYTHONPATH."""
import json
import simpy
from pathlib import Path

def fifo(capacity, durations):
    env=simpy.Environment()
    resource=simpy.Resource(env,capacity=capacity)
    log=[]
    def job(index,duration):
        with resource.request() as req:
            yield req
            yield env.timeout(duration)
            log.append([index,env.now])
    for j,d in enumerate(durations): env.process(job(j,d))
    env.run()
    return log
actual={'fifo':fifo(1,[1,1]),'capacity-two':fifo(2,[2,1,1])}
assert actual=={'fifo':[[0,1],[1,2]],'capacity-two':[[1,1],[0,2],[2,2]]}
print(json.dumps(actual,sort_keys=True))

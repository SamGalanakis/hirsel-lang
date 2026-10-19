import concurrent.futures, http.client, json, socket, sys, time
port=int(sys.argv[1])
# Two long-lived update subscribers keep broadcast/render tasks runnable.
streams=[]
for _ in range(2):
 s=socket.create_connection(('127.0.0.1',port),timeout=15)
 s.sendall(b'GET /updates?datastar=%7B%22filter%22%3A%7B%22status%22%3A%22%22%2C%22tag%22%3A%22%22%2C%22text%22%3A%22%22%2C%22sort%22%3A%22manual%22%7D%2C%22selected%22%3A0%7D HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n')
 streams.append(s)
def request(i):
 c=http.client.HTTPConnection('127.0.0.1',port,timeout=20)
 if i%3==0:
  body=json.dumps({'draft':dict(title=f'Profile {i}',description='measurement',priority='medium',due='',tags='profile')})
  c.request('POST','/tasks',body,{'Content-Type':'application/json','Connection':'close'})
 else:
  c.request('GET','/?sort=title',headers={'Connection':'close'})
 r=c.getresponse();r.read();c.close()
 if r.status!=200:raise RuntimeError((i,r.status))
 return r.status
# Open loop at 50 requests/s, 60 requests. Submission does not wait for replies.
with concurrent.futures.ThreadPoolExecutor(max_workers=60) as pool:
 start=time.monotonic();pending=[]
 for i in range(60):
  time.sleep(max(0,start+i/50-time.monotonic()))
  pending.append(pool.submit(request,i))
 for f in pending:f.result()
for s in streams:s.close()
print('60 requests, 20 creates, 2 update streams',flush=True)

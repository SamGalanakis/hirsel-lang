"""Independent persisted-state reducer: pairs, never application helpers."""
def restart(rows):
 return [(id,at,'uncertain' if status=='admitted' else status) for id,at,status in rows]
def admit(rows,id,at):
 old=next((r for r in rows if r[0]==id),None)
 if old is not None and at<=old[1]:raise ValueError('duplicate')
 return [r for r in rows if r[0]!=id]+[(id,at,'admitted')]
def complete(rows,id,at,status):
 if (id,at,'admitted') not in rows:raise ValueError('not admitted')
 return [(i,a,status if i==id else s) for i,a,s in rows]
if __name__=='__main__':
 rows=admit([], 'a',1)
 assert restart(rows)==[('a',1,'uncertain')]
 assert restart(complete(rows,'a',1,'success'))==[('a',1,'success')]
 try:admit(restart(rows),'a',1)
 except ValueError:pass
 else:raise AssertionError('uncertain reran')
 assert admit(restart(rows),'a',2)==[('a',2,'admitted')]
 print('4 transitions pass')

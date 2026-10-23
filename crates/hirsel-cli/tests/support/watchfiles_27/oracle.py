"""Independent schedule/snapshot oracle. All expected times are integer ns.

Tokio ce685ee6 zero/delay vector shapes, with Hirsel completion-relative policy:
interval=300; two batches of work=650 each -> starts 0,1600,3200;
observation rows at 1300,2900,4500. No +1 ms timer epsilon.
"""
NAMES = ['stable', 'one-change', 'missing-present', 'present-missing',
         'same-size-change', 'read-error', 'zero-interval', 'slow-no-burst',
         'stop-first', 'deadline-entry', 'deadline-idle', 'deadline-read']


def expected(which):
    if which == 6:
        return b'test log: invalid\n'
    if which in (9, 11):
        return b'test log: deadline\n'
    ticks = 1 if which in (8, 10) else 3
    rows = []
    previous = [None] * 5
    now = 0
    for tick in range(ticks):
        if which == 7:
            now += 1300  # 5 reads in groups 4+1, each group waits 650.
        for index in range(5):
            data = b'abc'
            if which == 2 and tick == 0 or which == 3 and tick > 0:
                data = None
                status = 'missing'
            elif which == 5:
                data = None
                status = 'error'
            else:
                if which == 1 and tick:
                    data = b'changed'
                if which == 4 and tick:
                    data = b'xyz'
                status = ('initial' if previous[index] is None else
                          'present' if previous[index] == data else 'changed')
            previous[index] = data
            rows.append(f'test log: {tick}:{index}:{status}:{len(data or b"")}@{now}\n')
        now += 300
    rows.append('test log: deadline\n' if which >= 9 else 'test log: complete\n')
    return ''.join(rows).encode()

if __name__ == '__main__':
    import json
    print(json.dumps([dict(id=i, name=name, stdout=expected(i).decode(), exit=0,
                           cleanup='active=0; maximum<=4; entry/zero reads=0; nested cause=Parent')
                      for i, name in enumerate(NAMES)]))

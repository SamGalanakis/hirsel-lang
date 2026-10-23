"""Independent SHA256 framing and snapshot oracle; no Hirsel output is an input.
Git t/t1007-hash-object.sh:44-111, 47fdf2208c5b (GPL-2.0-only):
empty/file/stdin/multiple vectors adapted to explicit file inputs, SHA256 only.
"""
import hashlib
import json


def frame(kind, body):
    return kind.encode() + b' ' + str(len(body)).encode() + b'\0' + body


def oid(kind, body):
    return hashlib.sha256(frame(kind, body)).hexdigest()


def canonical(entries):
    return json.dumps(sorted(entries, key=lambda e: e['name'].encode()), ensure_ascii=False,
                      separators=(',', ':')).encode()


def graph(payloads):
    objects, dirs = {}, {''}
    for path, data in payloads.items():
        objects[oid('blob', data)] = frame('blob', data)
        parts = path.split('/')
        dirs.update('/'.join(parts[:i]) for i in range(1, len(parts)))
    ids = {}
    for directory in sorted(dirs, key=lambda s: s.count('/') + bool(s), reverse=True):
        prefix = directory + '/' if directory else ''
        entries = []
        for path, data in payloads.items():
            if path.rpartition('/')[0] == directory:
                entries.append(dict(type='blob', name=path[len(prefix):], id=oid('blob', data)))
        for path in dirs:
            if path and path.rpartition('/')[0] == directory:
                entries.append(dict(type='tree', name=path[len(prefix):], id=ids[path]))
        body = canonical(entries)
        ids[directory] = oid('tree', body)
        objects[ids[directory]] = frame('tree', body)
    return ids[''], objects


def cases():
    cells = []

    def add(name, args, files=None, output=b'', error=None, after=None, **extra):
        files = files or {}
        expected = dict(files)
        expected.update(after or {})
        cell = dict(id=name, args=args, files={k: v.hex() for k, v in files.items()},
                    after={k: v.hex() for k, v in expected.items()}, stdout=output.hex(),
                    stderr=(json.dumps(dict(kind=error[0], path=error[1]), separators=(',', ':'),
                                       ensure_ascii=False).encode()+b'\n').hex() if error else '',
                    exit=2 if error else 0, steps=8_000_000 if args[0] == 'snapshot' else 1_000_000)
        cell.update(extra)
        cells.append(cell)
        return cell

    add('argument-1025', ['put', 'x'*1025], error=('argument_limit',''),boundary=True)
    for name, data in [('empty', b''), ('hello', b'hello\n'), ('stdin-example', b'example\n'),
                       ('binary', bytes([0, 255, 128, 10])), ('foo', b'foo\n'), ('bar', b'bar\n')]:
        identity = oid('blob', data)
        files = {'source/input': data}
        stored = {'objects/'+identity: frame('blob', data)}
        add('put-'+name, ['put', 'input'], files, (identity+'\n').encode(), after=stored)
        add('get-'+name, ['get', identity], stored, data)
    identity = oid('blob', b'hello\n')
    add('put-repeat', ['put', 'input'], {'source/input': b'hello\n', 'objects/'+identity: frame('blob', b'hello\n')}, (identity+'\n').encode())
    add('put-corrupt-existing', ['put', 'input'], {'source/input': b'hello\n', 'objects/'+identity: b'corrupt'}, error=('digest', identity))
    add('get-invalid-id', ['get', '../escape'], error=('invalid_id', '../escape'))
    add('get-missing', ['get', '0'*64], error=('missing', '0'*64))
    for label, raw in [('wrong-type', b'tree 0\0'), ('bad-length', b'blob 01\0x'),
                       ('bad-header', b'blob 1 x'), ('long-header', b'b'*32+b'\0')]:
        identity = hashlib.sha256(raw).hexdigest()
        add('get-'+label, ['get', identity], {'objects/'+identity: raw}, error=('header', identity))
    identity = oid('blob', b'x')
    add('get-digest', ['get', identity], {'objects/'+identity: b'blob 1\0y'}, error=('digest', identity))

    def snap(label, payloads, **extra):
        root, objects = graph(payloads)
        manifest = [dict(path=p, size=len(data), id=oid('blob', data)) for p, data in payloads.items()]
        files = {'source/'+p: b for p, b in payloads.items()}
        files['source/manifest'] = json.dumps(manifest, ensure_ascii=False, separators=(',', ':')).encode()
        after = {'objects/'+k: v for k, v in objects.items()}
        after['roots/head'] = (root+'\n').encode()
        cell = add(label, ['snapshot', 'manifest', 'head', '4'], files, (root+'\n').encode(), after=after, **extra)
        return cell, root, objects

    snap('snapshot-empty', {})
    snap('snapshot-one', {'hello': b'hello\n'})
    race, root, objects = snap('snapshot-race', {'a': b'x', 'b': b'x', 'dir/c': b'\0\xff'})
    initial = {'objects/'+k: v for k, v in objects.items()}
    initial['roots/head'] = (root+'\n').encode()
    root_body = objects[root].split(b'\0', 1)[1]
    add('list', ['list', 'head'], initial, root_body+b'\n')
    add('verify', ['verify', 'head'], initial, b'{"objects":4,"bytes":3}\n')
    bad = dict(initial)
    missing = oid('blob', b'\0\xff')
    del bad['objects/'+missing]
    add('verify-missing-child', ['verify', 'head'], bad, error=('missing', missing))
    for label, entries in [
        ('wrong-reference', [dict(type='tree', name='a', id=oid('blob', b'x'))]),
        ('duplicate-entry', [dict(type='blob', name='a', id=oid('blob', b'x'))]*2),
        ('escape-entry', [dict(type='blob', name='../a', id=oid('blob', b'x'))]),
    ]:
        body = canonical(entries); identity = oid('tree', body)
        files = {'objects/'+identity: frame('tree', body), 'objects/'+oid('blob', b'x'): frame('blob', b'x'), 'roots/head': (identity+'\n').encode()}
        error = ('header', oid('blob', b'x')) if label == 'wrong-reference' else ('entry_order', 'a') if label == 'duplicate-entry' else ('invalid_path', '../a')
        add('verify-'+label, ['verify', 'head'], files, error=error)
    body=b'[ ]'; identity=oid('tree',body)
    add('list-noncanonical', ['list','head'], {'objects/'+identity:frame('tree',body),'roots/head':(identity+'\n').encode()}, error=('tree_canonical',''))
    add('root-format', ['list','head'], {'roots/head':b'bad'}, error=('root_format','head'))

    def rejected(label, rows, error, files=None, **extra):
        inputs = {'source/manifest': json.dumps(rows, ensure_ascii=False, separators=(',', ':')).encode()}
        inputs.update(files or {})
        return add(label, ['snapshot','manifest','head','4'], inputs, error=error, **extra)
    row = dict(path='a',size=1,id=oid('blob',b'x'))
    rejected('duplicate-path', [row,row], ('duplicate_path','a'))
    for path in ['../a','a/../b','/a','a//b','./a','a\\b','a:foo']:
        rejected('escape-'+str(len(cells)-1), [dict(row,path=path)], ('invalid_path',path))
    rejected('path-collision',[row,dict(row,path='a/b')],('path_collision','a/b'))
    rejected('mutation',[row],('input_changed','a'),{'source/a':b'y'})
    rejected('symlink',[row],('symlink','a'), links={'source/a':'target'})
    rejected('parent-symlink',[dict(row,path='d/a')],('symlink','d/a'),links={'source/d':'target'})
    canceled = rejected('pending-cancel',[row,dict(row,path='b'),dict(row,path='c')],('input_changed','b'),{'source/a':b'x','source/b':b'y','source/c':b'x'}, pending='source/a')
    canceled['args'][-1]='2'
    rejected('signal-pending-cancel',[row],('canceled',''),{'source/a':b'x'}, pending='source/a', signals=True)
    signal_success, _, _ = snap('signal-success', {'a':b'x'}, signals=True)
    signal_success.pop('signals')
    signal_success['signal_watch']=True
    emptyroot = oid('tree',b'[]')
    rejected('root-readonly',[],('root_publish','head'), after={'objects/'+emptyroot:frame('tree',b'[]')}, readonly=True)
    rejected('root-conflict',[],('root_exists','head'),{'roots/head':b'old'}, after={'objects/'+emptyroot:frame('tree',b'[]')})
    rejected('jobs-zero',[],('jobs_limit',''))['args'][-1]='0'
    rejected('jobs-five',[],('jobs_limit',''))['args'][-1]='5'
    rejected('files-33',[dict(row,path=f'f{i}') for i in range(33)],('files_limit',''),boundary=True)
    rejected('blob-16385',[dict(row,size=16385)],('blob_limit','a'),boundary=True)
    rejected('total-over',[dict(row,path=f'f{i}',size=16384) for i in range(9)],('total_limit','f8'),boundary=True)
    rejected('depth-9',[dict(row,path='/'.join(['d']*8+['a']))],('depth_limit','/'.join(['d']*8+['a'])),boundary=True)
    rejected('name-1025',[dict(row,path='n'*1025)],('name_limit',''),boundary=True)
    rejected('objects-over',[dict(row,path=f'd{i}/a/b/c/d/e/f/g') for i in range(19)],('objects_limit','d15/a/b/c'),boundary=True)
    entries=[dict(type='blob',name=f'f{i:02}',id=oid('blob',b'')) for i in range(33)]
    body=canonical(entries); identity=oid('tree',body)
    add('entries-33',['list','head'],{'objects/'+identity:frame('tree',body),'roots/head':(identity+'\n').encode()},error=('entries_limit',''),boundary=True)
    cap={f'd{i}/a/b/c/d/e/f/g':bytes([i]) for i in range(13)}
    cap['d13/a/b/c/z']=b'X'
    cap.update({f'f{i}':bytes([i]) for i in range(14,32)})
    snap('objects-128',cap,boundary=True)
    # True caps stay intact even if a reference tooling budget prevents completion.
    snap('files-32',{f'f{i}':b'' for i in range(32)},boundary=True)
    snap('blob-16384',{'a':b'x'*16384},boundary=True)
    snap('total-131072',{f'f{i}':bytes([i])*16384 for i in range(8)},boundary=True)
    snap('depth-8',{'d/d/d/d/d/d/d/a':b''},boundary=True)
    # Names are logical UTF-8 paths, with components that fit a host filesystem.
    longname='/'.join(['n'*255]*3+['n'*254,'n'])
    snap('name-1024',{longname:b''},boundary=True)
    # The index is byte-bounded independently of its row count.
    add('manifest-262144',['snapshot','manifest','head','4'],{'source/manifest':b'[]'+b' '*262142},(emptyroot+'\n').encode(),after={'objects/'+emptyroot:frame('tree',b'[]'),'roots/head':(emptyroot+'\n').encode()},boundary=True)
    add('manifest-262145',['snapshot','manifest','head','4'],{'source/manifest':b'[]'+b' '*262143},error=('input_limit','manifest'),boundary=True)
    for c in cells:
        if c.get('boundary'): c['steps']=40_000_000
    return cells


if __name__ == '__main__':
    print(json.dumps(cases(), ensure_ascii=False, separators=(',', ':')))

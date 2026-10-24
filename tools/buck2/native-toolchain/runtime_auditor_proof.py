#!/usr/bin/env python3
"""Exercise the actual audit DSOs at the supported glibc floor."""
import argparse
import os
from pathlib import Path
import shutil
import subprocess
import tempfile


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('llvm', type=Path)
    parser.add_argument('auditors', type=Path)
    parser.add_argument('floor', type=Path)
    parser.add_argument('qemu', type=Path)
    parser.add_argument('--resume-x86-refusals', action='store_true')
    args = parser.parse_args()
    llvm = args.llvm.resolve()
    tools = dict(os.environ, LD_LIBRARY_PATH=str(llvm / 'lib'))

    def command(argv, **kwargs):
        return subprocess.run([str(a) for a in argv], capture_output=True, **kwargs)

    def checked(argv):
        result = command(argv, env=tools)
        assert result.returncode == 0, (argv, result.stderr)
        return result

    for target, register, number, instruction, syscall in [
        ('x86_64-unknown-linux-gnu', '%rax', '$42', 'mov $60,%rax', 'syscall'),
        ('aarch64-unknown-linux-gnu', 'x0', '#42', 'mov x8,#93', 'svc #0'),
    ]:
        with tempfile.TemporaryDirectory(prefix='hirsel-runtime-auditor-') as raw:
            root = Path(raw)
            original = root / 'original with spaces\nand newline'
            original.mkdir()
            frozen = root / 'frozen'
            frozen.mkdir()
            loader = (args.floor / target / 'ld-2.28.so').resolve()
            audit = (args.auditors / (target + '.so')).resolve()
            prefix = [] if target.startswith('x86_64') else [args.qemu.resolve() / 'qemu-aarch64']

            def assemble(name, text):
                source = root / (name + '.s')
                obj = root / (name + '.o')
                source.write_text(text)
                checked([llvm / 'bin/llvm-mc', '-triple=' + target, '-filetype=obj', source, '-o', obj])
                return obj

            leaf = original / 'libleaf.so'
            leaf_object = assemble('leaf', f'.globl leaf\nleaf:\n mov {register},{number}\n ret\n' if prefix else '.globl leaf\nleaf:\n mov $42,%rax\n ret\n')
            checked([llvm / 'bin/ld.lld', '-shared', '--threads=1', '--soname=libleaf.so', leaf_object, '-o', leaf])
            top = original / 'top.so'
            top_object = assemble('top', '.globl top\ntop:\n b leaf\n' if prefix else '.globl top\ntop:\n jmp leaf@PLT\n')
            checked([llvm / 'bin/ld.lld', '-shared', '--threads=1', '-rpath', '$ORIGIN', top_object, leaf, '-o', top])
            start = ('.globl _start\n_start:\n bl top\n' + instruction + '\n' + syscall + '\n') if prefix else '.globl _start\n_start:\n call top@PLT\n mov %rax,%rdi\n mov $60,%rax\n syscall\n'
            obj = assemble('start', start)
            program = root / 'program'
            checked([llvm / 'bin/ld.lld', '--threads=1', '--allow-shlib-undefined', '--dynamic-linker=' + str(loader), obj, top, '-o', program])
            mapping = {str(top): frozen / 'top.so', 'libleaf.so': frozen / 'leaf.so'}
            for request, source in [(str(top), top), ('libleaf.so', leaf)]:
                shutil.copyfile(source, mapping[request])
            map_file = root / 'map'

            def launch(mode, pairs, *, events=None, limit=8192, listing=False, raw=None, size=None):
                data = raw if raw is not None else mode.encode() + b''.join(
                    os.fsencode(key) + b'\0' + os.fsencode(value) + b'\0' for key, value in sorted(pairs.items())
                )
                map_file.write_bytes(data)
                env = {'HIRSEL_RUNTIME_MAP': str(map_file), 'HIRSEL_RUNTIME_MAP_LENGTH': str(len(data) if size is None else size)}
                if events is not None:
                    env.update(HIRSEL_RUNTIME_EVENTS=str(events), HIRSEL_RUNTIME_EVENTS_LIMIT=str(limit))
                return command([*prefix, loader, '--audit', audit, *(['--list'] if listing else []), program], env=env)

            if not (args.resume_x86_refusals and not prefix):
                events = root / 'events'
                captured = launch('C', {}, events=events, listing=True)
                assert captured.returncode == 0, captured.stderr
                records = events.read_bytes().split(b'\0')
                assert os.fsencode('S' + str(top)) in records, records
                assert os.fsencode('O' + str(leaf)) in records, records
                print(target + ' bounded discovery events preserve original spaces/newlines and $ORIGIN: PASS', flush=True)
                shutil.rmtree(original)
                replay = launch('R', mapping)
                assert replay.returncode == 42, (replay.returncode, replay.stderr)
                print(target + ' absolute noSONAME + transitive SONAME replay after originals removed: PASS', flush=True)
            original.mkdir(exist_ok=True)
            shutil.copyfile(mapping[str(top)], top)
            shutil.copyfile(mapping['libleaf.so'], leaf)
            refused = launch('R', {str(top): mapping[str(top)]})
            assert refused.returncode != 42 and b'libleaf.so' in refused.stderr, refused.stderr
            print(target + ' uncaptured transitive DSO refused with original RUNPATH restored: PASS', flush=True)
            for label, kwargs in [
                ('unterminated map', {'raw': b'Rname\0/path'}),
                ('duplicate requests', {'raw': b'Ra\0/path\0a\0/other\0'}),
                ('map length mismatch', {'size': 100000}),
                ('discovery event cap', {'events': root / 'small-events', 'limit': 1, 'listing': True}),
            ]:
                refused = launch('C' if kwargs.get('events') else 'R', mapping, **kwargs)
                assert refused.returncode == 127, (label, refused.returncode, refused.stderr)
                print(target + ' ' + label + ' refused before target entry: PASS', flush=True)


if __name__ == '__main__':
    main()

#!/usr/bin/env python3
"""Bundle native dependencies into rustc's staticlib with LLVM ar's MRI API.

RustLinkInfo identifies the native archives. llvm-ar ADDLIB expands both normal
and thin archives, preserving duplicate member names (extracting to one directory
would overwrite them). The result is one ordinary, deterministic archive, just
as Cargo's rustc -l static:+bundle produces. No object-code format is parsed here.
"""
import argparse
from pathlib import Path
import subprocess
import tempfile


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--ar', required=True)
    parser.add_argument('--output', required=True)
    parser.add_argument('--archive', action='append', required=True)
    parser.add_argument('--object', action='append', default=[])
    parser.add_argument('--member', nargs=2, action='append', default=[], metavar=('ARCHIVE', 'NAME'))
    args = parser.parse_args()
    output = Path(args.output)
    output.parent.mkdir(parents=True, exist_ok=True)
    # All paths are Buck2 artifacts; quote them for the MRI grammar, never a shell.
    def quoted(path):
        if any(c in str(path) for c in '\n\r"'):
            raise ValueError('archive artifact path cannot be represented in MRI')
        return '"' + str(path) + '"'
    with tempfile.TemporaryDirectory(prefix='hirsel-archive-members-') as scratch:
        objects = list(args.object)
        for index, (archive, member) in enumerate(args.member):
            path = Path(scratch) / f'{index}.o'
            with path.open('wb') as destination:
                subprocess.run([args.ar, 'p', archive, member], stdout=destination, check=True)
            if path.stat().st_size == 0:
                raise ValueError(f'{archive}: missing archive member {member}')
            objects.append(path)
        script = ['CREATE ' + quoted(output)]
        script.extend('ADDLIB ' + quoted(path) for path in args.archive)
        script.extend('ADDMOD ' + quoted(path) for path in objects)
        script.extend(['SAVE', 'END', ''])
        subprocess.run([args.ar, '-M'], input='\n'.join(script), text=True, check=True)


if __name__ == '__main__':
    main()

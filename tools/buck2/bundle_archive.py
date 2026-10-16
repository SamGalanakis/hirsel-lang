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


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--ar', required=True)
    parser.add_argument('--output', required=True)
    parser.add_argument('--archive', action='append', required=True)
    parser.add_argument('--object', action='append', default=[])
    args = parser.parse_args()
    output = Path(args.output)
    output.parent.mkdir(parents=True, exist_ok=True)
    # All paths are Buck2 artifacts; quote them for the MRI grammar, never a shell.
    def quoted(path):
        if any(c in str(path) for c in '\n\r"'):
            raise ValueError('archive artifact path cannot be represented in MRI')
        return '"' + str(path) + '"'
    script = ['CREATE ' + quoted(output)]
    script.extend('ADDLIB ' + quoted(path) for path in args.archive)
    script.extend('ADDMOD ' + quoted(path) for path in args.object)
    script.extend(['SAVE', 'END', ''])
    subprocess.run([args.ar, '-M'], input='\n'.join(script), text=True, check=True)


if __name__ == '__main__':
    main()

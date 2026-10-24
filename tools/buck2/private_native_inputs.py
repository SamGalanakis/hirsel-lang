#!/usr/bin/env python3
"""Give one native test action private inodes for the tools it hashes.

The executor may materialize immutable CAS inputs with hard links. Another
materialization can change their ctime without changing bytes. Copy top-level
bin/lib files once before native hashing; retain aliases only within this owner.
Other SDK directories remain read-only inputs. GNU cp's --reflink=auto uses
copy-on-write where available and copies bytes otherwise; it never links back
into the CAS. No digest or metadata verification policy changes here.
"""
import argparse
import os
from pathlib import Path
import subprocess


def materialize(source: Path, destination: Path) -> Path:
    source = source.resolve(strict=True)
    destination.mkdir(mode=0o700)
    copies = {}
    for entry in source.iterdir():
        target = destination / entry.name
        if entry.name not in ('bin', 'lib'):
            target.symlink_to(entry, target_is_directory=entry.is_dir())
            continue
        target.mkdir()
        for item in entry.iterdir():
            output = target / item.name
            if item.is_file():
                metadata = item.stat()
                key = (metadata.st_dev, metadata.st_ino)
                previous = copies.get(key)
                if previous is None:
                    subprocess.run(['cp', '--reflink=auto', '--preserve=mode,timestamps',
                                    '--', str(item), str(output)], check=True)
                    copies[key] = output
                else:
                    # All alias creation completes before any native read.
                    os.link(previous, output)
            elif item.is_dir():
                output.symlink_to(item.resolve(strict=True), target_is_directory=True)
            else:
                raise ValueError(f'unsupported native SDK input: {item}')
    return destination.resolve(strict=True)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('source', type=Path)
    parser.add_argument('destination', type=Path)
    args = parser.parse_args()
    print(materialize(args.source, args.destination))


if __name__ == '__main__':
    main()

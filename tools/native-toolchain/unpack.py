#!/usr/bin/env python3
"""Unpack verified Buck2 downloads, resolving symlinks before they enter the CAS."""
from pathlib import Path
import shutil
import subprocess
import sys
import tarfile
import tempfile


def copy_tree(source, destination):
    shutil.copytree(source, destination, symlinks=False, dirs_exist_ok=True)


def unpack(kind, output, archives):
    with tempfile.TemporaryDirectory(prefix="hirsel-native-unpack-") as scratch:
        tree = Path(scratch)
        if kind == "arm":
            with tarfile.open(archives[0]) as archive:
                archive.extractall(tree, filter="tar")
            copy_tree(next(tree.iterdir()), output)
            return
        for archive in archives:
            subprocess.run(["dpkg-deb", "-x", archive, tree], check=True)
        if kind == "llvm":
            prefix = tree / "usr/lib/llvm-21"
            for name in ("bin", "include"):
                copy_tree(prefix / name, output / name)
            copy_tree(prefix / "lib/clang", output / "lib/clang")
            for source in (prefix / "lib").glob("*.so*"):
                shutil.copy2(source, output / "lib" / source.name)
            for source in (tree / "usr/lib/x86_64-linux-gnu").glob("*.so*"):
                if source.is_file():
                    shutil.copy2(source, output / "lib" / source.name)
        elif kind == "qemu":
            output.mkdir()
            shutil.copy2(tree / "usr/bin/qemu-aarch64-static", output / "qemu-aarch64")
        elif kind == "valgrind":
            copy_tree(tree / "usr", output)
        else:
            raise ValueError(kind)


if __name__ == "__main__":
    unpack(sys.argv[1], Path(sys.argv[2]), sys.argv[3:])

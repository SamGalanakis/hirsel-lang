#!/usr/bin/env python3
"""Unpack verified Buck2 downloads, resolving symlinks before they enter the CAS."""
from pathlib import Path
import os
import re
import shutil
import subprocess
import sys
import tarfile
import tempfile


def copy_tree(source, destination):
    shutil.copytree(source, destination, symlinks=False, dirs_exist_ok=True)


def normalize_links(tree):
    # Debian's usr-merged runtime packages can contain absolute /lib links.
    # Resolve them inside the archive namespace, never against the unpack host.
    for name in ("lib", "lib64"):
        if (tree / "usr" / name).is_dir() and not (tree / name).exists():
            (tree / name).symlink_to("usr/" + name)
    for source in tree.rglob("*"):
        if source.is_symlink():
            target = source.readlink()
            if target.is_absolute():
                source.unlink()
                source.symlink_to(os.path.relpath(tree / str(target).lstrip("/"), source.parent))
            if not source.resolve().is_relative_to(tree):
                raise ValueError(f"archive link escapes its root: {source}")


def place_runtime_debug(output):
    # Valgrind searches the ELF's adjacent .debug/ directory by .gnu_debuglink.
    # A global extra-debuginfo prefix cannot locate a relocated object's symbols.
    for library in (output / "lib/x86_64-linux-gnu").glob("*.so*"):
        if not library.is_file():
            continue
        notes = subprocess.check_output(["readelf", "-n", library], text=True, env={"PATH": "/usr/bin:/bin", "LC_ALL": "C"})
        identity = re.search(r"Build ID: ([0-9a-f]+)", notes)
        if not identity:
            continue
        build_id = identity[1]
        debug = output / "lib/debug/.build-id" / build_id[:2] / (build_id[2:] + ".debug")
        if not debug.is_file():
            continue
        section = subprocess.check_output(["readelf", "--string-dump=.gnu_debuglink", library], env={"PATH": "/usr/bin:/bin", "LC_ALL": "C"})
        # readelf also prints the binary CRC after the filename; it need not be UTF-8.
        link = re.search(rb"\[\s*0\]\s+(\S+)", section)
        if not link:
            raise ValueError(f"no debuglink for {library}")
        adjacent = library.parent / ".debug" / link[1].decode("ascii")
        adjacent.parent.mkdir(exist_ok=True)
        shutil.copyfile(debug, adjacent)


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
        normalize_links(tree)
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
            place_runtime_debug(output)
        else:
            raise ValueError(kind)


if __name__ == "__main__":
    unpack(sys.argv[1], Path(sys.argv[2]), sys.argv[3:])

#!/usr/bin/env python3
"""The two-host link gate: build a fixed program set and record, or compare, its bytes.

    link_hosts.py build --hirsel H --list LIST --output DIGESTS
    link_hosts.py check --hirsel H --list LIST [--local DIGESTS]

Each row of LIST is `<source or manifest> <target> <default|debug|release>
[fixed-root]`. Every build runs `hirsel build` from the repository root with a
fresh native cache (`HIRSEL_NATIVE_CACHE`) and scratch directory (`TMPDIR`)
under a private root, so two hosts differ in checkout, cache and scratch
directories. A `fixed-root` row instead runs from a copy of its manifest,
sources and dependency manifests at FIXED_ROOT, the same path on every host,
for programs whose output still embeds absolute source paths. The
record is one line per output file: the row, the file's role (`executable`, or
the debug file's `<build-id>.debug` name) and its SHA-256. `check` builds on
this host and requires exactly the record `build` made on another
(`--local`, or `HIRSEL_LINK_HOSTS_LOCAL`).
"""
import argparse
import fcntl
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile

PROFILES = {"default": [], "debug": ["--debug"], "release": ["--release"]}
FIXED_ROOT = Path("/tmp/hirsel-link-hosts-root")


def rows(listing):
    for line in Path(listing).read_text().splitlines():
        if line and not line.startswith("#"):
            source, target, profile, *flags = line.split()
            if flags not in ([], ["fixed-root"]):
                sys.exit(f"{listing}: unknown flags {flags}")
            yield source, target, profile, bool(flags)


def package_files(manifest):
    """`manifest`, its sources and, recursively, its dependencies' manifests and sources."""
    files, pending = set(), [Path(manifest)]
    while pending:
        manifest = Path(os.path.normpath(pending.pop()))
        if manifest in files:
            continue
        files.add(manifest)
        declared = json.loads(manifest.read_text())
        files.update(Path(os.path.normpath(manifest.parent / s["path"])) for s in declared["sources"])
        pending.extend(manifest.parent / d["manifest"] for d in declared.get("dependencies", []))
    return files


def build(command, environment, source, fixed):
    if not fixed:
        return subprocess.run(command, env=environment, capture_output=True, text=True)
    # One fixed-root build at a time on a host; the copy is rebuilt for each.
    with open(f"{FIXED_ROOT}.lock", "a") as lock:
        fcntl.flock(lock, fcntl.LOCK_EX)
        shutil.rmtree(FIXED_ROOT, ignore_errors=True)
        for file in package_files(source):
            if file.is_absolute() or ".." in file.parts:
                sys.exit(f"{source}: {file} is outside the repository")
            (FIXED_ROOT / file).parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(file, FIXED_ROOT / file)
        try:
            return subprocess.run(command, env=environment, capture_output=True, text=True, cwd=FIXED_ROOT)
        finally:
            shutil.rmtree(FIXED_ROOT, ignore_errors=True)


def record(hirsel, listing):
    lines = []
    with tempfile.TemporaryDirectory(prefix="hirsel-link-hosts-") as root:
        root = Path(root)
        environment = dict(os.environ)
        environment["HIRSEL_NATIVE_CACHE"] = str(root / "cache")
        environment["TMPDIR"] = str(root / "tmp")
        (root / "tmp").mkdir()
        # Tool inputs may name repository-relative paths; the driver takes absolute ones.
        for name in ("LLVM_SYS_231_PREFIX", "HIRSEL_LINK_KIT", "HIRSEL_RUNTIME_ARCHIVES", "HIRSEL_PRODUCTS",
                     "HIRSEL_QEMU_AARCH64", "HIRSEL_AARCH64_SYSROOT"):
            if name in environment:
                environment[name] = str(Path(environment[name]).resolve())
        environment["LD_LIBRARY_PATH"] = environment["LLVM_SYS_231_PREFIX"] + "/lib"
        for index, (source, target, profile, fixed) in enumerate(rows(listing)):
            out = root / "out" / str(index)
            out.mkdir(parents=True)
            command = [hirsel, "build", source, "--target", target, *PROFILES[profile], "-o", str(out / "app")]
            built = build(command, environment, source, fixed)
            if built.returncode != 0:
                sys.exit(f"{' '.join(command)} failed:\n{built.stdout}{built.stderr}")
            for path in sorted(out.iterdir()):
                role = "executable" if path.name == "app" else path.name
                digest = hashlib.sha256(path.read_bytes()).hexdigest()
                lines.append(f"{source} {target} {profile}{' fixed-root' if fixed else ''} {role} {digest}")
    return "\n".join(lines) + "\n"


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("mode", choices=["build", "check"])
    parser.add_argument("--hirsel", required=True)
    parser.add_argument("--list", required=True)
    parser.add_argument("--output")
    parser.add_argument("--local")
    args = parser.parse_args()
    hirsel = str(Path(args.hirsel).resolve())
    here = record(hirsel, args.list)
    if args.mode == "build":
        Path(args.output).write_text(here)
        return
    there = Path(args.local or os.environ["HIRSEL_LINK_HOSTS_LOCAL"]).read_text()
    if here != there:
        local, pool = set(there.splitlines()), set(here.splitlines())
        print("two-host link gate: outputs differ between hosts", file=sys.stderr)
        for line in sorted(local - pool):
            print(f"  local only: {line}", file=sys.stderr)
        for line in sorted(pool - local):
            print(f"  this host only: {line}", file=sys.stderr)
        sys.exit(1)
    print(f"two-host link gate: {len(here.splitlines())} files identical across hosts")


if __name__ == "__main__":
    main()

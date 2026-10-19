#!/usr/bin/env python3
"""Derive a runtime archive's link forms: debug-stripped, and its compiler builtins alone.

The stripped archive is what default links read; the debug-bearing input stays
beside it for `hirsel build --debug`. The builtins archive holds the archive's
`compiler_builtins` members in archive order: the integer and float helpers a
program library (which links no runtime) needs, the same implementations the
runtime archive gives an executable. Both are deterministic.
"""
import argparse
from pathlib import Path
import subprocess
import tempfile


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--llvm", required=True, help="the pinned LLVM tree")
    parser.add_argument("--archive", required=True)
    parser.add_argument("--stripped", required=True)
    parser.add_argument("--builtins")
    args = parser.parse_args()
    tree = Path(args.llvm).resolve()
    environment = {"LD_LIBRARY_PATH": str(tree / "lib"), "PATH": "/usr/bin:/bin"}
    tool = lambda name: str(tree / "bin" / name)
    for output in filter(None, [args.stripped, args.builtins]):
        Path(output).parent.mkdir(parents=True, exist_ok=True)
    subprocess.run([tool("llvm-objcopy"), "--strip-debug", args.archive, args.stripped], check=True, env=environment)
    if args.builtins:
        members = subprocess.check_output([tool("llvm-ar"), "t", args.stripped], text=True, env=environment).split()
        chosen = [m for m in members if m.startswith("compiler_builtins-")]
        if not chosen or len(set(chosen)) != len(chosen):
            raise SystemExit(f"{args.stripped}: {len(chosen)} compiler_builtins members, or duplicate names")
        with tempfile.TemporaryDirectory(prefix="hirsel-runtime-parts-") as scratch:
            subprocess.run([tool("llvm-ar"), "x", f"--output={scratch}", str(Path(args.stripped).resolve()), *chosen], check=True, env=environment)
            subprocess.run([tool("llvm-ar"), "rcsD", str(Path(args.builtins).resolve()), *chosen], check=True, cwd=scratch, env=environment)


if __name__ == "__main__":
    main()

#!/usr/bin/env python3
"""Build the pinned ELF linker with LLVM VFS input admission.

The LLVM script parser and archive reader remain the only implementations.
The patch routes their reads through LLVM's existing RedirectingFileSystem;
capture stops after parseFiles, before LTO or final script evaluation.
"""
from pathlib import Path
import argparse
import json
import os
import re
import shutil
import subprocess
import tarfile
import tempfile


def run(command, env, **kwargs):
    return subprocess.run([str(arg) for arg in command], env=env, check=True, **kwargs)


def build(args):
    llvm = args.llvm.resolve()
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    env = {**os.environ, "LD_LIBRARY_PATH": str(llvm / "lib")}
    with tempfile.TemporaryDirectory(prefix="hirsel-lld-") as raw:
        scratch = Path(raw)
        source = scratch / "source"
        source.mkdir()
        # Read only LLD and its license from the verified LLVM release archive.
        with tarfile.open(args.source, mode="r|*") as archive:
            for member in archive:
                parts = Path(member.name).parts
                if (len(parts) == 2 and parts[1] == "LICENSE.TXT") or (
                    len(parts) > 2 and parts[1] == "lld"
                    and parts[2] in ("ELF", "Common", "include")
                ):
                    archive.extract(member, source, filter="data")
        roots = list(source.iterdir())
        if len(roots) != 1:
            raise ValueError("LLVM release archive has no unique source root")
        root = roots[0]
        run([args.patch_tool.resolve() / "bin/patch", "--batch", "-p1", "-i", args.patch.resolve()], env, cwd=root)
        include = scratch / "include/lld/Common"
        include.mkdir(parents=True)
        (include / "Version.inc").write_text(f'#define LLD_VERSION_STRING "{args.version}"\n')
        (scratch / "VCSVersion.inc").write_text("// Pinned LLVM release.\n")
        # GCC 13's Ubuntu configuration advertises pthread clock APIs added
        # after the SDK's glibc 2.28 headers. libstdc++ supplies timed-wait
        # fallbacks for these features (mutex:186/373, bits/std_mutex.h:181);
        # select the actual SDK header capabilities.
        floor = scratch / "libstdcxx-floor.h"
        floor.write_text("#include <bits/c++config.h>\n"
                         "#undef _GLIBCXX_USE_PTHREAD_COND_CLOCKWAIT\n"
                         "#undef _GLIBCXX_USE_PTHREAD_MUTEX_CLOCKLOCK\n"
                         "#undef _GLIBCXX_USE_PTHREAD_RWLOCK_CLOCKLOCK\n")
        options = scratch / "Options.inc"
        run([llvm / "bin/llvm-tblgen", "-gen-opt-parser-defs",
             "-I", llvm / "include", "-I", root / "lld/include",
             root / "lld/ELF/Options.td", "-o", options], env)
        sysroot = llvm / "sysroot"
        compiler = llvm / "bin/hirsel-clang++"
        flags = ["-std=c++17", "-O2", "-DNDEBUG", "-fPIC", "-fno-exceptions",
                 "-fno-rtti", "-ffunction-sections", "-fdata-sections", "-nostdinc++",
                 f"--sysroot={sysroot}", "-isystem", sysroot / "usr/include/c++/13",
                 "-isystem", sysroot / "usr/include/x86_64-linux-gnu/c++/13",
                 "-I", llvm / "include", "-I", root / "lld/include",
                 "-I", scratch / "include", "-I", root / "lld/ELF", "-I", scratch,
                 "-include", floor,
                 f"-ffile-prefix-map={root}=/llvm-source"]
        for headers in args.headers:
            flags.extend(["-isystem", headers.resolve() / "usr/include"])
            flags.extend(["-isystem", headers.resolve() / "usr/include/x86_64-linux-gnu"])
        sources = sorted((root / "lld/ELF").glob("**/*.cpp"))
        sources += sorted((root / "lld/Common").glob("*.cpp"))
        sources.append(args.main.resolve())
        print(f"Compiling {len(sources)} pinned LLD ELF/Common sources", flush=True)
        objects = []
        def compile_one(item):
            index, source_file = item
            target = scratch / f"{index}.o"
            run([compiler, *flags, "-c", source_file, "-o", target], env)
            return target
        objects = [compile_one(item) for item in enumerate(sources)]
        # Use the same pinned startup and libc files as other native links.
        manifest = json.loads((args.kit / "manifest.json").read_text())
        target = manifest["targets"]["x86_64-unknown-linux-gnu"]
        kit = args.kit.resolve() / "x86_64-unknown-linux-gnu"
        binary = scratch / "ld.lld"
        # The compiler itself runs on the pool's host libc, as the pinned LLVM
        # executables do; the kit startup objects also run on newer glibc.
        libraries = [Path("/lib/x86_64-linux-gnu") / name
                     for name in ("libc.so.6", "libm.so.6", "libz.so.1", "libzstd.so.1")]
        run([llvm / "bin/ld.lld", "-pie", "--threads=1", "--gc-sections",
             "--dynamic-linker=" + target["interpreter"], "-rpath", "$ORIGIN/../lib",
             *(kit / name for name in target["start"]), *objects,
             llvm / "lib/libLLVM.so.23.1", "-L/lib/x86_64-linux-gnu",
             "-L/usr/lib/x86_64-linux-gnu", "-l:libstdc++.so.6", *libraries,
             kit / target["nonshared"], *(kit / name for name in target["end"]),
             "-o", binary], env)
        shutil.copy2(root / "LICENSE.TXT", output / "LICENSE.TXT")
        (output / "bin").mkdir()
        (output / "lib").mkdir()
        shutil.copyfile(binary, output / "bin/ld.lld")
        (output / "bin/ld.lld").chmod(0o755)
        shutil.copy2(llvm / "bin/llvm-ar", output / "bin/llvm-ar")
        # glibc --list uses the real loader's dependency resolution without
        # entering either executable. Retain both tools' entire resolved DSO
        # closure, including the loader, in their private library directory.
        libraries = {}
        for executable in (binary, llvm / "bin/llvm-ar"):
            listing = run([target["interpreter"], "--list", executable], env,
                          text=True, capture_output=True).stdout
            for line in listing.splitlines():
                if "linux-vdso.so" in line:
                    continue
                match = re.fullmatch(r"\s*(?:\S+\s+=>\s+)?(/.+)\s+\(0x[0-9a-fA-F]+\)\s*", line)
                if not match:
                    raise ValueError("unrecognized glibc dependency entry: " + line)
                library = Path(match.group(1).rstrip())
                previous = libraries.setdefault(library.name, library)
                if previous.resolve() != library.resolve():
                    raise ValueError("conflicting tool loader names: " + library.name)
        for name, library in sorted(libraries.items()):
            shutil.copy2(library, output / "lib" / name)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("source", "llvm", "kit", "patch", "patch-tool", "main", "output"):
        parser.add_argument("--" + name, required=True, type=Path)
    parser.add_argument("--version", required=True)
    parser.add_argument("--headers", action="append", type=Path, default=[])
    build(parser.parse_args())

#!/usr/bin/env python3
"""Build the link kit: every input of a shipped link except the program and runtime.

For each target the kit holds the glibc 2.28 ABI floor (link-only stub DSOs
and glibc 2.28's own Scrt1.o, crti.o, crtn.o and libc_nonshared.a), LLVM's crtbegin/crtend and a static LLVM
libunwind, both compiled here from the pinned LLVM sources by the pinned clang.
`manifest.json` names every file with its SHA-256, its source and the exact
command that made it; `digest` is the manifest's SHA-256, the kit's identity.

The stubs follow Zig's technique (src/libs/glibc.zig): assembly placeholders
named by `.symver`, linked as a DSO with the library's SONAME. Their ABI source
is the pinned glibc 2.28 release itself: each public versioned definition of
its library with its kind, size, binding and default version, so a symbol that
became compat-only (`matherr` in 2.27) cannot bind a new link. Zig's pinned
`abilists` at 2.28 must name exactly the same versioned symbols, and the linked
stub must define exactly them.
"""
from pathlib import Path
import argparse
import hashlib
import json
import os
import re
import shutil
import subprocess
import tarfile
import tempfile

FLOOR = (2, 28, 0)

# Zig's library order (src/libs/glibc.zig `libs`): the index an abilists
# inclusion names. Pre-2.34 floors also have pthread, dl, rt and util.
LIBRARIES = [("m", 6), ("c", 6), ("ld", 2), ("resolv", 2), ("pthread", 0), ("dl", 2), ("rt", 1), ("util", 1)]
# The order a link names the stubs, which is the dynamic loader's search order:
# libc first, so a symbol libc and an add-on both define (read, write, close,
# clock_gettime, ...) binds to libc as on glibc 2.34 and later, where they are one
# library: an add-on is NEEDED only for what libc lacks at the floor (pthread_create,
# dladdr), librt not at all, and each load-time lookup ends at libc instead of
# walking the add-ons first (print-0 start: 48K fewer loader instructions). The
# add-ons' copies are the same cancellation-point wrappers, and Hirsel never cancels
# a thread. The loader also exports `malloc`, `realloc` and `free` (its minimal
# startup allocator), so it stays last.
LINK_ORDER = ["c", "m", "pthread", "dl", "rt", "util", "resolv", "ld"]

TARGETS = {
    "x86_64-unknown-linux-gnu": {
        "clang": "x86_64-unknown-linux-gnu",
        "zig": "x86_64-linux-gnu",
        "deb": "amd64",
        "multiarch": "x86_64-linux-gnu",
        "loader": "ld-linux-x86-64.so.2",
        "interpreter": "/lib64/ld-linux-x86-64.so.2",
        "options": [],
    },
    "aarch64-unknown-linux-gnu": {
        "clang": "aarch64-unknown-linux-gnu",
        "zig": "aarch64-linux-gnu",
        "deb": "arm64",
        "multiarch": "aarch64-linux-gnu",
        "loader": "ld-linux-aarch64.so.1",
        "interpreter": "/lib/ld-linux-aarch64.so.1",
        # GNU toolchains for aarch64-linux enable this erratum workaround by default.
        "options": ["--fix-cortex-a53-843419"],
    },
}

# Symbols abilists lists that the floor release no longer exports at all:
# abilists records when a symbol appeared, not when it went away. glibc 2.26
# stopped exporting `__libc_memalign` from the loader.
REMOVED = {("ld", "__libc_memalign")}

CRT_FLAGS = ["-std=c11", "-O2", "-fPIC", "-DCRT_HAS_INITFINI_ARRAY", "-Wno-pedantic"]
# LLVM's own static libunwind configuration (libunwind/CMakeLists.txt defaults
# for a native, threaded, static, symbol-hiding build), without debug info.
UNWIND_FLAGS = [
    "-O2", "-fPIC", "-DNDEBUG", "-D_LIBUNWIND_IS_NATIVE_ONLY", "-D_LIBUNWIND_HIDE_SYMBOLS",
    "-fvisibility=hidden", "-funwind-tables", "-Werror=return-type",
]
UNWIND_C_FLAGS = ["-std=c99", "-fexceptions"]
UNWIND_CXX_FLAGS = ["-std=c++17", "-nostdinc++", "-fno-exceptions", "-fno-rtti", "-fstrict-aliasing"]
UNWIND_SOURCES = ["libunwind.cpp", "UnwindLevel1.c", "UnwindLevel1-gcc-ext.c", "UnwindRegistersRestore.S", "UnwindRegistersSave.S"]


def sha256(path):
    digest = hashlib.sha256()
    with open(path, "rb") as file:
        for chunk in iter(lambda: file.read(1 << 20), b""):
            digest.update(chunk)
    return digest.hexdigest()


def tool_env(**values):
    """A pinned environment that keeps the LLVM tree's library path: RUNPATH
    finds libLLVM, but not its own dependencies (libedit's libbsd), which the
    tree ships and a pool worker's image does not."""
    return {"LD_LIBRARY_PATH": os.environ.get("LD_LIBRARY_PATH", ""), "LC_ALL": "C", **values}


def run(command, **kwargs):
    subprocess.run([str(c) for c in command], check=True, env=tool_env(PATH="/usr/bin:/bin", SOURCE_DATE_EPOCH="0"), **kwargs)


class Abilists:
    """Zig's `lib/libc/glibc/abilists` (format: src/libs/glibc.zig `loadMetaData`)."""

    def __init__(self, data):
        self.data = data
        self.at = 0
        names = [self.string() for _ in range(self.byte())]
        if names != [name for name, _ in LIBRARIES]:
            raise ValueError(f"abilists libraries {names}")
        self.versions = [tuple(self.take(3)) for _ in range(self.byte())]
        self.targets = [self.string() for _ in range(self.byte())]
        self.inclusions = self.at

    def byte(self):
        self.at += 1
        return self.data[self.at - 1]

    def take(self, n):
        self.at += n
        return self.data[self.at - n:self.at]

    def string(self):
        end = self.data.index(0, self.at)
        value = self.data[self.at:end].decode()
        self.at = end + 1
        return value

    def leb128(self):
        value = shift = 0
        while True:
            byte = self.byte()
            value |= (byte & 0x7F) << shift
            shift += 7
            if not byte & 0x80:
                return value

    def symbols(self, target, floor):
        """{library index: {name: (kind, {version: size}, default version)}} at `floor`."""
        bit = 1 << self.targets.index(target)
        top = self.versions.index(floor)
        self.at = self.inclusions
        result = {i: {} for i in range(len(LIBRARIES))}
        for kind in ("function", "object"):
            count = int.from_bytes(self.take(2), "little")
            name, chosen = None, {}
            for _ in range(count):
                if name is None:
                    name, chosen = self.string(), {}
                targets = self.leb128()
                size = self.leb128() if kind == "object" else None
                library = self.byte()
                terminal = library & 0x80
                library &= 0x7F
                while True:
                    byte = self.byte()
                    version = byte & 0x7F
                    if targets & bit and version <= top:
                        chosen.setdefault(library, {})[version] = size
                    if byte & 0x80:
                        break
                if terminal:
                    for index, versions in chosen.items():
                        result[index][name] = (kind, versions, max(versions))
                    name = None
        return result

    def version_name(self, index):
        major, minor, patch = self.versions[index]
        return f"GLIBC_{major}.{minor}" + (f".{patch}" if patch else "")


def dynamic_symbols(readelf, library):
    """{(name, version): (kind, size, binding, default)} of a library's public
    versioned definitions (GLIBC_PRIVATE and version names excluded)."""
    printed = subprocess.check_output([str(readelf), "--dyn-syms", "--wide", str(library)], text=True, env=tool_env())
    found = {}
    for line in printed.splitlines():
        fields = line.split()
        if len(fields) < 8 or not fields[0][:-1].isdigit() or fields[6] == "UND":
            continue
        kind, size, binding, symbol = fields[3], int(fields[2], 0), fields[4], fields[7]
        match = re.fullmatch(r"([^@]+)(@@?)(GLIBC_[0-9.]+)", symbol)
        if not match or match[1] == match[3]:
            continue
        name, at, version = match.groups()
        kind = {"FUNC": "function", "IFUNC": "function", "OBJECT": "object", "TLS": "tls"}[kind]
        found[(name, version)] = (kind, size if kind != "function" else 0, binding, at == "@@")
    return found


def stub_assembly(symbols, library):
    """Zig's stub technique (src/libs/glibc.zig `buildSharedObjects`): one
    placeholder definition per versioned symbol, named by `.symver`, with the
    real library's kind, size, binding and default version."""
    sections = {"function": [".text"], "object": [".data"], "tls": [".section .tbss,\"awT\",@nobits"]}
    for (name, version), (kind, size, binding, default) in sorted(symbols.items()):
        label = f"{name}_{version}"
        lines = sections[kind]
        lines += [".balign 8", (".weak " if binding == "WEAK" else ".globl ") + label]
        if kind == "function":
            lines += [f".type {label}, %function"]
        else:
            lines += [f".type {label}, %{'tls_object' if kind == 'tls' else 'object'}", f".size {label}, {size}"]
        lines += [f".symver {label}, {name}{'@@' if default else '@'}{version}, remove"]
        lines += [f"{label}: .quad 0" if kind == "function" else f"{label}: .zero {max(size, 1)}"]
    if library == "c":
        # As Zig: a strong reference keeps `_IO_stdin_used` in the dynamic
        # symbol table, so glibc does not assume a pre-2.1 FILE layout.
        sections["object"] += [".balign 8", ".globl _IO_stdin_used", ".quad _IO_stdin_used"]
    return "\n".join(sections["function"] + sections["object"] + sections["tls"]) + "\n"


def cross_check(abilists, zig, index, ours, name):
    """Zig's abilists at the floor names the same versioned symbols. Defaults may
    differ: abilists cannot say a symbol became compat-only (e.g. `matherr`)."""
    theirs = {(symbol, abilists.version_name(v)) for symbol, (_, versions, _) in zig[index].items() for v in versions
              if (LIBRARIES[index][0], symbol) not in REMOVED}
    problems = [f"{name}: {s}@{v} only in abilists" for s, v in sorted(theirs - set(ours))]
    problems += [f"{name}: {s}@{v} only in glibc 2.28" for s, v in sorted(set(ours) - theirs)]
    if problems:
        raise SystemExit("\n".join(problems))


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--llvm", type=Path, required=True)
    parser.add_argument("--arm-sysroot", type=Path, required=True)
    parser.add_argument("--abilists", type=Path, required=True)
    parser.add_argument("--libunwind", type=Path, required=True)
    parser.add_argument("--compiler-rt", type=Path, required=True)
    parser.add_argument("--deb", action="append", default=[], help="<role>:<arch>=<deb>")
    parser.add_argument("--pins", type=Path, required=True, help="the lock naming each input's URL")
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    llvm = args.llvm.resolve()
    clang, ar, lld, readelf = (llvm / "bin" / tool for tool in ("clang", "llvm-ar", "ld.lld", "llvm-readelf"))
    os.environ["LD_LIBRARY_PATH"] = str(llvm / "lib")
    debs = {}
    for spec in args.deb:
        key, path = spec.split("=", 1)
        debs[key] = Path(path)
    output = args.output
    output.mkdir(parents=True)
    pins = json.loads(args.pins.read_text())
    manifest = {
        "schema": 1,
        "floor": "glibc " + ".".join(map(str, FLOOR[:2])),
        "tools": {
            "clang": subprocess.check_output([str(clang), "--version"], text=True).splitlines()[0],
            "ld.lld": subprocess.check_output([str(lld), "--version"], text=True).strip(),
            # The linker every kit line runs: its executable and the LLVM library it loads.
            "ld.lld.sha256": sha256(lld),
            "libLLVM.sha256": sha256(next(llvm.glob("lib/libLLVM.so.*"))),
        },
        "sources": pins,
        "targets": {},
    }
    abilists = Abilists(args.abilists.read_bytes())
    with tempfile.TemporaryDirectory(prefix="hirsel-link-kit-") as scratch:
        scratch = Path(scratch)
        with tarfile.open(args.libunwind) as archive:
            members = [m for m in archive.getmembers() if "/libunwind/" in m.name]
            archive.extractall(scratch, members=members, filter="data")
        with tarfile.open(args.compiler_rt) as archive:
            members = [m for m in archive.getmembers() if "/compiler-rt/lib/builtins/" in m.name]
            archive.extractall(scratch, members=members, filter="data")
        source = next(scratch.glob("llvm-project-*.src"))
        unwind = source / "libunwind"
        builtins = source / "compiler-rt/lib/builtins"
        for triple, target in TARGETS.items():
            root = scratch / ("root-" + target["deb"])
            for role in ("glibc-dev", "glibc", "linux-headers"):
                run(["dpkg-deb", "-x", debs[f"{role}:{target['deb']}"], root])
            files = {}
            directory = output / triple
            (directory / "lib").mkdir(parents=True)

            def record(name, **facts):
                files[name] = {"sha256": sha256(directory / name), **facts}

            # glibc 2.28's own startup objects and nonshared archive, unchanged.
            development = root / "usr/lib" / target["multiarch"]
            # Static libc needs built-in files/DNS NSS modules: glibc 2.36
            # nss/nss_module.c:174-180. The 2.28 static resolver instead loads
            # host modules with incompatible private libc symbols. Arm's pinned
            # sysroot already supplies these and its static CRT. Keep the
            # dynamic ABI floor at 2.28 on both targets.
            static_root = scratch / ("static-" + target["deb"])
            if target["deb"] == "amd64":
                run(["dpkg-deb", "-x", debs["glibc-static:amd64"], static_root])
            static_development = (args.arm_sysroot / "usr/lib64" if target["deb"] == "arm64" else
                                  static_root / "usr/lib" / target["multiarch"])
            static_source = ("Arm GNU Toolchain 14.3.rel1 sysroot: usr/lib64" if target["deb"] == "arm64" else
                             f"libc6-dev 2.36-9+deb12u14 {target['deb']}: usr/lib/{target['multiarch']}")
            for name in ("Scrt1.o", "crti.o", "crtn.o", "libc_nonshared.a"):
                shutil.copyfile(development / name, directory / name)
                record(name, source=f"libc6-dev {target['deb']}: usr/lib/{target['multiarch']}/{name}")
            # The release's fixed-address static startup and archives. Keep these apart
            # from the link-only DSOs used by hosts, images and shared foreign
            # calls. In particular libm.a may be an absolute-path linker script;
            # select the actual archives it names, never the invoking sysroot.
            shutil.copyfile(static_development / "crt1.o", directory / "crt1.o")
            record("crt1.o", source=f"{static_source}/crt1.o")
            static_libraries = []
            # Static glibc before 2.34 needs pthread's private definitions to
            # precede libc's archive fallbacks, as on GCC's -lpthread -lc line.
            for library in [library for library in LINK_ORDER if library != "c"] + ["c"]:
                if library == "ld":
                    continue
                names = ([f"lib{library}.a"] if library != "m" else
                         [path.name for pattern in ("libm.a", "libm-*.a", "libmvec.a")
                          for path in sorted(static_development.glob(pattern))])
                for name in names:
                    original = static_development / name
                    if not original.read_bytes().startswith(b"!<arch>\n"):
                        if library == "m" and name == "libm.a":
                            continue
                        raise SystemExit(f"{triple} {name}: static library is not an archive")
                    (directory / "static").mkdir(exist_ok=True)
                    shutil.copyfile(original, directory / "static" / name)
                    static_libraries.append("static/" + name)
                    record("static/" + name, source=f"{static_source}/{name}")
            shutil.copyfile(static_development / "libc_nonshared.a", directory / "static/libc_nonshared.a")
            static_libraries.append("static/libc_nonshared.a")
            record("static/libc_nonshared.a", source=f"{static_source}/libc_nonshared.a")
            # Link-only stubs at the floor: the release's own dynamic symbol
            # tables, cross-checked against Zig's abilists.
            zig = abilists.symbols(target["zig"], FLOOR)
            versions = "".join(f"{abilists.version_name(i)} {{ }};\n" for i in range(abilists.versions.index(FLOOR) + 1))
            (scratch / "all.map").write_text(versions)
            for index, (library, sover) in enumerate(LIBRARIES):
                soname = target["loader"] if library == "ld" else f"lib{library}.so.{sover}"
                real = root / "lib" / target["multiarch"] / soname
                symbols = dynamic_symbols(readelf, real)
                cross_check(abilists, zig, index, symbols, f"{triple} {soname}")
                assembly = scratch / f"{triple}-{library}.s"
                assembly.write_text(stub_assembly(symbols, library))
                obj = assembly.with_suffix(".o")
                assemble = [clang, f"--target={target['clang']}", "-c", assembly, "-o", obj]
                run(assemble)
                stub = directory / "lib" / soname
                link = [lld, "-shared", f"-soname={soname}", f"--version-script={scratch / 'all.map'}", "--build-id=none", "-z", "noexecstack", "--hash-style=gnu", obj, "-o", stub]
                run(link)
                if dynamic_symbols(readelf, stub) != symbols:
                    raise SystemExit(f"{triple} {soname}: the stub does not define exactly the release's symbols")
                record(f"lib/{soname}", source=f"libc6 {target['deb']}: lib/{target['multiarch']}/{soname}, public versions",
                       symbols=len(symbols), checked="abilists at GLIBC_2.28")
            # LLVM crtbegin/crtend: __dso_handle and the init/fini array hooks,
            # against the floor's headers, never the invoking host's.
            floor_headers = ["-nostdlibinc", "-isystem", root / "usr/include" / target["multiarch"], "-isystem", root / "usr/include"]
            for name, source in (("crtbeginS.o", "crtbegin.c"), ("crtendS.o", "crtend.c")):
                command = [clang, f"--target={target['clang']}", *CRT_FLAGS, *floor_headers, "-c", builtins / source, "-o", directory / name]
                run(command)
                record(name, source=f"compiler-rt lib/builtins/{source}", flags=CRT_FLAGS)
            # Static LLVM libunwind, compiled against the floor's headers.
            headers = [*floor_headers, "-I", unwind / "include", f"-ffile-prefix-map={unwind}=libunwind"]
            objects = []
            for source in UNWIND_SOURCES:
                language = UNWIND_CXX_FLAGS if source.endswith(".cpp") else UNWIND_C_FLAGS if source.endswith(".c") else []
                obj = scratch / f"{triple}-{source}.o"
                run([clang, f"--target={target['clang']}", *UNWIND_FLAGS, *language, *headers, "-c", unwind / "src" / source, "-o", obj])
                objects.append(obj)
            run([ar, "rcsD", directory / "libunwind.a", *objects])
            record("libunwind.a", source="libunwind " + " ".join(UNWIND_SOURCES), flags=UNWIND_FLAGS,
                   c_flags=UNWIND_C_FLAGS, cxx_flags=UNWIND_CXX_FLAGS, headers="libc6-dev and linux-libc-dev at the floor")
            # C cleanup frames in static glibc use this personality. The Rust
            # runtime supplies its own language personality and builtins; never
            # resolve this remaining symbol from the host's libgcc_eh.
            personality = scratch / f"{triple}-gcc_personality_v0.o"
            personality_flags = ["-std=c11", "-O2", "-fPIC", "-fvisibility=hidden"]
            run([clang, f"--target={target['clang']}", *personality_flags, *floor_headers,
                 "-I", unwind / "include", "-c", builtins / "gcc_personality_v0.c", "-o", personality])
            run([ar, "rcsD", directory / "libcompiler_rt_personality.a", personality])
            record("libcompiler_rt_personality.a", source="compiler-rt lib/builtins/gcc_personality_v0.c",
                   flags=personality_flags, headers="libc6-dev, linux-libc-dev and LLVM libunwind")
            sonames = {library: target["loader"] if library == "ld" else f"lib{library}.so.{sover}" for library, sover in LIBRARIES}
            manifest["targets"][triple] = {
                # What a link names, in order around the program, runtime and foreign inputs.
                "interpreter": target["interpreter"],
                "options": target["options"],
                "start": ["Scrt1.o", "crti.o", "crtbeginS.o"],
                "unwind": "libunwind.a",
                "libraries": [f"lib/{sonames[library]}" for library in LINK_ORDER],
                "nonshared": "libc_nonshared.a",
                "end": ["crtendS.o", "crtn.o"],
                "static": {
                    "start": ["crt1.o", "crti.o", "crtbeginS.o"],
                    "libraries": static_libraries,
                    "personality": "libcompiler_rt_personality.a",
                },
                "files": files,
            }
    text = json.dumps(manifest, indent=2, sort_keys=True) + "\n"
    (output / "manifest.json").write_text(text)
    (output / "digest").write_text(hashlib.sha256(text.encode()).hexdigest() + "\n")


if __name__ == "__main__":
    main()

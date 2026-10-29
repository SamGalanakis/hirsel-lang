#!/usr/bin/env python3
"""Generate lld startup orderfiles from the runtime package's serial/pool traces.

Valgrind Lackey's trace-mem records instruction fetches and data accesses
(lackey/lk_main.c:651-801). First use supplies the rank. lld
ELF/Writer.cpp:1085-1155 consumes both function and data symbols through
--symbol-ordering-file. No recorded profile is checked in.
"""
import argparse
import bisect
import json
import os
from pathlib import Path
import re
import subprocess
import tempfile


def run(command, environment, directory, stderr=None):
    result = subprocess.run(
        list(map(str, command)), env=environment, cwd=directory,
        stdout=subprocess.PIPE, stderr=stderr or subprocess.PIPE,
    )
    if result.returncode:
        if stderr:
            stderr.flush()
            detail = Path(stderr.name).read_text()[-8000:]
        else:
            detail = result.stderr.decode(errors="replace")
        raise RuntimeError(f"{command[0]} exited {result.returncode}: {detail}")
    return result.stdout


def symbols(llvm, binary, environment, directory, *options):
    text = run(
        [llvm / "bin/llvm-nm", *options, "--format=posix", binary],
        environment, directory,
    ).decode()
    return [fields for line in text.splitlines() if len(fields := line.split()) == 4]


def demangle(llvm, names, environment):
    return subprocess.check_output(
        [llvm / "bin/llvm-cxxfilt"], input=("\n".join(names) + "\n").encode(),
        env=environment,
    ).decode().splitlines()


def trace_symbols(trace, rows):
    ranges = {}
    for name, kind, address, size in rows:
        if kind not in "tTwWrRdD" or not int(size, 16):
            continue
        address, size = int(address, 16), int(size, 16)
        entry = ranges.setdefault(address, [address + size, []])
        entry[0] = max(entry[0], address + size)
        entry[1].append(name)
    addresses = sorted(ranges)
    for line in trace.open():
        event = re.match(r" ?[ILSM] +([0-9a-fA-F]+),", line)
        if not event:
            continue
        address = int(event[1], 16)
        index = bisect.bisect_right(addresses, address) - 1
        if index >= 0:
            end, names = ranges[addresses[index]]
            if address < end:
                yield from sorted(names)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    tools = ["hirsel", "products", "llvm", "kit", "linker", "glibc-floor",
             "runtime-auditor", "valgrind", "capped", "private-native-inputs",
             "archives", "source", "output"]
    for name in tools:
        parser.add_argument("--" + name, required=True, type=lambda p: Path(p).resolve())
    args = parser.parse_args()
    environment = dict(
        os.environ, LD_LIBRARY_PATH=str(args.llvm / "lib"),
        LLVM_SYS_231_PREFIX=str(args.llvm), HIRSEL_LINK_KIT=str(args.kit),
        HIRSEL_GLIBC_FLOOR=str(args.glibc_floor), HIRSEL_CAPTURE_LINKER=str(args.linker),
        HIRSEL_AUDITOR_PACKAGE=str(args.runtime_auditor),
        HIRSEL_RUNTIME_ARCHIVES=str(args.archives), HIRSEL_PRODUCTS=str(args.products),
        HIRSEL_JOBS="4", HIRSEL_MEMORY_LIMIT="2G", HIRSEL_DAEMON="off",
        HIRSEL_RESULTS="none@1", VALGRIND_LIB=str(args.valgrind / "libexec/valgrind"),
        HIRSEL_RT_WORKERS="1",
    )
    selected = {}
    with tempfile.TemporaryDirectory(prefix="hirsel-startup-order-") as directory:
        directory = Path(directory)
        # Match native tests: LLVM's bin/lib files get private inodes before
        # strict selected-input hashing, independent of shared CAS hard links.
        args.llvm = Path(run(
            ["python3", args.private_native_inputs, args.llvm, directory / "llvm"],
            environment, directory,
        ).decode().strip())
        environment["LLVM_SYS_231_PREFIX"] = str(args.llvm)
        environment["LD_LIBRARY_PATH"] = str(args.llvm / "lib")
        environment["XDG_CACHE_HOME"] = str(directory / "cache")
        for shape, width in [("serial", 1), ("pool", 4)]:
            binary = directory / shape
            run([args.hirsel, "build", "--release", args.source / (shape + ".hir"),
                 "-o", binary], environment, directory)
            rows = symbols(args.llvm, binary, environment, directory,
                           "--numeric-sort", "--print-size")
            grants = {
                "io": {"stdout": "stdout.bytes@1", "tasks": "tasks.cooperative@1", "dirs": []},
                "placement": {"cores": 1, "commit": "shared", "reorder": "auto"},
            }
            if shape == "pool":
                grants["io"]["cpu"] = {
                    "provider": "cpu.scoped@1", "max_workers": 4, "max_domains": 1,
                    "max_physical_bytes": 268435456, "max_batch_bytes": 8388608,
                }
            trace = directory / (shape + ".trace")
            with trace.open("wb") as log:
                output = run(
                    [args.capped, "--timeout", "60", args.valgrind / "bin/valgrind",
                     "--tool=lackey", "--trace-mem=yes", "--basic-counts=no",
                     "--detailed-counts=no", binary, "--hirsel-grants", json.dumps(grants),
                     shape, str(width), "21"], environment, directory, stderr=log,
                )
            expected = b"42\n" if shape == "serial" else b"180\n"
            if output != expected:
                raise RuntimeError(f"{shape} startup fixture returned {output!r}")
            for name in trace_symbols(trace, rows):
                selected.setdefault(name, len(selected))

        # Resolve host Rust names in every target/feature archive. CRT and
        # anonymous constant names also keep their exact matches. A link can
        # omit any traced function; ordering never creates a GC root.
        human = demangle(args.llvm, selected, environment)
        ranks = {name: rank for rank, name in enumerate(human)}
        for archive in sorted(args.archives.glob("*/*/libhirsel_rt.a")):
            names = [row[0] for row in symbols(args.llvm, archive, environment, directory,
                                              "--defined-only")]
            human = demangle(args.llvm, names, environment)
            translated = {}
            for name, readable in zip(names, human):
                if readable in ranks:
                    translated[name] = ranks[readable]
            for name, rank in selected.items():
                translated.setdefault(name, rank)
            destination = args.output / archive.relative_to(args.archives).parent / "startup.order"
            destination.parent.mkdir(parents=True, exist_ok=True)
            destination.write_text("\n".join(
                name for name, rank in sorted(translated.items(), key=lambda row: (row[1], row[0]))
            ) + "\n")


if __name__ == "__main__":
    main()

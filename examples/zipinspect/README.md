# ZIP inspector and binary pipe

List central-directory entries from stdin, or pass one exact member name to write
its raw bytes to stdout. Input and decoded member data are each limited to 1 MiB,
with at most 1,024 entries. Listing does not decode members; extraction checks CRC.
Names are compared as data and are never used as filesystem paths.

```sh
hirsel test examples/zipinspect/hirsel.json
hirsel run examples/zipinspect/main.hir --grants examples/zipinspect/grants.json < archive.zip
hirsel run examples/zipinspect/main.hir --grants examples/zipinspect/grants.json -- raw.bin < archive.zip > raw.bin
hirsel run examples/zipinspect/main.hir --native --backend llvm --grants examples/zipinspect/grants.json < archive.zip
hirsel run examples/zipinspect/main.hir --native --backend cranelift --grants examples/zipinspect/grants.json < archive.zip
python3 examples/verify-dogfood6.py --hirsel "$(command -v hirsel)" --tool zipinspect
```

`main.hir` intentionally has no sidecar: it selects the toolchain's std. The package
manifest provides its three tests. The verifier creates a deterministic ZIP with
Python's `zipfile`, checks binary output including NUL and `0xff`, and compares
successful/malformed-input record and replay in all three engines.

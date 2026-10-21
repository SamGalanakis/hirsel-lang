# Distribution compiler experiments

The `//tools/buck2:dist` platform and Cargo `--profile dist` use fat LTO and
one codegen unit. The CLI already uses mimalloc v3 (`mimalloc 0.1.52`,
`libmimalloc-sys 0.1.49`, Buck2's v3/static.c fixup). Developer dev/release,
test tools, build scripts, proc macros and native runtime profiles stay unchanged.
No target CPU, panic policy or hot/cold linker grouping is changed.

```sh
# Plain fat LTO candidate; build actions run on the pool.
tools/toolchain-dist/build --output /tmp/hirsel-dist
# PGO: instrument, train 24 workloads, merge, rebuild. Use fresh output/work dirs.
tools/toolchain-dist/build --pgo --output /tmp/hirsel-pgo --work /tmp/hirsel-pgo-work
# Smoke matrix, one run per cell; instructions and peak RSS in JSON.
tools/compiler-bench/run --quick --repeat 1 --toolchain /tmp/hirsel-pgo --output /tmp/quick.json
# Three seeded rounds for instructions/RSS; use --rounds 10 for wall-time ratios.
tools/toolchain-dist/measure --bundle release=/tmp/hirsel-release \
    --bundle dist=/tmp/hirsel-dist --bundle pgo=/tmp/hirsel-pgo --output /tmp/dist-results
tools/compiler-bench/compare /tmp/dist-results/release.json /tmp/dist-results/pgo.json
```

PGO instruments Rust code, including third-party Rust libraries. The C mimalloc
fixup stays unchanged. Both passes use the same dist platform, resource requests,
fat LTO, strip setting and crate metadata: Buck2's `_metadata` keys symbols by
target and configuration hash. `hirsel.dist_pgo=generate|use` changes action
flags and inputs, not that platform. `profile.profdata` is a declared, ignored
Buck2 source, so its content participates in remote action keys. A checkout lock
serializes profile writers. Do not edit the tree between PGO passes.

Training uses the compiler-bench corpus generator, seven examples plus syn3k,
each in check, debug Cranelift and release LLVM mode. Every workload must return
zero and produce a nonempty `.profraw`. Training and measurements each acquire
CPU29's timing lock (SMT sibling CPU13; override with `--cpu`); every individual compiler process goes through tools/capped.
The program cache is disabled. The exact matching LLVM 23.1.1 profile tools come
from a checksum-pinned Rust 1.99.0 llvm-tools archive; system LLVM versions can
be incompatible. `--profdata` can name an already installed matching tool.
Keep the profile and manifest with release artifacts; never commit generated profiles.

BOLT 23.1.1 was tried on the PGO compiler relinked with `--emit-relocs`.
Its instrumentation pass reached the 4 GiB tools/capped limit and was killed
(exit 137) before training. Per the host rule, the limit was not raised and the
experiment was declined. No BOLT shipping step or relocation flags are enabled.
The trial used `llvm-bolt hirsel -instrument -o hirsel.inst
--instrumentation-file=profiles/hirsel --instrumentation-file-append-pid`.

Retain only measured wins (at least 1% instructions on a case, without regressions;
compiler-bench's existing +1% instruction threshold is the regression bar).
No huge pages, CPU tuning above the baseline, alternate allocators or panic-abort
experiment is repeated.

Borrowed from tsrs `notes/perf-pgo.md:14-32,128-169` and
`.github/scripts/pgo-train.sh:1-58`: two builds, public corpus, nonempty profile
checks, matching symbol metadata. Differs: Buck2 artifacts and remote action
keys replace Cargo target directories; Hirsel has a single compiler binary and
a separate std precheck. The declined BOLT trial followed `.github/scripts/bolt.sh:36-75` and
`notes/perf-build-level.md:66-115` (instrumentation mode, ext-tsp/cdsort,
preserve debug sections). This host does not supply LBR profiles.

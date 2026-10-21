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

Release packaging writes `notices/manifest.json`, `notices/crates/`,
`notices/rust/` and `notices/adapted-source/` beside the compiler. Copy or archive
the **whole output directory**, including `notices/`; there is no user-directory
installation step. The bare Buck `hirsel-toolchain` target is a build artifact;
use `tools/toolchain-dist/build` for the redistributable output.

`notices.py` walks the compiler's configured Buck link edges and identifies
crate archives from source edges, then joins them to locked Cargo metadata.
It excludes build scripts and proc-macro executables, and inventories linked
inputs before dead-code elimination. It reads license-file inputs and all
upstream LICENSE/COPYING/COPYRIGHT/NOTICE files (including nested C and assembly
licenses), plus full text files with embedded copyright or SPDX notices,
from Cargo's cached archives after checking their Cargo.lock SHA-256.
Run Cargo metadata to populate an absent cache. Missing archives, declared
license files, required native texts or Rust notices fail packaging and remove
the incomplete output. No license text is synthesized from a package name or
SPDX identifier. All supplied alternatives are retained without electing an
alternative; a missing Cargo license expression is reported as a manifest gap.
`notice-sources.json` records the bundle boundary and the explicitly required
mimalloc v3 and ring/BoringSSL/fiat/once_cell texts. Changing the bundle contents
requires updating that boundary; unexpected top-level files are refused.

The manifest records the pinned archive provenance, license expressions,
upstream text paths and hashes, source/build-input hashes, and every bundled
file's hash. It has no timestamps, checkout paths or host identifiers. Notice
collection checks that existing file bytes did not change; compiler identity,
std image and native objects remain untouched. `hirsel-build.json` records the
notice manifest hash and the current `hirsel-std.image` hash (the older
`hirsel-std.precheck` filename is no longer shipped).

The standard bundle does not copy native runtime archives, the link kit, LLVM
shared libraries, clang or host system libraries. Their external status and
pin paths are explicit in the manifest. Native checks still select those
inputs through the existing environment. `tools/native-toolchain/install
--unpack DIR` retains the pinned Debian packages' `usr/share/doc/*/copyright`
through its full `dpkg -x` extraction. Extending the release to carry any of
those code objects requires extending the notice inventory too; this collector
does not claim to cover an added native toolchain tree.

Prior art: Rust release packaging at rust-lang/rust
`a7c37f9f0ee5e93396106e9de20f4a86b4154e63`,
`src/bootstrap/src/core/build_steps/dist.rs:678-697`, installs generated HTML
copyright inventories and the accompanying REUSE license directory. We retain
the pinned toolchain's `COPYRIGHT-library.html` and its complete `licenses/`
directory for the linked Rust libraries, instead of the compiler-wide inventory
for an unshipped rustc executable. Crate text selection follows the upstream
archive and its Cargo license-file declaration; ring 0.17.14 `LICENSE:1-9`
explicitly points to BoringSSL, ISC and once_cell texts. This differs from a
single chosen-license report: keeping all upstream texts preserves multiple
licenses and nested native notices without making a legal-policy choice.

Focused checks:

```sh
.buck2/bin/buck2 test //:gate-distribution-notices
python3 tools/tests/native_toolchain.py
```

Three published Wasmtime archives omit their license file: assembler-x64 0.136.3,
bitset 0.136.3 and internal-core 49.0.3. Explicit per-version fallbacks retain
root `LICENSE:1-219` from wasmtime `f96eabaa11a06086e4fb779ed1c1688481067ab9`,
using its identical copy in the locked cranelift-codegen archive. Both archives'
VCS revisions and the donor text hash must match. The manifest reports each
archive omission, donor checksum and upstream URL. This is a verified upstream
text, not a replacement chosen from its SPDX expression. License declarations
are in that revision's `cranelift/assembler-x64/Cargo.toml:5`,
`cranelift/bitset/Cargo.toml:6` and `crates/core/Cargo.toml:5`.

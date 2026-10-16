# Building Hirsel

Hirsel is a systems-level, general-purpose language whose primary user is a
coding agent. Its fixed goals:

- native performance;
- every definition checked only against the signatures it uses;
- one way per job;
- all authority passed as parameters (capabilities);
- reference counting with no lifetime variables;
- deterministic behaviour outside capabilities.

## Layout

- `docs/spec/` is the specification, starting at `docs/spec/index.md`. Rule IDs
  such as `OWN-6` are stable, each is defined once as a `### ID: Title`
  heading, and every diagnostic cites one. `docs/spec/syntax.md` is the grammar.
- `crates/` is the Cargo workspace: lexer, parser, checker, typed IR, the
  interpreter, the reference-counting pass, the LLVM backend, the runtime and
  the driver. Each crate's `README.md` lists its invariants.
- `conformance/` is the rule-ID conformance corpus: one `.hir` program per
  case, named after the rule it exercises.
- `native-tests/` holds native-only fixtures (heap, layout, replay, topology)
  and the native exclusion list.
- `stdlib/` and `packages/` are Hirsel library sources and package fixtures.
- `proofs/rc/` is the Lean model of the reference-counting protocol.
- `bench/native/` is the native benchmark harness and its programs.
- `tools/` holds the Buck2 build (`tools/buck2/`), the pinned LLVM toolchain
  (`tools/native-toolchain/`) and gate scripts.

## Toolchains

Rust is pinned in `rust-toolchain.toml` (1.99.0) and installed by rustup.
The native backend needs LLVM 21.1.8 (`tools/native-toolchain/llvm-21.lock`);
`.cargo/config.toml` points `LLVM_SYS_211_PREFIX` at `/usr/lib/llvm-21`
unless the environment already sets it. `tools/native-toolchain/README.md`
installs the pinned tree. The full gate also needs qemu-user, the AArch64
cross toolchain, Valgrind and Lean on the host.

The build caps itself for a shared host: `.cargo/config.toml` limits Cargo
to 4 jobs and sets `RUST_TEST_THREADS` and `HIRSEL_JOBS` (in-process worker
threads) to 4. Export them explicitly when a wrapper might override them:

```sh
export CARGO_BUILD_JOBS=4 HIRSEL_JOBS=4 RUST_TEST_THREADS=4
```

## Cargo

Cargo needs no build service:

```sh
cargo build --workspace
cargo test -p hirsel-lir
cargo run -p hirsel-conformance -- --verbose
```

`hirsel-conformance` reports counts by rule prefix; `--json` gives tool
output. It finds the repository from the current directory; `--root <repo>`
selects another checkout. `--update-passing` (run on this host by `tools/bless.sh passing`) adds newly
passing tests to `conformance/passing.txt` without removing existing entries.

## Buck2

The gate builds and tests with Buck2. Cargo manifests and `Cargo.lock` stay
the source of truth: after editing a `Cargo.toml`, run
`python3 tools/buck2/sync.py` to regenerate the BUCK files (`tools/check.sh`
fails while they are stale). The toolchains (Rust, the pinned LLVM, Buck2 and
its prelude) come in hermetically; nothing is installed system-wide.

```sh
python3 tools/buck2/prepare.py                  # once per checkout
.buck2/bin/buck2 build //:
.buck2/bin/buck2 test //:hirsel-lir-unittest //:hirsel-lir-test-it \
  //:hirsel-lir-test-it-host //:hirsel-lir
.buck2/bin/buck2 test //:hirsel-lir-test-it -- --test-arg=layout::recursive_layouts_do_not_depend_on_request_order --test-arg=--exact
tools/check.sh
```

Without further configuration every action runs on this host. Buck2 can
also use an optional remote-execution pool; its endpoint, credentials and
platform properties live only in the gitignored `.buckconfig.local`
(`tools/buck2/README.md`). Use `-c hirsel.tests=host` for tests that need
qemu, the cross linker or Valgrind; compilation still goes wherever the other
actions go. More target names and the filtered native commands are in
[tools/buck2/README.md](../tools/buck2/README.md).

## Gates

`tools/check.sh` is the per-push gate: it builds every workspace target, runs
Clippy, checks formatting, tests changed crates and their reverse
dependencies, checks the canonical conformance corpus, samples source/IR
checker agreement with seed 1, and runs native smoke. Changes to native code
also run the filtered native matrix for the programs they add or touch:

```sh
.buck2/bin/buck2 test -c hirsel.full=true -c hirsel.tests=host //:gate-native-full -- --test-arg=--filter --test-arg=<program> --timeout 14400
```

Tests that execute the full native matrix or take about a minute or longer
carry `#[ignore = "full gate"]` and a `full_gate_` name prefix. Keep their
inputs and assertions intact; add a separate bounded smoke test when useful.

`tools/check.sh --full` is the release and scheduled gate: unrestricted
source/IR and native agreement, release tests, the `full_gate_` tests in both
debug and release, the full native matrix and every `tools/check-native.sh`
runtime, sanitizer, toolchain and Lean check. The full gate and
`tools/check-native.sh` run every stage even after one fails and fail at the end;
the per-push gate stops at its first failing stage. `tools/main-full-run.sh`
runs the full gate on a fresh detached worktree of `origin/main`.

The per-push gate selects tests relative to the merge-base with
`origin/main`: every Buck2 test with a changed input, directly or through
any dependency (`tools/buck2/changed_tests.py`).

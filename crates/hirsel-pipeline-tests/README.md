# hirsel-pipeline-tests

Tests that an implementation crate owns but that compile source through
`hirsel-driver`: the real checker, lowering, native export and import feed the
crate under test. They live here so that no implementation crate dev-depends on
the driver. Tests that run the interpreter (`hirsel-interp`) as their reference
live here too, so that no crate below the interpreter dev-depends on it. Each
implementation crate's own `tests/it` keeps the tests that build their input by
hand and need neither.

Allowed dependencies: dev-dependencies only, on the driver, the interpreter and
the crates whose public API the tests read. The library is empty and nothing depends on this
crate. The TLS native export test also uses rustls as its independent local server.
Only this crate, `hirsel-cli`, `hirsel-conformance`, `hirsel-native-build`, and
check and modes (whose internal unit-test harnesses read the core library
signatures from the driver's `BuiltinRegistry`) depend on `hirsel-driver`.

Layout:

- Two binaries, each a third to two thirds of the former single harness, so a
  check or modes edit relinks them in parallel:
  `tests/rc/main.rs` links hirsel-rc's suites and `tests/front/main.rs` those of
  check, modes, lir, lower, rt and code-store, and `analysis_canaries`, which exports
  `front/fixtures/analysis/` and runs hirsel-analysis-canary's analyses over the
  export files. A module named `<crate>_<file>` holds what was
  `crates/hirsel-<crate>/tests/it/<file>.rs`, so `rc::cleanup::x` became
  `rc_cleanup::x` (`.buck2/bin/buck2 test //:hirsel-pipeline-tests-test-rc-shard-1 --
  --test-arg=rc_cleanup::x --test-arg=--exact`). A crate prefix such as `modes_` selects one crate's tests.
- Each binary keeps its helpers local: `rc/support.rs` (compile a source program
  and adapt its validated native input into an rc `Program`), `rc/fixtures/`
  (golden LIR dumps, `tools/bless.sh rc-import` rewrites them), `rc/maps/seed.c`,
  `front/check_support.rs` and `front/rt_replay.rs` (independent replay of
  runtime completion tapes in the interpreter).
- Shared files of other crates are included by path: the driver's
  `tests/support/source_std.rs`, the runtime's `tests/support/mod.rs` (its
  integration-test helpers; the runtime's own test binary installs their
  counting allocator), the lowering fixture `crates/hirsel-lower/src/support.rs`
  and the LLVM emission fixtures `crates/hirsel-llvm/tests/support/mod.rs`.
- `full_gate_` tests keep their names and `#[ignore = "full gate"]`; the full
  gate runs them with the rest of the workspace.

The `check_*` modules retain their source-fixture names after the checker split:
signatures are normalized in hirsel-signature, bodies/audits checked in
hirsel-check, and TIR emitted in hirsel-lower. Their tests still run through the
driver.

`rt_net_native` records the compiled TCP program against a live loopback server.
After the server exits, the runtime tape helper independently executes its
scripted fixture in the interpreter, compares every completion and final state,
and strictly replays the interpreter tape. An altered reply byte must fail this
comparison while the original recording passes.

Type-store test fixtures use `hirsel-layout` for type-to-layout and ABI derivation.

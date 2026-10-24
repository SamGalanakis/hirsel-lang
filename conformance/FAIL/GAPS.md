# Failure coverage dispositions

[FAIL-1–4](../../docs/spec/failure.md#fail-1-kinds) and the
[core concurrency types](../../docs/spec/core-library.md#concurrency) define
`Failure`, `SourceSite` and `FailureKind`. Old section/issue numbers are historical.

- **Implemented finite depth/cleanup controls — LANG-10 / FAIL-3:**
  the [depth revision](../../docs/spec/index.md#deterministic-depth-revision) has
  four-engine controls in [logical_depth_31](../../crates/hirsel-conformance/tests/it/logical_depth_31.rs)
  for pre-entry Take rejection, independent 64-invocation cleanup, nested field
  lifetime, primary/ordered causes, cancellation and isolate recovery. Independent
  C/H adversaries check omitted events and malformed retirement. These finite
  controls do not close the universal allocation/scheduler oracles below.
  Bounded-arena OutOfSpace accounting stays independently normative under MEM-6.
- **Resolved fixture-support history:** `test.check`, test execution/selection,
  entry validation and capability-denial launch controls exist. Failure metadata
  has concrete source controls; they do not prove a universal data ABI.
- **Model control present — FAIL-4:** dead-mutex payload release and original-cause
  preservation are directly tested at the interpreter operation boundary. The existing source case
  `FAIL-4-owned-payload-dead-run.hir` also re-acquires after isolate: current
  CLO-3 / CONC-2 permit copied Mutex handles. The old visibility question is
  resolved for these handles; it does not permit arbitrary non-sendable captures.
- **Open oracle scope:** failure/cancellation cleanup for every frame and schedule
  still needs allocation/scheduler models. Unsafe-FFI failing-drop process
  termination already has a native runtime subprocess control.

- **Finite synchronous model:** `failure_cleanup_21` retains a division failure
  after take as primary while three failing finalizers extend its ordered cause
  chain; early returns and Result errors have normal-cleanup twins. Interpreter
  cause links, H reached-fault audits and full public four-engine observations
  are checked. Native injected faults remain unsupported by the public adapter;
  synchronous native failures are exercised.

See [UNTESTABLE.md](UNTESTABLE.md) and the
[evidence index](../coverage-ledger.md#destruction-and-failure). Pending corpus rows
are executed cases with exact failure reasons, not unspecified-contract tickets.

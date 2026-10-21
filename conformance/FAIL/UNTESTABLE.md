# Failure properties requiring another oracle

| Rule | Part | Oracle |
| --- | --- | --- |
| FAIL-1 | Environmental heap exhaustion, guard-page overflow and foreign crash end the process and fall outside determinism | Native fault-injection/process harness. Safe code cannot force environmental allocation failure deterministically. |
| FAIL-2 | Cancellation/join behavior for all sibling schedules; exact Failure cause/source/spawn-path data ABI | Failure/scheduler model with injected schedules and interpreter/native differential traces. Concrete scope/child propagation, first-spawn failure and isolate outcomes are tested. Data equality is exercised by successful/nested Result comparisons. |
| FAIL-3 | Every frame table releases all counts, resets all regions/arenas, and runs no other user code | Unwind/RC model and allocation instrumentation. Resource and arena-body failure traces, including fields after failed drop, are concrete programs. |
| FAIL-3 | Process termination on failing drop in `unsafe ffi` packages across all compiled cleanup paths | Native runtime subprocess controls in `hirsel-rt/tests/it/finalizers.rs` cover direct, nested-field and derived failing finalizers. End-to-end package metadata-to-descriptor coverage is separate; the ordinary safe-drop unwind contrast is a run test. |
| FAIL-4 | Dead payload release and original-cause preservation across every payload shape and schedule | Interpreter operation model `library::dead_mutex_releases_updated_payload_and_preserves_original_failure`, plus native runtime mutex controls. `FAIL-4-owned-payload-dead-run.hir` is a concrete legal source witness: CLO-3 permits copied Mutex handles. Finite witnesses do not prove every schedule. |

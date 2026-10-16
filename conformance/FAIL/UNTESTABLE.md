# Failure properties requiring another oracle

| Rule | Part | Oracle |
| --- | --- | --- |
| FAIL-1 | Environmental heap exhaustion, guard-page overflow and foreign crash end the process and fall outside determinism | Native fault-injection/process harness. Safe code cannot force environmental allocation failure deterministically. |
| FAIL-2 | Cancellation/join behavior for all sibling schedules; exact Failure cause/source/spawn-path data ABI | Failure/scheduler model with injected schedules and interpreter/native differential traces. Concrete scope/child propagation, first-spawn failure and isolate outcomes are tested. Data equality is exercised by successful/nested Result comparisons. |
| FAIL-3 | Every frame table releases all counts, resets all regions/arenas, and runs no other user code | Unwind/RC model and allocation instrumentation. Resource and arena-body failure traces, including fields after failed drop, are concrete programs. |
| FAIL-3 | Process termination on failing drop in `unsafe ffi` packages | Subprocess fixture with package metadata and a failing foreign-resource drop (#6). The ordinary safe-drop unwind contrast is a run test. |
| FAIL-4 | Dead payload release, subsequent acquiring-task failure and its original cause when safe code cannot retain/access the mutex across a failure boundary | Mutex/failure runtime model pending a legal visibility mechanism (#22). MODE-4/CLO-3 disallow the handle capture required to re-acquire after isolate; no invalid accepting program is substituted. |

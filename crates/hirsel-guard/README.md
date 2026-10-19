# hirsel-guard

The memory guard every hirsel binary and test harness installs, so no run can
exhaust the host however it is invoked.

Allowed dependencies: libc. Any crate may depend on it; it depends on no
workspace crate.

- `hirsel_guard::install!()` at a binary's or harness's root (`#[cfg(test)]` in a
  library's `lib.rs`) makes `Guarded` the global allocator; `install!(Counting)`
  wraps a test's own unit-struct allocator. `tools/buck2/sync.py` refuses a binary,
  example or test root that does not install it, directly or in a `#[path]` module.
- The first allocation, before `main`, sets the soft `RLIMIT_DATA` to
  `HIRSEL_MEMORY_LIMIT`: positive decimal bytes or a count of binary `K`, `M`, `G`
  or `T` (`4G`, `512M`), the syntax systemd's `MemoryMax` and `tools/capped` read;
  default 4 GiB. A malformed value exits 2 before any work. The limit never rises
  above one the process inherited, and child processes, native executables
  included, inherit it. The hard limit stays as inherited: `sanitized(command)`
  lifts the soft limit in that child alone, for an executable built with
  AddressSanitizer, which maps terabytes of shadow address space at startup that
  any data limit refuses. The run's cgroup or pool reservation still bounds it.
- An allocation the limit refuses ends the process at once: `hirsel: memory limit
  4G reached (HIRSEL_MEMORY_LIMIT); allocating N more bytes failed with a resident
  peak of RM. ...` on stderr, exit status 71 (`EXIT_STATUS`). The resident peak
  (`VmHWM`) tells growth apart from reservations the data limit also counts. The report is built on the stack and written with
  `write(2)`; nothing allocates after the failure. `spawn_failed(stack, error)` ends
  the same way when a thread's stack would not fit beside the process's `VmData`
  (pthread reports that as `EAGAIN`), and otherwise panics with the error.
- The data limit counts private writable mappings: heap, anonymous `mmap`, thread
  stacks and reservations such as `Vec::with_capacity`, but neither file mappings
  nor `PROT_NONE` ranges. An address-space limit would also count the shared
  libraries and the arenas glibc reserves without touching.
- `jobs()` is the worker count for a runner's threads or processes:
  `HIRSEL_JOBS`, else 2. A malformed or zero value exits 2.

Under Buck2 the test rule (`tools/buck2/defs.bzl`) sets `HIRSEL_MEMORY_LIMIT` to
the run's grant, twice the binary's measured `memory_kb` (the pool kills a run past
that), and the launcher applies it with `ulimit -d` before the harness starts.
Raising a cap means re-measuring the request (`tools/buck2/test_sizes_from_usage.py`),
never an ad hoc variable. Locally, `tools/capped` passes its cap through.

Tests spawn the harness itself with a small limit: an allocation and a thread stack
past it end with the message and status, the same work fits under a larger limit,
a malformed limit is refused, a shell child inherits the limit, a guarded child
cannot raise it, and only a `sanitized` child gets the hard limit back. The CLI (`hirsel-cli` `cli.rs`) and the conformance runner
(`hirsel-conformance` `runner.rs`) each show a run whose working set exceeds a
64M limit ending with the message.

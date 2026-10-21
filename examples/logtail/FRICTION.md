# First-contact friction

Minutes are approximate elapsed investigation time; overlapping waits are not a benchmark.
Copied diagnostics normalize checkout paths and strip trailing display whitespace.

| Tried | Exact diagnostic | Fix | Minutes lost | Class |
| --- | --- | --- | ---: | --- |
| Compiled natively while the shared host could not create another worker thread. | [Exact panic](repros/thread-limit.txt): `job worker creation failed: Os { code: 11, kind: WouldBlock, message: "Resource temporarily unavailable" }` | `HIRSEL_JOBS=1` avoids the observed worker pressure. Compiler worker creation now falls back to the started workers (or the caller when none started), with one operational warning naming `HIRSEL_JOBS=1`; successful compilation keeps exit 0. [Control program](repros/thread-creation.hir); the required trigger is `pthread_create` failing, so the default-worker crash is environment-dependent. Covered by injected first-worker and partial-start failures in `hirsel-intern` and CLI tests. | 0.5 | bug |

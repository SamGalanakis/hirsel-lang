# First-contact friction

Minutes are approximate elapsed investigation time; overlapping waits are not a benchmark.
Copied diagnostics normalize checkout paths and strip trailing display whitespace.

| Tried | Exact diagnostic | Fix | Minutes lost | Class |
| --- | --- | --- | ---: | --- |
| Compiled natively while the shared host could not create another worker thread. | [Exact panic](repros/thread-limit.txt): `job worker creation failed: Os { code: 11, kind: WouldBlock, message: "Resource temporarily unavailable" }` | `HIRSEL_JOBS=1` avoids the observed worker pressure. Compiler startup should report a tooling error instead of panic. [Control program](repros/thread-creation.hir); the required trigger is `pthread_create` failing, so the default-worker crash is environment-dependent. Routed to @agentlang-24. | 0.5 | bug |

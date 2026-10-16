# Ownership implementation oracles

| Rule | Part no single program can observe | Oracle |
| --- | --- | --- |
| OWN-1 | Borrow implementation cost and absence of unnecessary copies. | Inspect borrow/projection annotations and instrumented count traces; source read-only and retained-owner cases are covered. |
| OWN-4 | Sharing versus memcpy and the exact first detach allocation. | RC/flat representation model and allocation traces; value preservation is covered by run tests. |
| OWN-6 | No counted cross-task alias except Shared roots; atomic Shared counts versus plain task-local counts; no marking walk. | Machine-checked RC transfer model, weak-memory stress tests and instrumented traces over simulated schedules. |
| OWN-7 | Permitted in-place reuse when count is one; no counts/mutation inside Shared, regions or immutable arena blocks. | RC/arena/Shared model and allocation/address traces. Addresses and placement are unobservable; run tests check only values. V20 reconciles OWN-7 with the unique arena-root and unpublished-region builder exceptions; the nested-arena run case checks value preservation. |
| OWN-8 | Exact count increment (or elision) on projection; count before owner storage changes. | RC/projection model and compiler count-insertion snapshots; projection survival is covered by a run test. |

OWN-2, OWN-3 and OWN-5 have source-level rejection/control coverage. No claim is made that an M0 runner can execute these before a compiler exists. Rule attribution follows v20 section 23.

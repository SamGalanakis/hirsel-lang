# Brand and placement implementation oracles

| Rule | Part no single program can observe | Oracle |
| --- | --- | --- |
| PROV-1 | Identity and brand preservation across optimized copies and moves without observable addresses. | Typed-IR identity/brand snapshots and modes/identity soundness proof; source identity, invalidation, copied-capability and container cases are written. |
| PROV-2 | No lifetime-variable language, since v19 supplies no lifetime-variable syntax. | Grammar completeness/exclusion audit and parser negative fixtures using the v20 diagnostic policy. |
| PROV-3 | Brands erased at runtime remain sound under separately compiled library signatures. | Library-signature/typed-IR audit and differential topologies; all five listed families have static pairs. |
| PROV-4 | Actual blocks reside in the declared arena or longer-lived storage, with expected-type constructor placement. Placement is unobservable. | Instrumented allocation traces, arena/modes proof and typed-IR destination snapshots. |
| PROV-5 | Sibling relations survive representation changes, whole moves and destruction without address-based identity. | Brand-substitution snapshots and soundness proof; construction, destructuring, whole moves and prohibited field mutation have source coverage. |

V20 ratifies the file/position/container signatures and permits conservative read-only placement. Both annotated and unannotated read-only controls accept. Handle narrowing has a self-contained run test; exact placement and identity propagation still need the model oracles above.

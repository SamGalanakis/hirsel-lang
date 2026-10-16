# Closure implementation oracles

| Rule | Part no single program can observe | Oracle |
| --- | --- | --- |
| CLO-1 | Scoped environment is in the caller's region and never allocates. | Allocation instrumentation and typed-IR placement snapshots; an @noalloc acceptance case exercises the source promise. |
| CLO-2 | Transparent kind, authority and allocation joins agree across specialization boundaries. | Signature/typed-IR snapshots and random-topology differential testing; restricted-parameter source tests cover rejection. |
| CLO-3 | Child environment has no counted parent borrow and Shared roots are copied atomically. | RC/modes transfer proof, count traces and simulated-schedule stress; capture and runtime move cases are written. |
| CLO-4 | Escaping capture implementation preserves declared authority through erasure and separate compilation. | Capture-layout/type-summary snapshots and capability soundness model. |
| CLO-5 | Direct/inlinable specialized scoped calls, with indirect calls inside recursive cycles. This is explicitly "a cost guarantee, not an observable one." | Inspect generated IR/native code for the direct and recursive accepted programs; benchmark scoped map against erased Iter, across compilation topologies. |

No source violation exists for CLO-5's compiler cost promise. Its two accepted shapes are inputs to the code-generation oracle, not proof of the cost guarantee. Signature defaults, closure capture/type spelling, diagnostic precedence and iterator adapter signatures are ratified in v20.

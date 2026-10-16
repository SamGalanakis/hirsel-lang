# Semantic properties requiring another oracle

| Rule | Part | Oracle |
| --- | --- | --- |
| SEM-1 | Every width/value obeys checked arithmetic, conversions and operator typing | Exhaustive small-width numeric model and generated larger-width boundary/property tests; `.hir` gives concrete boundaries, signs and mixed-width twins. |
| SEM-2 | Correct rounding of every math operation/input, one observable NaN across all bit encodings, no contraction/reassociation at every topology/optimization | Correctly rounded arithmetic reference plus generated IEEE bit-pattern cases and interpreter/native/optimizer differential runs. Safe code exposes no bit reinterpretation primitive to manufacture NaN payloads. |
| SEM-3 | Always-valid UTF-8 across all construction paths, Unicode 16.0.0 tables, scalar-safe slicing for all strings/positions, bytewise hashing | UTF-8 construction proof plus generated encoding/view cases, pinned Unicode conformance data and hashing library model. Program examples exercise views, UTF-8 byte lengths, equality/order, slicing and bidi rejection. |
| SEM-4 | Each erased Iter adapter step is an indirect call | IR/call-graph audit. This is a cost property, not an observable program result. |
| SEM-5 | Hash seed changes placement only, keyed SipHash-1-3, behavior under every hash seed | Repeat map-order/equality corpus under many configured hash seeds; published SipHash-1-3 vectors and instrumented placement model. Safe code cannot observe buckets or set the run configuration. |

V20 fixes float.sum to adjacent-pair rounds with odd-tail carry. Compare emitted
reduction graphs and adversarial rounding vectors to that tree; sum_exact uses an
exact accumulator oracle, independently of target vector width.

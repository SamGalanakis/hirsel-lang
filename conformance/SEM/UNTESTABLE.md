# Semantic properties requiring another oracle

| Rule | Part | Oracle |
| --- | --- | --- |
| SEM-1 | Every width/value obeys checked arithmetic, conversions and operator typing | Exhaustive small-width numeric model and generated larger-width boundary/property tests; `.hir` gives concrete boundaries, signs and mixed-width twins. |
| SEM-2 | Correct rounding of every math operation/input, one observable NaN across all bit encodings, no contraction/reassociation at every topology/optimization | Correctly rounded arithmetic reference plus generated IEEE bit-pattern cases and interpreter/native/optimizer differential runs. Observable NaNs are canonicalized, including through `from_bits`. |
| SEM-3 | Always-valid UTF-8 across all construction paths, Unicode 16.0.0 tables, scalar-safe slicing for all strings/positions, bytewise hashing | UTF-8 construction proof plus generated encoding/view cases, pinned Unicode conformance data and hashing library model. Program examples exercise views, UTF-8 byte lengths, equality/order, slicing and bidi rejection. |
| SEM-4 | Each erased Iter adapter step is an indirect call | IR/call-graph audit. This is a cost property, not an observable program result. |
| SEM-5 | Hash seed changes placement only, keyed SipHash-1-3, behavior under every hash seed | Repeat map-order/equality corpus under many configured hash seeds; published SipHash-1-3 vectors and instrumented placement model. Safe code cannot observe buckets or set the run configuration. |

V20 fixes float.sum to adjacent-pair rounds with odd-tail carry. Compare emitted
reduction graphs and adversarial rounding vectors to that tree; sum_exact uses an
exact accumulator oracle, independently of target vector width.

Finite SEM-2 reduction evidence: `float_reduction_model_16` reuses the iterative
round oracle from `stdlib_wave2_num`, rather than the recursive std implementation.
It tracks symbolic leaf/pair DAGs for every length 0..65, checks hand-derived trees
at 0/1/3/5/7 and adjacency, node use and depth at every length. A symbolic
instantiation of each live std `pair_range` traversal must produce exactly the
same leaf-pair tree, preserving its count/split/recursion control flow. It records
scalar node IDs and edges in 132 independent executions, each under the existing
million-step budget. For each width, 28 hand-derived IEEE vectors independently validate the oracle before 198 bounded
generated cases (226 total, in batches of at most 16) run on the interpreter,
checked heap, LLVM Speed and Cranelift. These cover cancellation, halfway rounding, subnormal/normal boundaries,
order-sensitive permutations, signed zeros, infinities and canonical NaN. A wrong
reassociated answer must produce a language test failure on all four engines;
checked heap runs must leave zero live bytes. Public-contract twins are
`SEM-2-float-reduction-model-16-{run,invalid-run}.hir`.

The scoped emission audit follows `hirsel-llvm/src/scalar.rs` through
`context.rs::Builder::binary`: plain `LLVMBuildBinOp`, no fast-math flags;
only explicit `mul_add` uses `llvm.fma`. `hirsel-cranelift/src/scalar.rs`
emits separate `fadd`/`fmul`, and explicit `mul_add` uses `fma`;
`module.rs::isa_with_verifier` selects speed without relaxing IEEE arithmetic.
Executable cancellation and contraction controls check observable bits after
optimization, without depending on LLVM text or instruction scheduling. These
finite tests do not prove all reduction graphs, bit patterns or target optimizers.
The existing `stdlib_exact_sum_vectors` coverage remains separate and unchanged.

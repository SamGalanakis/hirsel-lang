# hirsel-kernels

Pure leaf kernels extracted without changing semantics: integer conversion, SipHash-1-3, Unicode tables, the Test report line encoding (`report`: escaped `test log:` and `test snapshot` records and the snapshot name rule) and the simulated disk's crash model (`disk`: durable directory listings and file bytes, and the tree a seeded crash leaves). Both the
runtime and interpreter adapters may call them; independent mathematical vectors
remain the oracle, because a shared kernel introduces common-mode errors.

Allowed dependencies: `std`, `hirsel-core-contract`, and exactly pinned pure Unicode table crates with default features disabled. This is the narrow `LeafKernel` class, consumed only by runtime, interpreter and heapcheck; `hirsel-lir/tests/dependencies.rs` enforces it.

Invariants:

- Kernels are pure functions of their arguments: no ambient time, randomness,
  environment, allocation-visible state or I/O.
- A kernel never reads interpreter `Value`s or native heap blocks; adapters
  convert at the boundary.

`schedule` holds the scheduler policies `fifo@1`, `random@1` and `pct@1`
(docs/spec/determinism.md, "Scheduler"): a SplitMix64 state, `pct@1`'s depth, horizon and
change points, priorities for new tasks (`admit`), the change point a scheduling point hits
(`point`) and the choice among runnable priorities (`choose`). Both engines call them at the
same points; `//:hirsel-kernels-test-vectors` pins choices from an independent transcription.

`integer::{Format, Width}` transports scalar bit vectors, not runtime values. Grapheme segmentation and NFC normalization ship as ordinary std@1 source with vendored Unicode16 tables; there is no Rust segmentation or normalization kernel.

`//:hirsel-kernels-test-vectors` checks every format conversion pair and masked-shift vectors, plus byte lengths 0–256 against the independent standard-library SipHash-1-3 implementation. Vector provenance lives in `native-tests/library/README.md`.

Numeric formatting, parsing and reductions are ordinary std source; the former
exact-sum kernel is deleted. Its independent rational vectors remain in
`native-tests/library/exact-sums.txt` and now test the source superaccumulator.

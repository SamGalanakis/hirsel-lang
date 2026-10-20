# hirsel-rt-operations

The native operation inventory of the runtime interface `hirsel_rt_v2`: which registry
operations have a native entry, an inline instruction or a library body, the ABI exports
emission calls (`HIRSEL_ABI_EXPORTS`), and the typed adapters that classify registry rows.
The compiler reads it; the runtime archive does not link it.

Allowed dependencies: std and the data-only core-contract.

`manifest` joins registry-produced rows against all reviewed identities in `native-tests/library/operations.tsv`, including exact declaration digests and passing/cleanup conventions. Missing, duplicate, stale and foreign identities fail closed. M2 entries name their feature; remaining aggregate/backend gaps explicitly use NotImplemented. Effects retain registry conservatism. The scalar operations (`SCALAR_INTRINSICS`) are `Instruction` rows: LIR instructions since schema revision 6, with no runtime entry. So are the inline library operations (`INLINE_OPERATIONS`: the sequence lengths, which import lowers to `SeqLen`, and the legalized `simd.add` and `simd.splat`, and `forget`, which consumes without cleanup); a library body's row is `Body`, and each library lane edits only its own operations' rows. The four f32/f64 bits/from_bits operations are among them: `Convert` in its `Reinterpret` mode (LIR revision 23), a bitcast with NaN canonicalization; so are `f64.from_f32` and `f32.from_f64`, `Convert` in its `FloatResize` mode (LIR revision 38). Formatting, parsing and reductions are ordinary std source, with no host algorithm entry or boxed universal value.

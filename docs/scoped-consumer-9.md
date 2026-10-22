Strict source and linked physical proofs authorize bounded explicit captures. Source semantic rules and TIR stay unchanged. FnAbi appends ScopedEntry {parameter: ParameterPath, identity: ContentHash}; it binds selected callable parameter content to a declaration-derived invocation contract. Missing evidence refuses complete qualification, retaining ordinary heap acceptance. The whole-value selector is initially supported; field selectors remain conservative fallback. Captured callable contracts, arbitrary dynamic projections and unavailable bodies are unsupported.

The identity includes declaration parameter order, each parameter convention/passing and layout content key, result layout/result-loan parameters, multiplicity, divergence, nonretaining formal selectors and recursively nested entries. It excludes body symbol, data-instance identity, hidden environment and caller-site decoration. This permits checked compatible entries with different capture data, while the optimizer separately requires a single actual creation/carrier chain and passes its exact ordered data.

C independently unions declaration-contract seeds, checked FunctionRef/ClosureCode identities and reaching whole-storage SSA versions. Mutable storage is unknown; CFG joins union all possible signatures, and an unknown arm poisons qualification. Each contracted indirect call must match its callee's reconstructed identity; each higher-order actual must match the expected nested identity. Every physical body receives strict per-unit C first, including retainers; complete links bind selectors and identity. Recursion is conditional on every body satisfying its declared contract and every outgoing call being checked. Source V separately reconstructs nested retention selectors from declared callable types and checks scoped actual correspondence; it never reads physical optimizer metadata.

Direct transformation uses ordinary Value IR and Definition ABI: ordinary arguments precede explicit borrowed captures in stable physical field order. A Unit carrier replaces only fully eliminated callable values/storage; existing source loans/cleanup anchors remain. The existing Editor prunes unused failure edges and compacts values after removal. Captured owners/resources/Shared counts are not environment credits and are retained. Scalar snapshots and Borrow captures qualify; owning/once and borrowed-result mapping stay heap fallback. No frame environment construct or data-pointer ABI exists.

Whole-site transactional fallback is essential: saturation, a mixed-data join, an unsupported use, failed V or complete C (before or after) leaves that site's original units/layouts untouched. Existing 16-per-function/256-extra ceilings include the preceding code-target specialization work.

## Reference and limits

The reference is aardappel/goose at `8511ae34bdc9aabf77baaab80b4d659ea87835e2`, Apache-2.0; license text and attribution are retained in `third-party/NOTICES.md`. Ordered captures, transitive propagation and ordinary-before-capture arguments follow `src/codegen_frames.h:115–154,210–235,329–350` and `src/codegen_calls.h:245–255`. Nested/repeated/recursive/loop vector structures come from `test/codegen/funval_body_captures.goose:32–89`; its independent stdout oracle is `test/expected/funval_body_captures.out:1–31`.

Hirsel deliberately keeps runtime-selected/owning/once callables, generic heap fallback, its existing authority/loans and independent V/C/H. It lowers ordinary capture data through borrowed Definition parameters and a dead Unit carrier, rather than importing Goose's static-only function-value restrictions or stack representation. Budget saturation is a whole-site fallback. No arbitrary dynamic projection, generic declaration nonretention promise, borrowed-result remapping or captured callable-entry identity is invented. Stage 3 frames, adapters, generations and runtime layouts remain unimplemented. Full CLO-1 and V-flow's independent 2ms target are not claimed.

Production enables this pass only for explicit `--release` and `--lto`
builds. Debug/default builds and interactive native runs retain ordinary heap
lowering to avoid expensive qualification in the edit loop. The low-level
`Options::new` harness configuration enables all optimizations; its
`scoped_specialization` flag permits explicit ablation and is part of native
plan and differential cache identity. Cached imports remain pre-specialization.

C derives each ABI's invocation identity once per checked table and shares it
across units. Bodies without nested actual or contracted indirect-call
obligations skip entry-flow reconstruction. Strict body proofs cached within
one compile bind invocation identity, exact body bytes and reached input rows;
operation/discharge-bearing bodies recheck. Every proposed transaction checks
linked actuals again. These caches never merge same-signature bodies or confer
nonretention from code identity. Source V retains one independently checked
signature universe per compilation. Qualification remains expensive on large
packages; this change claims runtime environment elimination, with no compile
performance improvement claim.

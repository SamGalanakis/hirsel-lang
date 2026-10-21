# Origin flow storage

`Indexed` numbers complete keys independently inside V. A shallow radix prefix
holds the first 512 keys; the remaining keys use a separate sparse radix tail.
The prefix is a storage boundary, not a fact-domain bound. Both segments retain
the original combined capacity, share the same key numbering across snapshots,
and participate in equality, iteration and joins. Independently numbered maps
still join by key. A present empty value remains distinct from a missing key.

The small/large domain split follows rustc's
[`MixedBitSet`](https://github.com/rust-lang/rust/blob/19833af7224424e8b53e288097d56b6af43d5bcf/compiler/rustc_index/src/bit_set.rs#L1057).
Hirsel keeps a sparse tail alongside the prefix because origin keys can first
appear during flow; rustc selects one representation from a fixed domain size.
Hirsel retains its existing persistent radix rows rather than copying a flat
array at each branch. The 512-entry prefix spans three existing radix levels.
The boundary/snapshot controls adapt rustc's
[bitset tests](https://github.com/rust-lang/rust/blob/19833af7224424e8b53e288097d56b6af43d5bcf/compiler/rustc_index/src/bit_set/tests.rs#L237),
with additional absent/empty and independently numbered-map cases required by
origin semantics.

Overwriting a value clears its seven origin fact rows through equality-checked
replacement. Empty rows stay unmaterialized, and shared collections can be
discarded without first cloning their contents. Retained/environment rows still
update their reverse holder indexes; name bindings, locations and expiration
history remain separate facts. Explicit empty entries inside a part or proof
map are removed only when the entire value is overwritten.
The allocation issue follows Rust's
[`Rc::make_mut`](https://github.com/rust-lang/rust/blob/a7c37f9f0ee5e93396106e9de20f4a86b4154e63/library/alloc/src/rcs/rc.rs#L2223):
requesting mutable access to a shared row clones it before `clear` can run.
Replacement uses the existing `Rows::set` equality check and copy-on-write path;
it introduces no alternative origin-join policy.

These changes preserve the full solver, reachability and audit classes. No
inference facts or entry certificates determine acceptance. The counterexamples
in `flow_evidence_laws.rs` remain controls for those boundaries.

`HIRSEL_VALIDATE_PROFILE=1` reports complete definition validation and individual
body flow separately. `hirsel-validate-flow-profile` rows name the definition,
body ordinal, optional `@preserves_unique` proof parameter and elapsed
nanoseconds. Body-flow intervals include model construction, fixed-point search
and reporting replay; they exclude the final model destructor, the enclosing
definition's type checks and read finalization. Nested body invocations remain
separate rows and can overlap their parent's interval. The timer is
observational and never changes the validation policy.

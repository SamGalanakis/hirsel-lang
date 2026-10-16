# SER-2 version 1 evidence

The normative format is MEM-12/SER-1/2 in `docs/spec/serialization.md`.
`golden.tsv` contains the complete canonical descriptor, hand-specified payload
hex and decoded data for 44 values. `malformed.tsv` names an accepted schema
from that corpus, invalid payload bytes and the expected error. Every rejection
has its canonical control in the golden corpus.

`oracle.py` reads descriptors and payloads and independently encodes decoded data
back to identical bytes. It imports no compiler or native layout code. The Rust
integration test runs it, checks every envelope truncation, exercises hash
placement seeds and recursive trees, and validates the fixed flat common subset.
`hirsel-lir/tests/canonical_wire.rs` proves that subset's field offsets and bytes
on the x86-64 and aarch64 target descriptions; ordinary padding is a rejected twin.

Reproduce the actual target round trips with the installed tools:

```sh
export CARGO_BUILD_JOBS=4 HIRSEL_JOBS=4 RUST_TEST_THREADS=4
cargo test -p hirsel-types --test wire -- --test-threads=4
CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER=aarch64-linux-gnu-gcc \
CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_RUNNER='qemu-aarch64-static -L /usr/aarch64-linux-gnu' \
cargo test -p hirsel-types --target aarch64-unknown-linux-gnu --test wire -- --test-threads=4
cargo test -p hirsel-lir --test it canonical_wire:: -- --test-threads=4
```

The ignored `write_wire_goldens` test is only a manual regeneration tool. Payload
hex remains explicitly asserted in the Rust fixtures; regeneration cannot change
those expectations. The Python oracle has its own finite host depth/work budget,
whereas the Rust codec's arena walks and destruction use no host recursion.

# Reference vectors

The shared runner is `crates/hirsel-driver/tests/it-stdlib/reference_vectors.rs`.
It follows `stdlib_wave2_num.rs:61`'s host-side vector generation and bounded
batches: ordinary Hirsel source is checked and run through the driver. Interpreter
steps are unlimited (`Limits { steps: None }`); call depth remains bounded at 256.
Each case has its own function and upstream identifier. A failure prints the suite,
batch IDs and generated source, including the failing assertion's source location.

Hash, MAC and KDF suites: see [HASHES.md](HASHES.md) for counts, exact sources
and every adaptation. `run_native` links the same assertions for expensive
PBKDF2 work factors and long Adler-32 inputs.

## AEAD suites

`stdlib_aead_vectors.rs` registers 26 Rust tests: eight shards for each Wycheproof
suite, one GCM-spec test and one internal primitive test. Shards partition the
selected vectors by ordinal modulo eight, including the remainder. Generated
programs contain at most eight AEAD cases or four spec/primitive cases.

| Suite | Applicable | Valid | Invalid | Before this harness |
| --- | ---: | ---: | ---: | ---: |
| AES-GCM Wycheproof | 197 | 116 | 81 | 23 |
| ChaCha20-Poly1305 Wycheproof | 316 | 256 | 60 | 18 |
| XChaCha20-Poly1305 Wycheproof | 306 | 246 | 60 | 17 |
| GCM specification appendix B | 12 | 12 | 0 | 4 |
| RFC 8439 A.1 / A.2 / A.3 | 5 / 3 / 11 | 19 | 0 | 0 |

Every valid AEAD row checks **both** seal against published ciphertext/tag and open
against published plaintext. Every invalid row must return `AuthenticationFailed()`.
No expected output is obtained by a round trip. Full upstream JSON is retained,
including unsupported parameter groups; only the supported nonce widths (96 bits,
or 192 for XChaCha) and 128-bit tags are selected. Applicable counts and upstream
`numberOfTests` are asserted. All three AES key sizes are included. No `acceptable`
row occurs in this selection; an unknown result is a host test failure.

The audit's narrative says 87 invalid AES cases; parsing its pinned file gives
81 invalid and 116 valid among the 197 applicable cases. No applicable row is skipped.

## Sources and exact locations

The references were cloned before porting. Wycheproof is C2SP/wycheproof at
`7a325932231bd7a724c791c8b0d65ea4d6d2f435`; Zig is ziglang/zig at
`bff490a23889750de7096d9057c0a432efc929a8`.

- [Wycheproof `schemas/aead_test_schema_v1.json:4`](https://github.com/C2SP/wycheproof/blob/7a325932231bd7a724c791c8b0d65ea4d6d2f435/schemas/aead_test_schema_v1.json#L4):
  group parameters and test rows; `:37` specifies separate hex key, nonce, AAD,
  message, ciphertext and tag. The runner preserves this structure and result contract.
- [Wycheproof `testvectors_v1/aes_gcm_test.json:1`](https://github.com/C2SP/wycheproof/blob/7a325932231bd7a724c791c8b0d65ea4d6d2f435/testvectors_v1/aes_gcm_test.json#L1),
  [`chacha20_poly1305_test.json:1`](https://github.com/C2SP/wycheproof/blob/7a325932231bd7a724c791c8b0d65ea4d6d2f435/testvectors_v1/chacha20_poly1305_test.json#L1),
  [`xchacha20_poly1305_test.json:1`](https://github.com/C2SP/wycheproof/blob/7a325932231bd7a724c791c8b0d65ea4d6d2f435/testvectors_v1/xchacha20_poly1305_test.json#L1):
  vendored byte for byte under their original names. Copyright 2016–2026 The
  Wycheproof Authors, Apache-2.0; see `LICENSE.wycheproof` and `third-party/NOTICES.md`.
- [Zig `lib/std/crypto/test.zig:13`](https://github.com/ziglang/zig/blob/bff490a23889750de7096d9057c0a432efc929a8/lib/std/crypto/test.zig#L13):
  decode expected hex and compare bytes, ported as `reference_vectors::bytes` and
  Hirsel equality assertions. [`chacha20.zig:777`](https://github.com/ziglang/zig/blob/bff490a23889750de7096d9057c0a432efc929a8/lib/std/crypto/chacha20.zig#L777)
  checks a published encryption then decryption; both directions are retained here.
- [Zig `lib/std/crypto/poly1305.zig:156`](https://github.com/ziglang/zig/blob/bff490a23889750de7096d9057c0a432efc929a8/lib/std/crypto/poly1305.zig#L156):
  final partial block appends `1`, then zeroes, and suppresses the full-block high
  bit. `primitive-poly1305.hir` ports this for direct private-kernel testing.
- [RFC 8439](https://www.rfc-editor.org/rfc/rfc8439.txt), plain text
  `rfc8439.txt:1636` (A.1), `:1799` (A.2), `:1977` (A.3), through `:2275`:
  `rfc8439.json` transcribes all 19 vectors with IDs `A.section.vector`, literal
  bytes and the published block counters. A.1 feeds 64 zero bytes to the actual
  ChaCha20 XOR primitive to observe its block; A.2 includes multiblock encryption;
  A.3 includes partial blocks, carries and final reductions. Copyright (c) 2018
  IETF Trust and the persons identified as the document authors; Simplified BSD
  terms are in `third-party/NOTICES.md`.
- McGrew–Viega, [The Galois/Counter Mode of Operation (GCM)](https://luca-giuzzi.unibs.it/corsi/Support/papers-cryptography/gcm-spec.pdf),
  appendix B, printed pages 27–39: `gcm-spec.json` transcribes K, IV, P, A, C and T
  from cases **1–4, 7–10, 13–16**, all twelve with 96-bit IVs. Case IDs are the
  original table numbers. Blank P/A/C fields become empty hex strings. Running
  `pdftotext -layout` puts these tables at `gcm-spec.txt:1295–1832`.

Upstream SHA-256 values (the Wycheproof files are unchanged):

```text
985e5ecc172e181eaf49e89508b9470dcf478002eb7e8559c707eb42dc97dfe7  aes_gcm_test.json
fe61d25f90e1bde4461d00eafe61049e5f29bd999f36b766df9cda90906ad53d  chacha20_poly1305_test.json
a79de072571b90eb40c3a63ce0c7f75dcb4b62323c8870228e1f61dcc61d63a9  xchacha20_poly1305_test.json
25bef70fbf7a07ff45c2fe4cb7c6ce954eac687413d8610603268b4e4415324c  rfc8439.txt
8cec9d63d2e28400773fff488c7aa9fcecb9636d50d994baca3cc025aa23df95  gcm-spec.pdf
```

## Adaptations and reasons

1. There are three upstream AEAD JSON files, shared by the four Hirsel modules.
   `chacha_poly` is an internal implementation, not a separate Wycheproof algorithm;
   it is covered by both ChaCha AEAD suites and the direct RFC primitive test.
2. Unsupported nonce/tag groups stay vendored but are not executed: Hirsel's APIs
   require fixed nonce widths and full tags. Existing size-failure corpus twins
   test their preconditions. The six GCM-spec cases with other IV widths likewise
   do not belong to the requested twelve.
3. Rust generates small Hirsel functions instead of upstream Zig comptime arrays.
   This follows the existing numeric harness, permits the same helper to serve
   hash/MAC/KDF and exact-sum suites, and avoids corpus interpreter budgets and
   giant generated programs. Named Rust shards let the normal test runner schedule
   them; the runner changes no compiler or production execution limits.
4. The internal test includes the actual `chacha_poly.hir` verbatim in the fixture
   module so it can call private primitives without exporting a testing API. The
   partial-block adapter is test-only. The private `poly_block` high bit is now an
   explicit argument, as in Zig's full/final block distinction; existing AEAD callers
   always pass `1 << 40`, preserving their padded-block calculation.
5. RFC/PDF tables are transcribed into compact JSON instead of vendoring document
   layout and parsing it during tests. Their published IDs, bytes, counters and
   source locations are preserved; the three Wycheproof files are unmodified.
6. Ciphertext and tag are concatenated at the call boundary because Hirsel's AEAD
   API represents a sealed message as `ciphertext || tag`.

## Add a suite

1. Clone and pin a primary reference. Vendor its fixed vectors here, preserving
   identifiers, expected values, license and file/line provenance. Prefer the raw
   upstream format; document any selection criteria and assert the selected count.
2. Add an it-stdlib module and register it in `main.rs`. Parse the data on the host
   (`serde_json` is already available); build a `Vec<reference_vectors::Case>`.
   Each case has a diagnostic `id` and a Hirsel `body` with `t: Test` in scope.
   `reference_vectors::bytes(hex)` emits validated byte literals. Other suites may
   generate float bit patterns, iteration counts, arrays, or any other source.
3. Call `reference_vectors::run(suite, &cases, shard, shards, batch_size, preamble)`.
   For example a digest body can be
   `core@1/test.check(t, std@1/sha256.digest_hex(b"abc") == EXPECTED_HEX)`. Exact sums can compare `core@1/f64.bits(sum)` with a fixed
   expected `u64`; KDFs can use their real iteration counts. The helper imposes no
   crypto-specific data model or step limit. Use an empty preamble for public APIs.
4. Register every ordinal shard as a named `#[test]`; use small source batches.
   Do not silently drop remainders or treat unsupported/unknown results as passing.
   `include_str!` inputs are discovered by the Buck2 build-file generator; do not
   hand-edit generated BUCK files.
5. Run the affected it-stdlib tests directly with `.buck2/bin/buck2 test` and
   std importers through `-c hirsel.conformance_paths='path-a|path-b'
   //:gate-conformance-selected-shard-1`.
   Measure new case costs with libtest `--report-time` and update the normal
   `tools/buck2/test-costs/hirsel-driver-test-it-stdlib.tsv` via
   `tools/buck2/test_costs.py`; this keeps future suites spread over pool shards.

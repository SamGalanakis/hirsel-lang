# Hash, MAC and KDF reference vectors

`stdlib_hash_vectors.rs` adds 31 named Rust tests using `reference_vectors.rs`.
Every imported row is counted before execution. These are fixed published answers;
no test computes its expected digest using the implementation under test.

| Suite | Before (audit) | Published rows now executed |
| --- | ---: | ---: |
| Go SHA-1 golden | 7/32 | 32 |
| Go SHA-224 golden224 | 6/31 | 31 |
| Go SHA-256 golden (optional addition) | 12 distinct digests | 31 |
| Go SHA-384 / SHA-512 / SHA-512-256 | 5/31 each | 31 each |
| Go MD5 golden | 14/31 | 31 |
| Go BLAKE2b / BLAKE2s keyed KAT | 8/256, 9/256 | 256 each |
| RFC 4231 SHA-224/256/384/512 | 7/28 | 28 |
| RFC 2202 SHA-1 / MD5 | 3/7 SHA-1 | 7 each |
| Wycheproof HMAC | 0 | 33 per supported hash, 330 total |
| Go PBKDF2 SHA-1 / SHA-256 | 2 published RFC counts | 5 each |
| Wycheproof PBKDF2 SHA-1 / SHA-256 / SHA-512 | 0 | 63 / 60 / 58 |
| Go Adler-32 golden | 33/43 | 43 |

There are **1,336 rows** in the new suites, including duplicate RFC 6070 rows
retained in both the Go and Wycheproof source suites. Existing corpus coverage
remains. Hashes run both one-shot and split updates (including an empty update),
and BLAKE2 runs both one-write and byte-at-a-time, matching Go's KAT loop.
HMAC checks published tags and `verify`; RFC 4231 case 5 checks the published
128-bit prefix, and RFC 2202 case 5 checks both the full MAC and 96-bit prefix.
The ten HMAC algorithms are SHA-1, SHA-224/256/384/512/512-256 and all four SHA-3
sizes. Wycheproof full-length valid selection yields exactly 33 rows per hash;
its 66 valid total includes 33 truncated tags.

PBKDF2 and Adler-32 use `reference_vectors::run_native`: the same checked Hirsel
assertions are lowered, linked with the pinned toolchain/runtime and executed.
Nonzero exit status fails with the batch IDs, generated source and child output.
The other suites retain the harness's unlimited-step interpreter. Native programs
have bounded source batches (16 cases; Go PBKDF2 has 10), and no work factor is
reduced. RFC 6070 cases 1, 2, 3, 5 and 6 run at **1, 2, 4096, 4096, 4096** rounds.
Wycheproof SHA-256 tcId 2 runs at **80,000** rounds with a 64-byte output.
Adler-32 checks one-shot and split updates for all rows, including eight
`ff * 5548..5555 + digit` rows, 100,000 zero bytes, 100,000 `a` bytes, and
`ABCDEFGHIJKLMNOPQRSTUVWXYZ * 10000` (**260,000**, not 100,000 bytes).

## Pinned references and file:line citations

The sources were cloned before porting: Go `ebe2feefc400f813aefdc8a036e66d2619da6649`,
Go supplementary crypto `0ae6809b7559060fe62abe65e26bd18b1fcaf6b7`, and Wycheproof
`7a325932231bd7a724c791c8b0d65ea4d6d2f435` (the same revisions as the audit/harness).

- Go [src/crypto/sha1/sha1_test.go:29](https://github.com/golang/go/blob/ebe2feefc400f813aefdc8a036e66d2619da6649/src/crypto/sha1/sha1_test.go#L29)
  `golden` (32), `:67` one-shot/streaming loop.
- Go [src/crypto/sha256/sha256_test.go:25](https://github.com/golang/go/blob/ebe2feefc400f813aefdc8a036e66d2619da6649/src/crypto/sha256/sha256_test.go#L25)
  `golden` (31), `:59` `golden224` (31), `:97` test loop.
- Go [src/crypto/sha512/sha512_test.go:187](https://github.com/golang/go/blob/ebe2feefc400f813aefdc8a036e66d2619da6649/src/crypto/sha512/sha512_test.go#L187)
  `golden256` (31), `:345` `golden384` (31), `:503` `golden512` (31), `:661`
  `testHash`. The seven tables become `go-hash-golden.json` with original
  table/ordinal IDs, input bytes and expected hex.
- Go [src/crypto/md5/md5_test.go:25](https://github.com/golang/go/blob/ebe2feefc400f813aefdc8a036e66d2619da6649/src/crypto/md5/md5_test.go#L25)
  `golden` (31), `:59` test loop, in the same JSON.
- Go crypto [blake2b/blake2b_test.go:123](https://github.com/golang/crypto/blob/0ae6809b7559060fe62abe65e26bd18b1fcaf6b7/blake2b/blake2b_test.go#L123)
  key `00..3f`, input `00..i-1`, one-write and byte-at-a-time loop;
  `:333` `hashes` (256) becomes `go-blake2b-keyed.json` unchanged in order.
  [blake2s/blake2s_test.go:115](https://github.com/golang/crypto/blob/0ae6809b7559060fe62abe65e26bd18b1fcaf6b7/blake2s/blake2s_test.go#L115)
  uses key `00..1f`; `:273` `hashes` (256) becomes `go-blake2s-keyed.json`.
- Go [src/crypto/pbkdf2/pbkdf2_test.go:26](https://github.com/golang/go/blob/ebe2feefc400f813aefdc8a036e66d2619da6649/src/crypto/pbkdf2/pbkdf2_test.go#L26)
  `sha1TestVectors`, `:92` `sha256TestVectors`, `:145` derive/compare loop:
  `go-pbkdf2.json` retains all ten active rows and iteration counts.
- Go [src/hash/adler32/adler32_test.go:21](https://github.com/golang/go/blob/ebe2feefc400f813aefdc8a036e66d2619da6649/src/hash/adler32/adler32_test.go#L21)
  `golden` (43), `:82` compare loop: `go-adler32-golden.json` preserves Go's
  repeated-pattern construction and suffix, avoiding huge source literals.
- [RFC 4231 section 4.2–4.8](https://www.rfc-editor.org/rfc/rfc4231.txt), text
  `rfc4231.txt:175–390`: all seven inputs and four published tags per case.
  [RFC 2202 sections 2–3](https://www.rfc-editor.org/rfc/rfc2202.txt), text
  `rfc2202.txt:78–237`: seven MD5 and seven SHA-1 cases, including case 5's
  `digest-96`. `rfc-hmac.json` preserves RFC/case/hash IDs and published bytes.
- [RFC 6070 section 2](https://www.rfc-editor.org/rfc/rfc6070.txt), text
  `rfc6070.txt:89`: all five practical cases are present in Go's SHA-1 rows
  and Wycheproof SHA-1 tcIds 1, 2, 3, 5, 6, with original counts and NUL bytes.
- Wycheproof [schemas/mac_test_schema_v1.json:1](https://github.com/C2SP/wycheproof/blob/7a325932231bd7a724c791c8b0d65ea4d6d2f435/schemas/mac_test_schema_v1.json#L1)
  grouped `tagSize`, `key`, `msg`, `tag`, `result`, `tcId` structure;
  [schemas/pbkdf_test_schema.json:1](https://github.com/C2SP/wycheproof/blob/7a325932231bd7a724c791c8b0d65ea4d6d2f435/schemas/pbkdf_test_schema.json#L1)
  `password`, `salt`, `iterationCount`, byte `dkLen`, `dk`, `result` structure.
  The ten `testvectors_v1/hmac_{sha1,sha224,sha256,sha384,sha512,sha512_256,sha3_224,sha3_256,sha3_384,sha3_512}_test.json:1`
  and three `testvectors_v1/pbkdf2_hmac{sha1,sha256,sha512}_test.json:1` files
  are vendored **byte for byte**, with complete original groups and counts.

Go/Go crypto vector copyright and BSD-3-Clause license are recorded in
`third-party/NOTICES.md`. Wycheproof's Apache-2.0 terms are in
`LICENSE.wycheproof`; RFC copyright and reproduction terms are in the notices.

## Complete adaptation/deviation list

1. Compact JSON transcribes Go/RFC table data instead of parsing Go source or
   RFC layout during every test. Go marshaled intermediate states are omitted:
   Hirsel has no public hash-state serialization. Original input/output bytes,
   iteration counts, table order and identifiers remain. Go's runtime CPU
   implementation selection and `Reset` become fresh Hirsel states; Hirsel has
   one portable source implementation and consuming `finish`. MD5 has no
   `hash()` convenience function, so its HMAC fixture supplies the existing
   public digest dictionary directly (new/update/finish, 64-byte block).
2. Rust emits small ordinary Hirsel programs using the shared harness instead of
   Go's `testing.T`. Bounded batches avoid giant programs. PBKDF2/Adler use linked
   native execution for full work factors/long inputs, following existing native
   stdlib tests, with no production API, ABI, or interpreter-limit changes.
3. Only valid full-length Wycheproof HMAC rows execute, as requested. Invalid and
   truncated groups remain vendored. SHA-512/224 and SM3 have no std hash and are
   not imported. RFC truncation cases compare the published prefix explicitly;
   `verify` accepts full tags, so it is checked only for full published tags.
4. Wycheproof PBKDF2 SHA-1 tcId 4 / RFC 6070 case 4 (16,777,216 iterations) is
   explicitly excluded, as authorized in the audit and as Go does. The complete
   Wycheproof file retains it; the test asserts its exact ID/count and that it is
   the sole exclusion. No other PBKDF2 row is skipped or count weakened.
5. The audit specifies three PBKDF2 Wycheproof suites; newer SHA-224/384 suites
   are outside that list. Optional RustCrypto SHA-3 KAT and Wycheproof HKDF suites
   are deferred: their existing tests are assessed as aligned, and this change
   completes the required missing-vector suites. SHA-256's small optional Go
   table is included alongside the other Go hashes.
6. RFC 2202's source text repeats SHA-1 cases 5–7; each distinct numbered vector
   executes once. Adler's last long row is 260,000 bytes, correcting the audit's
   shorthand description of all three long rows as “1e5-byte rows”.

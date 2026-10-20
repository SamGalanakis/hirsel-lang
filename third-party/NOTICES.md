# Third-party notices

Hirsel adapts code and test vectors from the sources below. Each is used under
the license named in its row; the license texts follow once each.

| Source | Adapted | Where in this tree | Upstream revision | License |
|---|---|---|---|---|
| commonmark.js | CommonMark container matching, inline delimiter/bracket processing and HTML rendering; CommonMark 0.31.2 spec examples | `stdlib/std@1/markdown.hir`, generated `conformance/SEM/SEM-3-markdown-commonmark-*` | commonmark/commonmark.js `898886961b49c1df1175666d03c3ac66c9cc9040` | BSD-2-Clause (implementation); CC-BY-SA-4.0 (spec vectors) |
| cmark-gfm | Generated HTML entity data, autolink scanning and email postprocessing and GFM table, strikethrough, autolink and task-list spec examples with reference-rendered malformed twins | `stdlib/std@1/markdown{,_tables}.hir`, `tools/generate-markdown.py`, generated `conformance/SEM/SEM-3-markdown-gfm-*` | github/cmark-gfm `27d942c8b0a62d192f616e5bf3578f4b6a89e180` | BSD-2-Clause (implementation/data); CC-BY-SA-4.0 (spec vectors) |
| Rust standard library | `library/alloc/src/raw_vec/mod.rs` grow_amortized doubling policy, bounded by the native host extent | unique heap sequence reservation in `crates/hirsel-rt/src/alloc.rs`, incremental-reservation regression | rust-lang/rust `f6c1107f8be13e48465e12c0fd26e6a39fe18e4f` | MIT |
| Go x/net/html | `escape.go` five-character replacement table and `escape_test.go` empty, plain, entity and special-character vectors; CR remains unchanged | `stdlib/std@1/html.hir`, HTML escaping conformance | golang/net `7e562009b3884a0845019b5f886f0242a7c0b970` | BSD-3-Clause (Go) |
| rmp and rmpv | Marker dispatch, big-endian reads, shortest integer/string/collection headers, DOM traversal, and `rmp/tests/func/{encode,decode}` vectors; adapted to the JSON value domain | `stdlib/std@1/{binary_json,msgpack}.hir`, MessagePack conformance | 3Hren/msgpack-rust `cf880019f72e35a884659a9b29d9eb89b862670b` | MIT (Copyright (c) 2017 Evgeny Safronov) |
| ciborium | `ciborium-ll/src/{dec,hdr,enc,seg}.rs` header dispatch, definite/indefinite containers, exact shortest float widths; `ciborium/tests/codec.rs` RFC 8949 Appendix A vectors; modified for Hirsel's JSON domain and explicit deterministic option | `stdlib/std@1/{binary_json,cbor}.hir`, CBOR conformance | enarx/ciborium `00279e48e75ecf9546aae36f6d004447c676065b` | Apache-2.0 (license text below, without the Cranelift LLVM exception) |
| Zig standard library | `lib/std/sort/pdq.zig` pivot sampling, equal partition, pattern breaking, bounded recursion and heapsort fallback; arbitrary-range and patterned input vectors | `stdlib/std@1/list.hir`, `packages/fixtures/list_sort_vectors@1`, list sort conformance | ziglang/zig `bff490a23889750de7096d9057c0a432efc929a8` | MIT |
| Rust standard library | `library/core/src/hash/sip.rs` streaming SipHash-1-3 tail buffering and finalization; chunk-boundary tests checked against `DefaultHasher` | `crates/hirsel-kernels/src/siphash.rs`, native composite-key hashing and kernel vectors | rust-lang/rust `f6c1107f8be13e48465e12c0fd26e6a39fe18e4f` | MIT |
| rustls | RootCertStore::add trust-anchor decoding, TLS verification-name parsing, `ServerConfig::with_single_cert` identity loading (`CertifiedKey::keys_match`), `WebPkiClientVerifier` client-certificate verification and `ResolvesServerCertUsingSni`-style identity selection, with the existing rsa-2048 ca.cert accepted control | `crates/hirsel-run-config/src/tls.rs`, TLS input conformance | rustls/rustls `f04804fa13341a9b1dc02b5d1dd8a746f2c4b7fb` (v/0.23.46) | MIT |
| Rust standard library | `library/std/src/net/udp.rs` and `sys/net/connection/socket/unix.rs` bind/sendto/recvfrom conventions, empty datagram and truncation vectors | UDP core operations, host provider and UDP conformance | rust-lang/rust `f6c1107f8be13e48465e12c0fd26e6a39fe18e4f` | MIT |
| Rust standard library | `library/std/src/sys/fs/unix.rs` no-follow descriptor deletion and pinned permission mutation; remove_dir_all symlink and nested vectors; `library/alloc/src/io/read.rs` 8 KiB stream-copy buffer policy | filesystem breadth providers, `stdlib/std@1/file.hir` copy_file and conformance/host replay tests | rust-lang/rust `f6c1107f8be13e48465e12c0fd26e6a39fe18e4f` | MIT |
| Zig standard library | `lib/std/compress/zstd.zig`, `lib/std/compress/zstd/Decompress.zig` frame, FSE, Huffman and repeat-offset decoder and vectors | `stdlib/std@1/zstd.hir`, `conformance/SEM/SEM-3-zstd-*.hir` | ziglang/zig `bff490a23889750de7096d9057c0a432efc929a8` | MIT |
| Zstandard | `tests/golden-decompression/`, `tests/golden-decompression-errors/`, `tests/golden-compression/{http,huffman-compressed-larger,large-literal-and-match-lengths}` | Zstandard conformance and `packages/fixtures/zstd@1/` | facebook/zstd `49cf51799ea051457f57243527aa2058f999f005` | BSD-3-Clause (Zstandard) |
| Zstandard | libzstd 1.5.5 compression and dictionary-training outputs, with deterministic HTTP/JSON and varied-length fragments | `packages/fixtures/zstd@1/vectors.hir`, Zstandard conformance | facebook/zstd v1.5.5 | BSD-3-Clause (Zstandard) |
| RFC 8878 | section 4.2 Huffman weights example and appendix A default tables | `stdlib/std@1/zstd.hir`, Zstandard entropy conformance | RFC 8878 (February 2021) | Simplified BSD (IETF Trust) |
| Python standard library | Generated SHA-256, SHA-512, BLAKE2b-512 and BLAKE2s-256 reference digests for `bytes(range(256)) * 12288 + bytes(range(17))`; no implementation copied | `crates/hirsel-driver/tests/fixtures/hash-large-native.hir` | CPython 3.12.3 `hashlib` | Generated digest values |
| Zig standard library | `lib/std/crypto/{Sha1,sha2}.zig` partial-block / direct full-block / tail streaming update | `stdlib/std@1/{sha1,sha256,sha512,md5}.hir` block update paths and split-boundary conformance | ziglang/zig `bff490a23889750de7096d9057c0a432efc929a8` | MIT |
| Zig standard library | `lib/std/Io/Dir.zig` Walker explicit stack, directory-only descent and recursive traversal vectors | `stdlib/std@1/file.hir` list_recursive, filesystem breadth conformance | ziglang/zig `bff490a23889750de7096d9057c0a432efc929a8` | MIT |
| Go standard library | `src/unicode/letter.go` range binary search adapted from the existing Unicode property lookup; Unicode16 UCD supplies display-width data and P1868R2 supplies the estimate | `stdlib/std@1/display_width.hir`, display-width conformance | golang/go `ebe2feefc400f813aefdc8a036e66d2619da6649` | BSD-3-Clause (Go) |
| Go x/net | `http2/hpack/{hpack,encode,huffman,tables,static_table}.go` and decoder, encoder, Huffman test vectors | `stdlib/std@1/hpack*.hir`, HPACK conformance | golang/net `7e562009b3884a0845019b5f886f0242a7c0b970` | BSD-3-Clause (Go) |
| Go standard library | `src/net/http/internal/http2/{frame,transport,server}.go` and `frame_test.go`: frame validation, connection flow control and framing tests | `stdlib/std@1/http2_wire.hir`, `stdlib/std@1/http2.hir`, HTTP/2 conformance | golang/go `ebe2feefc400f813aefdc8a036e66d2619da6649` | BSD-3-Clause (Go) |
| Go standard library | `src/net/http/internal/http2/{transport,server}.go` request/header mapping, DATA streaming and transport/server body vectors | `stdlib/std@1/http_h2.hir`, `conformance/SEM/SEM-1-http-h2-*-run.hir` | golang/go `ebe2feefc400f813aefdc8a036e66d2619da6649` | BSD-3-Clause (Go) |
| Go standard library | `src/net/http/internal/{chunked.go,chunked_test.go}` chunkedWriter.Write/Close, empty-write rule and TestChunk vectors, adapted to owned server streams and explicit end_response_body | `stdlib/std@1/http.hir`, HTTP streaming conformance | golang/go `ebe2feefc400f813aefdc8a036e66d2619da6649` | BSD-3-Clause (Go) |
| Go standard library | `src/net/http/transport_test.go` TestTransportKeepAlives, TestTransportConnectionCloseOnResponse, TestTransportRespectRequestWantsClose, TestTransportReadToEndReusesConn and stale-connection controls; adapted to scripted peers | `packages/fixtures/http_client_vectors@1`, `conformance/SEM/SEM-1-http-client-*` | golang/go `ebe2feefc400f813aefdc8a036e66d2619da6649` | BSD-3-Clause (Go) |
| Rust standard library | `library/core/src/panic/location.rs` new_location and constant_location documentation examples, adapted to caught failures and explicit task isolation | caller-attribution conformance | rust-lang/rust `f6c1107f8be13e48465e12c0fd26e6a39fe18e4f` | MIT |
| Rust standard library | `library/core/src/{mem/mod,option}.rs` replace and Option::take implementation/documentation examples, adapted to mutable places | core replace lowering, `stdlib/std@1/option.hir`, replace/remove conformance | rust-lang/rust `f6c1107f8be13e48465e12c0fd26e6a39fe18e4f` | MIT |
| Rust standard library | `library/test/src/lib.rs` filter_tests substring/OR selection and `tests.rs` filter vectors, adapted to explicit @test definitions | `crates/hirsel-driver/src/testing.rs`, CLI test-runner tests | rust-lang/rust `f6c1107f8be13e48465e12c0fd26e6a39fe18e4f` | MIT |
| Rust standard library | `library/std/tests/sync/mpsc.rs` smoke and smoke_threads channel vectors, extended to a ping-pong exchange | `conformance/LANG/LANG-9-test-tasks-ping-pong.hir` | rust-lang/rust `f6c1107f8be13e48465e12c0fd26e6a39fe18e4f` | MIT |
| Rust standard library | `library/core/src/{option,result}.rs` owned helpers and documentation examples, adapted to taking callbacks and Hirsel error spelling | `stdlib/std@1/{option,results}.hir`, Option/Result conformance | rust-lang/rust `f6c1107f8be13e48465e12c0fd26e6a39fe18e4f` | MIT |
| Rust standard library | `library/core/src/option.rs` null-pointer representation and `ptr/non_null.rs` checked null construction, with null/non-null and round-trip vectors adapted to Hirsel C boundaries | pointer niche layout, foreign adapters, PROG-4 conformance and C fixture | rust-lang/rust `f6c1107f8be13e48465e12c0fd26e6a39fe18e4f` | MIT |
| Go standard library | `src/net/http/serve_test.go` TestServeHTTP10Close, TestClientCanClose, HTTP10/11KeepAlive204Response, HTTP10KeepAlive304Response, testTCPConnectionStaysOpen and CloseNotifierPipelined cases; adapted to deterministic scripted connections and real clients | HTTP persistence conformance and hello-web-concurrent E2Es | golang/go `ebe2feefc400f813aefdc8a036e66d2619da6649` | BSD-3-Clause (Go) |
| Go standard library | `src/net/url/{url.go,url_test.go}` parseQuery, QueryUnescape, QueryEscape and ParseQuery vectors; ordered owned pairs, atomic errors and byte offsets replace Go maps and partial results | `stdlib/std@1/uri.hir`, URI query conformance | golang/go `ebe2feefc400f813aefdc8a036e66d2619da6649` | BSD-3-Clause (Go) |
| Rust standard library | `library/core/src/fmt/{mod,num,float}.rs`, `library/alloctests/tests/fmt.rs`, `library/coretests/tests/num/flt2dec/mod.rs` padding, sign/prefix, numeric precision and format vectors; `library/alloc/src/fmt.rs` named `name$` width/precision count grammar | `stdlib/std@1/format.hir`, scalar `format_into_spec`, `text.pad_from`, `crates/hirsel-parser/src/formats.rs`, `conformance/SEM/SEM-3-fstring-*`, `conformance/SEM/SEM-3-format-*` | rust-lang/rust `f6c1107f8be13e48465e12c0fd26e6a39fe18e4f` | MIT |
| Zig standard library | `lib/std/unicode.zig` utf8Encode scalar-boundary cases | UTF-8 conformance (`char.format_into` and `char.from_u32` boundaries) | ziglang/zig `bff490a23889750de7096d9057c0a432efc929a8` | MIT |
| Rust standard library | `library/core/src/str/validations.rs`, `library/alloc/src/string.rs`, `library/alloctests/tests/string.rs` lossy-prefix/error-span semantics and test_from_utf8_lossy vectors | `stdlib/std@1/utf8.hir`, UTF-8 conformance | rust-lang/rust `f6c1107f8be13e48465e12c0fd26e6a39fe18e4f` | MIT |
| Rust standard library | `library/core/src/char/methods.rs` encode_utf8_raw and `char::from_u32`'s scalar test; `library/coretests/tests/char.rs` test_convert and test_encode_utf8 vectors | `stdlib/std@1/char.hir`, `char.from_u32` native legalization, char conformance | rust-lang/rust `f6c1107f8be13e48465e12c0fd26e6a39fe18e4f` | MIT |
| Rust standard library | `library/core/src/str/mod.rs`, `library/alloc/src/str.rs`, `library/alloctests/tests/str.rs` contains, lines and replace algorithms/contracts and multilingual/line-ending vectors | `stdlib/std@1/string.hir`, common string conformance | rust-lang/rust `f6c1107f8be13e48465e12c0fd26e6a39fe18e4f` | MIT |
| Zig standard library | `lib/std/crypto/sha2.zig` portable SHA-256 state, rounds, update/final and empty/abc/long/streaming/aligned vectors | `stdlib/std@1/sha256.hir`, SHA-256 conformance | ziglang/zig `bff490a23889750de7096d9057c0a432efc929a8` | MIT |
| Zig standard library | `lib/std/hash/fnv.zig` FNV-1a 64 offset basis, prime, byte update and empty/"a"/"foobar" vectors | `stdlib/std@1/fnv.hir`, `std@1/derive.hash`, FNV conformance | ziglang/zig `bff490a23889750de7096d9057c0a432efc929a8` | MIT |
| Go standard library | `src/crypto/sha1/{sha1.go,sha1block.go}` and `src/crypto/md5/{md5.go,md5block.go}` generic blocks, constants and padding; `sha1_test.go`/`md5_test.go` golden vectors | `stdlib/std@1/{sha1,md5}.hir`, SHA-1 and MD5 conformance | golang/go `ebe2feefc400f813aefdc8a036e66d2619da6649` | BSD-3-Clause (Go) |
| Go standard library | `src/crypto/internal/fips140/sha256/sha256.go` SHA-224 initial words and `src/crypto/internal/fips140/sha512/{sha512.go,sha512block.go}` constants, generic block, 128-bit length padding and SHA-384/512-256 initial words; `src/crypto/sha256/sha256_test.go` golden224 and `src/crypto/sha512/sha512_test.go` golden384/512/256 vectors | `stdlib/std@1/{sha224,sha384,sha512,sha512_256}.hir`, SHA-2 conformance | golang/go `ebe2feefc400f813aefdc8a036e66d2619da6649` | BSD-3-Clause (Go) |
| Go supplementary crypto | `blake2b/{blake2b.go,blake2b_generic.go}` and `blake2s/{blake2s.go,blake2s_generic.go}` parameter block, keyed first block, held-back final block and G/round schedule; `blake2b_test.go`/`blake2s_test.go` keyed known-answer vectors | `stdlib/std@1/{blake2b,blake2s}.hir`, BLAKE2 conformance | golang/crypto `0ae6809b7559060fe62abe65e26bd18b1fcaf6b7` | BSD-3-Clause (Go) |
| Zig standard library | `lib/std/crypto/keccak_p.zig` and `sha3.zig` Keccak-f[1600] round constants, rotation offsets, sponge padding and SHA-3/SHAKE vectors | `stdlib/std@1/{keccak,sha3_224,sha3_256,sha3_384,sha3_512,shake128,shake256}.hir`, SHA-3 and SHAKE conformance | ziglang/zig `bff490a23889750de7096d9057c0a432efc929a8` | MIT |
| Go standard library | `src/crypto/internal/fips140/sha3/keccakf.go` round structure over the 25 lanes in locals | `stdlib/std@1/keccak.hir` permute | golang/go `ebe2feefc400f813aefdc8a036e66d2619da6649` | BSD-3-Clause (Go) |
| Zig standard library | `lib/std/crypto/chacha20.zig` ChaCha20 and HChaCha20 rounds, the ChaCha20-Poly1305 and XChaCha20-Poly1305 constructions and their seal/open vectors; `poly1305.zig` final reduction | `stdlib/std@1/{chacha_poly,chacha20poly1305,xchacha20poly1305,aead}.hir`, ChaCha20-Poly1305 conformance | ziglang/zig `bff490a23889750de7096d9057c0a432efc929a8` | MIT |
| poly1305-donna | `poly1305-donna-64.h` 44-bit limb multiplication and carry chain | `stdlib/std@1/chacha_poly.hir` Poly1305 | floodyberry/poly1305-donna (2016) | Public domain (MIT where unavailable) |
| Zig standard library | `lib/std/crypto/aes/bitsliced.zig` S-box Boolean circuit; `ghash_polyval.zig` clmulSoft128 carry-less product and reduce; `aes_gcm.zig` AES-256-GCM vectors | `stdlib/std@1/aes_gcm.hir`, AES-GCM conformance | ziglang/zig `bff490a23889750de7096d9057c0a432efc929a8` | MIT |
| Wycheproof | `testvectors_v1/{chacha20_poly1305,xchacha20_poly1305,aes_gcm}_test.json` cases | AEAD Wycheproof conformance | C2SP/wycheproof `7a325932231bd7a724c791c8b0d65ea4d6d2f435` | Apache-2.0 |
| RFC 8439 | ChaCha20-Poly1305 (2.8.2) test vectors | ChaCha20-Poly1305 conformance | RFC 8439 (June 2018) | Simplified BSD (IETF Trust) |
| Go standard library | `src/crypto/internal/fips140/{hmac/hmac.go,hkdf/hkdf.go,pbkdf2/pbkdf2.go}` key padding, keyed-state reuse, HKDF extract/expand and PBKDF2 block loop; `src/crypto/subtle/constant_time.go` ConstantTimeCompare and its test vectors; `src/crypto/hmac/hmac_test.go` FIPS 198 sample; RFC 2202, 4231, 5869, 6070 and 7914 vectors | `stdlib/std@1/{digest,hmac,hkdf,pbkdf2,constant_time}.hir`, HMAC/HKDF/PBKDF2/constant-time conformance and the constant-time trace test | golang/go `ebe2feefc400f813aefdc8a036e66d2619da6649` | BSD-3-Clause (Go) |
| Go standard library | `src/hash/crc32/{crc32.go,crc32_generic.go}` IEEE and Castagnoli simple tables and update, `src/hash/adler32/adler32.go` nmax-batched sums; `crc32_test.go` and `adler32_test.go` golden vectors | `stdlib/std@1/{crc32,crc32c,adler32}.hir`, checksum conformance | golang/go `ebe2feefc400f813aefdc8a036e66d2619da6649` | BSD-3-Clause (Go) |
| cespare/xxhash | `xxhash.go` XXH64 primes, lanes, merge rounds and tail finalization; `xxhash_test.go` TestAll seeded and unseeded vectors | `stdlib/std@1/xxhash64.hir`, xxHash64 conformance | cespare/xxhash `ab37246c889f9db16b606fda1c232d659df9271d` | MIT |
| Go standard library | `src/flag/flag.go` FlagSet/parseOne token splitting, explicit values, terminator and error handling; adapted cases use immutable specs and retain repeated values | `stdlib/std@1/cli.hir`, CLI conformance | golang/go `ebe2feefc400f813aefdc8a036e66d2619da6649` | BSD-3-Clause (Go) |
| Go standard library | `src/log/slog/{record,json_handler,handler}.go` structured records, severity filtering, escaped JSON-lines and single-write convention | `stdlib/std@1/log.hir`, logging conformance and simulated sink fault tests (Hirsel uses existing JSON and explicit sinks) | golang/go `ebe2feefc400f813aefdc8a036e66d2619da6649` | BSD-3-Clause (Go) |
| Go standard library | `src/testing/quick/quick.go` explicit MaxCount case loop | `stdlib/std@1/test.hir` property_cases, test-helper conformance | golang/go `ebe2feefc400f813aefdc8a036e66d2619da6649` | BSD-3-Clause (Go) |
| Go standard library | `src/internal/diff/diff.go` anchored unified diff (Szymanski's unique-line LCS) and its `testdata/*.txt` | `crates/hirsel-driver/src/testing/diff.rs` and `diff-testdata/` | golang/go `ebe2feefc400f813aefdc8a036e66d2619da6649` | BSD-3-Clause (Go) |
| Go standard library | `src/testing/benchmark.go` launch/predictN calibration: a warm-up run, iteration counts grown toward 1.2 times the target time but at most a hundredfold per step, the 1s default benchtime and the `N iterations, X ns/op` line; adapted to powers of ten timed around whole launches | `crates/hirsel-driver/src/testing.rs` calibrate, `hirsel test --bench` | golang/go `ebe2feefc400f813aefdc8a036e66d2619da6649` | BSD-3-Clause (Go) |
| minithesis (design reference only) | the choice-sequence shrinker's pass order and the expected minimal counterexamples of `test_minithesis.py` (`[1001]` list sums, `1, 1000` additive pairs); Hirsel's implementation is written independently and copies no code | `stdlib/std@1/test.hir` property shrinking, property conformance | DRMacIver/minithesis `b20e7e9b4d7156a186e0a7e78a8c79e9235214a9` | not adapted (MPL-2.0 upstream) |
| SlateDB (design reference only) | the writer claim (a fence object at the next log id created only if absent, then the next manifest with the next epoch), the manifest naming the current snapshot and first log object, and the durable watermark behind awaiting a commit; Hirsel's store is written independently over a narrowed `Dir` and copies no code, format or test | `stdlib/std@1/store.hir`, store conformance | slatedb/slatedb `a5c4fd22b36c836a89d8c4f27bf0b7b045e62e32` | not adapted (Apache-2.0 upstream) |
| Rust standard library | `library/core/src/panicking.rs` assert_failed left/right report | `stdlib/std@1/test.hir` assert_eq | rust-lang/rust `f6c1107f8be13e48465e12c0fd26e6a39fe18e4f` | MIT |
| Zig standard library | `lib/std/testing.zig` assertion cleanup semantics | `stdlib/std@1/test.hir` expect_ok and owned Result cleanup conformance | ziglang/zig `bff490a23889750de7096d9057c0a432efc929a8` | MIT |
| Go standard library | `src/unicode/{letter,graphic,digit}.go` range binary search, property meanings and simple scalar conversion | `stdlib/std@1/unicode_properties.hir`, Unicode16 property conformance (tables generated from Unicode16 UCD) | golang/go `ebe2feefc400f813aefdc8a036e66d2619da6649` | BSD-3-Clause (Go) |
| Zig standard library | `lib/std/ascii.zig` charToDigit/radix and ASCII classification boundaries | `stdlib/std@1/ascii.hir` digit_value_radix, shared codec/parser callers and ASCII radix conformance | ziglang/zig `bff490a23889750de7096d9057c0a432efc929a8` | MIT |
| Go standard library | `src/encoding/csv/{reader,writer}.go` quoted-field transitions/fieldNeedsQuotes and `reader_test.go`, `writer_test.go`, `fuzz_test.go` record/quote/error/boundary ideas (Hirsel preserves its existing CSV example dialect) | `stdlib/std@1/csv.hir`, CSV conformance; `examples/csvjson` uses std and its private parser is removed | golang/go `ebe2feefc400f813aefdc8a036e66d2619da6649` | BSD-3-Clause (Go) |
| Go standard library | `src/crypto/internal/fips140/sha256/{sha256.go,sha256block.go}` cross-check of constants, scalar schedule/rounds and big-endian padding/output | `stdlib/std@1/sha256.hir` | golang/go `ebe2feefc400f813aefdc8a036e66d2619da6649` | BSD-3-Clause (Go) |
| Go standard library | `src/compress/flate/{inflate.go,flate_test.go}`, `src/compress/gzip/{gunzip.go,gunzip_test.go}`, `src/compress/zlib/{reader.go,reader_test.go}` block decoding, code completeness rule, member and header checks, TestStreams, TestTruncatedStreams, gunzipTests and zlibTests | `stdlib/std@1/flate.hir`, `conformance/SEM/SEM-3-flate-*`, `crates/hirsel-driver/tests/fixtures/flate-large.hir` | golang/go `ebe2feefc400f813aefdc8a036e66d2619da6649` | BSD-3-Clause (Go) |
| Go standard library | `src/archive/tar/{reader.go,writer.go,strconv.go,format.go,common.go}` header parsing and checksums, octal and base-256 numbers, PAX record parsing and formatting, GNU long names, USTAR prefix splitting, PAX header naming and format choice; `reader_test.go` TestReader and `writer_test.go` TestWriter vectors with their `testdata` archives | `stdlib/std@1/tar.hir`, `conformance/SEM/SEM-3-tar-*` | golang/go `ebe2feefc400f813aefdc8a036e66d2619da6649` | BSD-3-Clause (Go) |
| Go standard library | `src/archive/zip/{reader.go,writer.go,struct.go}` end-of-directory search, ZIP64 locator and end records, base-offset recovery, directory and extra-field parsing, data descriptors, checksum checks, UTF-8 flag, MS-DOS time and the writer layout; `reader_test.go` TestReader vectors with their `testdata` archives | `stdlib/std@1/zip.hir`, `conformance/SEM/SEM-3-zip-*`, `crates/hirsel-driver/tests/fixtures/zip-large.hir` | golang/go `ebe2feefc400f813aefdc8a036e66d2619da6649` | BSD-3-Clause (Go) |
| zlib | `deflate.c` fill_window, longest_match, deflate_fast, deflate_slow, the configuration table and stored-block sizing, and `trees.c` tree construction, length limiting, tree sending and block choice; byte-exact outputs of zlib 1.3 as golden vectors | `stdlib/std@1/flate.hir`, `conformance/SEM/SEM-3-deflate-*`, `crates/hirsel-driver/tests/fixtures/deflate-zlib-native.hir` | madler/zlib `09155eaa2f9270dc4ed1fa13e2b4b2613e6e4851` (v1.3) | Zlib |
| Go standard library | `src/net/http/{cookie.go,cookie_test.go}` ParseCookie, Cookie.String, token and cookie-octet checks, TestParseCookie, writeSetCookiesTests and sanitize vectors | `stdlib/std@1/cookie.hir`, `conformance/SEM/SEM-3-cookie-run.hir` | golang/go `ebe2feefc400f813aefdc8a036e66d2619da6649` | BSD-3-Clause (Go) |
| cookie | `src/lib.rs` `format` test vectors | `conformance/SEM/SEM-3-cookie-run.hir` | cookie 0.18.2 crate release | MIT |
| Go standard library | `src/encoding/base64/{base64.go,base64_test.go}` quanta, strict unused-bit checks and four-mode reference pairs | `stdlib/std@1/base64.hir`, base64 conformance | golang/go `ebe2feefc400f813aefdc8a036e66d2619da6649` | BSD-3-Clause (Go) |
| Go standard library | `src/encoding/hex/{hex.go,hex_test.go}` nibble codec and vectors | `stdlib/std@1/hex.hir`, hex conformance | golang/go `ebe2feefc400f813aefdc8a036e66d2619da6649` | BSD-3-Clause (Go) |
| Go standard library | `src/path/match.go`, `src/path/match_test.go` slash grammar, scalar classes and matching vectors | `stdlib/std@1/glob.hir`, `conformance/SEM/SEM-3-glob-*.hir` | golang/go `ebe2feefc400f813aefdc8a036e66d2619da6649` | BSD-3-Clause (Go) |
| Zig standard library | `lib/std/Io/Dir.zig` iterate/statFile/createDir/createDirPath/rename/deleteDir contracts | confined filesystem lifecycle providers, `stdlib/std@1/file.hir` | ziglang/zig `bff490a23889750de7096d9057c0a432efc929a8` | MIT |
| Rust standard library | `library/std/src/fs.rs` directory/metadata/rename contracts; `library/std/src/fs/tests.rs` mkdir, recursive mkdir, rename-over-open-file, nonempty-directory and dangling-symlink vectors | filesystem conformance and provider/replay tests | rust-lang/rust `f6c1107f8be13e48465e12c0fd26e6a39fe18e4f` | MIT |
| Rust standard library | `library/std/src/collections/hash/map.rs` length, lookup/removal and ownership contract vectors | Map lifecycle kernels and conformance | rust-lang/rust `f6c1107f8be13e48465e12c0fd26e6a39fe18e4f` | MIT |
| Rust standard library | `library/std/src/collections/hash/set.rs` length and boolean removal contract vectors | Set lifecycle kernels and conformance | same | MIT |
| Zig standard library | `lib/std/array_hash_map.zig` ordered removal and private-index reconstruction | `crates/hirsel-rc/src/library/map.rs`, map/set lifecycle conformance | ziglang/zig `bff490a23889750de7096d9057c0a432efc929a8` | MIT |
| Rust standard library | `library/alloc/src/collections/btree/map.rs` ordered lookup/removal and ownership vectors | SortedMap access kernels and conformance, retaining Hirsel's sequence representation | rust-lang/rust `f6c1107f8be13e48465e12c0fd26e6a39fe18e4f` | MIT |
| Rust standard library | `library/alloc/src/collections/vec_deque/mod.rs` endpoint and move-ownership vectors | `stdlib/std@1/deque.hir`, deque conformance | rust-lang/rust `f6c1107f8be13e48465e12c0fd26e6a39fe18e4f` | MIT |
| Zig standard library | `lib/std/deque.zig` endpoint and empty/reuse vectors | `stdlib/std@1/deque.hir`, deque conformance | ziglang/zig `bff490a23889750de7096d9057c0a432efc929a8` | MIT |
| Rust standard library | `library/alloc/src/collections/binary_heap/mod.rs` priority and move-ownership vectors | `stdlib/std@1/priority_queue.hir`, priority-queue conformance | rust-lang/rust `f6c1107f8be13e48465e12c0fd26e6a39fe18e4f` | MIT |
| Zig standard library | `lib/std/priority_queue.zig` sift and ordering vectors | `stdlib/std@1/{priority_queue,heap_ops}.hir`, priority-queue conformance | ziglang/zig `bff490a23889750de7096d9057c0a432efc929a8` | MIT |
| lexopt | command-line parser dependency | `crates/hirsel-cli-args` | blyxxyz/lexopt 0.3.2 | MIT |
| gimli | DWARF writer dependency | `crates/hirsel-cranelift/src/dwarf.rs` | gimli-rs/gimli 0.33.0 crate release | MIT |
| Cranelift | code generator dependency (`cranelift-codegen`, `-frontend`, `-module`, `-object`) | `crates/hirsel-cranelift` | bytecodealliance/wasmtime cranelift 0.136.3 crate releases | Apache-2.0 WITH LLVM-exception |
| Zig standard library | `lib/std/Uri.zig`, `fmt.zig` parseIntWithSign | `stdlib/std@1/uri.hir`, `crates/hirsel-driver/tests/it-stdlib/stdlib_uri.rs` | ziglang/zig `bff490a23889750de7096d9057c0a432efc929a8` | MIT |
| Zig standard library | `lib/std/Io/net/HostName.zig` validate, fromUri | `stdlib/std@1/net.hir`, `crates/hirsel-run-config` grant validation, their tests | same | MIT |
| Zig standard library | `lib/std/http/Client.zig`, `HeaderIterator.zig`, `ChunkParser.zig`, `http.zig`, `ascii.zig` | `stdlib/std@1/http_wire.hir`, `stdlib/std@1/http.hir`, `crates/hirsel-driver/tests/it-stdlib/stdlib_http_wire.rs`, `crates/hirsel-driver/tests/it/stdlib_http.rs` | same | MIT |
| Zig standard library | `lib/std/ascii.zig` character classes, whitespace and case-conversion vectors | `stdlib/std@1/ascii.hir`, `conformance/SEM/SEM-3-ascii-run.hir` | same | MIT |
| Zig standard library | `lib/std/unicode.zig` utf8Decode2/3/4 | `stdlib/std@1/graphemes.hir` | same | MIT |
| Zig standard library | `lib/std/fs/path.zig`, `lib/std/time/epoch.zig` | std@1 path sources and tests; epoch.zig RFC 3339 test vectors | same | MIT |
| jiff | `crates/jiff/src/civil/{date,time,datetime}.rs` validation, month arithmetic with day clamping, ISO week dates and the expanded six-digit year form; `crates/jiff/src/fmt/strtime/{printer,parse}.rs` directive semantics (%Y sign padding, %f trimming, %z seconds, case-insensitive names, leap second 60 read as 59) and their test vectors | `stdlib/std@1/time_civil.hir`, `date.hir`, `time_of_day.hir`, `date_time.hir`, `time_format.hir`, civil and strftime conformance | BurntSushi/jiff `fff640d47f72ca6d3a122678315bbd76488306fe` | MIT |
| jiff (time zones) | `crates/jiff-core/src/tz/tzif/{parser,query}.rs` TZif validation, transition clamping and civil gap/fold ranges; `crates/jiff-core/src/tz/posix.rs` POSIX TZ parsing, DST windows, clamping and the permanent-DST case; `crates/jiff/src/tz/timezone.rs` disambiguation and its ambiguity and to-datetime test vectors; TZif test files `crates/jiff/src/tz/testdata/{america-new-york,america-sitka,america-st-johns,antarctica-troll,australia-tasmania,europe-dublin,pacific-honolulu,utc}.tzif` (tz database data, public domain) | `stdlib/std@1/tzif.hir`, `time_zone.hir`, `packages/fixtures/tzif@1`, `conformance/SEM/SEM-1-time-zone-*` | BurntSushi/jiff `fff640d47f72ca6d3a122678315bbd76488306fe` | MIT |
| Go standard library (time zones) | `src/time/zoneinfo_test.go` TestLoadLocationFromTZDataSlim and TestTzset vectors; slim TZif files `src/time/testdata/{2020b_Europe_Berlin,2021a_America_Nuuk,2021a_Asia_Gaza,2021a_Europe_Dublin}` (tz database data, public domain) | `packages/fixtures/tzif@1/zones.hir`, `conformance/SEM/SEM-1-time-zone-slim-*`, `SEM-1-time-zone-posix-rules-run.hir` | golang/go `ebe2feefc400f813aefdc8a036e66d2619da6649` | BSD-3-Clause (Go) |
| Howard Hinnant date algorithms | `days_from_civil` and `civil_from_days` from "chrono-Compatible Low-Level Date Algorithms" | `stdlib/std@1/time_civil.hir` | howardhinnant.github.io/date_algorithms.html | Public domain (author's dedication) |
| Go standard library | `src/time/time_test.go` isoWeekTests, yearDayTests and the January 4 week-1 invariant | `conformance/SEM/SEM-1-time-date-calendar-run.hir` | golang/go `ebe2feefc400f813aefdc8a036e66d2619da6649` | BSD-3-Clause (Go) |
| Zig standard library | `lib/std/mem.zig` eqlBytes bounded overlapping loads and eql unequal-byte vectors | `crates/hirsel-llvm/src/compare.rs`, `conformance/SEM/SEM-5-short-byte-equality-run.hir` | same | MIT |
| Zig standard library | `lib/std/heap/ArenaAllocator.zig` alloc | `crates/hirsel-rt/src/region.rs` | same | MIT |
| Zig standard library | `lib/std/Io/Threaded.zig` netLookup, `lib/std/Io/net.zig` ListenOptions | `crates/hirsel-rt/src/net_host.rs` | same | MIT |
| Zig standard library | `lib/std/json/dynamic.zig` | std@1 JSON ordered-object duplicates | same | MIT |
| Zig standard library | `lib/std/testing.zig`, `lib/std/math.zig` | std@1 test helpers and their vectors | same | MIT |
| Rust standard library | `library/std/src/sys/random/linux.rs` getrandom fill loop | `crates/hirsel-rt/src/io.rs` RandomSource::draw | rust-lang/rust `f6c1107f8be13e48465e12c0fd26e6a39fe18e4f` | MIT |
| Rust standard library | `library/core/src/iter/adapters/{filter,take,enumerate,zip}.rs`, `library/coretests/tests/iter/adapters/` next, ownership, short-input and limit vectors | `stdlib/std@1/iter.hir`, `conformance/CLO/CLO-4-std-iter-*` | rust-lang/rust `f6c1107f8be13e48465e12c0fd26e6a39fe18e4f` | MIT |
| Zig standard library | `lib/std/Io.zig` Timeout.toDeadline, Clock.Timestamp.fromNow, Timeout.sleep | `stdlib/std@1/time.hir` deadline and sleep | ziglang/zig `bff490a23889750de7096d9057c0a432efc929a8` | MIT |
| Zig standard library | `lib/std/Io/Threaded.zig` sleepPosix absolute-deadline clock_nanosleep loop | `crates/hirsel-rt/src/io_real.rs` sleep_until | ziglang/zig `bff490a23889750de7096d9057c0a432efc929a8` | MIT |
| Zig standard library | `lib/std/Random/Xoshiro256.zig`, `lib/std/Random/SplitMix64.zig`, `lib/std/Random.zig` int and uintLessThan, their sequence, fill and bounded-sampling tests | `stdlib/std@1/prng.hir`, `stdlib/std@1/sample.hir`, `stdlib/std@1/random.hir`, `conformance/SEM/SEM-1-prng-xoshiro-run.hir` | ziglang/zig `bff490a23889750de7096d9057c0a432efc929a8` | MIT |
| Rust standard library | `library/alloc/src/io/buffered/{bufreader.rs,bufreader/buffer.rs}`, `library/alloc/src/io/{read,buf_read}.rs`, `library/alloctests/tests/io/buffered.rs` refill/consume, exact-read, delimiter, line and short-read vectors | `stdlib/std@1/bufio.hir`, buffered reader conformance | rust-lang/rust `f6c1107f8be13e48465e12c0fd26e6a39fe18e4f` | MIT |
| Rust standard library | `library/alloc/src/io/{read,buf_read}.rs`, `library/alloctests/tests/io/mod.rs` read-to-end and delimiter/EOF vectors | `stdlib/std@1/input.hir`, host input tests | rust-lang/rust `f6c1107f8be13e48465e12c0fd26e6a39fe18e4f` | MIT |
| Rust standard library | `library/std/src/fs/tests.rs` incremental read/absolute seek vectors; `Read`/`Write` interrupted-call loops | file stream providers and conformance/host replay tests | rust-lang/rust `f6c1107f8be13e48465e12c0fd26e6a39fe18e4f` | MIT |
| Rust standard library | `library/std/src/net/tcp/tests.rs` | `crates/hirsel-interp/src/library/net_tests.rs`, `crates/hirsel-rt/tests/it/net_script.rs` | rust-lang/rust `f6c1107f8be13e48465e12c0fd26e6a39fe18e4f` | MIT |
| Rust standard library | `library/core/src/net/{parser,ip_addr,socket_addr}.rs`, `library/coretests/tests/net/` | `stdlib/std@1/net.hir`, `crates/hirsel-run-config` grant tests, driver tests | same | MIT |
| Rust standard library | `library/std/src/sys/net/connection/socket/unix.rs`, `net/socket_addr.rs`, `sys/pal/unix/mod.rs` | `crates/hirsel-rt/src/net_host.rs` | same | MIT |
| Rust standard library | `library/std/src/sys/net/connection/socket/mod.rs` `TcpListener::bind` sets `SO_REUSEADDR` before binding on Unix; `library/std/src/net/tcp/tests.rs` `double_bind` control | `crates/hirsel-rt/src/net_host.rs`, `native-tests/library/net-reuseaddr.hir`, `conformance/CAP/CAP-1-net-reuseaddr-run.hir` | rust-lang/rust `f6c1107f8be13e48465e12c0fd26e6a39fe18e4f` | MIT |
| Rust standard library | `library/std/src/process.rs` (`Command`, `Output`, `ExitStatus`), `library/std/src/sys/process/unix/common.rs` `read_output` poll loop, `library/std/src/process/tests.rs` smoke, exit/signal status, stdin/stdout, fail-to-start, output and environment vectors | `crates/hirsel-rt/src/process.rs`, `crates/hirsel-rt/tests/it/process.rs`, `crates/hirsel-interp/src/library/process.rs` | rust-lang/rust `f6c1107f8be13e48465e12c0fd26e6a39fe18e4f` | MIT |
| Zig standard library | `lib/std/process.zig` `run` per-stream `stdout_limit`/`stderr_limit` (`StreamTooLong`, child killed) | `proc.run` `max_output` and `OutputLimit()` | ziglang/zig `bff490a23889750de7096d9057c0a432efc929a8` | MIT |
| Rust standard library | `library/std/src/os/unix/net/addr.rs` `sockaddr_un`, `listener.rs` bind and `stream.rs` connect; `tests.rs` basic, long_path and test_read_timeout vectors | `crates/hirsel-rt/src/net_host.rs` Unix stream sockets, `crates/hirsel-rt/tests/it/net.rs` | rust-lang/rust `f6c1107f8be13e48465e12c0fd26e6a39fe18e4f` | MIT |
| Go standard library | `src/net/unixsock_posix.go` UnixListener close unlinking the socket file it created, extended with a device/inode check | `crates/hirsel-rt/src/net_host.rs` | golang/go `ebe2feefc400f813aefdc8a036e66d2619da6649` | BSD-3-Clause (Go) |
| Rust standard library | Unix socket nonblocking, poll and SO_ERROR conventions; TCP nonblocking test vectors | `crates/hirsel-rt/src/net_host/cooperative.rs`, `crates/hirsel-rt/tests/it/net.rs` | same | MIT |
| Rust standard library | `library/coretests/tests/num/{cast,floats}.rs`, `library/core/src/num/{f32,f64}.rs` | integer-to-float boundary tests | same | MIT |
| Rust standard library | `library/core/src/num/{int_macros,uint_macros}.rs`, integer-policy boundary tests | `tools/generate-integer-ops.py`, std integer policy modules and conformance vectors | rust-lang/rust `f6c1107f8be13e48465e12c0fd26e6a39fe18e4f` | MIT |
| Rust standard library | `library/alloc/src/rcs/arc.rs` `Arc::make_mut` isolation examples | heapcheck persistent snapshots and fork-isolation controls in `crates/hirsel-heapcheck/src/{snapshot,ledger}.rs` | same | MIT |
| Rust standard library | `str::split_ascii_whitespace`, slice split/filter algorithm, string examples and ASCII whitespace vectors | `stdlib/std@1/string.hir`, `conformance/SEM/SEM-3-split-ascii-whitespace-run.hir` | same | MIT |
| Rust standard library | `str` `Ord` (byte-lexicographic order) and `alloctests/tests/str.rs` `test_le` vectors | `stdlib/std@1/string.hir` `compare`, `conformance/SEM/SEM-3-string-compare-run.hir` | same | MIT |
| Rust standard library | `str::split_once` and `alloctests/tests/str.rs` `test_split_once` vectors | `stdlib/std@1/string.hir` `split_once`, `conformance/SEM/SEM-3-split-once-run.hir` | same | MIT |
| Rust standard library | `library/core/src/time.rs` | std@1 Duration text and tests | same | MIT |
| Rust standard library | `library/core/src/fmt/num.rs`, `library/coretests/tests/fmt/num.rs` | `stdlib/std@1/decimal.hir`, `conformance/SEM/SEM-1-format-into-appends-run.hir` | same | MIT |
| Rust standard library | integer Display buffer formatting and integer-limit vectors | std@1 integer `to_string`/`format_into` share the decimal appender; `crates/hirsel-pipeline-tests/tests/rc/rc_append_into.rs` checks consumers that only select `to_string` | same | MIT |
| Rust standard library | `library/alloctests/tests/str.rs` `test_is_utf8` accepted boundary vectors | `conformance/SER/SER-2-variable-flat-run.hir` | rust-lang/rust `f6c1107f8be13e48465e12c0fd26e6a39fe18e4f` | MIT |
| unicode-segmentation | `src/grapheme.rs`, `src/tables.rs`, `tests/test.rs` | `stdlib/std@1/graphemes.hir` and its tests | unicode-rs/unicode-segmentation `ff47b3187adddb14813e6fb2cfc1a079f58cfddc` | MIT |
| Rust standard library | `library/alloc/src/str.rs` Final_Sigma context rule and `library/alloctests/tests/str.rs` full casing/casefold vectors | `stdlib/std@1/string_case.hir`, Unicode casing/folding conformance | rust-lang/rust `f6c1107f8be13e48465e12c0fd26e6a39fe18e4f` | MIT |
| Unicode Character Database 16.0.0 | data files and generated tables | `stdlib/unicode16/`, `stdlib/std@1/{unicode_tables,unicode_compatibility_tables,string_case_tables,grapheme_tables,unicode_property_tables,display_width_tables}.hir` | Unicode 16.0.0 | Unicode, `stdlib/unicode16/LICENSE.txt` |
| serde_json | `src/{de,read,ser}.rs`, `tests/test.rs` | std@1 JSON parsing, escaping and writing | serde-rs/json `20572a6d066958542d79cdd6cf1a7cd9f0c8e1f8` | MIT |
| JSONTestSuite | `test_parsing/` | std@1 JSON corpus vectors | nst/JSONTestSuite `1ef36fa01286573e846ac449e8683f8833c5b26a` | MIT |
| saphyr | `parser/src/{scanner,parser,char_traits}.rs` (scanner, simple keys, indentation, block and flow scalars, event state machine); `saphyr/tests/basic.rs`, `parser/tests/{issues,basic}.rs` vectors | `stdlib/std@1/yaml.hir`, `conformance/SEM/SEM-3-yaml-*-run.hir` | saphyr-rs/saphyr `b8205d5f60ea44f757f7d075fe4f3704bbe5dccc` (v0.2.2) | MIT |
| go-yaml v3 | `decode_test.go` mergeTests, mergeTestsNested and merge rejection vectors | `conformance/SEM/SEM-3-yaml-merge-*-run.hir` | go-yaml/yaml `944c86a7d29391925ed6ac33bee98a0516f1287a` (v3.0.1) | Apache-2.0 |
| yaml-test-suite | every case of the data branch (`in.yaml`, `in.json`, `test.event`, `error`) | `tools/generate-yaml-test.py`, `conformance/SEM/SEM-3-yaml-test-*-run.hir`, `packages/fixtures/yaml_test@1/harness.hir`, `crates/hirsel-driver/tests/fixtures/yaml-tagged-invoice.hir` | yaml/yaml-test-suite `6e6c296ae9c9d2d5c4134b4b64d01b29ac19ff6f` (data-2022-01-17) | MIT |
| toml-test | `tests/files-toml-1.0.0` valid and invalid cases with their tagged JSON | `tools/generate-toml-test.py`, `conformance/SEM/SEM-3-toml-test-*-run.hir`, `packages/fixtures/toml_test@1/harness.hir` | toml-lang/toml-test `ff49d109861c1ad25af53f687f2aef19ab650600` | MIT |
| toml_edit | `tests/testsuite/edit.rs` insertion, removal, array-of-tables and key representation cases; decor ownership and key/string representation choices | `stdlib/std@1/toml.hir`, `conformance/SEM/SEM-3-toml-edit-*-run.hir` | toml-rs/toml `ceac6e4412420d5a5c19e4f7293ad005cf1a31b2` | MIT |
| mimalloc | `src/{alloc,free,page}.c`, `include/mimalloc/types.h`, `test/test-stress.c` | `crates/hirsel-rt/src/alloc.rs` | microsoft/mimalloc `31d034d94cdb8e22f7d7ed55967f581a2d6e831d` | MIT |
| httparse | `src/lib.rs` (request, response, headers, chunk parser and test vectors) | `stdlib/std@1/http_wire.hir`, `crates/hirsel-driver/tests/it-stdlib/stdlib_http_wire.rs` | seanmonstar/httparse `a0fa552e4e0fa9a5914a736ef3a556748fef709b` | MIT |
| rustls | `tests/api.rs` vectors (client and server close_notify, server ALPN, client auth, SNI), `test-ca/rsa-2048/{ca.cert,end.fullchain,end.key}` | `crates/hirsel-rt/tests/it/tls.rs`, `crates/hirsel-rt/tests/it/tls_server.rs`, `crates/hirsel-rt/tests/it/tls-data/` | rustls/rustls `f04804fa13341a9b1dc02b5d1dd8a746f2c4b7fb` (v/0.23.46) | MIT |
| ureq | `src/config.rs` defaults | `stdlib/std@1/http.hir` | algesten/ureq `0ebb046e` | MIT |
| Go standard library | `src/regexp/syntax/{parse,compile,prog}.go`, `src/regexp/exec.go` ordered Thompson/Pike compilation and matching | `stdlib/std@1/regex.hir`, regex conformance | golang/go `ebe2feefc400f813aefdc8a036e66d2619da6649` | BSD-3-Clause (Go) |
| RE2 | `regexp/testdata/re2-search.txt` bundled with Go: 96 portable ordered partial-match vectors | `conformance/SEM/SEM-3-regex-go-*-run.hir` | fixture in golang/go `ebe2feefc400f813aefdc8a036e66d2619da6649` | BSD-3-Clause (RE2) |
| The Computer Language Benchmarks Game | binarytrees C gcc #2 and #3 | `bench/native/c/binarytrees-arena.h` | benchmarksgame-team.pages.debian.net | BSD-3-Clause |
| Valgrind | `include/valgrind.h`, `memcheck/memcheck.h`, vendored unchanged | `third-party/valgrind/`; their amd64 client request sequence and request codes transcribed in `crates/hirsel-rt/src/memcheck.rs` | Valgrind 3.22.0 (Ubuntu `1:3.22.0-0ubuntu3`) | Valgrind header licence (BSD-style, these two files only) |
| postcard | `de::test_alloc::{vec,zst_vec}`, `de::tests::{de_str,enums}` vectors adapted to SER-2 lengths/tags | `conformance/SER/SER-2-*` | jamesmunns/postcard `872b9082589ebd1384a1b95ca232b1727bcddd29` | MIT |
| bincode | binary serde format dependency for the std precheck file | `crates/hirsel-engine/src/queries/precheck.rs` | bincode 1.3.3 crate release | MIT |
| bincode | `tests/{basic_types,derive}.rs` scalar, UTF-8 and enum-boundary controls adapted to SER-2 | `conformance/SER/SER-2-*` | bincode 2.0.1 crate release | MIT |
| RFC 8949 | Appendix A data examples and sections 4.2, 5.3.1, 5.6 deterministic/validity cases adapted to SER-2 | `conformance/SER/SER-2-*` | RFC 8949 (December 2020) | Simplified BSD (IETF Trust) |
| Go standard library | `src/internal/strconv/{atoi,itoa}.go` cutoff radix loops and radix vectors | `stdlib/std@1/integer.hir`, integer radix conformance | golang/go `ebe2feefc400f813aefdc8a036e66d2619da6649` | BSD-3-Clause (Go) |
| Go standard library | `src/math/{floor,sqrt}.go`, `src/math/all_test.go`, IEEE rounding and square-root vectors | `tools/generate-float-math.py`, std f32/f64 math sources and conformance | golang/go `ebe2feefc400f813aefdc8a036e66d2619da6649` | BSD-3-Clause (Go); Sun Microsystems permission below for sqrt |
| Rust libm crate | `libm/src/math/` exp, exp2, expm1, log, log2, log10, log1p, pow, cbrt, hypot, sin, cos, tan, asin, acos, atan, atan2, sinh, cosh, tanh, asinh, acosh, atanh and their `f` forms, kernels, rem_pio2, rem_pio2_large, generic copysign and scalbn; unit-test vectors | `stdlib/std@1/f{32,64}_{exp,power,trig,hyperbolic}.hir`, `stdlib/std@1/libm.hir`, `tools/generate-float-math.py`, `conformance/SEM/SEM-2-std-float-*` | rust-lang/compiler-builtins `f6c1107f8be13e48465e12c0fd26e6a39fe18e4f` | MIT; musl and FreeBSD msun notices below |
| libm-test | `libm-test/src/generate/case_list.rs` MPFR-checked vectors, `src/precision.rs` ULP bounds | `conformance/SEM/SEM-2-std-float-libm-mpfr-run.hir`, `docs/spec/std-library.md` | rust-lang/compiler-builtins `f6c1107f8be13e48465e12c0fd26e6a39fe18e4f` | MIT |
| compiler-builtins | `compiler-builtins/src/float/pow.rs` (`__powidf2`, `__powisf2`) | `tools/generate-float-math.py` powi | rust-lang/compiler-builtins `f6c1107f8be13e48465e12c0fd26e6a39fe18e4f` | MIT |
| Go standard library | `src/os/exec/exec_test.go` TestWaitInterrupt (graceful interrupt, then kill after a delay), TestDedupEnvEcho, TestEnvNULCharacter and TestIgnorePipeErrorOnSuccess (an upstream stage ended by a downstream one that stopped reading); Hirsel refuses a repeated or NUL-bearing variable as invalid input where Go deduplicates or fails at start, and reports the upstream stage's `Signaled(13)` where Go ignores the pipe error | host process tests in `crates/hirsel-rt/tests/it/process.rs` | golang/go `ebe2feefc400f813aefdc8a036e66d2619da6649` | BSD-3-Clause (Go) |
| num-bigint | `src/biguint/{addition,subtraction,multiplication,division,shift,power}.rs` magnitude kernels (Karatsuba step of mac3 and its 32-limb threshold, Knuth D div_rem_core with sub_mul_digit_same_len), the Stein gcd of `src/biguint.rs`; `tests/consts` SUM_TRIPLES, MUL_TRIPLES, DIV_REM_QUADRUPLES and `tests/{bigint,biguint}.rs` conversion and big-division cases | `stdlib/std@1/bigint.hir`, `conformance/SEM/SEM-1-bigint-*` | rust-num/num-bigint `41f0b035d12971c5c7f7501917e31f0f6439c26c` | MIT |
| Go standard library | `src/math/big/{natconv,int}.go` chunked radix conversion and two's-complement bitwise identities; `int_test.go` sumZZ, prodZZ, divisionSignsTests, quoTests, gcdTests, lshTests, rshTests, bitLenTests, expTests, notTests, bitwiseTests and `natconv_test.go` strTests | `stdlib/std@1/bigint.hir`, `conformance/SEM/SEM-1-bigint-go-run.hir` | golang/go `ebe2feefc400f813aefdc8a036e66d2619da6649` | BSD-3-Clause (Go) |
| rust_decimal | `src/ops/{add,mul,div}.rs` scale rules and `src/decimal.rs` rounding strategies, without the 96-bit limit; `tests/decimal_tests/{arithmetic,rounding,scale}.rs` add, subtract, multiply, divide, rounding, normalize and rescale vectors with exact expectations | `stdlib/std@1/bigdecimal.hir`, `conformance/SEM/SEM-1-decimal-*` | paupino/rust-decimal `b23fb077e301e77cb0ac36cc1ac4df1fba6c45d5` | MIT |
| FoundationDB | tuple layer `design/tuple.md` and `bindings/python/fdb/tuple.py`: the order-preserving byte-string escape, sign-bit integer transform, IEEE total-order float transform and `strinc`; the typecode table's test cases (`foo\x00bar`, `F\u00d4O\u0000bar`, `-5551212`, `-42f`) without their type codes | `stdlib/std@1/keys.hir`, `conformance/SEM/SEM-3-keys-*`, `docs/spec/std-library.md` | apple/foundationdb `487620d5aa41152b1f034f4e932005a87eb2947f` | Apache-2.0 (text below, without the LLVM exception) |
| rustc-hash | `src/lib.rs` FxHasher: the rotate-xor-multiply word step and its 64-bit seed, with the 8/4/1-byte tail split | `crates/hirsel-intern/src/lib.rs` `IdHash` (session-id maps and spelling indexes) | rust-lang/rustc-hash `v1.1.0` | MIT |
| Rust standard library | `library/core/src/num/imp/dec2flt/{lemire,table}.rs` Eisel-Lemire algorithm and 128-bit power-of-five table; `library/coretests/tests/num/dec2flt/lemire.rs` halfway, shifted-exponent and boundary vectors | `stdlib/std@1/float_lemire.hir`, `stdlib/std@1/float_decimal.hir` fast path, `conformance/SEM/SEM-2-float-parse-lemire-vectors.hir` | rust-lang/rust `f6c1107f8be13e48465e12c0fd26e6a39fe18e4f` | MIT |
| Datastar | The v1.0.4 browser bundle, vendored unchanged except for its trailing `sourceMappingURL` line (upstream sha256 `727844adfc825ee651fb93c544a2a739986f9a21820a94524b35f0cac470cf91`) | `examples/tasks/static/datastar.js` | starfederation/datastar `v1.0.4` | MIT |
| Datastar | `sdk/ADR.md` server SDK specification and the Go and Python SDKs' API shape; `sdk/tests/golden` SDK conformance cases, inputs and outputs verbatim, with the Go test runner's event comparison | `packages/datastar@1/`, `conformance/PROG/PROG-1-datastar-sdk-golden.hir` | starfederation/datastar `1efcdc3cb336ec3e9139491604e770e0329657bc` | MIT |
| Zig compiler | `src/libs/glibc.zig` stub shared objects: `.symver` placeholder assembly per versioned symbol, linked as a DSO with the library's SONAME and a version script; `lib/libc/glibc/abilists` read at GLIBC_2.28 to cross-check the stubs | `tools/buck2/native-toolchain/link_kit.py` (the link kit) | ziglang/zig `bff490a23889750de7096d9057c0a432efc929a8` | MIT |
| LLVM | `compiler-rt/lib/builtins/crt{begin,end}.c` and `libunwind/` compiled into the link kit and linked into every executable | link kit (`tools/buck2/native-toolchain/link_kit.py`) | llvm/llvm-project `llvmorg-21.1.8` | Apache-2.0 WITH LLVM-exception (license text below) |
| GNU C Library | glibc 2.28's `Scrt1.o`, `crti.o`, `crtn.o` and `libc_nonshared.a` (Debian 2.28-10+deb10u1) linked into every executable; its libraries' public dynamic symbol tables generate the link kit's stubs | link kit (`tools/buck2/native-toolchain/link_kit.py`) | glibc 2.28 (Debian 2.28-10+deb10u1) | LGPL-2.1-or-later, with glibc's exception granting unlimited permission to link these compiled startup and nonshared files into other programs |
| protobuf-go | `encoding/protowire/wire.go` varint, zigzag, fixed and tag rules and `wire_test.go` vectors; `encoding/protojson` proto3 JSON number, string, bytes, Duration, Timestamp, FieldMask and Any rules | `packages/protobuf@1/{wire,json,tests}.hir`, `packages/protobuf@1/protoc/gen_wkt.hir` | protocolbuffers/protobuf-go `e9ee2a76ab8eef359299e31736cf5cbdbd4d7048` | BSD-3-Clause (Go) |
| prost | `prost/src/encoding.rs` length-delimited merging, packed/unpacked acceptance and depth limit; `prost-build` naming of nested types and oneofs | `packages/protobuf@1/wire.hir`, `packages/protobuf@1/protoc/gen_{model,emit}.hir` | tokio-rs/prost `aed74ad0e844959ee6948f29c4e9cff0f278e89e` | Apache-2.0 (text below, without the LLVM exception) |
| Protocol Buffers | well-known type `.proto` files (embedded), `conformance/conformance.proto`, `src/google/protobuf/test_messages_proto3.proto` and the conformance test runner (built outside the tree) | `packages/protobuf@1/protoc/gen_wkt.hir`, `packages/protobuf@1/wkt.hir`, `packages/protobuf@1/conformance/` | protocolbuffers/protobuf `1d8382d5bdbd06af4e60e4d4e4d53241bfa5dbe8` | BSD-3-Clause (Protocol Buffers) |
| grpc-go | `codes/codes.go`, `internal/transport/http_util.go` status, grpc-message percent-encoding and grpc-timeout rules, `internal/grpcutil/encode_duration.go` and their tests; `interop/test_utils.go` interop cases and server behaviour | `packages/grpc@1/`, `packages/grpc@1/interop/main.hir` | grpc/grpc-go `679b224109d3373eb0e4d80b6e41aafc8d295b6d` | Apache-2.0 (text below, without the LLVM exception) |
| tonic | `interop/proto/grpc/testing/{test,messages,empty}.proto` (gRPC authors) | `packages/grpc@1/interop/proto/`, generated `packages/grpc@1/interop/testing.hir` | hyperium/tonic `cfe80f9d1e5e8e660cd2c1cd3b0a7579df37ba25` | Apache-2.0 (protos, text below); MIT (tonic) |
| gRPC | `examples/protos/helloworld.proto`, with a streaming method added | `examples/grpc-greeter/` | grpc/grpc examples | Apache-2.0 (text below, without the LLVM exception) |
| CPython | `Lib/shlex.py` shlex.read_token in POSIX mode with whitespace_split, quote and join; `Lib/test/test_shlex.py` posix_data and data rows, testQuote, testForceQuote (without force), testJoin and testJoinRoundtrip. Changes from shlex.py: rewritten as a byte scanner of POSIX mode with whitespace_split only (no punctuation_chars, non-POSIX mode or stream input), and an open quote or final backslash is an error value carrying its byte offset where Python raises ValueError | `stdlib/std@1/shlex.hir`, `conformance/SEM/SEM-3-shlex-*` | python/cpython `036d309cf542986a39eeba5ca805b2ec7619731d` | PSF-2.0 |
| google/uuid | `uuid.go` Parse's four spellings, Version and Variant; `uuid_test.go` tests table and TestFromBytes | `stdlib/std@1/uuid.hir`, `conformance/SEM/SEM-3-uuid-*` | google/uuid `2d3c2a9cc518326daf99a383f07c4d3c44317e4d` | BSD-3-Clause (google/uuid) |
| uuid | `src/builder.rs` from_random_bytes and from_unix_timestamp_millis layouts; `src/parser.rs` test_parse_valid and test_parse_invalid inputs; `src/v7.rs` test_new and test_sorting | `stdlib/std@1/uuid.hir`, `conformance/SEM/SEM-3-uuid-*` | uuid-rs/uuid `276c23628a669ca9fca637f3326c21b114e88049` | MIT |

Where an upstream offers MIT or Apache-2.0, the MIT option is used.

## Copyright notices

- postcard: Copyright (c) 2019 Anthony James Munns
- bincode: Copyright (c) 2014 Ty Overby
- RFC 8878: Copyright (c) 2021 IETF Trust and the persons identified as the document authors
- RFC 8949: Copyright (c) 2020 IETF Trust and the persons identified as the document authors
- RFC 8439: Copyright (c) 2018 IETF Trust and the persons identified as the document authors
- Wycheproof: Copyright 2016-2026 The Wycheproof Authors
- lexopt: Copyright (c) 2021 Jan Verbeek
- Cranelift: Copyright (c) the Bytecode Alliance and Cranelift contributors
- Zig standard library: Copyright (c) Zig contributors
- LLVM: Copyright (c) LLVM Project contributors
- GNU C Library: Copyright (C) Free Software Foundation, Inc.
- jiff: Copyright (c) 2015 Andrew Gallant
- Rust standard library: Copyright (c) The Rust Project Contributors
- unicode-segmentation: Copyright (c) 2015 The Rust Project Developers
- serde_json: Copyright (c) Serde project contributors
- JSONTestSuite: Copyright (c) 2016 Nicolas Seriot
- saphyr: Copyright (c) 2015 Chen Yuheng; Copyright (c) 2023 Ethiraric
- go-yaml v3: Copyright 2011-2016 Canonical Ltd.
- yaml-test-suite: Copyright (c) 2016-2020 Ingy döt Net
- toml-test: Copyright (c) 2018 TOML authors
- toml_edit: Copyright (c) Individual contributors
- mimalloc: Copyright (c) 2018-2025 Microsoft Corporation, Daan Leijen
- httparse: Copyright (c) 2015-2026 Sean McArthur
- rustls: Copyright (c) 2016 Joseph Birr-Pixton
- ureq: Copyright (c) 2019 Martin Algesten
- cookie: Copyright (c) 2017 Sergio Benitez, Copyright (c) 2014 Alex Crichton
- The Computer Language Benchmarks Game: Copyright (c) 2004-2008 Brent Fulgham,
  2005-2025 Isaac Gouy. The binarytrees programs are by Jeremy Zerfas, based on
  the C++ program by Jon Harrop, Alex Mizrahi and Bruno Coutinho.
- Valgrind `valgrind.h` and `memcheck.h`: Copyright (C) 2000-2017 Julian Seward
- Rust libm crate: Copyright (c) 2018 Jorge Aparicio; portions from musl libc,
  Copyright (c) 2005-2020 Rich Felker, et al., and from CORE-MATH (cbrt Copyright
  (c) 2021-2022 Alexei Sibidanov, hypot Copyright (c) 2022 Alexei Sibidanov)
- compiler-builtins: Copyright (c) The Rust Project Contributors
- num-bigint: Copyright (c) 2014 The Rust Project Developers
- rust_decimal: Copyright (c) 2016 Paul Mason
- cespare/xxhash: Copyright (c) 2016 Caleb Spare
- zlib: Copyright (C) 1995-2023 Jean-loup Gailly and Mark Adler
- FoundationDB: Copyright 2013-2026 Apple Inc. and the FoundationDB project authors
- rustc-hash: Copyright (c) The Rust Project Developers
- Datastar: Copyright (c) Star Federation
- Protocol Buffers: Copyright 2008 Google Inc. All rights reserved.
- protobuf-go: Copyright (c) 2018 The Go Authors. All rights reserved.
- prost: Copyright Dan Burkert and Tokio contributors
- grpc-go, tonic interop protos and helloworld.proto: Copyright 2014-2019 gRPC authors
- tonic: Copyright 2019 gRPC authors (MIT)
- CPython: Copyright (c) 2001 Python Software Foundation; All Rights Reserved
- google/uuid: Copyright (c) 2009,2014 Google Inc. All rights reserved.
- uuid: Copyright (c) 2014 The Rust Project Developers; Copyright (c) 2018 Ashley
  Mannix, Christopher Armstrong, Dylan DPC, Hunar Roop Kahlon

## Zlib License (zlib)

```text
This software is provided 'as-is', without any express or implied
warranty.  In no event will the authors be held liable for any damages
arising from the use of this software.

Permission is granted to anyone to use this software for any purpose,
including commercial applications, and to alter it and redistribute it
freely, subject to the following restrictions:

1. The origin of this software must not be misrepresented; you must not
   claim that you wrote the original software. If you use this software
   in a product, an acknowledgment in the product documentation would be
   appreciated but is not required.
2. Altered source versions must be plainly marked as such, and must not be
   misrepresented as being the original software.
3. This notice may not be removed or altered from any source distribution.
```

## MIT License

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.

## BSD-3-Clause License (The Computer Language Benchmarks Game)

All rights reserved.

Redistribution and use in source and binary forms, with or without modification,
are permitted provided that the following conditions are met:

1. Redistributions of source code must retain the above copyright notice, this
   list of conditions and the following disclaimer.
2. Redistributions in binary form must reproduce the above copyright notice,
   this list of conditions and the following disclaimer in the documentation
   and/or other materials provided with the distribution.
3. Neither the name "The Computer Language Benchmarks Game" nor the name "The
   Benchmarks Game" nor the name "The Computer Language Shootout Benchmarks" nor
   the names of its contributors may be used to endorse or promote products
   derived from this software without specific prior written permission.

THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS "AS IS" AND
ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED TO, THE IMPLIED
WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE ARE DISCLAIMED.
IN NO EVENT SHALL THE COPYRIGHT OWNER OR CONTRIBUTORS BE LIABLE FOR ANY DIRECT,
INDIRECT, INCIDENTAL, SPECIAL, EXEMPLARY, OR CONSEQUENTIAL DAMAGES (INCLUDING,
BUT NOT LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS OR SERVICES; LOSS OF USE,
DATA, OR PROFITS; OR BUSINESS INTERRUPTION) HOWEVER CAUSED AND ON ANY THEORY OF
LIABILITY, WHETHER IN CONTRACT, STRICT LIABILITY, OR TORT (INCLUDING NEGLIGENCE
OR OTHERWISE) ARISING IN ANY WAY OUT OF THE USE OF THIS SOFTWARE, EVEN IF ADVISED
OF THE POSSIBILITY OF SUCH DAMAGE.

## Valgrind header licence (valgrind.h, memcheck.h)

It applies to those two files only; the rest of Valgrind is GPL-2.0 and is not
vendored.

Redistribution and use in source and binary forms, with or without
modification, are permitted provided that the following conditions
are met:

1. Redistributions of source code must retain the above copyright
   notice, this list of conditions and the following disclaimer.

2. The origin of this software must not be misrepresented; you must
   not claim that you wrote the original software.  If you use this
   software in a product, an acknowledgment in the product
   documentation would be appreciated but is not required.

3. Altered source versions must be plainly marked as such, and must
   not be misrepresented as being the original software.

4. The name of the author may not be used to endorse or promote
   products derived from this software without specific prior written
   permission.

THIS SOFTWARE IS PROVIDED BY THE AUTHOR ``AS IS'' AND ANY EXPRESS
OR IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED TO, THE IMPLIED
WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE
ARE DISCLAIMED.  IN NO EVENT SHALL THE AUTHOR BE LIABLE FOR ANY
DIRECT, INDIRECT, INCIDENTAL, SPECIAL, EXEMPLARY, OR CONSEQUENTIAL
DAMAGES (INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF SUBSTITUTE
GOODS OR SERVICES; LOSS OF USE, DATA, OR PROFITS; OR BUSINESS
INTERRUPTION) HOWEVER CAUSED AND ON ANY THEORY OF LIABILITY,
WHETHER IN CONTRACT, STRICT LIABILITY, OR TORT (INCLUDING
NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE USE OF THIS
SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.

## Simplified BSD License (RFC 8949 code components)

Redistribution and use in source and binary forms, with or without
modification, are permitted provided that the following conditions are met:

1. Redistributions of source code must retain the above copyright notice,
   this list of conditions and the following disclaimer.
2. Redistributions in binary form must reproduce the above copyright notice,
   this list of conditions and the following disclaimer in the documentation
   and/or other materials provided with the distribution.

THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS "AS IS"
AND ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED TO, THE
IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE
ARE DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT HOLDER OR CONTRIBUTORS BE
LIABLE FOR ANY DIRECT, INDIRECT, INCIDENTAL, SPECIAL, EXEMPLARY, OR
CONSEQUENTIAL DAMAGES (INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF
SUBSTITUTE GOODS OR SERVICES; LOSS OF USE, DATA, OR PROFITS; OR BUSINESS
INTERRUPTION) HOWEVER CAUSED AND ON ANY THEORY OF LIABILITY, WHETHER IN
CONTRACT, STRICT LIABILITY, OR TORT (INCLUDING NEGLIGENCE OR OTHERWISE)
ARISING IN ANY WAY OUT OF THE USE OF THIS SOFTWARE, EVEN IF ADVISED OF THE
POSSIBILITY OF SUCH DAMAGE.

## BSD-3-Clause License (Go)

Copyright 2009 The Go Authors.

Redistribution and use in source and binary forms, with or without
modification, are permitted provided that the following conditions are
met:

   * Redistributions of source code must retain the above copyright
notice, this list of conditions and the following disclaimer.
   * Redistributions in binary form must reproduce the above
copyright notice, this list of conditions and the following disclaimer
in the documentation and/or other materials provided with the
distribution.
   * Neither the name of Google LLC nor the names of its
contributors may be used to endorse or promote products derived from
this software without specific prior written permission.

THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS
"AS IS" AND ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT
LIMITED TO, THE IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR
A PARTICULAR PURPOSE ARE DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT
OWNER OR CONTRIBUTORS BE LIABLE FOR ANY DIRECT, INDIRECT, INCIDENTAL,
SPECIAL, EXEMPLARY, OR CONSEQUENTIAL DAMAGES (INCLUDING, BUT NOT
LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS OR SERVICES; LOSS OF USE,
DATA, OR PROFITS; OR BUSINESS INTERRUPTION) HOWEVER CAUSED AND ON ANY
THEORY OF LIABILITY, WHETHER IN CONTRACT, STRICT LIABILITY, OR TORT
(INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE USE
OF THIS SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.

## BSD-3-Clause License (RE2)

Copyright (c) 2009 The RE2 Authors. All rights reserved.

Redistribution and use in source and binary forms, with or without
modification, are permitted provided that the following conditions are
met:

   * Redistributions of source code must retain the above copyright
notice, this list of conditions and the following disclaimer.
   * Redistributions in binary form must reproduce the above
copyright notice, this list of conditions and the following disclaimer
in the documentation and/or other materials provided with the
distribution.
   * Neither the name of Google Inc. nor the names of its
contributors may be used to endorse or promote products derived from
this software without specific prior written permission.

THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS
"AS IS" AND ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT
LIMITED TO, THE IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR
A PARTICULAR PURPOSE ARE DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT
OWNER OR CONTRIBUTORS BE LIABLE FOR ANY DIRECT, INDIRECT, INCIDENTAL,
SPECIAL, EXEMPLARY, OR CONSEQUENTIAL DAMAGES (INCLUDING, BUT NOT
LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS OR SERVICES; LOSS OF USE,
DATA, OR PROFITS; OR BUSINESS INTERRUPTION) HOWEVER CAUSED AND ON ANY
THEORY OF LIABILITY, WHETHER IN CONTRACT, STRICT LIABILITY, OR TORT
(INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE USE
OF THIS SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.

## Sun Microsystems square root

The Go portable square-root algorithm retains the following original notice:

```text
Copyright (C) 1993 by Sun Microsystems, Inc. All rights reserved.

Developed at SunPro, a Sun Microsystems, Inc. business.
Permission to use, copy, modify, and distribute this
software is freely granted, provided that this notice
is preserved.
```

## musl and FreeBSD msun math notices (Rust libm crate)

The musl libc COPYRIGHT file, carried by the libm crate, says of its math library:

```text
Much of the math library code (src/math/* and src/complex/*) is
Copyright © 1993,2004 Sun Microsystems or
Copyright © 2003-2011 David Schultz or
Copyright © 2003-2009 Steven G. Kargl or
Copyright © 2003-2009 Bruce D. Evans or
Copyright © 2008 Stephen L. Moshier or
Copyright © 2017-2018 Arm Limited
and labelled as such in comments in the individual source files. All
have been licensed under extremely permissive terms.
```

The ported FreeBSD msun routines retain:

```text
Copyright (C) 1993 by Sun Microsystems, Inc. All rights reserved.

Developed at SunPro, a Sun Microsystems, Inc. business.
Permission to use, copy, modify, and distribute this
software is freely granted, provided that this notice
is preserved.
```

`s_exp2.c` and `s_exp2f.c` retain:

```text
Copyright (c) 2005 David Schultz <das@FreeBSD.ORG>
All rights reserved.

Redistribution and use in source and binary forms, with or without
modification, are permitted provided that the following conditions
are met:
1. Redistributions of source code must retain the above copyright
   notice, this list of conditions and the following disclaimer.
2. Redistributions in binary form must reproduce the above copyright
   notice, this list of conditions and the following disclaimer in the
   documentation and/or other materials provided with the distribution.

THIS SOFTWARE IS PROVIDED BY THE AUTHOR AND CONTRIBUTORS ``AS IS'' AND
ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED TO, THE
IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE
ARE DISCLAIMED.  IN NO EVENT SHALL THE AUTHOR OR CONTRIBUTORS BE LIABLE
FOR ANY DIRECT, INDIRECT, INCIDENTAL, SPECIAL, EXEMPLARY, OR CONSEQUENTIAL
DAMAGES (INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS
OR SERVICES; LOSS OF USE, DATA, OR PROFITS; OR BUSINESS INTERRUPTION)
HOWEVER CAUSED AND ON ANY THEORY OF LIABILITY, WHETHER IN CONTRACT, STRICT
LIABILITY, OR TORT (INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY
OUT OF THE USE OF THIS SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF
SUCH DAMAGE.
```

## Python Software Foundation License Version 2 (CPython)

```text
1. This LICENSE AGREEMENT is between the Python Software Foundation
("PSF"), and the Individual or Organization ("Licensee") accessing and
otherwise using this software ("Python") in source or binary form and
its associated documentation.

2. Subject to the terms and conditions of this License Agreement, PSF hereby
grants Licensee a nonexclusive, royalty-free, world-wide license to reproduce,
analyze, test, perform and/or display publicly, prepare derivative works,
distribute, and otherwise use Python alone or in any derivative version,
provided, however, that PSF's License Agreement and PSF's notice of copyright,
i.e., "Copyright (c) 2001 Python Software Foundation; All Rights Reserved"
are retained in Python alone or in any derivative version prepared by Licensee.

3. In the event Licensee prepares a derivative work that is based on
or incorporates Python or any part thereof, and wants to make
the derivative work available to others as provided herein, then
Licensee hereby agrees to include in any such work a brief summary of
the changes made to Python.

4. PSF is making Python available to Licensee on an "AS IS"
basis.  PSF MAKES NO REPRESENTATIONS OR WARRANTIES, EXPRESS OR
IMPLIED.  BY WAY OF EXAMPLE, BUT NOT LIMITATION, PSF MAKES NO AND
DISCLAIMS ANY REPRESENTATION OR WARRANTY OF MERCHANTABILITY OR FITNESS
FOR ANY PARTICULAR PURPOSE OR THAT THE USE OF PYTHON WILL NOT
INFRINGE ANY THIRD PARTY RIGHTS.

5. PSF SHALL NOT BE LIABLE TO LICENSEE OR ANY OTHER USERS OF PYTHON
FOR ANY INCIDENTAL, SPECIAL, OR CONSEQUENTIAL DAMAGES OR LOSS AS
A RESULT OF MODIFYING, DISTRIBUTING, OR OTHERWISE USING PYTHON,
OR ANY DERIVATIVE THEREOF, EVEN IF ADVISED OF THE POSSIBILITY THEREOF.

6. This License Agreement will automatically terminate upon a material
breach of its terms and conditions.

7. Nothing in this License Agreement shall be deemed to create any
relationship of agency, partnership, or joint venture between PSF and
Licensee.  This License Agreement does not grant permission to use PSF
trademarks or trade name in a trademark sense to endorse or promote
products or services of Licensee, or any third party.

8. By copying, installing or otherwise using Python, Licensee
agrees to be bound by the terms and conditions of this License
Agreement.
```

## BSD-3-Clause License (google/uuid)

```text
Copyright (c) 2009,2014 Google Inc. All rights reserved.

Redistribution and use in source and binary forms, with or without
modification, are permitted provided that the following conditions are
met:

   * Redistributions of source code must retain the above copyright
notice, this list of conditions and the following disclaimer.
   * Redistributions in binary form must reproduce the above
copyright notice, this list of conditions and the following disclaimer
in the documentation and/or other materials provided with the
distribution.
   * Neither the name of Google Inc. nor the names of its
contributors may be used to endorse or promote products derived from
this software without specific prior written permission.

THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS
"AS IS" AND ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT
LIMITED TO, THE IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR
A PARTICULAR PURPOSE ARE DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT
OWNER OR CONTRIBUTORS BE LIABLE FOR ANY DIRECT, INDIRECT, INCIDENTAL,
SPECIAL, EXEMPLARY, OR CONSEQUENTIAL DAMAGES (INCLUDING, BUT NOT
LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS OR SERVICES; LOSS OF USE,
DATA, OR PROFITS; OR BUSINESS INTERRUPTION) HOWEVER CAUSED AND ON ANY
THEORY OF LIABILITY, WHETHER IN CONTRACT, STRICT LIABILITY, OR TORT
(INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE USE
OF THIS SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.
```

## Apache License 2.0 with LLVM Exception (Cranelift, LLVM)

Wycheproof is under the Apache License 2.0 alone: the text below without the
LLVM exceptions that follow it.

```
                                 Apache License
                           Version 2.0, January 2004
                        http://www.apache.org/licenses/

   TERMS AND CONDITIONS FOR USE, REPRODUCTION, AND DISTRIBUTION

   1. Definitions.

      "License" shall mean the terms and conditions for use, reproduction,
      and distribution as defined by Sections 1 through 9 of this document.

      "Licensor" shall mean the copyright owner or entity authorized by
      the copyright owner that is granting the License.

      "Legal Entity" shall mean the union of the acting entity and all
      other entities that control, are controlled by, or are under common
      control with that entity. For the purposes of this definition,
      "control" means (i) the power, direct or indirect, to cause the
      direction or management of such entity, whether by contract or
      otherwise, or (ii) ownership of fifty percent (50%) or more of the
      outstanding shares, or (iii) beneficial ownership of such entity.

      "You" (or "Your") shall mean an individual or Legal Entity
      exercising permissions granted by this License.

      "Source" form shall mean the preferred form for making modifications,
      including but not limited to software source code, documentation
      source, and configuration files.

      "Object" form shall mean any form resulting from mechanical
      transformation or translation of a Source form, including but
      not limited to compiled object code, generated documentation,
      and conversions to other media types.

      "Work" shall mean the work of authorship, whether in Source or
      Object form, made available under the License, as indicated by a
      copyright notice that is included in or attached to the work
      (an example is provided in the Appendix below).

      "Derivative Works" shall mean any work, whether in Source or Object
      form, that is based on (or derived from) the Work and for which the
      editorial revisions, annotations, elaborations, or other modifications
      represent, as a whole, an original work of authorship. For the purposes
      of this License, Derivative Works shall not include works that remain
      separable from, or merely link (or bind by name) to the interfaces of,
      the Work and Derivative Works thereof.

      "Contribution" shall mean any work of authorship, including
      the original version of the Work and any modifications or additions
      to that Work or Derivative Works thereof, that is intentionally
      submitted to Licensor for inclusion in the Work by the copyright owner
      or by an individual or Legal Entity authorized to submit on behalf of
      the copyright owner. For the purposes of this definition, "submitted"
      means any form of electronic, verbal, or written communication sent
      to the Licensor or its representatives, including but not limited to
      communication on electronic mailing lists, source code control systems,
      and issue tracking systems that are managed by, or on behalf of, the
      Licensor for the purpose of discussing and improving the Work, but
      excluding communication that is conspicuously marked or otherwise
      designated in writing by the copyright owner as "Not a Contribution."

      "Contributor" shall mean Licensor and any individual or Legal Entity
      on behalf of whom a Contribution has been received by Licensor and
      subsequently incorporated within the Work.

   2. Grant of Copyright License. Subject to the terms and conditions of
      this License, each Contributor hereby grants to You a perpetual,
      worldwide, non-exclusive, no-charge, royalty-free, irrevocable
      copyright license to reproduce, prepare Derivative Works of,
      publicly display, publicly perform, sublicense, and distribute the
      Work and such Derivative Works in Source or Object form.

   3. Grant of Patent License. Subject to the terms and conditions of
      this License, each Contributor hereby grants to You a perpetual,
      worldwide, non-exclusive, no-charge, royalty-free, irrevocable
      (except as stated in this section) patent license to make, have made,
      use, offer to sell, sell, import, and otherwise transfer the Work,
      where such license applies only to those patent claims licensable
      by such Contributor that are necessarily infringed by their
      Contribution(s) alone or by combination of their Contribution(s)
      with the Work to which such Contribution(s) was submitted. If You
      institute patent litigation against any entity (including a
      cross-claim or counterclaim in a lawsuit) alleging that the Work
      or a Contribution incorporated within the Work constitutes direct
      or contributory patent infringement, then any patent licenses
      granted to You under this License for that Work shall terminate
      as of the date such litigation is filed.

   4. Redistribution. You may reproduce and distribute copies of the
      Work or Derivative Works thereof in any medium, with or without
      modifications, and in Source or Object form, provided that You
      meet the following conditions:

      (a) You must give any other recipients of the Work or
          Derivative Works a copy of this License; and

      (b) You must cause any modified files to carry prominent notices
          stating that You changed the files; and

      (c) You must retain, in the Source form of any Derivative Works
          that You distribute, all copyright, patent, trademark, and
          attribution notices from the Source form of the Work,
          excluding those notices that do not pertain to any part of
          the Derivative Works; and

      (d) If the Work includes a "NOTICE" text file as part of its
          distribution, then any Derivative Works that You distribute must
          include a readable copy of the attribution notices contained
          within such NOTICE file, excluding those notices that do not
          pertain to any part of the Derivative Works, in at least one
          of the following places: within a NOTICE text file distributed
          as part of the Derivative Works; within the Source form or
          documentation, if provided along with the Derivative Works; or,
          within a display generated by the Derivative Works, if and
          wherever such third-party notices normally appear. The contents
          of the NOTICE file are for informational purposes only and
          do not modify the License. You may add Your own attribution
          notices within Derivative Works that You distribute, alongside
          or as an addendum to the NOTICE text from the Work, provided
          that such additional attribution notices cannot be construed
          as modifying the License.

      You may add Your own copyright statement to Your modifications and
      may provide additional or different license terms and conditions
      for use, reproduction, or distribution of Your modifications, or
      for any such Derivative Works as a whole, provided Your use,
      reproduction, and distribution of the Work otherwise complies with
      the conditions stated in this License.

   5. Submission of Contributions. Unless You explicitly state otherwise,
      any Contribution intentionally submitted for inclusion in the Work
      by You to the Licensor shall be under the terms and conditions of
      this License, without any additional terms or conditions.
      Notwithstanding the above, nothing herein shall supersede or modify
      the terms of any separate license agreement you may have executed
      with Licensor regarding such Contributions.

   6. Trademarks. This License does not grant permission to use the trade
      names, trademarks, service marks, or product names of the Licensor,
      except as required for reasonable and customary use in describing the
      origin of the Work and reproducing the content of the NOTICE file.

   7. Disclaimer of Warranty. Unless required by applicable law or
      agreed to in writing, Licensor provides the Work (and each
      Contributor provides its Contributions) on an "AS IS" BASIS,
      WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or
      implied, including, without limitation, any warranties or conditions
      of TITLE, NON-INFRINGEMENT, MERCHANTABILITY, or FITNESS FOR A
      PARTICULAR PURPOSE. You are solely responsible for determining the
      appropriateness of using or redistributing the Work and assume any
      risks associated with Your exercise of permissions under this License.

   8. Limitation of Liability. In no event and under no legal theory,
      whether in tort (including negligence), contract, or otherwise,
      unless required by applicable law (such as deliberate and grossly
      negligent acts) or agreed to in writing, shall any Contributor be
      liable to You for damages, including any direct, indirect, special,
      incidental, or consequential damages of any character arising as a
      result of this License or out of the use or inability to use the
      Work (including but not limited to damages for loss of goodwill,
      work stoppage, computer failure or malfunction, or any and all
      other commercial damages or losses), even if such Contributor
      has been advised of the possibility of such damages.

   9. Accepting Warranty or Additional Liability. While redistributing
      the Work or Derivative Works thereof, You may choose to offer,
      and charge a fee for, acceptance of support, warranty, indemnity,
      or other liability obligations and/or rights consistent with this
      License. However, in accepting such obligations, You may act only
      on Your own behalf and on Your sole responsibility, not on behalf
      of any other Contributor, and only if You agree to indemnify,
      defend, and hold each Contributor harmless for any liability
      incurred by, or claims asserted against, such Contributor by reason
      of your accepting any such warranty or additional liability.

   END OF TERMS AND CONDITIONS

   APPENDIX: How to apply the Apache License to your work.

      To apply the Apache License to your work, attach the following
      boilerplate notice, with the fields enclosed by brackets "[]"
      replaced with your own identifying information. (Don't include
      the brackets!)  The text should be enclosed in the appropriate
      comment syntax for the file format. We also recommend that a
      file or class name and description of purpose be included on the
      same "printed page" as the copyright notice for easier
      identification within third-party archives.

   Copyright [yyyy] [name of copyright owner]

   Licensed under the Apache License, Version 2.0 (the "License");
   you may not use this file except in compliance with the License.
   You may obtain a copy of the License at

       http://www.apache.org/licenses/LICENSE-2.0

   Unless required by applicable law or agreed to in writing, software
   distributed under the License is distributed on an "AS IS" BASIS,
   WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
   See the License for the specific language governing permissions and
   limitations under the License.


--- LLVM Exceptions to the Apache 2.0 License ----

As an exception, if, as a result of your compiling your source code, portions
of this Software are embedded into an Object form of such source code, you
may redistribute such embedded portions in such Object form without complying
with the conditions of Sections 4(a), 4(b) and 4(d) of the License.

In addition, if you combine or link compiled forms of this Software with
software that is licensed under the GPLv2 ("Combined Software") and if a
court of competent jurisdiction determines that the patent provision (Section
3), the indemnity provision (Section 9) or other Section of the License
conflicts with the conditions of the GPLv2, you may retroactively and
prospectively choose to deem waived or otherwise exclude such Section(s) of
the License, but only in their entirety and only with respect to the Combined
Software.
```

## BSD-3-Clause License (Zstandard)

BSD License

For Zstandard software

Copyright (c) Meta Platforms, Inc. and affiliates. All rights reserved.

Redistribution and use in source and binary forms, with or without modification,
are permitted provided that the following conditions are met:

 * Redistributions of source code must retain the above copyright notice, this
   list of conditions and the following disclaimer.

 * Redistributions in binary form must reproduce the above copyright notice,
   this list of conditions and the following disclaimer in the documentation
   and/or other materials provided with the distribution.

 * Neither the name Facebook, nor Meta, nor the names of its contributors may
   be used to endorse or promote products derived from this software without
   specific prior written permission.

THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS "AS IS" AND
ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED TO, THE IMPLIED
WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE ARE
DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT HOLDER OR CONTRIBUTORS BE LIABLE FOR
ANY DIRECT, INDIRECT, INCIDENTAL, SPECIAL, EXEMPLARY, OR CONSEQUENTIAL DAMAGES
(INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS OR SERVICES;
LOSS OF USE, DATA, OR PROFITS; OR BUSINESS INTERRUPTION) HOWEVER CAUSED AND ON
ANY THEORY OF LIABILITY, WHETHER IN CONTRACT, STRICT LIABILITY, OR TORT
(INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE USE OF THIS
SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.

## BSD-2-Clause (commonmark.js and cmark-gfm)

Copyright (c) 2014, John MacFarlane

All rights reserved.

Redistribution and use in source and binary forms, with or without
modification, are permitted provided that the following conditions are met:

    * Redistributions of source code must retain the above copyright
      notice, this list of conditions and the following disclaimer.

    * Redistributions in binary form must reproduce the above
      copyright notice, this list of conditions and the following
      disclaimer in the documentation and/or other materials provided
      with the distribution.

THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS
"AS IS" AND ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT
LIMITED TO, THE IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR
A PARTICULAR PURPOSE ARE DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT
OWNER OR CONTRIBUTORS BE LIABLE FOR ANY DIRECT, INDIRECT, INCIDENTAL,
SPECIAL, EXEMPLARY, OR CONSEQUENTIAL DAMAGES (INCLUDING, BUT NOT
LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS OR SERVICES; LOSS OF USE,
DATA, OR PROFITS; OR BUSINESS INTERRUPTION) HOWEVER CAUSED AND ON ANY
THEORY OF LIABILITY, WHETHER IN CONTRACT, STRICT LIABILITY, OR TORT
(INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE USE
OF THIS SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.


## CC-BY-SA-4.0 (CommonMark and GFM specification vectors)

The CommonMark specification is Copyright (C) 2014–2024 John MacFarlane.
The GitHub Flavored Markdown specification is derived from CommonMark by GitHub.
The published examples are adapted into Hirsel conformance programs, with separate
truncated-input controls generated by the reference parser. The adapted vectors
remain under [Creative Commons Attribution-ShareAlike 4.0](https://creativecommons.org/licenses/by-sa/4.0/).
The full license text follows.

```text
Attribution-ShareAlike 4.0 International

=======================================================================

Creative Commons Corporation ("Creative Commons") is not a law firm and
does not provide legal services or legal advice. Distribution of
Creative Commons public licenses does not create a lawyer-client or
other relationship. Creative Commons makes its licenses and related
information available on an "as-is" basis. Creative Commons gives no
warranties regarding its licenses, any material licensed under their
terms and conditions, or any related information. Creative Commons
disclaims all liability for damages resulting from their use to the
fullest extent possible.

Using Creative Commons Public Licenses

Creative Commons public licenses provide a standard set of terms and
conditions that creators and other rights holders may use to share
original works of authorship and other material subject to copyright
and certain other rights specified in the public license below. The
following considerations are for informational purposes only, are not
exhaustive, and do not form part of our licenses.

     Considerations for licensors: Our public licenses are
     intended for use by those authorized to give the public
     permission to use material in ways otherwise restricted by
     copyright and certain other rights. Our licenses are
     irrevocable. Licensors should read and understand the terms
     and conditions of the license they choose before applying it.
     Licensors should also secure all rights necessary before
     applying our licenses so that the public can reuse the
     material as expected. Licensors should clearly mark any
     material not subject to the license. This includes other CC-
     licensed material, or material used under an exception or
     limitation to copyright. More considerations for licensors:
    wiki.creativecommons.org/Considerations_for_licensors

     Considerations for the public: By using one of our public
     licenses, a licensor grants the public permission to use the
     licensed material under specified terms and conditions. If
     the licensor's permission is not necessary for any reason--for
     example, because of any applicable exception or limitation to
     copyright--then that use is not regulated by the license. Our
     licenses grant only permissions under copyright and certain
     other rights that a licensor has authority to grant. Use of
     the licensed material may still be restricted for other
     reasons, including because others have copyright or other
     rights in the material. A licensor may make special requests,
     such as asking that all changes be marked or described.
     Although not required by our licenses, you are encouraged to
     respect those requests where reasonable. More considerations
     for the public:
    wiki.creativecommons.org/Considerations_for_licensees

=======================================================================

Creative Commons Attribution-ShareAlike 4.0 International Public
License

By exercising the Licensed Rights (defined below), You accept and agree
to be bound by the terms and conditions of this Creative Commons
Attribution-ShareAlike 4.0 International Public License ("Public
License"). To the extent this Public License may be interpreted as a
contract, You are granted the Licensed Rights in consideration of Your
acceptance of these terms and conditions, and the Licensor grants You
such rights in consideration of benefits the Licensor receives from
making the Licensed Material available under these terms and
conditions.


Section 1 -- Definitions.

  a. Adapted Material means material subject to Copyright and Similar
     Rights that is derived from or based upon the Licensed Material
     and in which the Licensed Material is translated, altered,
     arranged, transformed, or otherwise modified in a manner requiring
     permission under the Copyright and Similar Rights held by the
     Licensor. For purposes of this Public License, where the Licensed
     Material is a musical work, performance, or sound recording,
     Adapted Material is always produced where the Licensed Material is
     synched in timed relation with a moving image.

  b. Adapter's License means the license You apply to Your Copyright
     and Similar Rights in Your contributions to Adapted Material in
     accordance with the terms and conditions of this Public License.

  c. BY-SA Compatible License means a license listed at
     creativecommons.org/compatiblelicenses, approved by Creative
     Commons as essentially the equivalent of this Public License.

  d. Copyright and Similar Rights means copyright and/or similar rights
     closely related to copyright including, without limitation,
     performance, broadcast, sound recording, and Sui Generis Database
     Rights, without regard to how the rights are labeled or
     categorized. For purposes of this Public License, the rights
     specified in Section 2(b)(1)-(2) are not Copyright and Similar
     Rights.

  e. Effective Technological Measures means those measures that, in the
     absence of proper authority, may not be circumvented under laws
     fulfilling obligations under Article 11 of the WIPO Copyright
     Treaty adopted on December 20, 1996, and/or similar international
     agreements.

  f. Exceptions and Limitations means fair use, fair dealing, and/or
     any other exception or limitation to Copyright and Similar Rights
     that applies to Your use of the Licensed Material.

  g. License Elements means the license attributes listed in the name
     of a Creative Commons Public License. The License Elements of this
     Public License are Attribution and ShareAlike.

  h. Licensed Material means the artistic or literary work, database,
     or other material to which the Licensor applied this Public
     License.

  i. Licensed Rights means the rights granted to You subject to the
     terms and conditions of this Public License, which are limited to
     all Copyright and Similar Rights that apply to Your use of the
     Licensed Material and that the Licensor has authority to license.

  j. Licensor means the individual(s) or entity(ies) granting rights
     under this Public License.

  k. Share means to provide material to the public by any means or
     process that requires permission under the Licensed Rights, such
     as reproduction, public display, public performance, distribution,
     dissemination, communication, or importation, and to make material
     available to the public including in ways that members of the
     public may access the material from a place and at a time
     individually chosen by them.

  l. Sui Generis Database Rights means rights other than copyright
     resulting from Directive 96/9/EC of the European Parliament and of
     the Council of 11 March 1996 on the legal protection of databases,
     as amended and/or succeeded, as well as other essentially
     equivalent rights anywhere in the world.

  m. You means the individual or entity exercising the Licensed Rights
     under this Public License. Your has a corresponding meaning.


Section 2 -- Scope.

  a. License grant.

       1. Subject to the terms and conditions of this Public License,
          the Licensor hereby grants You a worldwide, royalty-free,
          non-sublicensable, non-exclusive, irrevocable license to
          exercise the Licensed Rights in the Licensed Material to:

            a. reproduce and Share the Licensed Material, in whole or
               in part; and

            b. produce, reproduce, and Share Adapted Material.

       2. Exceptions and Limitations. For the avoidance of doubt, where
          Exceptions and Limitations apply to Your use, this Public
          License does not apply, and You do not need to comply with
          its terms and conditions.

       3. Term. The term of this Public License is specified in Section
          6(a).

       4. Media and formats; technical modifications allowed. The
          Licensor authorizes You to exercise the Licensed Rights in
          all media and formats whether now known or hereafter created,
          and to make technical modifications necessary to do so. The
          Licensor waives and/or agrees not to assert any right or
          authority to forbid You from making technical modifications
          necessary to exercise the Licensed Rights, including
          technical modifications necessary to circumvent Effective
          Technological Measures. For purposes of this Public License,
          simply making modifications authorized by this Section 2(a)
          (4) never produces Adapted Material.

       5. Downstream recipients.

            a. Offer from the Licensor -- Licensed Material. Every
               recipient of the Licensed Material automatically
               receives an offer from the Licensor to exercise the
               Licensed Rights under the terms and conditions of this
               Public License.

            b. Additional offer from the Licensor -- Adapted Material.
               Every recipient of Adapted Material from You
               automatically receives an offer from the Licensor to
               exercise the Licensed Rights in the Adapted Material
               under the conditions of the Adapter's License You apply.

            c. No downstream restrictions. You may not offer or impose
               any additional or different terms or conditions on, or
               apply any Effective Technological Measures to, the
               Licensed Material if doing so restricts exercise of the
               Licensed Rights by any recipient of the Licensed
               Material.

       6. No endorsement. Nothing in this Public License constitutes or
          may be construed as permission to assert or imply that You
          are, or that Your use of the Licensed Material is, connected
          with, or sponsored, endorsed, or granted official status by,
          the Licensor or others designated to receive attribution as
          provided in Section 3(a)(1)(A)(i).

  b. Other rights.

       1. Moral rights, such as the right of integrity, are not
          licensed under this Public License, nor are publicity,
          privacy, and/or other similar personality rights; however, to
          the extent possible, the Licensor waives and/or agrees not to
          assert any such rights held by the Licensor to the limited
          extent necessary to allow You to exercise the Licensed
          Rights, but not otherwise.

       2. Patent and trademark rights are not licensed under this
          Public License.

       3. To the extent possible, the Licensor waives any right to
          collect royalties from You for the exercise of the Licensed
          Rights, whether directly or through a collecting society
          under any voluntary or waivable statutory or compulsory
          licensing scheme. In all other cases the Licensor expressly
          reserves any right to collect such royalties.


Section 3 -- License Conditions.

Your exercise of the Licensed Rights is expressly made subject to the
following conditions.

  a. Attribution.

       1. If You Share the Licensed Material (including in modified
          form), You must:

            a. retain the following if it is supplied by the Licensor
               with the Licensed Material:

                 i. identification of the creator(s) of the Licensed
                    Material and any others designated to receive
                    attribution, in any reasonable manner requested by
                    the Licensor (including by pseudonym if
                    designated);

                ii. a copyright notice;

               iii. a notice that refers to this Public License;

                iv. a notice that refers to the disclaimer of
                    warranties;

                 v. a URI or hyperlink to the Licensed Material to the
                    extent reasonably practicable;

            b. indicate if You modified the Licensed Material and
               retain an indication of any previous modifications; and

            c. indicate the Licensed Material is licensed under this
               Public License, and include the text of, or the URI or
               hyperlink to, this Public License.

       2. You may satisfy the conditions in Section 3(a)(1) in any
          reasonable manner based on the medium, means, and context in
          which You Share the Licensed Material. For example, it may be
          reasonable to satisfy the conditions by providing a URI or
          hyperlink to a resource that includes the required
          information.

       3. If requested by the Licensor, You must remove any of the
          information required by Section 3(a)(1)(A) to the extent
          reasonably practicable.

  b. ShareAlike.

     In addition to the conditions in Section 3(a), if You Share
     Adapted Material You produce, the following conditions also apply.

       1. The Adapter's License You apply must be a Creative Commons
          license with the same License Elements, this version or
          later, or a BY-SA Compatible License.

       2. You must include the text of, or the URI or hyperlink to, the
          Adapter's License You apply. You may satisfy this condition
          in any reasonable manner based on the medium, means, and
          context in which You Share Adapted Material.

       3. You may not offer or impose any additional or different terms
          or conditions on, or apply any Effective Technological
          Measures to, Adapted Material that restrict exercise of the
          rights granted under the Adapter's License You apply.


Section 4 -- Sui Generis Database Rights.

Where the Licensed Rights include Sui Generis Database Rights that
apply to Your use of the Licensed Material:

  a. for the avoidance of doubt, Section 2(a)(1) grants You the right
     to extract, reuse, reproduce, and Share all or a substantial
     portion of the contents of the database;

  b. if You include all or a substantial portion of the database
     contents in a database in which You have Sui Generis Database
     Rights, then the database in which You have Sui Generis Database
     Rights (but not its individual contents) is Adapted Material,
     including for purposes of Section 3(b); and

  c. You must comply with the conditions in Section 3(a) if You Share
     all or a substantial portion of the contents of the database.

For the avoidance of doubt, this Section 4 supplements and does not
replace Your obligations under this Public License where the Licensed
Rights include other Copyright and Similar Rights.


Section 5 -- Disclaimer of Warranties and Limitation of Liability.

  a. UNLESS OTHERWISE SEPARATELY UNDERTAKEN BY THE LICENSOR, TO THE
     EXTENT POSSIBLE, THE LICENSOR OFFERS THE LICENSED MATERIAL AS-IS
     AND AS-AVAILABLE, AND MAKES NO REPRESENTATIONS OR WARRANTIES OF
     ANY KIND CONCERNING THE LICENSED MATERIAL, WHETHER EXPRESS,
     IMPLIED, STATUTORY, OR OTHER. THIS INCLUDES, WITHOUT LIMITATION,
     WARRANTIES OF TITLE, MERCHANTABILITY, FITNESS FOR A PARTICULAR
     PURPOSE, NON-INFRINGEMENT, ABSENCE OF LATENT OR OTHER DEFECTS,
     ACCURACY, OR THE PRESENCE OR ABSENCE OF ERRORS, WHETHER OR NOT
     KNOWN OR DISCOVERABLE. WHERE DISCLAIMERS OF WARRANTIES ARE NOT
     ALLOWED IN FULL OR IN PART, THIS DISCLAIMER MAY NOT APPLY TO YOU.

  b. TO THE EXTENT POSSIBLE, IN NO EVENT WILL THE LICENSOR BE LIABLE
     TO YOU ON ANY LEGAL THEORY (INCLUDING, WITHOUT LIMITATION,
     NEGLIGENCE) OR OTHERWISE FOR ANY DIRECT, SPECIAL, INDIRECT,
     INCIDENTAL, CONSEQUENTIAL, PUNITIVE, EXEMPLARY, OR OTHER LOSSES,
     COSTS, EXPENSES, OR DAMAGES ARISING OUT OF THIS PUBLIC LICENSE OR
     USE OF THE LICENSED MATERIAL, EVEN IF THE LICENSOR HAS BEEN
     ADVISED OF THE POSSIBILITY OF SUCH LOSSES, COSTS, EXPENSES, OR
     DAMAGES. WHERE A LIMITATION OF LIABILITY IS NOT ALLOWED IN FULL OR
     IN PART, THIS LIMITATION MAY NOT APPLY TO YOU.

  c. The disclaimer of warranties and limitation of liability provided
     above shall be interpreted in a manner that, to the extent
     possible, most closely approximates an absolute disclaimer and
     waiver of all liability.


Section 6 -- Term and Termination.

  a. This Public License applies for the term of the Copyright and
     Similar Rights licensed here. However, if You fail to comply with
     this Public License, then Your rights under this Public License
     terminate automatically.

  b. Where Your right to use the Licensed Material has terminated under
     Section 6(a), it reinstates:

       1. automatically as of the date the violation is cured, provided
          it is cured within 30 days of Your discovery of the
          violation; or

       2. upon express reinstatement by the Licensor.

     For the avoidance of doubt, this Section 6(b) does not affect any
     right the Licensor may have to seek remedies for Your violations
     of this Public License.

  c. For the avoidance of doubt, the Licensor may also offer the
     Licensed Material under separate terms or conditions or stop
     distributing the Licensed Material at any time; however, doing so
     will not terminate this Public License.

  d. Sections 1, 5, 6, 7, and 8 survive termination of this Public
     License.


Section 7 -- Other Terms and Conditions.

  a. The Licensor shall not be bound by any additional or different
     terms or conditions communicated by You unless expressly agreed.

  b. Any arrangements, understandings, or agreements regarding the
     Licensed Material not stated herein are separate from and
     independent of the terms and conditions of this Public License.


Section 8 -- Interpretation.

  a. For the avoidance of doubt, this Public License does not, and
     shall not be interpreted to, reduce, limit, restrict, or impose
     conditions on any use of the Licensed Material that could lawfully
     be made without permission under this Public License.

  b. To the extent possible, if any provision of this Public License is
     deemed unenforceable, it shall be automatically reformed to the
     minimum extent necessary to make it enforceable. If the provision
     cannot be reformed, it shall be severed from this Public License
     without affecting the enforceability of the remaining terms and
     conditions.

  c. No term or condition of this Public License will be waived and no
     failure to comply consented to unless expressly agreed to by the
     Licensor.

  d. Nothing in this Public License constitutes or may be interpreted
     as a limitation upon, or waiver of, any privileges and immunities
     that apply to the Licensor or You, including from the legal
     processes of any jurisdiction or authority.


=======================================================================

Creative Commons is not a party to its public
licenses. Notwithstanding, Creative Commons may elect to apply one of
its public licenses to material it publishes and in those instances
will be considered the “Licensor.” The text of the Creative Commons
public licenses is dedicated to the public domain under the CC0 Public
Domain Dedication. Except for the limited purpose of indicating that
material is shared under a Creative Commons public license or as
otherwise permitted by the Creative Commons policies published at
creativecommons.org/policies, Creative Commons does not authorize the
use of the trademark "Creative Commons" or any other trademark or logo
of Creative Commons without its prior written consent including,
without limitation, in connection with any unauthorized modifications
to any of its public licenses or any other arrangements,
understandings, or agreements concerning use of licensed material. For
the avoidance of doubt, this paragraph does not form part of the
public licenses.

Creative Commons may be contacted at creativecommons.org.
```

## BSD-3-Clause License (Protocol Buffers)

```
Copyright 2008 Google Inc.  All rights reserved.

Redistribution and use in source and binary forms, with or without
modification, are permitted provided that the following conditions are
met:

    * Redistributions of source code must retain the above copyright
notice, this list of conditions and the following disclaimer.
    * Redistributions in binary form must reproduce the above
copyright notice, this list of conditions and the following disclaimer
in the documentation and/or other materials provided with the
distribution.
    * Neither the name of Google Inc. nor the names of its
contributors may be used to endorse or promote products derived from
this software without specific prior written permission.

THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS
"AS IS" AND ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT
LIMITED TO, THE IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR
A PARTICULAR PURPOSE ARE DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT
OWNER OR CONTRIBUTORS BE LIABLE FOR ANY DIRECT, INDIRECT, INCIDENTAL,
SPECIAL, EXEMPLARY, OR CONSEQUENTIAL DAMAGES (INCLUDING, BUT NOT
LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS OR SERVICES; LOSS OF USE,
DATA, OR PROFITS; OR BUSINESS INTERRUPTION) HOWEVER CAUSED AND ON ANY
THEORY OF LIABILITY, WHETHER IN CONTRACT, STRICT LIABILITY, OR TORT
(INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE USE
OF THIS SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.

Code generated by the Protocol Buffer compiler is owned by the owner
of the input file used when generating it.  This code is not
standalone and requires a support library to be linked with it.  This
support library is itself covered by the above license.
```

# Third-party notices

Hirsel adapts code and test vectors from the sources below. Each is used under
the license named in its row; the license texts follow once each.

This is the adapted-source inventory. Release packaging separately derives the
linked dependency inventory and retains upstream license texts under the
bundle’s `notices/` directory; see `tools/toolchain-dist/README.md`.

| Source | Adapted | Where in this tree | Upstream revision | License |
|---|---|---|---|---|
| simdutf8 | Complete-buffer edge vectors and separate safe/forced-ISA test structure (`tests/tests.rs:35-51,113-157`); bounded guard-page and seeded cases added for Hirsel | `crates/hirsel-rt/src/utf8.rs`; linked `simdutf8::basic` dependency is otherwise unmodified | simdutf8 0.1.5, registry checksum `e3a9fe34e3e7a50316060351f37187a3f546bce95496156754b601a5fa71b76e` | MIT OR Apache-2.0; MIT notice in `third-party/LICENSE.simdutf8`; dependency licenses retained by release packaging |
| zmij | `src/lib.rs:1369-1504` direct binary64 shorter candidate and `tests/test.rs:11-115` vectors; portable wide products, regenerated normalized powers and Hirsel shortest-pair/render-policy adapter | `stdlib/std@1/float_decimal.hir`, `shortest_direct_candidate_28` driver controls | dtolnay/zmij `7b7cc48b58028e8af7be87e94c0c1c8936f1a57c` | MIT (full text in `third-party/LICENSE.zmij`) |
| Rayon | Independent fold identity and partition/reduce structure (`src/iter/fold.rs:35-95`), sequential map/reduce equivalence (`src/iter/test.rs:78-128`); fixed contiguous chunks and ordered reduction replace dynamic splitting; mapped +1 cells become CSV input | `examples/csvreduce`, CLI `csvreduce_27` support | rayon-rs/rayon `ee0a00bdb1ab039e178a215ad5712fb7fa58e58f` | MIT (Rust), Copyright (c) 2010 The Rust Project Developers; upstream also Apache-2.0 |
| Go standard library | Complete CSV record API (`src/encoding/csv/reader.go:197-205`) and multiline/CRLF/NoEOL fixture shapes (`reader_test.go:50-100`); remove §/¶ position markers and add integer cells, retaining Hirsel CSV policy | `examples/csvreduce`, CLI `csvreduce_27` support | golang/go `ebe2feefc400f813aefdc8a036e66d2619da6649` | BSD-3-Clause (Go), Copyright 2009 The Go Authors |
| Go standard library | `src/strings/strings.go:184-240,948-967` byte-set search and lookup table; Hirsel scans exact byte extents, including all non-ASCII byte values | `crates/hirsel-kernels/src/byte_scan.rs` | golang/go `dc70946c365d0c23dc2fccc2c3ff94c20c77e3b8` | BSD-3-Clause (Go), Copyright 2009 The Go Authors; full license below |
| rust-lang/regex | `regex-automata/src/nfa/thompson/backtrack.rs:1425-1586,1653-1664,1744-1774,1837-1871` bounded Step/RestoreCapture, numeric slot reuse and visited pairs; precompiled ByteRange/Sparse/Dense transition structure | `stdlib/std@1/regex_capture.hir`, `regex-backtrack-25.hir`; scan-local packed scalar-NFA states and exact ASCII acceptance masks retain the Unicode evaluator | rust-lang/regex `80b9d14e4be9f0b703a742d11c064abce05f210b` | MIT, Copyright (c) 2014 The Rust Project Developers; full notice in `third-party/LICENSE.regex` |
| walkdir | `src/lib.rs:901-930` per-directory cursor reference and `src/tests/recursive.rs:211-254` nested-tree vector, adapted to bounded cap/lookahead trees with global UTF-8 file/link order and explicit directory-read effects | `crates/hirsel-cli/tests/support/filemanifest_stream_25`, `filemanifest_stream_25.rs` | BurntSushi/walkdir `6fd031c82ba5a4204b4ce6eae73dacb00dc072ec` | MIT, Copyright (c) 2015 Andrew Gallant; MIT text below |
| Rust standard library | `library/alloc/src/vec/mod.rs:2995-3031` reserve/copy/publish structure and `library/alloctests/tests/vec.rs:169-180` fixed append vector; adapted to borrowed bytes, COW and bounded-arena preparation | byte append runtime and `byte_append_fastpath_28` controls | rust-lang/rust `f6c1107f8be13e48465e12c0fd26e6a39fe18e4f` | MIT (Rust), The Rust Project Contributors; upstream also Apache-2.0 |
| Rust standard library | `library/alloc/src/string.rs:2848-2858` append value oracle and `library/alloctests/tests/string.rs:378-385` "12345", "2", "2" value vector; Hirsel uses f-strings and concat without adopting Rust addition or ownership rules | `crates/hirsel-driver/tests/fixtures/string_concat_guidance_24.hir` | rust-lang/rust `f6c1107f8be13e48465e12c0fd26e6a39fe18e4f` | MIT (Rust), The Rust Project Contributors |
| Rust standard library | `library/core/src/option.rs:1150-1168` callback-to-Some shape and `library/coretests/tests/option.rs:103-121` Some/None vectors; callbacks return owned Hirsel values instead of references or Rust trait bounds | `crates/hirsel-pipeline-tests/tests/front/generic_result_v_21.rs` | rust-lang/rust `f6c1107f8be13e48465e12c0fd26e6a39fe18e4f` | MIT (Rust), The Rust Project Contributors |
| Rust compiler dataflow | Independent transition/oracle test shape (`compiler/rustc_mir_dataflow/src/framework/tests.rs:245–300`); reviewed fixed-point propagation structure (`framework/mod.rs:288–375`). Adapted to nested source loop/match transitions and independent scalar/vector expectations; no MIR domain or cursor API is ported | `crates/hirsel-pipeline-tests/tests/rc/native_nested_loan_18.rs` | rust-lang/rust `19833af7224424e8b53e288097d56b6af43d5bcf` | MIT (Rust), The Rust Project Contributors |
| rust-lang/regex | Borrowed regex subjects and forward/reverse UTF-8 cursors (`regex-automata/src/util/search.rs:102-130`, `util/utf8.rs:56-96,115-135`), conservative prefix selection, candidate confirmation and vectors (`regex-automata/src/meta/strategy.rs:81-153`, `util/prefilter/mod.rs:584-688`, `hybrid/search.rs:50-90`, `testdata/{misc,regression,iter,utf8,regex-lite,flags}.toml`) | `stdlib/std@1/regex.hir`, `regex_dfa.hir`, `regex_capture.hir`, `regex_prefilter.hir`, regex-prefilter-17 and regex-subject-borrow-25 driver vectors; bounded scalar masks, existing Two-Way search and exact-budget fallback adaptations | rust-lang/regex `80b9d14e4be9f0b703a742d11c064abce05f210b` | MIT (Copyright (c) 2014 The Rust Project Developers; full notice in `third-party/LICENSE.regex`) |
| Rust standard library | `library/std/src/process.rs:2210-2228,2303-2308,2704-2722` opaque ExitCode, SUCCESS/FAILURE, From<u8> and Result return mapping | `stdlib/std@1/process.hir`, PROG-3 launch mapping and exit-code-16 vectors; pure constructor and closed entry types replace traits | rust-lang/rust `afe686de2708699b69dd31ed14be326f4e95edfc` | MIT (Rust) |
| Zig standard library | `lib/std/start.zig:700-739` callMain unsigned-byte return range and error-union success/failure mapping | PROG-3 unsigned byte exit contract and exit-code-16 boundary vectors; bare u8 remains ineligible | ziglang/zig `738d2be9d6b6ef3ff3559130c05159ef53336224` | MIT |
| Go x/net dnsmessage | Header/message model and currOff/newOff bounded name decoder (`dns/dnsmessage/message.go:332-395,2039-2123`); name/message vectors (`message_test.go:188-312,411-484,1517-1672`). Adapted to raw labels, packed flags, strict extents, explicit budgets and uncompressed encoding; all Unknown encoding refused. | `stdlib/std@1/dns_message.hir`, dns-message-15 fixtures and corpus | golang/net `7e562009b3884a0845019b5f886f0242a7c0b970` | BSD-3-Clause (Go), Copyright 2009 The Go Authors; full text below |
| Go standard library | `src/net/mail/message.go:228-1008` mailbox/list/group parsing, CFWS, quoting and canonical formatting; `message_test.go:369-878,1065-1180` independent address vectors; `src/mime/encodedword.go:30-175,190-430` B/Q word encoding/decoding and charset conversion, `encodedword_test.go:20-100` vectors | `stdlib/std@1/mail.hir`, `mail-address-15` fixtures; bounds, typed offsets, strict UTF-8/base64, explicit charset set, injection rejection and validated formatter adaptations in `docs/spec/std-library.md` | golang/go `ebe2feefc400f813aefdc8a036e66d2619da6649` (net/mail), `d934b909f6a50247e2a2e9b89de02cdcef93a163` (mime) | BSD-3-Clause (Go), Copyright 2011, 2015 The Go Authors. All rights reserved. |
| Go standard library | `src/sort/search.go:9-17,58-73` monotonic half-open partition search; `search_test.go:15-62` boundary vectors | `stdlib/std@1/sorted.hir`, sorted-bounds-7 tests | golang/go `ebe2feefc400f813aefdc8a036e66d2619da6649` | BSD-3-Clause (Go), Copyright 2010 The Go Authors. All rights reserved. |
| Rust standard library | `library/core/src/iter/adapters/peekable.rs:13-42,216-219` cached item/end and lazy observation; `library/coretests/tests/iter/adapters/peekable.rs:5-33` vectors | `stdlib/std@1/peekable.hir`, iter-peekable-7 fixtures, `crates/hirsel-pipeline-tests/tests/rc/native_scoped_view_18.rs` | rust-lang/rust `f6c1107f8be13e48465e12c0fd26e6a39fe18e4f` | MIT (Rust) |
| strsim-rs | `src/lib.rs:229-266` scalar Levenshtein one-row recurrence and `985-1026` exact vectors | `stdlib/std@1/edit_distance.hir`, `edit-distance-7` fixtures; full upstream license in `docs/edit-distance.md` | dguo/strsim-rs `dacc84c0dc61eff0ee0ff66962bcf2e17018ad26` | MIT (Copyright (c) 2015 Danny Guo; 2016 Titus Wormer; 2018 Akash Kurdekar) |
| LangArena | `rust/src/benchmarks/cache_simulation.rs:5-158` map, reusable arena slots and doubly linked recency structure | `stdlib/std@1/lru.hir` | kostya/LangArena `3c9f84ddaeec834c0631747cb789e8cfd602f993` | MIT (Copyright (c) 2026 Konstantin Makarchev; full notice in `docs/lru-cache.md`) |
| BuildBuddy | `server/util/lru/lru_test.go:40-103,129-158` unit-count insertion, replacement and oldest controls | `crates/hirsel-driver/tests/fixtures/lru-cache-7/vectors.hir` | buildbuddy-io/buildbuddy `51a570d9a2e641262e5912b6ccff7336b1b734ad` | MIT (Copyright (c) 2020-present Iteration, Inc.; full notice in `docs/lru-cache.md`) |
| Rust standard library | `library/core/src/slice/mod.rs:4280-4388` fixed-length checked copying and `library/coretests/tests/slice.rs:2030-2088` overlap, empty and invalid-range vectors | `bytes.replace_range` runtime, lowering and private parity tests | rust-lang/rust `f6c1107f8be13e48465e12c0fd26e6a39fe18e4f` | MIT (Rust), The Rust Project Contributors; upstream also Apache-2.0 |
| Rust standard library | `library/core/src/slice/sort/stable/{mod,drift,merge,quicksort}.rs`, `shared/{pivot,smallsort,mod}.rs` stable driftsort structure; `library/alloctests/tests/sort/tests.rs:18-22,450-565,599-638` stability, pattern and failure vectors | `stdlib/std@1/list_drift.hir` and `list_copy.hir`, driver drift vectors; adaptations in `docs/prior-art-sort.md` | rust-lang/rust `f6c1107f8be13e48465e12c0fd26e6a39fe18e4f` | MIT (Rust) |
| rust-csv | `src/writer.rs:945-982,1263-1295` empty-record handling and sole-empty-field goldens; `csv-core/src/writer.rs:366-401` terminator decision. Adapted to an opt-in policy for exactly one empty field, preserving zero-field and Go defaults; borrowed rows and growable output replace streaming writer state | `stdlib/std@1/csv.hir`, csv-empty-record-16 fixtures and conformance; `examples/csvjoin/main.hir` opts in | BurntSushi/rust-csv `05612e87e6e92910d40d7214b70952b8434fef9b` | MIT (Copyright (c) 2015 Andrew Gallant; full text in `third-party/LICENSE.csv-core`) |
| csv-core | `csv-core/src/reader.rs:622-700,980-1068,1254-1272` record/field state transitions and ordinary-byte run scan; `1495-1580,1603-1612` boundary and quoted-field vectors | `stdlib/std@1/csv.hir` borrowed slice cursor and `SEM-3-csv-slice-run` | BurntSushi/rust-csv `05612e87e6e92910d40d7214b70952b8434fef9b` (csv-core 0.1.13) | MIT (Copyright (c) 2015 Andrew Gallant; `third-party/LICENSE.csv-core`) |
| aklomp/base64 | `lib/arch/generic/64/enc_loop.c:14-24`, `lib/tables/table_enc_12bit.py:11-26` paired-symbol encode lookup; `lib/arch/generic/32/dec_loop.c:4-20` shifted decode lookups and OR error bit. Hirsel uses checked byte loads, explicit endian-independent shifts, exact stores, both alphabets and its existing fallible suffix parser. | `stdlib/std@1/base64.hir` | aklomp/base64 `e4cc438b7a8f2780b15f88aed24a89927f6bd6c6` | BSD-2-Clause; complete notice in `third-party/LICENSE.aklomp-base64` |
| Rust base64 | `src/engine/general_purpose/mod.rs:151-268` scalar four-block encode; `decode.rs:224-306` eight-symbol/six-byte decode and lookup tables. Hirsel initializes preallocated output, uses checked source loads/stores and retains Go suffix/error precedence | `stdlib/std@1/base64.hir`, `base64-perf-8.hir` boundary vectors | marshallpierce/rust-base64 `5b98ee180cc7c2a64af6cb12c724b1721550e0d0` | MIT (Rust base64) |
| Rust standard library | `library/alloc/src/vec/mod.rs:1801-1825`, `spec_from_elem.rs:12-17`, `raw_vec/mod.rs:935-941`; `library/alloctests/tests/vec.rs:548-575,2327-2338,2803-2807` truncate/drop, filled and maximum-count vectors | `stdlib/std@1/list.hir`, core list tail cleanup and checked reserve, list-truncate-filled-8 conformance and driver controls | rust-lang/rust `f6c1107f8be13e48465e12c0fd26e6a39fe18e4f` | MIT (Rust) |
| clap | `clap_builder/src/parser/parser.rs:78-170` trailing-values scan and `tests/builder/positionals.rs:5-19,43-75` vectors (adaptations in `docs/prior-art-web-small.md`); `clap_builder/src/error/{kind.rs:4-35,context.rs:8-15,mod.rs:434-480,720-747,format.rs:207-233,351-365}` kind/context split and diagnostic wording; `tests/builder/error.rs:84-206,313-346` vectors | `stdlib/std@1/cli.hir`, CLI error context fixtures; adaptations in `docs/cli-errors.md` | clap-rs/clap `ccc89b83875474399e9d9d70841163f6319d1a41` | MIT (Copyright (c) Individual contributors) |
| Goose | `stdlib/json.goose:906–1104` dense pool, child/sibling traversal and iterative parser frame structure; `test/stdlib/stdlib_json.goose:145–218` vectors read alongside Hirsel semantic oracles | `stdlib/std@1/json_document.hir`, scoped JSON conformance | aardappel/goose `8511ae34bdc9aabf77baaab80b4d659ea87835e2` | Apache-2.0 (structure reference) |
| Goose | `src/codegen_frames.h:115–154,210–235,329–350`, `src/codegen_calls.h:245–255` ordered/transitive explicit captures; `test/codegen/funval_body_captures.goose:32–89` nested/repeated/recursive/loop vectors | `hirsel-rc/src/scoped_consumer_9.rs`, scoped consumer pipeline controls; semantic adaptations in `docs/scoped-consumer-9.md` | aardappel/goose `8511ae34bdc9aabf77baaab80b4d659ea87835e2` | Apache-2.0 (structure reference) |
| rr | `src/TraceStream.cc:408-557` binary event/stream framing structure | ordinary and durable tape codec | rr-debugger/rr `50d54f2f541d79b508044fefdb2574fe2ec746f7` | MIT (structure reference) |
| SQLite | `src/wal.c:47-68,990-1022` frame header/payload/checksum structure | shared tape envelope | sqlite/sqlite `049891534ddad2583a33750c05433fcce3648b9c` | Public domain (structure reference) |
| crc32c | `src/lib.rs:49-70`, `src/sw.rs:42-70`, `src/hw_x86_64.rs:49-90` dispatch, slicing-by-8 and hardware CRC structure; published CRC vectors | `hirsel-run-schema/src/tape.rs` | zowens/crc32c `c92fcff3c76aed4f1083894f243981c0df278f7f` | MIT (license in `third-party/LICENSE.crc32c`) |
| bincode | `src/ser/mod.rs:132-136,220-231`, `src/de/mod.rs:247-258,344-351` fixed integer, variant and byte-slice encoding structure | `hirsel-rt/src/tape_wire.rs` | bincode 1.3.3 `c44b5e364e7084cdbabf9f94b63a3c7f32b8fb68` | MIT (structure reference) |
| serde_json | `src/de.rs:255,1820-1860,1919-2045` pull struct/sequence/map decoding; `src/read.rs:521-566` borrowed unescaped strings; `tests/test.rs:1098-1155,1252-1475` lexical and derived decode vectors | `stdlib/std@1/{json,derive}.hir`, `SEM-3-json-{typed-*,reader-*}-run`, `json_skip_small_keys_32.rs` | serde-rs/json `8c535a260ce561eaab4111aafc29038eb9f77ff3` | MIT / Apache-2.0 |
| Rust standard library | `library/core/src/str/validations.rs:123-217` UTF-8 scalar width and continuation ranges, adapted to byte indexing | `stdlib/std@1/json.hir` borrowed UTF-8 validation | rust-lang/rust `a7c37f9f0ee5e93396106e9de20f4a86b4154e63` | MIT (Rust) |
| Rust standard library | `library/core/src/str/validations.rs:146-215`, `error.rs:82-105`, `library/alloctests/tests/string.rs:63-114` validation/error spans and lossy vectors | `stdlib/std@1/utf8.hir` streaming decoder and `utf8-stream-5` fixtures | rust-lang/rust `a7c37f9f0ee5e93396106e9de20f4a86b4154e63` | MIT (Rust) |
| Rust standard library | `library/core/src/str/lossy.rs:201-286` Utf8Chunks scan and `library/coretests/tests/str_lossy.rs:1-72` vectors | `stdlib/std@1/utf8.hir` carry classifier and `utf8-stream-5` fixtures | rust-lang/rust `f6c1107f8be13e48465e12c0fd26e6a39fe18e4f` | MIT (Rust) |
| Go standard library | `src/unicode/utf16/utf16.go:36-58,77-112,125-159` surrogate arithmetic and ordered scan; `utf16_test.go:51-58,133-140` vectors | `stdlib/std@1/utf16.hir`, driver `utf16_8.rs`; strict errors and valid String scalars are Hirsel adaptations | golang/go `ebe2feefc400f813aefdc8a036e66d2619da6649` | BSD-3-Clause (Go) |
| Rust standard library | `library/core/src/str/validations.rs:146-219` valid_up_to/error_len oracle; `library/alloctests/tests/str.rs:878-903` fixed vector identities, composed with copied decoder states and bounded seeded histories | `crates/hirsel-driver/tests/it-stdlib/utf8_stream_histories_26.rs` | rust-lang/rust `f6c1107f8be13e48465e12c0fd26e6a39fe18e4f` | MIT (Rust) |
| Go standard library | `src/unicode/utf8/utf8.go:110-127,171-208` FullRune/DecodeRune, `utf8_test.go:30-67,248-302` vectors | `stdlib/std@1/utf8.hir` carry classifier and `utf8-stream-5-go-vectors.tsv` | golang/go `ebe2feefc400f813aefdc8a036e66d2619da6649` | BSD-3-Clause (Go) |
| Rust standard library | `library/core/src/str/pattern.rs:989-1017` early empty/equal/long needle containment checks | `stdlib/std@1/string.hir`, substring conformance | rust-lang/rust `a7c37f9f0ee5e93396106e9de20f4a86b4154e63` | MIT (Rust) |
| Go standard library | `src/strings/replace.go:550-579` unchanged-run append structure; `src/encoding/json/encode.go:733-739,1180-1183` prequoted static field names | `stdlib/std@1/html.hir`, `stdlib/std@1/derive.hir`, HTML/direct-JSON conformance | golang/go `ebe2feefc400f813aefdc8a036e66d2619da6649` | BSD-3-Clause (Go) |
| Go standard library | `src/encoding/binary/binary.go:69-229` scalar endian structure and `binary_test.go:49-56,75-80,93-98` unsigned byte vectors; bounded source loops, checked offsets and list-builder appends are Hirsel adaptations | `stdlib/std@1/binary.hir`, binary-endian-5 conformance and integration vectors | golang/go `ebe2feefc400f813aefdc8a036e66d2619da6649` | BSD-3-Clause (Go) |
| Go standard library | `src/encoding/binary/varint.go:39-132,134-159` base128 and ZigZag, `varint_test.go:75-227` extrema, overflow and noncanonical vectors; typed absolute offsets, atomic errors and immediate tenth-byte rejection | `stdlib/std@1/varint.hir`, varint-7 conformance and integration vectors | golang/go `ebe2feefc400f813aefdc8a036e66d2619da6649` | BSD-3-Clause (Go) |
| Zig standard library | `lib/std/mem.zig:1953-1985,2078-2120` explicit endian selection and unsigned 16-bit read/write vectors; checked source shifts replace fixed-array casts and byte swaps | `stdlib/std@1/binary.hir`, binary-endian-5 conformance | ziglang/zig `bff490a23889750de7096d9057c0a432efc929a8` | MIT |
| mimalloc | `src/theap.c:124-147` collection also reclaims retired pages; `src/prim/unix/prim.c:549-558` immediate RSS reduction with MADV_DONTNEED | worker collection in `crates/hirsel-rt/src/context.rs`, slab retirement in `alloc.rs` | microsoft/mimalloc `31d034d94cdb8e22f7d7ed55967f581a2d6e831d` | MIT (mimalloc) |
| Rust compiler | `compiler/rustc_data_structures/src/union_find.rs:28-95` parent/rank forest, iterative grandparent compression and snapshot; `union_find/tests.rs:1-32` isolated/transitive vectors | `stdlib/std@1/union_find.hir`, union-find-9 fixtures | rust-lang/rust `15c10447a46529b68e38c7f138aeb95240e4d7d6` | MIT (Rust) |
| Rust compiler | `compiler/rustc_index/src/bit_set.rs:73-180,243-258,320-336,1594-1644` dense words, membership, algebra and tail masking; `bit_set/tests.rs:8-87` boundary/union vectors | `stdlib/std@1/bitset.hir`, bitset-7 fixtures | rust-lang/rust `19833af7224424e8b53e288097d56b6af43d5bcf` | MIT (Rust) |
| Rust compiler | restricted Damerau-Levenshtein distance and name matching thresholds (`compiler/rustc_span/src/edit_distance.rs:24-104,235-334`) and test vectors (`edit_distance/tests.rs`) | `crates/hirsel-diag/src/near_miss.rs` | rust-lang/rust `a7c37f9f0ee5e93396106e9de20f4a86b4154e63` | MIT (Rust) |
| Buck2 allocative | Arc allocation layout and pointer-identity deduplication (`allocative/allocative/src/impls/std/sync.rs:37-78`), Vec live and spare capacity accounting (`unsorted.rs:86-99`); Hirsel walks retained TIR payloads without visiting session interners | `crates/hirsel-types/src/path.rs`, `crates/hirsel-engine/src/memo/retained.rs` | facebook/buck2 `6507dd157a6f81a810c48583edf1758dd0c337c5` | MIT |
| Go compiler | Worker-local function compilation and completed-IR lifetime structure (`src/cmd/compile/internal/gc/compile.go:166-184`); Hirsel uses ordered `jobs::map` results, local phase products and serial DefPath memo publication | `crates/hirsel-engine/src/queries/parallel.rs` and `parallel/compact.rs`, engine concurrent-checker and serial-versus-worker controls | golang/go `ebe2feefc400f813aefdc8a036e66d2619da6649` | BSD-3-Clause (Go) |
| Salsa | Separate input-change, verification and memo-product lifetime structure (`src/function/maybe_changed_after.rs:339-390`, `backdate.rs:12-46`, `memo.rs:115-122,206-223`); Hirsel uses its reverse subscriber index, body-stable signature snapshots, compact program products and weak producer identity for retained frozen rows and canonical package-universe input receipts | `crates/hirsel-engine/src/{snapshot,queries}`, snapshot/body/header/private-view and phase-product controls | salsa-rs/salsa `30b614d826d697c47bc0f21591fab218f2004032` | MIT |
| Rust standard library | Owner-lifetime once-only initialization (`library/std/src/sync/once_lock.rs:319-352`) through `OnceLock::get_or_init`; no source copied | `crates/hirsel-engine/src/representation.rs` and `snapshot/mod.rs`, private-view and deferred-universe receipt controls | rust-lang/rust `f6c1107f8be13e48465e12c0fd26e6a39fe18e4f` | MIT |
| Go runtime | cgroup-v2 membership/mount/CPU-limit reader structure (`src/internal/runtime/cgroup/cgroup_linux.go:116-140,201-237`), quota parser and `TestParseV2Limit` vectors (`cgroup.go:72-107`, `cgroup_test.go:74-155`) | `crates/hirsel-run-config/src/managed_cores`, managed-core integration tests | golang/go `ebe2feefc400f813aefdc8a036e66d2619da6649` | BSD-3-Clause (Go) |
| h2 | Stream id index, dense storage removal and active-stream count transitions (`src/proto/streams/store.rs:11-15,102-128,402-418`, `counts.rs:281-330`) | `stdlib/std@1/http2.hir`, indexed retirement corpus | hyperium/h2 `e64c1662ebb14794fc19cbcbb5b5dd0f67925f27` | MIT (Copyright (c) 2017 h2 authors) |
| hyper | HTTP/1 inline/persistent dispatch, reused response storage, owned read buffer, keep-alive and pipelined-request vectors (`src/proto/h1/{conn,dispatch,io,role}.rs`, `tests/server.rs`) | `stdlib/std@1/{http_h2,http1_parse}.hir`, `conformance/SEM/SEM-1-http-server-{inline,serial-handoff}-run.hir` | hyperium/hyper `b07337878fe57b34b2065335ae8cae9ce5a780a0` | MIT (Copyright (c) 2014-2026 Sean McArthur) |
| xsum | Signed overlapping small accumulator, deferred carries and complete one/two/three/ten-term tables | `stdlib/std@1/superacc.hir`, `native-tests/library/vectors/xsum.json` | radfordneal/xsum `b766b3e104cdbd6769c8df033fb430700fce32dd` | MIT, Copyright 2015, 2018, 2021, 2024 Radford M. Neal; full text in `native-tests/library/vectors/LICENSE.xsum` |
| CPython | Named predecessor usage and singleton, reverse-chain, disconnected-chain vectors from `Lib/graphlib.py:56–85` and `Lib/test/test_graphlib.py:40–57`. Explicit string declarations, strict duplicate/unknown rejection, caps and complete blocked reports replace the incremental Python API. | `examples/depplan`, `crates/hirsel-cli/tests/support/depplan_23/verify.py` | python/cpython `8b7d6c53eee2226b08bed761d5a1742b5adb8507` | PSF-2.0; Copyright (c) 2001 Python Software Foundation; All Rights Reserved; full license below |
| CPython | `Lib/graphlib.py:56–85,125–135,189–200` successor counts and ready queue; `Lib/test/test_graphlib.py:39–137` graph vectors. Adapted to dense IDs, explicit caps, one-shot FIFO and sorted blocked sets; no concurrent API or DFS cycle witness. | `stdlib/std@1/toposort.hir`, `crates/hirsel-driver/tests/it-stdlib/toposort_8.rs` | python/cpython `8b7d6c53eee2226b08bed761d5a1742b5adb8507` | PSF-2.0; Copyright (c) 2001 Python Software Foundation; All Rights Reserved; full license below |
| CPython | All fixed `testFsum` vectors and telescoping case | `native-tests/library/vectors/cpython-fsum.json` | python/cpython `8b7d6c53eee2226b08bed761d5a1742b5adb8507` | PSF-2.0; full text in `native-tests/library/vectors/LICENSE.cpython` |
| CPython / IBM General Decimal Arithmetic and rust_decimal | Selected rounding.decTest radx100..102 and rmux116..117; bankers rounding goldens from tests/decimal_tests/rounding.rs:73-90; finite independent coefficient/scale expectations, exact arithmetic and strict parser adaptations documented with the vectors | `crates/hirsel-driver/tests/fixtures/decimal-upstream-25` | python/cpython `8b7d6c53eee2226b08bed761d5a1742b5adb8507`; paupino/rust-decimal `b23fb077e301e77cb0ac36cc1ac4df1fba6c45d5` | PSF-2.0 / IBM as-is notice retained in fixture NOTICE.txt; MIT, Copyright (c) 2016 Paul Mason; full licenses below |
| CPython / IBM General Decimal Arithmetic | `Lib/test/decimaltestdata/{add,subtract,multiply,divide,quantize}.decTest` (unmodified); `Lib/test/test_decimal.py:297-355` directive/row runner structure | `native-tests/library/vectors/dectest`, `stdlib_bigdecimal_dectest.rs` | python/cpython `b12497756ba0242a2a389dd1bb6fb26ddbd48602` | PSF-2.0; original IBM copyright and as-is notices retained; CPython license accompanies vectors |
| num-bigint | `src/biguint/multiplication.rs:285-431` five-point Toom-3 and Bodrato interpolation | `stdlib/std@1/bigint.hir`, `stdlib_bigint_toom3.rs` | rust-num/num-bigint `41f0b035d12971c5c7f7501917e31f0f6439c26c` | MIT (Copyright (c) 2014 The Rust Project Developers) |
| CPython | `Objects/dictobject.c:35-88,2934-2973` compact entries/index split and dummy deletion | Map/Set native table and interpreter keyed storage | python/cpython `59942e247df0b31207b734ed36c57feff4a6dde2` | Python-2.0 (algorithm reference; no implementation text copied) |
| indexmap | `src/inner.rs:21-38` entries/index split; `tests/quick.rs:82-115` contains, insertion order and insert/remove model vectors | Map/Set table citations, `SEM-5-std-keyed-indexmap-model-run` | indexmap-rs/indexmap `41a870887c4c77adf665886e63df08f406bfe37a` | MIT (algorithm and vector structure) |
| indexmap | `src/map/tests.rs:243-288` present/missing removal and drain-to-empty lifecycle vectors; reduced keys, insertion-order-preserving removal, explicit ownership and counted payloads | `OWN-2-map-loop-validity-17` conformance and pipeline controls | indexmap-rs/indexmap `41a870887c4c77adf665886e63df08f406bfe37a` | MIT, Copyright (c) 2016--2017 (`third-party/LICENSE.indexmap`) |
| Rust standard library | `library/alloc/src/collections/btree/map/tests.rs:571-929` nineteen range tests, adapted to fold semantics | `SEM-5-std-ordered-map-range-*`, driver exhaustive range vectors | rust-lang/rust `f6c1107f8be13e48465e12c0fd26e6a39fe18e4f` | MIT |
| OpenTelemetry Protocol | OTLP JSON examples and protoc descriptor field/enum projection | `tools/tests/fixtures/obs-export`, `tools/obs_export_schema.py` | open-telemetry/opentelemetry-proto `09769935deb8b2ebcf73ad1e2142b691d17bf70b` | Apache-2.0 (license text also in fixture directory) |
| serde_json | `src/ser.rs:620,795` ordered object member encoding and String key serialization | `stdlib/std@1/schema_structural.hir`, String map conformance | serde-rs/json `8c535a260ce561eaab4111aafc29038eb9f77ff3` | MIT / Apache-2.0 |
| serde_path_to_error | `tests/deserialize.rs::test_vec` nested sequence/field rejection path and `src/path.rs` unwind path composition design | `conformance/GEN/GEN-2-schema-boundary-run.hir`, `stdlib/std@1/schema.hir` | dtolnay/path-to-error `7aad3f59acd6d239615ee8b315e0c666915a13bf` | MIT |
| Effect v4 | Paired traversal structure (`SchemaParser.ts:270,633`) and generated round-trip/trim vectors (`test/schema/toDifferJsonPatch.test.ts:27,181`, `Schema.test.ts:5184`), adapted to explicit canonical DTO projections | `stdlib/std@1/schema_checked.hir`, `crates/hirsel-cli/tests/fixtures/schema-laws/main.hir` | Effect-TS/effect `4cddd3c41e71fc4990d37c42b1bcf2ce3780b41d` | MIT (Copyright (c) 2023 Effectful Technologies Inc) |
| Zod | `src/v4/classic/tests/codec.test.ts:392` trim normalization vector, adapted with explicit preservation refusal | `crates/hirsel-cli/tests/fixtures/schema-laws/main.hir` | colinhacks/zod `afdac65e0f80c0ccf9e8909da97c411567908b57` | MIT (Copyright (c) 2020 Colin McDonnell) |
| Effect | Schedule delay composition and exponential/Fibonacci timestamp vectors, adapted to explicit capabilities | `stdlib/std@1/schedule.hir`, schedule conformance | Effect-TS/effect `e95695057b2446d5bc4de81bac0a77ef49d7a4c0` | MIT (Copyright (c) 2023 Effectful Technologies Inc) |
| cenkalti/backoff | Retry classification, elapsed budget and capped exponential vectors | `stdlib/std@1/schedule.hir`, schedule conformance | cenkalti/backoff `6f228bfdb51209aa375869feb27dcede4fa0db06` | MIT (Copyright (c) 2014 Cenk Altı) |
| Rust backoff | Elapsed stop regression and exponential vectors | schedule conformance | ihrwein/backoff `587e2da8fb2dcfc65ca544cb9249022c51f1406e` | MIT (Copyright (c) 2016 Tibor Benke <ihrwein@gmail.com>) |
| mio | Epoll driver and eventfd waker | `crates/hirsel-rt/src/free_driver.rs` | tokio-rs/mio `a88e5e119172e4fb1638926faf65a4fe889e05b1` | MIT (Copyright (c) 2014 Carl Lerche) |
| Tokio | Packed-head worker deque, steal-half and overflow protocol; task wake state and cooperative/LIFO budgets | `crates/hirsel-free/src/{queue,schedule}.rs` | tokio-rs/tokio `ce685ee6916549ed7e52dbfd5430858bbdc734d3` | MIT (Copyright (c) Tokio Contributors) |
| tokio-retry | Saturating delay growth and Fibonacci/cap vectors | `stdlib/std@1/schedule.hir`, schedule conformance | srijs/rust-tokio-retry `69a8766aaead1190c2a305937d45740d6c2d9b33` | MIT (Copyright (c) 2017 Sam Rijs) |
| OpenTelemetry Rust | trace-context extraction/injection validation vectors, adapted to strict W3C wire v00 and explicit malformed-state errors | `conformance/SEM/SEM-3-telemetry-context-run.hir` | open-telemetry/opentelemetry-rust `fb6b53566815062f6be838d49cbea77019669d6f` | Apache-2.0 |
| AWS SigV4 test suite (vendored by botocore) | SigV4Auth canonical query/headers/request, string-to-sign and HMAC derivation structure, S3 path preservation; published canonical requests and Authorization vector assertions | `packages/s3@1/sigv4.hir`, `packages/s3@1/tests/aws-*.hir` | boto/botocore `4d9f375e71c1a97693d961cdefb1827bdaaccfb9` | Apache-2.0 |
| Serde | `serde_derive/src/internals/attr.rs` alias/rename/default policy, `src/de.rs` validation structure and `test_suite/tests/test_annotations.rs` alias/unknown-field vectors | `stdlib/std@1/schema_derive.hir`, `stdlib/std@1/schema_structural.hir`, `conformance/CT/CT-5-schema-derive-run.hir` | serde-rs/serde `0db9b3b504bcb0a7fa9cf001429869519afd3e9a` | MIT / Apache-2.0 |
| serde_path_to_error | `src/path.rs:100` unwind path composition, `tests/deserialize.rs::test_vec` and `test_tuple_variant` vectors | `stdlib/std@1/schema_structural.hir`, `conformance/CT/CT-5-schema-derive-run.hir` | dtolnay/path-to-error `7aad3f59acd6d239615ee8b315e0c666915a13bf` | MIT |
| commonmark.js | CommonMark container matching, inline delimiter/bracket processing and safe HTML rendering; CommonMark 0.31.2 spec examples, regression and bounded pathological vectors | `stdlib/std@1/markdown.hir`, `tools/generate-markdown-hostile.py`, generated `conformance/SEM/SEM-3-markdown-{commonmark,js,safe}*` | commonmark/commonmark.js `898886961b49c1df1175666d03c3ac66c9cc9040` | BSD-2-Clause (implementation); CC-BY-SA-4.0 (spec vectors) |
| cmark-gfm | Generated HTML entity data, autolink scanning, email postprocessing, raw-HTML tag filtering, regression and bounded pathological vectors and GFM table, strikethrough, autolink and task-list spec examples with reference-rendered malformed twins | `stdlib/std@1/markdown{,_render,_footnotes,_tables}.hir`, `tools/generate-markdown.py`, `tools/generate-markdown-hostile.py`, generated `conformance/SEM/SEM-3-markdown-{gfm,cmark}-*` | github/cmark-gfm `27d942c8b0a62d192f616e5bf3578f4b6a89e180` | BSD-2-Clause (implementation/data); CC-BY-SA-4.0 (spec vectors) |
| Rust standard library | `library/alloc/src/raw_vec/mod.rs` grow_amortized doubling policy, bounded by the native host extent | unique heap sequence reservation in `crates/hirsel-rt/src/alloc.rs`, incremental-reservation regression | rust-lang/rust `f6c1107f8be13e48465e12c0fd26e6a39fe18e4f` | MIT |
| Go standard library | `src/html/escape.go:166-181` htmlEscaper five-character replacement table and EscapeString; CR remains unchanged | `stdlib/std@1/html.hir`, HTML escaping conformance | golang/go `ebe2feefc400f813aefdc8a036e66d2619da6649` | BSD-3-Clause (Go) |
| rmp and rmpv | Marker dispatch, big-endian reads, shortest integer/string/collection headers, DOM traversal, and `rmp/tests/func/{encode,decode}` vectors; adapted to the JSON value domain | `stdlib/std@1/{binary_json,msgpack}.hir`, MessagePack conformance, `msgpack-upstream-25` rmpv decode goldens | 3Hren/msgpack-rust `cf880019f72e35a884659a9b29d9eb89b862670b` | MIT (Copyright (c) 2017 Evgeny Safronov) |
| ciborium | `ciborium-ll/src/{dec,hdr,enc,seg}.rs` header dispatch, definite/indefinite containers, exact shortest float widths; `ciborium/tests/codec.rs` RFC 8949 Appendix A vectors; modified for Hirsel's JSON domain and explicit deterministic option | `stdlib/std@1/{binary_json,cbor}.hir`, CBOR conformance and `crates/hirsel-driver/tests/fixtures/cbor-upstream-25/` | enarx/ciborium `00279e48e75ecf9546aae36f6d004447c676065b` | Apache-2.0 (license text below, without the Cranelift LLVM exception) |
| Zig standard library | `lib/std/sort/block.zig:7-104,320-983`, `lib/std/math/sqrt.zig:36-61` cache-free WikiSort structure; `lib/std/sort.zig:178-330,471-513` sort, stability, fuzz and binary-search vectors | `stdlib/std@1/list_block.hir`, `packages/fixtures/list_sort_vectors@1/vectors.hir`, sort conformance | ziglang/zig `bff490a23889750de7096d9057c0a432efc929a8` | MIT |
| Zig standard library | `lib/std/sort/pdq.zig` pivot sampling, equal partition, pattern breaking, bounded recursion and heapsort fallback; arbitrary-range and patterned input vectors | `stdlib/std@1/list.hir`, `packages/fixtures/list_sort_vectors@1`, list sort conformance | ziglang/zig `bff490a23889750de7096d9057c0a432efc929a8` | MIT |
| Rust standard library | `library/core/src/hash/sip.rs:245-335` SipHash-1-3 compression/finalization; `library/coretests/tests/hash/sip.rs:43-132` 64 fixed vectors; integer-root lowering fixes byte width while preserving Hirsel seed, second key and little-endian bytes | `crates/hirsel-{llvm,cranelift}/src/scalar.rs`, `scalar_hash_width_28` pipeline controls | rust-lang/rust `f6c1107f8be13e48465e12c0fd26e6a39fe18e4f` | MIT (Rust), The Rust Project Contributors |
| Rust standard library | `library/core/src/hash/sip.rs` streaming SipHash-1-3 tail buffering and finalization; chunk-boundary tests checked against `DefaultHasher` | `crates/hirsel-kernels/src/siphash.rs`, native composite-key hashing and kernel vectors | rust-lang/rust `f6c1107f8be13e48465e12c0fd26e6a39fe18e4f` | MIT |
| Rust standard library | `library/core/src/hash/sip.rs` SipHash-1-3 state, rounds, `write` and `finish`; `library/core/src/hash/mod.rs` integer, bool, str, slice and tuple `Hash` streams; `library/coretests/tests/hash/sip.rs` test_siphash_1_3 (64 vectors), short-write, idempotence, dropped-byte and concatenation-alias tests | `stdlib/std@1/siphash.hir`, `std@1/derive.hash`, `conformance/SEM/SEM-3-siphash-vectors-run.hir` | rust-lang/rust `f6c1107f8be13e48465e12c0fd26e6a39fe18e4f` | MIT |
| rustc | `compiler/rustc_builtin_macros/src/deriving/hash.rs` and `deriving/generic/mod.rs` `#[derive(Hash)]` field order and multi-variant discriminant; hash vectors computed with rustc 1.99.0 | `std@1/derive.hash`, `conformance/CT/CT-5-std-derive-hash-vectors-run.hir` | rust-lang/rust `a7c37f9f0ee5e93396106e9de20f4a86b4154e63` | MIT |
| serde | `serde_derive/src/ser.rs` externally tagged variant styles, `de/struct_.rs` and `de.rs` missing-field and length messages, `serde_core/src/de/mod.rs` `OneOf`, `Unexpected` and error wording | `std@1/derive.json_value`, `from_json`, `json_into` | serde-rs/serde `0db9b3b504bcb0a7fa9cf001429869519afd3e9a` | MIT |
| redb | `src/transactions.rs:696-717` typed table handles and `tests/integration_tests.rs:3363-3438` range_query/range_query_reversed vectors; adapted to explicit creation, scoped Tx and owned copies | `stdlib/std@1/table.hir`, `crates/hirsel-driver/tests/fixtures/typed_tables/STORE-5-typed-rows-run.hir` | cberner/redb `6b2d24eb9a027a16a46fc8384a24c43e087feb5a` | MIT |
| serde | `serde_derive/src/de/struct_.rs:199-319` field-name dispatch and duplicate/missing-field discipline, adapted to exact storage schemas and canonical payload projection; field reorder and renamed-field vectors | `stdlib/std@1/table_schema.hir`, typed-table conformance | serde-rs/serde `0db9b3b504bcb0a7fa9cf001429869519afd3e9a` | MIT |
| serde_json | `tests/test.rs` test_write_enum, test_parse_enum and test_parse_enum_errors Animal vectors; `src/value/de.rs` enum, variant and array rules and `src/error.rs` `JsonUnexpected` | `std@1/derive.from_json`, `conformance/CT/CT-5-std-derive-serde-enum-run.hir` | serde-rs/json `8c535a260ce561eaab4111aafc29038eb9f77ff3` | MIT |
| rustls | RootCertStore::add trust-anchor decoding, TLS verification-name parsing, `ServerConfig::with_single_cert` identity loading (`CertifiedKey::keys_match`), `WebPkiClientVerifier` client-certificate verification and `ResolvesServerCertUsingSni`-style identity selection, with the existing rsa-2048 ca.cert accepted control | `crates/hirsel-run-schema/src/tls.rs`, TLS input conformance | rustls/rustls `f04804fa13341a9b1dc02b5d1dd8a746f2c4b7fb` (v/0.23.46) | MIT |
| Rust standard library | `library/std/src/net/udp.rs` and `sys/net/connection/socket/unix.rs` bind/sendto/recvfrom conventions, empty datagram and truncation vectors | UDP core operations, host provider and UDP conformance | rust-lang/rust `f6c1107f8be13e48465e12c0fd26e6a39fe18e4f` | MIT |
| Rust standard library | `library/std/src/sys/fs/unix.rs` no-follow descriptor deletion and pinned permission mutation; remove_dir_all symlink and nested vectors; `library/alloc/src/io/read.rs` 8 KiB stream-copy buffer policy | filesystem breadth providers, `stdlib/std@1/file.hir` copy_file and conformance/host replay tests | rust-lang/rust `f6c1107f8be13e48465e12c0fd26e6a39fe18e4f` | MIT |
| Zig standard library | `lib/std/compress/zstd.zig`, `lib/std/compress/zstd/Decompress.zig` frame, FSE, Huffman and repeat-offset decoder and vectors | `stdlib/std@1/zstd.hir`, `conformance/SEM/SEM-3-zstd-*.hir`, `native-tests/library/zstd-large.hir` | ziglang/zig `bff490a23889750de7096d9057c0a432efc929a8` | MIT |
| Zstandard | `lib/common/fse_decompress.c:59-160` FSE_buildDTable_internal table construction; `tests/golden-decompression/`, `tests/golden-decompression-errors/`, `tests/golden-compression/{http,huffman-compressed-larger,large-literal-and-match-lengths}` | Zstandard conformance and `packages/fixtures/zstd@1/` | facebook/zstd `49cf51799ea051457f57243527aa2058f999f005` | BSD-3-Clause (Zstandard) |
| Zstandard | libzstd 1.5.5 compression and dictionary-training outputs, with deterministic HTTP/JSON and varied-length fragments | `packages/fixtures/zstd@1/vectors.hir`, Zstandard conformance | facebook/zstd v1.5.5 | BSD-3-Clause (Zstandard) |
| RFC 8878 | section 4.2 Huffman weights example and appendix A default tables | `stdlib/std@1/zstd.hir`, Zstandard entropy conformance | RFC 8878 (February 2021) | Simplified BSD (IETF Trust) |
| Zig standard library | `lib/std/crypto/{Sha1,sha2}.zig` partial-block / direct full-block / tail streaming update | `stdlib/std@1/{sha1,sha256,sha512,md5}.hir` block update paths and split-boundary conformance | ziglang/zig `bff490a23889750de7096d9057c0a432efc929a8` | MIT |
| Zig standard library | `lib/std/Io/Dir.zig` Walker explicit stack, directory-only descent and recursive traversal vectors | `stdlib/std@1/file.hir` list_recursive, filesystem breadth conformance | ziglang/zig `bff490a23889750de7096d9057c0a432efc929a8` | MIT |
| LLVM libc++ | `libcxx/utils/generate_width_estimation_table.py:42–61,78–99` W/F filter, C++23 width overrides and range compaction; adapted to Unicode16 and existing six-byte Hirsel rows | `stdlib/unicode16/generate_display_width.py`, C++23 width corpus | llvm/llvm-project `89e3afe2b38064b136cfb37a9d07f8c28709a6c4` | Apache-2.0 WITH LLVM-exception |
| Go standard library | `src/unicode/letter.go` range binary search adapted from the existing Unicode property lookup; Unicode16 UCD supplies display-width data and C++23 [format.string.std] supplies the estimate | `stdlib/std@1/display_width.hir`, display-width conformance | golang/go `ebe2feefc400f813aefdc8a036e66d2619da6649` | BSD-3-Clause (Go) |
| Go standard library and vendored cryptobyte | `src/crypto/x509/parser.go:38-154,209-247,465-534,1395-1452`, `pkix/pkix.go:28-104`, `src/net/url/url.go:441-649`, vendored `golang.org/x/crypto/cryptobyte/asn1.go:755-835`; TestPKIXNameString (14 rows + input-preservation assertion), TestRDNSequenceString (1), TestParseASN1String (15), TestDomainNameValid (25) | `stdlib/std@1/x509.hir`, `conformance/SEM/SEM-3-x509-*`; adaptations in `docs/x509-prior-art.md` | golang/go `ebe2feefc400f813aefdc8a036e66d2619da6649` | BSD-3-Clause (Go) |
| Go x/net HTML | `html/token.go:338-1155,1202-1278` tokenizer states/accessors, `html/escape.go:14-220` character references, `html/entity.go` named tables and `html/token_test.go:30-743` vectors | `stdlib/std@1/html_tokenizer.hir`, `crates/hirsel-driver/tests/fixtures/html-tokenizer-15/`, HTML tokenizer conformance | golang/net `7e562009b3884a0845019b5f886f0242a7c0b970` | BSD-3-Clause (Go; fixture `GO-LICENSE.txt`) |
| Go x/net | `http2/hpack/{hpack,encode,huffman,tables,static_table}.go` and decoder, encoder, Huffman test vectors | `stdlib/std@1/hpack*.hir`, HPACK conformance | golang/net `7e562009b3884a0845019b5f886f0242a7c0b970` | BSD-3-Clause (Go) |
| Go standard library | `src/net/http/internal/http2/{frame,transport,server}.go` and `frame_test.go`: frame validation, connection flow control and framing tests | `stdlib/std@1/http2_wire.hir`, `stdlib/std@1/http2.hir`, HTTP/2 conformance | golang/go `ebe2feefc400f813aefdc8a036e66d2619da6649` | BSD-3-Clause (Go) |
| Go standard library | `src/net/http/internal/http2/{transport,server}.go` request/header mapping, DATA streaming and transport/server body vectors | `stdlib/std@1/http_h2.hir`, `conformance/SEM/SEM-1-http-h2-*-run.hir` | golang/go `ebe2feefc400f813aefdc8a036e66d2619da6649` | BSD-3-Clause (Go) |
| Go standard library | `src/net/http/{response_test.go,readrequest_test.go,response.go,request.go,transfer.go}` and `src/net/url/url.go`: HTTP/1 read tables, request target validation and response framing | `stdlib/std@1/http.hir`, `stdlib/std@1/http1_parse.hir`, `stdlib/std@1/http_wire.hir`, `conformance/SEM/SEM-1-http-go-{requests,responses}-run.hir` | golang/go `ebe2feefc400f813aefdc8a036e66d2619da6649` | BSD-3-Clause (Go) |
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
| Rust standard library | `library/core/src/num/imp/flt2dec/strategy/dragon.rs:296-409` integer scaling and half-even cutoff (bounded u128 for precision<=18), `library/core/src/fmt/{mod,num,float}.rs`, `library/alloctests/tests/fmt.rs`, `library/coretests/tests/num/flt2dec/mod.rs:82–165,277–290,344–385,454–538` padding, sign/prefix, numeric precision and all 97 exact-mode vectors; `library/alloc/src/fmt.rs` named `name$` width/precision count grammar | `stdlib/std@1/format.hir`, scalar `format_into_spec`, `text.pad_from`, `crates/hirsel-parser/src/formats.rs`, `conformance/SEM/SEM-3-fstring-*`, `conformance/SEM/SEM-3-format-*` | rust-lang/rust `f6c1107f8be13e48465e12c0fd26e6a39fe18e4f` | MIT |
| Zig standard library | `lib/std/unicode.zig` utf8Encode scalar-boundary cases | UTF-8 conformance (`char.format_into` and `char.from_u32` boundaries) | ziglang/zig `bff490a23889750de7096d9057c0a432efc929a8` | MIT |
| Rust standard library | `library/core/src/str/validations.rs`, `library/alloc/src/string.rs`, `library/alloctests/tests/string.rs` lossy-prefix/error-span semantics and test_from_utf8_lossy vectors | `stdlib/std@1/utf8.hir`, UTF-8 conformance | rust-lang/rust `f6c1107f8be13e48465e12c0fd26e6a39fe18e4f` | MIT |
| Rust standard library | `library/core/src/char/methods.rs` encode_utf8_raw and `char::from_u32`'s scalar test; `library/coretests/tests/char.rs` test_convert and test_encode_utf8 vectors | `stdlib/std@1/char.hir`, `char.from_u32` native legalization, char conformance | rust-lang/rust `f6c1107f8be13e48465e12c0fd26e6a39fe18e4f` | MIT |
| Rust standard library | `library/core/src/str/mod.rs`, `library/alloc/src/str.rs`, `library/alloctests/tests/str.rs` contains, lines and replace algorithms/contracts and multilingual/line-ending vectors | `stdlib/std@1/string.hir`, common string conformance | rust-lang/rust `f6c1107f8be13e48465e12c0fd26e6a39fe18e4f` | MIT |
| Rust standard library | `library/core/src/str/pattern.rs:1362-1633,1735-1770` TwoWaySearcher critical factorization, period memory and byte fingerprint; `library/core/src/str/iter.rs:937-967` reverse split remaining-count state; `library/alloctests/tests/str.rs:54-100,245-264,1484-1540,1566-1586,1708-1730` find, prefix/suffix, split, reverse and whitespace vectors | `stdlib/std@1/substring.hir`, string/bytes search consumers, substring conformance | rust-lang/rust `f6c1107f8be13e48465e12c0fd26e6a39fe18e4f` | MIT |
| Go standard library | `src/bytes/bytes_test.go:1770-1790` all 19 ReplaceTests rows, adapted to replace-all and Hirsel's empty-byte-pattern rejection; `src/bytes/bytes.go:1192-1237` replacement contract | substring Go replacement conformance | golang/go `ebe2feefc400f813aefdc8a036e66d2619da6649` | BSD-3-Clause (Go) |
| Zig standard library | `lib/std/crypto/sha2.zig` portable SHA-256 state, rounds, update/final and empty/abc/long/streaming/aligned vectors | `stdlib/std@1/sha256.hir`, SHA-256 conformance | ziglang/zig `bff490a23889750de7096d9057c0a432efc929a8` | MIT |
| Zig standard library | `lib/std/hash/fnv.zig` FNV-1a 64 offset basis, prime, byte update and empty/"a"/"foobar" vectors | `stdlib/std@1/fnv.hir`, FNV conformance | ziglang/zig `bff490a23889750de7096d9057c0a432efc929a8` | MIT |
| Go / Go supplementary crypto / RFC 2202, 4231, 6070 | Full hash golden tables, 256-row BLAKE2 keyed KATs, MAC and KDF tables; exact file:line sources and adaptations in `native-tests/library/vectors/HASHES.md` | `native-tests/library/vectors/go-*.json`, `rfc-hmac.json`, `stdlib_hash_vectors.rs` | same Go/crypto revisions below; RFC 2202 (1997), 4231 (2005), 6070 (2011) | BSD-3-Clause (Go); RFC reproduction terms below |
| Go standard library | `src/crypto/sha1/{sha1.go,sha1block.go}` and `src/crypto/md5/{md5.go,md5block.go}` generic blocks, constants and padding; `sha1_test.go`/`md5_test.go` golden vectors | `stdlib/std@1/{sha1,md5}.hir`, SHA-1 and MD5 conformance | golang/go `ebe2feefc400f813aefdc8a036e66d2619da6649` | BSD-3-Clause (Go) |
| Go standard library | `src/crypto/internal/fips140/sha256/sha256.go` SHA-224 initial words and `src/crypto/internal/fips140/sha512/{sha512.go,sha512block.go}` constants, generic block, 128-bit length padding and SHA-384/512-256 initial words; `src/crypto/sha256/sha256_test.go` golden224 and `src/crypto/sha512/sha512_test.go` golden384/512/256 vectors | `stdlib/std@1/{sha224,sha384,sha512,sha512_256}.hir`, SHA-2 conformance | golang/go `ebe2feefc400f813aefdc8a036e66d2619da6649` | BSD-3-Clause (Go) |
| Go supplementary crypto | `blake2b/{blake2b.go,blake2b_generic.go}` and `blake2s/{blake2s.go,blake2s_generic.go}` parameter block, keyed first block, held-back final block and G/round schedule; `blake2b_test.go`/`blake2s_test.go` keyed known-answer vectors | `stdlib/std@1/{blake2b,blake2s}.hir`, BLAKE2 conformance | golang/crypto `0ae6809b7559060fe62abe65e26bd18b1fcaf6b7` | BSD-3-Clause (Go) |
| Zig standard library | `lib/std/crypto/keccak_p.zig` and `sha3.zig` Keccak-f[1600] round constants, rotation offsets, sponge padding and SHA-3/SHAKE vectors | `stdlib/std@1/{keccak,sha3_224,sha3_256,sha3_384,sha3_512,shake128,shake256}.hir`, SHA-3 and SHAKE conformance | ziglang/zig `bff490a23889750de7096d9057c0a432efc929a8` | MIT |
| Go standard library | `src/crypto/internal/fips140/sha3/keccakf.go` round structure over the 25 lanes in locals | `stdlib/std@1/keccak.hir` permute | golang/go `ebe2feefc400f813aefdc8a036e66d2619da6649` | BSD-3-Clause (Go) |
| Zig standard library | `lib/std/crypto/chacha20.zig` ChaCha20 and HChaCha20 rounds, the ChaCha20-Poly1305 and XChaCha20-Poly1305 constructions and their seal/open vectors; `poly1305.zig` final reduction | `stdlib/std@1/{chacha_poly,chacha20poly1305,xchacha20poly1305,aead}.hir`, ChaCha20-Poly1305 conformance | ziglang/zig `bff490a23889750de7096d9057c0a432efc929a8` | MIT |
| poly1305-donna | `poly1305-donna-64.h` 44-bit limb multiplication and carry chain | `stdlib/std@1/chacha_poly.hir` Poly1305 | floodyberry/poly1305-donna (2016) | Public domain (MIT where unavailable) |
| Zig standard library | `lib/std/crypto/aes/bitsliced.zig` S-box Boolean circuit; `ghash_polyval.zig` clmulSoft128 carry-less product and reduce; `aes_gcm.zig` AES-256-GCM vectors | `stdlib/std@1/aes_gcm.hir`, AES-GCM conformance | ziglang/zig `bff490a23889750de7096d9057c0a432efc929a8` | MIT |
| Wycheproof | `testvectors_v1/{chacha20_poly1305,xchacha20_poly1305,aes_gcm,hmac_*,pbkdf2_hmac*}_test.json` cases | AEAD, HMAC and PBKDF2 reference suites and complete upstream JSON files in `native-tests/library/vectors/` | C2SP/wycheproof `7a325932231bd7a724c791c8b0d65ea4d6d2f435` | Apache-2.0 |
| RFC 8439 | ChaCha20-Poly1305 (2.8.2) and primitive A.1–A.3 test vectors | ChaCha20-Poly1305 conformance and `native-tests/library/vectors/rfc8439.json` | RFC 8439 (June 2018) | Simplified BSD (IETF Trust) |
| Zig standard library | `lib/std/crypto/25519/{field,curve25519,edwards25519,x25519,ed25519,scalar}.zig` field arithmetic, Montgomery ladder, point formulas and decoding, Ed25519 signing and cofactored verification, and RFC 7748 vectors | `stdlib/std@1/internal/curve25519.hir`, `stdlib/std@1/{x25519,ed25519}.hir`, X25519 and Ed25519 conformance | ziglang/zig `bff490a23889750de7096d9057c0a432efc929a8` | MIT |
| TweetNaCl | `modL` reduction modulo the Ed25519 group order | `stdlib/std@1/internal/curve25519.hir` scalar_reduce and scalar_mul_add | tweetnacl.cr.yp.to 20140427 | Public domain |
| SUPERCOP ref10 Ed25519 | `ge_madd`, `ge_precomp` table layout, `select` and `ge_scalarmult_base` signed radix-16 recoding | `stdlib/std@1/internal/curve25519.hir` base_mul, `stdlib/curve25519/generate.py` and its `curve25519_table.hir` | SUPERCOP 20221122 `crypto_sign/ed25519/ref10` | Public domain |
| Wycheproof | `testvectors_v1/{chacha20_poly1305,xchacha20_poly1305,aes_gcm,x25519,ed25519}_test.json` cases | AEAD Wycheproof conformance, `native-tests/library/crypto-{x25519,ed25519}.hir` and complete 518/151-case `crypto-{x25519,ed25519}-wycheproof.hir` programs | C2SP/wycheproof `7a325932231bd7a724c791c8b0d65ea4d6d2f435` | Apache-2.0 |
| RFC 7748, RFC 8032, RFC 8439 | X25519 (5.2), Ed25519 (7.1) and ChaCha20-Poly1305 (2.8.2) test vectors | X25519, Ed25519 and ChaCha20-Poly1305 conformance | RFC 7748 (January 2016), RFC 8032 (January 2017), RFC 8439 (June 2018) | Simplified BSD (IETF Trust) |
| Go standard library | `src/crypto/internal/fips140/{hmac/hmac.go,hkdf/hkdf.go,pbkdf2/pbkdf2.go}` key padding, keyed-state reuse, HKDF extract/expand and PBKDF2 block loop; `src/crypto/subtle/constant_time.go` ConstantTimeCompare and its test vectors; `src/crypto/hmac/hmac_test.go` FIPS 198 sample; RFC 2202, 4231, 5869, 6070 and 7914 vectors | `stdlib/std@1/{digest,hmac,hkdf,pbkdf2,constant_time}.hir`, HMAC/HKDF/PBKDF2/constant-time conformance and the constant-time trace test | golang/go `ebe2feefc400f813aefdc8a036e66d2619da6649` | BSD-3-Clause (Go) |
| Go standard library | `src/hash/crc32/{crc32.go,crc32_generic.go}` IEEE and Castagnoli slicing-by-8 table construction and update, `src/hash/adler32/adler32.go` nmax-batched sums; `crc32_test.go` and `adler32_test.go` golden vectors | `stdlib/std@1/{crc32,crc32c,adler32}.hir`, checksum conformance | golang/go `ebe2feefc400f813aefdc8a036e66d2619da6649` | BSD-3-Clause (Go) |
| cespare/xxhash | `xxhash.go:72-109` Write, XXH64 primes, lanes, merge rounds and tail finalization; `xxhash_other.go:64-75` portable writeBlocks; `xxhash_test.go:13-67` TestAll seeded and unseeded vectors | `stdlib/std@1/xxhash64.hir`, xxHash64 conformance | cespare/xxhash `ab37246c889f9db16b606fda1c232d659df9271d` | MIT |
| Go standard library | `src/flag/flag.go` FlagSet/parseOne token splitting, explicit values, terminator and error handling; `src/flag/flag_test.go` TestParse, TestParseError, TestInvalidFlags, TestRangeError rows; immutable specs and repeated text values | `stdlib/std@1/cli.hir`, CLI conformance | golang/go `ebe2feefc400f813aefdc8a036e66d2619da6649` | BSD-3-Clause (Go) |
| Go standard library | `src/log/slog/handler.go:272-325,478-493`, `json_handler.go:94-140,185-262`; `src/testing/slogtest/slogtest.go:36-81` (4 cases), `src/log/slog/json_handler_test.go:111-141,154-170` (15+4 vectors); ordered flat JSON records, typed values, escaping and single-write convention | `stdlib/std@1/log.hir`, logging conformance and simulated sink fault tests (Hirsel uses existing JSON and explicit sinks) | golang/go `ebe2feefc400f813aefdc8a036e66d2619da6649` | BSD-3-Clause (Go) |
| Go standard library | `src/testing/quick/quick.go` explicit MaxCount case loop | `stdlib/std@1/test.hir` property_cases, test-helper conformance | golang/go `ebe2feefc400f813aefdc8a036e66d2619da6649` | BSD-3-Clause (Go) |
| Go standard library | `src/internal/diff/diff.go` anchored unified diff (Szymanski's unique-line LCS) and its `testdata/*.txt` | `crates/hirsel-driver/src/testing/diff.rs` and `diff-testdata/` | golang/go `ebe2feefc400f813aefdc8a036e66d2619da6649` | BSD-3-Clause (Go) |
| Go standard library | `src/testing/benchmark.go` launch/predictN calibration: a warm-up run, iteration counts grown toward 1.2 times the target time but at most a hundredfold per step, the 1s default benchtime and the `N iterations, X ns/op` line; adapted to powers of ten timed around whole launches | `crates/hirsel-driver/src/testing.rs` calibrate, `hirsel test --bench` | golang/go `ebe2feefc400f813aefdc8a036e66d2619da6649` | BSD-3-Clause (Go) |
| Rust standard library | `library/coretests/tests/iter/adapters/{filter,take,enumerate,zip}.rs` forward, short-input and limit vectors | `conformance/CLO/CLO-4-std-iter-rust-vectors-run.hir` | rust-lang/rust `f6c1107f8be13e48465e12c0fd26e6a39fe18e4f` | MIT |
| futures-rs | `futures-util/src/stream/{try_stream,stream}/mod.rs` fallible adapter examples and `futures/tests/stream_try_stream.rs` error sequences | `conformance/CLO/CLO-4-std-stream-futures-vectors-run.hir` | rust-lang/futures-rs `706e29bce353e3e72655b99a4a6411291cd4762a` | MIT |
| Erlang/OTP | gen_server call/cast/reply semantics and stopped-server cases, design and vector facts only | `stdlib/std@1/actor.hir`, actor conformance | erlang/otp `cfe4f037932061f9d82a2622cd9a9577ee4f0226` | not adapted (Apache-2.0 upstream) |
| Go x/sync and CPython | errgroup and TaskGroup structured cancellation/join semantics, citation only | `stdlib/std@1/task.hir` | golang/sync `1f920df1e8fcf601d1aaf0f2ea500e1d2ccce105`; python/cpython `114de198c0a05d8169cdbbd8f1e0976dc1319284` | not adapted (BSD-3-Clause and PSF-2.0 upstream) |
| corosensei and Boost.Context | context frame, callee-saved registers and FP control state, citation only | `crates/hirsel-rt/src/fiber.rs` | Amanieu/corosensei `4b5db4845204a4b2663d3f7f75866be4cc3c680d`; boostorg/context `fe7194267df7d8a18d6669200cff110e99a80d74` | not adapted (MIT/Apache-2.0 and BSL-1.0 upstream) |
| minithesis (design reference only) | the choice-sequence shrinker's pass order and the expected minimal counterexamples of `test_minithesis.py` (`[1001]` list sums including drawn lengths, `1, 1000` additive pairs, Hypothesis regressions 1/2 and bounded workout decision trees); Hirsel's implementation is written independently and copies no code | `stdlib/std@1/test.hir` property shrinking, property conformance | DRMacIver/minithesis `b20e7e9b4d7156a186e0a7e78a8c79e9235214a9` | not adapted (MPL-2.0 upstream) |
| SlateDB | the writer claim (a fence object at the next log id created only if absent, then the next manifest with the next epoch), the manifest naming the current snapshot and first log object, and the durable watermark behind awaiting a commit (design only); the transaction manager's structure, snapshot and serializable snapshot conflict checks and recycling rules (`slatedb/src/transaction_manager.rs:13-371`, `db_transaction.rs:109-160,280-320,410-425,547-630`); the `transaction_manager.rs` unit tests (`:419-1092`), the `test_txn_table_driven` cases and conflict tests of `db_transaction.rs` (`:810-1155,1316-1700`) and the `fence.rs` test checks (`:395-600`) as Hirsel cases | `stdlib/std@1/txn_manager.hir`, `stdlib/std@1/store.hir`, `stdlib/std@1/txn.hir`, `conformance/STORE/STORE-2-txn-*-run.hir`, `conformance/DET/DET-1-std-store-fence-chain-run.hir` | slatedb/slatedb `a5c4fd22b36c836a89d8c4f27bf0b7b045e62e32` | Apache-2.0 |
| Rust standard library | `library/core/src/panicking.rs` assert_failed left/right report | `stdlib/std@1/test.hir` assert_eq | rust-lang/rust `f6c1107f8be13e48465e12c0fd26e6a39fe18e4f` | MIT |
| Zig standard library | `lib/std/testing.zig` assertion cleanup semantics | `stdlib/std@1/test.hir` expect_ok and owned Result cleanup conformance | ziglang/zig `bff490a23889750de7096d9057c0a432efc929a8` | MIT |
| Go standard library | `src/unicode/{letter,graphic,digit}.go` range binary search, property meanings and simple scalar conversion | `stdlib/std@1/unicode_properties.hir`, Unicode16 property conformance (tables generated from Unicode16 UCD) | golang/go `ebe2feefc400f813aefdc8a036e66d2619da6649` | BSD-3-Clause (Go) |
| Zig standard library | `lib/std/ascii.zig` charToDigit/radix and ASCII classification boundaries | `stdlib/std@1/ascii.hir` digit_value_radix, shared codec/parser callers and ASCII radix conformance | ziglang/zig `bff490a23889750de7096d9057c0a432efc929a8` | MIT |
| Go standard library | `src/encoding/csv/{reader,writer}.go` reader.go:250-281 line normalization and :297-453 strict quoted-field transitions; writer.go:160-184 fieldNeedsQuotes; reader_test.go readTests (41 applicable rows) and writer_test.go writeTests (26 non-UseCRLF rows, binary strings rejected at UTF-8 boundary). Bounded resumable input, ASCII delimiters, explicit width, UTF-8 fields and terminal errors are Hirsel adaptations; optional Go reader/writer modes are omitted | `stdlib/std@1/csv.hir`, CSV conformance; `examples/csvjson` uses std and its private parser is removed | golang/go `ebe2feefc400f813aefdc8a036e66d2619da6649` | BSD-3-Clause (Go) |
| Go standard library | `src/crypto/internal/fips140/sha256/{sha256.go,sha256block.go}` cross-check of constants, scalar schedule/rounds and big-endian padding/output | `stdlib/std@1/sha256.hir` | golang/go `ebe2feefc400f813aefdc8a036e66d2619da6649` | BSD-3-Clause (Go) |
| Go standard library | `src/compress/flate/{inflate.go,flate_test.go,reader_test.go}`, `src/compress/gzip/{gunzip.go,gunzip_test.go}`, `src/compress/zlib/{reader.go,reader_test.go}` block decoding, code completeness rule, member and header checks, TestStreams, TestNlitOutOfRange, TestTruncatedStreams, huffmanDecoder.init sets, gunzipTests and zlibTests | `stdlib/std@1/flate.hir`, `conformance/SEM/SEM-3-flate-*`, `crates/hirsel-driver/tests/fixtures/flate-large.hir` | golang/go `ebe2feefc400f813aefdc8a036e66d2619da6649` | BSD-3-Clause (Go) |
| Go standard library | `src/archive/tar/{reader.go,writer.go,strconv.go,format.go,common.go}` header parsing and checksums, octal and base-256 numbers, PAX record parsing and formatting, GNU long names, USTAR prefix splitting, PAX header naming and format choice; `reader_test.go` TestReader and `writer_test.go` TestWriter vectors with their `testdata` archives | `stdlib/std@1/tar.hir`, `conformance/SEM/SEM-3-tar-*` | golang/go `ebe2feefc400f813aefdc8a036e66d2619da6649` | BSD-3-Clause (Go) |
| Go standard library | `src/archive/zip/{reader.go,writer.go,struct.go}` end-of-directory search, ZIP64 locator and end records, base-offset recovery, directory and extra-field parsing, data descriptors, checksum checks, UTF-8 flag, MS-DOS time and the writer layout; `reader_test.go` TestReader vectors with their `testdata` archives | `stdlib/std@1/zip.hir`, `conformance/SEM/SEM-3-zip-*`, `crates/hirsel-driver/tests/fixtures/zip-large.hir` | golang/go `ebe2feefc400f813aefdc8a036e66d2619da6649` | BSD-3-Clause (Go) |
| zlib | `contrib/puff/puff.c:235-255,340-379` canonical Huffman decode/construct (Copyright (C) 2002-2013 Mark Adler); `deflate.c` fill_window, longest_match, deflate_fast, deflate_slow, the configuration table and stored-block sizing, and `trees.c` tree construction, length limiting, tree sending and block choice; byte-exact outputs of zlib 1.3 as golden vectors | `stdlib/std@1/flate.hir`, `conformance/SEM/SEM-3-deflate-*`, `crates/hirsel-driver/tests/fixtures/deflate-zlib-native.hir` | madler/zlib `09155eaa2f9270dc4ed1fa13e2b4b2613e6e4851` (v1.3) | Zlib |
| Go standard library | `src/net/http/{cookie.go,cookie_test.go}` ParseCookie, Cookie.String, token and cookie-octet checks, TestParseCookie, writeSetCookiesTests and sanitize vectors | `stdlib/std@1/cookie.hir`, `conformance/SEM/SEM-3-cookie-run.hir` | golang/go `ebe2feefc400f813aefdc8a036e66d2619da6649` | BSD-3-Clause (Go) |
| cookie | `src/lib.rs` `format` test vectors | `conformance/SEM/SEM-3-cookie-run.hir` | cookie 0.18.2 crate release | MIT |
| Go standard library | `src/encoding/base64/{base64.go,base64_test.go}` base64.go:328-408 decodeQuantum quanta, padding/error precedence and Strict offsets; base64_test.go pairs in four modes and all 24 TestDecodeCorrupt rows. Hirsel rejects CR/LF, uses typed errors and returns no partial output | `stdlib/std@1/base64.hir`, base64 conformance | golang/go `ebe2feefc400f813aefdc8a036e66d2619da6649` | BSD-3-Clause (Go) |
| Go standard library | `src/encoding/hex/{hex.go,hex_test.go}` nibble codec and vectors | `stdlib/std@1/hex.hir`, hex conformance | golang/go `ebe2feefc400f813aefdc8a036e66d2619da6649` | BSD-3-Clause (Go) |
| Go standard library | `src/path/path_test.go:18-204` Clean/Dir/Base/Ext tables, mapped to the documented lexical path policy | `conformance/SEM/SEM-3-path-go-run.hir` | golang/go `ebe2feefc400f813aefdc8a036e66d2619da6649` | BSD-3-Clause (Go) |
| Go standard library | `src/path/match.go`, `src/path/match_test.go` slash grammar, scalar classes and matching vectors | `stdlib/std@1/glob.hir`, `conformance/SEM/SEM-3-glob-*.hir` | golang/go `ebe2feefc400f813aefdc8a036e66d2619da6649` | BSD-3-Clause (Go) |
| Zig standard library | `lib/std/Io/Dir.zig` iterate/statFile/createDir/createDirPath/rename/deleteDir contracts | confined filesystem lifecycle providers, `stdlib/std@1/file.hir` | ziglang/zig `bff490a23889750de7096d9057c0a432efc929a8` | MIT |
| Rust standard library | `library/std/src/fs.rs` directory/metadata/rename contracts; `library/std/src/fs/tests.rs` mkdir, recursive mkdir, rename-over-open-file, nonempty-directory and dangling-symlink vectors | filesystem conformance and provider/replay tests | rust-lang/rust `f6c1107f8be13e48465e12c0fd26e6a39fe18e4f` | MIT |
| Rust standard library | `library/std/src/collections/hash/map.rs` length, lookup/removal and ownership contract vectors | Map lifecycle kernels and conformance | rust-lang/rust `f6c1107f8be13e48465e12c0fd26e6a39fe18e4f` | MIT |
| Rust standard library | `library/std/src/collections/hash/set.rs` length and boolean removal contract vectors | Set lifecycle kernels and conformance | same | MIT |
| Zig standard library | `lib/std/array_hash_map.zig` ordered removal and private-index reconstruction | `crates/hirsel-rc/src/library/map.rs`, map/set lifecycle conformance | ziglang/zig `bff490a23889750de7096d9057c0a432efc929a8` | MIT |
| Rust standard library | `library/alloc/src/collections/btree/map.rs` ordered lookup/removal and ownership vectors | SortedMap access kernels and conformance, retaining Hirsel's sequence representation | rust-lang/rust `f6c1107f8be13e48465e12c0fd26e6a39fe18e4f` | MIT |
| Rust standard library | `library/alloc/src/collections/vec_deque/mod.rs` endpoint and move-ownership vectors | `stdlib/std@1/deque.hir`, deque conformance | rust-lang/rust `f6c1107f8be13e48465e12c0fd26e6a39fe18e4f` | MIT |
| Zig standard library | `lib/std/deque.zig:15-29,68-283,374-517,634-710` ring storage, endpoint/growth/slice/iteration vectors and List oracle | `stdlib/std@1/deque.hir`, deque conformance | ziglang/zig `bff490a23889750de7096d9057c0a432efc929a8` | MIT |
| Rust standard library | `library/alloc/src/collections/binary_heap/mod.rs` priority and move-ownership vectors | `stdlib/std@1/priority_queue.hir`, priority-queue conformance | rust-lang/rust `f6c1107f8be13e48465e12c0fd26e6a39fe18e4f` | MIT |
| Zig standard library | `lib/std/priority_queue.zig:17-176,294-483,654-674` stored comparator, peek, bottom-up heapify, ordering and direct sift regression vectors | `stdlib/std@1/{priority_queue,heap_ops}.hir`, priority-queue conformance | ziglang/zig `bff490a23889750de7096d9057c0a432efc929a8` | MIT |
| lexopt | command-line parser dependency | `crates/hirsel-cli-args` | blyxxyz/lexopt 0.3.2 | MIT |
| gimli | DWARF writer dependency | `crates/hirsel-cranelift/src/dwarf.rs` | gimli-rs/gimli 0.33.0 crate release | MIT |
| Cranelift | code generator dependency (`cranelift-codegen`, `-frontend`, `-module`, `-object`) | `crates/hirsel-cranelift` | bytecodealliance/wasmtime cranelift 0.136.3 crate releases | Apache-2.0 WITH LLVM-exception |
| Zig standard library | `lib/std/Uri.zig:23-141,180-260,371-489,541-866` parsing, formatting, resolution and all 22 test groups; numeric grammar tightened to RFC 3986 §§2.1/3.2.3 | `stdlib/std@1/uri.hir`, `crates/hirsel-driver/tests/it-stdlib/stdlib_uri.rs`, `conformance/SEM/SEM-3-uri-zig-*` | ziglang/zig `bff490a23889750de7096d9057c0a432efc929a8` | MIT |
| Zig standard library | `lib/std/Io/net/HostName.zig` validate, fromUri | `stdlib/std@1/net.hir`, `crates/hirsel-run-config` grant validation, their tests | same | MIT |
| Zig standard library | `lib/std/http/Client.zig`, `HeaderIterator.zig`, `ChunkParser.zig`, `http.zig`, `ascii.zig` | `stdlib/std@1/http_wire.hir`, `stdlib/std@1/http.hir`, `crates/hirsel-driver/tests/it-stdlib/stdlib_http_wire.rs`, `crates/hirsel-driver/tests/it/stdlib_http.rs` | same | MIT |
| Zig standard library | `lib/std/ascii.zig` character classes, whitespace and case-conversion vectors | `stdlib/std@1/ascii.hir`, `conformance/SEM/SEM-3-ascii-run.hir` | same | MIT |
| Zig standard library | `lib/std/unicode.zig` utf8Decode2/3/4 | `stdlib/std@1/graphemes.hir` | same | MIT |
| Zig standard library | `lib/std/fs/path.zig`, `lib/std/time/epoch.zig` | std@1 path sources and tests; epoch.zig RFC 3339 test vectors | same | MIT |
| Go standard library (duration text) | `src/time/format.go:1555-1731` leadingInt, leadingFraction, unitMap and ParseDuration; `src/time/time_test.go:945-1043` all 43 success vectors and representable error inputs | `stdlib/std@1/duration.hir`, `conformance/SEM/SEM-1-duration-go-*` | golang/go `7f86f957633ee31a1c17a1eb8452ca7a3d9bc141` | BSD-3-Clause (Go) |
| jiff | `crates/jiff/src/civil/{date,time,datetime}.rs` validation, month arithmetic with day clamping, ISO week dates and the expanded six-digit year form; `crates/jiff/src/fmt/temporal/parser.rs:524-623,768-838` RFC3339 date/clock parsing and leap-second clamping; `crates/jiff/src/fmt/strtime/{printer,parse}.rs` directive semantics (%Y sign padding, %f trimming, %z seconds, case-insensitive names, leap second 60 read as 59) and their test vectors | `stdlib/std@1/time_civil.hir`, `date.hir`, `time_of_day.hir`, `date_time.hir`, `time_format.hir`, civil and strftime conformance | BurntSushi/jiff `fff640d47f72ca6d3a122678315bbd76488306fe` | MIT |
| jiff (time zones) | `crates/jiff-core/src/tz/tzif/{parser,query}.rs` TZif validation, transition clamping and civil gap/fold ranges; `crates/jiff-core/src/tz/posix.rs` POSIX TZ parsing, DST windows, clamping and the permanent-DST case; `crates/jiff/src/tz/timezone.rs` disambiguation and its ambiguity and to-datetime test vectors; TZif test files `crates/jiff/src/tz/testdata/{america-new-york,america-sitka,america-st-johns,antarctica-troll,australia-tasmania,europe-dublin,pacific-honolulu,utc}.tzif` (tz database data, public domain) | `stdlib/std@1/tzif.hir`, `time_zone.hir`, `packages/fixtures/tzif@1`, `conformance/SEM/SEM-1-time-zone-*`, `crates/hirsel-driver/tests/fixtures/time-zone-upstream-25` (pinned TZif bytes and Jiff transition/civil vectors) | BurntSushi/jiff `fff640d47f72ca6d3a122678315bbd76488306fe` | MIT |
| Go standard library (time zones) | `src/time/zoneinfo_test.go` TestLoadLocationFromTZDataSlim and TestTzset vectors; slim TZif files `src/time/testdata/{2020b_Europe_Berlin,2021a_America_Nuuk,2021a_Asia_Gaza,2021a_Europe_Dublin}` (tz database data, public domain) | `packages/fixtures/tzif@1/zones.hir`, `conformance/SEM/SEM-1-time-zone-slim-*`, `SEM-1-time-zone-posix-rules-run.hir` | golang/go `ebe2feefc400f813aefdc8a036e66d2619da6649` | BSD-3-Clause (Go) |
| Chrono | NaiveDate serde string adapter and JSON test vectors, `src/naive/date/mod.rs:2568-2692` at `6adaa5240c26fecb7bd9077334a91f8f67f4f3fe` | `stdlib/std@1/date.hir`, `conformance/SEM/SEM-3-date-json-run.hir` | github.com/chronotope/chrono | MIT OR Apache-2.0 |
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
| Rust standard library | `library/core/src/net/{parser,ip_addr,socket_addr}.rs`, `library/coretests/tests/net/` | `stdlib/std@1/net.hir`, `conformance/SEM/SEM-3-net-rust-vectors-run.hir`, `crates/hirsel-driver/tests/fixtures/net-rust-display.hir`, `crates/hirsel-run-config` grant tests, driver tests | same | MIT |
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
| itoa | `src/lib.rs:264-320` digit-pair/four-digit structure for the inline formatter and decimal appender; `src/lib.rs:245-255` ASCII construction proof | `stdlib/std@1/decimal.hir`, primitive-only f-string finish | dtolnay/itoa `1577ed901354d0d7448ac162328f9dbf5183124c` | MIT OR Apache-2.0 |
| Rust standard library | `library/std/src/collections/hash/set.rs:135-150` defer backing allocation until first insertion | `stdlib/std@1/derive.hir` lazy unknown-key tracker | rust-lang/rust `f6c1107f8be13e48465e12c0fd26e6a39fe18e4f` | MIT |
| SmallVec | `src/lib.rs:586-615,674-703` bounded inline storage before spill allocation | `stdlib/std@1/{derive,json}.hir` two inline owned-key slots and hashed overflow; `json_skip_small_keys_32.rs` inline/spill vectors | servo/rust-smallvec `4999262d672a92b1b6a4aab7bef43b3d788f3d83` | MIT |
| Go standard library | `src/html/template/js.go:254-263,336-354` escaping replacement-table structure | `examples/tasks/view.hir` combined JSON/HTML attribute writer; existing JSON-then-HTML spelling retained | golang/go `c0be4674865b06f0be851e27cef93136708be506` | BSD-3-Clause |
| Rust standard library | `library/alloc/src/fmt.rs:650-662`, `library/core/src/fmt/mod.rs:754-806` literal-size estimate and one-buffer formatting; `library/alloc/src/string.rs:1038-1040` construction from known UTF-8 | `crates/hirsel-parser/src/formats.rs`, `crates/hirsel-check/src/body/formats.rs`, `crates/hirsel-rc/src/library/bytes.rs` | rust-lang/rust `19833af7224424e8b53e288097d56b6af43d5bcf` | MIT |
| Rust standard library | integer Display buffer formatting and integer-limit vectors | std@1 integer `to_string`/`format_into` share the decimal appender; `crates/hirsel-pipeline-tests/tests/rc/rc_append_into.rs` checks consumers that only select `to_string` | same | MIT |
| Rust standard library | `library/alloctests/tests/str.rs` `test_is_utf8` accepted boundary vectors | `conformance/SER/SER-2-variable-flat-run.hir` | rust-lang/rust `f6c1107f8be13e48465e12c0fd26e6a39fe18e4f` | MIT |
| unicode-segmentation | `src/grapheme.rs`, `src/tables.rs`, `tests/test.rs` | `stdlib/std@1/graphemes.hir` and its tests | unicode-rs/unicode-segmentation `ff47b3187adddb14813e6fb2cfc1a079f58cfddc` | MIT |
| Rust standard library | `library/alloc/src/str.rs` Final_Sigma context rule and `library/alloctests/tests/str.rs` full casing/casefold vectors | `stdlib/std@1/string_case.hir`, Unicode casing/folding conformance | rust-lang/rust `f6c1107f8be13e48465e12c0fd26e6a39fe18e4f` | MIT |
| Unicode Character Database 16.0.0 | data files and generated tables | `stdlib/unicode16/`, `stdlib/std@1/{unicode_tables,unicode_compatibility_tables,string_case_tables,grapheme_tables,unicode_property_tables,display_width_tables}.hir`, `bench/apps/tasks/rust` (same casing/property data) | Unicode 16.0.0 | Unicode, `stdlib/unicode16/LICENSE.txt` |
| serde_json | `src/value/mod.rs:299–310,368–374,431–437,477–499,695–715` and `src/value/index.rs:52–57,85–90`; exact-variant accessor structure and doctest vectors | `stdlib/std@1/json_access.hir`, JSON Value access fixtures and conformance twins | serde-rs/json `20572a6d066958542d79cdd6cf1a7cd9f0c8e1f8` | MIT |
| serde_json | `src/{de,read,ser}.rs`, `tests/test.rs` | std@1 JSON parsing, escaping and writing | serde-rs/json `20572a6d066958542d79cdd6cf1a7cd9f0c8e1f8` | MIT |
| JSONTestSuite | every `test_parsing/*.json` (318 inputs; explicit duplicate-key, implementation-defined and UTF-8 policies) | `tools/generate-json-vectors.py`, `conformance/SEM/SEM-3-json-import-*-run.hir`, `packages/fixtures/json_vectors@1/harness.hir` | nst/JSONTestSuite `1ef36fa01286573e846ac449e8683f8833c5b26a` | MIT |
| CBOR test-vectors / RFC 8949 Appendix A | complete `appendix_a.json` (82 entries, including RFC 7049 bignum examples); JSON-domain decoding and explicit Unsupported checks | `tools/generate-json-vectors.py`, `conformance/SEM/SEM-3-cbor-import-*-run.hir` | cbor/test-vectors `aba89b653e484bc8573c22f3ff35641d79dfd8c1` | Simplified BSD (IETF Trust; RFC examples) |
| MessagePack test-suite | every `src/*.yaml` row / `dist/msgpack-test-suite.json` encoding (194 JSON-domain, 39 Unsupported) | `tools/generate-json-vectors.py`, `conformance/SEM/SEM-3-msgpack-import-*-run.hir` | kawanet/msgpack-test-suite `e04f6edeaae589c768d6b70fcce80aa786b7800e` | MIT |
| saphyr | `parser/src/{scanner,parser,char_traits}.rs` (scanner, simple keys, indentation, block and flow scalars, event state machine); `saphyr/tests/{basic,tags}.rs`, `parser/tests/{issues,basic}.rs` vectors | `stdlib/std@1/yaml.hir`, `conformance/SEM/SEM-3-yaml-*-run.hir`, `crates/hirsel-driver/tests/fixtures/yaml-upstream-25/` | saphyr-rs/saphyr `b8205d5f60ea44f757f7d075fe4f3704bbe5dccc` (v0.2.2) | MIT |
| go-yaml v3 | `decode_test.go` mergeTests, mergeTestsNested and merge rejection vectors | `conformance/SEM/SEM-3-yaml-merge-*-run.hir` | go-yaml/yaml `944c86a7d29391925ed6ac33bee98a0516f1287a` (v3.0.1) | Apache-2.0 |
| yaml-test-suite | every case of the data branch (`in.yaml`, `in.json`, `test.event`, `error`) | `tools/generate-yaml-test.py`, `conformance/SEM/SEM-3-yaml-test-*-run.hir`, `packages/fixtures/yaml_test@1/harness.hir`, `crates/hirsel-driver/tests/fixtures/yaml-tagged-invoice.hir` | yaml/yaml-test-suite `6e6c296ae9c9d2d5c4134b4b64d01b29ac19ff6f` (data-2022-01-17) | MIT |
| toml-test | `tests/files-toml-1.1.0` valid and invalid cases with their tagged JSON | `tools/generate-toml-test.py`, `conformance/SEM/SEM-3-toml-test-*-run.hir`, `packages/fixtures/toml_test@1/harness.hir`, bounded `crates/hirsel-driver/tests/fixtures/toml-upstream-25/{gold-*.golden.json,vectors.json}` | toml-lang/toml-test `ff49d109861c1ad25af53f687f2aef19ab650600` | MIT |
| toml_edit | `tests/testsuite/edit.rs` insertion, removal, array-of-tables and key representation cases; decor ownership and key/string representation choices | `stdlib/std@1/toml.hir`, `conformance/SEM/SEM-3-toml-edit-*-run.hir` | toml-rs/toml `ceac6e4412420d5a5c19e4f7293ad005cf1a31b2` | MIT |
| mimalloc | `src/{alloc,free,page}.c`, `include/mimalloc/types.h`, `test/test-stress.c` | `crates/hirsel-rt/src/alloc.rs` | microsoft/mimalloc `31d034d94cdb8e22f7d7ed55967f581a2d6e831d` | MIT |
| httparse | `src/lib.rs` (request, response, headers, chunk parser and test vectors) | `stdlib/std@1/{http_wire,http1_parse}.hir`, `crates/hirsel-driver/tests/it-stdlib/stdlib_http_wire.rs`, `conformance/SEM/SEM-1-http-httparse-vectors-run.hir` | seanmonstar/httparse `a0fa552e4e0fa9a5914a736ef3a556748fef709b` | MIT |
| rustls | `tests/api.rs` vectors (client and server close_notify, server ALPN, client auth, SNI), `test-ca/rsa-2048/{ca.cert,end.fullchain,end.key}` | `crates/hirsel-rt/tests/it/tls.rs`, `crates/hirsel-rt/tests/it/tls_server.rs`, `crates/hirsel-rt/tests/it/tls-data/` | rustls/rustls `f04804fa13341a9b1dc02b5d1dd8a746f2c4b7fb` (v/0.23.46) | MIT |
| ureq | `src/config.rs` defaults | `stdlib/std@1/http.hir` | algesten/ureq `0ebb046e` | MIT |
| Go standard library | `src/regexp/syntax/{parse,compile,prog}.go`, `src/regexp/exec.go` ordered Thompson/Pike compilation and matching; `syntax/parse.go:996-1015,1099-1141,1272-1303` repeat recognition; `find_test.go:29-149` whole spans and `syntax/parse_test.go:464-509` invalid patterns (coverage and exclusions in `reports/regex-vectors.md`) | `stdlib/std@1/regex.hir`, regex conformance, `tools/generate-regex-test.py` | golang/go `ebe2feefc400f813aefdc8a036e66d2619da6649` | BSD-3-Clause (Go) |
| RE2 | `src/regexp/testdata/re2-search.txt` bundled with Go: all in-dialect ordered partial-match rows, imported with source lines and exclusions by `tools/generate-regex-test.py`; Go `exec_test.go:70-200` reader structure | `conformance/SEM/SEM-3-regex-{go,re2}-*-run.hir`, `reports/regex-vectors.md` | fixture in golang/go `ebe2feefc400f813aefdc8a036e66d2619da6649` | BSD-3-Clause (RE2) |
| The Computer Language Benchmarks Game | binarytrees C gcc #2 and #3 | `bench/native/c/binarytrees-arena.h` | benchmarksgame-team.pages.debian.net | BSD-3-Clause |
| Valgrind | `include/valgrind.h`, `memcheck/memcheck.h`, vendored unchanged | `third-party/valgrind/`; their amd64 client request sequence and request codes transcribed in `crates/hirsel-rt/src/memcheck.rs` | Valgrind 3.22.0 (Ubuntu `1:3.22.0-0ubuntu3`) | Valgrind header licence (BSD-style, these two files only) |
| postcard | `de::test_alloc::{vec,zst_vec}`, `de::tests::{de_str,enums}` vectors adapted to SER-2 lengths/tags | `conformance/SER/SER-2-*` | jamesmunns/postcard `872b9082589ebd1384a1b95ca232b1727bcddd29` | MIT |
| bincode | binary serde format dependency for the std precheck file | `crates/hirsel-engine/src/queries/precheck.rs` | bincode 1.3.3 crate release | MIT |
| bincode | `tests/{basic_types,derive}.rs` scalar, UTF-8 and enum-boundary controls adapted to SER-2 | `conformance/SER/SER-2-*` | bincode 2.0.1 crate release | MIT |
| RFC 8949 | Appendix A data examples and sections 4.2, 5.3.1, 5.6 deterministic/validity cases adapted to SER-2 | `conformance/SER/SER-2-*` | RFC 8949 (December 2020) | Simplified BSD (IETF Trust) |
| Go standard library | `src/internal/strconv/{atoi,itoa}.go` cutoff radix loops and radix vectors | `stdlib/std@1/integer.hir`, integer radix conformance | golang/go `ebe2feefc400f813aefdc8a036e66d2619da6649` | BSD-3-Clause (Go) |
| Go standard library | `src/math/all_test.go` IEEE rounding and square-root vectors | std f32/f64 math conformance | golang/go `ebe2feefc400f813aefdc8a036e66d2619da6649` | BSD-3-Clause (Go) |
| Rust libm crate | `libm/src/math/` exp, exp2, expm1, log, log2, log10, log1p, pow, cbrt, hypot, sin, cos, tan, asin, acos, atan, atan2, sinh, cosh, tanh, asinh, acosh, atanh and their `f` forms, kernels, rem_pio2, rem_pio2_large, generic copysign and scalbn; unit-test vectors | `stdlib/std@1/f{32,64}_{exp,power,trig,hyperbolic}.hir`, `stdlib/std@1/libm.hir`, `tools/generate-float-math.py`, `conformance/SEM/SEM-2-std-float-*` | rust-lang/compiler-builtins `f6c1107f8be13e48465e12c0fd26e6a39fe18e4f` | MIT; musl and FreeBSD msun notices below |
| libm-test | `libm-test/src/generate/case_list.rs` MPFR-checked vectors, `src/precision.rs` ULP bounds | `docs/spec/std-library.md` | rust-lang/compiler-builtins `f6c1107f8be13e48465e12c0fd26e6a39fe18e4f` | MIT |
| compiler-builtins | `compiler-builtins/src/float/pow.rs` (`__powidf2`, `__powisf2`) | `tools/generate-float-math.py` powi | rust-lang/compiler-builtins `f6c1107f8be13e48465e12c0fd26e6a39fe18e4f` | MIT |
| Go standard library | `src/os/exec/exec_test.go` TestWaitInterrupt (graceful interrupt, then kill after a delay), TestDedupEnvEcho, TestEnvNULCharacter and TestIgnorePipeErrorOnSuccess (an upstream stage ended by a downstream one that stopped reading); Hirsel refuses a repeated or NUL-bearing variable as invalid input where Go deduplicates or fails at start, and reports the upstream stage's `Signaled(13)` where Go ignores the pipe error | host process tests in `crates/hirsel-rt/tests/it/process.rs` | golang/go `ebe2feefc400f813aefdc8a036e66d2619da6649` | BSD-3-Clause (Go) |
| num-bigint | `src/biguint/{addition,subtraction,multiplication,division,shift,power}.rs` magnitude kernels (Karatsuba step of mac3 and its 32-limb threshold, Knuth D div_rem_core with sub_mul_digit_same_len), the Stein gcd of `src/biguint.rs`; `tests/consts` SUM_TRIPLES, MUL_TRIPLES, DIV_REM_QUADRUPLES and `tests/{bigint,biguint}.rs` conversion and big-division cases | `stdlib/std@1/bigint.hir`, `conformance/SEM/SEM-1-bigint-*` | rust-num/num-bigint `41f0b035d12971c5c7f7501917e31f0f6439c26c` | MIT |
| Go standard library | `src/math/big/{natconv,int}.go` chunked radix conversion and two's-complement bitwise identities; `int_test.go` sumZZ, prodZZ, divisionSignsTests, quoTests, gcdTests, lshTests, rshTests, bitLenTests, expTests, notTests, bitwiseTests and `natconv_test.go` strTests | `stdlib/std@1/bigint.hir`, `conformance/SEM/SEM-1-bigint-go-run.hir` | golang/go `ebe2feefc400f813aefdc8a036e66d2619da6649` | BSD-3-Clause (Go) |
| rust_decimal | `src/ops/{add,mul,div}.rs` scale rules and `src/decimal.rs` rounding strategies, without the 96-bit limit; `tests/decimal_tests/{arithmetic,rounding,scale}.rs` add, subtract, multiply, divide, rounding, normalize and rescale vectors with exact expectations | `stdlib/std@1/bigdecimal.hir`, `conformance/SEM/SEM-1-decimal-*` | paupino/rust-decimal `b23fb077e301e77cb0ac36cc1ac4df1fba6c45d5` | MIT |
| FoundationDB | tuple layer `design/tuple.md` and `bindings/python/fdb/tuple.py`: the order-preserving byte-string escape, sign-bit integer transform, IEEE total-order float transform and `strinc`; the typecode table's test cases (`foo\x00bar`, `F\u00d4O\u0000bar`, `-5551212`, `-42f`) without their type codes | `stdlib/std@1/keys.hir`, `conformance/SEM/SEM-3-keys-*`, `docs/spec/std-library.md` | apple/foundationdb `487620d5aa41152b1f034f4e932005a87eb2947f` | Apache-2.0 (text below, without the LLVM exception) |
| rustc-hash | `src/lib.rs` FxHasher: the rotate-xor-multiply word step and its 64-bit seed, with the 8/4/1-byte tail split | `crates/hirsel-intern/src/lib.rs` `IdHash` (session-id maps and spelling indexes) | rust-lang/rustc-hash `v1.1.0` | MIT |
| Rust standard library | `library/core/src/num/imp/dec2flt/{lemire,table}.rs` Eisel-Lemire algorithm and 128-bit power-of-five table; `library/coretests/tests/num/dec2flt/lemire.rs` halfway, shifted-exponent and boundary vectors; `dec2flt/{decimal_seq,slow}.rs` DecimalSeq and parse_long_mantissa (the exact fallback, also exact fixed-precision formatting) | `stdlib/std@1/float_lemire.hir`, `stdlib/std@1/decimal_seq.hir`, `stdlib/std@1/float_decimal.hir`, `stdlib/std@1/format_round.hir`, `conformance/SEM/SEM-2-float-parse-lemire-vectors-run.hir` | rust-lang/rust `f6c1107f8be13e48465e12c0fd26e6a39fe18e4f` | MIT |
| Ryū | `ryu/{d2s.c,f2s.c,d2s_intrinsics.h,f2s_intrinsics.h,common.h}` shortest binary64/binary32 digits, `ryu/{d2s_full_table.h,f2s_full_table.h}` power-of-five tables; `ryu/tests/{d2s_test.cc,f2s_test.cc}` vectors | `stdlib/std@1/float_decimal.hir`, `native-tests/library/ryu-vectors.txt` | ulfjack/ryu `4c0618b0e44f7ef027ebae05d2cc7812048f7c8f` | Apache-2.0 or BSL-1.0 (Apache-2.0 chosen; text below) |
| Datastar | The v1.0.4 browser bundle, vendored unchanged except for its trailing `sourceMappingURL` line (upstream sha256 `727844adfc825ee651fb93c544a2a739986f9a21820a94524b35f0cac470cf91`) | `examples/tasks/static/datastar.js` | starfederation/datastar `v1.0.4` | MIT |
| Datastar | `sdk/ADR.md` server SDK specification and the Go and Python SDKs' API shape; `sdk/tests/golden` SDK conformance cases, inputs and outputs verbatim, with the Go test runner's event comparison | `packages/datastar@1/`, `conformance/PROG/PROG-1-datastar-sdk-golden.hir` | starfederation/datastar `1efcdc3cb336ec3e9139491604e770e0329657bc` | MIT |
| Zig compiler | `src/libs/glibc.zig` stub shared objects: `.symver` placeholder assembly per versioned symbol, linked as a DSO with the library's SONAME and a version script; `lib/libc/glibc/abilists` read at GLIBC_2.28 to cross-check the stubs | `tools/buck2/native-toolchain/link_kit.py` (the link kit) | ziglang/zig `bff490a23889750de7096d9057c0a432efc929a8` | MIT |
| LLVM | `compiler-rt/lib/builtins/crt{begin,end}.c` and `libunwind/` compiled into the link kit and linked into every executable | link kit (`tools/buck2/native-toolchain/link_kit.py`) | llvm/llvm-project `llvmorg-23.1.4` | Apache-2.0 WITH LLVM-exception (license text below) |
| GNU C Library | glibc 2.28's `Scrt1.o`, `crti.o`, `crtn.o` and `libc_nonshared.a` (Debian 2.28-10+deb10u1) linked into every executable; its libraries' public dynamic symbol tables generate the link kit's stubs | link kit (`tools/buck2/native-toolchain/link_kit.py`) | glibc 2.28 (Debian 2.28-10+deb10u1) | LGPL-2.1-or-later, with glibc's exception granting unlimited permission to link these compiled startup and nonshared files into other programs |
| protobuf-go | `encoding/protowire/wire.go` varint, zigzag, fixed and tag rules and `wire_test.go` vectors; `encoding/protojson` proto3 JSON number, string, bytes, Duration, Timestamp, FieldMask and Any rules | `packages/protobuf@1/{wire,json,tests}.hir`, `packages/protobuf@1/protoc/gen_wkt.hir` | protocolbuffers/protobuf-go `e9ee2a76ab8eef359299e31736cf5cbdbd4d7048` | BSD-3-Clause (Go) |
| prost | `prost/src/encoding.rs` length-delimited merging, packed/unpacked acceptance and depth limit; `prost-build` naming of nested types and oneofs | `packages/protobuf@1/wire.hir`, `packages/protobuf@1/protoc/gen_{model,emit}.hir` | tokio-rs/prost `aed74ad0e844959ee6948f29c4e9cff0f278e89e` | Apache-2.0 (text below, without the LLVM exception) |
| Protocol Buffers | well-known type `.proto` files (embedded), `conformance/conformance.proto`, `src/google/protobuf/test_messages_proto3.proto` and the conformance test runner (built outside the tree) | `packages/protobuf@1/protoc/gen_wkt.hir`, `packages/protobuf@1/wkt.hir`, `packages/protobuf@1/conformance/` | protocolbuffers/protobuf `1d8382d5bdbd06af4e60e4d4e4d53241bfa5dbe8` | BSD-3-Clause (Protocol Buffers) |
| grpc-go | `codes/codes.go`, `internal/transport/http_util.go` status, grpc-message percent-encoding and grpc-timeout rules, `internal/grpcutil/encode_duration.go` and their tests; `interop/test_utils.go` interop cases and server behaviour | `packages/grpc@1/`, `packages/grpc@1/interop/main.hir` | grpc/grpc-go `679b224109d3373eb0e4d80b6e41aafc8d295b6d` | Apache-2.0 (text below, without the LLVM exception) |
| tonic | `interop/proto/grpc/testing/{test,messages,empty}.proto` (gRPC authors) | `packages/grpc@1/interop/proto/`, generated `packages/grpc@1/interop/testing.hir` | hyperium/tonic `cfe80f9d1e5e8e660cd2c1cd3b0a7579df37ba25` | Apache-2.0 (protos, text below); MIT (tonic) |
| gRPC | `examples/protos/helloworld.proto`, with a streaming method added | `examples/grpc-greeter/` | grpc/grpc examples | Apache-2.0 (text below, without the LLVM exception) |
| CPython | `Lib/shlex.py` shlex.read_token in POSIX mode with whitespace_split, quote and join; `Lib/test/test_shlex.py` posix_data and data rows, testQuote, testForceQuote (without force), testJoin and testJoinRoundtrip. Changes from shlex.py: rewritten as a byte scanner of POSIX mode with whitespace_split only (no punctuation_chars, non-POSIX mode or stream input), and an open quote or final backslash is an error value carrying its byte offset where Python raises ValueError | `stdlib/std@1/shlex.hir`, `conformance/SEM/SEM-3-shlex-*` | python/cpython `036d309cf542986a39eeba5ca805b2ec7619731d` | PSF-2.0 |
| google/uuid | `uuid.go` Parse's four spellings, Version and Variant; `uuid_test.go` tests table and TestFromBytes | `stdlib/std@1/uuid.hir`, `conformance/SEM/SEM-3-uuid-*`, `crates/hirsel-driver/tests/fixtures/uuid-upstream-25/` | google/uuid `2d3c2a9cc518326daf99a383f07c4d3c44317e4d` | BSD-3-Clause (google/uuid) |
| uuid | `src/builder.rs` from_random_bytes and from_unix_timestamp_millis layouts; `src/parser.rs` test_parse_valid and test_parse_invalid inputs; `src/v7.rs` test_new and test_sorting | `stdlib/std@1/uuid.hir`, `conformance/SEM/SEM-3-uuid-*`, `crates/hirsel-driver/tests/fixtures/uuid-upstream-25/` | uuid-rs/uuid `276c23628a669ca9fca637f3326c21b114e88049` | MIT |
| semver | `src/{parse,eval,impls,display}.rs` version and requirement parsing, prerelease precedence, comparator matching and display; `tests/test_version.rs` and `tests/test_version_req.rs` vectors | `stdlib/std@1/semver.hir`, `conformance/SEM/SEM-3-semver-*`, `crates/hirsel-driver/tests/fixtures/semver-upstream-25/` | dtolnay/semver `280ebcb6edac3aa4cdc545dbff8a26c5ac4861fe` | MIT |
| node-semver | `classes/range.js` parseRange, replaceCaret, replaceTilde, replaceXRange, hyphenReplace and testSet; `test/fixtures/{range-include,range-exclude,range-parse,comparisons}.js` rows without loose or includePrerelease | `stdlib/std@1/semver.hir`, `conformance/SEM/SEM-3-semver-*`, `crates/hirsel-driver/tests/fixtures/semver-upstream-25/` | npm/node-semver `6e05b7637396ac66522cff8731f07cfe0ef49a29` | ISC |
| imara-diff | `src/myers.rs` and `src/myers/{preprocess,slice,middle_snake}.rs` Myers search with preprocessing and cost cutoff; `src/histogram.rs` and `src/histogram/lcs.rs` histogram diff; `src/lib.rs` common head and tail; `src/postprocess.rs` and `src/util.rs` hunk sliding without a slider heuristic; `src/tests.rs` myers_is_even and myers_is_odd vectors. Changes: worklists in place of recursion, a linked occurrence chain in place of the list pool, i64 positions | `stdlib/std@1/diff.hir`, `conformance/SEM/SEM-3-diff-*`, `crates/hirsel-driver/tests/fixtures/diff-large.hir`, `crates/hirsel-cli/tests/support/patchctl_23/verify.py` (replace and identical-file inputs from `tests/integration/main.rs:26-94`; output checked by an independent application oracle) | pascalkuthe/imara-diff `f02e46a737c43c0d9c65a42e5e08046bd10fd739` | Apache-2.0 (text below, without the LLVM exception) |
| Go standard library | `src/internal/diff/testdata/*.txt` input pairs (expected output from git and imara-diff) | `conformance/SEM/SEM-3-diff-*` | golang/go `ebe2feefc400f813aefdc8a036e66d2619da6649` | BSD-3-Clause (Go) |
| git | `t/lib-diff-alternative.sh` test_diff_frobnitz and test_diff_unique inputs and expected output (test data only; no code adapted) | `conformance/SEM/SEM-3-diff-{pairs-4,context-2,alternative}*` | git/git 2.43.0 | GPL-2.0 (test data) |
| Go standard library | `src/internal/diff/diff.go` tgs unique-line anchoring and longest increasing subsequence; `testdata/*.txt` input pairs (expected output from git) | `stdlib/std@1/diff.hir`, `conformance/SEM/SEM-3-diff-*` | golang/go `ebe2feefc400f813aefdc8a036e66d2619da6649` | BSD-3-Clause (Go) |
| Go supplementary networking | `websocket/hybi.go` hybiFrameReaderFactory and hybiFrameWriter frame header layout, masking and length forms, hybiClientHandshake and hybiServerHandshaker key, accept and upgrade checks | `stdlib/std@1/websocket.hir`, WebSocket conformance | golang/net `7e562009b3884a0845019b5f886f0242a7c0b970` | BSD-3-Clause (Go) |
| Zig standard library | `lib/std/http/Server.zig` respondWebSocket upgrade response, readSmallMessage masked-frame reading (MissingMaskBit) and control-frame handling | `stdlib/std@1/websocket.hir` | ziglang/zig `bff490a23889750de7096d9057c0a432efc929a8` | MIT |
| RFC 6455 | sections 1.3 and 5.7 handshake, accept key and frame examples; section 7.4 close code ranges | `conformance/SEM/SEM-{1,3}-websocket-*`, `crates/hirsel-driver/tests/fixtures/websocket-large.hir` | RFC 6455 (December 2011) | Simplified BSD (IETF Trust code components) |
| Autobahn testsuite | `autobahntestsuite/case/case{1,2,3,4,5,6,7,9}_*.py` framing, ping/pong, reserved bit and opcode, fragmentation, UTF-8, close and limit cases, adapted to scripted clients with RFC 6455's example key | `conformance/SEM/SEM-1-websocket-autobahn-*` | crossbario/autobahn-testsuite `ff462c0a7ccb6512ad8442d09dab371a3c40a843` | Apache-2.0 |
| eventsource-stream | `src/parser.rs` line and field parsing, `src/event_stream.rs` EventBuilder add/dispatch and last-event-id handling; parser test streams | `stdlib/std@1/sse.hir`, Server-Sent Events conformance | jpopesculian/eventsource-stream `3d46f1c758f9ee4681e9da0427556d24c53f9c01` | MIT |
| WHATWG HTML Standard | "Server-sent events" parsing and interpretation rules and its stock-ticker, multi-line, typed-event and id examples | `conformance/SEM/SEM-3-sse-run.hir`, `conformance/SEM/SEM-3-sse-fields-run.hir` | HTML Living Standard (2026) | CC-BY-4.0 (https://creativecommons.org/licenses/by/4.0/) |
| Go standard library | `src/mime/multipart/{multipart.go,writer.go}` NextPart, scanUntilBoundary, matchAfterPrefix, delimiter and LF-mode rules, CreateFormFile/CreateFormField layout, SetBoundary and escapeQuotes; `src/mime/mediatype.go` ParseMediaType, consumeMediaParam and RFC 2231 decoding; `multipart_test.go`, `writer_test.go` and `mediatype_test.go` vectors including greenbytes tc2231 | `stdlib/std@1/multipart.hir`, `conformance/SEM/SEM-3-multipart-*` | golang/go `d934b909f6a50247e2a2e9b89de02cdcef93a163` | BSD-3-Clause (Go) |
| golang-jwt | `parser.go` segment decoding, `hmac.go` SigningMethodHMAC, `ed25519.go:23-78` algorithm registration and signing/verification, `ed25519_test.go:18-113` with `test/ed25519-{private,public}.pem`, `validator.go` exp/nbf/aud/iss checks with leeway; `hmac_test.go` hmacTestData with `test/hmacTestKey`, `hmac_example_test.go` and `example_test.go` tokens | `stdlib/std@1/jwt.hir`, `conformance/SEM/SEM-3-jwt-*`, `native-tests/library/crypto-jwt-eddsa.hir`, `crates/hirsel-driver/tests/fixtures/jwt-clock.hir` | golang-jwt/jwt `d2cffb184933cab5b685db7e9eea56326f0acc0d` | MIT |
| Rust standard library | `library/core/src/str/iter.rs:937-951` SplitN remaining count; `library/alloctests/tests/str.rs:1412-1427,2278-2282` vectors | `stdlib/std@1/string.hir` splitn, `SEM-3-string-splitn-5-*`, driver `string_splitn_5.rs` | rust-lang/rust `a7c37f9f0ee5e93396106e9de20f4a86b4154e63` | MIT (Rust) |
| Go standard library | `src/strings/strings_test.go:453-473` SplitN table, adapted to Rust limits and valid UTF-8/nonempty-separator contracts; `strings.go:274-314` comparison | `SEM-3-string-splitn-5-go-run.hir`, driver `string_splitn_5.rs` | golang/go `ebe2feefc400f813aefdc8a036e66d2619da6649` | BSD-3-Clause (Go) |
| Serde | `serde_derive/src/de.rs:25–75` per-nominal expansion structure; `test_suite/tests/test_annotations.rs:425–462` unknown-before-known/nested-sequence vector | `tools/json-hooks`, `examples/typed-json-kit/main.hir` (existing Hirsel hooks only; extended JSON nested-value vector) | serde-rs/serde `0db9b3b504bcb0a7fa9cf001429869519afd3e9a` | MIT |

| Go standard library | `src/mime/quotedprintable/writer.go:31-168`, `reader.go:30-140`, `writer_test.go:22-118`, `reader_test.go:21-96`: binary token wrapping, compatible line decoder and vectors; whole-buffer/atomic/offset adaptations documented in `docs/quoted-printable.md` | `stdlib/std@1/quoted_printable.hir`, driver `quoted_printable_8.rs` | golang/go `d934b909f6a50247e2a2e9b89de02cdcef93a163` | BSD-3-Clause (Go) |

Where an upstream offers MIT or Apache-2.0, the MIT option is used.

Bounded stdin lines: `stdlib/std@1/input.hir` and the driver
`stdin-line-bound-35` fixtures adapt delimiter-before-full ordering from Go
`src/bufio/bufio.go:338-386` and stride-one/long/remainder vectors from
`bufio_test.go:918-1021` at `ebe2feefc400f813aefdc8a036e66d2619da6649`.
Copyright 2009 The Go Authors. BSD-3-Clause (Go), full license below.
Hirsel's existing MIT input/file/Sheet structure supplies the owned-result and
error-wrapper conventions; its LICENSE remains applicable. The fixture README
lists all cursor, ownership, error and line-ending adaptations.

## Copyright notices

- postcard: Copyright (c) 2019 Anthony James Munns
- bincode: Copyright (c) 2014 Ty Overby
- RFC 8878: Copyright (c) 2021 IETF Trust and the persons identified as the document authors
- RFC 8949: Copyright (c) 2020 IETF Trust and the persons identified as the document authors
- RFC 8439: Copyright (c) 2018 IETF Trust and the persons identified as the document authors
- Wycheproof: Copyright 2016-2026 The Wycheproof Authors
- SlateDB: Copyright the SlateDB authors
- RFC 7748, RFC 8032 and RFC 8439: Copyright (c) 2016, 2017 and 2018 IETF Trust and the persons identified as the document authors
- lexopt: Copyright (c) 2021 Jan Verbeek
- Cranelift: Copyright (c) the Bytecode Alliance and Cranelift contributors
- Zig standard library: Copyright (c) Zig contributors
- LLVM: Copyright (c) LLVM Project contributors
- GNU C Library: Copyright (C) Free Software Foundation, Inc.
- jiff: Copyright (c) 2015 Andrew Gallant
- Rust standard library: Copyright (c) The Rust Project Contributors
- unicode-segmentation: Copyright (c) 2015 The Rust Project Developers
- serde_json: Copyright (c) Serde project contributors
- SmallVec: Copyright (c) 2018 The Servo Project Developers
- JSONTestSuite: Copyright (c) 2016 Nicolas Seriot
- MessagePack test-suite: Copyright (c) 2017-2018 Yusuke Kawasaki
- saphyr: Copyright (c) 2015 Chen Yuheng; Copyright (c) 2023 Ethiraric
- go-yaml v3: Copyright 2011-2016 Canonical Ltd.
- yaml-test-suite: Copyright (c) 2016-2020 Ingy döt Net
- toml-test: Copyright (c) 2018 TOML authors
- toml_edit: Copyright (c) Individual contributors
- clap: Copyright (c) Individual contributors
- mimalloc: Copyright (c) 2018-2025 Microsoft Corporation, Daan Leijen
- h2: Copyright (c) 2017 h2 authors
- hyper: Copyright (c) 2014-2026 Sean McArthur
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
- zlib puff: Copyright (C) 2002-2013 Mark Adler, all rights reserved
- FoundationDB: Copyright 2013-2026 Apple Inc. and the FoundationDB project authors
- rustc-hash: Copyright (c) The Rust Project Developers
- Datastar: Copyright (c) Star Federation
- Protocol Buffers: Copyright 2008 Google Inc. All rights reserved.
- protobuf-go: Copyright (c) 2018 The Go Authors. All rights reserved.
- imara-diff: Copyright (c) Pascal Kuthe and imara-diff contributors
- mio: Copyright (c) 2014 Carl Lerche
- Tokio: Copyright (c) Tokio Contributors
- prost: Copyright Dan Burkert and Tokio contributors
- grpc-go, tonic interop protos and helloworld.proto: Copyright 2014-2019 gRPC authors
- tonic: Copyright 2019 gRPC authors (MIT)
- CPython: Copyright (c) 2001 Python Software Foundation; All Rights Reserved
- google/uuid: Copyright (c) 2009,2014 Google Inc. All rights reserved.
- uuid: Copyright (c) 2014 The Rust Project Developers; Copyright (c) 2018 Ashley
  Mannix, Christopher Armstrong, Dylan DPC, Hunar Roop Kahlon
- semver: Copyright (c) David Tolnay and semver contributors
- node-semver: Copyright (c) Isaac Z. Schlueter and Contributors
- RFC 6455: Copyright (c) 2011 IETF Trust and the persons identified as the document authors
- Autobahn testsuite: Copyright (c) typedef int GmbH
- eventsource-stream: Copyright (c) Julian Popescu
- WHATWG HTML Standard: Copyright (c) WHATWG (Apple, Google, Mozilla, Microsoft)
- golang-jwt: Copyright (c) 2012 Dave Grijalva; Copyright (c) 2021 golang-jwt maintainers

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

## ISC License (node-semver)

```text
Permission to use, copy, modify, and/or distribute this software for any
purpose with or without fee is hereby granted, provided that the above
copyright notice and this permission notice appear in all copies.

THE SOFTWARE IS PROVIDED "AS IS" AND THE AUTHOR DISCLAIMS ALL WARRANTIES
WITH REGARD TO THIS SOFTWARE INCLUDING ALL IMPLIED WARRANTIES OF
MERCHANTABILITY AND FITNESS. IN NO EVENT SHALL THE AUTHOR BE LIABLE FOR
ANY SPECIAL, DIRECT, INDIRECT, OR CONSEQUENTIAL DAMAGES OR ANY DAMAGES
WHATSOEVER RESULTING FROM LOSS OF USE, DATA OR PROFITS, WHETHER IN AN
ACTION OF CONTRACT, NEGLIGENCE OR OTHER TORTIOUS ACTION, ARISING OUT OF OR
IN CONNECTION WITH THE USE OR PERFORMANCE OF THIS SOFTWARE.
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

## RFC vector reproduction

RFC 2202: Copyright (C) The Internet Society (1997). All Rights Reserved.

These documents and translations of them may be copied and furnished to others,
and derivative works that comment on or otherwise explain them or assist in
their implementation may be prepared, copied, published and distributed, in
whole or in part, without restriction of any kind, provided that the above
copyright notice and this paragraph are included on all such copies and
derivative works.

RFC 4231: Copyright (C) The Internet Society (2005).

This document is subject to the rights, licenses and restrictions contained in
BCP 78, and except as set forth therein, the authors retain all their rights.

This document and the information contained herein are provided on an "AS IS"
basis and THE CONTRIBUTOR, THE ORGANIZATION HE/SHE REPRESENTS OR IS SPONSORED BY
(IF ANY), THE INTERNET SOCIETY AND THE INTERNET ENGINEERING TASK FORCE DISCLAIM
ALL WARRANTIES, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO ANY WARRANTY
THAT THE USE OF THE INFORMATION HEREIN WILL NOT INFRINGE ANY RIGHTS OR ANY
IMPLIED WARRANTIES OF MERCHANTABILITY OR FITNESS FOR A PARTICULAR PURPOSE.

The RFC 6070 code components use the Simplified BSD terms reproduced above.
Copyright (c) 2011 IETF Trust and the persons identified as the document authors.

## MIT (Rust)

```text
Copyright (c) The Rust Project Contributors
Copyright (c) 2010 The Rust Project Developers (Rayon)

Permission is hereby granted, free of charge, to any
person obtaining a copy of this software and associated
documentation files (the "Software"), to deal in the
Software without restriction, including without
limitation the rights to use, copy, modify, merge,
publish, distribute, sublicense, and/or sell copies of
the Software, and to permit persons to whom the Software
is furnished to do so, subject to the following
conditions:

The above copyright notice and this permission notice
shall be included in all copies or substantial portions
of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF
ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED
TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A
PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT
SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY
CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION
OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR
IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER
DEALINGS IN THE SOFTWARE.
```

## Apache License 2.0 (Autobahn testsuite)

The Autobahn testsuite is used under the Apache License, Version 2.0, whose terms
are reproduced in the Cranelift section above through "END OF TERMS AND
CONDITIONS"; the LLVM exceptions there do not apply to it.

## Chrono date JSON adapter

Adapted from Chrono (MIT OR Apache-2.0), copyright 2014, Kang Seonghoon.
Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in
all copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN
THE SOFTWARE.

Go byte replacement: `stdlib/std@1/bytes.hir` ports the unmatched/replacement span-copy structure from Go `ebe2feefc400f813aefdc8a036e66d2619da6649`, `src/bytes/bytes.go:1192–1228`; `crates/hirsel-driver/tests/it-stdlib/bytes_replace_bulk_6.rs` adapts compatible all-replacement vectors from `src/bytes/bytes_test.go:1763–1821`. BSD-3-Clause (Go), copyright 2009 The Go Authors; the Go license above applies. Hirsel rejects empty patterns, keeps its prepared search and returns fresh storage; a measured singleton push avoids bulk-call overhead, and the measured extra count/reserve pass is declined.

Base32 port: Copyright 2011 The Go Authors. All rights reserved.
`stdlib/std@1/base32.hir` and `crates/hirsel-driver/tests/fixtures/base32-8/`
adapt Go `src/encoding/base32/base32.go:122-186,312-398` and
`base32_test.go:21-40,349-393` at
`ebe2feefc400f813aefdc8a036e66d2619da6649`, under the BSD-3-Clause (Go)
license reproduced above. See `docs/base32.md` for adaptations.

| Go standard library | `src/net/netip/netip.go:1363-1394,1406-1484` prefix parsing, masking, containment and overlap; `netip_test.go:1526-1618,1673-1739` vectors | `stdlib/std@1/cidr.hir`, driver `cidr_8.rs` | golang/go `ebe2feefc400f813aefdc8a036e66d2619da6649` | BSD-3-Clause (Go), full license above |

Rust bounded tape hashing: `crates/hirsel-rt/src/observation_timing.rs` adapts the fixed-buffer, EOF and interrupted-read structure from rust-lang/rust `1a2c4797ef2570bd76ca5dee34449169bc7dc457`, `library/alloc/src/io/copy/generic.rs:232-254`, with short-read and size vectors from `library/alloctests/tests/io/copy.rs:6-14,16-30`. MIT (Rust), copyright The Rust Project Developers; the Rust MIT license above applies. SHA-256 receives each chunk instead of an output writer; a 64 KiB initialized buffer uses stable Read rather than read_buf.

ASCII85 body port: Copyright 2009 The Go Authors. All rights reserved.
`stdlib/std@1/ascii85.hir` and `crates/hirsel-driver/tests/fixtures/ascii85-9/`
adapt Go `src/encoding/ascii85/ascii85.go:27-83,173-239` and
`ascii85_test.go:18-43,147-167` at
`ebe2feefc400f813aefdc8a036e66d2619da6649`, under the BSD-3-Clause (Go)
license reproduced above. See `docs/ascii85.md` for the ownership, capacity,
forward digit emission and explicit wrapping adaptations.

## MIT (Rust base64)

The MIT License (MIT)

Copyright (c) 2025 Alice Maz, Marshall Pierce

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in
all copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN
THE SOFTWARE.

Package formatting: `crates/hirsel-cli/src/main.rs` adapts the prepare/compare/write
structure from Go `ebe2feefc400f813aefdc8a036e66d2619da6649`,
`src/cmd/gofmt/gofmt.go:238–303`; `crates/hirsel-cli/tests/cli/fmt_manifest_11.rs`
adapts the input/expected-output test structure from `gofmt_test.go:67–115`.
Copyright 2009 The Go Authors; BSD-3-Clause (Go), license above. Hirsel retains
its formatter and syntax, prepares the entire direct-source batch before writes,
skips dependencies, and uses its existing exit codes; Go-specific rewriting,
fragment, diff and flag vectors do not apply.

JSON Patch: `stdlib/std@1/json_patch.hir` adapts json-patch
`src/lib.rs:318-434,435-502`; `crates/hirsel-driver/tests/fixtures/json-patch-15/`
ports the enabled `specs/tests.json` and `specs/spec_tests.json` vectors using the
`tests/suite.rs:1-95` harness structure, at revision
`1b0f707f9f4aad1009f0364f6ca73c5e88cdd85c`. Implementation and vectors MIT,
Copyright (c) 2017 Ivan Dubrov. Full text:
`docs/json-merge-patch-LICENSE-MIT.txt`. Hirsel adaptations are documented in the
JSON Patch section of `docs/spec/std-library.md`.

UTF-8 construction model: `crates/hirsel-driver/tests/fixtures/utf8-construction-model-16/go-scalars.tsv`
adapts the 32 scalar/encoding pairs and two surrogate pairs from Go
`ebe2feefc400f813aefdc8a036e66d2619da6649`, `src/unicode/utf8/utf8_test.go:30-75`.
Copyright 2009 The Go Authors. BSD-3-Clause (Go), full license above.
The host test calls Rust's validator; its rejection-class descriptions refer to
rust-lang/rust `f6c1107f8be13e48465e12c0fd26e6a39fe18e4f`,
`library/core/src/str/validations.rs:128-188`; no Rust validator source is copied.

Ordered lazy regex DFA: `stdlib/std@1/regex_dfa.hir` adapts rust-lang/regex
`80b9d14e4be9f0b703a742d11c064abce05f210b`,
`regex-automata/src/util/determinize/mod.rs:350-490`,
`hybrid/search.rs:13-210`, `hybrid/regex.rs:442-505`,
`hybrid/dfa.rs:2119-2171,3460-3729`. Private driver vectors derive from
`testdata/{regression,flags,iter,utf8,regex-lite,substring}.toml` at that pin.
Copyright (c) 2014 The Rust Project Developers; MIT, full text in
`third-party/LICENSE.regex`. Hirsel keeps its syntax, Go iteration, exact
work-limit admission and scalar VM fallback; one combined primitive cache quits
on first fullness instead of clearing and retrying. Reverse graph traversal
uses the compiled Thompson predecessors rather than a separately compiled HIR.

JSON upstream fixtures: `crates/hirsel-driver/tests/fixtures/json-upstream-25/`
imports numeric and string golden vectors from serde_json `tests/test.rs:1043-1089,1151-1164`
at `20572a6d066958542d79cdd6cf1a7cd9f0c8e1f8`. Copyright (c) Serde project contributors;
MIT, full text above. Number spellings use raw spans; insertion order, duplicate
keys, byte offsets, depth and public-number validation follow Hirsel policy as
recorded in the fixture provenance manifest.

Base64 upstream vectors: `base64-upstream-25/vectors.json` adapts Go
`src/encoding/base64/base64_test.go:25-38` (including RFC 4648 section 10),
revision `ebe2feefc400f813aefdc8a036e66d2619da6649`.
Copyright 2009 The Go Authors. All rights reserved. BSD-3-Clause (Go), full
license above. Generated independent strict-Go expectations and Hirsel policy
dispositions are described in the fixture's `provenance.json`.

CSV upstream fixtures: `crates/hirsel-driver/tests/fixtures/csv-upstream-25/` adapts Go `src/encoding/csv/reader_test.go:40-90,134-153,248-309` and `writer_test.go` at `ebe2feefc400f813aefdc8a036e66d2619da6649` (Copyright 2011 The Go Authors, BSD-3-Clause, license above), and csv-core `csv-core/src/reader.rs:1476-1555,1598-1599` at `05612e87e6e92910d40d7214b70952b8434fef9b` (Copyright (c) 2015 Andrew Gallant, MIT/Unlicense; `third-party/LICENSE.csv-core`). The fixture provenance records strict-error, normalized-limit, UTF-8 and writer-policy adaptations.

Independent regex fixture goldens: `crates/hirsel-driver/tests/fixtures/regex-upstream-25/`
adapts rust-lang/regex `80b9d14e4be9f0b703a742d11c064abce05f210b`,
`testdata/iter.toml:1-77` and `src/regex/string.rs:280-320`.
Copyright (c) 2014 The Rust Project Developers; MIT, full text in
`third-party/LICENSE.regex`. The independent Go span/capture oracle uses
Go `ebe2feefc400f813aefdc8a036e66d2619da6649`,
`src/regexp/regexp.go:753-795`, Copyright 2010-2011 The Go Authors,
BSD-3-Clause (Go), full license above. The fixture provenance records ASCII
class configuration, optional-span representation and syntax dispositions.

Unicode normalization fixtures: `unicode-normalization-upstream-25/vectors.json`
ports `unicode-rs/unicode-normalization` `src/test.rs:19-118` goldens at
`c9921309f09ebd05108920fda92efbf5f8124a7d` (v0.1.24, Unicode 16.0.0).
Copyright (c) 2015 The Rust Project Developers; MIT, full text above.
Selected Unicode16 `NormalizationTest.txt` identities retain original row and
column IDs under the Unicode license in `stdlib/unicode16/LICENSE.txt`.

Unicode casing upstream fixtures: `crates/hirsel-driver/tests/fixtures/unicode-case-upstream-25/`
adapts contextual Final_Sigma vectors from Rust `library/alloctests/tests/str.rs:1864-1899`
at `f6c1107f8be13e48465e12c0fd26e6a39fe18e4f` (Copyright The Rust Project
Developers, MIT, license above), and lower/upper/casefold vectors from CPython
`Lib/test/test_str.py:892-938` at `ebf955df7a89ed0c7968f79faec1de49f61ed7cb`
(Copyright (c) 2001 Python Software Foundation; All Rights Reserved,
PSF-2.0 license above). The independent generated expectations use that CPython
release's Unicode 16.0.0 mappings. Unicode data licensing remains in
`stdlib/unicode16/LICENSE.txt`. The fixture provenance describes the local-slice,
locale-independent full casing and default folding contract; no semantic
adaptation of the upstream answers is applied.

Relative text-view bounds fixtures: `text-view-bounds-26/oracle.tsv` adapts
Rust `library/alloctests/tests/str.rs:774-791,878-903` boundary/vector shapes and
`library/core/src/str/traits.rs:260-284`, `str/validations.rs:146-219` at
`f6c1107f8be13e48465e12c0fd26e6a39fe18e4f` (Copyright The Rust Project
Developers, MIT, license above), and Go `src/unicode/utf8/utf8_test.go:31-107`,
`utf8.go:108-128` at `ebe2feefc400f813aefdc8a036e66d2619da6649`
(Copyright 2009 The Go Authors; BSD-3-Clause, license above). The fixture README
records the independent Rust oracle and Hirsel relative-bound/brand policy.

Base64 acceleration links unmodified **base64-simd =0.8.0** (Nugine, MIT), with
vsimd =0.8.0 and outref =0.5.2 resolved by Cargo.lock. Safe complete-buffer APIs
are base64-simd/src/lib.rs:234-277; strict padding and canonical bits are in
src/decode.rs:30-75,148-170; lazy cached dispatch is vsimd/src/macros.rs:96-133.
The independent scalar test oracle is base64 =0.22.1 (MIT OR Apache-2.0).
Round-trip/comparison structure follows base64-simd/tests/it.rs; Hirsel adds all
four configurations, malformed bytes and exhaustive canonical-tail checks.
The MIT credits are retained in LICENSE.base64-simd (also vsimd), LICENSE.outref
and LICENSE.base64; linked dependency license
notices accompany release packaging.

Bounded JSON text decoding (`json_text_bound_36`) also adapts serde_json
8c535a260ce561eaab4111aafc29038eb9f77ff3 `src/read.rs:521-568,925-1033` and
`tests/test.rs:1099-1167` (MIT OR Apache-2.0, Serde project contributors).
Hirsel adds decoded-byte admission with scan-only overflow, retains its own
error offsets and delayed UTF-8 validation, and never ports lax/WTF-8 decoding.

Fixed capture boundary analysis adapts re2c `src/regexp/fixed_tags.cc:12-163`
and optional/nested controls from `test/codegen/c/nested_fixed_tags.re:11-90`,
commit `8dca815eb424d8ebcb3df805d17504835e14de11`. re2c is in the public domain.
Hirsel uses UTF-8 byte distances and leftmost-first captures; it conservatively
declines tags inside alternatives/repetition, publishes only complete plans,
and does not import POSIX disambiguation or tag histories.


Numeric primitive leaves (`hirsel-kernels/src/numeric.rs`) adapt serde_json
8c535a260ce561eaab4111aafc29038eb9f77ff3 `src/de.rs:462-623` strict number grammar
(MIT OR Apache-2.0), and Go ebe2feefc400f813aefdc8a036e66d2619da6649
`src/internal/strconv/atoi.go:70-162,215-247` digit/cutoff accumulation (BSD-3-Clause).
Hirsel keeps no-plus/no-underscore grammar and Invalid-over-OutOfRange precedence.
Decimal emission directly uses the existing itoa 1.0.18 dependency (MIT OR Apache-2.0),
whose pair-table structure is `src/lib.rs:264-320` at the retained 1577ed9 reference.
Diff line scanning follows imara-diff `src/sources.rs` ByteLines::next: find LF,
include it, and retain a final unterminated line; Hirsel uses its existing bulk
byte search/append and returns owned strings (existing Apache-2.0 notice above).

# Third-party notices

Hirsel adapts code and test vectors from the sources below. Each is used under
the license named in its row; the license texts follow once each.

| Source | Adapted | Where in this tree | Upstream revision | License |
|---|---|---|---|---|
| rustls | RootCertStore::add trust-anchor decoding, TLS verification-name parsing and `ServerConfig::with_single_cert` identity loading (`CertifiedKey::keys_match`), with the existing rsa-2048 ca.cert accepted control | `crates/hirsel-run-config/src/tls.rs`, TLS input conformance | rustls/rustls `f04804fa13341a9b1dc02b5d1dd8a746f2c4b7fb` (v/0.23.46) | MIT |
| Rust standard library | `library/std/src/net/udp.rs` and `sys/net/connection/socket/unix.rs` bind/sendto/recvfrom conventions, empty datagram and truncation vectors | UDP core operations, host provider and UDP conformance | rust-lang/rust `f6c1107f8be13e48465e12c0fd26e6a39fe18e4f` | MIT |
| Rust standard library | `library/std/src/sys/fs/unix.rs` no-follow descriptor deletion and pinned permission mutation; remove_dir_all symlink and nested vectors; `library/alloc/src/io/read.rs` 8 KiB stream-copy buffer policy | filesystem breadth providers, `stdlib/std@1/file.hir` copy_file and conformance/host replay tests | rust-lang/rust `f6c1107f8be13e48465e12c0fd26e6a39fe18e4f` | MIT |
| Zig standard library | `lib/std/Io/Dir.zig` Walker explicit stack, directory-only descent and recursive traversal vectors | `stdlib/std@1/file.hir` list_recursive, filesystem breadth conformance | ziglang/zig `bff490a23889750de7096d9057c0a432efc929a8` | MIT |
| Go standard library | `src/unicode/letter.go` range binary search adapted from the existing Unicode property lookup; Unicode16 UCD supplies display-width data and P1868R2 supplies the estimate | `stdlib/std@1/display_width.hir`, display-width conformance | golang/go `ebe2feefc400f813aefdc8a036e66d2619da6649` | BSD-3-Clause (Go) |
| Go standard library | `src/net/http/transport_test.go` TestTransportKeepAlives, TestTransportConnectionCloseOnResponse, TestTransportRespectRequestWantsClose, TestTransportReadToEndReusesConn and stale-connection controls; adapted to scripted peers | `packages/fixtures/http_client_vectors@1`, `conformance/SEM/SEM-1-http-client-*` | golang/go `ebe2feefc400f813aefdc8a036e66d2619da6649` | BSD-3-Clause (Go) |
| Rust standard library | `library/core/src/panic/location.rs` new_location and constant_location documentation examples, adapted to caught failures and explicit task isolation | caller-attribution conformance | rust-lang/rust `f6c1107f8be13e48465e12c0fd26e6a39fe18e4f` | MIT |
| Rust standard library | `library/core/src/{mem/mod,option}.rs` replace and Option::take implementation/documentation examples, adapted to mutable places | core replace lowering, `stdlib/std@1/option.hir`, replace/remove conformance | rust-lang/rust `f6c1107f8be13e48465e12c0fd26e6a39fe18e4f` | MIT |
| Rust standard library | `library/test/src/lib.rs` filter_tests substring/OR selection and `tests.rs` filter vectors, adapted to explicit @test definitions | `crates/hirsel-driver/src/testing.rs`, CLI test-runner tests | rust-lang/rust `f6c1107f8be13e48465e12c0fd26e6a39fe18e4f` | MIT |
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
| Go standard library | `src/flag/flag.go` FlagSet/parseOne token splitting, explicit values, terminator and error handling; adapted cases use immutable specs and retain repeated values | `stdlib/std@1/cli.hir`, CLI conformance | golang/go `ebe2feefc400f813aefdc8a036e66d2619da6649` | BSD-3-Clause (Go) |
| Go standard library | `src/log/slog/{record,json_handler,handler}.go` structured records, severity filtering, escaped JSON-lines and single-write convention | `stdlib/std@1/log.hir`, logging conformance and simulated sink fault tests (Hirsel uses existing JSON and explicit sinks) | golang/go `ebe2feefc400f813aefdc8a036e66d2619da6649` | BSD-3-Clause (Go) |
| Go standard library | `src/testing/quick/quick.go` explicit MaxCount/generator loop | `stdlib/std@1/test.hir` for_all_cases shared kernel, test-helper conformance | golang/go `ebe2feefc400f813aefdc8a036e66d2619da6649` | BSD-3-Clause (Go) |
| Zig standard library | `lib/std/testing.zig` assertion cleanup semantics | `stdlib/std@1/test.hir` expect_ok and owned Result cleanup conformance | ziglang/zig `bff490a23889750de7096d9057c0a432efc929a8` | MIT |
| Go standard library | `src/unicode/{letter,graphic,digit}.go` range binary search, property meanings and simple scalar conversion | `stdlib/std@1/unicode_properties.hir`, Unicode16 property conformance (tables generated from Unicode16 UCD) | golang/go `ebe2feefc400f813aefdc8a036e66d2619da6649` | BSD-3-Clause (Go) |
| Zig standard library | `lib/std/ascii.zig` charToDigit/radix and ASCII classification boundaries | `stdlib/std@1/ascii.hir` digit_value_radix, shared codec/parser callers and ASCII radix conformance | ziglang/zig `bff490a23889750de7096d9057c0a432efc929a8` | MIT |
| Go standard library | `src/encoding/csv/{reader,writer}.go` quoted-field transitions/fieldNeedsQuotes and `reader_test.go`, `writer_test.go`, `fuzz_test.go` record/quote/error/boundary ideas (Hirsel preserves its existing CSV example dialect) | `stdlib/std@1/csv.hir`, CSV conformance; `examples/csvjson` uses std and its private parser is removed | golang/go `ebe2feefc400f813aefdc8a036e66d2619da6649` | BSD-3-Clause (Go) |
| Go standard library | `src/crypto/internal/fips140/sha256/{sha256.go,sha256block.go}` cross-check of constants, scalar schedule/rounds and big-endian padding/output | `stdlib/std@1/sha256.hir` | golang/go `ebe2feefc400f813aefdc8a036e66d2619da6649` | BSD-3-Clause (Go) |
| Go standard library | `src/compress/flate/{inflate.go,flate_test.go}`, `src/compress/gzip/{gunzip.go,gunzip_test.go}`, `src/compress/zlib/{reader.go,reader_test.go}` block decoding, code completeness rule, member and header checks, TestStreams, TestTruncatedStreams, gunzipTests and zlibTests | `stdlib/std@1/flate.hir`, `conformance/SEM/SEM-3-flate-*`, `crates/hirsel-driver/tests/fixtures/flate-large.hir` | golang/go `ebe2feefc400f813aefdc8a036e66d2619da6649` | BSD-3-Clause (Go) |
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
| Rust standard library | `library/std/src/process.rs` (`Command`, `Output`, `ExitStatus`), `library/std/src/sys/process/unix/common.rs` `read_output` poll loop, `library/std/src/process/tests.rs` smoke, exit/signal status, stdin/stdout, fail-to-start, output and environment vectors | `crates/hirsel-rt/src/process.rs`, `crates/hirsel-rt/tests/it/process.rs`, `crates/hirsel-interp/src/library/process.rs` | rust-lang/rust `f6c1107f8be13e48465e12c0fd26e6a39fe18e4f` | MIT |
| Zig standard library | `lib/std/process.zig` `run` per-stream `stdout_limit`/`stderr_limit` (`StreamTooLong`, child killed) | `proc.run` `max_output` and `OutputLimit()` | ziglang/zig `bff490a23889750de7096d9057c0a432efc929a8` | MIT |
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
| mimalloc | `src/{alloc,free,page}.c`, `include/mimalloc/types.h`, `test/test-stress.c` | `crates/hirsel-rt/src/alloc.rs` | microsoft/mimalloc `31d034d94cdb8e22f7d7ed55967f581a2d6e831d` | MIT |
| httparse | `src/lib.rs` (request, response, headers, chunk parser and test vectors) | `stdlib/std@1/http_wire.hir`, `crates/hirsel-driver/tests/it-stdlib/stdlib_http_wire.rs` | seanmonstar/httparse `a0fa552e4e0fa9a5914a736ef3a556748fef709b` | MIT |
| rustls | `tests/api.rs` vectors (client and server close_notify, server ALPN), `test-ca/rsa-2048/{ca.cert,end.fullchain,end.key}` | `crates/hirsel-rt/tests/it/tls.rs`, `crates/hirsel-rt/tests/it/tls_server.rs`, `crates/hirsel-rt/tests/it/tls-data/` | rustls/rustls `f04804fa13341a9b1dc02b5d1dd8a746f2c4b7fb` (v/0.23.46) | MIT |
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

Where an upstream offers MIT or Apache-2.0, the MIT option is used.

## Copyright notices

- postcard: Copyright (c) 2019 Anthony James Munns
- bincode: Copyright (c) 2014 Ty Overby
- RFC 8949: Copyright (c) 2020 IETF Trust and the persons identified as the document authors
- lexopt: Copyright (c) 2021 Jan Verbeek
- Cranelift: Copyright (c) the Bytecode Alliance and Cranelift contributors
- Zig standard library: Copyright (c) Zig contributors
- jiff: Copyright (c) 2015 Andrew Gallant
- Rust standard library: Copyright (c) The Rust Project Contributors
- unicode-segmentation: Copyright (c) 2015 The Rust Project Developers
- serde_json: Copyright (c) Serde project contributors
- JSONTestSuite: Copyright (c) 2016 Nicolas Seriot
- mimalloc: Copyright (c) 2018-2025 Microsoft Corporation, Daan Leijen
- httparse: Copyright (c) 2015-2026 Sean McArthur
- rustls: Copyright (c) 2016 Joseph Birr-Pixton
- ureq: Copyright (c) 2019 Martin Algesten
- The Computer Language Benchmarks Game: Copyright (c) 2004-2008 Brent Fulgham,
  2005-2025 Isaac Gouy. The binarytrees programs are by Jeremy Zerfas, based on
  the C++ program by Jon Harrop, Alex Mizrahi and Bruno Coutinho.
- Valgrind `valgrind.h` and `memcheck.h`: Copyright (C) 2000-2017 Julian Seward
- Rust libm crate: Copyright (c) 2018 Jorge Aparicio; portions from musl libc,
  Copyright (c) 2005-2020 Rich Felker, et al., and from CORE-MATH (cbrt Copyright
  (c) 2021-2022 Alexei Sibidanov, hypot Copyright (c) 2022 Alexei Sibidanov)
- compiler-builtins: Copyright (c) The Rust Project Contributors

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

## Apache License 2.0 with LLVM Exception (Cranelift)

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

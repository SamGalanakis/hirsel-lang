# Web parity references and adaptations

Pinned sources, cloned for this port:

| Source | Commit | Files and lines |
|---|---|---|
| [Rust](https://github.com/rust-lang/rust) | `f6c1107f8be13e48465e12c0fd26e6a39fe18e4f` | `library/coretests/tests/net/parser.rs:24-150`, `ip_addr.rs:8-183`, `socket_addr.rs:4-72,165-192`; implementation `library/core/src/net/parser.rs` and Display in `ip_addr.rs`/`socket_addr.rs` |
| [Go](https://github.com/golang/go) | `ebe2feefc400f813aefdc8a036e66d2619da6649` | `src/flag/flag.go:1096-1187`, `src/flag/flag_test.go:164-235,652-706,784-816`, `src/html/escape.go:166-180` |
| [grpc-go](https://github.com/grpc/grpc-go) | `679b224109d3373eb0e4d80b6e41aafc8d295b6d` | `interop/test_utils.go:223-237,462-493,614-648`, `interop/server/server.go`, `interop/grpc_testing/test_grpc.pb.go` UnimplementedTestServiceServer |
| [rustls](https://github.com/rustls/rustls) | `f04804fa13341a9b1dc02b5d1dd8a746f2c4b7fb` (0.23.46) | `examples/src/bin/tlsclient-mio.rs:88-124,147-185` |

## Net

The port retains the upstream test functions and cites every assertion. It
adds 136 checks: all 52 parser assertions, all 57 IP parse and Display assertions,
all 26 socket Display assertions, and one additional unscoped maximum-length
socket check. `SEM-3-net-rust-vectors-run.hir` runs the 90 parse assertions.
`crates/hirsel-driver/tests/fixtures/net-rust-display.hir` runs 46 Display/scope
checks through the Rust test `stdlib_net::rust_display_vectors_with_padding`,
on both snapshot and checked interpreters with a 10M-step bound: running all
formatting rows exceeds the corpus's 1M-step budget. Existing direct
address/radix checks remain.

Adaptations:

- Fixed network-order octets replace Rust address constructors. Typed v4/v6
  socket error rows use wrappers that check the `IpAddr` variant, since Hirsel
  has one `SocketAddr` type. Duplicate typed/generic upstream rows remain.
- `SocketAddr` has no flowinfo or scope ID. The scoped parser success at
  `parser.rs:89` becomes an explicit rejection. The six scoped Display literals
  at `socket_addr.rs:50-59,187-191` also pin rejection. The maximum-length
  scope-free socket has its own positive Display assertion. Adding a scope ID
  would require a runtime ABI change beyond this parity batch.
- Padding and precision apply to the returned String, since the API returns
  text rather than implementing Rust's formatter traits. Addresses are ASCII,
  so Rust byte/character precision and Hirsel display-cell precision agree.
  Rust's alternate Debug row at `ip_addr.rs:182` checks the same plain string;
  Hirsel has no corresponding alternate Debug formatter.
- The upstream `Option<Ipv4Addr>` typo at `ip_addr.rs:80` is preserved: that
  malformed embedded-v4 input is tested with the IPv4 parser.

## CLI

The reference is **Go flag**. Both `-name` and `--name` name one complete flag;
`--` terminates option parsing; the first positional argument (including `-`
or an empty string) ends option parsing. A value can follow `=` or consume
the next argument, including a dash-prefixed value. Boolean switches use Go's
ParseBool spellings and default to text `true` when no `=` is present.

`SEM-3-cli-go-flag-run.hir` imports the complete TestParse argument list and
its nine values/positional result, all seven TestParseError rows, both
TestInvalidFlags rows, and all five TestRangeError rows. It also covers the
terminator, positional stopping, repeats, boolean spellings and token errors.
The pre-existing CLI fixture is updated to the selected reference.

Adaptations:

- Immutable explicit `Flag` specs and a `Parsed` result replace Go's mutable
  flag registry, pointers and global CommandLine. Parsing returns errors instead
  of printing usage, panicking, or exiting; automatic `-h`/`-help` handling is
  left to the caller. The two invalid registration rows expect `InvalidSpec`.
- The existing public `short` field registers another name for the same flag,
  accepted with either dash spelling. Alias/name collisions are rejected, as
  duplicate names are in Go. No short-option clustering is supported.
- The public `Kind` has only Switch and Value. Value flags retain text and
  repeated values in order, matching Go's user-defined repeated flag example;
  ordinary typed conversion belongs to the caller. The seven syntax and five
  range rows test that boundary using the existing scalar parsers (the boolean
  syntax row is rejected directly by the switch parser). Error variants and
  argument indices replace Go's diagnostic strings.
- Go int/uint rows use Hirsel i64/u64, matching this 64-bit host. The `0x23`
  and `2m` TestParse values are checked verbatim as text: Hirsel's generic
  scalar APIs take an explicit radix and signed seconds, respectively. No
  Go duration/base-auto parser is added to the CLI. The remaining numeric rows
  check their converted values; float overflow must return OutOfRange.
- Spec names and aliases retain Hirsel's existing whitespace/control-character
  restrictions, rather than permitting names unsuitable for its usage display.
  Usage retains its existing explicit alias presentation.

### Optional interspersed scan

`cli.parse_interspersed` adapts clap's explicit trailing-values state from
`clap_builder/src/parser/parser.rs:78-170` at
`ccc89b83875474399e9d9d70841163f6319d1a41` (MIT, Copyright (c) Individual
contributors). A shared scanner walks the original argv; a private positional-stop
policy preserves `cli.parse`'s Go behavior. Only standalone `--` disables scanning
in the new entrypoint; value consumption takes precedence over the terminator.

`crates/hirsel-driver/tests/fixtures/cli-interspersed-11/vectors.hir` ports
`tests/builder/positionals.rs:5-19` (`only_pos_follow`, terminator) and `:43-75`
(`positional`, before/after option vectors). The pinned `:5-19` range is a
terminator vector, rather than a before-positional vector. Adaptations omit
argv[0], use immutable Hirsel specs, and check repeated text values and ordered
positionals instead of clap matches. Single/double-dash complete names, aliases,
Go boolean spellings and dash-prefixed values retain existing Hirsel rules.
Clap clusters, derive, subcommands, inference and configurable positional grammar
are deliberately omitted. Scalar conversion stays with the caller.
`cli.error_message` retains its existing stop-policy validation and therefore
renders original-index fallback text for errors after positionals.

## gRPC

`interop/main.hir` ports all five missing non-auth cases. The server returns
UNIMPLEMENTED for UnimplementedCall with grpc-go's default message from its
embedded UnimplementedTestServiceServer. The generated dispatcher decodes the
request before invoking that callback, matching grpc-go. No generated service
code changes.

Adaptations:

- Canceling the explicit ClientCall replaces canceling a Go context. The first
  response also checks the requested 31415-byte payload, a stronger assertion.
- Status code and message are compared separately, replacing Go error-string
  comparison. The whitespace/BMP/non-BMP message is verbatim.
- Both unimplemented tests use the public byte-level unary call with an empty
  protobuf body, matching Go Invoke's empty request for the missing method.
- The special-status test uses the harness's 30-second process timeout rather
  than a per-call 10-second context; this bounds a hung peer without adding a
  clock to this test's protocol assertion.

`packages/grpc@1/interop/check.py` runs all 14 cases against grpc-go in both
directions with isolated loopback grants, bounded clients and reaped servers.
See its README for the measured results and reproduction command.

## Citation corrections

HTML's replacement table is Go standard library `src/html/escape.go`, whose
EscapeString leaves carriage returns unchanged. This corrects the former
x/net/html attribution; escaping behavior and the nine existing checks stay
unchanged. The notice entry now names the matching source.

The runtime's nonblocking TLS loop cites rustls tlsclient-mio's read/process/
write/readiness sequence. Hirsel parks an operation on WouldBlock instead of
reregistering a mio client: replay, deadlines and committed writes belong to
its provider boundary. There is no runtime behavior or ABI change.

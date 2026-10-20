# URI prior art and conformance

The implementation follows Zig [`lib/std/Uri.zig`](https://github.com/ziglang/zig/blob/bff490a23889750de7096d9057c0a432efc929a8/lib/std/Uri.zig)
at `bff490a23889750de7096d9057c0a432efc929a8` (MIT; see
[notices](../third-party/NOTICES.md)). The reference was cloned for this port.
Component encoding/decoding follows lines 23–141, parsing lines 180–260,
formatting lines 262–310, and reference resolution, dot removal and path merging
lines 371–489. The branch structure is retained; owned lists replace mutable slices.
Numeric grammar follows [RFC 3986 §2.1](https://www.rfc-editor.org/rfc/rfc3986#section-2.1)
and [§3.2.3](https://www.rfc-editor.org/rfc/rfc3986#section-3.2.3).

The following 22 fixtures port all named Zig test groups. Names are prefixed
`conformance/SEM/SEM-3-uri-` and suffixed `-run.hir`; every fixture has a launch
manifest. Repeated absolute URLs in Zig's RFC parse group are retained.

| Zig Uri.zig source | Test group | Fixture stem |
| --- | --- | --- |
| 466–471 | remove_dot_segments | zig-01-dot-segments |
| 541–547 | basic | zig-02-basic |
| 549–555 | with port | zig-03-port |
| 557–559 | should fail gracefully | zig-04-graceful |
| 561–564 | parse name too long | zig-05-long-name |
| 566–586 | file | zig-06-file |
| 588–602 | scheme | zig-07-scheme |
| 604–645 | authority | zig-08-authority |
| 647–660 | authority.password | zig-09-authority-password |
| 667–678 | authority.dns-names | zig-10-authority-dns-names |
| 680–690 | authority.IPv4 | zig-11-authority-ipv4 |
| 692–708 | authority.IPv6 | zig-12-authority-ipv6 |
| 710–722 | RFC example 1 | zig-13-rfc-example-1 |
| 724–736 | RFC example 2 | zig-14-rfc-example-2 |
| 740–754 | Examples from wikipedia | zig-15-wikipedia |
| 758–786 | Examples from RFC3986 | zig-16-rfc-parse |
| 788–791 | Special test | zig-17-special |
| 793–802 | URI percent encoding | zig-18-percent-encoding |
| 804–834 | URI percent decoding | zig-19-percent-decoding |
| 836–844 | URI query encoding | zig-20-query-encoding |
| 846–860 | format | zig-21-format |
| 862–866 | URI malformed input | zig-22-malformed |

`rfc3986-normal` and `rfc3986-abnormal` carry the literal reference/target pairs
from [RFC 3986 §5.4](https://www.rfc-editor.org/rfc/rfc3986#section-5.4):
23 normal and 19 abnormal examples, including strict `http:g`.
Expected targets are literal complete `Uri` values, independent of the parser and
formatter. `strict-numeric` supplies signed/underscored, non-ASCII, overflow and
empty-port refusals, accepted decimal/hex neighbors, IPv6 authority twins and
resolution error propagation. `SEM-3-ascii-radix-run` and the Rust driver tests
also enforce the corrected grammar.

Adaptations and deviations:

- Zig's `fmt.parseInt` at Uri.zig:71,154,230,236 accepts signs and underscores.
  Hex pairs instead require exactly two ASCII hex digits; ports require ASCII
  decimal digits. This intentional correction prevents protocol parser differences
  and follows RFC 3986. Malformed escapes remain literal, as in Zig's decoder;
  `percent_decode` returns bytes rather than a parse error.
- Components and decoding results are owned. There is no caller buffer,
  backward decoder or in-place decoder API, so the two Zig decoding inputs are
  checked through both `percent_decode` and `component_raw`; aliasing and
  `NoSpaceLeft` checks do not apply. This follows Hirsel ownership semantics.
- Hirsel has component formatting and full `to_string`, rather than Zig format
  flags. The query-format test composes the encoded path, `?` and encoded query;
  the file-format test uses `to_string`. The exact expected output is retained.
- Zig's final malformed-input vector contains byte `0x85`, which is not valid
  UTF-8. Hirsel `String` cannot hold it: the exact byte vector is refused at
  `string.from_bytes`, and its Unicode U+0085 twin tests the malformed authority.
- `to_string` retains Zig's empty-path-to-`/` rendering (Uri.zig:293–294).
  The RFC `//g` result is checked as host `g` with an empty path, so no resolution
  expectation is changed to accommodate serialization.
- Existing host checks decode and reject ambiguous numeric hosts using the same
  rule as network grants; Zig's URI parser alone does not. Absolute-reference
  parse errors are preserved during resolution rather than retried as relative
  paths (Zig Uri.zig:373). These existing security constraints remain covered by
  the driver tests.
- Existing nonempty `u16` ports reject empty ports and values above 65535 even
  though RFC's `*DIGIT` grammar permits both. The API and network port range are
  retained. Other permissive Zig parsing behavior, such as unescaped `@` in
  userinfo and empty-password normalization (Uri.zig:199–219), is unchanged;
  this change tightens numeric tokens without redesigning the URI API.

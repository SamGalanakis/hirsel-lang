# HTTP/1 parsing vectors

The references were cloned for this port at these exact revisions:

- httparse `a0fa552e4e0fa9a5914a736ef3a556748fef709b`,
  [src/lib.rs:1332–1779](https://github.com/seanmonstar/httparse/blob/a0fa552e4e0fa9a5914a736ef3a556748fef709b/src/lib.rs#L1332)
  (`req!` / `res!`) and
  [src/lib.rs:1987–2003](https://github.com/seanmonstar/httparse/blob/a0fa552e4e0fa9a5914a736ef3a556748fef709b/src/lib.rs#L1987)
  (`test_chunk_size`).
- Go `ebe2feefc400f813aefdc8a036e66d2619da6649`,
  [src/net/http/response_test.go:39–748](https://github.com/golang/go/blob/ebe2feefc400f813aefdc8a036e66d2619da6649/src/net/http/response_test.go#L39)
  (`respTests`),
  [src/net/http/readrequest_test.go:30–420](https://github.com/golang/go/blob/ebe2feefc400f813aefdc8a036e66d2619da6649/src/net/http/readrequest_test.go#L30)
  (`reqTests`) and
  [src/net/http/readrequest_test.go:460–482](https://github.com/golang/go/blob/ebe2feefc400f813aefdc8a036e66d2619da6649/src/net/http/readrequest_test.go#L460)
  (`badRequestTests`).

Each corpus vector cites its upstream line and table index or test name.
Inputs are literal upstream bytes, including trailing pipelined bytes, the
repeated chunk assertion, LF-only heads, opaque header bytes and compressed
body bytes. Expected fields, decoded bodies, trailers and consumed lengths
are asserted, rather than compared with another invocation of our parser.

| Corpus file in `conformance/SEM/` | Upstream vectors | Added controls |
| --- | ---: | ---: |
| `SEM-1-http-httparse-vectors-run.hir` | 29 requests, 13 responses, 14 chunk assertions | 2 empty-buffer parses, 3 chunk extension/offset cases |
| `SEM-1-http-go-requests-run.hir` | 16 requests, all 5 bad requests | Request-target boundaries |
| `SEM-1-http-go-responses-run.hir` | All 23 responses | Response framing boundaries |

The audit's httparse totals need a correction: its 30/14 counts include the
recursive macro invocations at `src/lib.rs:1334` and `:1638`, which are macro
definitions, not vectors. `test_chunk_size` has 14 assertions, including one
duplicate, rather than 17. Every actual vector is present; the five controls
bring the wire file to the requested 44 parse cases and 17 chunk cases.

The HTTP adapter follows Go's `ReadResponse` status-space handling
(`response.go:172–177`) and `transferReader.determineBodyLength`
(`transfer.go:632–644`, HTTP/1.0 ignores Transfer-Encoding and closes;
`:709–722`, unknown response length requires EOF and closes). The scalar
wire parser keeps httparse's strict default configuration. A package-private
delimiter option follows httparse
`ParserConfig` (`src/lib.rs:650–695`) and accepts leading status spaces in
the HTTP adapter; offsets and consumed counts still refer to the input.
Request-target validation follows `readRequest`
(`request.go:1125–1155`) and `url.parse` (`net/url/url.go:443–537`): origin
paths (including `//`), asterisk, absolute URIs and CONNECT authorities.
Request-target validation and incoming request construction live in
package-private `internal.http1_parse`, separate from the server state machine.

## Deviations and adaptations

1. **Borrowing and partial storage:** Hirsel returns offsets into caller-owned
   bytes and an explicit initialized header count. httparse exposes borrowed
   slices and leaves its header slice at capacity on Partial. The two partial
   header vectors assert one initialized field rather than a slice length of
   four; every upstream initialized field is checked. This is the ownership
   representation of the existing noalloc API.
2. **Headers and request targets:** Hirsel preserves field spelling, order,
   duplicates and framing fields in an owned list. Go canonicalizes into a map
   and moves Host, Transfer-Encoding and Trailer into separate fields. Hirsel
   exposes the original request target rather than a second URL object. Tests
   compare those owned representations, raw targets and decoded bodies; no
   routing or host capability is inferred from them.
3. **Lengths:** `ResponseHead.content_length` is optional wire metadata. An
   absent field remains `None`, including on 204, where Go reports an effective
   length of zero. The body assertion checks that 204 consumes no body. HEAD
   retains its advertised length and consumes no body. Unknown or chunked
   length is `None`, replacing Go's signed `-1` sentinel.
4. **Ambiguous HTTP/1.1 framing:** Existing smuggling protections reject CL+TE
   rather than deleting Content-Length as Go does. `reqTests[6]` and
   `respTests[6]` explicitly expect this rejection; `reqTests[7]` rejects the
   framing conflict before reporting the malformed length. This retains the
   established security policy pinned by `SEM-1-http-headers-limits-run.hir`
   and the approved deviations in `stdlib_http_wire.rs`. Rejecting the whole
   message prevents differing boundaries between the capability-facing
   server/client and another HTTP hop.
5. **HTTP/2 preface:** `reqTests[15]` is present as a Version rejection in the
   HTTP/1 parser. Hirsel dispatches HTTP/2 to its separate `http_h2` transport;
   it does not expose Go's connection-hijacking request representation.
6. **Redirects and a missing request:** The two raw 303 vectors use HEAD to
   inspect their status, empty reason and headers via `receive_head` without
   invoking its redirect-following policy. Their raw response bytes and empty
   expected bodies are unchanged. Go's nil request vector uses Get, which is
   exactly ReadResponse's nil-request rule. Hirsel requires an explicit
   request resource and Net capability.
7. **Host policy and errors:** CONNECT/absolute URI authorities use Hirsel's
   canonical host validation shared with network capability grants (including
   numeric hosts and IPv6), rather than Go's more permissive arbitrary host
   text. Invalid URI syntax is `Malformed(Token())`; leading whitespace in
   headers is `Malformed(Continuation())`. Typed errors replace Go's free-form
   strings. The invalid CONNECT host and both leading-header vectors retain
   the upstream rejection.
8. **Execution:** Go's reader is replaced by deterministic scripted Net peers;
   default decompression remains disabled for the compressed-body vector.
   `RawOut` is omitted because these are reader tests, not Go's separate
   `TestWriteResponse`. Header/body helpers check the actual decoded body and
   trailers; request tests also check the consumed prefix.

## Validation

All three new corpus programs pass ordinary and checked execution and the
formatter check. The affected set also passes 71 importer programs, 25 existing
HTTP Rust tests, seven example tests and 88 agreement variants (no divergence
clusters). The instruction-budget run stops at the separately recorded main
failure: `micro/bigint_mul` has no independent small-input output oracle.

Boundary vectors adapt Go net/http at ebe2feefc400f813aefdc8a036e66d2619da6649:
`request.go:1222–1254` (probe beyond the bound) and `serve_test.go:3660–3680`
(exact count and typed MaxBytesError). Copyright The Go Authors; BSD license
in GO-LICENSE.txt. Hirsel reuses its existing read_all probe rather than
porting a second reader. Adaptations: BodyTooLarge replaces MaxBytesError;
zero, u64::MAX, three response framings, capability denial, resource close,
compression and offline cross-engine tapes exercise Hirsel's contract.

Local authority/test structure: std http fetch/read_all/decode_content,
dogfood9's request/send_bodiless/receive_head/read_all65536 caller, and the
file-read-bounded-11 build-once replay harness. They are not external ports.
The main fixture demonstrates the compact replacement without editing the app.

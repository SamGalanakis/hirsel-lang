//! Ports of httparse src/lib.rs response/chunk tests (a0fa552e) and Zig
//! lib/std/http/{Client,HeaderIterator,ChunkParser}.zig (bff490a2).
use hirsel_driver::*;
use hirsel_engine::*;
use std::path::PathBuf;

fn details(e: &ToolError) -> String {
    match e {
        ToolError::Query { error, .. } => format!("{error:?}"),
        _ => e.to_string(),
    }
}

fn execute(body: &str) {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../conformance/SEM/SEM-1-http-wire-run.hir");
    let source =
        format!("{HELPERS}\npub fn main(t: Test) -> Result[(), ()] {{\n{body}\nOk(())\n}}\n");
    let mut program =
        load_program_source(&path, Some(&source)).unwrap_or_else(|e| panic!("{}", details(&e)));
    program
        .check_for_execution()
        .unwrap_or_else(|e| panic!("{}", details(&e)));
    program.engine.adapters.interpreter = Some(Box::new(InterpreterAdapter {
        limits: hirsel_interp::Limits {
            steps: Some(16_000_000),
            call_depth: 256,
            source: None,
        },
        ..Default::default()
    }));
    let run = run_observed_snapshot(
        &mut program.engine,
        &RunConfiguration {
            entry: program.entry,
            hash_seed: 0,
            inputs: vec![],
        },
        SimulationConfig::default(),
    )
    .unwrap();
    assert!(
        matches!(
            run.outcome,
            Ok(RunOutcome::Returned(hirsel_run_schema::Returned::Ok))
        ),
        "{:?}",
        run.outcome
    );
}

fn literal(bytes: &[u8]) -> String {
    let mut text = String::from("b\"");
    for &byte in bytes {
        match byte {
            b'"' => text.push_str("\\\""),
            b'\\' => text.push_str("\\\\"),
            32..=126 => text.push(char::from(byte)),
            _ => text.push_str(&format!("\\x{byte:02x}")),
        }
    }
    text.push('"');
    text
}

const HELPERS: &str = r#"
fn server_config(head: u64, body: u64) -> std@1/http.Config uses {} {
  let mut config = std@1/http.default_config()
  config.max_head_bytes = head
  config.max_body_bytes = body
  take config
}

fn empty() -> std@1/http_wire.RawHeader {
  std@1/http_wire.RawHeader(name_start: 0, name_end: 0, value_start: 0, value_end: 0)
}
fn server_parse(t: Test, buf: Bytes, head: u64, body: u64, expected: Result[Option[(std@1/http.ServerRequest, u64)], std@1/http.HttpError]) {
  test.check(t, std@1/http.parse_server_request(buf, server_config(head, body)) == expected)
}
fn request(t: Test, buf: Bytes, expected: Result[std@1/http_wire.ParseStatus, std@1/http_wire.HeadError]) {
  let mut line = std@1/http_wire.RequestLine(method_start: 0, method_end: 0, target_start: 0, target_end: 0, version: 0)
  let mut headers: Inline[std@1/http_wire.RawHeader, 4] = [empty(), empty(), empty(), empty()]
  let mut count: u64 = 99
  test.check(t, std@1/http_wire.parse_request(buf, mut line, mut headers, mut count) == expected)
}
fn response(t: Test, buf: Bytes, expected: Result[std@1/http_wire.ParseStatus, std@1/http_wire.HeadError]) {
  let mut line = std@1/http_wire.StatusLine(version: 0, code: 0, reason_start: 0, reason_end: 0)
  let mut headers: Inline[std@1/http_wire.RawHeader, 4] = [empty(), empty(), empty(), empty()]
  let mut count: u64 = 99
  test.check(t, std@1/http_wire.parse_response(buf, mut line, mut headers, mut count) == expected)
}
fn complete(t: Test, buf: Bytes, version: u8, code: u16, reason: Bytes, names: List[Bytes], values: List[Bytes]) {
  let mut line = std@1/http_wire.StatusLine(version: 0, code: 0, reason_start: 0, reason_end: 0)
  let mut headers: Inline[std@1/http_wire.RawHeader, 4] = [empty(), empty(), empty(), empty()]
  let mut count: u64 = 99
  test.check(t, std@1/http_wire.parse_response(buf, mut line, mut headers, mut count) == Ok(Complete(bytes.len(buf))))
  test.check(t, line.version == version && line.code == code)
  test.check(t, bytes.slice(buf, line.reason_start, line.reason_end) == reason)
  test.check(t, count == list.len(names))
  for i in 0..count {
    let h = headers[i]
    test.check(t, bytes.slice(buf, h.name_start, h.name_end) == names[i])
    test.check(t, bytes.slice(buf, h.value_start, h.value_end) == values[i])
  }
}
fn framed(t: Test, buf: Bytes, version: u8, expected: Result[std@1/http_wire.Framing, std@1/http_wire.HeadError]) {
  let mut headers: Inline[std@1/http_wire.RawHeader, 4] = [empty(), empty(), empty(), empty()]
  let mut count: u64 = 0
  match std@1/http_wire.parse_headers(buf, 0, mut headers, mut count) {
    Ok(Complete(_)) => (),
    _ => test.check(t, false),
  }
  test.check(t, std@1/http_wire.framing(buf, version, headers, count) == expected)
}
"#;

#[test]
fn httparse_thirteen_response_vectors() {
    with_compiler_stack(|| {
        let mut body = String::new();
        for (input, version, code, reason, names, values) in [
            (
                &b"HTTP/1.1 200 OK\r\n\r\n"[..],
                1,
                200,
                &b"OK"[..],
                vec![],
                vec![],
            ),
            (
                &b"HTTP/1.0 403 Forbidden\nServer: foo.bar\n\n"[..],
                0,
                403,
                &b"Forbidden"[..],
                vec![&b"Server"[..]],
                vec![&b"foo.bar"[..]],
            ),
            (
                &b"HTTP/1.1 200 \r\n\r\n"[..],
                1,
                200,
                &b""[..],
                vec![],
                vec![],
            ),
            (
                &b"HTTP/1.1 200\r\n\r\n"[..],
                1,
                200,
                &b""[..],
                vec![],
                vec![],
            ),
            (
                &b"HTTP/1.1 200\r\nFoo: bar\r\n\r\n"[..],
                1,
                200,
                &b""[..],
                vec![&b"Foo"[..]],
                vec![&b"bar"[..]],
            ),
            (
                &b"HTTP/1.1 101 Switching Protocols\t\r\n\r\n"[..],
                1,
                101,
                &b"Switching Protocols\t"[..],
                vec![],
                vec![],
            ),
            (
                &b"HTTP/1.1 200 X\xffZ\r\n\r\n"[..],
                1,
                200,
                &b""[..],
                vec![],
                vec![],
            ),
            (
                &b"\n\nHTTP/1.1 200 OK\n\n"[..],
                1,
                200,
                &b"OK"[..],
                vec![],
                vec![],
            ),
            (
                &b"HTTP/1.0 200\nContent-type: text/html\n\n"[..],
                0,
                200,
                &b""[..],
                vec![&b"Content-type"[..]],
                vec![&b"text/html"[..]],
            ),
        ] {
            let names = names
                .iter()
                .map(|b| literal(b))
                .collect::<Vec<_>>()
                .join(", ");
            let values = values
                .iter()
                .map(|b| literal(b))
                .collect::<Vec<_>>()
                .join(", ");
            body += &format!(
                "complete(t, {}, {version}, {code}, {}, [{names}], [{values}])\n",
                literal(input),
                literal(reason)
            );
        }
        for (input, expected) in [
            (&b"HTTP/1.1 200 \x00\r\n\r\n"[..], "Error(Status())"),
            (&b"HTTP/1.1"[..], "Ok(Partial())"),
            (&b"HTTP/1.1 200"[..], "Ok(Partial())"),
            (&b"HTTP/1.1 200 OK\r\nServer: yolo\r\n"[..], "Ok(Partial())"),
        ] {
            body += &format!("response(t, {}, {expected})\n", literal(input));
        }
        // On partial input the initialized header prefix remains observable.
        body += r#"
let buf: Bytes = b"HTTP/1.1 200 OK\r\nServer: yolo\r\n"
let mut line = std@1/http_wire.StatusLine(version: 0, code: 0, reason_start: 0, reason_end: 0)
let mut headers: Inline[std@1/http_wire.RawHeader, 1] = [empty()]
let mut count: u64 = 0
std@1/http_wire.parse_response(buf, mut line, mut headers, mut count);
test.check(t, count == 1 && line.code == 200 && line.version == 1)
test.check(t, bytes.slice(buf, headers[0].value_start, headers[0].value_end) == b"yolo")
"#;
        execute(&body);
    });
}

#[test]
fn httparse_strict_response_negatives_and_partial_inputs() {
    with_compiler_stack(|| {
        let mut body = String::new();
        for (input, error) in [
            (&b"HTTP/1.1 200 OK\r\nAccess-Control-Allow-Credentials : true\r\nBread: baguette\r\n\r\n"[..], "HeaderName"),
            (&b"HTTP/1.1 200 OK\r\nLine-Folded-Header: \r\n   \r\n hello there\r\n\r\n"[..], "Continuation"),
            (&b"HTTP/1.1 200 OK\r\nLine-Folded-Header: hello there\r\n   \r\n \r\n\r\n"[..], "Continuation"),
            (&b"HTTP/1.1 200 OK\r\nLine-Folded-Header: hello  \r\n \r\n there\r\n\r\n"[..], "Continuation"),
            (&b"HTTP/1.1 200 OK\r\nLine-Folded-Header:   \r\n \r\n \r\n\r\n"[..], "Continuation"),
            (&b"HTTP/1.1   200  OK\r\n\r\n"[..], "Status"),
            (&b"HTTP/1.1 200\rOK\r\n\r\n"[..], "Status"),
            (&b"HTTP/1.1 99 200 OK\r\n\r\n"[..], "Status"),
            (&b"HTTP/1.1 200 OK\r\n: hello\r\nBread: baguette\r\n\r\n"[..], "HeaderName"),
            (&b"HTTP/1.1 200 OK\r\nAccess-Control-Allow-Credentials\xff  : true\r\nBread: baguette\r\n\r\n"[..], "HeaderName"),
            (&b"HTTP/1.1 200 OK\r\nAccess-Control-Allow-Credentials\r\nBread: baguette\r\n\r\n"[..], "HeaderName"),
            (&b"HTTP/1.1 200 OK\r\nAccess-Control-Allow-Credentials   \r\n hello\r\nBread: baguette\r\n\r\n"[..], "HeaderName"),
            (&b"HTTP/1.1 200 OK\r\nAccess-Control-Allow-Cred\0entials: hello\r\nBread: baguette\r\n\r\n"[..], "HeaderName"),
            (&b"HTTP/1.1 200 OK\r\nAccess-Control-Allow-Cred\rentials: hello\r\nBread: baguette\r\n\r\n"[..], "HeaderName"),
            (&b"HTTP/1.1 200 OK\r\nAccess-Control-Allow-Credentials   \0: hello\r\nBread: baguette\r\n\r\n"[..], "HeaderName"),
            (&b"HTTP/1.1 200 OK\r\nAccess-Control-Allow-Credentials: hell\0o\r\nBread: baguette\r\n\r\n"[..], "HeaderValue"),
            (&b"HTTP/1.1 200 OK\r\nAccess-Control-Allow-Credentials: hell\x01o\r\nBread: baguette\r\n\r\n"[..], "HeaderValue"),
            (&b"HTTP/1.1 200 OK\r\nAccess-Control-Allow-Credentials: hell\x01o  \n world!\r\nBread: baguette\r\n\r\n"[..], "HeaderValue"),
            (&b"HTTP/1.1 200 OK\r\n Space-Before-Header: hello there\r\n\r\n"[..], "Continuation"),
            (&b"HTTP/2.0 200 OK\r\n\r\n"[..], "Version"),
            (&b"\rXHTTP/1.1 200 OK\r\n\r\n"[..], "NewLine"),
            (&b"HTTP/1.1 200 OK\r\n\rX"[..], "NewLine"),
        ] {
            body += &format!("response(t, {}, Error({error}()))\n", literal(input));
        }
        // Every strict prefix is partial, including CR split from LF and a full header.
        for input in [
            b"\r\nHTTP/1.1 200 OK\r\nx: y\r\n\r\n".as_slice(),
            b"HTTP/1.0 418\nempty:\t \n\n",
        ] {
            for end in 0..input.len() {
                body += &format!("response(t, {}, Ok(Partial()))\n", literal(&input[..end]));
            }
        }
        body += r#"
let mut line = std@1/http_wire.StatusLine(version: 0, code: 0, reason_start: 0, reason_end: 0)
let mut headers: Inline[std@1/http_wire.RawHeader, 0] = []
let mut count: u64 = 0
test.check(t, std@1/http_wire.parse_response(b"HTTP/1.1 200 OK\r\n\r\n", mut line, mut headers, mut count) == Ok(Complete(19)))
test.check(t, std@1/http_wire.parse_response(b"HTTP/1.1 200 OK\r\nx:y\r\n\r\n", mut line, mut headers, mut count) == Error(TooManyHeaders()))
"#;
        execute(&body);
    });
}

#[test]
fn httparse_fourteen_chunk_assertions_and_zig_leniency_negatives() {
    with_compiler_stack(|| {
        let mut body = String::new();
        for (input, expected) in [
            (&b"0\r\n"[..], "Ok(Complete(3, 0))"),
            (&b"12\r\nchunk"[..], "Ok(Complete(4, 18))"),
            (&b"3086d\r\n"[..], "Ok(Complete(7, 198765))"),
            (&b"3735AB1;foo bar*\r\n"[..], "Ok(Complete(18, 57891505))"),
            (&b"3735ab1 ; baz \r\n"[..], "Ok(Complete(16, 57891505))"),
            (&b"77a65\r"[..], "Ok(Partial())"),
            (&b"ab"[..], "Ok(Partial())"),
            (&b"567f8a\rfoo"[..], "Error(InvalidChunkSize())"),
            (&b"567f8a\rfoo"[..], "Error(InvalidChunkSize())"),
            (&b"567xf8a\r\n"[..], "Error(InvalidChunkSize())"),
            (
                &b"ffffffffffffffff\r\n"[..],
                "Ok(Complete(18, 18446744073709551615))",
            ),
            (&b"1ffffffffffffffff\r\n"[..], "Error(InvalidChunkSize())"),
            (&b"Affffffffffffffff\r\n"[..], "Error(InvalidChunkSize())"),
            (&b"fffffffffffffffff\r\n"[..], "Error(InvalidChunkSize())"),
            // Zig ChunkParser.test feed: first/third agree, LF-only second refuses.
            (
                &b"Ff\r\nf0f000 ; ext\n0\r\nffffffffffffffffffffffffffffffffffffffff\r\n"[..],
                "Ok(Complete(4, 255))",
            ),
            (&b"f0f000 ; ext\n"[..], "Ok(Partial())"),
            (
                &b"0\r\nffffffffffffffffffffffffffffffffffffffff\r\n"[..],
                "Ok(Complete(3, 0))",
            ),
            (
                &b"ffffffffffffffffffffffffffffffffffffffff\r\n"[..],
                "Error(InvalidChunkSize())",
            ),
            (&b"ffxyz\r\n"[..], "Error(InvalidChunkSize())"),
            (&b"ff\n"[..], "Error(InvalidChunkSize())"),
            (&b"ff 0\r\n"[..], "Error(InvalidChunkSize())"),
            (&b"ff \t;ignored\0\xff\r\n"[..], "Ok(Complete(16, 255))"),
        ] {
            body += &format!(
                "test.check(t, std@1/http_wire.parse_chunk_size({}, 0) == {expected})\n",
                literal(input)
            );
        }
        for input in [b"ab\r\n".as_slice(), b"ab ;x\r\n"] {
            for end in 0..input.len() {
                body += &format!(
                    "test.check(t, std@1/http_wire.parse_chunk_size({}, 0) == Ok(Partial()))\n",
                    literal(&input[..end])
                );
            }
        }
        body += "test.check(t, std@1/http_wire.parse_chunk_size(b\"prefixfF\\r\\nbody\", 6) == Ok(Complete(4, 255)))\n";
        execute(&body);
    });
}

#[test]
fn httparse_header_byte_tables_and_trimmed_values() {
    with_compiler_stack(|| {
        // Exhaustive independent RFC token and field-value byte sets from the
        // upstream TOKEN_MAP/HEADER_VALUE_MAP, including obs-text and DEL.
        let mut body = String::new();
        let names: Vec<_> = (0..=255u8)
            .map(|b| {
                if b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&b) {
                    b'1'
                } else {
                    b'0'
                }
            })
            .collect();
        let values: Vec<_> = (0..=255u8)
            .map(|b| {
                if b == b'\t' || b >= 32 && b != 127 {
                    b'1'
                } else {
                    b'0'
                }
            })
            .collect();
        body += &format!(
            "let names: Bytes = {}\nlet values: Bytes = {}\n",
            literal(&names),
            literal(&values)
        );
        body += r#"
for n in 0u16..256u16 {
  let byte: u8 = int.narrow(n)
  let index: u64 = int.widen(n)
  let input = bytes.from_list([72, 84, 84, 80, 47, 49, 46, 49, 32, 50, 48, 48, 32, 79, 75, 13, 10, 120, byte, 122, 58, 118, 13, 10, 13, 10])
  if names[index] == 49 || byte == 58 { response(t, input, Ok(Complete(26))) }
  else { response(t, input, Error(HeaderName())) }
  if byte != 13 && byte != 10 {
    let value = bytes.from_list([72, 84, 84, 80, 47, 49, 46, 49, 32, 50, 48, 48, 32, 79, 75, 13, 10, 120, 58, 97, byte, 122, 13, 10, 13, 10])
    if values[index] == 49 { response(t, value, Ok(Complete(26))) }
    else { response(t, value, Error(HeaderValue())) }
  }
}
"#;
        body += r#"
complete(t, b"HTTP/1.1 200 OK\r\nx: \t a\t b \t\r\ny:\t \r\nz: \xff\r\n\r\n", 1, 200, b"OK", [b"x", b"y", b"z"], [b"a\t b", b"", b"\xff"])
"#;
        execute(&body);
    });
}

#[test]
fn zig_framing_and_design_deviations() {
    with_compiler_stack(|| {
        let mut body = String::new();
        for (input, version, expected) in [
            (
                &b"\r\n"[..],
                0,
                "Ok(std@1/http_wire.Framing(content_length: None(), chunked: false, keep_alive: false))",
            ),
            (
                &b"\r\n"[..],
                1,
                "Ok(std@1/http_wire.Framing(content_length: None(), chunked: false, keep_alive: true))",
            ),
            (
                &b"connection: cLoSe\r\n\r\n"[..],
                1,
                "Ok(std@1/http_wire.Framing(content_length: None(), chunked: false, keep_alive: false))",
            ),
            (
                &b"Connection: keep-alive\r\n\r\n"[..],
                0,
                "Ok(std@1/http_wire.Framing(content_length: None(), chunked: false, keep_alive: true))",
            ),
            (
                &b"content-Length:10\r\nconnectioN:\t keep-alive \r\n\r\n"[..],
                1,
                "Ok(std@1/http_wire.Framing(content_length: Some(10), chunked: false, keep_alive: true))",
            ),
            (
                &b"Content-Length: 10\r\nContent-Length: 010\r\n\r\n"[..],
                1,
                "Ok(std@1/http_wire.Framing(content_length: Some(10), chunked: false, keep_alive: true))",
            ),
            (
                &b"Content-Length: 0\r\n\r\n"[..],
                1,
                "Ok(std@1/http_wire.Framing(content_length: Some(0), chunked: false, keep_alive: true))",
            ),
            (
                &b"Content-Length: 18446744073709551615\r\n\r\n"[..],
                1,
                "Ok(std@1/http_wire.Framing(content_length: Some(18446744073709551615), chunked: false, keep_alive: true))",
            ),
            (
                &b"TRansfer-encoding:\t ChUnKeD \r\n\r\n"[..],
                1,
                "Ok(std@1/http_wire.Framing(content_length: None(), chunked: true, keep_alive: true))",
            ),
            (
                &b"content-length: 10\r\ncontent-length: 11\r\n\r\n"[..],
                1,
                "Error(ContentLength())",
            ),
            (
                &b"content-length: 18446744073709551616\r\n\r\n"[..],
                1,
                "Error(ContentLength())",
            ),
            (
                &b"content-length: 999999999999999999999999\r\n\r\n"[..],
                1,
                "Error(ContentLength())",
            ),
            (&b"content-length:\r\n\r\n"[..], 1, "Error(ContentLength())"),
            (
                &b"content-length: -1\r\n\r\n"[..],
                1,
                "Error(ContentLength())",
            ),
            (
                &b"content-length: +10\r\n\r\n"[..],
                1,
                "Error(ContentLength())",
            ),
            (
                &b"content-length: 1_0\r\n\r\n"[..],
                1,
                "Error(ContentLength())",
            ),
            (
                &b"content-length: 10, 10\r\n\r\n"[..],
                1,
                "Error(ContentLength())",
            ),
            (
                &b"content-length: 0x10\r\n\r\n"[..],
                1,
                "Error(ContentLength())",
            ),
            (
                &b"content-length: 1 0\r\n\r\n"[..],
                1,
                "Error(ContentLength())",
            ),
            (
                &b"content-length: 10\r\ntransfer-encoding: chunked\r\n\r\n"[..],
                1,
                "Error(TransferEncoding())",
            ),
            (
                &b"transfer-encoding: chunked\r\ncontent-length: 10\r\n\r\n"[..],
                1,
                "Error(TransferEncoding())",
            ),
            (
                &b"transfer-encoding: deflate, chunked\r\n\r\n"[..],
                1,
                "Error(TransferEncoding())",
            ),
            (
                &b"transfer-encoding: gzip\r\n\r\n"[..],
                1,
                "Error(TransferEncoding())",
            ),
            (
                &b"transfer-encoding: identity\r\n\r\n"[..],
                1,
                "Error(TransferEncoding())",
            ),
            (
                &b"transfer-encoding: chunked, chunked\r\n\r\n"[..],
                1,
                "Error(TransferEncoding())",
            ),
            (
                &b"transfer-encoding: chunked\r\ntransfer-encoding: chunked\r\n\r\n"[..],
                1,
                "Error(TransferEncoding())",
            ),
            (
                &b"transfer-encoding:\r\n\r\n"[..],
                1,
                "Error(TransferEncoding())",
            ),
        ] {
            body += &format!("framed(t, {}, {version}, {expected})\n", literal(input));
        }
        for code in [0, 418, 999] {
            body += &format!(
                "complete(t, b\"HTTP/1.1 {code:03} OK\\r\\n\\r\\n\", 1, {code}, b\"OK\", [], [])\n"
            );
        }
        execute(&body);
    });
}

#[test]
fn zig_head_and_header_iterator_vectors() {
    with_compiler_stack(|| {
        let input = b"HTTP/1.1 200 OK\r\nLOcation:url\r\ncontent-tYpe: text/plain\r\ncontent-disposition:attachment; filename=example.txt \r\ncontent-Length:10\r\nTRansfer-encoding:\tdeflate, chunked \r\nconnectioN:\t keep-alive \r\n\r\n";
        let mut body = format!("let buf: Bytes = {}\n", literal(input));
        body += r#"
let mut headers: Inline[std@1/http_wire.RawHeader, 8] = [empty(), empty(), empty(), empty(), empty(), empty(), empty(), empty()]
let mut line = std@1/http_wire.StatusLine(version: 0, code: 0, reason_start: 0, reason_end: 0)
let mut count: u64 = 0
test.check(t, std@1/http_wire.parse_response(buf, mut line, mut headers, mut count) == Ok(Complete(bytes.len(buf))))
test.check(t, count == 6 && line.code == 200 && line.version == 1)
// Zig test parse is a negative under the approved CL+TE / deflate deviations.
test.check(t, std@1/http_wire.framing(buf, line.version, headers, count) == Error(TransferEncoding()))
match std@1/http_wire.find_header(buf, headers, count, b"LOCATION") {
  Some(h) => test.check(t, bytes.slice(buf, h.value_start, h.value_end) == b"url"),
  None() => test.check(t, false),
}
match std@1/http_wire.find_header(buf, headers, count, b"Content-Type") {
  Some(h) => test.check(t, bytes.slice(buf, h.value_start, h.value_end) == b"text/plain"),
  None() => test.check(t, false),
}
test.check(t, std@1/http_wire.find_header(buf, headers, count, b"missing") == None())
let mut it = std@1/http_wire.header_iterator(buf)
"#;
        for (name, value) in [
            ("LOcation", "url"),
            ("content-tYpe", "text/plain"),
            ("content-disposition", "attachment; filename=example.txt"),
            ("content-Length", "10"),
            ("TRansfer-encoding", "deflate, chunked"),
            ("connectioN", "keep-alive"),
        ] {
            body += &format!(
                r#"
match std@1/http_wire.next_header(buf, mut it) {{
  Some(h) => {{
    test.check(t, bytes.slice(buf, h.name_start, h.name_end) == {})
    test.check(t, bytes.slice(buf, h.value_start, h.value_end) == {})
    test.check(t, !it.is_trailer)
  }},
  None() => test.check(t, false),
}}
"#,
                literal(name.as_bytes()),
                literal(value.as_bytes())
            );
        }
        body += "test.check(t, std@1/http_wire.next_header(buf, mut it) == None())\n";
        body += r#"
let raw: Bytes = b"200 OK\r\na: b\r\nc:  \r\nd:e\r\n\r\nf: g\r\n\r\n"
let mut cursor = std@1/http_wire.header_iterator(raw)
"#;
        for (name, value, trailer) in [
            ("a", "b", false),
            ("c", "", false),
            ("d", "e", false),
            ("f", "g", true),
        ] {
            body += &format!(
                r#"
match std@1/http_wire.next_header(raw, mut cursor) {{
  Some(h) => {{
    test.check(t, bytes.slice(raw, h.name_start, h.name_end) == {})
    test.check(t, bytes.slice(raw, h.value_start, h.value_end) == {})
    test.check(t, cursor.is_trailer == {trailer})
  }},
  None() => test.check(t, false),
}}
"#,
                literal(name.as_bytes()),
                literal(value.as_bytes())
            );
        }
        body += r#"
test.check(t, std@1/http_wire.next_header(raw, mut cursor) == None())
test.check(t, std@1/http_wire.next_header(raw, mut cursor) == None())
let bad: Bytes = b"200 OK\r\n: ss\r\n\r\n"
let mut bad_it = std@1/http_wire.header_iterator(bad)
test.check(t, std@1/http_wire.next_header(bad, mut bad_it) == None() && !bad_it.is_trailer)
let bad_trailer: Bytes = b"200 OK\r\na:b\r\n\r\n: ss\r\n\r\n"
let mut trailer_it = std@1/http_wire.header_iterator(bad_trailer)
test.check(t, std@1/http_wire.next_header(bad_trailer, mut trailer_it) != None())
test.check(t, std@1/http_wire.next_header(bad_trailer, mut trailer_it) == None() && trailer_it.is_trailer)
"#;
        execute(&body);
    });
}

#[test]
fn caller_storage_offsets_capacity_and_reset() {
    with_compiler_stack(|| {
        execute(
            r#"
let buf: Bytes = b"prefixA: v \r\n\r\nbody"
let sentinel = std@1/http_wire.RawHeader(name_start: 99, name_end: 99, value_start: 99, value_end: 99)
let mut headers: Inline[std@1/http_wire.RawHeader, 2] = [sentinel, sentinel]
let mut count: u64 = 99
test.check(t, std@1/http_wire.parse_headers(buf, 6, mut headers, mut count) == Ok(Complete(15)))
test.check(t, count == 1 && headers[1] == sentinel)
test.check(t, headers[0] == std@1/http_wire.RawHeader(name_start: 6, name_end: 7, value_start: 9, value_end: 10))
test.check(t, std@1/http_wire.parse_headers(b"x:y\r\nz:unfinished", 0, mut headers, mut count) == Ok(Partial()))
test.check(t, count == 1 && headers[1] == sentinel)
test.check(t, std@1/http_wire.parse_headers(b"", 18446744073709551615, mut headers, mut count) == Ok(Partial()) && count == 0)
let mut one: Inline[std@1/http_wire.RawHeader, 1] = [sentinel]
let mut line = std@1/http_wire.StatusLine(version: 0, code: 0, reason_start: 0, reason_end: 0)
test.check(t, std@1/http_wire.parse_response(b"HTTP/1.1 200 OK\r\nx:y\r\nz:w\r\n\r\n", mut line, mut one, mut count) == Error(TooManyHeaders()))
test.check(t, count == 1 && one[0] != sentinel)
test.check(t, std@1/http_wire.parse_response(b"INVALID", mut line, mut one, mut count) == Error(Version()) && count == 0)
test.check(t, std@1/http_wire.framing(buf, 2, one, 0) == Error(Version()))
test.check(t, std@1/http_wire.framing(buf, 1, one, 2) == Error(TooManyHeaders()))
one[0] = std@1/http_wire.RawHeader(name_start: 0, name_end: 1, value_start: 18446744073709551615, value_end: 18446744073709551615)
test.check(t, std@1/http_wire.framing(buf, 1, one, 1) == Error(HeaderValue()))
one[0] = sentinel
test.check(t, std@1/http_wire.framing(buf, 1, one, 1) == Error(HeaderName()))
test.check(t, std@1/http_wire.find_header(buf, one, 1, b"x") == None())
let mut invalid = std@1/http_wire.HeaderIterator(index: 18446744073709551615, is_trailer: false, done: false)
test.check(t, std@1/http_wire.next_header(buf, mut invalid) == None())
test.check(t, std@1/http_wire.parse_chunk_size(buf, 18446744073709551615) == Ok(Partial()))
"#,
        )
    });
}

// httparse's Request vectors: simple/query/WHATWG target, OWS, LF, empty
// prefix, one-byte method, partial prefixes and strict delimiter errors.
#[test]
fn httparse_request_vectors_and_content_length_boundaries() {
    with_compiler_stack(|| {
        let mut body = String::new();
        for input in [
            &b"GET / HTTP/1.1\r\n\r\n"[..],
            b"GET /thing?data=a HTTP/1.1\r\n\r\n",
            b"GET /thing?data=a^ HTTP/1.1\r\n\r\n",
            b"GET / HTTP/1.1\r\nHost: \tfoo.com\t \r\nCookie: \t \r\n\r\n",
            b"GET / HTTP/1.0\n\n",
            b"\r\n\r\nGET / HTTP/1.1\r\n\r\n",
            b"A / HTTP/1.1\r\n\r\n",
            b"GET /a\\b HTTP/1.1\r\n\r\n",
        ] {
            body.push_str(&format!(
                "request(t, {}, Ok(Complete({})))\n",
                literal(input),
                input.len()
            ));
            // Every proper prefix must remain partial, including split CRLF.
            for end in 0..input.len() {
                body.push_str(&format!(
                    "request(t, {}, Ok(Partial()))\n",
                    literal(&input[..end])
                ));
            }
        }
        for (input, error) in [
            (&b" / HTTP/1.1\r\n\r\n"[..], "Token"),
            (b"GET  HTTP/1.1\r\n\r\n", "Token"),
            (b"GE(T / HTTP/1.1\r\n\r\n", "Token"),
            (b"GET / HTTP/1.2\r\n\r\n", "Version"),
            (b"GET / HTTP/1.1\rX", "NewLine"),
            (b"GET / HTTP/1.1\r\nHost : x\r\n\r\n", "HeaderName"),
            (b"GET / HTTP/1.1\r\n x: y\r\n\r\n", "Continuation"),
            (b"GET / HTTP/1.1\r\nX: \0\r\n\r\n", "HeaderValue"),
        ] {
            body.push_str(&format!(
                "request(t, {}, Error({error}()))\n",
                literal(input)
            ));
        }
        body.push_str(r#"
let request_bytes: Bytes = b"POST / HTTP/1.1\r\nContent-Length: 3\r\n\r\nabcNEXT"
match std@1/http.parse_server_request(request_bytes, server_config(1024, 3)) {
  Ok(Some((r, consumed))) => {
    test.check(t, r.method == "POST" && r.target == "/" && r.body == b"abc")
    match r.version { Http11() => (), _ => test.check(t, false) }
    test.check(t, consumed == bytes.len(request_bytes) - 4)
  },
  _ => test.check(t, false),
}
server_parse(t, b"POST / HTTP/1.1\r\nContent-Length: 3\r\n\r\nab", 1024, 3, Ok(None()))
server_parse(t, request_bytes, 1024, 2, Error(BodyTooLarge()))
server_parse(t, b"GET / HTTP/1.1\r\n\r\n", 17, 0, Error(HeadTooLarge()))
let invalid_lengths: List[String] = ["-1", "3x", "18446744073709551616", ""]
for value in invalid_lengths {
  let mut b: List[u8] = []
  std@1/text.append(mut b, "POST / HTTP/1.1\r\nContent-Length: ")
  std@1/text.append(mut b, value)
  std@1/text.append(mut b, "\r\n\r\n")
  server_parse(t, bytes.from_list(take b), 1024, 99, Error(Malformed(ContentLength())))
}
server_parse(t, b"POST / HTTP/1.1\r\nContent-Length: 1\r\nContent-Length: 2\r\n\r\nxx", 1024, 99, Error(Malformed(ContentLength())))
server_parse(t, b"POST / HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n", 1024, 99, Ok(None()))
server_parse(t, b"POST / HTTP/1.1\r\nTransfer-Encoding: gzip\r\n\r\n", 1024, 99, Error(Malformed(TransferEncoding())))
server_parse(t, b"POST / HTTP/1.1\r\nContent-Length: 0\r\nTransfer-Encoding: chunked\r\n\r\n", 1024, 99, Error(Malformed(TransferEncoding())))
"#);
        execute(&body);
    });
}

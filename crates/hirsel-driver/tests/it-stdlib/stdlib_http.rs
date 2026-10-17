//! Flow vectors ported first from Zig lib/std/http/Client.zig bff490a2:
//! sendBodiless, sendBodyComplete, receiveHead, redirect and fetch; body framing
//! follows lib/std/http.zig Reader.State. Scripted peers replace Zig's Io.
use hirsel_driver::*;
use hirsel_engine::*;

fn execute_source(source: &str) {
    with_compiler_stack(|| {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/std_source/main.hir");
        let mut program =
            load_program_source(&path, Some(source)).unwrap_or_else(|e| panic!("{}", details(&e)));
        program
            .check_for_execution()
            .unwrap_or_else(|e| panic!("{}", details(&e)));
        for (checked, seed) in [(false, 0), (true, 0), (false, 1)] {
            let config = RunConfiguration {
                entry: program.entry.clone(),
                hash_seed: seed,
                inputs: vec![],
            };
            let result = if checked {
                run_observed_checked(&mut program.engine, &config, SimulationConfig::default())
            } else {
                run_observed_snapshot(&mut program.engine, &config, SimulationConfig::default())
            }
            .unwrap();
            assert!(
                matches!(result.outcome, Ok(RunOutcome::Exit(0))),
                "{:?}",
                result.outcome
            );
        }
    });
}
fn details(e: &ToolError) -> String {
    match e {
        ToolError::Query {
            error: hirsel_types::QueryError::Diagnostics(d),
            sources,
            ..
        } => d
            .iter()
            .map(|d| {
                let file = sources.file(d.primary.file).unwrap();
                format!(
                    "{} {:?} {} at {}",
                    file.path.display(),
                    d.rule,
                    d.message,
                    file.location(d.primary.start).unwrap().line
                )
            })
            .collect::<Vec<_>>()
            .join("\n"),
        ToolError::Query { error, .. } => format!("{error:?}"),
        _ => e.to_string(),
    }
}

fn execute(body: &str) {
    execute_source(&format!(
        r#"
fn peer(take response: Bytes) -> NetPeer {{
  NetPeer(address: "example.com", port: 80, resolves_to: [], tls: Plain(),
    connections: [Open([Send(take response), Close()])])
}}
fn is_net_error(error: std@1/http.HttpError, wanted: NetError) -> bool {{
  match error {{ Net(actual) => actual == wanted, _ => false }}
}}
pub fn main(t: Test) -> Result[(), std@1/http.HttpError] {{
{body}
Ok(())
}}
"#
    ));
}

#[test]
fn zig_send_bodiless_and_content_length_response() {
    execute(
        r#"
let n = test.net(t, [NetPeer(address: "example.com", port: 80, resolves_to: [], tls: Plain(), connections: [Open([
  Expect(b"GET /items?q=x HTTP/1.1\r\nhost: example.com\r\nuser-agent: hirsel-std/1\r\nconnection: close\r\n\r\n"),
  Send(b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\nx-id: yes\r\n\r\nhello"), Close()
])])], [])
let response = std@1/http.fetch(n, std@1/http.default_config(), Get(), "http://example.com/items?q=x#ignored", [], None())?
test.check(t, response.status == 200 && response.body == b"hello")
test.check(t, std@1/http.header(response.headers, "X-ID") == Some("yes"))
"#,
    );
}

#[test]
fn zig_body_reader_framing_and_no_body_vectors() {
    for (response, method, expected) in [
        ("HTTP/1.1 200 OK\r\n\r\nread to eof", "Get", "read to eof"),
        ("HTTP/1.1 200 OK\r\nContent-Length: 99\r\n\r\n", "Head", ""),
        (
            "HTTP/1.1 204 No Content\r\nContent-Length: 99\r\n\r\n",
            "Get",
            "",
        ),
        (
            "HTTP/1.1 304 Not Modified\r\nContent-Length: 99\r\n\r\n",
            "Get",
            "",
        ),
        ("HTTP/1.1 103 Early Hints\r\n\r\n", "Get", ""),
        (
            "HTTP/1.1 100 Continue\r\n\r\nHTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok",
            "Get",
            "ok",
        ),
    ] {
        execute(&format!(
            "let n = test.net(t, [peer(b{response:?})], [])\nlet response = std@1/http.fetch(n, std@1/http.default_config(), {method}(), \"http://example.com\", [], None())?\ntest.check(t, response.body == b{expected:?})"
        ));
    }
}

#[test]
fn zig_chunked_body_and_trailers_across_reads() {
    execute(
        r#"
let n = test.net(t, [NetPeer(address: "example.com", port: 80, resolves_to: [], tls: Plain(), connections: [Open([
  Send(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n4;ext=yes\r"),
  Send(b"\nWi"), Send(b"ki\r\n5\r\npedia\r\n0\r\nx-check: yes\r\n\r\n"), Close()
])])], [])
let mut r = std@1/http.request(n, std@1/http.default_config(), Get(), "http://example.com", [])?
std@1/http.send_bodiless(mut r)?
let head = std@1/http.receive_head(mut r)?
test.check(t, head.chunked)
let body = std@1/http.read_all(mut r, 9)?
test.check(t, body == b"Wikipedia")
test.check(t, std@1/http.header(std@1/http.trailers(r), "X-Check") == Some("yes"))
"#,
    );
}

#[test]
fn zig_truncated_body_and_chunk_negatives() {
    for (response, expected) in [
        (
            "HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\nhi",
            "Truncated()",
        ),
        (
            "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n3\r\nab",
            "Truncated()",
        ),
        (
            "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n0\r\n",
            "Truncated()",
        ),
        (
            "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\nffxyz\r\n",
            "Chunk()",
        ),
        (
            "HTTP/1.1 200 OK\r\nTransfer-Encoding: gzip\r\n\r\n",
            "UnsupportedTransferEncoding()",
        ),
    ] {
        execute(&format!(
            "let n = test.net(t, [peer(b{response:?})], [])\nmatch std@1/http.fetch(n, std@1/http.default_config(), Get(), \"http://example.com\", [], None()) {{ Error({expected}) => (), _ => test.check(t, false) }}"
        ));
    }
}

#[test]
fn zig_redirect_method_and_resend_vectors() {
    for (status, method, body, next, expected) in [
        (301, "Post", "Some(b\"x\")", "GET", "ok"),
        (302, "Post", "Some(b\"x\")", "GET", "ok"),
        (303, "Put", "Some(b\"x\")", "GET", "ok"),
        (307, "Get", "None()", "GET", "ok"),
        (308, "Delete", "None()", "DELETE", "ok"),
    ] {
        execute(&format!(
            r#"
let n = test.net(t, [NetPeer(address: "example.com", port: 80, resolves_to: [], tls: Plain(), connections: [
  Open([Send(b"HTTP/1.1 {status} Redirect\r\nLocation: /next\r\nContent-Length: 0\r\n\r\n")]),
  Open([Expect(b"{next} /next HTTP/1.1\r\nhost: example.com\r\nuser-agent: hirsel-std/1\r\nconnection: close\r\n\r\n"), Send(b"HTTP/1.1 200 OK\r\n\r\nok")])
])], [])
let response = std@1/http.fetch(n, std@1/http.default_config(), {method}(), "http://example.com", [], {body})?
test.check(t, response.body == b"{expected}" && response.url == "http://example.com/next")
"#
        ));
    }
    for status in [301, 302, 307, 308] {
        execute(&format!(
            r#"
let n = test.net(t, [peer(b"HTTP/1.1 {status} Redirect\r\nLocation: /next\r\n\r\n")], [])
match std@1/http.fetch(n, std@1/http.default_config(), Put(), "http://example.com", [], Some(b"x")) {{
  Error(RedirectRequiresResend()) => (), _ => test.check(t, false),
}}
"#
        ));
    }
}

#[test]
fn origin_changes_strip_credentials_and_recheck_grants() {
    for (location, host, port, tls, authority) in [
        (
            "http://other.com/next",
            "other.com",
            80,
            "Plain()",
            "other.com",
        ),
        (
            "http://example.com:81/next",
            "example.com",
            81,
            "Plain()",
            "example.com:81",
        ),
        (
            "https://example.com/next",
            "example.com",
            443,
            "Certificate([\"example.com\"])",
            "example.com",
        ),
    ] {
        execute(&format!(
            r#"
let n = test.net(t, [peer(b"HTTP/1.1 302 Redirect\r\nLocation: {location}\r\nContent-Length: 0\r\n\r\n"),
  NetPeer(address: "{host}", port: {port}, resolves_to: [], tls: {tls}, connections: [Open([
    Expect(b"GET /next HTTP/1.1\r\nhost: {authority}\r\nuser-agent: hirsel-std/1\r\nconnection: close\r\nx-keep: yes\r\n\r\n"),
    Send(b"HTTP/1.1 200 OK\r\n\r\nok")])])], [])
let headers: List[std@1/http.Header] = [std@1/http.Header(name: "Authorization", value: "secret"), std@1/http.Header(name: "Cookie", value: "secret"),
 std@1/http.Header(name: "Proxy-Authorization", value: "secret"), std@1/http.Header(name: "x-keep", value: "yes")]
let response = std@1/http.fetch(n, std@1/http.default_config(), Get(), "http://example.com", headers, None())?
test.check(t, response.body == b"ok")
"#
        ));
    }
    execute(
        r#"
let n = test.net(t, [peer(b"HTTP/1.1 302 Redirect\r\nLocation: http://ungranted.com/\r\n\r\n")], [])
match std@1/http.fetch(n, std@1/http.default_config(), Get(), "http://example.com", [], None()) {
  Error(e) => test.check(t, is_net_error(e, NotGranted())), _ => test.check(t, false),
}
"#,
    );
}

#[test]
fn same_origin_redirect_retains_headers_and_budget_is_bounded() {
    execute(
        r#"
let n = test.net(t, [NetPeer(address: "example.com", port: 80, resolves_to: [], tls: Plain(), connections: [
 Open([Send(b"HTTP/1.1 302 Redirect\r\nLocation: /next\r\n\r\n")]),
 Open([Expect(b"GET /next HTTP/1.1\r\nhost: example.com\r\nuser-agent: hirsel-std/1\r\nconnection: close\r\nauthorization: secret\r\n\r\n"), Send(b"HTTP/1.1 200 OK\r\n\r\nok")])])], [])
let response = std@1/http.fetch(n, std@1/http.default_config(), Get(), "http://example.com", [std@1/http.Header(name: "authorization", value: "secret")], None())?
test.check(t, response.body == b"ok")
let denied = test.net(t, [peer(b"HTTP/1.1 302 Redirect\r\nLocation: /next\r\n\r\n")], [])
let mut config = std@1/http.default_config()
config.max_redirects = 0
match std@1/http.fetch(denied, config, Get(), "http://example.com", [], None()) {
 Error(TooManyRedirects()) => (), _ => test.check(t, false),
}
"#,
    );
}

#[test]
fn chunked_request_wire_and_order_errors() {
    execute(
        r#"
let n = test.net(t, [NetPeer(address: "example.com", port: 80, resolves_to: [], tls: Plain(), connections: [Open([
 Expect(b"POST / HTTP/1.1\r\nhost: example.com\r\nuser-agent: hirsel-std/1\r\nconnection: close\r\ntransfer-encoding: chunked\r\n\r\n2\r\nab\r\n1\r\nc\r\n0\r\nx-check: yes\r\n\r\n"),
 Send(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n")])])], [])
let mut r = std@1/http.request(n, std@1/http.default_config(), Post(), "http://example.com", [])?
match std@1/http.receive_head(mut r) { Error(OutOfOrder()) => (), _ => test.check(t, false) }
std@1/http.send_chunk(mut r, b"ab")?
std@1/http.send_chunk(mut r, b"")?
std@1/http.send_chunk(mut r, b"c")?
std@1/http.end_body(mut r, [std@1/http.Header(name: "x-check", value: "yes")])?
match std@1/http.send_chunk(mut r, b"bad") { Error(OutOfOrder()) => (), _ => test.check(t, false) }
let head = std@1/http.receive_head(mut r)?
test.check(t, head.status == 200 && std@1/http.read_all(mut r, 0)? == b"")
"#,
    );
}

#[test]
fn timeout_status_and_size_limits() {
    execute(
        r#"
let mut config = std@1/http.default_config()
config.read_timeout = Some(std@1/duration.from_parts(0, 1))
let n = test.net(t, [NetPeer(address: "example.com", port: 80, resolves_to: [], tls: Plain(), connections: [Open([Stall()])])], [])
match std@1/http.fetch(n, config, Get(), "http://example.com", [], None()) { Error(e) => test.check(t, is_net_error(e, TimedOut())), _ => test.check(t, false) }
config.connect_timeout = Some(std@1/duration.from_parts(0, 1))
let slow = test.net(t, [NetPeer(address: "example.com", port: 80, resolves_to: [], tls: Plain(), connections: [Unanswered()])], [])
match std@1/http.fetch(slow, config, Get(), "http://example.com", [], None()) { Error(e) => test.check(t, is_net_error(e, TimedOut())), _ => test.check(t, false) }
config.read_timeout = Some(std@1/duration.from_parts(0, 0))
match std@1/http.request(n, config, Get(), "http://example.com", []) { Error(e) => test.check(t, is_net_error(e, InvalidInput())), _ => test.check(t, false) }
let error_net = test.net(t, [peer(b"HTTP/1.1 404 Not Found\r\n\r\nmissing")], [])
match std@1/http.fetch(error_net, std@1/http.default_config(), Get(), "http://example.com", [], None()) { Error(Status(404)) => (), _ => test.check(t, false) }
config = std@1/http.default_config()
config.status_as_error = false
let ok_net = test.net(t, [peer(b"HTTP/1.1 500 Error\r\n\r\nbody")], [])
let response = std@1/http.fetch(ok_net, config, Get(), "http://example.com", [], None())?
test.check(t, response.status == 500 && response.body == b"body")
let large = test.net(t, [peer(b"HTTP/1.1 200 OK\r\n\r\n123")], [])
let mut r = std@1/http.request(large, config, Get(), "http://example.com", [])?
std@1/http.send_bodiless(mut r)?
std@1/http.receive_head(mut r)?;
match std@1/http.read_all(mut r, 2) { Error(BodyTooLarge()) => (), _ => test.check(t, false) }
config.max_head_bytes = 16
let big_head = test.net(t, [peer(b"HTTP/1.1 200 OK\r\nx: y\r\n\r\n")], [])
match std@1/http.fetch(big_head, config, Get(), "http://example.com", [], None()) { Error(HeadTooLarge()) => (), _ => test.check(t, false) }
"#,
    );
}

#[test]
fn json_round_trip_over_scripted_tls() {
    execute(
        r#"
let value: std@1/json.Value = Object({let mut fields: Map[String, box std@1/json.Value] = map.empty(); map.insert(mut fields, "name", box Text("Hirsel")); take fields})
let json = match std@1/json.stringify(value) { Ok(s) => take s, Error(_) => { test.check(t, false); return Ok(()) } }
let n = test.net(t, [NetPeer(address: "api.example.com", port: 443, resolves_to: [], tls: Certificate(["api.example.com"]), connections: [Open([
 Expect(b"POST /v1/items/7 HTTP/1.1\r\nhost: api.example.com\r\nuser-agent: hirsel-std/1\r\nconnection: close\r\ncontent-type: application/json\r\ncontent-length: 17\r\n\r\n{\"name\":\"Hirsel\"}"),
 Send(b"HTTP/1.1 200 OK\r\ncontent-type: application/json\r\nContent-Length: 17\r\n\r\n{\"name\":\"Hirsel\"}")])])], [])
let response = std@1/http.fetch(n, std@1/http.default_config(), Post(), "https://api.example.com/v1/items/7", [std@1/http.Header(name: "content-type", value: "application/json")], Some(string.as_bytes(json)))?
let decoded = match string.from_bytes(copy response.body) { Ok(s) => take s, Error(_) => { test.check(t, false); return Ok(()) } }
let parsed_value = match std@1/json.parse(take decoded) { Ok(v) => take v, Error(_) => { test.check(t, false); return Ok(()) } }
test.check(t, parsed_value == value)
"#,
    );
}

#[test]
fn injection_bad_targets_and_redirect_locations_are_data_errors() {
    execute(
        r#"
let n = test.net(t, [], [])
let invalid_headers: List[std@1/http.Header] = [std@1/http.Header(name: "x\r\nbad", value: "v"), std@1/http.Header(name: "x:y", value: "v")]
for h in invalid_headers {
 match std@1/http.request(n, std@1/http.default_config(), Get(), "http://example.com", [h]) {
   Error(Malformed(HeaderName())) => (), _ => test.check(t, false),
 }
}
match std@1/http.request(n, std@1/http.default_config(), Get(), "http://example.com", [std@1/http.Header(name: "x", value: "v\r\ninjected: yes")]) {
 Error(Malformed(HeaderValue())) => (), _ => test.check(t, false),
}
match std@1/http.request(n, std@1/http.default_config(), Get(), "http://example.com/a b", []) {
 Error(Url(UnexpectedCharacter())) => (), _ => test.check(t, false),
}
match std@1/http.request(n, std@1/http.default_config(), Get(), "ftp://example.com", []) {
 Error(UnsupportedScheme()) => (), _ => test.check(t, false),
}
let no_location = test.net(t, [peer(b"HTTP/1.1 301 Redirect\r\n\r\n")], [])
match std@1/http.fetch(no_location, std@1/http.default_config(), Get(), "http://example.com", [], None()) {
 Error(RedirectLocationInvalid()) => (), _ => test.check(t, false),
}
let bad_location = test.net(t, [peer(b"HTTP/1.1 301 Redirect\r\nLocation: http://127.1\r\n\r\n")], [])
match std@1/http.fetch(bad_location, std@1/http.default_config(), Get(), "http://example.com", [], None()) {
 Error(RedirectLocationInvalid()) => (), _ => test.check(t, false),
}
"#,
    );
}

#[test]
fn header_batches_preserve_framing_limits_and_first_lookup() {
    let prefix = (0..17).map(|i| format!("x-{i}: v\r\n")).collect::<String>();
    let response = format!("HTTP/1.1 200 OK\r\n{prefix}Content-Length: 2\r\nx-0: second\r\n\r\nok");
    execute(&format!(
        r#"
let n = test.net(t, [peer(b{response:?})], [])
let response = std@1/http.fetch(n, std@1/http.default_config(), Get(), "http://example.com", [], None())?
test.check(t, response.body == b"ok" && list.len(response.headers) == 19)
test.check(t, std@1/http.header(response.headers, "X-0") == Some("v"))
"#
    ));
    for (fields, expected) in [
        (
            format!("Content-Length: 2\r\n{prefix}Content-Length: 3\r\n"),
            "Malformed(ContentLength())",
        ),
        (
            format!("Content-Length: 2\r\n{prefix}transfer-encoding: chunked\r\n"),
            "Malformed(TransferEncoding())",
        ),
    ] {
        let response = format!("HTTP/1.1 200 OK\r\n{fields}\r\nok");
        execute(&format!(
            r#"
let n = test.net(t, [peer(b{response:?})], [])
match std@1/http.fetch(n, std@1/http.default_config(), Get(), "http://example.com", [], None()) {{
 Error({expected}) => (), _ => test.check(t, false),
}}
"#
        ));
    }
    execute(
        r#"
let n = test.net(t, [peer(b"HTTP/1.1 200 OK\r\na: b\r\nc: d\r\n\r\n")], [])
let mut config = std@1/http.default_config()
config.max_headers = 1
match std@1/http.fetch(n, config, Get(), "http://example.com", [], None()) {
 Error(Malformed(TooManyHeaders())) => (), _ => test.check(t, false),
}
"#,
    );
}

#[test]
fn non_utf8_values_fail_but_informational_reasons_are_empty() {
    execute(
        r#"
let n = test.net(t, [peer(b"HTTP/1.1 200 \xff\r\ncontent-length: 0\r\n\r\n")], [])
let mut r = std@1/http.request(n, std@1/http.default_config(), Get(), "http://example.com", [])?
std@1/http.send_bodiless(mut r)?
let head = std@1/http.receive_head(mut r)?
test.check(t, head.status == 200 && head.reason == "")
let invalid = test.net(t, [peer(b"HTTP/1.1 200 OK\r\nx-obs: \xff\r\n\r\n")], [])
match std@1/http.fetch(invalid, std@1/http.default_config(), Get(), "http://example.com", [], None()) {
 Error(Malformed(HeaderValue())) => (), _ => test.check(t, false),
}
"#,
    );
}

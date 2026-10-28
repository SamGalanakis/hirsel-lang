//! Flow vectors ported first from Zig lib/std/http/Client.zig bff490a2:
//! sendBodiless, sendBodyComplete, receiveHead, redirect and fetch; body framing
//! follows lib/std/http.zig Reader.State. Scripted peers replace Zig's Io.
use hirsel_driver::*;
use hirsel_engine::*;

fn execute_source(source: &str) {
    with_compiler_stack(|| {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/std_source/main.hir");
        let prepared = crate::source_std::tir(&path, Some(source), None)
            .unwrap_or_else(|e| panic!("{}", details(&e)));
        for (checked, seed) in [(false, 0), (true, 0), (false, 1)] {
            let config = RunConfiguration {
                entry: prepared.entry.clone(),
                hash_seed: seed,
                inputs: vec![],
            };
            let result = testing::observe_tir(
                &prepared,
                &config,
                SimulationConfig::default(),
                InterpreterAdapter::default().limits,
                checked,
            )
            .unwrap();
            assert!(
                matches!(
                    result.outcome,
                    Ok(RunOutcome::Returned(hirsel_run_schema::Returned::Ok))
                ),
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
                let file = sources.file(sources.resolve(d.primary).file).unwrap();
                format!(
                    "{} {:?} {} at {}",
                    file.path.display(),
                    d.rule,
                    d.message,
                    file.location(sources.resolve(d.primary).start)
                        .unwrap()
                        .line
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
std@1/http.write_request_body(mut r, b"ab")?
std@1/http.write_request_body(mut r, b"")?
std@1/http.write_request_body(mut r, b"c")?
std@1/http.end_request_body(mut r, [std@1/http.Header(name: "x-check", value: "yes")])?
match std@1/http.write_request_body(mut r, b"bad") { Error(OutOfOrder()) => (), _ => test.check(t, false) }
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
let value: std@1/json.Value = Object({let mut fields: Map[String, std@1/json.Value] = map.empty(); map.insert(mut fields, "name", Text("Hirsel")); take fields})
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

// A deterministic slow client: each successful read advances the shared clock.
// The normal library records the clock values and socket completions for replay.
struct HttpClockProgress {
    library: hirsel_interp::library::Library,
    ticks_per_read: u64,
}
impl hirsel_interp::OperationsAdapter for HttpClockProgress {
    fn operations(&self) -> &dyn hirsel_interp::Operations {
        &self.library
    }
    fn operations_mut(&mut self) -> &mut dyn hirsel_interp::Operations {
        &mut self.library
    }
    fn call_resolved(
        &mut self,
        operation: hirsel_interp::OperationId,
        call: hirsel_interp::OperationCall<'_>,
        context: &mut hirsel_interp::OperationContext<'_>,
    ) -> Result<Option<hirsel_interp::OperationResult>, hirsel_interp::Error> {
        let name = call.path;
        let read = name.0.package.package_is("core", "1")
            && name.0.modules.iter().map(|s| s.as_str()).eq(["net"])
            && matches!(name.0.name.as_str(), "read" | "read_into");
        let result =
            hirsel_interp::Operations::call_resolved(&mut self.library, operation, call, context)?;
        if read {
            self.library.simulation.config.monotonic += self.ticks_per_read;
        }
        Ok(result)
    }
}

fn http_progress_events(
    prepared: &hirsel_driver::requests::TirExecution,
    run: &RunConfiguration,
    config: SimulationConfig,
    ticks: u64,
) -> Vec<hirsel_tir::CapabilityEvent> {
    let mut library = hirsel_interp::library::Library::new(run, config).unwrap();
    let arguments = library
        .entry_arguments(&prepared.runtime.types, &prepared.signature)
        .unwrap();
    let mut operations = HttpClockProgress {
        library,
        ticks_per_read: ticks,
    };
    let execution = hirsel_interp::run_with_runtime(
        &mut prepared.provider(),
        &prepared.sources,
        run,
        arguments,
        hirsel_interp::Limits::default(),
        &mut operations,
        &prepared.runtime,
    )
    .unwrap();
    assert!(
        matches!(
            execution.outcome,
            hirsel_interp::Outcome::Returned {
                value: hirsel_interp::Value::Variant { variant: 0, .. },
                ..
            }
        ),
        "{:?}",
        execution.outcome
    );
    operations.library.simulation.finish_replay().unwrap();
    std::mem::take(&mut operations.library.simulation.events)
}

// The corpus has no clock grant seam. Exercise timed scripted listeners here and
// replay every capability completion. Virtual progress catches a per-read reset.
#[test]
fn server_request_deadlines_close_partial_heads_and_bodies_and_replay() {
    with_compiler_stack(|| {
        let source = r#"
pub type AppError = Grant(GrantError) | Network(NetError) | Http(http.HttpError)
pub fn main(rt: Io, args: Args) -> Result[(), AppError] {
  let n = io.net(rt) ? Grant
  let clock = io.monotonic_clock(rt) ? Grant
  let addr = SocketAddr(ip: V4([127u8, 0u8, 0u8, 1u8]), port: 8080u16)
  let mut listener = net.listen(n, addr) ? Network
  let (stream, _) = net.accept(n, mut listener) ? Network
  let mut config = http.default_config()
  config.read_timeout = Some(duration.from_parts(0, 1000))
  let mut connection = http.server_connection(io.tasks(rt) ? Grant, n, take stream, config)
  match http.next_stream(mut connection, Some(clock)) {
    Error(RequestTimeout()) => (),
    _ => failure.check(false, Arithmetic()),
  }
  match http.next_stream(mut connection, Some(clock)) {
    Ok(None()) => (),
    _ => failure.check(false, Arithmetic()),
  }
  Ok(())
}
"#;
        let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../conformance/SEM/SEM-1-http-wire-run.hir");
        let prepared = crate::source_std::tir(&path, Some(source), None)
            .unwrap_or_else(|e| panic!("{}", details(&e)));
        for (fragments, ticks) in [
            (vec![b"GET / HTTP/1.1\r\nHost:".to_vec()], 0u64),
            (
                vec![b"POST / HTTP/1.1\r\nContent-Length: 3\r\n\r\na".to_vec()],
                0,
            ),
            (
                vec![b"POST / HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n3\r\nab".to_vec()],
                0,
            ),
            (
                vec![
                    b"POST / HTTP/1.1\r\nContent-Length: 3\r\n\r\na".to_vec(),
                    b"b".to_vec(),
                    b"c".to_vec(),
                ],
                400,
            ),
        ] {
            let response =
                b"HTTP/1.1 408 \r\nContent-Length: 15\r\nConnection: close\r\n\r\nRequest Timeout";
            let mut steps: Vec<serde_json::Value> = fragments
                .into_iter()
                .map(|bytes| serde_json::json!({"send": bytes}))
                .collect();
            steps.push(serde_json::json!({"expect": response.as_slice()}));
            steps.push(serde_json::json!("close"));
            let mut config = SimulationConfig::default();
            config.grants.insert("MonotonicClock".into());
            config.net = Some(
                serde_json::from_value(serde_json::json!({
                    "provider": "net.script@2",
                    "rules": [{"address": "127.0.0.1", "service": "8080", "operation": "listen"}],
                    "peers": [],
                    "listeners": [{"address": "127.0.0.1", "port": 8080, "clients": [{
                        "address": "127.0.0.1", "port": 40001, "tls": "plain",
                        "steps": steps
                    }]}]
                }))
                .unwrap(),
            );
            let run = RunConfiguration {
                entry: prepared.entry.clone(),
                hash_seed: 0,
                inputs: vec![],
            };
            let events = http_progress_events(&prepared, &run, config.clone(), ticks);
            if ticks == 0 {
                let replay = testing::observe_tir(
                    &prepared,
                    &RunConfiguration {
                        inputs: events,
                        ..run
                    },
                    config,
                    hirsel_interp::Limits::default(),
                    false,
                )
                .unwrap();
                assert!(
                    matches!(
                        replay.outcome,
                        Ok(RunOutcome::Returned(hirsel_run_schema::Returned::Ok))
                    ),
                    "{:?}",
                    replay.outcome
                );
            } else {
                // Simulation inputs validate the provider's responses, so replay the
                // advancing-clock provider as well as the scripted socket world.
                let replay = http_progress_events(
                    &prepared,
                    &RunConfiguration {
                        inputs: events.clone(),
                        ..run
                    },
                    config,
                    ticks,
                );
                assert_eq!(replay, events);
            }
        }
    });
}

// Corpus outcomes check wire behavior; this additionally checks the recorded
// provider exchanges and resource cleanup replay to the same observation.
#[test]
fn pooled_client_scripts_replay_without_network_authority() {
    with_compiler_stack(|| {
        for relative in [
            "conformance/SEM/SEM-1-http-client-length-reuse-run.hir",
            "conformance/SEM/SEM-1-http-client-server-close-run.hir",
            "conformance/SEM/SEM-1-http-client-post-reset-run.hir",
            "conformance/SEM/SEM-1-http-client-idle-expiry-run.hir",
            "conformance/CONC/CONC-3-http-client-deadline-run.hir",
        ] {
            assert_http_script_replay(relative, |_| HttpScriptConnection::Connect);
        }
    });
}

#[derive(Clone, Copy)]
enum HttpScriptConnection {
    Connect,
    Accept,
}

fn assert_http_script_replay(relative: &str, connection: impl Fn(&str) -> HttpScriptConnection) {
    use hirsel_interp::observation::Effect;
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(relative);
    let mut program =
        load_program_source(&path, None).unwrap_or_else(|e| panic!("{}", details(&e)));
    program
        .check_for_execution()
        .unwrap_or_else(|e| panic!("{}", details(&e)));
    let source = std::fs::read_to_string(&path).unwrap();
    let entries = if source.lines().any(|line| line == "// expect: test") {
        let tests = testing::discover(&mut program, &[], None, testing::Selection::Tests)
            .unwrap_or_else(|e| panic!("{}", details(&e)));
        assert!(!tests.is_empty(), "{relative}: no tests discovered");
        tests
            .into_iter()
            .map(|test| (test.entry, test.definition.0.name.to_string()))
            .collect()
    } else {
        vec![(program.entry.clone(), program.entry.0.name.to_string())]
    };
    let mut simulation = SimulationConfig::default();
    if source.lines().any(|line| line == "// grant: clock") {
        simulation.grants.insert("MonotonicClock".into());
    }
    if let Some(net) = source
        .lines()
        .find_map(|line| line.strip_prefix("// net: "))
    {
        simulation.net = Some(serde_json::from_str(net).unwrap());
    }
    for (entry, name) in entries {
        let config = RunConfiguration {
            entry,
            hash_seed: 0,
            inputs: vec![],
        };
        let recorded =
            run_observed_snapshot(&mut program.engine, &config, simulation.clone()).unwrap();
        assert!(
            matches!(
                recorded.outcome,
                Ok(RunOutcome::Returned(hirsel_run_schema::Returned::Ok))
            ),
            "{:?}",
            recorded.outcome
        );
        let observation = recorded.observation.unwrap();
        let events: Vec<_> = observation
            .effects
            .iter()
            .filter_map(|effect| match &effect.effect {
                Effect::CapabilityConsumed(event) => Some(event.clone()),
                _ => None,
            })
            .collect();
        let count = |operation| events.iter().filter(|e| e.operation == operation).count();
        match connection(&name) {
            HttpScriptConnection::Connect => {
                assert!(count("net.connect") > 0, "{relative}::{name}: no connect");
                assert_eq!(count("net.listen"), 0, "{relative}::{name}");
                assert_eq!(count("net.accept"), 0, "{relative}::{name}");
            }
            HttpScriptConnection::Accept => {
                assert_eq!(count("net.connect"), 0, "{relative}::{name}");
                assert_eq!(count("net.listen"), 1, "{relative}::{name}");
                assert_eq!(count("net.accept"), 1, "{relative}::{name}");
            }
        }
        let mut output = Vec::new();
        for line in source
            .lines()
            .filter_map(|line| line.strip_prefix("// stdout: "))
        {
            output.extend_from_slice(line.as_bytes());
            output.push(b'\n');
        }
        assert_eq!(observation.output, output, "{relative}::{name}: output");
        assert_eq!(observation.exit_code, Some(0), "{relative}::{name}");
        assert!(observation.returned.is_some(), "{relative}::{name}");
        assert!(observation.failures.is_empty(), "{relative}::{name}");
        assert!(observation.blocked.is_none(), "{relative}::{name}");
        assert!(observation.signaled.is_none(), "{relative}::{name}");
        assert!(events.iter().any(|e| e.operation == "net.close"));
        if relative.ends_with("http-server-inline-errors-run.hir") {
            assert_eq!(count("net.write"), 0, "invalid response emitted bytes");
            assert_eq!(count("net.write_all"), 0, "invalid response emitted bytes");
            assert_eq!(count("net.close"), 9, "one close per rejected exchange");
            assert_eq!(count("net.shutdown"), 0);
        }
        if relative.ends_with("http-server-stream-drop-run.hir") {
            assert_eq!(
                events.iter().filter(|e| e.operation == "net.close").count(),
                1
            );
            assert!(!events.iter().any(|e| e.operation == "net.shutdown"));
            assert_eq!(events.last().unwrap().operation, "net.close");
        }
        let replayed = run_observed_snapshot(
            &mut program.engine,
            &RunConfiguration {
                inputs: events,
                ..config
            },
            simulation.clone(),
        )
        .unwrap();
        assert_eq!(replayed.observation.as_ref(), Some(&observation));
        assert!(
            matches!(
                replayed.outcome,
                Ok(RunOutcome::Returned(hirsel_run_schema::Returned::Ok))
            ),
            "{:?}",
            replayed.outcome
        );
    }
}

#[test]
fn streaming_http_scripts_replay_writes_completion_and_drop() {
    with_compiler_stack(|| {
        for relative in [
            "conformance/SEM/SEM-1-http-server-inline-run.hir",
            "conformance/SEM/SEM-1-http-server-output-reuse-run.hir",
            "conformance/SEM/SEM-1-http-server-inline-errors-run.hir",
            "conformance/SEM/SEM-1-http-server-stream-run.hir",
            "conformance/SEM/SEM-1-http-server-stream-drop-run.hir",
            "conformance/SEM/SEM-1-http-server-stream-malformed-run.hir",
            "conformance/SEM/SEM-1-http-client-stream-run.hir",
            "conformance/SEM/SEM-1-http-headers-many-run.hir",
            "conformance/SEM/SEM-1-http-headers-limits-run.hir",
        ] {
            assert_http_script_replay(relative, |_| HttpScriptConnection::Connect);
        }
    });
}

// The corpus checks headers and body semantics. Only the host observation API
// can additionally verify that the same provider effects replay without IO.
#[test]
fn http2_adapter_scripts_record_and_replay() {
    with_compiler_stack(|| {
        for relative in [
            "conformance/SEM/SEM-1-http-h2-client-run.hir",
            "conformance/SEM/SEM-1-http-h2-server-run.hir",
        ] {
            assert_http_script_replay(relative, |name| match (relative, name) {
                (
                    "conformance/SEM/SEM-1-http-h2-server-run.hir",
                    "peer_goaway_closes_gracefully",
                ) => HttpScriptConnection::Accept,
                _ => HttpScriptConnection::Connect,
            });
        }
    });
}

// The full 250-stream vector exceeds the corpus's 1M instruction cap even with
// indexed stream storage. Keep its original acceptance/refusal assertions here.
#[test]
fn http2_250_concurrent_streams_and_refusals_agree_on_all_engines() {
    with_compiler_stack(|| {
        let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/http2-stream-limit.hir");
        let prepared = crate::source_std::tir(&fixture, None, None).unwrap();
        let config = RunConfiguration {
            entry: prepared.entry.clone(),
            hash_seed: 0,
            inputs: vec![],
        };
        for checked in [false, true] {
            let observed = testing::observe_tir(
                &prepared,
                &config,
                SimulationConfig::default(),
                hirsel_interp::Limits {
                    steps: Some(12_000_000),
                    ..Default::default()
                },
                checked,
            )
            .unwrap();
            assert!(
                matches!(
                    observed.outcome,
                    Ok(RunOutcome::Returned(hirsel_run_schema::Returned::Ok))
                ),
                "checked={checked}: {:?}",
                observed.outcome
            );
        }
        // Release the completed interpreter graph before admitting a native graph.
        drop(prepared);
        for backend in [native::Backend::Llvm, native::Backend::Cranelift] {
            let mut stdout = Vec::new();
            let mut stderr = Vec::new();
            let outcome = native::run_path_native_backend(
                &fixture,
                HostLaunch::empty(),
                None,
                backend,
                native::Output::Streams {
                    stdout: &mut stdout,
                    stderr: &mut stderr,
                },
                Verify::On,
            )
            .unwrap()
            .unwrap();
            assert!(
                matches!(
                    outcome,
                    RunOutcome::Returned(hirsel_run_schema::Returned::Ok)
                ),
                "{backend:?}: {outcome:?}; {}",
                String::from_utf8_lossy(&stderr)
            );
            assert!(stdout.is_empty(), "{backend:?}: {stdout:?}");
        }
    });
}

#[test]
fn http2_session_freeze_controls_agree_with_checked_interpreter() {
    with_compiler_stack(|| {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../native-tests/regressions/http2-session-freeze.hir");
        let mut program = load_program_source(&path, None).unwrap();
        let tests = testing::discover(&mut program, &[], None, testing::Selection::Tests).unwrap();
        assert_eq!(tests.len(), 4);
        for test in tests {
            let config = RunConfiguration {
                entry: test.entry,
                hash_seed: 0,
                inputs: vec![],
            };
            let simulation = SimulationConfig {
                test_seed: test.seed,
                ..Default::default()
            };
            let plain =
                run_observed_snapshot(&mut program.engine, &config, simulation.clone()).unwrap();
            let checked = run_observed_checked(&mut program.engine, &config, simulation).unwrap();
            for result in [&plain, &checked] {
                assert!(
                    matches!(
                        result.outcome,
                        Ok(RunOutcome::Returned(hirsel_run_schema::Returned::Ok))
                    ),
                    "{}: {:?}",
                    test.definition,
                    result.outcome
                );
            }
            assert_eq!(
                plain.observation, checked.observation,
                "{}",
                test.definition
            );
        }
    });
}
// Bodies around the existing inline/direct-write boundary exceed the corpus
// budget. The Rust control keeps its own fixed budget and compares full IO
// observations on H, LLVM and Cranelift, as well as scripted replay.
#[test]
fn inline_output_boundary_aliases_and_replay() {
    with_compiler_stack(|| {
        let payload = "x".repeat(65537);
        let mut source = format!(
            r#"
fn network(e: NetError) -> http.HttpError {{ Net(e) }}
pub fn main(t: Test) -> Result[(), http.HttpError] {{
  let payload = b{payload:?}
  let name = "X-Alias"
  let value = "kept"
"#
        );
        // Each connection grows once, then reuses its initialized prefix for
        // shorter replies. The separate native/socket experiment also covers
        // repeated full large-body overwrites; keep H's audited case bounded.
        for size in [0, 3, 65535, 65536, 65537] {
            let mut requests = String::new();
            let mut expectations = String::new();
            let mut responses = String::new();
            for (i, (body, method, status)) in [
                (size, "GET", 200),
                (0, "HEAD", 200),
                (0, "GET", 204),
                (0, "GET", 304),
                (3, "GET", 200),
            ]
            .into_iter()
            .enumerate()
            {
                requests.push_str(&format!(
                    "{method} /{i} HTTP/1.1\r\nHost: example.com\r\n\r\n"
                ));
                let mut wire = format!("HTTP/1.1 {status} \r\nX-Alias: kept\r\n");
                if status == 200 {
                    let length = if method == "HEAD" { 65537 } else { body };
                    wire.push_str(&format!("Content-Length: {length}\r\n"));
                }
                wire.push_str("Connection: keep-alive\r\n\r\n");
                wire.push_str(&payload[..body]);
                expectations.push_str(&format!("Expect(b{wire:?}),\n"));
                let body_size = if method == "HEAD" || status != 200 {
                    65537
                } else {
                    body
                };
                responses.push_str(&format!(
                    r#"
if http.server_request_target(r) == "/{i}" {{
  return Ok(http.ServerResponse(status: {status}u16,
    headers: [http.Header(name: unique name, value: unique value)],
    body: unique bytes.slice(payload, 0, {body_size}), close: false))
}}
"#
                ));
            }
            source.push_str(&format!(
                r#"
{{
  let n = test.net(t, [NetPeer(address: "example.com", port: 80u16,
    resolves_to: [], tls: Plain(), connections: [Open([
      Send(b{requests:?}), {expectations} Close()
    ])])], [])
  let socket = net.connect(n, "example.com", 80u16, Unbounded()) ? network
  let mut c = http.server_connection(test.tasks(t), n, take socket, http.default_config())
  for _ in 0u64..5u64 {{
    test.check(t, http.serve_next(mut c, None(),
      fn(r: http.ServerRequestView) -> Result[http.ServerResponse, http.HttpError] uses {{}} {{
        {responses}
        Error(OutOfOrder())
      }} )?)
    test.check(t, bytes.len(payload) == 65537 && payload[0] == 120u8 && payload[65536] == 120u8)
    test.check(t, name == "X-Alias" && value == "kept")
  }}
}}
"#
            ));
        }
        source.push_str("Ok(())\n}\n");
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/std_source/main.hir");
        let mut program =
            load_program_source(&path, Some(&source)).unwrap_or_else(|e| panic!("{}", details(&e)));
        program
            .check_for_execution()
            .unwrap_or_else(|e| panic!("{}", details(&e)));
        program.engine.adapters.interpreter = Some(Box::new(InterpreterAdapter {
            limits: hirsel_interp::Limits {
                steps: Some(16_000_000),
                ..Default::default()
            },
            ..Default::default()
        }));
        let config = RunConfiguration {
            entry: program.entry.clone(),
            hash_seed: 0,
            inputs: vec![],
        };
        let recorded =
            run_observed_snapshot(&mut program.engine, &config, SimulationConfig::default())
                .unwrap();
        assert!(
            matches!(
                recorded.outcome,
                Ok(RunOutcome::Returned(hirsel_run_schema::Returned::Ok))
            ),
            "{:?}",
            recorded.outcome
        );
        let observation = recorded.observation.unwrap();
        let checked =
            run_observed_checked(&mut program.engine, &config, SimulationConfig::default())
                .unwrap();
        assert_eq!(checked.observation.as_ref(), Some(&observation));
        let events = observation
            .effects
            .iter()
            .filter_map(|e| match &e.effect {
                hirsel_interp::observation::Effect::CapabilityConsumed(e) => Some(e.clone()),
                _ => None,
            })
            .collect();
        let replayed = run_observed_snapshot(
            &mut program.engine,
            &RunConfiguration {
                inputs: events,
                ..config
            },
            SimulationConfig::default(),
        )
        .unwrap();
        assert_eq!(replayed.observation.as_ref(), Some(&observation));
        let expected = native::Observation::of_reference(&observation);
        let input = program.engine.export_native_input().unwrap();
        let validated = program.engine.import_native_input(input).unwrap();
        let options = native::Options::new(native::TargetSpec::x86_64_unknown_linux_gnu());
        let matrix = vec![
            native::Configuration {
                options: options.clone(),
                engine: native::Engine::Heap { campaign: false },
            },
            native::Configuration {
                options: options.clone(),
                engine: native::Engine::Native {
                    opt: native::OptLevel::Speed,
                    partition: native::Partition::Whole,
                    runtime: native::Runtime::Diagnostic,
                },
            },
            native::Configuration {
                options,
                engine: native::Engine::Cranelift {
                    partition: native::Partition::Whole,
                    runtime: native::Runtime::Diagnostic,
                },
            },
        ];
        let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("http1-output-boundary");
        std::fs::create_dir_all(&dir).unwrap();
        let tools = native::Toolchain::for_matrix(&dir.join("runtime"), &matrix).unwrap();
        for (name, result) in native::differential_in(
            &validated,
            &program.entry,
            &expected,
            &matrix,
            &tools,
            &dir,
            &native::Fixture {
                steps: Some(16_000_000),
                ..Default::default()
            },
        ) {
            assert_eq!(result, native::Outcome::Agree, "{name}: {result:?}");
        }
    });
}

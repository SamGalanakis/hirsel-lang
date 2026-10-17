//! Zig lib/std/Uri.zig's 22 test groups, bff490a2 (MIT).
//! Resolution adds the actual RFC3986 §5.4 reference/target pairs; Zig's named
//! RFC test lists only absolute results. See third-party/NOTICES.md.
use super::stdlib_net::{execute, execute_source};

#[test]
fn parse_result_survives_helper_scope_exit() {
    execute(
        "match std@1/uri.parse(\"x:\") { Ok(value) => core@1/test.check(t, value.scheme == \"x\"), Error(_) => core@1/test.check(t, false) }",
    );
}

#[test]
fn deep_copy_of_borrowed_string_has_no_parameter_root() {
    execute_source(
        "fn own(text: String) -> String uses {} { unique text }\n\
         pub fn main(t: Test) -> Result[(), ()] {\n\
           let value = own(\"x\")\n\
           core@1/test.check(t, value == \"x\")\n\
           Ok(())\n\
         }\n",
    );
}

fn parsed(text: &str, checks: &str) -> String {
    format!(
        "{{ let parsed_uri = match std@1/uri.parse({text:?}) {{ Ok(parsed_uri) => take parsed_uri, Error(_) => {{core@1/test.check(t, false); return Error(())}} }}\n{checks}\n}}\n"
    )
}
fn string_field(field: &str, expected: &str) -> String {
    format!("core@1/test.check(t, parsed_uri.{field} == {expected:?})\n")
}
fn component(field: &str, expected: Option<&str>) -> String {
    let expected = match expected {
        Some(text) => format!("Some(Encoded({text:?}))"),
        None => "None()".into(),
    };
    format!("core@1/test.check(t, parsed_uri.{field} == {expected})\n")
}
fn bad(texts: &[&str], expected: &str) {
    for chunk in texts.chunks(10) {
        let body: String = chunk.iter().map(|text| format!("match std@1/uri.parse({text:?}) {{ Error({expected}()) => (), _ => core@1/test.check(t, false) }}\n")).collect();
        execute(&body);
    }
}

#[test]
fn zig_basic_and_with_port() {
    execute(&parsed(
        "https://ziglang.org/download",
        &(string_field("scheme", "https")
            + &component("host", Some("ziglang.org"))
            + "core@1/test.check(t, parsed_uri.path == Encoded(\"/download\"))\ncore@1/test.check(t, parsed_uri.port == None())"),
    ));
    execute(&parsed(
        "http://example:1337/",
        &(string_field("scheme", "http")
            + &component("host", Some("example"))
            + "core@1/test.check(t, parsed_uri.path == Encoded(\"/\"))\ncore@1/test.check(t, parsed_uri.port == Some(1337))"),
    ));
}
#[test]
fn zig_should_fail_gracefully_and_name_too_long() {
    bad(&["foobar://"], "InvalidFormat");
    bad(&[&format!("http://{}", "Z".repeat(255))], "InvalidHostName");
}
#[test]
fn zig_file() {
    for (text, host, path) in [
        ("file:///", None, "/"),
        (
            "file:///an/absolute/path/to/something",
            None,
            "/an/absolute/path/to/something",
        ),
        (
            "file://localhost/an/absolute/path/to/another/thing/",
            Some("localhost"),
            "/an/absolute/path/to/another/thing/",
        ),
        ("file:/an/absolute/path", None, "/an/absolute/path"),
    ] {
        execute(&parsed(
            text,
            &(string_field("scheme", "file")
                + &component("host", host)
                + &format!("core@1/test.check(t, parsed_uri.path == Encoded({path:?}))")),
        ));
    }
}
#[test]
fn zig_scheme() {
    let mut body = String::new();
    for scheme in ["http", "scheme-mee", "a.b.c", "ab+", "X+++", "Y+-."] {
        body += &parsed(&format!("{scheme}:_"), &string_field("scheme", scheme));
    }
    execute(&body);
    bad(&[""], "InvalidFormat");
    bad(&[":", "-:", "+hTTp:", "$:", "h$:"], "UnexpectedCharacter");
}
#[test]
fn zig_authority_and_password() {
    let users = [
        ("hostname", None, None, None),
        ("userinfo@hostname", Some("userinfo"), None, None),
        ("userinfo@", Some("userinfo"), None, None),
        (
            "user:password@hostname",
            Some("user"),
            Some("password"),
            None,
        ),
        ("hostname:0", None, None, Some(0)),
        ("hostname:1234", None, None, Some(1234)),
        ("userinfo@hostname:1234", Some("userinfo"), None, Some(1234)),
        (
            "user:password@hostname:1234",
            Some("user"),
            Some("password"),
            Some(1234),
        ),
        (
            "user:p@ssword@hostname:1234",
            Some("user"),
            Some("p@ssword"),
            Some(1234),
        ),
        (
            "user:p@@@word@hostname:1234",
            Some("user"),
            Some("p@@@word"),
            Some(1234),
        ),
        ("user@name@hostname", Some("user@name"), None, None),
        (
            "user@name@hostname:1234",
            Some("user@name"),
            None,
            Some(1234),
        ),
        ("username@a", Some("username"), None, None),
        ("username:@a", Some("username"), None, None),
        (
            "username:password@a",
            Some("username"),
            Some("password"),
            None,
        ),
        ("username::@a", Some("username"), Some(":"), None),
    ];
    for chunk in users.chunks(5) {
        let mut body = String::new();
        for &(authority, user, password, port) in chunk {
            let host = if authority.ends_with('@') {
                None
            } else if authority.ends_with('a') {
                Some("a")
            } else {
                Some("hostname")
            };
            let port = port.map_or_else(|| "None()".into(), |p| format!("Some({p})"));
            body += &parsed(
                &format!("scheme://{authority}"),
                &(component("user", user)
                    + &component("password", password)
                    + &component("host", host)
                    + &format!("core@1/test.check(t, parsed_uri.port == {port})")),
            );
        }
        execute(&body);
    }
}
#[test]
fn zig_authority_dns_ipv4_ipv6() {
    let hosts = [
        "a",
        "a.b",
        "example.com",
        "www.example.com",
        "example.org.",
        "www.example.org.",
        "xn--nw2a.xn--j6w193g",
        "fe80--1ff-fe23-4567-890as3.ipv6-literal.net",
        "127.0.0.1",
        "255.255.255.255",
        "0.0.0.0",
        "8.8.8.8",
        "1.2.3.4",
        "192.168.0.1",
        "10.42.0.0",
        "[2001:db8:0:0:0:0:2:1]",
        "[2001:db8::2:1]",
        "[2001:db8:0000:1:1:1:1:1]",
        "[2001:db8:0:1:1:1:1:1]",
        "[0:0:0:0:0:0:0:0]",
        "[0:0:0:0:0:0:0:1]",
        "[::1]",
        "[::]",
        "[2001:db8:85a3:8d3:1319:8a2e:370:7348]",
        "[fe80::1ff:fe23:4567:890a%25eth2]",
        "[fe80::1ff:fe23:4567:890a]",
        "[fe80::1ff:fe23:4567:890a%253]",
        "[fe80:3::1ff:fe23:4567:890a]",
    ];
    for chunk in hosts.chunks(7) {
        execute(
            &chunk
                .iter()
                .map(|host| parsed(&format!("scheme://{host}"), &component("host", Some(host))))
                .collect::<String>(),
        );
    }
}
#[test]
fn zig_rfc_examples() {
    execute(&parsed(
        "foo://example.com:8042/over/there?name=ferret#nose",
        &(string_field("scheme", "foo")
            + &component("user", None)
            + &component("password", None)
            + &component("host", Some("example.com"))
            + &component("query", Some("name=ferret"))
            + &component("fragment", Some("nose"))
            + "core@1/test.check(t, parsed_uri.port == Some(8042))\ncore@1/test.check(t, parsed_uri.path == Encoded(\"/over/there\"))"),
    ));
    execute(&parsed(
        "urn:example:animal:ferret:nose",
        &(string_field("scheme", "urn")
            + &component("host", None)
            + &component("user", None)
            + &component("password", None)
            + &component("query", None)
            + &component("fragment", None)
            + "core@1/test.check(t, parsed_uri.port == None())\ncore@1/test.check(t, parsed_uri.path == Encoded(\"example:animal:ferret:nose\"))"),
    ));
}
#[test]
fn zig_wikipedia_rfc3986_and_special_parse_examples() {
    let texts = [
        "https://john.doe@www.example.com:123/forum/questions/?tag=networking&order=newest#top",
        "ldap://[2001:db8::7]/c=GB?objectClass?one",
        "mailto:John.Doe@example.com",
        "news:comp.infosystems.www.servers.unix",
        "tel:+1-816-555-1212",
        "telnet://192.0.2.16:80/",
        "urn:oasis:names:specification:docbook:dtd:xml:4.1.2",
        "http://a/b/c/d;p?q",
        "http://a/b/c/g",
        "http://a/b/c/g/",
        "http://a/g",
        "http://g",
        "http://a/b/c/d;p?y",
        "http://a/b/c/g?y",
        "http://a/b/c/d;p?q#s",
        "http://a/b/c/g#s",
        "http://a/b/c/g?y#s",
        "http://a/b/c/;x",
        "http://a/b/c/g;x",
        "http://a/b/c/g;x?y#s",
        "http://a/b/c/",
        "http://a/b/",
        "http://a/b/g",
        "http://a/",
        "https://www.youtube.com/watch?v=dQw4w9WgXcQ&feature=youtu.be&t=0",
    ];
    for chunk in texts.chunks(10) {
        execute(
            &chunk
                .iter()
                .map(|text| parsed(text, "()"))
                .collect::<String>(),
        );
    }
}
#[test]
fn zig_percent_encoding_decoding_and_query_encoding() {
    let raw = "\\ö/ äöß ~~.adas-https://canvas:123/#ads&&sad";
    let encoded =
        "%5C%C3%B6%2F%20%C3%A4%C3%B6%C3%9F%20~~.adas-https%3A%2F%2Fcanvas%3A123%2F%23ads%26%26sad";
    execute(&format!(
        "core@1/test.check(t, std@1/uri.percent_encode(core@1/string.as_bytes({raw:?})) == {encoded:?})\ncore@1/test.check(t, std@1/uri.percent_decode({encoded:?}) == core@1/string.as_bytes({raw:?}))"
    ));
    for raw in ["/abc%", "%", "%0", "%GG", "%0G%41", "%ff%00+", "%25%32%35"] {
        let expected = match raw {
            "%0G%41" => "%0GA".as_bytes(),
            "%ff%00+" => &[255, 0, 43],
            "%25%32%35" => "%25".as_bytes(),
            _ => raw.as_bytes(),
        };
        execute(&format!(
            "core@1/test.check(t, std@1/uri.percent_decode({raw:?}) == core@1/bytes.from_list({expected:?}))"
        ));
    }
    execute(&parsed(
        "https://objects.githubusercontent.com/?response-content-type=application%2Foctet-stream",
        "core@1/test.check(t, std@1/uri.component_encoded(parsed_uri.path, Path()) == \"/\")\ncore@1/test.check(t, parsed_uri.query == Some(Encoded(\"response-content-type=application%2Foctet-stream\")))",
    ));
}
#[test]
fn zig_format_and_component_specific_escape_sets() {
    execute(
        "let parsed_uri = std@1/uri.Uri(scheme: \"file\", user: None(), password: None(), host: None(), port: None(), path: Raw(\"/foo/bar/baz\"), query: None(), fragment: None())\ncore@1/test.check(t, std@1/uri.format(parsed_uri) == \"file:/foo/bar/baz\")\ncore@1/test.check(t, std@1/uri.component_encoded(Raw(\"a/b:@? #\"), Path()) == \"a/b:@%3F%20%23\")\ncore@1/test.check(t, std@1/uri.component_encoded(Raw(\"a/b:@? #\"), Query()) == \"a/b:@?%20%23\")\ncore@1/test.check(t, std@1/uri.component_encoded(Raw(\"a:b@c\"), User()) == \"a%3Ab%40c\")",
    );
    execute(
        "core@1/test.check(t, std@1/uri.component_encoded(Encoded(\"a%2Fb\"), Path()) == \"a%2Fb\")",
    );
    execute(&parsed(
        "http://user:pass@example.com:80?x#y",
        "core@1/test.check(t, std@1/uri.format(parsed_uri) == \"http://user:pass@example.com:80/?x#y\")",
    ));
}
#[test]
fn zig_malformed_input_and_numeric_host_guard() {
    bad(
        &[
            "http://][",
            "http://]@[",
            "http://lo]s\u{85}hc@[/8\u{10}?0Q",
        ],
        "InvalidFormat",
    );
    bad(
        &[
            "http://0x7f.0.0.1",
            "http://example.123",
            "http://127.1",
            "http://1.2.3.4.5",
            "http://%30x7f.0.0.1",
            "http://example.%31%32%33",
        ],
        "InvalidHostName",
    );
    bad(&["http://host:65536", "http://host:"], "InvalidPort");
}
#[test]
fn zig_remove_dot_segments_and_rfc3986_reference_resolution() {
    execute(
        "core@1/test.check(t, std@1/uri.remove_dot_segments(\"/a/b/c/./../../g\") == \"/a/g\")",
    );
    let vectors = [
        ("g:h", "g:h"),
        ("g", "http://a/b/c/g"),
        ("./g", "http://a/b/c/g"),
        ("g/", "http://a/b/c/g/"),
        ("/g", "http://a/g"),
        ("//g", "http://g/"),
        ("?y", "http://a/b/c/d;p?y"),
        ("g?y", "http://a/b/c/g?y"),
        ("#s", "http://a/b/c/d;p?q#s"),
        ("g#s", "http://a/b/c/g#s"),
        ("g?y#s", "http://a/b/c/g?y#s"),
        (";x", "http://a/b/c/;x"),
        ("g;x", "http://a/b/c/g;x"),
        ("g;x?y#s", "http://a/b/c/g;x?y#s"),
        ("", "http://a/b/c/d;p?q"),
        (".", "http://a/b/c/"),
        ("./", "http://a/b/c/"),
        ("..", "http://a/b/"),
        ("../", "http://a/b/"),
        ("../g", "http://a/b/g"),
        ("../..", "http://a/"),
        ("../../", "http://a/"),
        ("../../g", "http://a/g"),
        ("../../../g", "http://a/g"),
        ("../../../../g", "http://a/g"),
        ("/./g", "http://a/g"),
        ("/../g", "http://a/g"),
        ("g.", "http://a/b/c/g."),
        (".g", "http://a/b/c/.g"),
        ("g..", "http://a/b/c/g.."),
        ("..g", "http://a/b/c/..g"),
        ("./../g", "http://a/b/g"),
        ("./g/.", "http://a/b/c/g/"),
        ("g/./h", "http://a/b/c/g/h"),
        ("g/../h", "http://a/b/c/h"),
        ("g;x=1/./y", "http://a/b/c/g;x=1/y"),
        ("g;x=1/../y", "http://a/b/c/y"),
        ("g?y/./x", "http://a/b/c/g?y/./x"),
        ("g?y/../x", "http://a/b/c/g?y/../x"),
        ("g#s/./x", "http://a/b/c/g#s/./x"),
        ("g#s/../x", "http://a/b/c/g#s/../x"),
        ("http:g", "http:g"),
    ];
    for chunk in vectors.chunks(6) {
        let mut body = String::new();
        body += "let base = match std@1/uri.parse(\"http://a/b/c/d;p?q\") { Ok(parsed_uri) => take parsed_uri, Error(_) => {core@1/test.check(t, false); return Error(())} }\n";
        for &(reference, expected) in chunk {
            body += &format!(
                "match std@1/uri.resolve(base, {reference:?}) {{ Ok(parsed_uri) => core@1/test.check(t, std@1/uri.format(parsed_uri) == {expected:?}), Error(_) => core@1/test.check(t, false) }}\n"
            );
        }
        execute(&body);
    }
}

#[test]
fn absolute_reference_numeric_refusals_do_not_fall_back_to_paths() {
    execute(
        "let base = match std@1/uri.parse(\"http://example.com/a\") { Ok(parsed_uri) => take parsed_uri, Error(_) => {core@1/test.check(t, false); return Error(())} }\nmatch std@1/uri.resolve(base, \"http://127.1/a\") { Error(InvalidHostName()) => (), _ => core@1/test.check(t, false) }\nmatch std@1/uri.resolve(base, \"http://127.0.0.1/a\") { Ok(parsed_uri) => core@1/test.check(t, std@1/uri.format(parsed_uri) == \"http://127.0.0.1/a\"), Error(_) => core@1/test.check(t, false) }",
    );
}

#[test]
fn zig_uri_decimal_ports_and_signed_hex_pairs_follow_fmt_parse_int() {
    for (text, port) in [
        ("+10", 10),
        ("-0", 0),
        ("1__0", 10),
        ("00080", 80),
        ("65535", 65535),
    ] {
        execute(&parsed(
            &format!("http://example.com:{text}/"),
            &format!("core@1/test.check(t, parsed_uri.port == Some({port}))"),
        ));
    }
    bad(
        &[
            "http://example.com:_10/",
            "http://example.com:10_/",
            "http://example.com:-1/",
            "http://example.com:+/",
            "http://example.com:65536/",
        ],
        "InvalidPort",
    );
    execute(
        "core@1/test.check(t, std@1/uri.percent_decode(\"%+A%-0%+f\") == b\"\\x0a\\x00\\x0f\")",
    );
}

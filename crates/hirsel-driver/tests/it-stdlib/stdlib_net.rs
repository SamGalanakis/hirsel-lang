//! Vectors ported before implementation from Rust coretests/tests/net/{parser,
//! ip_addr,socket_addr}.rs (f6c1107f), and Zig Io/net/HostName.zig (bff490a2).
use crate::source_std::tir;
use hirsel_driver::*;
use hirsel_engine::*;

pub(super) fn execute(body: &str) {
    execute_source(&format!(
        "pub fn main(t: Test) -> Result[(), ()] {{\n{body}\nOk(())\n}}"
    ));
}

pub(super) fn execute_source(source: &str) {
    with_compiler_stack(|| {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/std_source/main.hir");
        let prepared = tir(&path, Some(source), None).unwrap_or_else(|e| panic!("{e}\n{source}"));
        let config = RunConfiguration {
            entry: prepared.entry.clone(),
            hash_seed: 0,
            inputs: vec![],
        };
        let run = testing::observe_tir(
            &prepared,
            &config,
            SimulationConfig::default(),
            InterpreterAdapter::default().limits,
            false,
        )
        .unwrap();
        assert!(
            matches!(
                run.outcome,
                Ok(RunOutcome::Returned(hirsel_run_schema::Returned::Ok))
            ),
            "{:?}\n{source}",
            run.outcome
        );
        let checked = testing::observe_tir(
            &prepared,
            &config,
            SimulationConfig::default(),
            InterpreterAdapter::default().limits,
            true,
        )
        .unwrap();
        assert!(
            matches!(
                checked.outcome,
                Ok(RunOutcome::Returned(hirsel_run_schema::Returned::Ok))
            ),
            "{:?}\n{source}",
            checked.outcome
        );
    });
}

fn ip_vectors(texts: &[&str], operation: &str) {
    for chunk in texts.chunks(12) {
        let mut body = String::new();
        for &text in chunk {
            let value = match operation {
                "parse_ipv4" => text
                    .parse::<std::net::Ipv4Addr>()
                    .map(std::net::IpAddr::V4)
                    .ok(),
                "parse_ipv6" => text
                    .parse::<std::net::Ipv6Addr>()
                    .map(std::net::IpAddr::V6)
                    .ok(),
                _ => text.parse::<std::net::IpAddr>().ok(),
            };
            let check = match value {
                Some(ip) => {
                    let (variant, octets) = match ip {
                        std::net::IpAddr::V4(ip) => ("V4", ip.octets().to_vec()),
                        std::net::IpAddr::V6(ip) => ("V6", ip.octets().to_vec()),
                    };
                    let octets = format!("{octets:?}");
                    let expected = if operation == "parse_ip" {
                        format!("{variant}({octets})")
                    } else {
                        octets
                    };
                    format!(
                        "Ok(value) => core@1/test.check(t, value == {expected}), Error(_) => core@1/test.check(t, false)"
                    )
                }
                None => "Error(_) => (), Ok(_) => core@1/test.check(t, false)".into(),
            };
            body += &format!("match std@1/net.{operation}({text:?}) {{ {check} }}\n");
        }
        execute(&body);
    }
}

const V4: &[&str] = &[
    "192.168.0.1",
    "192.168.0.1:8080",
    "0127.0.0.1",
    "0x10.0.0.1",
    "127.0.0.1",
    "255.255.255.255",
    "0.0.0.0",
    "256.0.0.1",
    "255.0.0",
    "255.0.0.1.2",
    "255.0..1",
    "255.0.0.01",
    "255.0.0.00",
    "255.0.00.0",
];
const V6: &[&str] = &[
    "2001:db8:0:0:0:0:c0a8:1",
    "2001:db8::c0a8:1",
    "2001:db8::192.168.0.1",
    "2001:db8::0127.0.0.1",
    "2001:db8::0x10.0.0.1",
    "[2001:db8::c0a8:1]:8080",
    "1::",
    "1:1::",
    "::1",
    "::1:1",
    "::",
    "::192.168.0.1",
    "::1:192.168.0.1",
    "1:1:1:1:1:1:192.168.0.1",
    "1:192.168.0.1::",
    "1:1:1:1::1:1:1:1",
    "1:1:1:1:1:1:1:1:8080",
    "0:0:0:0:0:0:0:0",
    "0:0:0:0:0:0:0:1",
    "2a02:6b8::11:11",
    "::00000",
    "1:2:3:4:5:6:7",
    "1:2:3:4:5:6:7:8:9",
    "1:2:::6:7:8",
    "1:2::6::8",
    "1:2:3:4::5:6:7:8",
    "::192.0.2.33",
    "::FFFF:192.0.2.33",
    "64:ff9b::192.0.2.33",
    "2001:db8:122:c000:2:2100:192.0.2.33",
    "::127.0.0.1:",
    "1:2:3:4:5:127.0.0.1",
    "1:2:3:4:5:6:7:127.0.0.1",
];

#[test]
fn rust_parse_ipv4() {
    ip_vectors(&[V4, V6].concat(), "parse_ipv4");
}
#[test]
fn rust_parse_ipv6() {
    ip_vectors(&[V4, V6].concat(), "parse_ipv6");
}
#[test]
fn rust_parse_ip_and_corner_cases() {
    ip_vectors(&[V4, V6].concat(), "parse_ip");
}

#[test]
fn rust_socket_parser() {
    let vectors = [
        "192.168.0.1:8080",
        "[2001:db8::c0a8:1]:8080",
        "77.88.21.11:80",
        "[2a02:6b8:0:1::1]:53",
        "[::127.0.0.1]:22",
        "127.0.0.1",
        "127.0.0.1:",
        "[127.0.0.1]:22",
        "127.0.0.1:123456",
        "2001:db8::c0a8:1",
        "1:1:1:1:1:1:1:1:8080",
        "0.0.0.0:0",
        "[::]:0",
        "[1111:2222:3333:4444:5555:6666:7777:8888]:65535",
        "1.1.1.1:53",
        "[1:2:3:4:5:6:7:8]:9",
        "255.255.255.255:65535",
        "127.0.0.1:00000000000000000000000000080",
        "[::1]:00065535",
        "[::1]:65536",
        "[::1%1337]:80",
        "[::1]:+80",
        "[::1]:-1",
        "[::1]:",
    ];
    for chunk in vectors.chunks(10) {
        let mut body = String::new();
        for text in chunk {
            let expected = if text.contains('%') {
                None
            } else {
                text.parse::<std::net::SocketAddr>().ok()
            };
            let check = match expected {
                Some(addr) => format!(
                    "Ok(addr) => core@1/test.check(t, std@1/net.socket_addr_to_string(addr) == {:?}), Error(_) => core@1/test.check(t, false)",
                    addr.to_string()
                ),
                None => "Error(_) => (), Ok(_) => core@1/test.check(t, false)".into(),
            };
            body += &format!("match std@1/net.parse_socket_addr({text:?}) {{ {check} }}\n");
        }
        execute(&body);
    }
}

#[test]
fn rust_display_rfc5952_first_longest_run_and_mapped_ipv4() {
    let vectors = [
        "127.0.0.1",
        "1.1.1.1",
        "127.127.127.127",
        "::ffff:192.0.2.128",
        "::c000:280",
        "8:9:a:b:c:d:e:f",
        "1111:2222:3333:4444:5555:6666:7777:8888",
        "1:2:3:4:5:6:7:8",
        "ae:0:0:0:0:ffff:102:304",
        "1:2:3:4:5:6:0:8",
        "::",
        "::1",
        "1::",
        "1:0:0:4:0:0:0:8",
        "1:0:0:4:5:0:0:8",
    ];
    for chunk in vectors.chunks(8) {
        let mut body = String::new();
        for text in chunk {
            let ip: std::net::IpAddr = text.parse().unwrap();
            body += &format!(
                "match std@1/net.parse_ip({text:?}) {{ Ok(ip) => {{ core@1/test.check(t, std@1/net.ip_to_string(ip) == {:?}); core@1/test.check(t, std@1/net.socket_addr_to_string(SocketAddr(ip: ip, port: 8080)) == {:?}) }}, Error(_) => core@1/test.check(t, false) }}\n",
                ip.to_string(),
                std::net::SocketAddr::new(ip, 8080).to_string()
            );
        }
        execute(&body);
    }
}

#[test]
fn zig_hostname_validate_and_numeric_host_deviations() {
    let valid = [
        "example",
        "example.com",
        "www.example.com",
        "sub.domain.example.com",
        "example.com.",
        "host-name.example.com.",
        "123.example.com.",
        "a-b.com",
        "a.b.c.d.e.f.g",
        "127.0.0.1",
        "EXAMPLE.com",
        &format!("{}.com", "a".repeat(63)),
        &format!("{}a", "a.".repeat(126)),
        &format!("{}a.", "a.".repeat(126)),
    ];
    let invalid = [
        "",
        ".example.com",
        "example.com..",
        "host..domain",
        "-hostname",
        "hostname-",
        "hostname-.com",
        "a.-.b",
        "host_name.com",
        ".",
        "..",
        &format!("{}a.com", "a".repeat(63)),
        &format!("{}ab", "a.".repeat(126)),
        &format!("{}ab.", "a.".repeat(126)),
        "0x7f.0.0.1",
        "example.123",
        "127.1",
        "1.2.3.4.5",
        "example.0x",
        "example.0XFF",
        "127.0.0.1.",
    ];
    for (vectors, accepts) in [(valid.as_slice(), true), (invalid.as_slice(), false)] {
        for chunk in vectors.chunks(8) {
            let mut body = String::new();
            for text in chunk {
                if accepts {
                    body += &format!(
                        "match std@1/net.validate_host({text:?}) {{ Ok(_) => (), Error(_) => core@1/test.check(t, false) }}\n"
                    );
                } else {
                    let expected = if text.strip_suffix('.').unwrap_or(text).len() > 253 {
                        "NameTooLong"
                    } else {
                        "InvalidHostName"
                    };
                    body += &format!(
                        "match std@1/net.validate_host({text:?}) {{ Error({expected}()) => (), _ => core@1/test.check(t, false) }}\n"
                    );
                }
            }
            execute(&body);
        }
    }
}

#[test]
fn rust_rejects_malformed_address_text_without_panics() {
    let vectors = [
        "",
        " ",
        "1.2.3.4\0",
        "+1.2.3.4",
        "1.2.3.4.",
        ":::",
        ":1",
        "1:",
        "::ffff:256.1.1.1",
        "fe80::1%1",
        "[::1]",
        "é",
        "１２７.0.0.1",
    ];
    ip_vectors(&vectors, "parse_ip");
}

/// Rust coretests/tests/net/ip_addr.rs:123-183 and socket_addr.rs:4-72,165-192,
/// at f6c1107f. Formatting all literal rows exceeds the corpus's 1M steps.
#[test]
fn rust_display_vectors_with_padding() {
    with_compiler_stack(|| {
        let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/net-rust-display.hir");
        let source = std::fs::read_to_string(fixture).unwrap();
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/std_source/main.hir");
        let mut program = load_program_source(&path, Some(&source)).unwrap();
        program.check_for_execution().unwrap();
        program.engine.adapters.interpreter = Some(Box::new(InterpreterAdapter {
            limits: hirsel_interp::Limits {
                steps: Some(10_000_000),
                call_depth: 256,
                source: None,
            },
            ..Default::default()
        }));
        let config = RunConfiguration {
            entry: program.entry.clone(),
            hash_seed: 0,
            inputs: vec![],
        };
        let snapshot =
            run_observed_snapshot(&mut program.engine, &config, SimulationConfig::default())
                .unwrap();
        assert!(
            matches!(
                snapshot.outcome,
                Ok(RunOutcome::Returned(hirsel_run_schema::Returned::Ok))
            ),
            "{:?}",
            snapshot.outcome
        );
        let checked =
            run_observed_checked(&mut program.engine, &config, SimulationConfig::default())
                .unwrap();
        assert!(
            matches!(
                checked.outcome,
                Ok(RunOutcome::Returned(hirsel_run_schema::Returned::Ok))
            ),
            "{:?}",
            checked.outcome
        );
    });
}

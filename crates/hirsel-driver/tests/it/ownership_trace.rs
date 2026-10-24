//! The admitted source program runs independently of its resource diagnostics.
use hirsel_driver::ownership_trace::run_resources_on_host;
use hirsel_driver::{HostLaunch, LaunchConfig};
use hirsel_engine::Verify;

#[test]
fn ownership_host_reference_and_checked_h_agree_on_real_source_lifetimes() {
    let dir = tempfile::tempdir().unwrap_or_else(|error| panic!("source fixture: {error}"));
    let source = dir.path().join("main.hir");
    std::fs::write(&source, "resource type Token {}\ndrop(x) {}\n\nfn identity(take token: Token) -> Token { take token }\npub fn main() -> Result[(), ()] {\n let first = Token()\n let second = identity(take first)\n let third = take second\n Ok(())\n}\n").unwrap_or_else(|error| panic!("source fixture: {error}"));
    let run = |checked| {
        run_resources_on_host(
            &source,
            HostLaunch::Live {
                config: LaunchConfig::Grants {
                    input: std::ffi::OsStr::new("{}"),
                    arguments: None,
                },
                record: None,
                revocations: None,
            },
            Some(100_000),
            Verify::On,
            checked,
            100,
        )
        .unwrap()
    };
    let reference = run(false);
    let checked = run(true);
    assert_eq!(reference.outcome, checked.outcome);
    assert_eq!(reference.program, checked.program);
    let trace = reference
        .trace
        .unwrap_or_else(|error| panic!("source fixture: {error}"));
    assert_eq!(trace, checked.trace.unwrap());
    let analysis = trace
        .analyze(reference.program, 100)
        .unwrap_or_else(|error| panic!("source fixture: {error}"));
    assert_eq!(
        (analysis.created, analysis.dropped, analysis.live.len()),
        (1, 1, 0)
    );
    assert!(
        trace
            .events
            .iter()
            .all(|e| e.site.file.ends_with("main.hir"))
    );
    assert!(trace.events.iter().any(|e| matches!(
        e.action,
        hirsel_run_schema::ownership_trace::Action::Move { .. }
    )));
}

#[test]
fn ownership_host_trace_overflow_does_not_change_the_source_outcome() {
    let dir = tempfile::tempdir().unwrap_or_else(|error| panic!("source fixture: {error}"));
    let source = dir.path().join("main.hir");
    std::fs::write(&source,"resource type Token {}\ndrop(x) {}\npub fn main() -> Result[(), ()] { let token = Token()\n Ok(()) }\n").unwrap_or_else(|error| panic!("source fixture: {error}"));
    let launch = || HostLaunch::Live {
        config: LaunchConfig::Grants {
            input: std::ffi::OsStr::new("{}"),
            arguments: None,
        },
        record: None,
        revocations: None,
    };
    let ordinary = hirsel_driver::run_path_on_host(&source, launch(), Some(100_000), Verify::On)
        .unwrap_or_else(|error| panic!("source fixture: {error}"));
    let traced = run_resources_on_host(&source, launch(), Some(100_000), Verify::On, false, 0)
        .unwrap_or_else(|error| panic!("source fixture: {error}"));
    assert_eq!(ordinary, traced.outcome);
    assert!(traced.trace.is_err());
}

#[test]
fn ownership_host_plain_builtin_calls_preserve_live_resource_custody_in_both_engines() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("main.hir");
    std::fs::write(&source, "resource type Token {}\ndrop(x) {}\npub fn main() -> Result[(), ()] {\n let token = Token()\n let n = string.byte_len(\"abc\")\n if n == 3u64 { Ok(()) } else { Error(()) }\n}\n").unwrap();
    let mut traces = vec![];
    for checked in [false, true] {
        let launch = || HostLaunch::Live {
            config: LaunchConfig::Grants {
                input: std::ffi::OsStr::new("{}"),
                arguments: None,
            },
            record: None,
            revocations: None,
        };
        let ordinary =
            hirsel_driver::run_path_on_host(&source, launch(), Some(100_000), Verify::On)
                .unwrap_or_else(|error| panic!("source fixture: {error}"));
        let traced =
            run_resources_on_host(&source, launch(), Some(100_000), Verify::On, checked, 100)
                .unwrap_or_else(|error| panic!("source fixture: {error}"));
        assert_eq!(traced.outcome, ordinary);
        let trace = traced.trace.unwrap();
        let analysis = trace.analyze(traced.program, 100).unwrap();
        assert_eq!(
            (analysis.created, analysis.dropped, analysis.live.len()),
            (1, 1, 0)
        );
        traces.push(trace);
    }
    assert_eq!(traces[0], traces[1]);
}

//! Named arena callbacks use the source-selected target and lexical boundary.
use hirsel_driver::{HostLaunch, LaunchConfig, ownership_trace::run_resources_on_host};
use hirsel_engine::Verify;

#[test]
fn named_arena_callback_preserves_outer_resource_on_reference_and_checked_h() {
    let directory = tempfile::tempdir().unwrap();
    let source = directory.path().join("main.hir");
    let manifest = directory.path().join("hirsel.json");
    std::fs::write(&source, "resource type Token {value:u64}\ndrop(x){}\nfn body(a:Arena)->Token{Token(value:7u64)}\npub fn main()->Result[(),u64]{\nlet first=Token(value:7u64)\nlet n=arena.with(body)\nlet second=take first\nOk(())\n}\n").unwrap();
    std::fs::write(&manifest, r#"{"schema":1,"package":{"name":"ownership","version":"1"},"sources":[{"path":"main.hir","module":[]}],"dependencies":[],"entry":{"module":[],"name":"main"}}"#).unwrap();
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
            hirsel_driver::run_path_on_host(&manifest, launch(), Some(100_000), Verify::On)
                .unwrap();
        let observed = run_resources_on_host(
            &manifest,
            launch(),
            Some(100_000),
            Verify::On,
            checked,
            1000,
        )
        .unwrap();
        assert_eq!(observed.outcome, ordinary);
        assert_eq!(observed.outcome.exit_code(), 0);
        let trace = observed.trace.unwrap();
        let analysis = trace.analyze(observed.program, 1000).unwrap();
        assert_eq!(
            (analysis.created, analysis.dropped, analysis.live.len()),
            (2, 2, 0)
        );
        traces.push(trace);
    }
    assert_eq!(traces[0], traces[1]);
}

//! The affine sealed owner has lineage independent of its pointer-free root.
use hirsel_driver::{HostLaunch, LaunchConfig, ownership_trace::run_resources_on_host};
use hirsel_run_schema::ownership_trace::Action;

#[test]
fn sealed_owner_moves_and_repeated_root_loans_keep_one_lifetime_on_reference_and_checked_h() {
    let directory = tempfile::tempdir().unwrap();
    std::fs::write(
        directory.path().join("main.hir"),
        r#"
pub fn main()->Result[(),u64] {
 let first=arena.with(fn(a:Arena)->Region[u64]{let root=7u64
arena.seal(take a,take root)})
 let mut second=take first
 let a=region.with(mut second,fn(root:u64)->u64{root})
 let b=region.with(mut second,fn(root:u64)->u64{root})
 failure.check(a+b==14u64,Bounds())
 Ok(())
}
"#,
    )
    .unwrap();
    let path = directory.path().join("hirsel.json");
    std::fs::write(&path,r#"{"schema":1,"package":{"name":"ownership","version":"1"},"sources":[{"path":"main.hir","module":[]}],"dependencies":[],"entry":{"module":[],"name":"main"}}"#).unwrap();
    let accepted = hirsel_driver::check_path(&path).unwrap();
    assert_eq!(
        accepted.completion,
        hirsel_engine::CheckCompletion::Complete,
        "{accepted:?}"
    );
    let mut traces = Vec::new();
    for checked in [false, true] {
        let result = run_resources_on_host(
            &path,
            HostLaunch::Live {
                config: LaunchConfig::Grants {
                    input: std::ffi::OsStr::new("{}"),
                    arguments: None,
                },
                record: None,
                revocations: None,
            },
            Some(100_000),
            hirsel_engine::Verify::On,
            checked,
            1000,
        )
        .unwrap();
        assert_eq!(result.outcome.exit_code(), 0);
        let trace = result.trace.unwrap();
        let analysis = trace.analyze(result.program, 1000).unwrap();
        assert_eq!(
            (analysis.created, analysis.dropped, analysis.live.len()),
            (1, 1, 0)
        );
        assert!(trace.events.iter().any(|event| matches!(&event.action,Action::Create{value_type,..} if value_type=="core@1::Region")));
        assert!(
            trace
                .events
                .iter()
                .filter(|event| matches!(event.action, Action::Move { .. }))
                .count()
                >= 2
        );
        traces.push(trace);
    }
    assert_eq!(traces[0], traces[1]);
}

#[test]
#[ignore = "requires diagnostic runtime: -c hirsel.ownership_trace=true -- --test-arg=--include-ignored"]
fn native_sealed_scalar_owner_has_one_lifetime_on_four_engines() {
    super::ownership_owned_call_49::source_calls(
        r#"
pub fn main()->Result[(),u64] {
 let first=arena.with(fn(a:Arena)->Region[u64]{let root=7u64
arena.seal(take a,take root)})
 let mut second=take first
 let a=region.with(mut second,fn(root:u64)->u64{root})
 let b=region.with(mut second,fn(root:u64)->u64{root})
 failure.check(a+b==14u64,Bounds())
 Ok(())
}
"#,
        1,
    );
}

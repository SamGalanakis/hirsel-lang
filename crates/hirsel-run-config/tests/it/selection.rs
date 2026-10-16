use hirsel_run_config::*;
use std::ffi::OsString;

fn revision() -> ProgramRevision {
    ProgramRevision::encode(
        "selection@1/main",
        [ProgramNode {
            identity: "selection@1/main".into(),
            content: vec![b"pub fn main(rt: Io, args: Args) -> Result[(), ()] { Ok(()) }".to_vec()],
            dependencies: vec![],
        }],
    )
    .unwrap()
}
fn select(revision: &ProgramRevision, args: &[OsString]) -> Result<ValidatedRun, RunRefusal> {
    select_run_record(
        revision.digest(),
        revision.entry(),
        LaunchKind::Main,
        args.iter().cloned(),
    )
}
#[test]
fn no_record_denies_roots_and_an_explicit_empty_record_authorizes_only_its_snapshots() {
    let revision = revision();
    let denied = select(&revision, &[]).unwrap();
    assert!(matches!(
        denied.require_roots(&[Root::Io, Root::Args]),
        Err(RunRefusal::NotGranted { .. })
    ));
    let path =
        std::env::temp_dir().join(format!("hirsel-run-selection-{}.json", std::process::id()));
    let mut record = RunConfig::for_program(&revision, LaunchKind::Main);
    record.io = Some(IoGrants::default());
    record.args = Some(snapshot_args([]).unwrap());
    record.env = Some(snapshot_env([(OsString::from("UNSET"), None)]).unwrap());
    std::fs::write(&path, record.validated().unwrap().canonical()).unwrap();
    let arguments = [
        OsString::from("--hirsel-run"),
        path.clone().into_os_string(),
    ];
    let accepted = select(&revision, &arguments).unwrap();
    accepted
        .require_roots(&[Root::Io, Root::Args, Root::Env])
        .unwrap();
    assert_eq!(accepted.record().args, Some(vec![]));
    assert_eq!(
        accepted.record().env.as_ref().unwrap().get("UNSET"),
        Some(&None)
    );
    assert_eq!(accepted.record().env.as_ref().unwrap().get("DENIED"), None);
    assert!(accepted.io_projections().is_empty());
    for invalid in [
        vec![OsString::from("application-argument")],
        vec![
            OsString::from("--hirsel-run"),
            OsString::from("relative.json"),
        ],
        vec![
            arguments[0].clone(),
            arguments[1].clone(),
            arguments[0].clone(),
            arguments[1].clone(),
        ],
    ] {
        assert!(select(&revision, &invalid).is_err());
    }
    let changed = ProgramRevision::encode(
        revision.entry(),
        [ProgramNode {
            identity: revision.entry().into(),
            content: vec![b"changed private body".to_vec()],
            dependencies: vec![],
        }],
    )
    .unwrap();
    assert!(
        matches!(select(&changed, &arguments), Err(RunRefusal::Binding { field }) if field == "program")
    );
    std::fs::write(&path, b"\xff").unwrap();
    assert!(matches!(
        select(&revision, &arguments),
        Err(RunRefusal::Malformed { .. })
    ));
    std::fs::write(&path, record.validated().unwrap().canonical()).unwrap();
    assert!(select(&revision, &arguments).is_ok());
    std::fs::remove_file(path).unwrap();
}

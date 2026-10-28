use hirsel_engine::{CompileConfig, QueryKind, store, with_compiler_stack};

const SOURCE: &str = r#"@loop_source(open, next)
type View { source: String }
type Cursor { source: String }
fn open(v: View) -> Cursor in v { Cursor(source: copy v.source) }
fn next(mut c: Cursor) -> Option[u64] { None }
fn probe(v: View) { for n in v {} }
"#;

#[test]
fn a_broken_dependency_association_is_reported_before_client_body_planning() {
    use std::path::PathBuf;
    struct Fixture(PathBuf);
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    let scratch = tempfile::Builder::new()
        .prefix("hirsel-protocol-preflight-")
        .tempdir()
        .unwrap();
    let dir = Fixture(scratch.path().to_path_buf());
    std::fs::create_dir_all(&dir.0).unwrap();
    std::fs::write(
        dir.0.join("client.hir"),
        "fn probe(v: vendor.View) { for n in v {} }\npub fn main() {}\n",
    )
    .unwrap();
    std::fs::write(dir.0.join("client.json"), r#"{"schema":1,"package":{"name":"a_client","version":"1"},"entry":{"module":[],"name":"main"},"sources":[{"path":"client.hir","module":[]}],"dependencies":[{"manifest":"dependency.json","package":{"name":"z_protocol","version":"1"},"expose":[{"head":"vendor","module":[]}]}]}"#).unwrap();
    std::fs::write(dir.0.join("dependency.json"), r#"{"schema":1,"package":{"name":"z_protocol","version":"1"},"sources":[{"path":"api.hir","module":[]}],"dependencies":[]}"#).unwrap();
    let api = SOURCE
        .replace("type View", "pub type View")
        .replace("type Cursor", "pub type Cursor");
    with_compiler_stack(|| {
        for (text, valid) in [
            (&api, true),
            (&api.replace("open(v: View)", "open(take v: View)"), false),
        ] {
            std::fs::write(dir.0.join("api.hir"), text).unwrap();
            let mut program = hirsel_driver::load_program(&dir.0.join("client.json")).unwrap();
            let association_file = program
                .files
                .iter()
                .find(|(_, c)| c.package.name == "z_protocol")
                .unwrap()
                .0;
            let output = program
                .check(hirsel_engine::LoweringPolicy::Reachable)
                .unwrap();
            if valid {
                assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
            } else {
                assert_eq!(output.diagnostics[0].rule.as_str(), "SEM-4");
                let at = output.sources.resolve(output.diagnostics[0].primary);
                assert_eq!(at.file, association_file);
                assert_eq!(at.start, 0);
            }
        }
    });
}

#[test]
fn association_preflight_reuses_unchanged_headers_and_rechecks_an_edited_target() {
    with_compiler_stack(|| {
        let directory = tempfile::tempdir().unwrap();
        let manifest = directory.path().join("hirsel.json");
        let file = directory.path().join("main.hir");
        let source = "@format(render)\ntype View { value:u64 }\nfn render(v:View, mut out:List[u8]) {}\nfn neighbor()->u64 { 1u64 }\npub fn main() {}\n";
        std::fs::write(&file, source).unwrap();
        std::fs::write(&manifest, r#"{"schema":1,"package":{"name":"association","version":"1"},"sources":[{"path":"main.hir","module":[]}],"dependencies":[],"entry":{"module":[],"name":"main"}}"#).unwrap();
        let mut session = hirsel_driver::session::Session::open_configured(
            &manifest,
            CompileConfig {
                store: std::sync::Arc::new(store::MemStore::default()),
                ..Default::default()
            },
        );
        assert!(session.check().unwrap().diagnostics.is_empty());
        let changed = source.replace("1u64", "2u64");
        std::fs::write(&file, &changed).unwrap();
        session.apply(vec![hirsel_driver::session::FileChange {
            path: file.clone(),
            bytes: Some(changed.as_bytes().to_vec()),
        }]);
        assert!(session.check_applied().unwrap().diagnostics.is_empty());
        assert!(
            session.touched_memos().iter().all(|key| {
                key.kind != QueryKind::Signature
                    || !matches!(key.def.path().0.name.as_str(), "View" | "render")
            }),
            "{:?}",
            session.touched_memos()
        );

        let invalid = changed.replace("render(v:View", "render(take v:View");
        std::fs::write(&file, &invalid).unwrap();
        session.apply(vec![hirsel_driver::session::FileChange {
            path: file,
            bytes: Some(invalid.into_bytes()),
        }]);
        let output = session.check_applied().unwrap();
        assert!(
            output
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.rule.as_str() == "SEM-3"),
            "{:?}",
            output.diagnostics
        );
    });
}

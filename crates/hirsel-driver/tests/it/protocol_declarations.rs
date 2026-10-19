use hirsel_engine::with_compiler_stack;

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
    let dir = Fixture(
        std::env::temp_dir().join(format!("hirsel-protocol-preflight-{}", std::process::id())),
    );
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
                assert_eq!(output.diagnostics[0].primary.file, association_file);
                assert_eq!(output.diagnostics[0].primary.start, 0);
            }
        }
    });
}

#[test]
fn parser_rejections_remain_language_diagnostics_after_package_preflight() {
    let file = std::env::temp_dir().join(format!(
        "hirsel-preflight-syntax-{}.hir",
        std::process::id()
    ));
    std::fs::write(&file, "pub fn main() { let fn = 1u64 }\n").unwrap();
    let result = hirsel_driver::check_path(&file);
    std::fs::remove_file(file).unwrap();
    let output = result.unwrap();
    assert_eq!(output.diagnostics[0].rule.as_str(), "LANG-14");
}

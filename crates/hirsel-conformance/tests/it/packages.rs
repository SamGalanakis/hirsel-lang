use hirsel_conformance::{Compiler, Driver, Status};

#[test]
fn package_source_indexing_diagnostics_remain_language_rejections() {
    let dir = std::env::temp_dir().join(format!("hirsel-package-indexing-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("main.hir");
    std::fs::write(
        path.with_extension("hir.json"),
        serde_json::json!({
            "schema":1, "package":{"name":"app","version":"1"},
            "entry":{"module":[],"name":"main"},
            "sources":[{"path":"main.hir","module":[]}], "dependencies":[]
        })
        .to_string(),
    )
    .unwrap();
    std::fs::write(&path, "pub fn fn(t: Test) -> Result[(), ()] { Ok(()) }\n").unwrap();
    let expect = hirsel_conformance::Expect::Reject {
        rule: hirsel_diag::RuleId::new("LANG-14").unwrap(),
        line: 1,
    };
    assert_eq!(Compiler.evaluate(&path, &expect).0, Status::Pass);
    std::fs::write(&path, "pub fn main(t: Test) -> Result[(), ()] { Ok(()) }\n").unwrap();
    assert_eq!(
        Compiler
            .evaluate(
                &path,
                &hirsel_conformance::Expect::Run {
                    stdout: String::new(),
                    exit: 0
                }
            )
            .0,
        Status::Pass
    );
    std::fs::remove_dir_all(dir).unwrap();
}

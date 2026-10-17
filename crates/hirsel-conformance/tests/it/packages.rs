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
        rule: const { hirsel_diag::RuleId::of("LANG-14") },
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

#[test]
fn dependency_failure_tokens_and_lines_belong_to_the_dependency_snapshot() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let path = root.join("conformance/FACT/FACT-2-external-invariant-bad.hir");
    let source = std::fs::read_to_string(&path).unwrap();
    let expect = hirsel_conformance::parse_header(
        std::path::Path::new("conformance/FACT/FACT-2-external-invariant-bad.hir"),
        &source,
        &hirsel_conformance::spec_rules(&root).unwrap(),
    )
    .unwrap()
    .expect;
    let (status, observed) = Compiler.evaluate(&path, &expect);
    assert_eq!(status, Status::Pass);
    let primary = observed.primary.as_ref().expect("dependency failure token");
    assert_eq!(primary.spelling, "lower");
    assert_eq!(primary.line, 9);
    assert!(observed.primary_external);
    let reformatted = source.replace("pub fn main", "\n\npub fn main");
    let rewritten =
        hirsel_conformance::corpus_format::rewrite(&source, &reformatted, &expect, &observed)
            .unwrap();
    assert!(rewritten.contains("// expect: fail FACT-1 at line 9\n"));
}

#[test]
fn std_head_diagnostics_name_versioned_candidates() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let cases = [
        ("short-run", None, &[][..]),
        ("qualified-run", None, &[][..]),
        (
            "majors-bad",
            Some(4),
            &["std@1/string/starts_with", "std@2/string/starts_with"][..],
        ),
        ("majors-ok", None, &[][..]),
        (
            "member-bad",
            Some(4),
            &[
                "core@1/string/unknown_member",
                "std@1/string/unknown_member",
            ][..],
        ),
        ("member-ok", None, &[][..]),
        (
            "duplicate-bad",
            Some(1),
            &["std@1/string/byte_len", "core@1/string/byte_len"][..],
        ),
        ("duplicate-ok", None, &[][..]),
        ("alias-core-bad", Some(4), &["probe@1/api/byte_len"][..]),
        ("alias-core-ok", None, &[][..]),
    ];
    for (suffix, line, candidates) in cases {
        let path = root.join(format!("conformance/LANG/LANG-4-std-{suffix}.hir"));
        let expect = match line {
            Some(line) => hirsel_conformance::Expect::Reject {
                rule: const { hirsel_diag::RuleId::of("LANG-4") },
                line,
            },
            None => hirsel_conformance::Expect::Run {
                stdout: String::new(),
                exit: 0,
            },
        };
        let (status, observation) = Compiler.evaluate(&path, &expect);
        assert_eq!(status, Status::Pass, "{suffix}: {observation:?}");
        for candidate in candidates {
            assert!(
                observation.actual.contains(candidate),
                "{suffix}: missing {candidate} in {observation:?}"
            );
        }
    }
}

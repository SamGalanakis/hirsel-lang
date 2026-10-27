use hirsel_conformance::{Compiler, Driver, Expect, Status};
use hirsel_diag::Severity;
use hirsel_engine::CheckOutput;
use std::path::{Path, PathBuf};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn checked(relative: &str) -> CheckOutput {
    hirsel_driver::check_path(&root().join(relative)).unwrap()
}

fn core_source(dir: &Path, name: &str, source: &str) -> PathBuf {
    std::fs::create_dir_all(dir).unwrap();
    let path = dir.join(format!("{name}.hir"));
    std::fs::write(&path, source).unwrap();
    std::fs::write(
        path.with_extension("hir.json"),
        serde_json::json!({
            "schema": 1,
            "package": {"name": "standalone", "version": "0"},
            "sources": [{"path": format!("{name}.hir"), "module": []}],
            "entry": {"module": [], "name": "main"},
            "dependencies": []
        })
        .to_string(),
    )
    .unwrap();
    path
}

#[test]
fn mechanical_json_repairs_recheck_and_preserve_primary_sites() {
    hirsel_engine::with_compiler_stack(|| {
        for (case, twin, rule, line, before, after) in [
            (
                "conformance/LANG/LANG-11-block-local-element-borrow-bad.hir",
                "conformance/LANG/LANG-11-block-local-element-copy-run.hir",
                "MODE-1",
                14,
                "",
                "copy ",
            ),
            (
                "conformance/OWN/OWN-2-branch-tail-place-bad.hir",
                "conformance/OWN/OWN-2-branch-tail-place-ok.hir",
                "OWN-2",
                13,
                "",
                "take ",
            ),
            (
                "conformance/OWN/OWN-4-redundant-copy-repair-ok.hir",
                "conformance/OWN/OWN-4-redundant-copy-repair-ok.hir",
                "OWN-4",
                9,
                "copy xs",
                "xs",
            ),
        ] {
            let source = std::fs::read_to_string(root().join(case)).unwrap();
            let dir =
                std::env::temp_dir().join(format!("hirsel-json-repair-{}", std::process::id()));
            // These vectors use core declarations only. Keep their original
            // bytes without repeatedly checking an unused std dependency.
            let case_path = core_source(&dir, "case", &source);
            let output = hirsel_driver::check_path(&case_path).unwrap();
            let d = output.diagnostics.first().unwrap();
            assert_eq!(d.rule.as_str(), rule, "{case}");
            assert_eq!(
                output.sources.location(d.primary).unwrap().line,
                line,
                "{case}"
            );
            if d.severity == Severity::Warning {
                let path = case_path;
                assert_eq!(Compiler.evaluate(&path, &Expect::Accept).0, Status::Pass);
                assert!(
                    matches!(
                        Compiler
                            .evaluate(
                                &path,
                                &Expect::Reject {
                                    rule: d.rule.clone(),
                                    line: line as usize,
                                }
                            )
                            .0,
                        Status::Fail(_)
                    ),
                    "a warning cannot satisfy a rejection"
                );
            }
            let json = d.to_json(&output.sources);
            let fix = &json["fix"];
            assert_eq!(fix["span"]["file_id"], json["primary"]["file_id"]);
            let start = fix["span"]["start"].as_u64().expect("replacement start") as usize;
            let end = fix["span"]["end"].as_u64().expect("replacement end") as usize;
            assert_eq!(&source[start..end], before, "{case}");
            assert_eq!(fix["replacement"], after, "{case}");
            let mut repaired = source.clone();
            repaired.replace_range(start..end, fix["replacement"].as_str().unwrap());
            let path = core_source(&dir, "repair", &repaired);
            let accepted = hirsel_driver::check_path(&path).unwrap();
            assert!(
                !accepted
                    .diagnostics
                    .iter()
                    .any(|d| d.severity == Severity::Error),
                "{case}: {:?}",
                accepted.diagnostics
            );
            // Existing accepted twins remain independently checked. The repair
            // need not be textually identical to the twin.
            let twin_source = std::fs::read_to_string(root().join(twin)).unwrap();
            let twin = hirsel_driver::check_path(&core_source(&dir, "twin", &twin_source)).unwrap();
            assert!(
                !twin
                    .diagnostics
                    .iter()
                    .any(|d| d.severity == Severity::Error)
            );
            std::fs::remove_dir_all(dir).unwrap();
        }
    });
}

#[test]
fn near_miss_help_and_json_fixes_recheck_corpus() {
    hirsel_engine::with_compiler_stack(|| {
        for (kind, before, after, help) in [
            (
                "std",
                "starts_wtih",
                "starts_with",
                "help: did you mean `string.starts_with`?",
            ),
            (
                "field",
                "lenght",
                "length",
                "help: did you mean `x.length`?",
            ),
            ("variant", "Readyy", "Ready", "help: did you mean `Ready`?"),
        ] {
            let case = format!("conformance/LANG/LANG-4-near-miss-{kind}-bad.hir");
            let output = checked(&case);
            let diagnostic = output.diagnostics.first().expect("rejected typo");
            assert_eq!(
                diagnostic.rule.as_str(),
                "LANG-4",
                "{case}: {:?}",
                output.diagnostics
            );
            assert_eq!(diagnostic.notes, [help], "{case}");
            assert!(diagnostic.render_human(&output.sources).contains(help));
            let json = diagnostic.to_json(&output.sources);
            assert_eq!(json["notes"], serde_json::json!([help]));
            let fix = diagnostic.fix.as_ref().expect("one precise suggestion");
            assert_eq!(output.sources.slice(fix.span), Some(before), "{case}");
            assert_eq!(json["fix"]["replacement"], after);
            let mut source = std::fs::read_to_string(root().join(&case)).unwrap();
            let at = output.sources.resolve(fix.span);
            source.replace_range(at.start as usize..at.end as usize, &fix.replacement);
            let dir = std::env::temp_dir()
                .join(format!("hirsel-near-miss-{}-{kind}", std::process::id()));
            std::fs::create_dir_all(&dir).unwrap();
            let path = dir.join("repair.hir");
            std::fs::write(&path, source).unwrap();
            let sidecar = root().join(format!("{case}.json"));
            if sidecar.exists() {
                let mut manifest: serde_json::Value =
                    serde_json::from_slice(&std::fs::read(sidecar).unwrap()).unwrap();
                manifest["sources"][0]["path"] = "repair.hir".into();
                manifest["dependencies"][0]["manifest"] = root()
                    .join("stdlib/std@1/hirsel.json")
                    .canonicalize()
                    .unwrap()
                    .to_string_lossy()
                    .into_owned()
                    .into();
                std::fs::write(
                    dir.join("repair.hir.json"),
                    serde_json::to_vec(&manifest).unwrap(),
                )
                .unwrap();
            }
            let repaired = hirsel_driver::check_path(&path).unwrap();
            assert!(
                !repaired
                    .diagnostics
                    .iter()
                    .any(|d| d.severity == Severity::Error),
                "{case}: {:?}",
                repaired.diagnostics
            );
            let twin = checked(&case.replace("-bad.hir", "-ok.hir"));
            assert!(
                !twin
                    .diagnostics
                    .iter()
                    .any(|d| d.severity == Severity::Error)
            );
            std::fs::remove_dir_all(dir).unwrap();
        }
    });
}

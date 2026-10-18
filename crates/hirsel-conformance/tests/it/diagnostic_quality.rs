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

#[test]
fn mechanical_json_repairs_recheck_and_preserve_primary_sites() {
    hirsel_engine::with_compiler_stack(|| {
        for (case, twin, rule, line, before, after) in [
            (
                "conformance/LANG/LANG-11-block-local-element-borrow-bad.hir",
                "conformance/LANG/LANG-11-block-local-element-copy-run.hir",
                "MODE-1",
                13,
                "",
                "copy ",
            ),
            (
                "conformance/OWN/OWN-2-branch-tail-place-bad.hir",
                "conformance/OWN/OWN-2-branch-tail-place-ok.hir",
                "OWN-2",
                12,
                "",
                "take ",
            ),
            (
                "conformance/OWN/OWN-2-arena-branch-tail-place-bad.hir",
                "conformance/OWN/OWN-2-arena-branch-tail-place-ok.hir",
                "OWN-2",
                15,
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
            let output = checked(case);
            let d = output.diagnostics.first().unwrap();
            assert_eq!(d.rule.as_str(), rule, "{case}");
            assert_eq!(
                output.sources.location(d.primary).unwrap().line,
                line,
                "{case}"
            );
            if d.severity == Severity::Warning {
                let path = root().join(case);
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
            let source = std::fs::read_to_string(root().join(case)).unwrap();
            assert_eq!(&source[start..end], before, "{case}");
            assert_eq!(fix["replacement"], after, "{case}");
            let mut repaired = source.clone();
            repaired.replace_range(start..end, fix["replacement"].as_str().unwrap());
            let dir =
                std::env::temp_dir().join(format!("hirsel-json-repair-{}", std::process::id()));
            std::fs::create_dir_all(&dir).unwrap();
            let path = dir.join("repair.hir");
            std::fs::write(&path, repaired).unwrap();
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
            let twin = checked(twin);
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
fn diagnostic_json_names_operands_bounds_and_declared_slots() {
    hirsel_engine::with_compiler_stack(|| {
        for (case, rule, expected) in [
            (
                "conformance/MODE/MODE-2-box-payload-unique-bad.hir",
                "MODE-2",
                "not proved unique",
            ),
            (
                "conformance/MODE/MODE-4-local-sendable-storage-bad.hir",
                "MODE-4",
                "`p`",
            ),
            (
                "conformance/GEN/GEN-1-copyable-resource-bad.hir",
                "GEN-1",
                "`copyable`",
            ),
            (
                "conformance/LANG/LANG-4-inert-recovery-bad.hir",
                "LANG-4",
                "`Missing`",
            ),
            (
                "conformance/LANG/LANG-12-call-recovery-duplicate-bad.hir",
                "LANG-12",
                "declared:",
            ),
        ] {
            let output = checked(case);
            let d = output.diagnostics.first().unwrap();
            assert_eq!(d.rule.as_str(), rule, "{case}");
            let json = d.to_json(&output.sources);
            assert!(d.message.contains(expected), "{case}: {}", d.message);
            assert!(
                json["notes"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|n| n.as_str().unwrap().starts_with("help: ")),
                "{case}: {json}"
            );
            assert!(!d.message.contains("{0}"), "{case}: {json}");
            assert!(
                !d.secondary.iter().any(|l| l.message.contains("{0}")
                    || l.message.contains("shared with another binding")),
                "{case}: {json}"
            );
            if rule == "GEN-1" {
                assert!(d.message.contains("Token") && d.message.contains("`T`"));
                assert!(d.secondary.iter().any(|l| l.message.contains("copyable")));
            }
        }
    });
}

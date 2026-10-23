//! Checked record, binding, literal and capability-error repairs.
use hirsel_driver::{load_program, native};
use hirsel_engine::{LoweringPolicy, RunConfiguration, SimulationConfig, with_compiler_stack};
use std::path::PathBuf;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

#[test]
fn authoring_repairs_11_rejected_twins_have_one_intended_rule_and_span() {
    with_compiler_stack(|| {
        for (friction, name, rule, primary) in [
            ("dogfood7-F02", "path-copy", "MODE-1", "copy asset.path"),
            ("dogfood7-F02", "partial-move", "OWN-8", "take value.assets"),
            ("dogfood7-F02", "record-reuse", "OWN-2", "asset.path"),
            ("dogfood7-F02", "field-reuse", "OWN-2", "name"),
            ("dogfood7-F03", "mutable-pattern", "LANG-13", "mut"),
            ("dogfood7-F04", "empty-sequence-context", "LANG-2", "[]"),
            ("dogfood7-F05", "file-error", "LANG-8", "?"),
            ("dogfood7-F05", "grant-error", "LANG-8", "?"),
            ("dogfood8-F02", "unique-take", "MODE-2", "Ok(take config)"),
            ("dogfood8-F03", "byte-literal", "SEM-3", "雪"),
            ("dogfood8-F04", "numeric-context", "LANG-2", "65537"),
        ] {
            let path = root().join(format!(
                "crates/hirsel-driver/tests/fixtures/authoring-repairs-11/{name}.hir"
            ));
            let (diagnostics, sources) = match load_program(&path) {
                Ok(mut program) => {
                    let output = program.check(LoweringPolicy::Comptime).unwrap();
                    assert_eq!(output.completion, hirsel_engine::CheckCompletion::Complete);
                    (output.diagnostics, output.sources)
                }
                // Syntax/literal errors can be returned before a program is loaded.
                Err(hirsel_engine::ToolError::Query {
                    error: hirsel_engine::QueryError::Diagnostics(diagnostics),
                    sources,
                    ..
                }) => {
                    assert!(matches!(name, "mutable-pattern" | "byte-literal"));
                    (diagnostics, sources)
                }
                Err(error) => panic!("{name}: {error:?}"),
            };
            let errors: Vec<_> = diagnostics
                .iter()
                .filter(|d| d.severity == hirsel_diag::Severity::Error)
                .collect();
            assert_eq!(errors.len(), 1, "{name}: {errors:?}");
            assert_eq!(errors[0].rule.as_str(), rule, "{name}: {errors:?}");
            let at = sources.resolve(errors[0].primary);
            let source = std::fs::read_to_string(&path).unwrap();
            // Anchor the read rather than a declaration of the same field binding.
            let start = if name == "field-reuse" {
                source.find("let _ = name").unwrap() + "let _ = ".len()
            } else if name == "mutable-pattern" {
                source.find("mut f").unwrap()
            } else {
                source.rfind(primary).unwrap()
            } as u32;
            assert_eq!(
                (at.start, at.end),
                (start, start + primary.len() as u32),
                "{name}"
            );
            eprintln!(
                "{friction}: reject={name} count=1 rule={rule} span={}..{}",
                at.start, at.end
            );
        }
    });
}

#[test]
fn authoring_repairs_11_accepted_twins_agree_on_h_llvm_and_cranelift() {
    with_compiler_stack(|| {
        for (name, manifest) in [
            ("pure", "examples/ownership-repairs/authoring.hir.json"),
            ("io", "examples/ownership-repairs/authoring-io.hir.json"),
            (
                "sequence-context",
                "crates/hirsel-driver/tests/fixtures/authoring-repairs-11/sequence-context.hir.json",
            ),
        ] {
            let mut program = load_program(&root().join(manifest)).unwrap();
            program.check_for_execution().unwrap();
            let config = RunConfiguration {
                entry: program.entry.clone(),
                hash_seed: 0,
                inputs: vec![],
            };
            let execution = hirsel_engine::run_observed_checked(
                &mut program.engine,
                &config,
                SimulationConfig::default(),
            )
            .unwrap();
            let observation = execution.observation.as_ref().unwrap();
            if name != "io" {
                assert!(observation.effects.is_empty());
            } else {
                assert!(
                    !observation.effects.is_empty(),
                    "I/O twin must exercise real operations"
                );
            }
            let reference = native::Observation::of_reference(observation);
            assert_eq!(reference.exit_code, Some(0), "{reference:?}");
            assert!(reference.failures.is_empty());
            let input = program.engine.export_native_input().unwrap();
            let validated = program.engine.import_native_input(input).unwrap();
            let options = native::Options::new(native::TargetSpec::x86_64_unknown_linux_gnu());
            let configurations = vec![
                native::Configuration {
                    options: options.clone(),
                    engine: native::Engine::Heap { campaign: false },
                },
                native::Configuration {
                    options: options.clone(),
                    engine: native::Engine::Native {
                        opt: native::OptLevel::Speed,
                        partition: native::Partition::Whole,
                        runtime: native::Runtime::Diagnostic,
                    },
                },
                native::Configuration {
                    options,
                    engine: native::Engine::Cranelift {
                        partition: native::Partition::Whole,
                        runtime: native::Runtime::Diagnostic,
                    },
                },
            ];
            assert_eq!(configurations.len(), 3);
            let scratch = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
                .join("authoring-repairs-11")
                .join(name);
            std::fs::create_dir_all(&scratch).unwrap();
            let toolchain =
                native::Toolchain::for_matrix(&scratch.join("runtime"), &configurations).unwrap();
            let outcomes = native::differential(
                &validated,
                &program.entry,
                &reference,
                &configurations,
                &toolchain,
                &scratch,
            );
            assert_eq!(outcomes.len(), 3);
            for (configuration, outcome) in outcomes {
                eprintln!("authoring-repairs-11/{name}: {configuration}: {outcome:?}");
                assert!(
                    matches!(outcome, native::Outcome::Agree),
                    "{}: {outcome:?}",
                    configuration
                );
            }
        }
        for (friction, pure, io) in [
            ("dogfood7-F02", 4, 0),
            ("dogfood7-F03", 1, 1),
            ("dogfood7-F04", 1, 0),
            ("dogfood7-F05", 2, 2),
            ("dogfood8-F02", 1, 0),
            ("dogfood8-F03", 1, 0),
            ("dogfood8-F04", 1, 0),
        ] {
            eprintln!("{friction}: accepted pure={pure} io={io}; checked+H+LLVM+Cranelift agree");
        }
    });
}

fn shadow_fixture(source: &str, extra: Option<(&str, &[&str])>) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("main.hir"), source).unwrap();
    let mut sources = vec![serde_json::json!({"path":"main.hir","module":["work"]})];
    if let Some((text, module)) = extra {
        std::fs::write(dir.path().join("other.hir"), text).unwrap();
        sources.push(serde_json::json!({"path":"other.hir","module":module}));
    }
    std::fs::write(
        dir.path().join("hirsel.json"),
        serde_json::json!({
            "schema":1,"package":{"name":"shadow","version":"1"},
            "sources":sources,"dependencies":[],"entry":{"module":["work"],"name":"main"}
        })
        .to_string(),
    )
    .unwrap();
    dir
}

fn assert_shadow_sites(source: &str, declaration: &str, other: bool, name: &str) {
    let extra = other.then_some((declaration, &["work"][..]));
    let dir = shadow_fixture(source, extra);
    let mut program = load_program(&dir.path().join("hirsel.json")).unwrap();
    let output = program.check(LoweringPolicy::Comptime).unwrap();
    assert_eq!(output.completion, hirsel_engine::CheckCompletion::Complete);
    let errors: Vec<_> = output
        .diagnostics
        .iter()
        .filter(|d| d.severity == hirsel_diag::Severity::Error)
        .collect();
    assert_eq!(errors.len(), 1, "{source}: {errors:#?}");
    let d = errors[0];
    assert_eq!(d.rule.as_str(), "LANG-4");
    let local_start = source.find(&format!("let {name}")).unwrap() + 4;
    let primary = output.sources.resolve(d.primary);
    assert_eq!(
        (primary.start as usize, primary.end as usize),
        (local_start, local_start + name.len())
    );
    assert_eq!(output.sources.slice(d.primary), Some(name));
    assert_eq!(d.secondary.len(), 1);
    let label = &d.secondary[0];
    let declaration_source = if other { declaration } else { source };
    let declaration_start =
        declaration_source.find(declaration).unwrap() + declaration.find(name).unwrap();
    let secondary = output.sources.resolve(label.span);
    assert_eq!(
        (secondary.start as usize, secondary.end as usize),
        (declaration_start, declaration_start + name.len())
    );
    assert_eq!(output.sources.slice(label.span), Some(name));
    assert_eq!(primary.file == secondary.file, !other);
    assert!(label.message.contains("shadow@1/work"));
    assert!(
        d.notes
            .iter()
            .any(|n| n.contains("regardless of source order"))
    );
    assert!(d.notes.iter().any(|n| n.contains("update every reference")));
    assert!(d.fix.is_none());
    let human = d.render_human(&output.sources);
    let json = d.to_json(&output.sources);
    assert_eq!(json["primary"]["start"], primary.start);
    assert_eq!(json["secondary"][0]["span"]["start"], secondary.start);
    assert_eq!(json["secondary"][0]["message"], label.message);
    assert!(human.contains(&label.message));
    for note in json["notes"].as_array().unwrap() {
        assert!(human.contains(note.as_str().unwrap()));
    }
    assert!(
        human.find("main.hir").unwrap()
            < human
                .rfind(if other { "other.hir" } else { "main.hir" })
                .unwrap()
    );
}

#[test]
fn authoring_repairs_11_shadow_binding_sites_order_and_witnesses() {
    with_compiler_stack(|| {
        for (source, name) in [
            (
                include_str!("../../../../examples/wc2/repros/incident-7.hir"),
                "counts",
            ),
            (
                include_str!("../../../../examples/linkcheck/repros/incident-1.hir"),
                "deadline",
            ),
        ] {
            assert_shadow_sites(source, &format!("fn {name}() {{}}"), false, name);
        }
        for declaration in [
            "fn counts() {}",
            "const counts: u64 = 1",
            "@test\nfn counts() {}",
        ] {
            let main = "pub fn main() { let counts = 0u64 }";
            for source in [
                format!("{declaration}\n{main}\n"),
                format!("{main}\n{declaration}\n"),
            ] {
                assert_shadow_sites(&source, declaration, false, "counts");
            }
        }
    });
}

#[test]
fn authoring_repairs_11_shadow_binding_sites_multifile_and_utf8() {
    with_compiler_stack(|| {
        for name in ["counts", "deadline"] {
            let source = format!("// 雪\npub fn main() {{ let {name} = 0u64 }}\n");
            let declaration = format!("// λ\nfn {name}() {{}}\n");
            assert_shadow_sites(&source, &declaration, true, name);
        }
    });
}

#[test]
fn authoring_repairs_11_shadow_binding_sites_allowed_namespace_twins() {
    with_compiler_stack(|| {
        for (source, extra) in [
            (
                "pub fn main() { let counts = 0u64 }",
                Some(("fn counts() {}", &[][..])),
            ),
            (
                "type counts { value: u64 }\npub fn main() { let counts = 0u64 }",
                None,
            ),
            (
                "fn counts() {}\npub fn main() { let count_total = 0u64 }",
                None,
            ),
        ] {
            let dir = shadow_fixture(source, extra);
            let mut program = load_program(&dir.path().join("hirsel.json")).unwrap();
            let output = program.check(LoweringPolicy::Comptime).unwrap();
            assert_eq!(output.completion, hirsel_engine::CheckCompletion::Complete);
            assert!(
                output.diagnostics.is_empty(),
                "{source}: {:?}",
                output.diagnostics
            );
            assert!(
                !program
                    .engine
                    .export_tir(&program.entry)
                    .unwrap()
                    .is_empty()
            );
        }
    });
}

#[test]
fn authoring_repairs_11_shadow_binding_sites_follow_relocated_declarations() {
    with_compiler_stack(|| {
        let main = "pub fn main() { let counts = 0u64 }\n";
        let mut engine = hirsel_engine::QueryEngine::new(
            hirsel_engine::default_environment(),
            hirsel_driver::compiler_adapters(),
        );
        for source in [
            format!("fn counts() {{}}\n{main}"),
            format!("{main}// 雪\nfn counts() {{}}\n"),
            format!("{main}// λ\nfn counts() {{ let _ = 1u64 }}\n"),
        ] {
            let file = engine.sources.add("edits.hir", source.clone()).unwrap();
            let output =
                hirsel_engine::check_progress(&mut engine, file, &hirsel_engine::default_context())
                    .unwrap();
            assert_eq!(output.diagnostics.len(), 1, "{:?}", output.diagnostics);
            let d = &output.diagnostics[0];
            assert_eq!(output.sources.slice(d.primary), Some("counts"));
            assert_eq!(d.secondary.len(), 1);
            assert_eq!(output.sources.slice(d.secondary[0].span), Some("counts"));
            let at = output.sources.resolve(d.secondary[0].span);
            let expected = source.find("fn counts").unwrap() + 3;
            assert_eq!(
                (at.start as usize, at.end as usize),
                (expected, expected + 6)
            );
        }
    });
}

fn part_checked(source: &str, extra: Option<(&str, &[&str])>) -> hirsel_engine::CheckOutput {
    let dir = shadow_fixture(source, extra);
    let mut program = load_program(&dir.path().join("hirsel.json")).unwrap();
    let output = program.check(LoweringPolicy::Comptime).unwrap();
    assert_eq!(output.completion, hirsel_engine::CheckCompletion::Complete);
    output
}

fn part_recipe(source: &str, primary: &str) -> String {
    let output = part_checked(source, None);
    let errors: Vec<_> = output
        .diagnostics
        .iter()
        .filter(|d| d.severity == hirsel_diag::Severity::Error)
        .collect();
    let [d] = errors.as_slice() else {
        panic!("{source}: {errors:#?}")
    };
    assert_eq!(d.rule.as_str(), "OWN-8", "{d:#?}");
    assert_eq!(output.sources.slice(d.primary), Some(primary));
    assert!(
        d.message
            .starts_with("destructure the whole owned value to move a part")
    );
    let owner_site = if source.contains("take r: Row") {
        "Row"
    } else {
        "r"
    };
    assert!(
        d.secondary
            .iter()
            .any(|label| output.sources.slice(label.span) == Some(owner_site)
                && label.message.contains("leave `r` partly moved")),
        "{d:#?}"
    );
    assert!(d.fix.is_none());
    let note = d
        .notes
        .iter()
        .find(|n| n.contains("replace the enclosing field-extraction statement"))
        .unwrap_or_else(|| panic!("{d:#?}"));
    assert!(note.contains("consumes all of `r`"));
    assert!(note.contains("`r` cannot be used later"));
    assert!(note.contains("Bind every still-needed field explicitly"));
    let human = d.render_human(&output.sources);
    let json = d.to_json(&output.sources);
    for note in &d.notes {
        assert!(human.contains(note));
        assert!(
            json["notes"]
                .as_array()
                .unwrap()
                .contains(&serde_json::json!(note))
        );
    }
    note.split('`')
        .find(|text| text.starts_with("let "))
        .unwrap()
        .to_owned()
}

fn part_accept(source: &str) {
    let dir = shadow_fixture(source, None);
    let mut program = load_program(&dir.path().join("hirsel.json")).unwrap();
    let output = program.check(LoweringPolicy::Comptime).unwrap();
    assert_eq!(output.completion, hirsel_engine::CheckCompletion::Complete);
    assert!(
        output.diagnostics.is_empty(),
        "{source}: {:?}",
        output.diagnostics
    );
    assert!(
        !program
            .engine
            .export_tir(&program.entry)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn authoring_repairs_11_partial_move_record_recipes_apply_and_export() {
    with_compiler_stack(|| {
        for (source, expected) in [
            (
                "type Row { values: List[u64] }\npub fn main() { let r = Row(values: [1]); let values = take r.values }",
                "let Row(values: moved_values) = take r",
            ),
            (
                "type Inventory { values: List[u64], spare: List[u64] }\npub fn main() { let r = Inventory(values: [1], spare: [2]); let values = take r.values }",
                "let Inventory(values: moved_values, spare: moved_spare) = take r",
            ),
            (
                "type Row { values: List[u64] }\npub fn main() { let moved_values = 1u64; let moved_values_2 = 2u64; let r = Row(values: [1]); let values = take r.values }",
                "let Row(values: moved_values_3) = take r",
            ),
            (
                "type Row { values: List[u64] }\nfn moved_values() {}\nfn moved_values_2() {}\npub fn main() { let r = Row(values: [1]); let values = take r.values }",
                "let Row(values: moved_values_3) = take r",
            ),
            (
                "type Row { values: List[u64] }\ntype Collision = moved_values()\npub fn main() { let r = Row(values: [1]); let values = take r.values }",
                "let Row(values: moved_values) = take r",
            ),
            (
                "type Row { values: List[u64] }\nfn extract(take r: Row) { let values = take r.values }\npub fn main() { extract(Row(values: [1])) }",
                "let Row(values: moved_values) = take r",
            ),
        ] {
            let recipe = part_recipe(source, "take r.values");
            assert_eq!(recipe, expected);
            let repaired = source.replace("let values = take r.values", &recipe);
            part_accept(&repaired);
        }
    });
}

#[test]
fn authoring_repairs_11_partial_move_recipe_consumes_owner() {
    with_compiler_stack(|| {
        let source = "type Row { values: List[u64] }\npub fn main() { let r = Row(values: [1]); let values = take r.values; let _ = r.values }";
        let recipe = part_recipe(source, "take r.values");
        let repaired = source.replace("let values = take r.values", &recipe);
        let output = part_checked(&repaired, None);
        let errors: Vec<_> = output
            .diagnostics
            .iter()
            .filter(|d| d.severity == hirsel_diag::Severity::Error)
            .collect();
        assert_eq!(errors.len(), 1, "{errors:#?}");
        assert_eq!(errors[0].rule.as_str(), "OWN-2");
        assert_eq!(output.sources.slice(errors[0].primary), Some("r.values"));
        part_accept(&repaired.replace("let _ = r.values", "let _ = moved_values"));
    });
}

#[test]
fn authoring_repairs_11_partial_move_bounded_owners_have_no_recipe() {
    with_compiler_stack(|| {
        for (source, primary, resource) in [
            (
                "pub opaque type Row: data uses {} { values: List[u64] }\npub fn main() { let r = Row(values: [1]); let values = take r.values }",
                "take r.values",
                false,
            ),
            (
                "resource type Row { values: List[u64] } drop(x) {}\npub fn main() { let r = Row(values: [1]); let values = take r.values }",
                "take r.values",
                true,
            ),
            (
                "type Row[T] { values: T }\nfn extract[T](take r: Row[T]) { let values = take r.values }\npub fn main() {}",
                "take r.values",
                false,
            ),
            (
                "type Row[T] { values: T }\npub fn main() { let r = Row(values: [1u64]); let values = take r.values }",
                "take r.values",
                false,
            ),
            (
                "type Row { values: List[u64] }\nfn extract(r: Row) { let values = take r.values }\npub fn main() {}",
                "take r.values",
                false,
            ),
            (
                "type Row { values: List[u64] }\nfn extract(mut r: Row) { let values = take r.values }\npub fn main() {}",
                "take r.values",
                false,
            ),
            (
                "type Row { values: List[u64] }\ntype Outer { row: Row }\npub fn main() { let r = Outer(row: Row(values: [1])); let values = take r.row.values }",
                "take r.row.values",
                false,
            ),
            (
                "type Row { values: List[u64] }\npub fn main() { let r = box Row(values: [1]); let values = take r.values }",
                "take r.values",
                false,
            ),
            (
                "pub fn main() { let r: List[List[u64]] = [[1]]; let values = take r[0] }",
                "take r[0]",
                false,
            ),
        ] {
            let output = part_checked(source, None);
            let errors: Vec<_> = output
                .diagnostics
                .iter()
                .filter(|d| d.severity == hirsel_diag::Severity::Error)
                .collect();
            assert_eq!(errors.len(), 1, "{source}: {errors:#?}");
            let d = errors[0];
            assert_eq!(d.rule.as_str(), "OWN-8", "{source}: {d:#?}");
            assert_eq!(output.sources.slice(d.primary), Some(primary));
            assert!(d.fix.is_none());
            let notes = d.notes.join("\n");
            assert!(!notes.contains("let Row("), "{notes}");
            assert!(
                notes.contains(if resource {
                    "suppress its parent finalizer"
                } else {
                    "no concrete pattern is established"
                }),
                "{notes}"
            );
            assert!(notes.contains("LANG-13/DROP"));
        }
        // A public opaque's hidden field is rejected before OWN-8. Neither
        // that error nor an inaccessible private type may invent a pattern.
        for declaration in [
            "pub opaque type Row: data uses {} { values: List[u64] }",
            "type Row { values: List[u64] }",
        ] {
            let source = "type Row = shadow@1/hidden.Row\nfn extract(take r: Row) { let values = take r.values }\npub fn main() {}";
            let output = part_checked(source, Some((declaration, &["hidden"])));
            assert!(!output.diagnostics.is_empty());
            for d in &output.diagnostics {
                assert!(d.fix.is_none());
                assert!(!d.notes.iter().any(|n| n.contains("let Row(")));
            }
        }
        // A transparent type in another module is also conservatively bounded.
        let source = "type Row = shadow@1/hidden.Row\nfn extract(take r: Row) { let values = take r.values }\npub fn main() {}";
        let output = part_checked(
            source,
            Some(("pub type Row { values: List[u64] }", &["hidden"])),
        );
        let [d] = output.diagnostics.as_slice() else {
            panic!("{:?}", output.diagnostics)
        };
        assert_eq!(d.rule.as_str(), "OWN-8");
        assert!(
            d.notes
                .iter()
                .any(|n| n.contains("no concrete pattern is established"))
        );
    });
}

#[test]
fn authoring_repairs_11_partial_move_nested_and_boxed_whole_controls() {
    with_compiler_stack(|| {
        part_accept(
            "type Row { values: List[u64] }\ntype Outer { row: Row }\npub fn main() { let r = Outer(row: Row(values: [1])); let Outer(row: row) = take r; let Row(values: values) = take row; let _ = values }",
        );
        part_accept(
            "type Row { values: List[u64] }\npub fn main() { let r = box Row(values: [1]); let Row(values: values) = take r; let _ = values }",
        );
    });
}

#[test]
fn authoring_repairs_11_partial_move_transfer_and_resource_cleanup_agree() {
    with_compiler_stack(|| {
        // Transfer-without-double-drop and nested boxed ownership are the
        // reference shapes (Rust ptr/mod.rs:490-567, vec.rs:257-292 and
        // vec/into_iter.rs:600-630 at f6c1107f8be13e48465e12c0fd26e6a39fe18e4f).
        // Hirsel binds every field of ordinary records; resource whole
        // destruction has its own parent-finalizer suppression rule.
        let template = r#"
resource type Trace { stdout: Stdout, text: String }
drop(x) { let _ = out.print(x.stdout, x.text) }
type Row { values: List[Trace], spare: box Trace }
pub fn main(rt: Io) -> Result[(), GrantError] {
  let stdout = io.stdout(rt)?
  let r = Row(values: [Trace(stdout: stdout, text: "values\n")], spare: box Trace(stdout: stdout, text: "spare\n"))
  let values = take r.values
  let _ = out.print(stdout, "transferred\n")
  Ok(())
}
"#;
        let recipe = part_recipe(template, "take r.values");
        assert_eq!(
            recipe,
            "let Row(values: moved_values, spare: moved_spare) = take r"
        );
        let repaired = template.replace("let values = take r.values", &recipe);
        // Existing resource rules are controls, never a recommended repair:
        // ordinary cleanup finalizes parent+child, whole destruction suppresses
        // the parent and still finalizes the explicitly bound child once.
        let resource = r#"
resource type Trace { stdout: Stdout, text: String }
drop(x) { let _ = out.print(x.stdout, x.text) }
resource type Parent { child: Trace }
drop(x) { let _ = out.print(x.child.stdout, "parent\n") }
pub fn main(rt: Io) -> Result[(), GrantError] {
  let stdout = io.stdout(rt)?
  let p = Parent(child: Trace(stdout: stdout, text: "child\n"))
  WHOLE
  Ok(())
}
"#;
        for (name, source, expected) in [
            ("record", repaired, "transferred\nspare\nvalues\n"),
            (
                "resource-normal",
                resource.replace("WHOLE", ""),
                "parent\nchild\n",
            ),
            (
                "resource-whole",
                resource.replace("WHOLE", "let Parent(child: child) = take p"),
                "child\n",
            ),
        ] {
            let dir = shadow_fixture(&source, None);
            let mut program = load_program(&dir.path().join("hirsel.json")).unwrap();
            program.check_for_execution().unwrap();
            assert!(
                !program
                    .engine
                    .export_tir(&program.entry)
                    .unwrap()
                    .is_empty()
            );
            let execution = hirsel_engine::run_observed_checked(
                &mut program.engine,
                &RunConfiguration {
                    entry: program.entry.clone(),
                    hash_seed: 0,
                    inputs: vec![],
                },
                SimulationConfig::default(),
            )
            .unwrap();
            let reference =
                native::Observation::of_reference(execution.observation.as_ref().unwrap());
            assert_eq!(reference.exit_code, Some(0), "{name}: {reference:?}");
            assert!(reference.failures.is_empty(), "{name}: {reference:?}");
            assert_eq!(reference.output, expected.as_bytes(), "{name}");
            let input = program.engine.export_native_input().unwrap();
            let validated = program.engine.import_native_input(input).unwrap();
            let options = native::Options::new(native::TargetSpec::x86_64_unknown_linux_gnu());
            let configurations = vec![
                native::Configuration {
                    options: options.clone(),
                    engine: native::Engine::Heap { campaign: false },
                },
                native::Configuration {
                    options: options.clone(),
                    engine: native::Engine::Native {
                        opt: native::OptLevel::Speed,
                        partition: native::Partition::Whole,
                        runtime: native::Runtime::Diagnostic,
                    },
                },
                native::Configuration {
                    options,
                    engine: native::Engine::Cranelift {
                        partition: native::Partition::Whole,
                        runtime: native::Runtime::Diagnostic,
                    },
                },
            ];
            let scratch = tempfile::Builder::new()
                .prefix("partial-move-repairs-20-")
                .tempdir()
                .unwrap();
            let toolchain =
                native::Toolchain::for_matrix(&scratch.path().join("runtime"), &configurations)
                    .unwrap();
            let outcomes = native::differential(
                &validated,
                &program.entry,
                &reference,
                &configurations,
                &toolchain,
                scratch.path(),
            );
            assert_eq!(outcomes.len(), 3);
            for (configuration, outcome) in outcomes {
                eprintln!("partial-move-repairs-20/{name}: {configuration}: {outcome:?}");
                assert!(
                    matches!(outcome, native::Outcome::Agree),
                    "{name}: {configuration}: {outcome:?}"
                );
            }
        }
    });
}

#[test]
fn authoring_repairs_11_partial_move_fresh_names_follow_module_edits() {
    with_compiler_stack(|| {
        let mut engine = crate::source_std::engine();
        let main = "type Row { values: List[u64] }\npub fn main() { let r = Row(values: [1]); let values = take r.values }\n";
        for (prefix, binding) in [
            ("", "moved_values"),
            ("fn moved_values() {}\n", "moved_values_2"),
            ("", "moved_values"),
        ] {
            let source = format!("{prefix}{main}");
            let file = engine.sources.add("partial-move.hir", source).unwrap();
            for _ in 0..2 {
                let output = hirsel_engine::check_progress(
                    &mut engine,
                    file,
                    &hirsel_engine::default_context(),
                )
                .unwrap();
                assert_eq!(output.completion, hirsel_engine::CheckCompletion::Complete);
                let [d] = output.diagnostics.as_slice() else {
                    panic!("{:?}", output.diagnostics)
                };
                assert_eq!(d.rule.as_str(), "OWN-8");
                assert!(
                    d.notes
                        .iter()
                        .any(|n| n.contains(&format!("let Row(values: {binding}) = take r"))),
                    "{d:#?}"
                );
            }
        }
    });
}

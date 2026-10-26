//! Executable NetPeer repair recipe. Payload equality is not a lifetime proof.
use hirsel_driver::{load_program, native};
use hirsel_engine::{LoweringPolicy, RunConfiguration, SimulationConfig, with_compiler_stack};
use std::path::PathBuf;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

#[test]
fn ownership_repairs_reject_original_and_paired_misuses() {
    with_compiler_stack(|| {
        for (name, rule, primary) in [
            ("original", "MODE-1", "[Open([Send(data), Close()])]"),
            ("view-live", "MODE-1", "data = b\"changed\""),
            ("view-escape", "MODE-1", "peer(data)"),
            ("take-reuse", "OWN-2", "data"),
            ("copy-still-borrows", "MODE-1", "peer(copy data)"),
            ("take-borrowed", "OWN-1", "peer(take data)"),
        ] {
            let path = root().join(format!(
                "crates/hirsel-driver/tests/fixtures/ownership-repairs-6/{name}.hir"
            ));
            let mut program = load_program(&path).unwrap();
            let output = program.check(LoweringPolicy::Comptime).unwrap();
            let errors: Vec<_> = output
                .diagnostics
                .iter()
                .filter(|d| d.severity == hirsel_diag::Severity::Error)
                .collect();
            assert_eq!(errors.len(), 1, "{name}: {errors:?}");
            assert_eq!(output.completion, hirsel_engine::CheckCompletion::Complete);
            assert_eq!(errors[0].rule.as_str(), rule, "{name}: {errors:?}");
            let at = output.sources.resolve(errors[0].primary);
            let source = std::fs::read_to_string(&path).unwrap();
            let start = source.rfind(primary).unwrap() as u32;
            assert_eq!(
                (at.start, at.end),
                (start, start + primary.len() as u32),
                "{name}"
            );
            if name == "original" || name == "copy-still-borrows" {
                let notes = errors[0].notes.join("\n");
                for meaning in [
                    "declare the parameter `take data`",
                    "callers write `take data`",
                    "moves their value",
                    "`unique data`",
                    "deep copy",
                    "O(size)",
                    "`copy data` does not help",
                    "stay local",
                ] {
                    assert!(
                        notes.contains(meaning),
                        "{name}: missing {meaning}: {notes}"
                    );
                }
                if name == "original" {
                    let provided = output.sources.resolve(errors[0].secondary[0].span);
                    let parameter_type = source.find("Bytes").unwrap() as u32;
                    assert_eq!(
                        (provided.start, provided.end),
                        (parameter_type, parameter_type + 5)
                    );
                }
            }
        }
    });
}

#[test]
fn ownership_repairs_payloads_and_lifetimes_agree_on_h_llvm_and_cranelift() {
    with_compiler_stack(|| {
        let mut program =
            load_program(&root().join("examples/ownership-repairs/hirsel.json")).unwrap();
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
        assert!(
            observation.effects.is_empty(),
            "fixture must have no capability effects"
        );
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
        let scratch = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("ownership-repairs-6");
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
            eprintln!("ownership-repairs-6: {configuration}: {outcome:?}");
            assert!(
                matches!(outcome, native::Outcome::Agree),
                "{}: {outcome:?}",
                configuration
            );
        }
    });
}

fn checked_capture(source: &str) -> hirsel_engine::CheckOutput {
    let mut engine = crate::source_std::engine();
    let file = engine.sources.add("snapshot.hir", source.into()).unwrap();
    let output =
        hirsel_engine::check_progress(&mut engine, file, &hirsel_engine::default_context())
            .unwrap();
    assert_eq!(output.completion, hirsel_engine::CheckCompletion::Complete);
    output
}

fn mutable_capture_error(output: &hirsel_engine::CheckOutput) -> &hirsel_diag::Diagnostic {
    let errors: Vec<_> = output
        .diagnostics
        .iter()
        .filter(|d| d.severity == hirsel_diag::Severity::Error)
        .collect();
    let [d] = &errors[..] else {
        panic!("{errors:?}")
    };
    assert_eq!(d.rule.as_str(), "CLO-4", "{d:?}");
    d
}

#[test]
fn ownership_repairs_6_mutable_capture_plain_snapshot_and_fresh_names() {
    with_compiler_stack(|| {
        for (prefix, saved) in [
            ("", "saved"),
            ("let saved=1u64;", "saved_2"),
            ("let saved=1u64;let saved_2=2u64;", "saved_3"),
        ] {
            let source =
                format!("// λ 🐑\nfn f()->fn()->u64 {{{prefix}let mut n=0u64;fn()->u64 {{n}}}}");
            let output = checked_capture(&source);
            let d = mutable_capture_error(&output);
            assert_eq!(output.sources.slice(d.primary), Some("fn()->u64 {n}"));
            assert_eq!(
                output.sources.resolve(d.primary).start as usize,
                source.rfind("fn()->").unwrap()
            );
            assert!(
                d.secondary
                    .iter()
                    .any(|l| l.message.contains("`n` is declared `mut`")
                        && output.sources.slice(l.span) == Some("n")),
                "{d:?}"
            );
            let human = d.render_human(&output.sources);
            for meaning in [
                format!("let {saved} = n"),
                format!("capture `{saved}`"),
                "snapshot of the current value".into(),
                "future writes".into(),
                "shared mutable state".into(),
            ] {
                assert!(human.contains(&meaning), "missing {meaning}: {human}");
            }
            assert!(!human.contains("uses {net}"), "{human}");
            let json = d.to_json(&output.sources);
            assert!(
                json["notes"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|n| n.as_str().unwrap().contains(&format!("let {saved} = n")))
            );
            assert!(!json.to_string().contains("uses {net}"));
            let twin = source.replace(
                "fn()->u64 {n}",
                &format!("let {saved}=n;fn()->u64 {{{saved}}}"),
            );
            assert!(checked_capture(&twin).diagnostics.is_empty(), "{twin}");
        }
        // capture-take-reconcile-30/REPORT.md: Many invocations borrow their
        // capture slots. Mutation also violates the read-only alias contract.
        for (source, expected_rules) in [
            (
                "fn f()->fn()->u64 {let mut n=0u64;fn()->u64 {n+=1;n}}",
                &["CLO-4", "LANG-15"][..],
            ),
            (
                "fn f()->once fn()->u64 {let mut n=0u64;fn()->u64 {take n}}",
                &["CLO-4"][..],
            ),
        ] {
            let output = checked_capture(source);
            let errors: Vec<_> = output
                .diagnostics
                .iter()
                .filter(|d| d.severity == hirsel_diag::Severity::Error)
                .collect();
            assert_eq!(
                errors.iter().map(|d| d.rule.as_str()).collect::<Vec<_>>(),
                expected_rules
            );
            let d = errors.iter().find(|d| d.rule.as_str() == "CLO-4").unwrap();
            let human = d.render_human(&output.sources);
            assert!(!human.contains("let saved ="), "{human}");
            assert!(human.contains("takes or mutates"), "{human}");
        }
        // The exact planning witness and explicit immutable twin keep CLO-4.
        let source = "fn f()->fn()->u64 {let mut n=0u64;fn()->u64 {n}}";
        mutable_capture_error(&checked_capture(source));
        assert!(
            checked_capture("fn f()->fn()->u64 {let mut n=0u64;let saved=n;fn()->u64 {saved}}")
                .diagnostics
                .is_empty()
        );
    });
}

#[test]
fn ownership_repairs_6_mutable_counted_snapshot_and_restrictions() {
    with_compiler_stack(|| {
        for source in [
            "fn f()->fn()->u64 {let mut n=[1u64,2,3];fn()->u64 {n[0]}}",
            "fn f(mut n:List[u64])->fn()->u64 {fn()->u64 {n[0]}}",
        ] {
            let output = checked_capture(source);
            let d = mutable_capture_error(&output);
            let human = d.render_human(&output.sources);
            assert!(human.contains("let saved = unique copy n"), "{human}");
            assert!(
                human.contains("O(size)") && human.contains("allocate"),
                "{human}"
            );
            assert!(!human.contains("`take n`"), "{human}");
            let twin = source.replace(
                "fn()->u64 {n[0]}",
                "let saved=unique copy n;fn()->u64 {saved[0]}",
            );
            assert!(checked_capture(&twin).diagnostics.is_empty(), "{twin}");
        }
        for source in [
            "resource type Token { n:u64 } drop(x) {}\nfn f()->fn()->u64 {let mut n=Token(n:1);fn()->u64 {n.n}}",
            "fn f(a:Arena,mut n:List[u64] in a)->fn()->u64 {fn()->u64 {n[0]}}",
            "fn f[T:copyable + inert](mut n:T)->fn()->T {fn()->T {copy n}}",
            "fn f(mut n:fn()->u64)->fn()->u64 {fn()->u64 {n()}}",
        ] {
            let output = checked_capture(source);
            let d = output
                .diagnostics
                .iter()
                .find(|d| {
                    d.rule.as_str() == "CLO-4" && d.message.contains("immutable owned global")
                })
                .unwrap_or_else(|| panic!("{source}: {:?}", output.diagnostics));
            let human = d.render_human(&output.sources);
            assert!(
                human.contains("copyability")
                    && human.contains("current uniqueness")
                    && human.contains("allocation"),
                "{human}"
            );
            assert!(
                !human.contains("let saved =") && !human.contains("`take n`"),
                "{human}"
            );
            assert!(!human.contains("uses {net}"), "{human}");
        }
        let source = "@noalloc fn f(mut n:List[u64])->fn()->u64 {fn()->u64 {n[0]}}";
        let output = checked_capture(source);
        let d = output
            .diagnostics
            .iter()
            .find(|d| d.rule.as_str() == "CLO-4")
            .unwrap();
        let human = d.render_human(&output.sources);
        assert!(
            human.contains("@noalloc") && !human.contains("let saved ="),
            "{human}"
        );
        // Neither an ordinary counted copy nor a deep copy removes arena bounds.
        for (source, replacement) in [
            (
                "fn f(mut n:List[u64])->fn()->u64 {fn()->u64 {n[0]}}",
                "let saved=copy n;fn()->u64 {saved[0]}",
            ),
            (
                "fn f(a:Arena,mut n:List[u64] in a)->fn()->u64 {fn()->u64 {n[0]}}",
                "let saved=unique copy n;fn()->u64 {saved[0]}",
            ),
        ] {
            let output = checked_capture(&source.replace("fn()->u64 {n[0]}", replacement));
            assert!(
                output
                    .diagnostics
                    .iter()
                    .any(|d| d.severity == hirsel_diag::Severity::Error),
                "{source}"
            );
        }
    });
}

#[test]
fn ownership_repairs_6_mutable_snapshot_preserves_authority_failure() {
    with_compiler_stack(|| {
        let source =
            "fn f(dir:Dir)->fn()->Dir uses {Dir} { let g:fn()->Dir uses {} = fn()->Dir {dir}; g }";
        let output = checked_capture(source);
        let d = mutable_capture_error(&output);
        assert_eq!(output.sources.slice(d.primary), Some("dir"));
        assert!(
            !d.render_human(&output.sources)
                .contains("snapshot of the current value")
        );
        assert!(d.render_human(&output.sources).contains("uses {net}"));
    });
}

#[test]
fn ownership_repairs_6_mutable_snapshots_keep_values_on_h_heap_and_native() {
    with_compiler_stack(|| {
        let source = r#"
fn scalar()->fn()->u64 uses {} {
  let mut n=7u64
  let saved=n
  let f:fn()->u64 uses {}=fn()->u64 uses {} {saved}
  n=9
  failure.check(n==9,Bounds())
  f
}
fn counted()->fn()->u64 uses {} {
  let mut n=[1u64,2,3]
  let saved=unique copy n
  let f:fn()->u64 uses {}=fn()->u64 uses {} {saved[0]}
  n[0]=9
  failure.check(n==[9u64,2,3],Bounds())
  f
}
pub fn main()->Result[(),()] {
  let a=scalar()
  let b=counted()
  failure.check(a()==7,Bounds())
  failure.check(b()==1,Bounds())
  let mut empty:List[u64]=[]
  let saved=unique copy empty
  let f:fn()->u64 uses {}=fn()->u64 uses {} {list.len(saved)}
  list.push(mut empty,1)
  failure.check(f()==0 && list.len(empty)==1,Bounds())
  Ok(())
}
"#;
        let mut engine = crate::source_std::engine();
        let file = engine
            .sources
            .add("snapshot-run.hir", source.into())
            .unwrap();
        let diagnostics =
            hirsel_engine::check_snapshot(&mut engine, file, &hirsel_engine::default_context())
                .unwrap();
        assert!(diagnostics.is_empty(), "{diagnostics:?}");
        let entry = hirsel_engine::entry_path(&hirsel_engine::default_context());
        let execution = hirsel_engine::run_observed_snapshot(
            &mut engine,
            &RunConfiguration {
                entry: entry.clone(),
                hash_seed: 0,
                inputs: vec![],
            },
            SimulationConfig::default(),
        )
        .unwrap();
        let reference = native::Observation::of_reference(execution.observation.as_ref().unwrap());
        assert_eq!(reference.exit_code, Some(0), "{reference:?}");
        assert!(reference.failures.is_empty());
        let input = engine.export_native_input().unwrap();
        let validated = engine.import_native_input(input).unwrap();
        let options = native::Options::new(native::TargetSpec::x86_64_unknown_linux_gnu());
        let configurations = vec![
            native::Configuration {
                options: options.clone(),
                engine: native::Engine::Heap { campaign: false },
            },
            native::Configuration {
                options,
                engine: native::Engine::Native {
                    opt: native::OptLevel::Speed,
                    partition: native::Partition::Whole,
                    runtime: native::Runtime::Diagnostic,
                },
            },
        ];
        let scratch = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("mutable-capture-repairs-20");
        std::fs::create_dir_all(&scratch).unwrap();
        let toolchain =
            native::Toolchain::for_matrix(&scratch.join("runtime"), &configurations).unwrap();
        let outcomes = native::differential(
            &validated,
            &entry,
            &reference,
            &configurations,
            &toolchain,
            &scratch,
        );
        assert_eq!(outcomes.len(), 2);
        for (configuration, outcome) in outcomes {
            assert!(
                matches!(outcome, native::Outcome::Agree),
                "{configuration}: {outcome:?}"
            );
        }
    });
}

//! Borrowed captures cannot cross global storage by `copy` or `take`.
use hirsel_engine::*;

fn checked(source: &str) -> CheckOutput {
    checked_in(&mut crate::source_std::engine(), source)
}

fn checked_in(engine: &mut QueryEngine, source: &str) -> CheckOutput {
    let file = engine.sources.add("capture.hir", source.into()).unwrap();
    let output = check_progress(engine, file, &default_context()).unwrap();
    assert_eq!(output.completion, CheckCompletion::Complete);
    output
}

fn original() -> String {
    // Retained csvjoin helper; independent of the driver harness data inventory.
    r#"fn index_row(
  mut index: Map[String, List[List[String]]],
  row: List[String],
  key_index: u64
) {
  let key = row[key_index]
  if !map.update_value(
    mut index,
    key,
    fn(mut rows: List[List[String]]) -> () uses {} {
      list.push(mut rows, unique row)
    }
  ) {
    map.insert(mut index, unique key, [unique row])
  }
}
"#
    .to_owned()
}

#[test]
fn borrowed_capture_repairs_19_original_callback_payload_and_checked_twin() {
    with_compiler_stack(|| {
        for (operation, rule) in [("copy", "MODE-1"), ("take", "OWN-1")] {
            let source = original().replace(
                "list.push(mut rows, unique row)",
                &format!("list.push(mut rows, {operation} row)"),
            );
            let output = checked(&source);
            let errors: Vec<_> = output
                .diagnostics
                .iter()
                .filter(|d| d.severity == hirsel_diag::Severity::Error)
                .collect();
            let [d] = &errors[..] else {
                panic!("{errors:?}")
            };
            let primary = format!("list.push(mut rows, {operation} row)");
            assert_eq!(d.rule.as_str(), rule);
            assert_eq!(output.sources.slice(d.primary), Some(primary.as_str()));
            assert_eq!(
                output.sources.resolve(d.primary).start as usize,
                source.find(&primary).unwrap()
            );
            let json = d.to_json(&output.sources);
            eprintln!("{json}");
            let human = d.render_human(&output.sources);
            assert!(
                d.secondary
                    .iter()
                    .any(|l| l.message.contains("captured by borrowing")),
                "{human}"
            );
            assert!(
                d.secondary
                    .iter()
                    .any(|l| l.message.contains("borrowed parameter")
                        && output.sources.slice(l.span) == Some("List[String]")),
                "{human}"
            );
            assert!(
                d.secondary
                    .iter()
                    .any(|l| l.message.contains("list.push") && l.message.contains("global")),
                "{human}"
            );
            assert!(
                d.notes
                    .iter()
                    .any(|n| n.starts_with("help:") && n.contains("`unique row`")),
                "{human}"
            );
            assert!(
                !human.contains("declare the parameter `take row`"),
                "{human}"
            );
            assert!(!human.contains("copy it first"), "{human}");
            for label in &d.secondary {
                assert!(human.contains(&label.message));
            }
            assert!(
                json["notes"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|n| n.as_str().unwrap().contains("`unique row`"))
            );
        }
        assert!(checked(&original()).diagnostics.is_empty());
    });
}

fn callback(ty: &str, operation: &str) -> String {
    format!(
        "fn probe(row: {ty}, mut m: Map[u64, List[{ty}]]) {{\n let _ = map.update_value(mut m, 0, fn(mut rows: List[{ty}]) -> () uses {{}} {{ list.push(mut rows, {operation} row) }})\n}}\n"
    )
}

fn first_error(output: &CheckOutput) -> &hirsel_diag::Diagnostic {
    let errors: Vec<_> = output
        .diagnostics
        .iter()
        .filter(|d| d.severity == hirsel_diag::Severity::Error)
        .collect();
    let [d] = &errors[..] else {
        panic!("{errors:?}")
    };
    d
}

#[test]
fn borrowed_capture_repairs_19_concrete_sequences_shared_and_parameter_control() {
    with_compiler_stack(|| {
        // Reuse the ordinary source-std preparation for every original/repair
        // pair. Checking the same source path replaces its previous snapshot.
        let mut engine = crate::source_std::engine();
        let mut checks = 0;
        let mut checked = |source: &str| {
            checks += 1;
            checked_in(&mut engine, source)
        };
        for ty in [
            "String",
            "Bytes",
            "List[String]",
            "Array[String]",
            "Inline[String, 1]",
        ] {
            for operation in ["copy", "take"] {
                let source = callback(ty, operation);
                let output = checked(&source);
                let d = first_error(&output);
                assert_eq!(
                    d.rule.as_str(),
                    if operation == "copy" {
                        "MODE-1"
                    } else {
                        "OWN-1"
                    }
                );
                let text = d.render_human(&output.sources);
                assert!(text.contains("`unique row`"), "{text}");
                assert!(text.contains("arena/region bounds"), "{text}");
                assert!(!text.contains("declare the parameter `take row`"), "{text}");
            }
            assert!(checked(&callback(ty, "unique")).diagnostics.is_empty());
            let source = format!(
                "fn probe(row:{ty},mut rows:List[{ty}]) {{ list.push(mut rows, copy row) }}"
            );
            let output = checked(&source);
            let d = first_error(&output);
            assert_eq!(d.rule.as_str(), "MODE-1");
            assert!(
                d.notes
                    .iter()
                    .any(|n| n.contains("declare the parameter `take row`"))
            );
            assert!(
                checked(&source.replace("copy row", "unique row"))
                    .diagnostics
                    .is_empty()
            );
            assert!(
                checked(&source.replace("probe(row:", "probe(take row:"))
                    .diagnostics
                    .is_empty()
            );
        }
        let storage = "fn probe(row:List[String],mut m:Map[u64,List[String]]) { let _ = map.update_value(mut m,0,fn(mut rows:List[String])->() uses {} { rows = copy row }) }";
        let output = checked(storage);
        let d = first_error(&output);
        assert_eq!(d.rule.as_str(), "MODE-1");
        assert!(
            d.secondary.iter().any(|l| l.message.contains("`rows`")
                && l.message.contains("`mut` parameter")
                && l.message.contains("caller")),
            "{d:?}"
        );
        assert!(
            d.notes
                .iter()
                .any(|n| n.starts_with("help:") && n.contains("`unique row`"))
        );
        assert!(
            checked(&storage.replace("copy row", "unique row"))
                .diagnostics
                .is_empty()
        );
        let output = checked(&callback("Shared[String]", "take"));
        let d = first_error(&output);
        assert_eq!(d.rule.as_str(), "OWN-1");
        assert!(
            d.notes
                .iter()
                .any(|n| n.contains("`copy row`") && n.contains("Shared root")),
            "{d:?}"
        );
        assert!(
            checked(&callback("Shared[String]", "copy"))
                .diagnostics
                .is_empty()
        );
        assert_eq!(checks, 34, "every original/repair fixture must be checked");
        eprintln!("borrowed capture concrete sequences: all 34 source checks completed");
    });
}

#[test]
fn borrowed_capture_repairs_19_generic_resource_callable_and_arena_are_conditional() {
    with_compiler_stack(|| {
        let mut engine = crate::source_std::engine();
        let mut checked = |source: &str| checked_in(&mut engine, source);
        for (source, unique_accepted, unique_rule) in [
            (callback("T", "take").replace("fn probe(", "fn probe[T:copyable + inert]("), false, "MODE-1"),
            ("resource type Token { n:u64 } drop(x) {}\nfn consume(take item:Token) {}\nfn invoke(action:fn() -> () uses {}) { action() }\nfn probe(row:Token) { invoke(fn() -> () uses {} { consume(take row) }) }".to_owned(), false, "OWN-5"),
            (callback("fn() -> () uses {}", "take"), false, "MODE-1"),
            // An outer List can be copied even when its elements are generic;
            // the conservative diagnostic still does not promise this for T.
            (callback("List[T]", "take").replace("fn probe(", "fn probe[T:copyable + inert]("), true, ""),
        ] {
            let output = checked(&source);
            let d = first_error(&output);
            assert_eq!(d.rule.as_str(), "OWN-1", "{source}: {d:?}");
            let human = d.render_human(&output.sources);
            assert!(
                human.contains("construct an independent owned value"),
                "{human}"
            );
            assert!(human.contains("borrowed callable captures"), "{human}");
            assert!(
                !d.notes.iter().any(|n| n.starts_with("help:")
                    && (n.contains("`unique row`")
                        || n.contains("`copy row`")
                        || n.contains("declare the parameter"))),
                "{human}"
            );
            assert!(!human.contains("copy it first"), "{human}");
            let twin = checked(&source.replace("take row", "unique row"));
            assert_eq!(twin.diagnostics.is_empty(), unique_accepted, "{source}: {:?}", twin.diagnostics);
            if !unique_accepted {
                assert_eq!(first_error(&twin).rule.as_str(), unique_rule);
            }
        }
        let arena = "fn probe(a:Arena, row:List[String] in a, mut m:Map[u64,List[List[String]]]) {\n let _ = map.update_value(mut m, 0, fn(mut rows:List[List[String]]) -> () uses {} { list.push(mut rows, take row) })\n}";
        let output = checked(arena);
        let errors: Vec<_> = output
            .diagnostics
            .iter()
            .filter(|d| d.severity == hirsel_diag::Severity::Error)
            .collect();
        assert_eq!(
            errors.iter().map(|d| d.rule.as_str()).collect::<Vec<_>>(),
            ["OWN-1", "PROV-4"]
        );
        let d = errors[0];
        assert!(
            d.render_human(&output.sources)
                .contains("arena/region bounds")
        );
        assert!(
            !d.notes
                .iter()
                .any(|n| n.starts_with("help:") && n.contains("`unique row`"))
        );
        let twin = checked(&arena.replace("take row", "unique row"));
        assert_eq!(first_error(&twin).rule.as_str(), "PROV-4");
    });
}

#[test]
fn borrowed_capture_repairs_19_source_order_preserves_first_rule_and_span() {
    with_compiler_stack(|| {
        for (first, second) in [("copy", "take"), ("take", "copy")] {
            let two = format!(
                "fn probe(row:String,mut m:Map[u64,List[String]]) {{ let _ = map.update_value(mut m,0,fn(mut rows:List[String])->() uses {{}} {{ list.push(mut rows,{first} row) }}); let _ = map.update_value(mut m,1,fn(mut rows:List[String])->() uses {{}} {{ list.push(mut rows,{second} row) }}) }}"
            );
            let folded = checked(&two);
            let d = first_error(&folded);
            assert_eq!(
                d.rule.as_str(),
                if first == "copy" { "MODE-1" } else { "OWN-1" }
            );
            assert_eq!(
                folded.sources.resolve(d.primary).start as usize,
                two.find("list.push").unwrap()
            );

            let a = callback("String", first).replace("probe(", "earlier(");
            let b = callback("String", second).replace("probe(", "later(");
            let source = format!("// λ 🐑\n{a}\n{b}");
            let output = checked(&source);
            let errors: Vec<_> = output
                .diagnostics
                .iter()
                .filter(|d| d.severity == hirsel_diag::Severity::Error)
                .collect();
            assert_eq!(errors.len(), 2, "{errors:?}");
            for (d, op) in errors.iter().zip([first, second]) {
                let primary = format!("list.push(mut rows, {op} row)");
                assert_eq!(
                    d.rule.as_str(),
                    if op == "copy" { "MODE-1" } else { "OWN-1" }
                );
                assert_eq!(output.sources.slice(d.primary), Some(primary.as_str()));
                assert_eq!(
                    output.sources.resolve(d.primary).start as usize,
                    source.find(&primary).unwrap()
                );
            }
        }
    });
}

#[test]
fn borrowed_capture_repairs_19_repaired_nested_copy_on_h_llvm_and_cranelift() {
    with_compiler_stack(|| {
        use hirsel_driver::native::*;
        let source = format!(
            "{}\n{}",
            original(),
            r#"
pub fn main() -> Result[(), ()] {
  let mut row = [f"x", f"one"]
  let mut m: Map[String,List[List[String]]] = map.empty()
  index_row(mut m, row, 0)
  let saved = copy m
  index_row(mut m, row, 0)
  row[1] = "changed"
  failure.check(map.get(saved, "x") == Some([["x", "one"]]), Bounds())
  failure.check(map.get(m, "x") == Some([["x", "one"], ["x", "one"]]), Bounds())
  failure.check(row == ["x", "changed"], Bounds())
  let empty: List[String] = []
  let copied = unique empty
  failure.check(list.len(copied) == 0, Bounds())
  Ok(())
}
"#
        );
        let mut engine = crate::source_std::engine();
        let file = engine.sources.add("repaired.hir", source).unwrap();
        let diagnostics = check_snapshot(&mut engine, file, &default_context()).unwrap();
        assert!(diagnostics.is_empty(), "{diagnostics:?}");
        let entry = entry_path(&default_context());
        let execution = run_observed_snapshot(
            &mut engine,
            &RunConfiguration {
                entry: entry.clone(),
                hash_seed: 0,
                inputs: vec![],
            },
            Default::default(),
        )
        .unwrap();
        let reference = Observation::of_reference(execution.observation.as_ref().unwrap());
        assert_eq!(reference.exit_code, Some(0), "{reference:?}");
        assert!(reference.failures.is_empty());
        let input = engine.export_native_input().unwrap();
        let validated = engine.import_native_input(input).unwrap();
        let options = Options::new(TargetSpec::x86_64_unknown_linux_gnu());
        let configurations = vec![
            Configuration {
                options: options.clone(),
                engine: Engine::Heap { campaign: false },
            },
            Configuration {
                options: options.clone(),
                engine: Engine::Native {
                    opt: OptLevel::Speed,
                    partition: Partition::Whole,
                    runtime: Runtime::Diagnostic,
                },
            },
            Configuration {
                options,
                engine: Engine::Cranelift {
                    partition: Partition::Whole,
                    runtime: Runtime::Diagnostic,
                },
            },
        ];
        let scratch = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
            .join("borrowed-capture-repairs-19");
        std::fs::create_dir_all(&scratch).unwrap();
        let tools = Toolchain::for_matrix(&scratch.join("runtime"), &configurations).unwrap();
        let outcomes = differential(
            &validated,
            &entry,
            &reference,
            &configurations,
            &tools,
            &scratch,
        );
        assert_eq!(outcomes.len(), 3);
        for (configuration, outcome) in outcomes {
            eprintln!("borrowed-capture-repairs-19: {configuration}: {outcome:?}");
            assert_eq!(outcome, Outcome::Agree, "{configuration}");
        }
    });
}

//! First-element sequence inference and its contextual twins.
use hirsel_engine::{QueryEngine, check_snapshot, default_context, default_environment};

fn check(source: &str, rule: Option<&str>) {
    let mut engine = QueryEngine::new(default_environment(), hirsel_driver::compiler_adapters());
    let file = engine.sources.add("sequence.hir", source.into()).unwrap();
    let diagnostics = match check_snapshot(&mut engine, file, &default_context()) {
        Ok(ds) | Err(hirsel_types::QueryError::Diagnostics(ds)) => ds,
        Err(error) => panic!("{source}\n{error:?}"),
    };
    match rule {
        None => assert!(diagnostics.is_empty(), "{source}\n{diagnostics:?}"),
        Some(rule) => assert!(
            diagnostics.iter().any(|d| d.rule.as_str() == rule),
            "{source}\nexpected {rule}: {diagnostics:?}"
        ),
    }
}

#[test]
fn probe_01_typed_first() {
    check(
        r#"pub fn main() -> Result[(), u64] {
  let xs = [1u64, 2u64]
  Ok(())
}
"#,
        None,
    );
}

#[test]
fn probe_02_context_list() {
    check(
        r#"pub fn main() -> Result[(), u64] {
  let xs: List[u64] = [1u64, 2]
  let n: u64 = list.len(xs)
  if n != 2u64 || xs[0] != 1u64 || xs[1] != 2u64 { return Error(1u64) }
  Ok(())
}
"#,
        None,
    );
}

#[test]
fn probe_03_later_mismatch() {
    check(
        r#"pub fn main() -> Result[(), u64] {
  let xs: List[u64] = [1u64, true]
  Ok(())
}
"#,
        Some("LANG-7"),
    );
}

#[test]
fn probe_04_empty() {
    check(
        r#"pub fn main() -> Result[(), u64] {
  let xs = []
  Ok(())
}
"#,
        Some("LANG-2"),
    );
}

#[test]
fn probe_05_context_empty() {
    check(
        r#"pub fn main() -> Result[(), u64] {
  let xs: List[u64] = []
  if list.len(xs) != 0u64 { return Error(1u64) }
  Ok(())
}
"#,
        None,
    );
}

#[test]
fn probe_06_untyped_first() {
    check(
        r#"pub fn main() -> Result[(), u64] {
  let xs = [1, 2u64]
  Ok(())
}
"#,
        Some("LANG-2"),
    );
}

#[test]
fn probe_07_untyped_context() {
    check(
        r#"pub fn main() -> Result[(), u64] {
  let xs: List[u64] = [1, 2]
  if xs[0] != 1u64 || xs[1] != 2u64 { return Error(1u64) }
  Ok(())
}
"#,
        None,
    );
}

#[test]
fn probe_08_first_context_later() {
    check(
        r#"pub fn main() -> Result[(), u64] {
  let xs = [1u64, 2]
  Ok(())
}
"#,
        None,
    );
}

#[test]
fn probe_09_nested() {
    check(
        r#"pub fn main() -> Result[(), u64] {
  let xs = [[1u64], [2u64]]
  Ok(())
}
"#,
        None,
    );
}

#[test]
fn probe_10_nested_context() {
    check(
        r#"pub fn main() -> Result[(), u64] {
  let xs: List[List[u64]] = [[1], [2]]
  if xs[0][0] != 1u64 || xs[1][0] != 2u64 { return Error(1u64) }
  Ok(())
}
"#,
        None,
    );
}

#[test]
fn probe_11_nested_empty_first() {
    check(
        r#"pub fn main() -> Result[(), u64] {
  let xs = [[], [1u64]]
  Ok(())
}
"#,
        Some("LANG-2"),
    );
}

#[test]
fn probe_12_context_array() {
    check(
        r#"pub fn main() -> Result[(), u64] {
  let xs: Array[u64] = [1, 2]
  if array.len(xs) != 2u64 || xs[1] != 2u64 { return Error(1u64) }
  Ok(())
}
"#,
        None,
    );
}

#[test]
fn probe_13_context_inline() {
    check(
        r#"pub fn main() -> Result[(), u64] {
  let xs: Inline[u64, 2] = [1, 2]
  if xs[0] != 1u64 || xs[1] != 2u64 { return Error(1u64) }
  Ok(())
}
"#,
        None,
    );
}

#[test]
fn probe_14_inline_length() {
    check(
        r#"pub fn main() -> Result[(), u64] {
  let xs: Inline[u64, 3] = [1, 2]
  Ok(())
}
"#,
        Some("LANG-7"),
    );
}

#[test]
fn probe_15_undetermined_constructor() {
    check(
        r#"pub fn main() -> Result[(), u64] {
  let xs = [None(), Some(1u64)]
  Ok(())
}
"#,
        Some("LANG-2"),
    );
}

#[test]
fn probe_16_context_constructor() {
    check(
        r#"pub fn main() -> Result[(), u64] {
  let xs: List[Option[u64]] = [None(), Some(1)]
  if match xs[0] { None() => false, Some(v) => true } { return Error(1u64) }
  Ok(())
}
"#,
        None,
    );
}

#[test]
fn probe_17_resource_no_context() {
    check(
        r#"resource type Token { id: u64 }
drop(x) {}
pub fn main() -> Result[(), u64] {
  let xs = [Token(id: 1u64)]
  Ok(())
}
"#,
        None,
    );
}

#[test]
fn probe_18_resource_context() {
    check(
        r#"resource type Token { id: u64 }
drop(x) {}
pub fn main() -> Result[(), u64] {
  let x = Token(id: 1u64)
  let xs: List[Token] = [take x]
  if xs[0].id != 1u64 { return Error(1u64) }
  Ok(())
}
"#,
        None,
    );
}

#[test]
fn probe_19_resource_copy() {
    check(
        r#"resource type Token { id: u64 }
drop(x) {}
pub fn main() -> Result[(), u64] {
  let x = Token(id: 1u64)
  let xs: List[Token] = [copy x]
  Ok(())
}
"#,
        Some("OWN-5"),
    );
}

#[test]
fn probe_20_later_statement() {
    check(
        r#"fn accept_list(xs: List[u64]) {}
pub fn main() -> Result[(), u64] {
  let xs = [1u64]
  accept_list(xs)
  Ok(())
}
"#,
        None,
    );
}

#[test]
fn probe_21_outer_argument() {
    check(
        r#"fn accept_array(xs: Array[u64]) -> u64 { xs[1] }
pub fn main() -> Result[(), u64] {
  if accept_array([1, 2]) != 2u64 { return Error(1u64) }
  Ok(())
}
"#,
        None,
    );
}

#[test]
fn probe_22_incident3() {
    check(
        r#"pub fn main() -> Result[(), u64] {
  let sources = ["-"]
  Ok(())
}
"#,
        None,
    );
}

#[test]
fn probe_23_incident3_context() {
    check(
        r#"pub fn main() -> Result[(), u64] {
  let sources: List[String] = ["-"]
  if sources[0] != "-" { return Error(1u64) }
  Ok(())
}
"#,
        None,
    );
}

#[test]
fn probe_24_ambiguous_constructor() {
    check(
        r#"type Left = Mark(u64) | LeftOnly
type Right = Mark(u64) | RightOnly
pub fn main() -> Result[(), u64] {
  let xs = [Mark(1u64)]
  Ok(())
}
"#,
        Some("LANG-2"),
    );
}

#[test]
fn probe_25_context_ambiguous() {
    check(
        r#"type Left = Mark(u64) | LeftOnly
type Right = Mark(u64) | RightOnly
pub fn main() -> Result[(), u64] {
  let xs: List[Left] = [Mark(1)]
  if match xs[0] { Mark(v) => v != 1u64, LeftOnly => true } { return Error(1u64) }
  Ok(())
}
"#,
        None,
    );
}

#[test]
fn probe_26_later_statement_unsuffixed() {
    check(
        r#"fn accept_list(xs: List[u64]) {}
pub fn main() -> Result[(), u64] {
  let xs = [1]
  accept_list(xs)
  Ok(())
}
"#,
        Some("LANG-2"),
    );
}

#[test]
fn probe_27_later_statement_array() {
    check(
        r#"fn accept_array(xs: Array[u64]) {}
pub fn main() -> Result[(), u64] {
  let xs = [1u64]
  accept_array(xs)
  Ok(())
}
"#,
        Some("LANG-7"),
    );
}

#[test]
fn contract_boundaries_and_sites() {
    for (expression, rule, site) in [
        ("[1, 2u64]", "LANG-2", "1"),
        ("[1u64, true]", "LANG-7", "true"),
        ("[]", "LANG-2", "[]"),
        ("[1u64; 2]", "LANG-2", "[1u64; 2]"),
        ("[None(), Some(1u64)]", "LANG-2", "None()"),
        ("[[], [1u64]]", "LANG-2", "[]"),
        ("[1u8, 256]", "SEM-1", "256"),
        ("[return, 1u64]", "LANG-2", "[return, 1u64]"),
    ] {
        let source = format!("fn probe(c: bool) {{ if c {{ let xs = {expression} }}; () }}");
        let mut engine =
            QueryEngine::new(default_environment(), hirsel_driver::compiler_adapters());
        let file = engine.sources.add("site.hir", source).unwrap();
        let ds = match check_snapshot(&mut engine, file, &default_context()) {
            Ok(ds) | Err(hirsel_types::QueryError::Diagnostics(ds)) => ds,
            Err(error) => panic!("{expression}: {error:?}"),
        };
        assert!(
            ds.iter()
                .any(|d| d.rule.as_str() == rule && engine.sources.slice(d.primary) == Some(site)),
            "{expression}: {ds:?}"
        );
    }
}

#[test]
fn modes_and_prefix_contexts() {
    for source in [
        "fn probe() { let xs = [Some(1u64), None()] }",
        "fn probe() { let xs = [[1u64], []] }",
        "fn probe() { let xs = [(1u64 + 2), 3] }",
        "fn probe(c: bool) { let xs = if c { [1u64] } else { [2] } }",
        "fn id[T](take x: T) -> T { take x }\nfn probe() { let xs = id([1u64, 2]) }",
        "type List[T] { value:T }\nfn probe() { let xs = [1u64, 2]; let n:u64=xs[1] }",
        "fn probe(take x: unique Bytes) { let xs = [take x]; let ys: List[Bytes] = take xs }",
        "fn probe(take x: List[unique Bytes]) { let xs = [take x]; let ys: List[List[unique Bytes]] = take xs }",
        "fn probe(x: String) { let xs = [x]; let n = string.byte_len(xs[0]) }",
    ] {
        check(source, None);
    }
    for (source, rule) in [
        (
            "type List[T] { value:T }\nfn probe() { let xs: List[u64] = [1u64, 2] }",
            "LANG-2",
        ),
        ("fn probe() { let xs: Missing = [1u64] }", "LANG-4"),
        (
            "fn probe(x: String) -> List[String] { let xs = [x]; xs }",
            "MODE-1",
        ),
        (
            "fn probe() { let xs = [1]; let later:List[u64]=xs }",
            "LANG-2",
        ),
        (
            "fn probe() { let xs = [1u64]; let later:Array[u64]=xs }",
            "LANG-7",
        ),
        (
            "fn probe() { let xs: List[List[u64]] = [[1u64], [true]] }",
            "LANG-7",
        ),
    ] {
        check(source, Some(rule));
    }
}

#[test]
fn element_normalization_preserves_nested_contracts_and_brands() {
    use hirsel_types::{
        BuiltinType, GenericArgument, SendabilityRequirement, TypeKind, Uniqueness,
    };
    for (source, nested) in [
        (
            "fn probe(take x: unique Bytes) { let xs = [take x] }",
            false,
        ),
        (
            "fn probe(take x: List[unique Bytes]) { let xs = [take x] }",
            true,
        ),
    ] {
        let mut engine =
            QueryEngine::new(default_environment(), hirsel_driver::compiler_adapters());
        let file = engine.sources.add("contract.hir", source.into()).unwrap();
        assert!(
            check_snapshot(&mut engine, file, &default_context())
                .unwrap()
                .is_empty()
        );
        let input = engine.export_native_input().unwrap();
        let mut path = hirsel_engine::entry_path(&default_context());
        path.0.name = "probe".into();
        let definition = input.definitions.get(&path).unwrap();
        let element = definition
            .types
            .iter()
            .find_map(|t| {
                let row = engine.types().get(*t).unwrap();
                match &row.kind {
                    TypeKind::Builtin {
                        kind: BuiltinType::List,
                        arguments,
                    } => match arguments.as_slice() {
                        [GenericArgument::Type(element)]
                            if matches!(
                                engine.types().row(*element).kind,
                                TypeKind::Builtin {
                                    kind: BuiltinType::List,
                                    ..
                                }
                            ) == nested =>
                        {
                            Some(*element)
                        }
                        _ => None,
                    },
                    _ => None,
                }
            })
            .unwrap();
        let row = engine.types().row(element);
        assert_eq!(row.modes.uniqueness, Uniqueness::Aliased);
        assert_eq!(row.sendability_requirement, SendabilityRequirement::None);
        if nested {
            let TypeKind::Builtin { arguments, .. } = &row.kind else {
                unreachable!()
            };
            let GenericArgument::Type(part) = arguments[0] else {
                unreachable!()
            };
            assert_eq!(
                engine.types().row(part).modes.uniqueness,
                Uniqueness::Unique
            );
        }
    }
    check(
        "fn probe(text:String, p:StrPos of text) { let xs=[p]; let ys:List[StrPos of text]=xs }",
        None,
    );
    check(
        "fn probe(text:String, other:String, p:StrPos of text, q:StrPos of other) { let xs=[p,q] }",
        Some("PROV-1"),
    );
}

#[test]
fn constructors_remain_independently_validated() {
    use hirsel_tir::{Constructor, Rvalue};
    use hirsel_types::{BuiltinType, GenericArgument, Size, TypeKind};
    let mut engine = QueryEngine::new(default_environment(), hirsel_driver::compiler_adapters());
    let file = engine
        .sources
        .add("validate.hir", "fn probe() { let xs = [1u64, 2] }".into())
        .unwrap();
    assert!(
        check_snapshot(&mut engine, file, &default_context())
            .unwrap()
            .is_empty()
    );
    let input = engine.export_native_input().unwrap();
    let mut path = hirsel_engine::entry_path(&default_context());
    path.0.name = "probe".into();
    let original = (**input.definitions.get(&path).unwrap()).clone();
    let mut signatures = hirsel_validate::SignatureMap::new(engine.types().clone());
    for signature in input.signatures.values() {
        signatures.insert(signature.definition, (**signature).clone());
    }
    hirsel_validate::assert_valid(&original, engine.types(), &signatures);
    for wrong_length in [false, true] {
        let mut definition = original.clone();
        let body = &mut definition.bodies[definition.entry.0 as usize];
        if wrong_length {
            for place in &mut body.places {
                let mut row = engine.types().get(place.ty).unwrap().clone();
                if let TypeKind::Builtin {
                    kind: BuiltinType::List,
                    arguments,
                } = &row.kind
                {
                    row.kind = TypeKind::Builtin {
                        kind: BuiltinType::Inline,
                        arguments: vec![
                            arguments[0].clone(),
                            GenericArgument::Size(Size::Constant(3)),
                        ]
                        .into(),
                    };
                    place.ty = engine.types().intern(row);
                }
            }
        } else {
            let second = body
                .rvalues
                .iter()
                .find_map(|r| match r {
                    Rvalue::Construct {
                        constructor: Constructor::Sequence,
                        fields,
                        ..
                    } => Some(fields[1].value.place),
                    _ => None,
                })
                .unwrap();
            let mut row = engine
                .types()
                .get(body.places[second.0 as usize].ty)
                .unwrap()
                .clone();
            row.kind = TypeKind::Primitive(hirsel_types::Primitive::Bool);
            body.places[second.0 as usize].ty = engine.types().intern(row);
        }
        definition.types = hirsel_tir::definition_types(engine.types(), &definition);
        let report =
            hirsel_validate::validate(&definition, engine.types(), &signatures, &mut vec![]);
        assert!(!report.violations.is_empty(), "wrong_length={wrong_length}");
    }
}

fn compare_programs(names: &[&str]) {
    use hirsel_driver::native::*;
    use hirsel_engine::{
        RunConfiguration, check_progress, entry_path, run_observed_snapshot, with_compiler_stack,
    };
    with_compiler_stack(|| {
        let mut options = Options::new(TargetSpec::x86_64_unknown_linux_gnu());
        options.validation = Validation::EveryStage;
        let matrix = vec![
            Configuration {
                options: options.clone(),
                engine: Engine::Heap { campaign: true },
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
        let directory =
            std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("sequence-inference-17");
        let toolchain = Toolchain::for_matrix(&directory.join("runtime"), &matrix).unwrap();
        let root = std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."));
        for &name in names {
            let path = format!("conformance/LANG/LANG-2-sequence-inference-17-{name}.hir");
            let source = std::fs::read_to_string(root.join(path)).unwrap();
            let mut engine =
                QueryEngine::new(default_environment(), hirsel_driver::compiler_adapters());
            let file = engine
                .sources
                .add(format!("sequence-{name}.hir"), source)
                .unwrap();
            let checked = check_progress(&mut engine, file, &default_context()).unwrap();
            assert!(
                checked.diagnostics.is_empty(),
                "{name}: {:?}",
                checked.diagnostics
            );
            let entry = entry_path(&default_context());
            let run = run_observed_snapshot(
                &mut engine,
                &RunConfiguration {
                    entry: entry.clone(),
                    hash_seed: 0,
                    inputs: vec![],
                },
                Default::default(),
            )
            .unwrap();
            let reference = Observation::of_reference(run.observation.as_ref().unwrap());
            assert_eq!(
                reference.output,
                if name == "order-run" {
                    b"eval\neval\nfirst\nnamed\n".as_slice()
                } else {
                    b"".as_slice()
                },
                "{name}"
            );
            let input = engine.export_native_input().unwrap();
            let input = engine.import_native_input(input).unwrap();
            std::fs::create_dir_all(directory.join(name)).unwrap();
            let outcomes = differential(
                &input,
                &entry,
                &reference,
                &matrix,
                &toolchain,
                &directory.join(name),
            );
            assert_eq!(outcomes.len(), 3);
            for (configuration, outcome) in outcomes {
                assert!(
                    matches!(outcome, Outcome::Agree),
                    "{name} {configuration}: {outcome:?}"
                );
            }
        }
    });
}

#[test]
fn v_rejects_resource_copy_in_an_inferred_list() {
    use hirsel_tir::{Constructor, Rvalue};
    use hirsel_types::UseKind;
    let mut engine = QueryEngine::new(default_environment(), hirsel_driver::compiler_adapters());
    let file = engine
        .sources
        .add(
            "resource.hir",
            "resource type Token { id:u64 }\ndrop(x) {}\nfn probe() { let xs = [Token(id:1u64)] }"
                .into(),
        )
        .unwrap();
    assert!(
        check_snapshot(&mut engine, file, &default_context())
            .unwrap()
            .is_empty()
    );
    let input = engine.export_native_input().unwrap();
    let mut path = hirsel_engine::entry_path(&default_context());
    path.0.name = "probe".into();
    let mut definition = (**input.definitions.get(&path).unwrap()).clone();
    let mut signatures = hirsel_validate::SignatureMap::new(engine.types().clone());
    for signature in input.signatures.values() {
        signatures.insert(signature.definition, (**signature).clone());
    }
    hirsel_validate::assert_valid(&definition, engine.types(), &signatures);
    let body = &mut definition.bodies[definition.entry.0 as usize];
    let operand = body
        .rvalues
        .iter_mut()
        .find_map(|r| match r {
            Rvalue::Construct {
                constructor: Constructor::Sequence,
                fields,
                ..
            } => Some(&mut fields[0].value),
            _ => None,
        })
        .unwrap();
    assert_eq!(operand.usage, UseKind::Take);
    operand.usage = UseKind::Copy;
    let report = hirsel_validate::validate(&definition, engine.types(), &signatures, &mut vec![]);
    assert!(!report.violations.is_empty());
}

#[test]
fn inferred_and_contextual_lists_match_h_heap_llvm_cranelift() {
    compare_programs(&[
        "first-run",
        "first-context-run",
        "resource-run",
        "order-run",
    ]);
}

#[test]
fn explicit_kinds_and_shadow_match_h_heap_llvm_cranelift() {
    compare_programs(&["kinds-run"]);
}

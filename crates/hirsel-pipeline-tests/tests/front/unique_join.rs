//! MODE-2: unique consumers of expression joins keep a consistent destination type.
use hirsel_engine::{
    DefinitionQueries, QueryEngine, check_snapshot, default_context, default_environment,
};

const DIRECT: &str = r#"fn pick(a:String,b:String,select:bool)->unique String { if string.byte_len(a)==0u64 { return unique b }; unique (if select { a } else { b }) }
fn main()->unique String { pick("a","b",false) }"#;
const BOUND: &str = r#"fn pick(a:String,b:String,select:bool)->unique String { if string.byte_len(a)==0u64 { return unique b }; let selected = if select { a } else { b }; unique selected }
fn main()->unique String { pick("a","b",false) }"#;

fn accepts(source: &str) {
    let mut engine = QueryEngine::new(default_environment(), hirsel_driver::compiler_adapters());
    let file = engine
        .sources
        .add("unique-join.hir", source.into())
        .unwrap();
    let result = check_snapshot(&mut engine, file, &default_context()).unwrap();
    assert!(result.is_empty(), "{result:?}");
    let mut pick = hirsel_engine::entry_path(&default_context());
    pick.0.name = "pick".into();
    let definition = engine.lower_to_tir(&pick).unwrap().value;
    eprintln!("{}", hirsel_lower::print::definition(&definition));
}

#[test]
fn unique_if_join_matches_reference_heap_llvm_and_cranelift() {
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
            std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("unique-if-join");
        let toolchain = Toolchain::for_matrix(&directory.join("runtime"), &matrix).unwrap();
        for (name, source, expected) in [
            (
                "if",
                include_str!("../../../../conformance/MODE/MODE-2-unique-if-join-run.hir"),
                b"a\nb\nb\n".as_slice(),
            ),
            (
                "match",
                include_str!("../../../../conformance/MODE/MODE-2-unique-match-join-run.hir"),
                b"a\nb\nb\n".as_slice(),
            ),
            (
                "nested",
                include_str!("../../../../conformance/MODE/MODE-2-unique-nested-if-join-run.hir"),
                b"a\nb\nb\nb\n".as_slice(),
            ),
            (
                "bound",
                include_str!("../../../../conformance/MODE/MODE-2-unique-bound-join-run.hir"),
                b"a\nb\nb\n".as_slice(),
            ),
            (
                "history",
                r#"fn need(take xs:unique List[u64])->u64 {list.len(xs)}
fn branch(flag:bool)->u64 {
  let mut xs:unique List[u64]=[1u64]
  if flag {{let alias=copy xs};xs=[2u64,3u64]}
  need(take xs)
}
fn cycle(n:u64)->u64 {
  let mut xs:unique List[u64]=[1u64]
  let mut i=0u64
  while i<n {
    let saved=copy xs
    xs=[2u64,3u64]
    failure.check(list.len(saved)==if i==0u64 {1u64} else {2u64},Bounds())
    i=i+1u64
    continue
  }
  need(take xs)
}
fn early(flag:bool)->u64 {
  let xs:unique List[u64]=[1u64]
  if flag {let alias=copy xs;return list.len(alias)}
  need(take xs)
}
type Pair {left:List[u64],right:List[u64]}
fn pair(flag:bool)->u64 {
  let mut xs:unique Pair=Pair(left:[1u64],right:[2u64])
  {let alias=copy xs.left}
  if flag {xs.right=[3u64]}
  xs=Pair(left:[4u64],right:[5u64])
  let repaired:unique Pair=take xs
  list.len(repaired.left)+list.len(repaired.right)
}
pub fn main(rt:Io,args:Args)->Result[(),GrantError] {
  failure.check(branch(false)==1u64&&branch(true)==2u64,Bounds())
  failure.check(cycle(0u64)==1u64&&cycle(1u64)==2u64&&cycle(2u64)==2u64,Bounds())
  failure.check(early(false)==1u64&&early(true)==1u64,Bounds())
  failure.check(pair(false)==2u64&&pair(true)==2u64,Bounds())
  let output=io.stdout(rt)?
  let _=out.print(output,"history-ok\n")
  Ok(())
}
"#,
                b"history-ok\n".as_slice(),
            ),
        ] {
            let mut engine =
                QueryEngine::new(default_environment(), hirsel_driver::compiler_adapters());
            let file = engine
                .sources
                .add(format!("unique-{name}.hir"), source.into())
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
            assert_eq!(reference.output, expected, "{name}");
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
fn unique_if_join_detached_destination_is_still_rejected_by_v() {
    use hirsel_tir::{InstructionKind, Rvalue, Storage};
    use hirsel_types::UseKind;
    let mut engine = QueryEngine::new(default_environment(), hirsel_driver::compiler_adapters());
    let file = engine
        .sources
        .add("unique-join.hir", DIRECT.into())
        .unwrap();
    let checked = check_snapshot(&mut engine, file, &default_context()).unwrap();
    assert!(checked.is_empty(), "{checked:?}");
    let input = engine.export_native_input().unwrap();
    let mut pick = hirsel_engine::entry_path(&default_context());
    pick.0.name = "pick".into();
    let mut definition = (**input.definitions.get(&pick).unwrap()).clone();
    let mut signatures = hirsel_validate::SignatureMap::new(engine.types().clone());
    for signature in input.signatures.values() {
        signatures.insert(signature.definition, (**signature).clone());
    }
    hirsel_validate::assert_valid(&definition, engine.types(), &signatures);
    let body = &mut definition.bodies[definition.entry.0 as usize];
    let join = body
        .blocks
        .iter()
        .flat_map(|b| &b.instructions)
        .find_map(|i| {
            let InstructionKind::Assign { destination, value } = i.kind else {
                return None;
            };
            let Rvalue::Use(ref source) = body.rvalues[value.0 as usize] else {
                return None;
            };
            let root = body.places[destination.0 as usize].root;
            (source.usage == UseKind::Borrow
                && body.locals[root.0 as usize].storage == Storage::Temporary)
                .then_some(root)
        })
        .expect("the conditional's borrowed destination");
    // Reintroduce the original bug: a detached result row on the input view.
    body.locals[join.0 as usize].ty = body.result;
    for place in &mut body.places {
        if place.root == join && place.projection.is_empty() {
            place.ty = body.result;
        }
    }
    let report = hirsel_validate::validate(&definition, engine.types(), &signatures, &mut vec![]);
    assert!(
        report.violations.iter().any(|v| matches!(
            v.kind,
            hirsel_validate::ViolationKind::TypeMismatch {
                entity: "assignment"
            }
        )),
        "{:?}",
        report.violations
    );
}

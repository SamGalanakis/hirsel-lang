//! The retained four-module Propkit scoped-state witness, without its helper workaround.
use hirsel_driver::{load_program_source, native::*};
use hirsel_engine::*;
use hirsel_tir::{BodyId, Definition, InstructionKind, PlaceId, Rvalue, Storage};
use hirsel_types::*;
use std::path::Path;

fn original() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    for (name, source) in [
        (
            "hirsel.json",
            include_str!("fixtures/propkit_scoped_receipt_38/original/hirsel.json"),
        ),
        (
            "main.hir",
            include_str!("fixtures/propkit_scoped_receipt_38/original/main.hir"),
        ),
        (
            "runner.hir",
            include_str!("fixtures/propkit_scoped_receipt_38/original/runner.hir"),
        ),
        (
            "shrink.hir",
            include_str!("fixtures/propkit_scoped_receipt_38/original/shrink.hir"),
        ),
        (
            "strategy.hir",
            include_str!("fixtures/propkit_scoped_receipt_38/original/strategy.hir"),
        ),
    ] {
        std::fs::write(dir.path().join(name), source).unwrap();
    }
    dir
}

#[test]
fn original_source_export_reference_checked() {
    with_compiler_stack(|| {
        eprintln!("PROPKIT38 TOOLS {}", provenance());
        let dir = original();
        let mut program = load_program_source(&dir.path().join("hirsel.json"), None).unwrap();
        let source = program.check(LoweringPolicy::Comptime).unwrap();
        eprintln!("PROPKIT38 original source {:?}", source.diagnostics);
        assert!(source.diagnostics.is_empty(), "{:?}", source.diagnostics);
        program
            .check_for_execution()
            .expect("original source must agree with reachable V");
        let input = program.engine.export_native_input().unwrap();
        let _validated = program.engine.import_native_input(input).unwrap();
        eprintln!("PROPKIT38 original export/V PASS");
        let run = RunConfiguration {
            entry: program.entry.clone(),
            hash_seed: 38,
            inputs: vec![],
        };
        let reference =
            run_observed_snapshot(&mut program.engine, &run, SimulationConfig::default()).unwrap();
        let checked =
            run_observed_checked(&mut program.engine, &run, SimulationConfig::default()).unwrap();
        assert_eq!(reference.observation, checked.observation);
        let observation = Observation::of_reference(reference.observation.as_ref().unwrap());
        assert_eq!(
            observation,
            Observation {
                exit_code: Some(0),
                blocked: false,
                budget: None,
                signaled: None,
                output: vec![],
                failures: vec![],
                roots: vec![]
            }
        );
        eprintln!("PROPKIT38 original reference/H PASS {observation:?}");
    });
}

fn provenance() -> &'static serde_json::Value {
    static VALUE: std::sync::OnceLock<serde_json::Value> = std::sync::OnceLock::new();
    VALUE.get_or_init(|| {
        use sha2::{Digest,Sha256};
        use std::io::Read;
        fn collect(path:std::path::PathBuf, files:&mut std::collections::BTreeMap<String,String>, seen:&mut std::collections::BTreeSet<std::path::PathBuf>) {
            let real=path.canonicalize().unwrap();
            if !seen.insert(real.clone()) {return;}
            if real.is_dir() {
                for entry in std::fs::read_dir(&real).unwrap() {collect(entry.unwrap().path(),files,seen);}
            } else {
                let mut file=std::fs::File::open(&real).unwrap();let mut hash=Sha256::new();let mut buffer=[0u8;65536];
                loop {let n=file.read(&mut buffer).unwrap();if n==0 {break;}hash.update(&buffer[..n]);}
                files.insert(path.display().to_string(),format!("{:x}",hash.finalize()));
            }
        }
        let mut files=std::collections::BTreeMap::new();let mut seen=std::collections::BTreeSet::new();let mut environment=std::collections::BTreeMap::new();
        collect(std::env::current_exe().unwrap(),&mut files,&mut seen);
        for key in ["HIRSEL_PRODUCTS","HIRSEL_RUNTIME_ARCHIVES","HIRSEL_LINK_KIT","HIRSEL_GLIBC_FLOOR","LLVM_SYS_231_PREFIX"] {
            if let Some(value)=std::env::var_os(key) {
                environment.insert(key,value.to_string_lossy().into_owned());
                let path=std::path::PathBuf::from(value);
                if key=="LLVM_SYS_231_PREFIX" {
                    for tool in ["bin/clang","bin/ld.lld"] {collect(path.join(tool),&mut files,&mut seen);}
                } else {collect(path,&mut files,&mut seen);}
            }
        }
        serde_json::json!({"compiler_identity":format!("{:?}",hirsel_driver::COMPILER_IDENTITY),"files_sha256":files,"environment":environment,
            "observation_schema":OBSERVATION_SCHEMA,"matrix":MATRIX_VERSION,
            "observation_limits":"Native and H do not expose a separate resource/finalizer/capability event stream or arbitrary returned graph: those fields INCOMPLETE. Here visible destructor order and payload values are explicit output tokens, entry returns unit; roots tracing disabled/empty; no file stages (Pending/AfterWrite N/A)."})
    })
}

const FRONTIER: &str = include_str!("fixtures/propkit_scoped_receipt_38/frontier.hir");
const RUNNER: &str = include_str!("fixtures/propkit_scoped_receipt_38/original/runner.hir");
const HELPER: &str = r#"
fn reductions[T: copyable + inert](source: Shared[strategy.Strategy[T]], value: T) -> unique List[unique T] uses {} {
  shared.with(source, fn(s: strategy.Strategy[T]) -> unique List[unique T] uses {} { s.reduce(value) })
}
"#;

fn helper_runner() -> String {
    let first = RUNNER.find("  let frontier = shared.with(").unwrap();
    let end = RUNNER[first..].find("  let mut state").unwrap() + first;
    let mut source = RUNNER.to_string();
    source.replace_range(first..end, "  let frontier = reductions(source, initial)\n");
    let first = source.find("      let next = shared.with(").unwrap();
    let end = source[first..].find("      let current").unwrap() + first;
    source.replace_range(
        first..end,
        "      let next = reductions(source, state.current)\n",
    );
    source.push_str(HELPER);
    source
}

fn source_input(source: &str) -> NativeInput {
    let mut engine = QueryEngine::new(default_environment(), hirsel_driver::compiler_adapters());
    let file = engine.sources.add("capture.hir", source.into()).unwrap();
    let output = check_progress(&mut engine, file, &default_context()).unwrap();
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    engine.export_native_input().unwrap()
}

fn signatures(input: &NativeInput) -> hirsel_validate::SignatureMap {
    let mut signatures = hirsel_validate::SignatureMap::new(input.types.clone());
    signatures.extend_signatures(input.signatures.values().map(|s| s.as_ref().clone()));
    signatures
}

#[test]
fn bounded_source_v_controls() {
    with_compiler_stack(|| {
        let start = RUNNER.find("pub fn minimize").unwrap();
        let end = RUNNER.find("\n\nfn property_error").unwrap();
        let mono = RUNNER[start..end]
            .replace("[T: copyable + inert]", "")
            .replace("[T]", "[List[i64]]")
            .replace("unique T", "unique List[i64]")
            .replace("fn(T)", "fn(List[i64])");
        let concrete = format!("{}{}{}", &RUNNER[..start], mono, &RUNNER[end..]);
        for (label, source) in [
            (
                "straightline",
                RUNNER.replace(
                    "while tested < budget && shrink.simplify(mut state)",
                    "if shrink.simplify(mut state)",
                ),
            ),
            (
                "without-judge",
                RUNNER.replace(
                    "let verdict = judge(property, state.current) ? property_error",
                    "let verdict = Fail(category)",
                ),
            ),
            ("concrete", concrete),
            ("helper", helper_runner()),
        ] {
            let dir = original();
            std::fs::write(dir.path().join("runner.hir"), source).unwrap();
            let mut program = load_program_source(&dir.path().join("hirsel.json"), None).unwrap();
            let source = program.check(LoweringPolicy::Comptime).unwrap();
            assert!(
                source.diagnostics.is_empty(),
                "{label}: {:?}",
                source.diagnostics
            );
            program.check_for_execution().unwrap();
            let input = program.engine.export_native_input().unwrap();
            program.engine.import_native_input(input).unwrap();
            eprintln!("PROPKIT38 control {label} source/export/V PASS");
        }
        for (label, source) in [
            ("frontier", FRONTIER.to_string()),
            (
                "whole-owner",
                FRONTIER.replace("inspect(state.current)", "inspect(state)"),
            ),
        ] {
            let input = source_input(&source);
            for definition in input.definitions.values() {
                hirsel_validate::assert_valid(definition, &input.types, &signatures(&input));
            }
            eprintln!("PROPKIT38 control {label} source/export/V PASS");
        }
    });
}

fn native_matrix() -> Vec<Configuration> {
    [
        Engine::Native {
            opt: OptLevel::Speed,
            partition: Partition::Whole,
            runtime: Runtime::Default,
        },
        Engine::Cranelift {
            partition: Partition::Whole,
            runtime: Runtime::Diagnostic,
        },
    ]
    .into_iter()
    .map(|engine| {
        let mut options = Options::new(TargetSpec::x86_64_unknown_linux_gnu());
        options.validation = Validation::EveryStage;
        Configuration { options, engine }
    })
    .collect()
}

fn expected(output: &[u8]) -> Observation {
    Observation {
        exit_code: Some(0),
        blocked: false,
        budget: None,
        signaled: None,
        output: output.to_vec(),
        failures: vec![],
        roots: vec![],
    }
}

fn native_engines(
    dir: &Path,
    input: &ValidatedNativeInput,
    entry: &hirsel_syntax::DefPath,
    expected: &Observation,
) {
    eprintln!("PROPKIT38 TOOLS {}", provenance());
    let configurations = native_matrix();
    let mut toolchain = Toolchain::for_matrix(&dir.join("runtime"), &configurations).unwrap();
    toolchain.launch_timeout = std::time::Duration::from_secs(60);
    std::fs::create_dir_all(dir.join("engines")).unwrap();
    let outcomes = differential_in(
        input,
        entry,
        expected,
        &configurations,
        &toolchain,
        &dir.join("engines"),
        &Fixture::default(),
    );
    assert_eq!(outcomes.len(), 2);
    for (configuration, outcome) in outcomes {
        eprintln!("PROPKIT38 engine {configuration}: {outcome:?}");
        assert_eq!(outcome, Outcome::Agree);
    }
}

#[test]
fn original_native() {
    with_compiler_stack(|| {
        let dir = original();
        let mut program = load_program_source(&dir.path().join("hirsel.json"), None).unwrap();
        program.check_for_execution().unwrap();
        let input = program.engine.export_native_input().unwrap();
        let validated = program.engine.import_native_input(input).unwrap();
        native_engines(dir.path(), &validated, &program.entry, &expected(b""));
    });
}

const OBSERVED_MAIN: &str = r#"
resource type Mark { output: Stdout, label: String }
drop(m) { let _ = out.print(m.output, m.label) }
fn grant_error(e: GrantError) -> () uses {} {}
pub fn main(io: Io) -> Result[(), ()] {
  let output = io.stdout(io) ? grant_error
  let first = Mark(output: output, label: "1")
  let second = Mark(output: output, label: "2")
  let source: unique strategy.Strategy[List[i64]] = strategy.Strategy(
    draw: fn(seed: u64) -> Result[unique List[i64], strategy.GenError] uses {} { Ok([11i64]) },
    reduce: fn(xs: List[i64]) -> unique List[unique List[i64]] uses {} { shrink.lists(xs) }, depth: 1)
  let property: unique fn(List[i64]) -> Result[runner.Outcome,runner.PropertyError] uses {} =
    fn(xs: List[i64]) -> Result[runner.Outcome,runner.PropertyError] uses {} { Ok(Fail(7)) }
  let root = shared.freeze(take source)
  let callback = shared.freeze(take property)
  match runner.minimize(root, callback, [11i64], 7, 64) {
    Ok((value, tested)) => {
      if list.len(value) != 0 || tested != 1 { return Error(()) }
      let _ = out.print(output, "0:1;")
    },
    Error(_) => return Error(()),
  }
  Ok(())
}
"#;

#[test]
fn inline_and_helper_values_cleanup_four_engines() {
    with_compiler_stack(|| {
        for (label, runner) in [("inline", RUNNER.to_string()), ("helper", helper_runner())] {
            let dir = original();
            std::fs::write(dir.path().join("main.hir"), OBSERVED_MAIN).unwrap();
            std::fs::write(dir.path().join("runner.hir"), runner).unwrap();
            let mut program = load_program_source(&dir.path().join("hirsel.json"), None).unwrap();
            program.check_for_execution().unwrap();
            let input = program.engine.export_native_input().unwrap();
            let validated = program.engine.import_native_input(input).unwrap();
            let run = RunConfiguration {
                entry: program.entry.clone(),
                hash_seed: 38,
                inputs: vec![],
            };
            let reference =
                run_observed_snapshot(&mut program.engine, &run, SimulationConfig::default())
                    .unwrap();
            let checked =
                run_observed_checked(&mut program.engine, &run, SimulationConfig::default())
                    .unwrap();
            assert_eq!(reference.observation, checked.observation);
            let observation = Observation::of_reference(reference.observation.as_ref().unwrap());
            assert_eq!(observation, expected(b"0:1;21"), "{label}");
            eprintln!("PROPKIT38 {label} reference/H values and ordered cleanup PASS");
            native_engines(dir.path(), &validated, &program.entry, &observation);
        }
    });
}

fn replace_local_type(
    definition: &mut Definition,
    body: BodyId,
    place: PlaceId,
    ty: TypeId,
    types: &TypeInterner,
) {
    let body = &mut definition.bodies[body.0 as usize];
    let root = body.places[place.0 as usize].root;
    body.locals[root.0 as usize].ty = ty;
    for place in &mut body.places {
        if place.root == root && place.projection.is_empty() {
            place.ty = ty;
        }
    }
    definition.types = hirsel_tir::definition_types(types, definition);
}

fn rejected(definition: &Definition, input: &NativeInput, entity: &'static str) {
    let violations =
        hirsel_validate::validate(definition, &input.types, &signatures(input), &mut vec![])
            .violations;
    assert!(
        violations
            .iter()
            .any(|v| v.kind == hirsel_validate::ViolationKind::TypeMismatch { entity }),
        "{entity}: {violations:?}"
    );
    eprintln!("PROPKIT38 forged {entity} rejected {violations:?}");
}

#[test]
fn reached_original_capture_receipts_are_independent() {
    with_compiler_stack(|| {
        let dir = original();
        let mut program = load_program_source(&dir.path().join("hirsel.json"), None).unwrap();
        program.check_for_execution().unwrap();
        let input = program.engine.export_native_input().unwrap();
        let mut path = program.entry.clone();
        path.0.modules = vec!["runner".into()];
        path.0.name = "minimize".into();
        let definition = &input.definitions[&path];
        hirsel_validate::assert_valid(definition, &input.types, &signatures(&input));
        let body = definition.entry;
        let parent = &definition.bodies[body.0 as usize];
        let sites: Vec<_> = parent
            .blocks
            .iter()
            .flat_map(|b| &b.instructions)
            .filter_map(|i| {
                let InstructionKind::Assign { destination, value } = i.kind else {
                    return None;
                };
                let Rvalue::Closure { body: child, .. } = parent.rvalues[value.0 as usize] else {
                    return None;
                };
                Some((destination, value, child))
            })
            .collect();
        assert_eq!(
            sites.len(),
            2,
            "initial and loop-edge shared.with callbacks"
        );
        // A failed local run may recover only its missing inner cells. Routine
        // runs omit this switch and exercise both complete construction sites.
        let only_global = std::env::var_os("HIRSEL_PROPKIT38_ONLY_GLOBAL_CONTROL").is_some();
        eprintln!("PROPKIT38 receipt recovery only_global={only_global}");
        for (site, (destination, value, child)) in sites.into_iter().enumerate() {
            if site != 0 && only_global {
                continue;
            }
            let original = input
                .types
                .get(parent.places[destination.0 as usize].ty)
                .unwrap();
            assert!(matches!(
                original.modes.sendability,
                Sendability::DependsOn(_)
            ));
            assert_eq!(original.modes.multiplicity, Multiplicity::Many);
            let TypeKind::Function(header) = &original.kind else {
                panic!()
            };
            assert_eq!(header.uses, Uses::Only(AuthoritySummary::inert()));
            if !only_global {
                for claim in [Sendability::Sendable, Sendability::DependsOn(vec![].into())] {
                    let mut forged = definition.as_ref().clone();
                    let mut row = original.clone();
                    row.modes.sendability = claim;
                    replace_local_type(
                        &mut forged,
                        body,
                        destination,
                        input.types.intern(row),
                        &input.types,
                    );
                    rejected(&forged, &input, "closure sendability");
                }
                for (usage, entity) in [
                    (UseKind::Copy, "closure source classification"),
                    (UseKind::Take, "closure multiplicity"),
                ] {
                    let mut forged = definition.as_ref().clone();
                    let Rvalue::Closure { captures, .. } =
                        &mut forged.bodies[body.0 as usize].rvalues[value.0 as usize]
                    else {
                        panic!()
                    };
                    assert_eq!(captures[0].usage, UseKind::Borrow);
                    captures[0].usage = usage;
                    rejected(&forged, &input, entity);
                }
            }
            if site != 0 {
                continue;
            }
            let mut forged = definition.as_ref().clone();
            let nested = &forged.bodies[child.0 as usize];
            let slot = nested
                .locals
                .iter()
                .position(|l| l.storage == Storage::Capture)
                .unwrap();
            let place = nested
                .places
                .iter()
                .position(|p| p.root.0 as usize == slot && p.projection.is_empty())
                .unwrap();
            let mut row = input.types.get(nested.locals[slot].ty).unwrap().clone();
            assert!(!row.modes.relation.is_global());
            row.modes.relation = Relation::global();
            replace_local_type(
                &mut forged,
                child,
                PlaceId(place as u32),
                input.types.intern(row),
                &input.types,
            );
            // This source's content is already Global: a Global capture view
            // is a legal independently proved receipt, not a locality forgery.
            hirsel_validate::assert_valid(&forged, &input.types, &signatures(&input));
            eprintln!("PROPKIT38 original Global capture-content view remains valid");
        }
    });
}

#[test]
fn counted_local_stale_and_symbolic_source_negatives() {
    with_compiler_stack(|| {
        let prelude = "fn keep[T](take x:T) uses{}{}\nfn need(take f:sendable once fn()->() uses{}) uses{}{}\n";
        for (label, probe, rule) in [
            (
                "counted",
                "fn probe(){let xs:List[String]=[\"old\"];need(fn(){keep(copy xs)})}",
                "MODE-4",
            ),
            (
                "alias-live",
                "fn probe(){let xs:unique List[String]=[\"old\"];let alias=copy xs;need(fn(){keep(take xs)})}",
                "MODE-4",
            ),
            (
                "alias-dropped",
                "fn probe(){let xs:unique List[String]=[\"old\"];{let alias=copy xs;}need(fn(){keep(take xs)})}",
                "MODE-4",
            ),
            (
                "symbolic",
                "fn probe[T:copyable+inert](take xs:T){need(fn(){keep(take xs)})}",
                "MODE-4",
            ),
            (
                "local",
                "fn probe(xs:List[String])->once fn()->() uses{}{fn(){keep(copy xs)}}",
                "CLO-4",
            ),
        ] {
            let source = format!("{prelude}{probe}\npub fn main(){{}}");
            let mut engine =
                QueryEngine::new(default_environment(), hirsel_driver::compiler_adapters());
            let file = engine.sources.add("negative.hir", source).unwrap();
            engine.parse_file(file, &default_context()).unwrap();
            let mut path = entry_path(&default_context());
            path.0.name = "probe".into();
            let diagnostics = match engine.mode_definition(&path) {
                Ok(output) => output.diagnostics,
                Err(QueryError::Diagnostics(diagnostics)) => diagnostics,
                Err(error) => panic!("{label}: {error:?}"),
            };
            assert!(
                diagnostics.iter().any(|d| d.rule.as_str() == rule),
                "{label}: {diagnostics:?}"
            );
            eprintln!(
                "PROPKIT38 source negative {label} {rule} PASS {:?}",
                diagnostics
            );
        }
        let mut engine =
            QueryEngine::new(default_environment(), hirsel_driver::compiler_adapters());
        let file = engine
            .sources
            .add(
                "take.hir",
                FRONTIER.replace("inspect(state.current)", "inspect(take state.current)"),
            )
            .unwrap();
        let output = check_progress(&mut engine, file, &default_context()).unwrap();
        assert!(
            output
                .diagnostics
                .iter()
                .any(|d| d.rule.as_str() == "OWN-8")
        );
    });
}

#[test]
fn stale_symbolic_environment_receipts_are_rejected() {
    with_compiler_stack(|| {
        for alias in ["let alias=copy state;", "{let alias=copy state;}"] {
            let source = FRONTIER.replace("  invoke(fn()", &format!("  {alias}\n  invoke(fn()"));
            let input = source_input(&source);
            let mut path = entry_path(&default_context());
            path.0.name = "probe".into();
            let definition = &input.definitions[&path];
            hirsel_validate::assert_valid(definition, &input.types, &signatures(&input));
            let body = definition.entry;
            let parent = &definition.bodies[body.0 as usize];
            let destination = parent
                .blocks
                .iter()
                .flat_map(|b| &b.instructions)
                .find_map(|i| {
                    let InstructionKind::Assign { destination, value } = i.kind else {
                        return None;
                    };
                    matches!(parent.rvalues[value.0 as usize], Rvalue::Closure { .. })
                        .then_some(destination)
                })
                .unwrap();
            let original = input
                .types
                .get(parent.places[destination.0 as usize].ty)
                .unwrap();
            assert_eq!(original.modes.sendability, Sendability::NotSendable);
            for claim in [
                Sendability::Sendable,
                Sendability::DependsOn(
                    vec![TypeParameterRef::body(
                        parent.parameter_scope,
                        TypeParamId(0),
                    )]
                    .into(),
                ),
            ] {
                let mut row = original.clone();
                row.modes.sendability = claim;
                let mut forged = definition.as_ref().clone();
                replace_local_type(
                    &mut forged,
                    body,
                    destination,
                    input.types.intern(row),
                    &input.types,
                );
                rejected(&forged, &input, "closure sendability");
            }
        }
    });
}

#[test]
fn a_local_capture_cannot_claim_a_global_environment() {
    with_compiler_stack(|| {
        let input = source_input(
            "fn probe(value:List[String])->u64 uses{} {let f=fn()->u64 uses{}{list.len(value)};f()}\npub fn main(){let _=probe([\"a\"])}",
        );
        let mut path = entry_path(&default_context());
        path.0.name = "probe".into();
        let definition = &input.definitions[&path];
        hirsel_validate::assert_valid(definition, &input.types, &signatures(&input));
        let body = definition.entry;
        let parent = &definition.bodies[body.0 as usize];
        let destination = parent
            .blocks
            .iter()
            .flat_map(|b| &b.instructions)
            .find_map(|i| {
                let InstructionKind::Assign { destination, value } = i.kind else {
                    return None;
                };
                matches!(parent.rvalues[value.0 as usize], Rvalue::Closure { .. })
                    .then_some(destination)
            })
            .unwrap();
        let mut row = input
            .types
            .get(parent.places[destination.0 as usize].ty)
            .unwrap()
            .clone();
        assert!(!row.modes.relation.is_global());
        row.modes.relation = Relation::global();
        let mut forged = definition.as_ref().clone();
        replace_local_type(
            &mut forged,
            body,
            destination,
            input.types.intern(row),
            &input.types,
        );
        rejected(&forged, &input, "closure locality");
    });
}

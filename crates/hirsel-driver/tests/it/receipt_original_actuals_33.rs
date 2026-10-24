//! Real source/V and ordinary execution controls for the receipt factory seam.
//! No receipt-aware entry, provider authority or public seal is activated here.
use hirsel_driver::compiler_adapters;
use hirsel_driver::native::{
    self, Configuration, Engine, Observation, OptLevel, Options, Outcome, Partition, Runtime,
    TargetSpec, Toolchain,
};
use hirsel_engine::{
    QueryEngine, RunConfiguration, SimulationConfig, ValidatedNativeInput, Verify, check_progress,
    default_context, default_environment, entry_path, run_observed_snapshot, with_compiler_stack,
};
use hirsel_lir::sealed_payload_source::ClosedStorageProfile;
use hirsel_rc::instance::{Actuals, key_of};
use hirsel_rc::sealed_source::contract::registry::{Context, Registry};
use hirsel_rc::sealed_source::contract::{self, Input, Port};
use hirsel_rc::sealed_source::{SourceError, SourceInput};
use hirsel_syntax::ContentHash;
use hirsel_tir::{Callee, InstructionKind, Rvalue, Terminator};
use hirsel_types::{GenericArgument, TypeKind, TypeMemo, Uniqueness, UseKind};
use std::collections::BTreeMap;
use std::path::PathBuf;

const SOURCE: &str = r#"
fn id[T](take value:T)->T {take value}
pub fn main()->Result[(),()] {
  let ordinary:Bytes=copy_to(heap,b"abc")
  let owned:unique Bytes=unique b"abcde"
  let a:Bytes=id(take ordinary)
  let b:unique Bytes=id(take owned)
  failure.check(bytes.len(a)==3u64 && bytes.len(b)==5u64,Bounds())
  Ok(())
}
"#;

const DECLARED_SOURCE: &str = r#"
fn id[T](take value:T)->T {take value}
pub fn main()->Result[(),()] {
  let ordinary_id:fn(take value:Bytes)->Bytes=id
  let unique_id:fn(take value:unique Bytes)->unique Bytes=id
  let ordinary:Bytes=copy_to(heap,b"abc")
  let owned:unique Bytes=unique b"abcde"
  let a:Bytes=ordinary_id(take ordinary)
  let b:unique Bytes=unique_id(take owned)
  failure.check(bytes.len(a)==3u64 && bytes.len(b)==5u64,Bounds())
  Ok(())
}
"#;

fn checked_source(source: &str) -> (QueryEngine, ValidatedNativeInput) {
    let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
    let file = engine
        .sources
        .add("receipt-original-actuals-33.hir", source.into())
        .unwrap();
    let checked = check_progress(&mut engine, file, &default_context()).unwrap();
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
    let original = engine.export_native_input().unwrap();
    let validated = engine.import_native_input(original).unwrap();
    assert_eq!(validated.verification(), Verify::On);
    (engine, validated)
}

#[test]
fn port_context_direct_calls_keep_original_input_modes_beside_normalized_actuals() {
    with_compiler_stack(|| {
        let source = SOURCE.replace(
            "  failure.check(bytes.len(a)==3u64 && bytes.len(b)==5u64,Bounds())\n",
            "",
        );
        let (_, validated) = checked_source(&source);
        let original = validated.snapshot();
        let mut inputs = Vec::new();
        for body in original
            .definitions
            .values()
            .flat_map(|definition| &definition.bodies)
        {
            for block in &body.blocks {
                if let Terminator::Call {
                    callee:
                        Callee::Definition {
                            path,
                            arguments: generic,
                        },
                    arguments,
                    returns,
                    ..
                } = &block.terminator
                    && path.0.name == "id"
                {
                    let [GenericArgument::Type(actual)] = generic.as_slice() else {
                        panic!("one id type actual")
                    };
                    assert_eq!(
                        original.types.get(*actual).unwrap().modes.uniqueness,
                        Uniqueness::Aliased
                    );
                    assert_eq!(arguments.len(), 1);
                    assert_eq!(arguments[0].value.usage, UseKind::Take);
                    let argument = &body.places[arguments[0].value.place.0 as usize];
                    let mode = original.types.get(argument.ty).unwrap().modes.uniqueness;
                    assert_eq!(
                        mode,
                        original
                            .types
                            .get(body.locals[argument.root.0 as usize].ty)
                            .unwrap()
                            .modes
                            .uniqueness
                    );
                    inputs.push(mode);
                    let returned = &body.places[returns.as_ref().unwrap().destination.0 as usize];
                    assert_eq!(
                        original.types.get(returned.ty).unwrap().modes.uniqueness,
                        Uniqueness::Aliased
                    );
                }
            }
        }
        assert_eq!(inputs.len(), 2);
        assert_eq!(
            inputs
                .iter()
                .filter(|&&mode| mode == Uniqueness::Aliased)
                .count(),
            1
        );
        assert_eq!(
            inputs
                .iter()
                .filter(|&&mode| mode == Uniqueness::Unique)
                .count(),
            1
        );
    });
}

#[test]
fn port_context_declared_function_values_keep_distinct_actuals_and_reject_profile_collapse() {
    with_compiler_stack(|| {
        let (_, validated) = checked_source(DECLARED_SOURCE);
        let original = validated.snapshot();
        let (path, definition) = original
            .definitions
            .iter()
            .find(|(path, _)| path.0.name == "id")
            .unwrap();
        let signature = &original.signatures[path];
        let body = &definition.bodies[definition.entry.0 as usize];
        let selections = original
            .definitions
            .values()
            .flat_map(|definition| &definition.bodies)
            .flat_map(|body| {
                body.blocks.iter().flat_map(move |block| {
                    block
                        .instructions
                        .iter()
                        .map(move |instruction| (body, instruction))
                })
            })
            .filter_map(|(body, instruction)| {
                let InstructionKind::Assign { destination, value } = &instruction.kind else {
                    return None;
                };
                let Rvalue::FunctionReference {
                    definition,
                    arguments,
                } = &body.rvalues[value.0 as usize]
                else {
                    return None;
                };
                if definition != path {
                    return None;
                }
                let receiving = body.places[destination.0 as usize].ty;
                let row = original.types.get(receiving).unwrap();
                let TypeKind::Function(function) = &row.kind else {
                    panic!("original receiving callable type")
                };
                assert_eq!(function.parameters.len(), 1);
                let [GenericArgument::Type(actual)] = arguments.as_slice() else {
                    panic!("one original declared actual")
                };
                assert_eq!(
                    original.types.get(*actual).unwrap().modes.uniqueness,
                    original
                        .types
                        .get(function.parameters[0].ty)
                        .unwrap()
                        .modes
                        .uniqueness
                );
                assert_eq!(function.parameters[0].ty, function.result);
                Some(arguments.clone())
            })
            .collect::<Vec<_>>();
        assert_eq!(
            selections.len(),
            2,
            "the two original named function-value producers are selected"
        );
        // This test's source-data identity is not a query V receipt. Actual V
        // above revalidated the snapshot; no SourceDeclarations/provider Some
        // or reusable authority is manufactured from these bytes.
        let data_identity = ContentHash::of_parts(
            "hirsel.test.original-receipt-data.v1",
            [DECLARED_SOURCE.as_bytes()],
        );
        let source = SourceInput::capture(
            data_identity,
            original.source_owners[path].implementation_revision,
            definition.clone(),
        );
        let mut selected = BTreeMap::new();
        for arguments in selections {
            let [GenericArgument::Type(actual)] = arguments.as_slice() else {
                panic!("id has one original type actual")
            };
            let unique =
                original.types.get(*actual).unwrap().modes.uniqueness == Uniqueness::Unique;
            let actuals = Actuals {
                binder: Some(body.parameter_scope),
                arguments,
                outer: vec![],
            };
            let base = key_of(
                &original.types,
                path,
                definition.entry,
                &actuals,
                None,
                None,
            )
            .unwrap();
            let entry = source
                .named_entry_selected(
                    &base,
                    &original.types,
                    signature,
                    &actuals,
                    &TypeMemo::new(),
                )
                .unwrap();
            let projection = source.demand_projection(&base).unwrap();
            assert_eq!(entry.parameters().len(), 1);
            assert_eq!(&entry.parameters()[0].ty, entry.result());
            let profile = ClosedStorageProfile {
                schema: ClosedStorageProfile::SCHEMA,
                root: entry.parameters()[0].ty.clone(),
                functions: vec![],
            }
            .canonical()
            .unwrap();
            let profiles = BTreeMap::from([
                (Port::Parameter(0), profile.clone()),
                (Port::Result, profile),
            ]);
            assert!(
                selected
                    .insert(unique, (base, entry, projection, profiles))
                    .is_none()
            );
        }
        assert_eq!(selected.len(), 2);
        let a = &selected[&false];
        let b = &selected[&true];
        assert_eq!(a.0, b.0, "ordinary native base erases uniqueness");
        assert_eq!(a.1.parameters()[0].ty.ty, b.1.parameters()[0].ty.ty);
        assert_ne!(
            a.1.parameters()[0].ty.source_type,
            b.1.parameters()[0].ty.source_type
        );
        assert_ne!(
            a.3[&Port::Parameter(0)].content().unwrap(),
            b.3[&Port::Parameter(0)].content().unwrap()
        );
        let mut registry = Registry::default();
        let mut contexts = BTreeMap::new();
        for (&unique, (base, entry, source, ports)) in &selected {
            let context = || Context {
                base,
                profile: &ports[&Port::Parameter(0)],
                entry,
                source,
                ports,
            };
            let id = registry.register(context()).unwrap();
            assert_eq!(id, registry.register(context()).unwrap());
            registry
                .resolve(id, BTreeMap::new(), BTreeMap::new())
                .unwrap();
            contexts.insert(unique, id);
        }
        assert_eq!(registry.len(), 2);
        let requests = registry.solve().unwrap();
        let ordinary = &requests[contexts[&false].index()];
        let unique = &requests[contexts[&true].index()];
        assert_eq!(ordinary.ordinary_key(), unique.ordinary_key());
        assert_ne!(ordinary.context_key(), unique.context_key());
        assert!(requests.iter().all(|r| r.aware_key().is_none()));
        // The original typed function contracts select the complete contexts;
        // an equal erased native base cannot flatten the unique context.
        assert_eq!(
            registry
                .register(Context {
                    base: &b.0,
                    profile: &a.3[&Port::Parameter(0)],
                    entry: &b.1,
                    source: &b.2,
                    ports: &b.3,
                })
                .unwrap_err(),
            SourceError::Malformed("conflicting original data for receipt profile context")
        );
        let calls = BTreeMap::new();
        let closures = BTreeMap::new();
        let inputs = selected
            .values()
            .map(|(_, entry, source, profiles)| Input {
                entry,
                source,
                profiles,
                calls: &calls,
                closures: &closures,
            })
            .collect::<Vec<_>>();
        let declarations = contract::derive(&inputs).unwrap();
        assert_eq!(declarations.len(), 2);
        // This no-seal program has no receipt demand. These are two distinct
        // source/profile accounts, not two activated aware native entries.
        assert!(declarations.iter().all(|d| d.contract().is_none()));
        drop(inputs);
        let collapsed = selected[&false].3[&Port::Parameter(0)].clone();
        selected
            .get_mut(&true)
            .unwrap()
            .3
            .insert(Port::Parameter(0), collapsed);
        let inputs = selected
            .values()
            .map(|(_, entry, source, profiles)| Input {
                entry,
                source,
                profiles,
                calls: &calls,
                closures: &closures,
            })
            .collect::<Vec<_>>();
        assert_eq!(
            contract::derive(&inputs).unwrap_err(),
            SourceError::Malformed("receipt profile differs from full original source type")
        );
    });
}

fn agree_source(source: &'static str, scratch_name: &'static str) {
    with_compiler_stack(move || {
        let (mut engine, validated) = checked_source(source);
        let entry = entry_path(&default_context());
        let execution = run_observed_snapshot(
            &mut engine,
            &RunConfiguration {
                entry: entry.clone(),
                hash_seed: 0,
                inputs: vec![],
            },
            SimulationConfig::default(),
        )
        .unwrap();
        let reference = Observation::of_reference(execution.observation.as_ref().unwrap());
        assert_eq!(reference.exit_code, Some(0), "{}", reference.render());
        assert!(reference.failures.is_empty());
        let mut options = Options::new(TargetSpec::x86_64_unknown_linux_gnu());
        options.validation = native::Validation::EveryStage;
        let configurations = [
            Engine::Heap { campaign: false },
            Engine::Native {
                opt: OptLevel::Default,
                partition: Partition::Whole,
                runtime: Runtime::Diagnostic,
            },
            Engine::Cranelift {
                partition: Partition::Whole,
                runtime: Runtime::Diagnostic,
            },
        ]
        .into_iter()
        .map(|engine| Configuration {
            options: options.clone(),
            engine,
        })
        .collect::<Vec<_>>();
        let root = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(scratch_name);
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let toolchain = Toolchain::for_matrix(&root.join("runtime"), &configurations).unwrap();
        let outcomes = native::differential(
            &validated,
            &entry,
            &reference,
            &configurations,
            &toolchain,
            &root,
        );
        assert_eq!(outcomes.len(), 3);
        for (configuration, outcome) in outcomes {
            eprintln!("original generic identity: {configuration}: {outcome:?}");
            assert!(
                matches!(outcome, Outcome::Agree),
                "{configuration}: {outcome:?}"
            );
        }
    });
}

#[test]
fn ordinary_generic_identity_agrees_in_reference_heap_and_both_native_backends() {
    agree_source(SOURCE, "receipt-original-actuals-33");
}

#[test]
fn port_context_declared_values_agree_in_reference_heap_and_both_native_backends() {
    agree_source(DECLARED_SOURCE, "receipt-declared-actuals-33");
}

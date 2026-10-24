//! Independent executable storage reconstruction and permanent profile forgeries.
use hirsel_driver::*;
use hirsel_engine::*;
use hirsel_types::storage::{StorageBehavior, StorageProfile, StorageProfileError};
use hirsel_validate::storage::{StorageValidation, StorageViolation};
use std::sync::{Mutex, OnceLock};

fn fixture() -> &'static NativeInput {
    &fixture_state().input
}
struct Fixture {
    input: NativeInput,
    engine: Mutex<QueryEngine>,
}
fn fixture_state() -> &'static Fixture {
    static INPUT: OnceLock<Fixture> = OnceLock::new();
    INPUT.get_or_init(|| with_compiler_stack(|| {
        let source = "fn clone_heap(xs:List[u64])->unique List[u64]{unique xs}\npub fn main(){let mut r=arena.with(fn(a:Arena)->Region[List[u64]]{let root:List[u64] in a=[7u64];arena.seal(take a,take root)});let independent=region.with(mut r,fn(root:List[u64])->unique List[u64]{let fresh:List[u64] in heap=[8u64];clone_heap(fresh)});}";
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
        let file = engine.sources.add("storage-profile-v.hir", source.into()).unwrap();
        let output = check_progress(&mut engine, file, &default_context()).unwrap();
        assert_eq!(output.completion, CheckCompletion::Complete);
        assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
        let input = engine.export_native_input().unwrap();
        Fixture { input, engine: Mutex::new(engine) }
    }))
}

#[test]
fn sealed_storage_provenance_33_import_control() {
    let fixture = fixture_state();
    let mut engine = fixture.engine.lock().unwrap();
    let validated = engine.import_native_input(fixture.input.clone()).unwrap();
    assert!(!validated.storage_proof_reads().bodies.is_empty());
}
#[test]
fn sealed_storage_provenance_33_import_missing_table() {
    let fixture = fixture_state();
    let mut engine = fixture.engine.lock().unwrap();
    let mut input = fixture.input.clone();
    input.storage_profiles.clear();
    let error = engine
        .import_native_input(input)
        .err()
        .expect("mandatory profile proof");
    assert!(format!("{error:?}").contains("MissingProfile"), "{error:?}");
}
fn validate(input: &NativeInput) -> StorageValidation {
    let mut signatures = hirsel_validate::SignatureMap::new(input.types.clone());
    signatures.extend_signatures(
        input
            .signatures
            .values()
            .map(|signature| (**signature).clone()),
    );
    hirsel_validate::storage::validate_program(
        &input.types,
        &input.definitions.values().cloned().collect::<Vec<_>>(),
        &signatures,
        &input.constants,
        &input.instance_bodies,
        &input.storage_profiles,
    )
}
fn helper(input: &mut NativeInput) -> &mut StorageProfile {
    &mut input
        .storage_profiles
        .iter_mut()
        .find(|profile| profile.callable.definition.path().0.name == "clone_heap")
        .expect("demanded helper profile")
        .profile
}
fn profile_error(validation: &StorageValidation, expected: impl Fn(&StorageProfileError) -> bool) {
    assert!(validation.violations.iter().any(|violation| matches!(violation, StorageViolation::Profile { error, .. } if expected(error))), "{validation:?}");
}

#[test]
fn sealed_storage_profile_v_33_executable_control() {
    let input = fixture();
    let validation = validate(input);
    assert!(validation.violations.is_empty(), "{validation:?}");
    assert_eq!(validation.profiles, input.storage_profiles);
    assert!(!validation.reads.bodies.is_empty());
    assert!(!validation.reads.profiles.is_empty());
}
#[test]
fn sealed_storage_profile_v_33_forged_empty() {
    let mut input = fixture().clone();
    let profile = helper(&mut input);
    *profile = StorageProfile::new(profile.source_type, StorageBehavior::empty()).unwrap();
    profile_error(&validate(&input), |error| {
        matches!(error, StorageProfileError::MissingAllocation(_))
    });
}
#[test]
fn sealed_storage_profile_v_33_missing_result_selector() {
    let mut input = fixture().clone();
    let profile = helper(&mut input);
    let StorageBehavior::Known {
        allocations,
        results,
    } = &profile.behavior
    else {
        panic!("known helper")
    };
    assert!(!results.is_empty());
    *profile = StorageProfile::new(
        profile.source_type,
        StorageBehavior::Known {
            allocations: allocations.clone(),
            results: Default::default(),
        },
    )
    .unwrap();
    profile_error(&validate(&input), |error| {
        matches!(error, StorageProfileError::MissingResult(_))
    });
}
#[test]
fn sealed_storage_profile_v_33_stale_digest() {
    let mut input = fixture().clone();
    helper(&mut input).behavior = StorageBehavior::empty();
    profile_error(&validate(&input), |error| {
        *error == StorageProfileError::StaleDigest
    });
}
#[test]
fn sealed_storage_profile_v_33_missing_table_cannot_disable_proof() {
    let mut input = fixture().clone();
    input.storage_profiles.clear();
    let validation = validate(&input);
    assert!(
        validation
            .violations
            .iter()
            .any(|violation| matches!(violation, StorageViolation::MissingProfile(_))),
        "{validation:?}"
    );
}

fn export_and_import(source: &str) {
    with_compiler_stack(|| {
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
        let file = engine
            .sources
            .add("storage-context-v.hir", source.into())
            .unwrap();
        let output = check_progress(&mut engine, file, &default_context()).unwrap();
        assert_eq!(output.completion, CheckCompletion::Complete);
        assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
        let input = engine.export_native_input().unwrap();
        let validation = validate(&input);
        assert!(validation.violations.is_empty(), "{validation:?}");
        assert_eq!(validation.profiles, input.storage_profiles);
        let validated = engine.import_native_input(input).unwrap();
        assert!(!validated.storage_proof_reads().bodies.is_empty());
    });
}

#[test]
fn sealed_storage_context_v_33_selected_callable_argument() {
    export_and_import(
        "fn length(xs:List[u64])->u64{list.len(xs)}\nfn apply[T](f:fn(T)->u64,x:T)->u64{f(x)}\npub fn main(){let mut r=arena.with(fn(a:Arena)->Region[List[u64]]{let root:List[u64] in a=[7u64];arena.seal(take a,take root)});let n=region.with(mut r,fn(root:List[u64])->u64{apply(length,root)});}",
    );
}

#[test]
fn sealed_storage_context_v_33_sealed_callable_root() {
    export_and_import(
        "fn held(take payload:Bytes)->fn()->u64{fn()->u64{bytes.len(payload)}}\nfn fits(cap:u64)->u64{let outcome=arena.bounded(cap,fn(a:Arena)->Region[fn()->u64]{let payload:Bytes in heap=copy_to(heap,b\"abc\");let root=held(take payload);arena.seal(take a,take root)});match take outcome{Ok(r)=>{let mut owned=take r;region.with(mut owned,fn(view:fn()->u64)->u64{view()})},Error(e)=>0u64,}}\npub fn main(t:Test)->Result[(),()]{failure.check(fits(10u64)==0u64,Bounds());failure.check(fits(11u64)==3u64,Bounds());Ok(())}",
    );
}

fn captured_copy_fixture() -> &'static NativeInput {
    &captured_copy_fixture_state().input
}
fn captured_copy_fixture_state() -> &'static Fixture {
    static INPUT: OnceLock<Fixture> = OnceLock::new();
    INPUT.get_or_init(|| with_compiler_stack(|| {
        let source = "fn held(take payload:Bytes)->fn()->unique Bytes{fn()->unique Bytes{let detached:Bytes in heap=copy_to(heap,payload);unique detached}}\npub fn main(){let mut r=arena.with(fn(a:Arena)->Region[fn()->unique Bytes]{let payload:Bytes in heap=copy_to(heap,b\"abc\");let root=held(take payload);arena.seal(take a,take root)});let independent=region.with(mut r,fn(view:fn()->unique Bytes)->unique Bytes{view()});}";
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
        let file = engine.sources.add("storage-root-capture-v.hir", source.into()).unwrap();
        let output = check_progress(&mut engine, file, &default_context()).unwrap();
        assert_eq!(output.completion, CheckCompletion::Complete);
        assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
        let input = engine.export_native_input().unwrap();
        Fixture { input, engine: Mutex::new(engine) }
    }))
}

#[test]
fn sealed_storage_root_v_33_explicit_heap_capture_copy_control() {
    let input = captured_copy_fixture();
    let validation = validate(input);
    assert!(validation.violations.is_empty(), "{validation:?}");
    assert_eq!(validation.profiles, input.storage_profiles);
}

#[test]
fn sealed_storage_root_v_33_explicit_heap_capture_copy_import() {
    let fixture = captured_copy_fixture_state();
    let mut engine = fixture.engine.lock().unwrap();
    let validated = engine.import_native_input(fixture.input.clone()).unwrap();
    assert!(!validated.storage_proof_reads().bodies.is_empty());
}

#[test]
fn sealed_storage_root_v_33_forged_capture_allocation() {
    let mut input = captured_copy_fixture().clone();
    let definition = input
        .definitions
        .iter_mut()
        .find(|(path, _)| path.0.name == "held")
        .unwrap()
        .1;
    let definition = std::sync::Arc::make_mut(definition);
    let mut mutations = 0;
    for body in &mut definition.bodies {
        let Some(capture) = body.places.iter().position(|place| {
            place.projection.is_empty()
                && body.locals[place.root.0 as usize].storage == hirsel_tir::Storage::Capture
        }) else {
            continue;
        };
        for rvalue in &mut body.rvalues {
            if let hirsel_tir::Rvalue::Unary {
                op: hirsel_syntax::UnaryOp::Unique,
                value,
            } = rvalue
            {
                // Keep the code producer, destination, capture inventory and
                // claimed empty profiles; change only the real source operand.
                value.place = hirsel_tir::PlaceId(capture as u32);
                mutations += 1;
            }
        }
    }
    assert_eq!(mutations, 1);
    let validation = validate(&input);
    assert!(validation.violations.iter().any(|violation| matches!(
        violation,
        StorageViolation::ExecutableRead(hirsel_type_analysis::storage::program::StorageReadViolation {
            failure: hirsel_type_analysis::storage::program::StorageReadFailure::AllocatesFromRoot(_), ..
        })
    )), "{validation:?}");
}

fn destination_fixture_state() -> &'static Fixture {
    static INPUT: OnceLock<Fixture> = OnceLock::new();
    INPUT.get_or_init(|| with_compiler_stack(|| {
        let source = "pub fn main(){let mut r=arena.with(fn(a:Arena)->Region[List[u64]]{let root:List[u64] in a=[7u64];arena.seal(take a,take root)});let independent=region.with(mut r,fn(root:List[u64])->unique List[u64]{let fresh:List[u64] in heap=[list.len(root)];unique fresh});}";
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
        let file = engine.sources.add("storage-destination-v.hir", source.into()).unwrap();
        let output = check_progress(&mut engine, file, &default_context()).unwrap();
        assert_eq!(output.completion, CheckCompletion::Complete);
        assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
        let input = engine.export_native_input().unwrap();
        Fixture { input, engine: Mutex::new(engine) }
    }))
}

#[test]
fn sealed_storage_destination_33_missing_actual_operand_control() {
    let fixture = destination_fixture_state();
    let control = validate(&fixture.input);
    assert!(control.violations.is_empty(), "{control:?}");
    assert_eq!(control.profiles, fixture.input.storage_profiles);
    fixture
        .engine
        .lock()
        .unwrap()
        .import_native_input(fixture.input.clone())
        .unwrap();
}

#[test]
fn sealed_storage_destination_33_missing_actual_operand_refuses() {
    let mut input = destination_fixture_state().input.clone();
    let mut mutations = 0;
    for definition in input.definitions.values_mut() {
        for body in &mut std::sync::Arc::make_mut(definition).bodies {
            if !body.parameters.first().is_some_and(|parameter| {
                matches!(
                    input.types.row(body.locals[parameter.0 as usize].ty).kind,
                    hirsel_types::TypeKind::Builtin {
                        kind: hirsel_types::BuiltinType::List,
                        ..
                    }
                )
            }) {
                continue;
            }
            for rvalue in &mut body.rvalues {
                if let hirsel_tir::Rvalue::Construct {
                    constructor: hirsel_tir::Constructor::Sequence,
                    destination,
                    ..
                } = rvalue
                {
                    *destination = hirsel_types::Destination::Caller(
                        vec![hirsel_types::ParameterRef::bound(hirsel_types::ParamId(99))].into(),
                    );
                    mutations += 1;
                }
            }
        }
    }
    assert_eq!(mutations, 1);
    let validation = validate(&input);
    assert!(
        validation.violations.iter().any(|violation| matches!(
            violation,
            StorageViolation::ExecutableRead(
                hirsel_type_analysis::storage::program::StorageReadViolation {
                    failure:
                        hirsel_type_analysis::storage::program::StorageReadFailure::UnknownCallback,
                    ..
                }
            )
        )),
        "{validation:?}"
    );
}

fn nested_copy_fixture_state() -> &'static Fixture {
    static INPUT: OnceLock<Fixture> = OnceLock::new();
    INPUT.get_or_init(|| with_compiler_stack(|| {
        let source = "fn leaf(take payload:Bytes)->fn()->unique Bytes{fn()->unique Bytes{let fresh:Bytes in heap=copy_to(heap,payload);unique fresh}}\nfn wrap(take f:fn()->unique Bytes)->fn()->unique Bytes{fn()->unique Bytes{f()}}\nfn held(take payload:Bytes)->fn()->unique Bytes{let f=leaf(take payload);let middle=wrap(take f);wrap(take middle)}\npub fn main(){let mut r=arena.with(fn(a:Arena)->Region[fn()->unique Bytes]{let payload:Bytes in heap=copy_to(heap,b\"abc\");let root=held(take payload);arena.seal(take a,take root)});let independent=region.with(mut r,fn(view:fn()->unique Bytes)->unique Bytes{view()});}";
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
        let file = engine.sources.add("storage-nested-capture-v.hir", source.into()).unwrap();
        let output = check_progress(&mut engine, file, &default_context()).unwrap();
        assert_eq!(output.completion, CheckCompletion::Complete);
        assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
        let input = engine.export_native_input().unwrap();
        Fixture { input, engine: Mutex::new(engine) }
    }))
}

#[test]
fn sealed_storage_nested_v_33_heap_copy_control() {
    let fixture = nested_copy_fixture_state();
    let validation = validate(&fixture.input);
    assert!(validation.violations.is_empty(), "{validation:?}");
    assert_eq!(validation.profiles, fixture.input.storage_profiles);
    fixture
        .engine
        .lock()
        .unwrap()
        .import_native_input(fixture.input.clone())
        .unwrap();
}

#[test]
fn sealed_storage_nested_v_33_forged_leaf_capture_allocation() {
    let mut input = nested_copy_fixture_state().input.clone();
    let definition = input
        .definitions
        .iter_mut()
        .find(|(path, _)| path.0.name == "leaf")
        .unwrap()
        .1;
    let mut mutations = 0;
    for body in &mut std::sync::Arc::make_mut(definition).bodies {
        let Some(capture) = body.places.iter().position(|place| {
            place.projection.is_empty()
                && body.locals[place.root.0 as usize].storage == hirsel_tir::Storage::Capture
        }) else {
            continue;
        };
        for rvalue in &mut body.rvalues {
            if let hirsel_tir::Rvalue::Unary {
                op: hirsel_syntax::UnaryOp::Unique,
                value,
            } = rvalue
            {
                value.place = hirsel_tir::PlaceId(capture as u32);
                mutations += 1;
            }
        }
    }
    assert_eq!(mutations, 1);
    let validation = validate(&input);
    assert!(validation.violations.iter().any(|violation| matches!(
        violation,
        StorageViolation::ExecutableRead(hirsel_type_analysis::storage::program::StorageReadViolation {
            failure: hirsel_type_analysis::storage::program::StorageReadFailure::AllocatesFromRoot(_), ..
        })
    )), "{validation:?}");
}

fn shared_callback_fixture_state() -> &'static Fixture {
    static INPUT: OnceLock<Fixture> = OnceLock::new();
    INPUT.get_or_init(|| with_compiler_stack(|| {
        let source = "pub fn main(){let mut r=arena.with(fn(a:Arena)->Region[List[u64]]{let root:List[u64] in a=[7u64];arena.seal(take a,take root)});let n=region.with(mut r,fn(root:List[u64])->u64{let seed:unique Bytes=unique b\"x\";let s=shared.freeze(take seed);shared.with(s,fn(value:Bytes)->u64{let fresh:List[u64] in heap=[bytes.len(value)];let cloned=unique fresh;list.len(cloned)+list.len(root)})});}";
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
        let file = engine.sources.add("storage-shared-callback-v.hir", source.into()).unwrap();
        let output = check_progress(&mut engine, file, &default_context()).unwrap();
        assert_eq!(output.completion, CheckCompletion::Complete);
        assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
        let input = engine.export_native_input().unwrap();
        Fixture { input, engine: Mutex::new(engine) }
    }))
}

#[test]
fn sealed_storage_core_callback_33_executable_v_import_control() {
    let fixture = shared_callback_fixture_state();
    let validation = validate(&fixture.input);
    assert!(validation.violations.is_empty(), "{validation:?}");
    assert_eq!(validation.profiles, fixture.input.storage_profiles);
    fixture
        .engine
        .lock()
        .unwrap()
        .import_native_input(fixture.input.clone())
        .unwrap();
}

fn shared_allocation_fixture_state() -> &'static Fixture {
    static INPUT: OnceLock<Fixture> = OnceLock::new();
    INPUT.get_or_init(|| with_compiler_stack(|| {
        let source = "fn clone_heap(xs:List[u64])->unique List[u64]{unique xs}\npub fn main(){let mut r=arena.with(fn(a:Arena)->Region[List[u64]]{let root:List[u64] in a=[7u64];arena.seal(take a,take root)});let n=region.with(mut r,fn(root:List[u64])->u64{let seed:unique Bytes=unique b\"x\";let s=shared.freeze(take seed);shared.with(s,fn(value:Bytes)->u64{let fresh:List[u64] in heap=[bytes.len(value)];let cloned=clone_heap(fresh);list.len(cloned)+list.len(root)})});}";
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
        let file = engine.sources.add("storage-shared-allocation-v.hir", source.into()).unwrap();
        let output = check_progress(&mut engine, file, &default_context()).unwrap();
        assert_eq!(output.completion, CheckCompletion::Complete);
        assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
        let input = engine.export_native_input().unwrap();
        Fixture { input, engine: Mutex::new(engine) }
    }))
}

#[test]
fn sealed_storage_core_callback_33_v_forged_captured_allocation_control() {
    let fixture = shared_allocation_fixture_state();
    let validation = validate(&fixture.input);
    assert!(validation.violations.is_empty(), "{validation:?}");
    assert_eq!(validation.profiles, fixture.input.storage_profiles);
    fixture
        .engine
        .lock()
        .unwrap()
        .import_native_input(fixture.input.clone())
        .unwrap();
}

#[test]
fn sealed_storage_core_callback_33_v_forged_captured_allocation() {
    let mut input = shared_allocation_fixture_state().input.clone();
    let mut mutations = 0;
    for definition in input.definitions.values_mut() {
        for body in &mut std::sync::Arc::make_mut(definition).bodies {
            let Some(capture) = body.places.iter().position(|place| {
                place.projection.is_empty()
                    && body.locals[place.root.0 as usize].storage == hirsel_tir::Storage::Capture
                    && matches!(
                        input.types.row(place.ty).kind,
                        hirsel_types::TypeKind::Builtin {
                            kind: hirsel_types::BuiltinType::List,
                            ..
                        }
                    )
            }) else {
                continue;
            };
            for block in &mut body.blocks {
                if let hirsel_tir::Terminator::Call {
                    callee: hirsel_tir::Callee::Definition { path, .. },
                    arguments,
                    ..
                } = &mut block.terminator
                {
                    if path.0.name == "clone_heap" {
                        assert_eq!(arguments.len(), 1);
                        arguments[0].value.place = hirsel_tir::PlaceId(capture as u32);
                        mutations += 1;
                    }
                }
            }
        }
    }
    assert_eq!(mutations, 1);
    let validation = validate(&input);
    assert!(validation.violations.iter().any(|violation| matches!(
        violation,
        StorageViolation::ExecutableRead(hirsel_type_analysis::storage::program::StorageReadViolation {
            failure: hirsel_type_analysis::storage::program::StorageReadFailure::AllocatesFromRoot(_), ..
        })
    )), "{validation:?}");
}

#[test]
fn sealed_storage_core_callback_33_v_forged_core_contract() {
    let mut input = shared_callback_fixture_state().input.clone();
    let signature = input
        .signatures
        .iter_mut()
        .find(|(path, _)| path.is_core("shared.with"))
        .unwrap()
        .1;
    let hirsel_types::SignatureKind::Function(function) =
        &mut std::sync::Arc::make_mut(signature).kind
    else {
        panic!("function")
    };
    function.parameters[1].passing = hirsel_syntax::PassingMode::Take;
    let validation = validate(&input);
    assert!(
        validation.violations.iter().any(|violation| matches!(
            violation,
            StorageViolation::ExecutableRead(
                hirsel_type_analysis::storage::program::StorageReadViolation {
                    failure:
                        hirsel_type_analysis::storage::program::StorageReadFailure::UnknownCallback,
                    ..
                }
            )
        )),
        "{validation:?}"
    );
}

fn copied_branch_fixture_state() -> &'static Fixture {
    static INPUT: OnceLock<Fixture> = OnceLock::new();
    INPUT.get_or_init(|| with_compiler_stack(|| {
        let source = "fn clone_heap(xs:Bytes)->unique Bytes{unique xs}\nfn leaf(take payload:Bytes)->fn()->u64{fn()->u64{let fresh:Bytes in heap=copy_to(heap,payload);let copied=clone_heap(fresh);bytes.len(copied)}}\nfn choose(flag:bool)->Region[fn()->u64]{arena.with(fn(a:Arena)->Region[fn()->u64]{let payload:Bytes in heap=copy_to(heap,b\"abc\");let f=leaf(take payload);let root:fn()->u64 in heap=if flag{copy_to(heap,f)}else{fn()->u64{0u64}};arena.seal(take a,take root)})}\npub fn main(){let mut r=choose(true);let n=region.with(mut r,fn(view:fn()->u64)->u64{view()});}";
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
        let file = engine.sources.add("storage-copied-branch-v.hir", source.into()).unwrap();
        let output = check_progress(&mut engine, file, &default_context()).unwrap();
        assert_eq!(output.completion, CheckCompletion::Complete);
        assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
        let input = engine.export_native_input().unwrap();
        Fixture { input, engine: Mutex::new(engine) }
    }))
}

#[test]
fn sealed_storage_copied_v_33_executable_import_control() {
    let fixture = copied_branch_fixture_state();
    let validation = validate(&fixture.input);
    assert!(validation.violations.is_empty(), "{validation:?}");
    assert_eq!(validation.profiles, fixture.input.storage_profiles);
    fixture
        .engine
        .lock()
        .unwrap()
        .import_native_input(fixture.input.clone())
        .unwrap();
}

#[test]
fn sealed_storage_copied_v_33_allocating_alternative_refuses() {
    let mut input = copied_branch_fixture_state().input.clone();
    let definition = input
        .definitions
        .iter_mut()
        .find(|(path, _)| path.0.name == "leaf")
        .unwrap()
        .1;
    let mut mutations = 0;
    for body in &mut std::sync::Arc::make_mut(definition).bodies {
        let Some(capture) = body.places.iter().position(|place| {
            place.projection.is_empty()
                && body.locals[place.root.0 as usize].storage == hirsel_tir::Storage::Capture
        }) else {
            continue;
        };
        for block in &mut body.blocks {
            if let hirsel_tir::Terminator::Call {
                callee: hirsel_tir::Callee::Definition { path, .. },
                arguments,
                ..
            } = &mut block.terminator
            {
                if path.0.name == "clone_heap" {
                    assert_eq!(arguments.len(), 1);
                    arguments[0].value.place = hirsel_tir::PlaceId(capture as u32);
                    mutations += 1;
                }
            }
        }
    }
    assert_eq!(mutations, 1);
    let validation = validate(&input);
    assert!(validation.violations.iter().any(|violation| matches!(
        violation,
        StorageViolation::ExecutableRead(hirsel_type_analysis::storage::program::StorageReadViolation {
            failure: hirsel_type_analysis::storage::program::StorageReadFailure::AllocatesFromRoot(_), ..
        })
    )), "{validation:?}");
}

#[test]
fn sealed_storage_copied_v_33_missing_body_cannot_borrow_inert_alternative() {
    let mut input = copied_branch_fixture_state().input.clone();
    let before = input.definitions.len();
    input.definitions.retain(|path, _| path.0.name != "leaf");
    assert_eq!(input.definitions.len() + 1, before);
    let validation = validate(&input);
    assert!(
        validation.violations.iter().any(|violation| matches!(
            violation,
            StorageViolation::ExecutableRead(
                hirsel_type_analysis::storage::program::StorageReadViolation {
                    failure:
                        hirsel_type_analysis::storage::program::StorageReadFailure::UnknownCallback,
                    ..
                }
            )
        )),
        "{validation:?}"
    );
}

#[test]
fn sealed_storage_copied_v_33_forged_copy_contract_refuses() {
    let mut input = copied_branch_fixture_state().input.clone();
    let signature = input
        .signatures
        .iter_mut()
        .find(|(path, _)| path.is_core("copy_to"))
        .unwrap()
        .1;
    let hirsel_types::SignatureKind::Function(function) =
        &mut std::sync::Arc::make_mut(signature).kind
    else {
        panic!("function")
    };
    function.parameters[1].passing = hirsel_syntax::PassingMode::Take;
    let validation = validate(&input);
    assert!(
        validation.violations.iter().any(|violation| matches!(
            violation,
            StorageViolation::ExecutableRead(
                hirsel_type_analysis::storage::program::StorageReadViolation {
                    failure:
                        hirsel_type_analysis::storage::program::StorageReadFailure::UnknownCallback,
                    ..
                }
            )
        )),
        "{validation:?}"
    );
}

fn map_helper_fixture_state() -> &'static Fixture {
    static INPUT: OnceLock<Fixture> = OnceLock::new();
    INPUT.get_or_init(|| with_compiler_stack(|| {
        let source = format!("fn clone_heap(xs:Bytes)->unique Bytes{{unique xs}}\n{}", super::sealed_storage_placement_33::helper_map_callable_branch("fn leaf(take payload:Bytes)->fn()->u64{fn()->u64{let fresh:Bytes in heap=copy_to(heap,payload);let copied=clone_heap(fresh);bytes.len(copied)}}", true));
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
        let file = engine.sources.add("storage-map-helper-v.hir", source.into()).unwrap();
        let output = check_progress(&mut engine, file, &default_context()).unwrap();
        assert_eq!(output.completion, CheckCompletion::Complete);
        assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
        let input = engine.export_native_input().unwrap();
        Fixture { input, engine: Mutex::new(engine) }
    }))
}
#[test]
fn sealed_storage_map_v_33_executable_import_control() {
    let fixture = map_helper_fixture_state();
    let validation = validate(&fixture.input);
    assert!(validation.violations.is_empty(), "{validation:?}");
    assert_eq!(validation.profiles, fixture.input.storage_profiles);
    fixture
        .engine
        .lock()
        .unwrap()
        .import_native_input(fixture.input.clone())
        .unwrap();
}
#[test]
fn sealed_storage_map_v_33_allocating_helper_alternative() {
    let mut input = map_helper_fixture_state().input.clone();
    let definition = input
        .definitions
        .iter_mut()
        .find(|(path, _)| path.0.name == "leaf")
        .unwrap()
        .1;
    let mut mutations = 0;
    for body in &mut std::sync::Arc::make_mut(definition).bodies {
        let Some(capture) = body.places.iter().position(|place| {
            place.projection.is_empty()
                && body.locals[place.root.0 as usize].storage == hirsel_tir::Storage::Capture
        }) else {
            continue;
        };
        for block in &mut body.blocks {
            if let hirsel_tir::Terminator::Call {
                callee: hirsel_tir::Callee::Definition { path, .. },
                arguments,
                ..
            } = &mut block.terminator
            {
                if path.0.name == "clone_heap" {
                    assert_eq!(arguments.len(), 1);
                    arguments[0].value.place = hirsel_tir::PlaceId(capture as u32);
                    mutations += 1;
                }
            }
        }
    }
    assert_eq!(mutations, 1);
    let validation = validate(&input);
    assert!(validation.violations.iter().any(|violation|matches!(violation,StorageViolation::ExecutableRead(hirsel_type_analysis::storage::program::StorageReadViolation {failure:hirsel_type_analysis::storage::program::StorageReadFailure::AllocatesFromRoot(_),..}))),"{validation:?}");
}
fn map_unknown(validation: StorageValidation) {
    assert!(
        validation.violations.iter().any(|violation| matches!(
            violation,
            StorageViolation::ExecutableRead(
                hirsel_type_analysis::storage::program::StorageReadViolation {
                    failure:
                        hirsel_type_analysis::storage::program::StorageReadFailure::UnknownCallback,
                    ..
                }
            )
        )),
        "{validation:?}"
    );
}
#[test]
fn sealed_storage_map_v_33_missing_mutating_body() {
    let mut input = map_helper_fixture_state().input.clone();
    let before = input.definitions.len();
    input
        .definitions
        .retain(|path, _| path.0.name != "overwrite");
    assert_eq!(input.definitions.len() + 1, before);
    map_unknown(validate(&input));
}
#[test]
fn sealed_storage_map_v_33_missing_callable_body() {
    let mut input = map_helper_fixture_state().input.clone();
    let before = input.definitions.len();
    input.definitions.retain(|path, _| path.0.name != "leaf");
    assert_eq!(input.definitions.len() + 1, before);
    map_unknown(validate(&input));
}
#[test]
fn sealed_storage_map_v_33_forged_mutable_operand() {
    let mut input = map_helper_fixture_state().input.clone();
    let mut mutations = 0;
    for definition in input.definitions.values_mut() {
        for body in &mut std::sync::Arc::make_mut(definition).bodies {
            for block in &mut body.blocks {
                if let hirsel_tir::Terminator::Call {
                    callee: hirsel_tir::Callee::Definition { path, .. },
                    arguments,
                    ..
                } = &mut block.terminator
                {
                    if path.0.name == "overwrite" {
                        let argument = arguments
                            .iter_mut()
                            .find(|argument| argument.parameter.0 == 0)
                            .unwrap();
                        assert_eq!(argument.value.usage, hirsel_types::UseKind::Mut);
                        argument.value.usage = hirsel_types::UseKind::Borrow;
                        mutations += 1;
                    }
                }
            }
        }
    }
    assert_eq!(mutations, 1);
    let validation = validate(&input);
    assert!(
        validation.violations.contains(&StorageViolation::Malformed(
            "storage mutable operand modes"
        )),
        "{validation:?}"
    );
}
#[test]
fn sealed_storage_map_v_33_forged_get_contract() {
    let mut input = map_helper_fixture_state().input.clone();
    let signature = input
        .signatures
        .iter_mut()
        .find(|(path, _)| path.is_core("map.get"))
        .unwrap()
        .1;
    let hirsel_types::SignatureKind::Function(function) =
        &mut std::sync::Arc::make_mut(signature).kind
    else {
        panic!("function")
    };
    function.allocation = hirsel_types::Allocation::MayAllocate;
    map_unknown(validate(&input));
}

fn map_owned_capture_fixture_state() -> &'static Fixture {
    static INPUT: OnceLock<Fixture> = OnceLock::new();
    INPUT.get_or_init(||with_compiler_stack(||{
        let source=format!("fn clone_heap(xs:Bytes)->unique Bytes{{unique xs}}\n{}",super::sealed_storage_placement_33::owned_capture_map_callable_branch("fn leaf(take payload:Bytes)->fn()->u64{fn()->u64{let fresh:Bytes in heap=copy_to(heap,payload);let copied=clone_heap(fresh);bytes.len(copied)}}"));
        let mut engine=QueryEngine::new(default_environment(),compiler_adapters());
        let file=engine.sources.add("storage-map-owned-capture-v.hir",source.into()).unwrap();
        let output=check_progress(&mut engine,file,&default_context()).unwrap();
        assert_eq!(output.completion,CheckCompletion::Complete);assert!(output.diagnostics.is_empty(),"{:?}",output.diagnostics);
        let input=engine.export_native_input().unwrap();Fixture{input,engine:Mutex::new(engine)}
    }))
}
#[test]
fn sealed_storage_map_capture_v_33_import_control() {
    let fixture = map_owned_capture_fixture_state();
    let validation = validate(&fixture.input);
    assert!(validation.violations.is_empty(), "{validation:?}");
    assert_eq!(validation.profiles, fixture.input.storage_profiles);
    fixture
        .engine
        .lock()
        .unwrap()
        .import_native_input(fixture.input.clone())
        .unwrap();
}
#[test]
fn sealed_storage_map_capture_v_33_forged_captured_receiver() {
    let mut input = map_owned_capture_fixture_state().input.clone();
    let mut mutations = 0;
    for definition in input.definitions.values_mut() {
        for body in &mut std::sync::Arc::make_mut(definition).bodies {
            let Some(capture) = body.places.iter().position(|place| {
                place.projection.is_empty()
                    && body.locals[place.root.0 as usize].storage == hirsel_tir::Storage::Capture
                    && matches!(
                        input.types.row(place.ty).kind,
                        hirsel_types::TypeKind::Builtin {
                            kind: hirsel_types::BuiltinType::Map,
                            ..
                        }
                    )
            }) else {
                continue;
            };
            for block in &mut body.blocks {
                if let hirsel_tir::Terminator::Call {
                    callee: hirsel_tir::Callee::Definition { path, .. },
                    arguments,
                    ..
                } = &mut block.terminator
                {
                    if path.is_core("map.insert") {
                        arguments
                            .iter_mut()
                            .find(|argument| argument.parameter.0 == 0)
                            .unwrap()
                            .value
                            .place = hirsel_tir::PlaceId(capture as u32);
                        mutations += 1;
                    }
                }
            }
        }
    }
    assert_eq!(mutations, 1);
    let validation = validate(&input);
    assert!(
        validation.violations.contains(&StorageViolation::Malformed(
            "storage mutation of captured binding"
        )),
        "{validation:?}"
    );
}

fn map_nested_fixture_state() -> &'static Fixture {
    static INPUT: OnceLock<Fixture> = OnceLock::new();
    INPUT.get_or_init(|| {
        with_compiler_stack(|| {
            let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
            let source = super::sealed_storage_placement_33::nested_map_unknown_program(true);
            let file = engine
                .sources
                .add("storage-map-nested-v.hir", source.into())
                .unwrap();
            let output = check_progress(&mut engine, file, &default_context()).unwrap();
            assert_eq!(output.completion, CheckCompletion::Complete);
            assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
            let input = engine.export_native_input().unwrap();
            Fixture {
                input,
                engine: Mutex::new(engine),
            }
        })
    })
}
#[test]
fn sealed_storage_map_nested_v_33_control() {
    let fixture = map_nested_fixture_state();
    let validation = validate(&fixture.input);
    assert!(validation.violations.is_empty(), "{validation:?}");
    assert_eq!(validation.profiles, fixture.input.storage_profiles);
    fixture
        .engine
        .lock()
        .unwrap()
        .import_native_input(fixture.input.clone())
        .unwrap();
}
#[test]
fn sealed_storage_map_nested_v_33_missing_actual_caller_cannot_borrow_inert_code() {
    let mut input = map_nested_fixture_state().input.clone();
    let before = input.definitions.len();
    input.definitions.retain(|path, _| path.0.name != "main");
    assert_eq!(input.definitions.len() + 1, before);
    // An adversary can replace producer claims with fresh, internally hashed
    // claims. Unknown original code must still fail executable reconstruction.
    input.storage_profiles = validate(&input).profiles;
    map_unknown(validate(&input));
}

fn factory_join_fixture_state() -> &'static Fixture {
    static INPUT: OnceLock<Fixture> = OnceLock::new();
    INPUT.get_or_init(|| {
        with_compiler_stack(|| {
            let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
            let source = super::sealed_storage_placement_33::unknown_factory_program(true);
            let file = engine
                .sources
                .add("storage-factory-join-v.hir", source.into())
                .unwrap();
            let output = check_progress(&mut engine, file, &default_context()).unwrap();
            assert_eq!(output.completion, CheckCompletion::Complete);
            assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
            let input = engine.export_native_input().unwrap();
            Fixture {
                input,
                engine: Mutex::new(engine),
            }
        })
    })
}
#[test]
fn sealed_storage_factory_join_v_33_control() {
    let fixture = factory_join_fixture_state();
    let validation = validate(&fixture.input);
    assert!(validation.violations.is_empty(), "{validation:?}");
    assert_eq!(validation.profiles, fixture.input.storage_profiles);
    fixture
        .engine
        .lock()
        .unwrap()
        .import_native_input(fixture.input.clone())
        .unwrap();
}
#[test]
fn sealed_storage_factory_join_v_33_unknown_result_with_fresh_claims_refuses() {
    let mut input = factory_join_fixture_state().input.clone();
    let before = input.definitions.len();
    input.definitions.retain(|path, _| path.0.name != "main");
    assert_eq!(input.definitions.len() + 1, before);
    input.storage_profiles = validate(&input).profiles;
    map_unknown(validate(&input));
}

fn ct5_fixture_state() -> &'static Fixture {
    static INPUT: OnceLock<Fixture> = OnceLock::new();
    INPUT.get_or_init(||with_compiler_stack(||{
        let mut engine=QueryEngine::new(default_environment(),compiler_adapters());
        let source=format!("fn clone_heap(xs:List[u64])->unique List[u64]{{unique xs}}\n{}",super::sealed_storage_placement_33::ct5_storage_program(false).replace("list.len(v.items)","let independent:List[u64] in heap=[9u64];let cloned=clone_heap(independent);list.len(cloned)+list.len(v.items)"));
        let file=engine.sources.add("storage-ct5-v.hir",source.into()).unwrap();
        let output=check_progress(&mut engine,file,&default_context()).unwrap();assert_eq!(output.completion,CheckCompletion::Complete);assert!(output.diagnostics.is_empty(),"{:?}",output.diagnostics);
        let input=engine.export_native_input().unwrap();assert_eq!(input.instance_bodies.declarations.len(),1);
        Fixture{input,engine:Mutex::new(engine)}
    }))
}
#[test]
fn sealed_storage_ct5_v_33_executable_import_control() {
    let fixture = ct5_fixture_state();
    let validation = validate(&fixture.input);
    assert!(validation.violations.is_empty(), "{validation:?}");
    assert_eq!(validation.profiles, fixture.input.storage_profiles);
    let declaration = *fixture
        .input
        .instance_bodies
        .declarations
        .iter()
        .next()
        .unwrap();
    assert!(
        validation
            .reads
            .selections
            .iter()
            .any(|(owner, key, selected, valid)| *owner == declaration
                && key.is_some()
                && selected.is_some()
                && *valid)
    );
    let validated = fixture
        .engine
        .lock()
        .unwrap()
        .import_native_input(fixture.input.clone())
        .unwrap();
    assert!(validated.storage_proof_reads().selections.iter().any(
        |(owner, key, selected, valid)| *owner == declaration
            && key.is_some()
            && selected.is_some()
            && *valid
    ));
}
#[test]
fn sealed_storage_ct5_v_33_actual_selected_body_allocation() {
    let mut input = ct5_fixture_state().input.clone();
    let selected = input
        .instance_bodies
        .instances
        .values()
        .next()
        .unwrap()
        .values()
        .next()
        .unwrap()
        .definition;
    let definition = input.definitions.get_mut(selected.path()).unwrap();
    let mut mutations = 0;
    for body in &mut std::sync::Arc::make_mut(definition).bodies {
        let parameter = body.parameters[0];
        let original = body
            .places
            .iter()
            .position(|place| {
                place.root == parameter
                    && place.projection.len() == 1
                    && matches!(
                        place.projection.first(),
                        Some(hirsel_tir::Projection::Field(0))
                    )
            })
            .unwrap();
        for block in &mut body.blocks {
            if let hirsel_tir::Terminator::Call {
                callee: hirsel_tir::Callee::Definition { path, .. },
                arguments,
                ..
            } = &mut block.terminator
            {
                if path.0.name == "clone_heap" {
                    assert_eq!(arguments.len(), 1);
                    arguments[0].value.place = hirsel_tir::PlaceId(original as u32);
                    mutations += 1;
                }
            }
        }
    }
    assert_eq!(mutations, 1);
    let validation = validate(&input);
    assert!(validation.violations.iter().any(|violation| matches!(violation,
        StorageViolation::ExecutableRead(hirsel_type_analysis::storage::program::StorageReadViolation {
            failure: hirsel_type_analysis::storage::program::StorageReadFailure::AllocatesFromRoot(_),
            origin: Some(origin), ..
        }) if origin.callable.definition.path().0.name == "clone_heap")), "{validation:?}");
    assert!(validation.violations.iter().any(|violation|matches!(violation,StorageViolation::ExecutableRead(hirsel_type_analysis::storage::program::StorageReadViolation{failure:hirsel_type_analysis::storage::program::StorageReadFailure::AllocatesFromRoot(_),..}))),"{validation:?}");
}
#[test]
fn sealed_storage_ct5_v_33_missing_selected_row() {
    let mut input = ct5_fixture_state().input.clone();
    assert!(!input.instance_bodies.instances.is_empty());
    input.instance_bodies.instances.clear();
    map_unknown(validate(&input));
}
#[test]
fn sealed_storage_ct5_v_33_missing_selected_body() {
    let mut input = ct5_fixture_state().input.clone();
    let selected = input
        .instance_bodies
        .instances
        .values()
        .next()
        .unwrap()
        .values()
        .next()
        .unwrap()
        .definition;
    assert!(input.definitions.remove(selected.path()).is_some());
    map_unknown(validate(&input));
}
#[test]
fn sealed_storage_ct5_v_33_absent_call_actuals() {
    let mut input = ct5_fixture_state().input.clone();
    let declaration = *input.instance_bodies.declarations.iter().next().unwrap();
    let mut mutations = 0;
    for definition in input.definitions.values_mut() {
        for body in &mut std::sync::Arc::make_mut(definition).bodies {
            for block in &mut body.blocks {
                if let hirsel_tir::Terminator::Call {
                    callee: hirsel_tir::Callee::Definition { path, arguments },
                    ..
                } = &mut block.terminator
                {
                    if path == declaration.path() {
                        assert_eq!(arguments.len(), 1);
                        arguments.clear();
                        mutations += 1;
                    }
                }
            }
        }
    }
    assert_eq!(mutations, 1);
    map_unknown(validate(&input));
}

fn ct5_helper_fixture_state() -> &'static Fixture {
    static INPUT: OnceLock<Fixture> = OnceLock::new();
    INPUT.get_or_init(|| with_compiler_stack(|| {
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
        let source = format!("fn forward[T](v:T)->u64 uses{{}}{{generated_len(v)}}\nfn clone_heap(xs:List[u64])->unique List[u64]{{unique xs}}\n{}", super::sealed_storage_placement_33::ct5_storage_program(false)
            .replace("generated_len(root)", "forward(root)")
            .replace("list.len(v.items)", "let independent:List[u64] in heap=[9u64];let cloned=clone_heap(independent);list.len(cloned)+list.len(v.items)"));
        let file = engine.sources.add("storage-ct5-helper-v.hir", source.into()).unwrap();
        let output = check_progress(&mut engine, file, &default_context()).unwrap();
        assert_eq!(output.completion, CheckCompletion::Complete);
        assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
        let input = engine.export_native_input().unwrap();
        assert_eq!(input.instance_bodies.declarations.len(), 1);
        Fixture { input, engine: Mutex::new(engine) }
    }))
}
#[test]
fn sealed_storage_ct5_helper_v_33_executable_import_control() {
    let fixture = ct5_helper_fixture_state();
    let validation = validate(&fixture.input);
    assert!(validation.violations.is_empty(), "{validation:?}");
    assert_eq!(validation.profiles, fixture.input.storage_profiles);
    let declaration = *fixture
        .input
        .instance_bodies
        .declarations
        .iter()
        .next()
        .unwrap();
    assert!(
        validation
            .reads
            .selections
            .iter()
            .any(|(owner, key, selected, valid)| *owner == declaration
                && key.is_some()
                && selected.is_some()
                && *valid)
    );
    let validated = fixture
        .engine
        .lock()
        .unwrap()
        .import_native_input(fixture.input.clone())
        .unwrap();
    assert!(validated.storage_proof_reads().selections.iter().any(
        |(owner, key, selected, valid)| *owner == declaration
            && key.is_some()
            && selected.is_some()
            && *valid
    ));
}
#[test]
fn sealed_storage_ct5_helper_v_33_generated_allocation_with_fresh_claims() {
    let mut input = ct5_helper_fixture_state().input.clone();
    let selected = input
        .instance_bodies
        .instances
        .values()
        .next()
        .unwrap()
        .values()
        .next()
        .unwrap()
        .definition;
    let definition = input.definitions.get_mut(selected.path()).unwrap();
    let mut mutations = 0;
    for body in &mut std::sync::Arc::make_mut(definition).bodies {
        let parameter = body.parameters[0];
        let original = body
            .places
            .iter()
            .position(|place| {
                place.root == parameter
                    && place.projection.len() == 1
                    && matches!(
                        place.projection.first(),
                        Some(hirsel_tir::Projection::Field(0))
                    )
            })
            .unwrap();
        for block in &mut body.blocks {
            if let hirsel_tir::Terminator::Call {
                callee: hirsel_tir::Callee::Definition { path, .. },
                arguments,
                ..
            } = &mut block.terminator
            {
                if path.0.name == "clone_heap" {
                    assert_eq!(arguments.len(), 1);
                    arguments[0].value.place = hirsel_tir::PlaceId(original as u32);
                    mutations += 1;
                }
            }
        }
    }
    assert_eq!(mutations, 1);
    input.storage_profiles = validate(&input).profiles;
    let validation = validate(&input);
    assert!(validation.violations.iter().any(|violation| matches!(violation,
        StorageViolation::ExecutableRead(hirsel_type_analysis::storage::program::StorageReadViolation {
            failure: hirsel_type_analysis::storage::program::StorageReadFailure::AllocatesFromRoot(_),
            origin: Some(origin), ..
        }) if origin.callable.definition.path().0.name == "clone_heap")), "{validation:?}");
}
#[test]
fn sealed_storage_ct5_helper_v_33_absent_outer_actuals_refuses() {
    let mut input = ct5_helper_fixture_state().input.clone();
    let mut mutations = 0;
    for definition in input.definitions.values_mut() {
        for body in &mut std::sync::Arc::make_mut(definition).bodies {
            for block in &mut body.blocks {
                if let hirsel_tir::Terminator::Call {
                    callee: hirsel_tir::Callee::Definition { path, arguments },
                    ..
                } = &mut block.terminator
                {
                    if path.0.name == "forward" {
                        assert_eq!(arguments.len(), 1);
                        arguments.clear();
                        mutations += 1;
                    }
                }
            }
        }
    }
    assert_eq!(mutations, 1);
    input.storage_profiles = validate(&input).profiles;
    map_unknown(validate(&input));
}
#[test]
fn sealed_storage_ct5_helper_v_33_missing_selection_refuses() {
    let mut input = ct5_helper_fixture_state().input.clone();
    assert!(!input.instance_bodies.instances.is_empty());
    input.instance_bodies.instances.clear();
    input.storage_profiles = validate(&input).profiles;
    map_unknown(validate(&input));
}
#[test]
fn sealed_storage_ct5_helper_v_33_growing_executable_actuals_refuse_before_closing() {
    let mut input = ct5_helper_fixture_state().input.clone();
    let (path, definition) = input
        .definitions
        .iter_mut()
        .find(|(path, _)| path.0.name == "forward")
        .unwrap();
    let definition = std::sync::Arc::make_mut(definition);
    let mut mutations = 0;
    for body in &mut definition.bodies {
        for block in &mut body.blocks {
            if let hirsel_tir::Terminator::Call {
                callee:
                    hirsel_tir::Callee::Definition {
                        path: target,
                        arguments,
                    },
                ..
            } = &mut block.terminator
            {
                if target.0.name == "generated_len" {
                    assert_eq!(arguments.len(), 1);
                    let nested = input.types.intern(hirsel_types::Type::with_kind(
                        hirsel_types::TypeKind::Builtin {
                            kind: hirsel_types::BuiltinType::List,
                            arguments: arguments.clone().into(),
                        },
                    ));
                    definition.types = definition.types.iter().copied().chain([nested]).collect();
                    arguments[0] = hirsel_types::GenericArgument::Type(nested);
                    *target = path.clone();
                    mutations += 1;
                }
            }
        }
    }
    assert_eq!(mutations, 1);
    let structure = hirsel_validate::validate_structure(definition, &input.types);
    assert!(structure.is_empty(), "{structure:?}");
    let validation = validate(&input);
    assert!(
        validation
            .violations
            .contains(&StorageViolation::Malformed("executable storage equations")),
        "{validation:?}"
    );
    assert!(
        validation.reads.selections.is_empty(),
        "growth refused before context selection: {validation:?}"
    );
}

fn dictionary_fixture_state() -> &'static Fixture {
    static INPUT: OnceLock<Fixture> = OnceLock::new();
    INPUT.get_or_init(|| with_compiler_stack(|| {
        let source = "type ReadOps{read:fn(List[u64])->u64 uses{}}\nconst READ:ReadOps=ReadOps(read:read);\nfn clone_heap(xs:List[u64])->unique List[u64]{unique xs}\nfn read(xs:List[u64])->u64 uses{}{let fresh:List[u64] in heap=[list.len(xs)];let copied=clone_heap(fresh);list.len(copied)}\npub fn main(){let mut r=arena.with(fn(a:Arena)->Region[List[u64]]{let root:List[u64] in a=[7u64];arena.seal(take a,take root)});let n=region.with(mut r,fn(root:List[u64])->u64{let ops=READ;ops.read(root)});}";
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
        let file = engine.sources.add("storage-dictionary-v.hir", source.into()).unwrap();
        let output = check_progress(&mut engine, file, &default_context()).unwrap();
        assert_eq!(output.completion, CheckCompletion::Complete);
        assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
        let input = engine.export_native_input().unwrap();
        Fixture { input, engine: Mutex::new(engine) }
    }))
}

#[test]
fn sealed_storage_dictionary_v_33_executable_and_import_control() {
    let fixture = dictionary_fixture_state();
    let validation = validate(&fixture.input);
    assert!(validation.violations.is_empty(), "{validation:?}");
    assert_eq!(validation.profiles, fixture.input.storage_profiles);
    assert!(
        validation
            .reads
            .constants
            .iter()
            .any(|(definition, hash)| definition.path().0.name == "READ" && hash.is_some())
    );
    let mut engine = fixture.engine.lock().unwrap();
    let validated = engine.import_native_input(fixture.input.clone()).unwrap();
    assert!(
        validated
            .storage_proof_reads()
            .constants
            .iter()
            .any(|(definition, hash)| definition.path().0.name == "READ" && hash.is_some())
    );
}

#[test]
fn sealed_storage_dictionary_v_33_missing_constant_with_fresh_claims() {
    let mut input = dictionary_fixture_state().input.clone();
    let before = input.constants.len();
    input.constants.retain(|path, _| path.0.name != "READ");
    assert_eq!(before - input.constants.len(), 1);
    input.storage_profiles = validate(&input).profiles;
    let validation = validate(&input);
    assert!(
        validation
            .reads
            .constants
            .iter()
            .any(|(definition, hash)| definition.path().0.name == "READ" && hash.is_none())
    );
    map_unknown(validation);
}

#[test]
fn sealed_storage_dictionary_v_33_missing_body_with_fresh_claims() {
    let mut input = dictionary_fixture_state().input.clone();
    let before = input.definitions.len();
    input.definitions.retain(|path, _| path.0.name != "read");
    assert_eq!(before - input.definitions.len(), 1);
    input.storage_profiles = validate(&input).profiles;
    map_unknown(validate(&input));
}

#[test]
fn sealed_storage_dictionary_v_33_actual_allocation_with_fresh_claims() {
    let mut input = dictionary_fixture_state().input.clone();
    let definition = input
        .definitions
        .iter_mut()
        .find(|(path, _)| path.0.name == "read")
        .unwrap()
        .1;
    let definition = std::sync::Arc::make_mut(definition);
    let mut mutations = 0;
    for body in &mut definition.bodies {
        let original = body
            .places
            .iter()
            .position(|place| {
                place.projection.is_empty()
                    && body.locals[place.root.0 as usize].storage == hirsel_tir::Storage::Parameter
            })
            .unwrap();
        for block in &mut body.blocks {
            if let hirsel_tir::Terminator::Call {
                callee: hirsel_tir::Callee::Definition { path, .. },
                arguments,
                ..
            } = &mut block.terminator
            {
                if path.0.name == "clone_heap" {
                    assert_eq!(arguments.len(), 1);
                    arguments[0].value.place = hirsel_tir::PlaceId(original as u32);
                    mutations += 1;
                }
            }
        }
    }
    assert_eq!(mutations, 1);
    input.storage_profiles = validate(&input).profiles;
    let validation = validate(&input);
    assert!(validation.violations.iter().any(|violation| matches!(violation,
        StorageViolation::ExecutableRead(hirsel_type_analysis::storage::program::StorageReadViolation {
            failure: hirsel_type_analysis::storage::program::StorageReadFailure::AllocatesFromRoot(_),
            origin: Some(origin), ..
        }) if origin.callable.definition.path().0.name == "clone_heap")), "{validation:?}");
}

#[test]
fn sealed_storage_dictionary_v_33_open_constant_code_refuses_before_closing() {
    let mut input = dictionary_fixture_state().input.clone();
    let constant = input
        .constants
        .iter_mut()
        .find(|(path, _)| path.0.name == "READ")
        .unwrap()
        .1;
    let mut pending = vec![&mut constant.root];
    let mut mutations = 0;
    while let Some(value) = pending.pop() {
        match &mut value.kind {
            hirsel_tir::ValueKind::FunctionReference { arguments, .. } => {
                assert!(arguments.is_empty());
                arguments.push(hirsel_types::GenericArgument::Size(
                    hirsel_types::Size::Parameter(hirsel_types::SizeParameterRef::bound(
                        hirsel_types::SizeParamId(0),
                    )),
                ));
                mutations += 1;
            }
            hirsel_tir::ValueKind::Construct { fields, .. } => pending.extend(fields.iter_mut()),
            _ => (),
        }
    }
    assert_eq!(mutations, 1);
    input.storage_profiles = validate(&input).profiles;
    map_unknown(validate(&input));
}

#[test]
fn sealed_storage_dictionary_forms_33_literal_alias_projection() {
    export_and_import(
        "type ReadOps{read:fn(List[u64])->u64 uses{}}\nconst BASE:ReadOps=ReadOps(read:fn(xs:List[u64])->u64 uses{}{list.len(xs)});\nconst ALIAS:ReadOps=BASE;\nconst READ:ReadOps=ReadOps(read:(ALIAS).read);\npub fn main(){let mut r=arena.with(fn(a:Arena)->Region[List[u64]]{let root:List[u64] in a=[7u64];arena.seal(take a,take root)});let n=region.with(mut r,fn(root:List[u64])->u64{let ops=READ;ops.read(root)});}",
    );
}

#[test]
fn sealed_storage_dictionary_forms_33_closed_generic_member() {
    export_and_import(
        "type ReadOps{read:fn(List[u64])->u64 uses{}}\nconst READ:ReadOps=ReadOps(read:read);\nfn read[T](xs:List[T])->u64 uses{}{list.len(xs)}\npub fn main(){let mut r=arena.with(fn(a:Arena)->Region[List[u64]]{let root:List[u64] in a=[7u64];arena.seal(take a,take root)});let n=region.with(mut r,fn(root:List[u64])->u64{let ops=READ;ops.read(root)});}",
    );
}

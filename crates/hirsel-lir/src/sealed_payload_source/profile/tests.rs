use super::*;
use crate::abi_types::CallMultiplicity;
use crate::ir::SiteRange;
use crate::operation::{BuiltinType, OperationArgument, Primitive};
use crate::sealed_payload_source::{PayloadLiteral, PayloadProducer};
use hirsel_syntax::DefPath;

fn hash(value: u8) -> ContentHash {
    ContentHash([value; 32])
}
fn ty(ty: NativeType, key: u8) -> ClosedStorageType {
    ClosedStorageType {
        ty,
        source_type: hash(key),
    }
}
fn profile(capture: NativeType) -> ClosedStorageProfile {
    let function = ty(NativeType::Function(CallMultiplicity::Once), 1);
    ClosedStorageProfile {
        schema: ClosedStorageProfile::SCHEMA,
        root: function.clone(),
        functions: vec![ClosedStorageFunction {
            ty: function,
            cases: vec![ClosedStorageCase {
                body: ReferenceBody {
                    definition: DefPath::core("closed.test"),
                    instance: hash(2),
                    body: 0,
                },
                recipe: hash(3),
                environment: Some(ClosedEnvironment {
                    captures: vec![ClosedCapture {
                        ty: ty(capture, 4),
                        passing: Passing::Take,
                    }],
                    sizes: vec![7],
                    live: vec![0],
                }),
            }],
        }],
    }
    .canonical()
    .unwrap()
}

#[test]
fn closed_storage_profile_33_same_function_type_different_environment_is_distinct() {
    let bits = profile(NativeType::Primitive(Primitive::U64));
    let list = profile(NativeType::Builtin(
        BuiltinType::List,
        vec![OperationArgument::Type(NativeType::Primitive(
            Primitive::U64,
        ))],
    ));
    assert_eq!(bits.root, list.root);
    assert_ne!(bits.content().unwrap(), list.content().unwrap());
    let wire = hirsel_codec::to_text(&list);
    let decoded: ClosedStorageProfile = hirsel_codec::from_text(&wire).unwrap();
    assert_eq!(decoded, list);
    assert_eq!(
        decoded.canonical_bytes().unwrap(),
        list.canonical_bytes().unwrap()
    );
}

#[test]
fn closed_storage_profile_33_canonical_alternatives_and_exact_owned_slots() {
    let original = profile(NativeType::Primitive(Primitive::U64));
    let mut expanded = original.clone();
    let mut alternate = expanded.functions[0].cases[0].clone();
    alternate.body.instance = hash(5);
    expanded.functions[0].cases.push(alternate);
    let canonical = expanded.clone().canonical().unwrap();
    expanded.functions[0].cases.reverse();
    assert_eq!(expanded.canonical().unwrap(), canonical);

    let mut duplicate = original.clone();
    duplicate.functions[0]
        .cases
        .push(duplicate.functions[0].cases[0].clone());
    assert!(duplicate.canonical().is_err());
    let mut unknown = original.clone();
    unknown.functions[0].cases[0].recipe = hash(0);
    assert!(unknown.check_structure().is_err());
    let mut borrowed = original.clone();
    borrowed.functions[0].cases[0]
        .environment
        .as_mut()
        .unwrap()
        .captures[0]
        .passing = Passing::Borrow;
    assert!(borrowed.check_structure().is_err());
    for live in [vec![0, 0], vec![1]] {
        let mut invalid = original.clone();
        invalid.functions[0].cases[0]
            .environment
            .as_mut()
            .unwrap()
            .live = live;
        assert!(invalid.check_structure().is_err());
    }
}

#[test]
fn closed_storage_profile_33_recipe_type_sizes_and_empty_environment_are_observed() {
    let original = profile(NativeType::Primitive(Primitive::U64));
    let key = original.content().unwrap();
    for axis in 0..5 {
        let mut changed = original.clone();
        let case = &mut changed.functions[0].cases[0];
        match axis {
            0 => case.recipe = hash(6),
            1 => {
                case.environment.as_mut().unwrap().captures[0]
                    .ty
                    .source_type = hash(6)
            }
            2 => case.environment.as_mut().unwrap().sizes[0] += 1,
            3 => case.environment.as_mut().unwrap().live.clear(),
            4 => case.body.body += 1,
            _ => unreachable!(),
        }
        assert_ne!(changed.content().unwrap(), key);
    }
    let mut code_only = original.clone();
    code_only.functions[0].cases[0].environment = None;
    let mut empty_environment = code_only.clone();
    empty_environment.functions[0].cases[0].environment = Some(ClosedEnvironment {
        captures: vec![],
        sizes: vec![],
        live: vec![],
    });
    assert_ne!(
        code_only.content().unwrap(),
        empty_environment.content().unwrap()
    );
    let empty = ClosedStorageProfile {
        schema: 1,
        root: ty(NativeType::Primitive(Primitive::U64), 7),
        functions: vec![],
    };
    assert!(empty.check_structure().is_ok());
    assert_ne!(empty.content().unwrap(), key);
}

#[test]
fn closed_storage_profile_33_original_selector_is_separate_and_exactly_bound() {
    let profile = profile(NativeType::Primitive(Primitive::U64));
    let make_operand = |place, ty, producer| PayloadOperand {
        place,
        ty,
        producer,
        capture_slot: None,
    };
    let mut selector = SourceStorageSelection {
        body: ReferenceBody {
            definition: DefPath::core("caller"),
            instance: hash(8),
            body: 0,
        },
        call: SourceAnchor {
            body: 0,
            block: 0,
            statement: 0,
            site: SiteRange {
                source: 0,
                start: 1,
                end: 2,
            },
        },
        operation: OperationId {
            definition: DefPath::core("arena.seal"),
            declaration_digest: hash(9),
        },
        grant: make_operand(
            0,
            NativeType::Named(DefPath::core("Arena"), vec![]),
            PayloadProducer::Parameter(0),
        ),
        root: make_operand(
            1,
            profile.root.ty.clone(),
            PayloadProducer::Closure(SourceAnchor {
                body: 0,
                block: 0,
                statement: 0,
                site: SiteRange {
                    source: 0,
                    start: 0,
                    end: 1,
                },
            }),
        ),
        profile: profile.content().unwrap(),
    };
    assert!(selector.check_structure(&profile).is_ok());
    let original = profile.canonical_bytes().unwrap();
    selector.body.instance = hash(10);
    selector.call.statement += 1;
    assert!(selector.check_structure(&profile).is_ok());
    assert_eq!(profile.canonical_bytes().unwrap(), original);
    selector.profile = hash(10);
    assert!(selector.check_structure(&profile).is_err());
    selector.profile = profile.content().unwrap();
    selector.call.body += 1;
    assert!(selector.check_structure(&profile).is_err());
    selector.call.body = selector.body.body;
    selector.root.producer = PayloadProducer::Capture(2);
    assert!(selector.check_structure(&profile).is_err());
    selector.root.capture_slot = Some(2);
    assert!(selector.check_structure(&profile).is_ok());
    selector.operation.definition = DefPath::core("other.seal");
    assert!(selector.check_structure(&profile).is_err());
    // Literals are data too; no signature-derived capture ordinal is accepted.
    selector.root.producer = PayloadProducer::Literal(PayloadLiteral::Unit);
    assert!(selector.check_structure(&profile).is_err());
}

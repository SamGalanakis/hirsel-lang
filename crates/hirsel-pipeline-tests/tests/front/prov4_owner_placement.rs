//! PROV-4 owner placement must survive construction and closure capture.
use hirsel_engine::{
    DefinitionQueries, QueryEngine, Verify, default_context, default_environment, entry_path,
};

const CAPTURE: &str =
    include_str!("../../../../conformance/PROV/PROV-4-escaping-arena-boxed-part-capture-ok.hir");
const CONSTRUCTOR: &str =
    include_str!("../../../../conformance/PROV/PROV-4-union-arena-outside-binding-ok.hir");

fn lowered(
    source: &str,
    names: &[&str],
    entry: &str,
) -> (
    QueryEngine,
    hirsel_tir::Definition,
    hirsel_validate::SignatureMap,
) {
    let mut engine = QueryEngine::new(default_environment(), hirsel_driver::compiler_adapters());
    let file = engine
        .sources
        .add("owner-placement.hir", source.into())
        .unwrap();
    let parsed = engine.parse_file(file, &default_context()).unwrap();
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let mut path = entry_path(&default_context());
    path.0.name = entry.into();
    let modes = engine.mode_definition(&path).unwrap();
    assert!(modes.diagnostics.is_empty(), "{:?}", modes.diagnostics);
    // Obtain the producer output so the test also exercises V directly on
    // forged twins, independently of the producer's acceptance decision.
    engine.verify = Verify::Off;
    let definition = (*engine.lower_to_tir(&path).unwrap().value).clone();
    let mut signatures = hirsel_validate::SignatureMap::new(engine.types().clone());
    for name in names {
        let mut path = entry_path(&default_context());
        path.0.name = (*name).into();
        let signature = engine.item_signature(&path).unwrap().value;
        signatures.insert(signature.definition, (*signature).clone());
    }
    (engine, definition, signatures)
}

#[test]
fn capture_preserves_owner_placement() {
    let (engine, definition, signatures) = lowered(CAPTURE, &["probe"], "probe");
    hirsel_validate::assert_valid(&definition, engine.types(), &signatures);
}

#[test]
fn constructor_preserves_owner_placement() {
    let (engine, definition, signatures) =
        lowered(CONSTRUCTOR, &["View", "choose", "good"], "good");
    hirsel_validate::assert_valid(&definition, engine.types(), &signatures);
}

fn replace_type(
    definition: &mut hirsel_tir::Definition,
    old: hirsel_types::TypeId,
    new: hirsel_types::TypeId,
    types: &hirsel_types::TypeInterner,
) {
    for body in &mut definition.bodies {
        for local in &mut body.locals {
            if local.ty == old {
                local.ty = new;
            }
        }
        for place in &mut body.places {
            if place.ty == old {
                place.ty = new;
            }
        }
    }
    definition.types = hirsel_tir::definition_types(types, definition);
}

fn rejects_at(
    definition: &hirsel_tir::Definition,
    types: &hirsel_types::TypeInterner,
    signatures: &hirsel_validate::SignatureMap,
    entity: &'static str,
) {
    let report = hirsel_validate::validate(definition, types, signatures, &mut vec![]);
    assert!(report.violations.iter().any(|v| matches!(&v.kind, hirsel_validate::ViolationKind::TypeMismatch { entity: found } if *found == entity)), "{:#?}", report.violations);
}

#[test]
fn capture_rejects_a_different_explicit_arena_and_heap() {
    let source = CAPTURE.replace("probe(a: Arena)", "probe(a: Arena, other_arena: Arena)");
    let (engine, definition, signatures) = lowered(&source, &["probe"], "probe");
    hirsel_validate::assert_valid(&definition, engine.types(), &signatures);
    let nested = &definition.bodies[1];
    let old = nested
        .locals
        .iter()
        .find(|l| l.storage == hirsel_tir::Storage::Capture)
        .unwrap()
        .ty;
    for heap in [false, true] {
        let mut row = engine.types().get(old).unwrap().clone();
        row.modes.relation = if heap {
            hirsel_types::Relation::global()
        } else {
            hirsel_types::Relation::parameters([hirsel_types::ParameterRef::body(
                definition.bodies[0].parameter_scope,
                hirsel_types::ParamId(1),
            )])
            .union(&hirsel_types::Relation::environment())
        };
        let new = engine.types().intern(row);
        let mut forged = definition.clone();
        replace_type(&mut forged, old, new, engine.types());
        rejects_at(&forged, engine.types(), &signatures, "closure capture");
    }
}

#[test]
fn constructor_rejects_a_missing_explicit_arena_and_heap() {
    let (engine, definition, signatures) =
        lowered(CONSTRUCTOR, &["View", "choose", "good"], "good");
    hirsel_validate::assert_valid(&definition, engine.types(), &signatures);
    let old = definition.types.iter().copied().find(|t| matches!(&engine.types().get(*t).unwrap().kind, hirsel_types::TypeKind::Named { definition, .. } if definition.path().0.name == "View")).unwrap();
    for heap in [false, true] {
        let mut row = engine.types().get(old).unwrap().clone();
        row.modes.relation = if heap {
            hirsel_types::Relation::global()
        } else {
            hirsel_types::Relation::parameters([hirsel_types::ParameterRef::body(
                definition.bodies[0].parameter_scope,
                hirsel_types::ParamId(0),
            )])
            .union(&hirsel_types::Relation::environment())
        };
        let new = engine.types().intern(row);
        let mut forged = definition.clone();
        replace_type(&mut forged, old, new, engine.types());
        if heap {
            for rv in &mut forged.bodies[0].rvalues {
                if let hirsel_tir::Rvalue::Construct {
                    constructor: hirsel_tir::Constructor::Record(_),
                    destination,
                    ..
                } = rv
                {
                    *destination = hirsel_types::Destination::Inferred;
                }
            }
        }
        rejects_at(&forged, engine.types(), &signatures, "constructor field");
    }
}

#[test]
fn constructor_keeps_the_explicit_destination_when_it_sorts_after_content() {
    for (source, ordinal) in [
        (CONSTRUCTOR.to_owned(), 0),
        (
            CONSTRUCTOR.replace("good(a: Arena, b: Arena,", "good(b: Arena, a: Arena,"),
            1,
        ),
    ] {
        let (engine, definition, signatures) =
            lowered(&source, &["View", "choose", "good"], "good");
        hirsel_validate::assert_valid(&definition, engine.types(), &signatures);
        let scope = definition.bodies[0].parameter_scope;
        let arena = hirsel_types::Origin {
            root: hirsel_types::OriginRoot::Parameter(hirsel_types::ParameterRef::body(
                scope,
                hirsel_types::ParamId(ordinal),
            )),
            fields: vec![].into(),
        };
        let destination = definition.bodies[0]
            .rvalues
            .iter()
            .find_map(|rv| match rv {
                hirsel_tir::Rvalue::Construct {
                    constructor: hirsel_tir::Constructor::Record(_),
                    destination,
                    ..
                } => Some(destination),
                _ => None,
            })
            .unwrap();
        assert_eq!(*destination, hirsel_types::Destination::Arena(arena));
    }
}

#[test]
fn constructor_rejects_allocation_in_an_arena_outside_its_relation() {
    let source = CONSTRUCTOR.replace(
        "good(a: Arena, b: Arena,",
        "good(a: Arena, b: Arena, other_arena: Arena,",
    );
    let (engine, mut definition, signatures) =
        lowered(&source, &["View", "choose", "good"], "good");
    hirsel_validate::assert_valid(&definition, engine.types(), &signatures);
    let foreign = hirsel_types::Origin {
        root: hirsel_types::OriginRoot::Parameter(hirsel_types::ParameterRef::body(
            definition.bodies[0].parameter_scope,
            hirsel_types::ParamId(2),
        )),
        fields: vec![].into(),
    };
    for replacement in [
        hirsel_types::Destination::Arena(foreign),
        hirsel_types::Destination::Heap,
        hirsel_types::Destination::Inferred,
    ] {
        for value in &mut definition.bodies[0].rvalues {
            if let hirsel_tir::Rvalue::Construct {
                constructor: hirsel_tir::Constructor::Record(_),
                destination,
                ..
            } = value
            {
                *destination = replacement.clone();
            }
        }
        rejects_at(
            &definition,
            engine.types(),
            &signatures,
            "constructor placement",
        );
    }
}

#[test]
fn omitted_destination_receipt_never_invents_an_explicit_arena() {
    use hirsel_engine::LoweringPhase;
    for (source, ambiguous) in [(CAPTURE, false), (CONSTRUCTOR, true)] {
        let (mut engine, _, signatures) = if ambiguous {
            lowered(source, &["View", "choose", "good"], "good")
        } else {
            lowered(source, &["probe"], "probe")
        };
        let mut path = entry_path(&default_context());
        path.0.name = if ambiguous { "good" } else { "probe" }.into();
        let signature = engine.item_signature(&path).unwrap().value;
        let mut analyzed = engine.mode_definition(&path).unwrap().value;
        for row in &mut std::sync::Arc::make_mut(&mut analyzed.typed)
            .checked
            .expressions
        {
            row.construction_destination = None;
        }
        let result = hirsel_driver::LowerAdapter.lower(
            engine.types(),
            &hirsel_types::TypeMemo::new(),
            &analyzed,
            &hirsel_engine::Comptime::default(),
            &signature,
        );
        if ambiguous {
            let error = result.unwrap_err();
            assert!(
                format!("{error:?}")
                    .contains("missing construction destination for multiple arenas"),
                "{error:?}"
            );
        } else {
            hirsel_validate::assert_valid(&result.unwrap().value, engine.types(), &signatures);
        }
    }
}

#[test]
fn plain_construction_placement_remains_inert() {
    let source = include_str!("../../../../conformance/PROV/PROV-4-plain-placement-inert.hir");
    let (mut engine, definition, mut signatures) =
        lowered(source, &["P", "sum", "example"], "example");
    let signature = engine
        .item_signature(&hirsel_syntax::DefPath::core("arena.with"))
        .unwrap()
        .value;
    signatures.insert(signature.definition, (*signature).clone());
    hirsel_validate::assert_valid(&definition, engine.types(), &signatures);
}

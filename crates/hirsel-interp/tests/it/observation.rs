use hirsel_interp::observation::*;
use hirsel_interp::{Execution, Failure, Integer, Outcome, TraceEvent, Value};
use hirsel_span::{SourceMap, Span};
use hirsel_tir::{BodyId, CleanupId};
use hirsel_types::Primitive;
fn fixture() -> (SourceMap, Execution) {
    let mut sources = SourceMap::default();
    let file = sources.add("input.hir", "one\ntwo\n".into()).unwrap();
    let path = hirsel_syntax::DefPath::core("fixture");
    let anchor = Anchor {
        definition: path,
        body: BodyId(7),
        cleanup: Some((CleanupId(8), 3)),
        site: Span::new(file, 4, 7).unwrap(),
    };
    let failures = ["primary", "drop-one", "drop-two"]
        .into_iter()
        .enumerate()
        .map(|(i, kind)| FailureRecord {
            operation: None,
            failure: Failure {
                kind: kind.into(),
                rule: const { hirsel_diag::RuleId::of("FAIL-1") },
                line: 2,
                site: anchor.site,
            },
            anchor: anchor.clone(),
            cause: (i < 2).then_some(i + 1),
            spawn_path: vec![2, 5],
        })
        .collect::<Vec<_>>();
    (
        sources,
        Execution {
            outcome: Outcome::Failed(failures[0].failure.clone()),
            failures,
            stdout: b"before\0failure".to_vec(),
            trace: vec![TraceEvent::Effect(OrderedEffect {
                anchor,
                effect: Effect::Forgotten {
                    definition: hirsel_syntax::DefPath::core("Resource"),
                },
            })],
            steps: 19,
        },
    )
}
#[test]
fn failure_nodes_preserve_kind_spawn_causes_and_ignore_transient_arenas() {
    let (sources, mut execution) = fixture();
    let first = execution.observable(&sources).unwrap();
    assert_eq!(first.failures[0].kind, "primary");
    assert_eq!(first.failures[0].spawn_path, [2, 5]);
    assert_eq!(first.failures[0].cause_indices, [1]);
    assert_eq!(first.failures[1].cause_indices, [2]);
    assert!(first.failures[2].cause_indices.is_empty());
    for f in &mut execution.failures {
        f.anchor.body = BodyId(999);
        f.anchor.cleanup = None;
    }
    if let TraceEvent::Effect(e) = &mut execution.trace[0] {
        e.anchor.body = BodyId(500);
        e.anchor.cleanup = None;
    }
    execution.steps = 1;
    assert!(first.equivalent(&execution.observable(&sources).unwrap()));
    execution.failures[1].failure.kind = "other-kind".into();
    assert!(!first.equivalent(&execution.observable(&sources).unwrap()));
    execution.failures[0].cause = Some(100);
    assert!(execution.observable(&sources).is_none());
}
#[test]
fn values_preserve_width_and_float_bits_and_name_function_disposition() {
    let narrow =
        ObservedValue::from_value(&Value::Integer(Integer::new(Primitive::U8, 1).unwrap()));
    let wide = ObservedValue::from_value(&Value::Integer(Integer::new(Primitive::U64, 1).unwrap()));
    assert_ne!(narrow, wide);
    assert_ne!(
        ObservedValue::from_value(&Value::F64(0)),
        ObservedValue::from_value(&Value::F64(1 << 63))
    );
    let (sources, mut execution) = fixture();
    execution.failures.clear();
    execution.outcome = Outcome::Returned {
        value: Value::Closure {
            invoked: Default::default(),
            definition: hirsel_syntax::DefPath::core("f"),
            body: BodyId(2),
            captures: vec![],
            owned_captures: vec![],
            sizes: Default::default(),
            type_arguments: Default::default(),
        },
        exit_code: 0,
    };
    let observed = execution.observable(&sources).unwrap();
    assert!(!observed.returned.as_ref().unwrap().comparable());
    assert!(!observed.equivalent(&observed));
}
#[test]
fn deeply_nested_observation_uses_an_owned_flat_arena() {
    let mut value = Value::Unit;
    for _ in 0..10_000 {
        value = Value::Box(vec![Some(value)]);
    }
    let observed = ObservedValue::from_value(&value);
    assert_eq!(observed.nodes.len(), 10_001);
    assert!(observed.comparable());
    assert_eq!(observed.nodes[10_000], ObservedNode::Unit);
}

#[test]
fn semantic_anchor_roles_are_complete_and_independent_of_transient_ids() {
    let (sources, execution) = fixture();
    let TraceEvent::Effect(original) = &execution.trace[0] else {
        unreachable!()
    };
    let path = original.anchor.definition.clone();
    let cases = [
        (
            Effect::ResourceCreated {
                definition: path.clone(),
            },
            SemanticRole::ResourceConstruction,
        ),
        (
            Effect::Finalizer {
                definition: path.clone(),
            },
            SemanticRole::FinalizerEntry,
        ),
        (
            Effect::Forgotten {
                definition: path.clone(),
            },
            SemanticRole::Forget,
        ),
        (
            Effect::HandleDropped {
                kind: "File".into(),
                id: 7,
            },
            SemanticRole::HandleDrop,
        ),
        (
            Effect::CapabilityConsumed(hirsel_tir::CapabilityEvent {
                capability: path,
                operation: "read".into(),
                input: vec![0],
                output: vec![1],
            }),
            SemanticRole::CapabilityEffect,
        ),
    ];
    for (effect, role) in cases {
        let mut event = OrderedEffect {
            anchor: original.anchor.clone(),
            effect,
        };
        let stable = event.semantic_anchor(&sources).unwrap();
        assert_eq!(stable.role, role);
        event.anchor.body = BodyId(999);
        event.anchor.cleanup = Some((CleanupId(777), 42));
        assert_eq!(event.semantic_anchor(&sources).unwrap(), stable);
    }
}

#[test]
fn named_reference_specializations_ignore_arena_layout_and_preserve_argument_identity() {
    use hirsel_interp::value::NamedFunction;
    use hirsel_types::*;
    let reference = |primitive, size, padding| {
        let mut types = TypeStore::default();
        for _ in 0..padding {
            types.push(Type::with_kind(TypeKind::Primitive(Primitive::Bool)));
        }
        let root = types.push(Type::with_kind(TypeKind::Primitive(primitive)));
        Value::FunctionReference(NamedFunction {
            definition: hirsel_syntax::DefPath::core("target"),
            arguments: vec![
                GenericArgument::Type(root),
                GenericArgument::Size(Size::Constant(size)),
            ],
            types: std::sync::Arc::new(types),
            ty: root,
        })
    };
    let a = reference(Primitive::U32, 7, 0);
    let b = reference(Primitive::U32, 7, 13);
    let c = reference(Primitive::U64, 7, 0);
    let d = reference(Primitive::U32, 8, 0);
    assert_eq!(a, b);
    assert_ne!(a, c);
    assert_ne!(a, d);
    let a = ObservedValue::from_value(&a);
    assert!(a.comparable());
    assert_eq!(a, ObservedValue::from_value(&b));
    assert_ne!(a, ObservedValue::from_value(&c));
    assert_ne!(a, ObservedValue::from_value(&d));
}

#[test]
fn only_free_lexical_specialization_arguments_are_non_comparable() {
    use hirsel_types::*;
    let mut types = TypeStore::default();
    let root = types.push(Type::with_kind(TypeKind::Parameter(
        TypeParameterRef::body(NameSite::Item(hirsel_syntax::ItemId(7)), TypeParamId(0)),
    )));
    let definition = hirsel_syntax::DefPath::core("target");
    let value = Value::FunctionReference(hirsel_interp::value::NamedFunction {
        definition: definition.clone(),
        arguments: vec![
            GenericArgument::Type(root),
            GenericArgument::Size(Size::Constant(3)),
        ],
        types: std::sync::Arc::new(types),
        ty: root,
    });
    let observed = ObservedValue::from_value(&value);
    assert!(!observed.comparable());
    let ObservedNode::FunctionReference {
        definition: actual,
        arguments,
    } = &observed.nodes[0]
    else {
        panic!()
    };
    assert_eq!(actual, &definition);
    assert!(arguments[0].is_none());
    assert!(arguments[1].is_some());
}

#[test]
fn operation_sites_are_independent_semantic_failure_locations() {
    let (mut sources, mut execution) = fixture();
    let helper = sources.add("helper.hir", "operation\n".into()).unwrap();
    let mut operation = execution.failures[0].anchor.clone();
    operation.definition = hirsel_syntax::DefPath::core("helper");
    operation.site = Span::new(helper, 0, 9).unwrap();
    execution.failures[0].operation = Some(operation);
    let first = execution.observable(&sources).unwrap();
    assert_eq!(first.failures[0].operation.as_ref().unwrap().line, 1);
    execution.failures[0].operation.as_mut().unwrap().body = BodyId(999);
    assert!(first.equivalent(&execution.observable(&sources).unwrap()));
    execution.failures[0].operation.as_mut().unwrap().site = Span::new(helper, 1, 9).unwrap();
    let second = execution.observable(&sources).unwrap();
    assert_eq!(
        first.failures[0].stable_site,
        second.failures[0].stable_site
    );
    assert!(!first.equivalent(&second));
    execution.failures[0].operation.as_mut().unwrap().site = Span::new(helper, 0, 100).unwrap();
    assert!(execution.observable(&sources).is_none());
}

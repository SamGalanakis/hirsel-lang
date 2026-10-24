use super::*;
use hirsel_types::{CheckedBody, Elaboration, OwnershipFacts, Resolution, Type, UseKind};
use std::{collections::BTreeMap, task::Poll};

fn fixture() -> (Arc<hirsel_types::TypeInterner>, AnalyzedBody, Signature) {
    let mut engine = QueryEngine::new(default_environment(), crate::phases::engine_only_adapters());
    let file = engine
        .sources
        .add(
            "prepare.hir",
            "fn root()->(){comptime { LEAF; comptime () }}".into(),
        )
        .unwrap();
    let syntax = engine
        .parse_file(file, &default_context())
        .unwrap()
        .value
        .items
        .remove(0);
    let types = engine.types.clone();
    let unit = types.intern(Type::unit());
    let boolean = types.intern(Type::boolean());
    let mut resolution = Resolution::default();
    for (i, expression) in syntax.tree.expressions.iter().enumerate() {
        if matches!(expression.kind, ExprKind::Path(_)) {
            resolution.expressions.push((
                ExprId(i as u32),
                ResolvedName::Definition(DefPath::core("LEAF")),
            ));
        }
    }
    let checked = CheckedBody {
        lexical: syntax.owner(),
        places: vec![],
        calls: vec![],
        tails: vec![],
        bindings: vec![],
        patterns: vec![],
        resolution,
        expressions: syntax
            .tree
            .expressions
            .iter()
            .enumerate()
            .map(|(i, _)| Elaboration {
                construction_destination: None,
                expression: ExprId(i as u32),
                ty: unit,
                usage: UseKind::Take,
                instantiated_arguments: vec![],
            })
            .collect(),
        instantiations: vec![],
        closures: vec![],
    };
    let signature = Signature {
        definition: DefId::intern(&syntax.item.definition),
        kind: SignatureKind::Const {
            ty: unit,
            generics: Default::default(),
        },
        metadata: Default::default(),
        fingerprint: syntax.text_hash,
    };
    (
        types,
        AnalyzedBody {
            typed: Arc::new(TypedBody {
                syntax,
                checked,
                plans: Default::default(),
                unit,
                boolean,
            }),
            ownership: OwnershipFacts::default(),
            results: vec![],
            call_allocations: vec![],
        },
        signature,
    )
}
struct Source {
    signature: Arc<Signature>,
    signatures: usize,
    constants: usize,
    evaluations: BTreeMap<ExprId, usize>,
    substitutions: Vec<Vec<ExprId>>,
    fail: bool,
}
impl PreparingSource for Source {
    fn signature(&mut self, _: &DefPath) -> Poll<Result<Arc<Signature>, QueryError>> {
        self.signatures += 1;
        if self.signatures == 1 {
            Poll::Pending
        } else {
            Poll::Ready(Ok(self.signature.clone()))
        }
    }
    fn constant(&mut self, _: &DefPath, _: Span) -> Poll<Result<Materialized, QueryError>> {
        self.constants += 1;
        if self.constants == 1 {
            return Poll::Pending;
        }
        Poll::Ready(if self.fail {
            Err(QueryError::NotImplemented)
        } else {
            Ok(value(match self.signature.kind {
                SignatureKind::Const { ty, .. } => ty,
                _ => unreachable!(),
            }))
        })
    }
    fn evaluate(&mut self, input: EvaluationInput<'_>) -> Poll<Result<Materialized, QueryError>> {
        let calls = self.evaluations.entry(input.expression).or_default();
        *calls += 1;
        if *calls == 1 {
            return Poll::Pending;
        }
        self.substitutions
            .push(input.constants.iter().map(|(id, _)| *id).collect());
        Poll::Ready(Ok(value(input.ty)))
    }
}
fn value(ty: TypeId) -> Materialized {
    Materialized {
        root: hirsel_tir::ValueNode {
            ty,
            kind: ValueKind::Constant(Constant::Unit),
        },
    }
}
fn source(signature: &Signature) -> Source {
    Source {
        signature: Arc::new(signature.clone()),
        signatures: 0,
        constants: 0,
        evaluations: BTreeMap::new(),
        substitutions: vec![],
        fail: false,
    }
}
#[test]
fn suspension_retains_constant_and_inner_expression_substitutions_without_replay() {
    crate::with_compiler_stack(|| {
        let (types, body, signature) = fixture();
        let mut source = source(&signature);
        let mut preparation = ValuePreparation::new(&body);
        let values = loop {
            if let Poll::Ready(result) =
                preparation.poll(&types, &body, &[], &signature, &mut source)
            {
                break result.unwrap().to_vec();
            }
        };
        assert_eq!(
            values.len(),
            3,
            "one constant, then two nested comptime expressions"
        );
        assert_eq!(
            source.signatures, 2,
            "pending constant must retain its selected signature"
        );
        assert_eq!(source.constants, 2);
        assert_eq!(source.evaluations.len(), 2);
        assert!(source.evaluations.values().all(|calls| *calls == 2));
        assert_eq!(
            source.substitutions,
            [vec![values[0].0], vec![values[0].0, values[1].0]]
        );
        let repeated = preparation.poll(&types, &body, &[], &signature, &mut source);
        let Poll::Ready(Ok(repeated)) = repeated else {
            panic!("completed preparation must be ready");
        };
        assert_eq!(
            repeated
                .iter()
                .map(|(id, value)| (*id, &value.root))
                .collect::<Vec<_>>(),
            values
                .iter()
                .map(|(id, value)| (*id, &value.root))
                .collect::<Vec<_>>()
        );
        assert_eq!(source.constants, 2);
        assert_eq!(source.signatures, 2);
        assert_eq!(source.substitutions.len(), 2);
    });
}
#[test]
fn failed_dependency_stops_preparation_before_later_expression_selection() {
    crate::with_compiler_stack(|| {
        let (types, body, signature) = fixture();
        let mut source = source(&signature);
        source.fail = true;
        let mut preparation = ValuePreparation::new(&body);
        for _ in 0..2 {
            assert!(
                preparation
                    .poll(&types, &body, &[], &signature, &mut source)
                    .is_pending()
            );
        }
        for _ in 0..2 {
            assert!(matches!(
                preparation.poll(&types, &body, &[], &signature, &mut source),
                Poll::Ready(Err(QueryError::NotImplemented))
            ));
        }
        assert_eq!(source.signatures, 2);
        assert_eq!(source.constants, 2);
        assert!(source.evaluations.is_empty());
    });
}

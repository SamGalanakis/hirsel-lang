use crate::support;
use hirsel_interp::{Outcome, Value, run};
use hirsel_tir::*;
use hirsel_types::*;
use std::sync::Arc;
use support::*;

struct ConstantsProvider {
    definition: DefinitionRef,
    data: MaterializedRef,
    requests: Vec<String>,
}
impl TirProvider for ConstantsProvider {
    fn definition(&mut self, _: &hirsel_syntax::DefPath) -> Result<DefinitionRef, QueryError> {
        self.requests.push("entry".into());
        Ok(self.definition.clone())
    }
    fn constant(&mut self, _: &hirsel_syntax::DefPath) -> Result<MaterializedRef, QueryError> {
        self.requests.push("constant".into());
        Ok(self.data.clone())
    }
    fn constant_definitions(&mut self) -> Result<Vec<hirsel_syntax::DefPath>, QueryError> {
        Ok(vec![path("DATA")])
    }
}
#[test]
fn two_reads_share_one_pre_entry_materialization_for_every_data_kind() {
    for shape in 0..10 {
        let mut b = Builder::new();
        let number = b.types.push(ty(TypeKind::Primitive(Primitive::U32)));
        let string = b.types.push(ty(TypeKind::Primitive(Primitive::String)));
        let boolean = b.types.push(ty(TypeKind::Primitive(Primitive::Bool)));
        let bytes = b.types.push(ty(TypeKind::Primitive(Primitive::Bytes)));
        let scalar = ValueNode {
            ty: number,
            kind: ValueKind::Constant(Constant::Integer(7)),
        };
        let text = ValueNode {
            ty: string,
            kind: ValueKind::Constant(Constant::String("saved".into())),
        };
        let (kind, value, expected) = match shape {
            0 => (
                TypeKind::Primitive(Primitive::U32),
                scalar.kind.clone(),
                Value::Integer(hirsel_interp::Integer::new(Primitive::U32, 7).unwrap()),
            ),
            1 => (
                TypeKind::Primitive(Primitive::String),
                text.kind.clone(),
                Value::String("saved".into()),
            ),
            2 => (
                TypeKind::Primitive(Primitive::Bytes),
                ValueKind::Constant(Constant::Bytes(vec![1, 2])),
                Value::Bytes(vec![1, 2]),
            ),
            3 => (
                TypeKind::Tuple(vec![number, string].into()),
                ValueKind::Construct {
                    constructor: Constructor::Tuple,
                    fields: vec![scalar.clone(), text.clone()],
                },
                Value::Tuple(vec![
                    Some(Value::Integer(
                        hirsel_interp::Integer::new(Primitive::U32, 7).unwrap(),
                    )),
                    Some(Value::String("saved".into())),
                ]),
            ),
            4 => (
                TypeKind::Box(string),
                ValueKind::Construct {
                    constructor: Constructor::Box,
                    fields: vec![text.clone()],
                },
                Value::Box(vec![Some(Value::String("saved".into()))]),
            ),
            5 => (
                TypeKind::Builtin {
                    kind: BuiltinType::List,
                    arguments: vec![GenericArgument::Type(string)].into(),
                },
                ValueKind::Construct {
                    constructor: Constructor::Sequence,
                    fields: vec![text.clone()],
                },
                Value::Sequence {
                    kind: BuiltinType::List,
                    elements: vec![Some(Value::String("saved".into()))],
                },
            ),
            6 => (
                TypeKind::Builtin {
                    kind: BuiltinType::Set,
                    arguments: vec![GenericArgument::Type(number)].into(),
                },
                ValueKind::Construct {
                    constructor: Constructor::Set,
                    fields: vec![scalar.clone()],
                },
                Value::Set(vec![Some(Value::Integer(
                    hirsel_interp::Integer::new(Primitive::U32, 7).unwrap(),
                ))]),
            ),
            7 => {
                let pair = b
                    .types
                    .push(ty(TypeKind::Tuple(vec![number, string].into())));
                let entry = ValueNode {
                    ty: pair,
                    kind: ValueKind::Construct {
                        constructor: Constructor::Tuple,
                        fields: vec![scalar.clone(), text.clone()],
                    },
                };
                (
                    TypeKind::Builtin {
                        kind: BuiltinType::Map,
                        arguments: vec![
                            GenericArgument::Type(number),
                            GenericArgument::Type(string),
                        ]
                        .into(),
                    },
                    ValueKind::Construct {
                        constructor: Constructor::Map,
                        fields: vec![entry],
                    },
                    Value::Map(vec![Some(Value::Tuple(vec![
                        Some(Value::Integer(
                            hirsel_interp::Integer::new(Primitive::U32, 7).unwrap(),
                        )),
                        Some(Value::String("saved".into())),
                    ]))]),
                )
            }
            9 => {
                let function = FunctionType {
                    state_promises: Default::default(),
                    generics: vec![].into(),
                    parameters: vec![].into(),
                    result: b.body.result,
                    error: None,
                    allocation: Allocation::NoAlloc,
                    authority: AuthoritySummary::inert(),
                    uses: Uses::Only(AuthoritySummary::inert()),
                    contracts: vec![].into(),
                    incomplete: false,
                    foreign: None,
                };
                (
                    TypeKind::Function(function),
                    ValueKind::FunctionReference(path("target")),
                    Value::FunctionReference(hirsel_interp::value::NamedFunction {
                        definition: path("target"),
                        arguments: vec![],
                        types: Arc::new(b.types.clone()),
                        ty: b.body.result,
                    }),
                )
            }
            _ => (
                TypeKind::Primitive(Primitive::Bool),
                ValueKind::Constant(Constant::Bool(true)),
                Value::Bool(true),
            ),
        };
        let _ = (boolean, bytes);
        let root = b.types.push(ty(kind));
        let data = Arc::new(Materialized {
            types: b.types.clone(),
            root: ValueNode {
                ty: root,
                kind: value,
            },
        });
        let first = b.slot(root, Storage::Temporary);
        let second = b.slot(root, Storage::Temporary);
        for destination in [first, second] {
            b.assign(
                destination,
                Rvalue::ReadConstant {
                    definition: path("DATA"),
                },
                1,
                CleanupId(0),
            );
        }
        let pair = b.types.push(ty(TypeKind::Tuple(vec![root, root].into())));
        let result = b.slot(pair, Storage::Temporary);
        b.assign(
            result,
            Rvalue::Construct {
                constructor: Constructor::Tuple,
                fields: vec![
                    ConstructField {
                        field: 0,
                        value: take(first),
                    },
                    ConstructField {
                        field: 1,
                        value: take(second),
                    },
                ],
                destination: Destination::Inferred,
            },
            1,
            CleanupId(0),
        );
        let mut provider = ConstantsProvider {
            definition: Arc::new(b.finish("main", result)),
            data,
            requests: vec![],
        };
        let execution = run(
            &mut provider,
            &sources(),
            &RunConfiguration {
                entry: path("main"),
                hash_seed: 0,
                inputs: vec![],
            },
            vec![],
            Default::default(),
        )
        .unwrap();
        assert_eq!(provider.requests, ["constant", "entry"]);
        assert!(
            matches!(execution.outcome, Outcome::Returned { value: Value::Tuple(ref values), .. } if values == &vec![Some(expected.clone()),Some(expected)])
        );
    }
}

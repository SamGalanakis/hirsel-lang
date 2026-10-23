//! Runtime placement controls; these hand-built fixtures do not claim V proof.
use crate::support::*;
use hirsel_interp::{Outcome, Value};
use hirsel_tir::*;
use hirsel_types::*;

#[test]
fn literal_bound_is_not_an_arena_allocation() {
    for (primitive, literal, expected) in [
        (
            Primitive::Bytes,
            Constant::Bytes(b"abc".to_vec()),
            Value::Bytes(b"abc".to_vec()),
        ),
        (
            Primitive::String,
            Constant::String("abc".into()),
            Value::String("abc".into()),
        ),
    ] {
        let mut b = Builder::new();
        b.borrowed_parameter(TypeKind::Named {
            definition: hirsel_syntax::DefId::intern(&hirsel_syntax::DefPath::core("Arena")),
            arguments: vec![].into(),
        });
        let mut row = Type::with_kind(TypeKind::Primitive(primitive));
        row.modes.relation = Relation::parameters([ParameterRef::Body {
            scope: b.body.parameter_scope,
            parameter: ParamId(0),
        }]);
        let ty = b.types.intern(row);
        let result = b.slot(ty, Storage::Temporary);
        b.assign(result, Rvalue::Constant(literal), 1, CleanupId(0));
        // No open arena inventory: a literal must never try to enroll in one.
        let execution = execute_unvalidated(
            vec![b.finish("main", result)],
            vec![Value::Handle {
                kind: "Arena".into(),
                id: 41,
            }],
            Default::default(),
        );
        let Outcome::Returned { value, .. } = execution.outcome else {
            panic!("literal construction failed: {execution:?}");
        };
        assert_eq!(value, expected);
    }
}

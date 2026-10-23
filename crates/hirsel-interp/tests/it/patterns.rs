//! Typed pattern tables select arms; explicit instructions commit binders.
use crate::support;
use hirsel_interp::{Integer, Limits, Outcome, Value};
use hirsel_tir::*;
use hirsel_types::{Primitive as P, TypeKind};
use support::*;
#[test]
fn literal_patterns_include_signed_wide_integer_float_bool_string_and_bytes() {
    let cases = [
        (
            TypeKind::Primitive(P::I128),
            Constant::Integer(u128::MAX),
            Value::Integer(Integer::new(P::I128, u128::MAX).unwrap()),
        ),
        (
            TypeKind::Primitive(P::U128),
            Constant::Integer(u128::MAX),
            Value::Integer(Integer::new(P::U128, u128::MAX).unwrap()),
        ),
        (
            TypeKind::Primitive(P::F64),
            Constant::F64(1.25f64.to_bits()),
            Value::F64(1.25f64.to_bits()),
        ),
        (
            TypeKind::Primitive(P::Bool),
            Constant::Bool(true),
            Value::Bool(true),
        ),
        (
            TypeKind::Primitive(P::String),
            Constant::String("é".into()),
            Value::String("é".into()),
        ),
        (
            TypeKind::Primitive(P::Bytes),
            Constant::Bytes(vec![0, 255]),
            Value::Bytes(vec![0, 255]),
        ),
    ];
    for (kind, constant, input) in cases {
        let mut b = Builder::new();
        let p = b.parameter(kind.clone());
        b.body.matches.push(MatchTable {
            scrutinee: p,
            patterns: vec![
                MatchPattern {
                    place: p,
                    kind: MatchPatternKind::Literal(constant),
                },
                MatchPattern {
                    place: p,
                    kind: MatchPatternKind::Wildcard,
                },
            ],
            arms: vec![
                MatchArm {
                    pattern: MatchPatternId(0),
                    guard: None,
                    target: BlockId(1),
                },
                MatchArm {
                    pattern: MatchPatternId(1),
                    guard: None,
                    target: BlockId(2),
                },
            ],
        });
        b.block(
            Terminator::Match {
                table: MatchTableId(0),
                from: 0,
            },
            2,
        );
        let result = b.constant(TypeKind::Primitive(P::Bool), Constant::Bool(true));
        b.block(
            Terminator::Return {
                value: take(result),
                cleanup: CleanupId(0),
            },
            3,
        );
        let no = b.constant(TypeKind::Primitive(P::Bool), Constant::Bool(false));
        b.block(
            Terminator::Return {
                value: take(no),
                cleanup: CleanupId(0),
            },
            4,
        );
        assert!(matches!(
            execute(vec![b.definition("main")], vec![input], Limits::default()).outcome,
            Outcome::Returned {
                value: Value::Bool(true),
                ..
            }
        ));
    }
}

#[test]
fn match_lowering_22_guard_fallthrough_restarts_at_the_next_ordered_arm() {
    for subject in [false, true] {
        for guard in [false, true] {
            let mut b = Builder::new();
            let value = b.parameter(TypeKind::Primitive(P::Bool));
            let condition = b.parameter(TypeKind::Primitive(P::Bool));
            b.body.matches.push(MatchTable {
                scrutinee: value,
                patterns: vec![
                    MatchPattern {
                        place: value,
                        kind: MatchPatternKind::Wildcard,
                    },
                    MatchPattern {
                        place: value,
                        kind: MatchPatternKind::Literal(Constant::Bool(true)),
                    },
                ],
                arms: vec![
                    MatchArm {
                        pattern: MatchPatternId(0),
                        guard: Some(BlockId(1)),
                        target: BlockId(2),
                    },
                    MatchArm {
                        pattern: MatchPatternId(1),
                        guard: None,
                        target: BlockId(4),
                    },
                    MatchArm {
                        pattern: MatchPatternId(0),
                        guard: None,
                        target: BlockId(5),
                    },
                ],
            });
            b.block(
                Terminator::Match {
                    table: MatchTableId(0),
                    from: 0,
                },
                2,
            );
            b.block(
                Terminator::Branch {
                    condition: copy(condition),
                    yes: BlockId(2),
                    no: BlockId(3),
                },
                3,
            );
            let first = b.int(P::U32, 11);
            b.block(
                Terminator::Return {
                    value: take(first),
                    cleanup: CleanupId(0),
                },
                4,
            );
            b.block(
                Terminator::Match {
                    table: MatchTableId(0),
                    from: 1,
                },
                5,
            );
            for marker in [22, 33] {
                let result = b.int(P::U32, marker);
                b.block(
                    Terminator::Return {
                        value: take(result),
                        cleanup: CleanupId(0),
                    },
                    6,
                );
            }
            let expected = if guard {
                11
            } else if subject {
                22
            } else {
                33
            };
            let execution = execute(
                vec![b.definition("main")],
                vec![Value::Bool(subject), Value::Bool(guard)],
                Limits::default(),
            );
            assert!(
                matches!(execution.outcome, Outcome::Returned { value: Value::Integer(n), .. } if n == Integer::new(P::U32, expected).unwrap()),
                "{execution:?}"
            );
        }
    }
}

//! Typed pattern tables select arms; explicit instructions commit binders.
use crate::support;
use hirsel_interp::{Integer, Limits, Outcome, Value};
use hirsel_tir::*;
use hirsel_types::{Primitive as P, TypeKind};
use support::*;

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

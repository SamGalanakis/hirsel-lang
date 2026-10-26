//! LIR schema revision 31 (FAIL-2): `task.isolate`'s catch. Each mutation is
//! paired with its valid twin and is rejected by the rule it names; with that
//! rule removed, C accepts it.
use crate::support;

use hirsel_core_contract::Passing;
use hirsel_lir::effect::Opcode;
use hirsel_lir::ir::*;
use hirsel_lir::stage::{Rc, Stage, Value};
use support::*;

#[derive(Clone, Copy, Eq, PartialEq)]
enum Mutant {
    None,
    /// A start that is not a u64 record count.
    Start,
    /// A catch in ordinary code, reached by an ordinary edge.
    Ordinary,
    /// A catch whose caught edge stays in failure cleanup.
    Unresumed,
    /// A catch in a source unit.
    Source,
    Parameter,
    Constant,
    WrongBegin,
    DirectImpostor,
    Forwarded,
    MissingEnd,
    DoubleEnd,
    InfallibleRelease,
    FallibleRelease,
}

/// `task.isolate`'s shape: IsolateBegin returns its mark, the body fails,
/// and its cleanup catches the failures since that count and returns.
fn isolated(m: Mutant) -> Vec<K> {
    if matches!(m, Mutant::InfallibleRelease | Mutant::FallibleRelease) {
        isolated_stage::<Rc>(m)
    } else {
        isolated_stage::<Value>(m)
    }
}

fn isolated_stage<S: Stage>(m: Mutant) -> Vec<K> {
    let released = match m {
        Mutant::InfallibleRelease => Some(l::BLK),
        Mutant::FallibleRelease => Some(l::RESBLK),
        _ => None,
    };
    let mut parameters = vec![(Passing::Borrow, l::U64)];
    parameters.extend(released.map(|layout| (Passing::Take, layout)));
    let mut t = Lir::<S>::new(&parameters, None);
    if m != Mutant::Source {
        t.builder.unit.header.origin = UnitOrigin::Library;
    }
    let count = t.plain(l::U64);
    t.param(count);
    let after = t.builder.block(BlockKind::Body, &[]);
    let propagate = t.builder.propagate();
    let owner = released.map(|layout| {
        let owner = t.owned(layout);
        t.param(owner);
        owner
    });
    let begin_failed = match owner {
        Some(owner) => t.releasing(&[owner]),
        None => propagate,
    };
    let params = [(Passing::Borrow, l::U64)];
    let abi = t.abi(&params, Some(l::U64));
    let name = if m == Mutant::WrongBegin {
        "$intrinsic.IsolateCount"
    } else {
        "$intrinsic.IsolateBegin"
    };
    let signature = super::operation_abi::scalar_signature(
        name,
        &[(
            hirsel_syntax::PassingMode::Borrow,
            hirsel_types::Primitive::U64,
        )],
        Some(hirsel_types::Primitive::U64),
    );
    t.declare_operation(signature, Default::default(), abi);
    let callee = if m == Mutant::DirectImpostor {
        let symbol = t.symbol(SymbolKind::Function);
        t.builder.unit.tables.symbols[symbol.index()].definition =
            hirsel_syntax::DefPath::core(name);
        Callee::Direct(symbol)
    } else {
        Callee::Operation(OperationRef(0))
    };
    let begin_normal = EdgeId(t.builder.unit.tables.edges.len() as u32);
    let mark = t
        .call_to(
            ENTRY,
            callee,
            abi,
            &params,
            Some(l::U64),
            &[ArgumentValue::Value(count)],
            (after, &[]),
            begin_failed,
        )
        .unwrap();
    if let Some(owner) = owner {
        // Only the release's failure bypasses End. Its normal path still
        // reaches the ordinary catch and retires the invocation exactly once.
        let failure = t.failure(propagate);
        t.release_with(after, owner, failure);
    }
    let start = if m == Mutant::Start {
        t.int(after)
    } else if m == Mutant::Parameter {
        count
    } else if m == Mutant::Constant {
        let result = t.plain(l::U64);
        let ty = t.builder.unit.values[result.index()].ty;
        let constant = ConstantId(t.builder.unit.tables.constants.len() as u32);
        t.builder.unit.tables.constants.push(ConstantData {
            ty,
            value: ConstantValue::Integer(0),
        });
        t.builder.push(after, Inst::Const { result, constant });
        result
    } else if m == Mutant::Forwarded {
        let result = t.plain(l::U64);
        t.builder.unit.blocks[after.index()].parameters =
            PoolRange::push(&mut t.builder.unit.pools.values, [result]);
        t.builder.unit.tables.edges[begin_normal.index()].arguments =
            PoolRange::push(&mut t.builder.unit.pools.values, [mark]);
        result
    } else {
        mark
    };
    let begin_operation = t.builder.unit.tables.operations[0].clone();
    let begin_registry = t.operations.clone();
    let begin_row = t.manifest.rows().next().unwrap().clone();
    let end_abi = t.abi(&params, None);
    let end_signature = super::operation_abi::scalar_signature(
        "$intrinsic.IsolateEnd",
        &[(
            hirsel_syntax::PassingMode::Borrow,
            hirsel_types::Primitive::U64,
        )],
        None,
    );
    t.declare_operation(end_signature, Default::default(), end_abi);
    t.operations.signatures.extend(begin_registry.signatures);
    let end_operation = t.builder.unit.tables.operations[0].clone();
    let end_row = t.manifest.rows().next().unwrap().clone();
    t.builder.unit.tables.operations = vec![begin_operation.clone(), end_operation.clone()];
    t.manifest = hirsel_core_contract::Manifest::skeleton([begin_operation, end_operation]);
    t.manifest.replace(begin_row).unwrap();
    t.manifest.replace(end_row).unwrap();
    let caught = if m == Mutant::Unresumed {
        t.builder.propagate()
    } else {
        let caught = t.builder.block(BlockKind::Body, &[]);
        let mut returning = caught;
        if m != Mutant::MissingEnd {
            for _ in 0..if m == Mutant::DoubleEnd { 2 } else { 1 } {
                let next = t.builder.block(BlockKind::Body, &[]);
                t.call_to(
                    returning,
                    Callee::Operation(OperationRef(1)),
                    end_abi,
                    &params,
                    None,
                    &[ArgumentValue::Value(count)],
                    (next, &[]),
                    propagate,
                );
                returning = next;
            }
        }
        t.builder.ret(returning, None);
        caught
    };
    let kind = if m == Mutant::Ordinary {
        BlockKind::Body
    } else {
        BlockKind::FailureCleanup
    };
    let catch = t.builder.block(kind, &[]);
    let caught = t.builder.edge(caught, &[]);
    t.builder.term(catch, Terminator::Isolate { start, caught });
    if m == Mutant::Ordinary {
        t.builder.jump(after, catch, &[]);
    } else {
        let raise = t.failure(catch);
        t.builder.unit.tables.failures[raise.index()].rule = Some("SEM-1".into());
        t.builder.term(after, Terminator::Raise(raise));
    }
    t.check()
}

#[test]
fn an_isolation_ends_once_on_its_terminal_path() {
    pair(
        |m| isolated(if m { Mutant::MissingEnd } else { Mutant::None }),
        |k| *k == K::Placement("isolation terminal path without End"),
    );
    pair(
        |m| isolated(if m { Mutant::DoubleEnd } else { Mutant::None }),
        |k| *k == K::Placement("isolation End without its active begin"),
    );
}

#[test]
fn only_descriptor_proved_infallible_release_edges_may_skip_end() {
    pair(
        |m| {
            isolated(if m {
                Mutant::FallibleRelease
            } else {
                Mutant::InfallibleRelease
            })
        },
        |k| *k == K::Placement("isolation terminal path without End"),
    );
}

#[test]
fn an_isolation_mark_is_the_exact_intrinsic_begin_result() {
    for mutant in [
        Mutant::Parameter,
        Mutant::Constant,
        Mutant::WrongBegin,
        Mutant::DirectImpostor,
        Mutant::Forwarded,
    ] {
        pair(
            |m| isolated(if m { mutant } else { Mutant::None }),
            |k| *k == K::Placement("isolation mark is not an IsolateBegin result"),
        );
    }
}

#[test]
fn an_isolation_starts_from_a_u64_count() {
    pair(
        |m| isolated(if m { Mutant::Start } else { Mutant::None }),
        |k| *k == K::Type("isolation start"),
    );
}

#[test]
fn only_a_library_body_isolates() {
    pair(
        |m| isolated(if m { Mutant::Source } else { Mutant::None }),
        |k| *k == K::Vocabulary(Opcode::Isolate),
    );
}

#[test]
fn an_isolation_catches_in_failure_cleanup() {
    pair(
        |m| isolated(if m { Mutant::Ordinary } else { Mutant::None }),
        |k| *k == K::BlockKind("isolate outside failure cleanup"),
    );
}

#[test]
fn an_isolation_resumes_ordinary_code() {
    pair(
        |m| isolated(if m { Mutant::Unresumed } else { Mutant::None }),
        |k| *k == K::BlockKind("failure path transfer"),
    );
}

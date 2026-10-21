//! Private compiler proofs for spawn fast paths. Authority summaries are never
//! used as op-free evidence. Unknown calls, destruction and root-bearing values
//! conservatively refuse; direct-call dependencies are solved to a fixed point.
use crate::insert::Lay;
use hirsel_lir::{ir::*, layout_types::LayoutTable, stage::Value};
use std::collections::HashMap;

pub(crate) use hirsel_lir::runtime_abi::{
    SPAWN_IMMEDIATE_AWAIT as IMMEDIATE, SPAWN_OP_FREE as OP_FREE,
};

pub(crate) fn op_free(units: &[LirUnit<Value>], table: &LayoutTable) -> HashMap<Symbol, bool> {
    let symbols: Vec<_> = units
        .iter()
        .map(|u| Symbol {
            kind: SymbolKind::Function,
            definition: u.header.definition.clone(),
            instance: Some(u.header.instance),
        })
        .collect();
    let indices: HashMap<_, _> = symbols
        .iter()
        .cloned()
        .enumerate()
        .map(|(i, s)| (s, i))
        .collect();
    let mut safe = vec![true; units.len()];
    let mut users = vec![Vec::new(); units.len()];
    let lay = Lay(table);
    for (index, u) in units.iter().enumerate() {
        safe[index] = u
            .values
            .iter()
            .all(|v| !lay.anchored(u.types[v.ty.index()].layout))
            && u.instructions.iter().all(|i| {
                matches!(
                    i,
                    Inst::Const { .. }
                        | Inst::FunctionRef { .. }
                        | Inst::ReadStatic { .. }
                        | Inst::Unary { .. }
                        | Inst::Binary { .. }
                        | Inst::Compare { .. }
                        | Inst::Convert { .. }
                        | Inst::IntArith { .. }
                        | Inst::MulAdd { .. }
                        | Inst::ScalarUnary { .. }
                        | Inst::Check { .. }
                        | Inst::Read { .. }
                        | Inst::Copy { .. }
                        | Inst::Store { .. }
                        | Inst::Replace { .. }
                        | Inst::StorageLive { .. }
                        | Inst::StorageDead { .. }
                        | Inst::EndScope { .. }
                        | Inst::Address { .. }
                        | Inst::Construct { .. }
                        | Inst::Closure { .. }
                        | Inst::Weaken { .. }
                        | Inst::BeginLoan { .. }
                        | Inst::EndLoan { .. }
                        | Inst::BindOrigin { .. }
                        | Inst::BindFields { .. }
                        | Inst::EndOrigin { .. }
                        | Inst::Drop { .. }
                        | Inst::Recast { .. }
                )
            })
            && table.fn_abis[u.header.abi.0 as usize]
                .result
                .as_ref()
                .is_none_or(|r| lay.trivial(r.layout));
        for call in &u.tables.calls {
            match call.callee {
                Callee::Direct(s) | Callee::Bound { code: s, .. } => {
                    if let Some(&callee) = indices.get(&u.tables.symbols[s.index()]) {
                        users[callee].push(index);
                    } else {
                        safe[index] = false;
                    }
                }
                Callee::Indirect(_) | Callee::Operation(_) => safe[index] = false,
            }
        }
    }
    // Same widening/worklist structure as closure_calls::discover: each body
    // changes at most once. A recursive pure SCC is safe; one op poisons it.
    let mut queue: std::collections::VecDeque<_> = safe
        .iter()
        .enumerate()
        .filter_map(|(i, s)| (!s).then_some(i))
        .collect();
    while let Some(callee) = queue.pop_front() {
        for &user in &users[callee] {
            if std::mem::replace(&mut safe[user], false) {
                queue.push_back(user);
            }
        }
    }
    symbols.into_iter().zip(safe).collect()
}

/// A narrow normal-path proof. The first call after spawn must await its exact
/// SSA result. No branch, drop, computation, or other operation is crossed.
pub(crate) fn immediately_awaited(u: &LirUnit<Value>, call: usize) -> bool {
    let Some(value) = u.tables.calls[call].result else {
        return false;
    };
    let Some(normal) = u.terminators.iter().find_map(|t| match t {
        Terminator::Call {
            call: id, normal, ..
        } if id.index() == call => *normal,
        _ => None,
    }) else {
        return false;
    };
    let mut next_edge = normal;
    let mut visited = vec![false; u.blocks.len()];
    let mut same = vec![false; u.values.len()];
    same[value.index()] = true;
    loop {
        let edge = &u.tables.edges[next_edge.index()];
        let at = edge.target.index();
        if std::mem::replace(&mut visited[at], true) {
            return false;
        }
        let block = &u.blocks[at];
        for (&to, &from) in block
            .parameters
            .get(&u.pools.values)
            .unwrap_or(&[])
            .iter()
            .zip(edge.arguments.get(&u.pools.values).unwrap_or(&[]))
        {
            same[to.index()] = same[from.index()];
        }
        for i in block.instructions.get(&u.instructions).unwrap_or(&[]) {
            match *i {
                Inst::Read {
                    result,
                    address,
                    reaching,
                    ..
                } if whole(u, address) => same[result.index()] = same[reaching.index()],
                Inst::Store {
                    version,
                    address,
                    value,
                } if whole(u, address) => same[version.index()] = same[value.index()],
                Inst::Weaken { result, value } => same[result.index()] = same[value.index()],
                Inst::Const { .. }
                | Inst::Address { .. }
                | Inst::StorageLive { .. }
                | Inst::StorageDead { .. }
                | Inst::BeginLoan { .. }
                | Inst::EndLoan { .. }
                | Inst::BindOrigin { .. }
                | Inst::BindFields { .. }
                | Inst::EndOrigin { .. }
                | Inst::EndScope { .. } => {}
                _ => return false,
            }
        }
        match u.terminators[at] {
            Terminator::Jump(edge) => next_edge = edge,
            Terminator::Call { call, .. } => {
                let next = &u.tables.calls[call.index()];
                let Callee::Direct(symbol) = next.callee else {
                    return false;
                };
                return u.tables.symbols[symbol.index()]
                    .definition
                    .is_core("task.await")
                    && next
                        .arguments
                        .get(&u.pools.call_arguments)
                        .unwrap_or(&[])
                        .iter()
                        .any(|a| {
                            a.parameter == 1
                                && matches!(a.value, ArgumentValue::Value(v) if same[v.index()])
                        });
            }
            _ => return false,
        }
    }
}

fn whole(u: &LirUnit<Value>, address: AddressId) -> bool {
    let a = &u.addresses[address.index()];
    a.projection.len == 0
        && match a.base {
            AddressBase::Slot(_) => true,
            AddressBase::Address(a) => whole(u, a),
            AddressBase::Value(_) => false,
        }
}

pub(crate) fn set_flags(u: &mut LirUnit<Value>, flags: u64) {
    for call in &u.tables.calls {
        let Callee::Operation(op) = call.callee else {
            continue;
        };
        if !u.tables.operations[op.index()]
            .definition
            .is_core("$intrinsic.TaskSpawn")
        {
            continue;
        }
        let Some(argument) = call
            .arguments
            .get(&u.pools.call_arguments)
            .unwrap_or(&[])
            .iter()
            .find(|a| a.parameter == 2)
        else {
            continue;
        };
        let ArgumentValue::Value(v) = argument.value else {
            continue;
        };
        let Some(Inst::Const { constant, .. }) = u
            .instructions
            .iter_mut()
            .find(|i| matches!(i, Inst::Const { result, .. } if *result == v))
        else {
            continue;
        };
        let mut data = u.tables.constants[constant.index()].clone();
        data.value = ConstantValue::Integer(u128::from(flags));
        *constant = ConstantId(u.tables.constants.len() as u32);
        u.tables.constants.push(data);
    }
}

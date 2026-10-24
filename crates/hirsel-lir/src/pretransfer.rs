//! The private keyed-call adapter protocol. Target possibility is derived from
//! code flow and an independently supplied materialized-adapter inventory.
use crate::{LayoutTable, abi_types::FnAbiId, ir::*, stage::Stage};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AdapterReference {
    pub adapter: Symbol,
    pub primitive: Symbol,
    pub abi: FnAbiId,
}
hirsel_codec::snapshot_struct!(AdapterReference {
    adapter,
    primitive,
    abi
});

pub fn primitive(path: &hirsel_syntax::DefPath) -> Option<hirsel_syntax::DefPath> {
    ["map.insert", "set.insert", "map.get", "set.contains"]
        .into_iter()
        .find(|name| *path == adapter_path(name))
        .map(hirsel_syntax::DefPath::core)
}
pub fn adapter_path(primitive: &str) -> hirsel_syntax::DefPath {
    hirsel_syntax::DefPath::core(&format!("$native.keyed_{}", primitive.replace('.', "_")))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Targets {
    Pending,
    Ordinary,
    Adapter,
    Unknown,
}
impl Targets {
    fn join(self, other: Self) -> Self {
        match (self, other) {
            (Self::Pending, x) | (x, Self::Pending) => x,
            (a, b) if a == b => a,
            _ => Self::Unknown,
        }
    }
}
fn whole<S>(unit: &LirUnit<S>, address: AddressId) -> bool {
    let a = &unit.addresses[address.index()];
    a.projection.len == 0
        && match a.base {
            AddressBase::Slot(_) => true,
            AddressBase::Address(a) => whole(unit, a),
            AddressBase::Value(_) => false,
        }
}
fn compatible(table: &LayoutTable, a: FnAbiId, b: FnAbiId) -> bool {
    let (Some(a), Some(b)) = (
        table.fn_abis.get(a.0 as usize),
        table.fn_abis.get(b.0 as usize),
    ) else {
        return false;
    };
    a.parameters == b.parameters && a.result == b.result && a.diverges == b.diverges
}

/// No type alone establishes that a keyed adapter exists. Opaque values use
/// only concrete, independently materialized targets with compatible formals.
/// Known ordinary references stay ordinary through SSA transport and joins.
pub fn required<S: Stage>(
    unit: &LirUnit<S>,
    table: &LayoutTable,
    inventory: &[AdapterReference],
) -> Vec<bool> {
    let is_adapter = |symbol: &Symbol| inventory.iter().any(|r| &r.adapter == symbol);
    let mut constants = vec![Targets::Unknown; unit.values.len()];
    let mut inputs = vec![Vec::new(); unit.values.len()];
    for edge in &unit.tables.edges {
        for (&parameter, &value) in unit.blocks[edge.target.index()]
            .parameters
            .get(&unit.pools.values)
            .unwrap_or(&[])
            .iter()
            .zip(edge.arguments.get(&unit.pools.values).unwrap_or(&[]))
        {
            constants[parameter.index()] = Targets::Pending;
            inputs[parameter.index()].push(value);
        }
    }
    for inst in &unit.instructions {
        let transfer = match *inst {
            Inst::Read {
                result,
                address,
                reaching,
                ..
            }
            | Inst::Copy {
                result,
                address,
                reaching,
                ..
            } if whole(unit, address) => Some((result, reaching)),
            Inst::Store {
                address,
                value,
                version,
            }
            | Inst::Replace {
                address,
                value,
                version,
                ..
            } if whole(unit, address) => Some((version, value)),
            Inst::Weaken { result, value } | Inst::CopyValue { result, value, .. } => {
                Some((result, value))
            }
            _ => None,
        };
        if let Some((to, from)) = transfer {
            constants[to.index()] = Targets::Pending;
            inputs[to.index()].push(from);
        } else {
            match *inst {
                Inst::FunctionRef { result, symbol } => {
                    constants[result.index()] = if is_adapter(&unit.tables.symbols[symbol.index()])
                    {
                        Targets::Adapter
                    } else {
                        Targets::Ordinary
                    }
                }
                Inst::Closure { result, .. } => constants[result.index()] = Targets::Ordinary,
                _ => (),
            }
        }
    }
    let mut targets = constants.clone();
    loop {
        let mut changed = false;
        for i in 0..targets.len() {
            let next = inputs[i]
                .iter()
                .fold(constants[i], |s, v| s.join(targets[v.index()]));
            if next != targets[i] {
                targets[i] = next;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    unit.tables
        .calls
        .iter()
        .map(|call| match call.callee {
            Callee::Operation(_) => false,
            Callee::Direct(s) | Callee::Bound { code: s, .. } => {
                is_adapter(&unit.tables.symbols[s.index()])
            }
            Callee::Indirect(v) => match targets[v.index()] {
                Targets::Ordinary => false,
                Targets::Adapter => true,
                Targets::Pending | Targets::Unknown => {
                    inventory.iter().any(|r| compatible(table, call.abi, r.abi))
                }
            },
        })
        .collect()
}

pub fn check_edges<S: Stage>(
    unit: &LirUnit<S>,
    table: &LayoutTable,
    inventory: &[AdapterReference],
) -> Result<(), String> {
    for (call, required) in unit
        .tables
        .calls
        .iter()
        .zip(required(unit, table, inventory))
    {
        if call.pretransfer_failure.is_some() != required {
            return Err(if required {
                "missing pretransfer edge on possible keyed adapter dispatch"
            } else {
                "extra pretransfer edge on ordinary dispatch"
            }
            .into());
        }
        if let Some(f) = call.pretransfer_failure {
            let failure = &unit.tables.failures[f.index()];
            if failure.rule.is_some()
                || failure
                    .dispose
                    .get(&unit.pools.values)
                    .unwrap_or(&[])
                    .iter()
                    .any(|v| unit.types[unit.values[v.index()].ty.index()].repr != ValueRepr::Token)
            {
                return Err("pretransfer edge must preserve all pending actual credits".into());
            }
        }
    }
    Ok(())
}

/// Validate the compiler adapter's entire gate and handoff CFG. The inventory
/// comes from instance planning; a candidate header cannot grant this role.
pub fn check_adapter<S: Stage>(
    unit: &LirUnit<S>,
    table: &LayoutTable,
    inventory: &[AdapterReference],
) -> Result<bool, String> {
    use std::collections::{BTreeMap, VecDeque};
    let reference = inventory.iter().find(|r| {
        r.adapter.definition == unit.header.definition
            && r.adapter.instance == Some(unit.header.instance)
    });
    let has_reject = unit
        .terminators
        .iter()
        .any(|t| *t == Terminator::RejectTransfer);
    let Some(reference) = reference else {
        if has_reject || primitive(&unit.header.definition).is_some() {
            return Err(
                "private status 3 outside an independently materialized keyed adapter".into(),
            );
        }
        return Ok(false);
    };
    if unit.header.origin != UnitOrigin::Library || unit.header.abi != reference.abi || !has_reject
    {
        return Err("keyed adapter declaration or private rejection terminal differs".into());
    }
    let abi = &table.fn_abis[reference.abi.0 as usize];
    let key = abi
        .parameters
        .iter()
        .find(|p| p.source == 1)
        .ok_or("adapter key formal is absent")?;
    let parameter = |address: AddressId| -> Option<u32> {
        let a = &unit.addresses[address.index()];
        if a.projection.len != 0 {
            return None;
        }
        let AddressBase::Slot(slot) = a.base else {
            return None;
        };
        match unit.slots[slot.index()].kind {
            SlotKind::Parameter(n) => Some(n),
            _ => None,
        }
    };
    let key_value = |value: ValueId| {
        unit.instructions.iter().any(|i| matches!(*i,
        Inst::Read {result, address, mode:ReadMode::Borrow, ..} if result == value && parameter(address)==Some(1)))
    };
    if unit
        .instructions
        .iter()
        .filter(|i| matches!(i, Inst::Hash { .. }))
        .count()
        != 1
        || unit.tables.calls.len() != 1
    {
        return Err("adapter must have one key gate and one successful primitive handoff".into());
    }
    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum State {
        Pending,
        Gated,
        Handed,
        Rejected,
        Failed,
    }
    let mut incoming = BTreeMap::from([(unit.entry, State::Pending)]);
    let mut pending = VecDeque::from([unit.entry]);
    while let Some(block) = pending.pop_front() {
        let mut state = incoming[&block];
        let mut outgoing = Vec::new();
        for inst in unit.blocks[block.index()]
            .instructions
            .get(&unit.instructions)
            .unwrap_or(&[])
        {
            let before = state;
            if matches!(inst, Inst::FrameEnter { .. } | Inst::FrameLeave) {
                return Err("compiler adapter adds a semantic activation".into());
            }
            if state == State::Pending {
                match *inst {
                    Inst::Address { address, .. } if parameter(address).is_some() => (),
                    Inst::BeginLoan {
                        address,
                        mutable: false,
                        ..
                    } if parameter(address) == Some(1) => (),
                    Inst::Read {
                        address,
                        mode: ReadMode::Borrow,
                        ..
                    } if parameter(address) == Some(1) => (),
                    Inst::Hash {
                        value,
                        layout,
                        failure,
                        ..
                    } if layout == key.layout
                        && key_value(value)
                        && unit.tables.failures[failure.index()].rule.as_deref()
                            == Some("MEM-3") =>
                    {
                        state = State::Gated
                    }
                    _ => {
                        return Err(
                            "adapter transfers or changes an actual before key admission".into(),
                        );
                    }
                }
            } else if state == State::Rejected && !matches!(inst, Inst::EndLoan { .. }) {
                return Err("rejected adapter disposes or changes pending actuals".into());
            }
            if let Some(f) = inst.failure() {
                let failure = &unit.tables.failures[f.index()];
                if before == State::Pending && failure.dispose.len != 0 {
                    return Err("gate rejection disposes pending actuals".into());
                }
                outgoing.push((
                    failure.cleanup,
                    if before == State::Pending {
                        State::Rejected
                    } else {
                        State::Failed
                    },
                ));
            }
        }
        let term = &unit.terminators[block.index()];
        match *term {
            Terminator::Call { call, .. } => {
                let call = &unit.tables.calls[call.index()];
                let Callee::Direct(symbol) = call.callee else {
                    return Err(
                        "adapter handoff is not its independently selected primitive".into(),
                    );
                };
                if state != State::Gated
                    || unit.tables.symbols[symbol.index()] != reference.primitive
                    || !call.forward_caller
                    || call.pretransfer_failure.is_some()
                {
                    return Err(
                        "adapter must hand off once after admission to its selected primitive"
                            .into(),
                    );
                }
                for argument in call
                    .arguments
                    .get(&unit.pools.call_arguments)
                    .unwrap_or(&[])
                {
                    let source = match argument.value {
                        ArgumentValue::Address { address, .. } => parameter(address),
                        ArgumentValue::Value(value) => {
                            unit.instructions.iter().find_map(|i| match *i {
                                Inst::Read {
                                    result,
                                    address,
                                    mode,
                                    ..
                                } if result == value
                                    && mode
                                        == if argument.passing
                                            == hirsel_core_contract::Passing::Take
                                        {
                                            ReadMode::Take
                                        } else {
                                            ReadMode::Borrow
                                        } =>
                                {
                                    parameter(address)
                                }
                                _ => None,
                            })
                        }
                    };
                    if source != Some(argument.parameter) {
                        return Err(
                            "adapter substitutes an actual at its successful handoff".into()
                        );
                    }
                }
                state = State::Handed;
            }
            Terminator::RejectTransfer if state == State::Rejected => (),
            Terminator::Return(_) if state == State::Handed => (),
            Terminator::Propagate if state == State::Failed => (),
            Terminator::Jump(_) => (),
            _ => {
                return Err(
                    "adapter exit bypasses rejection or its single successful handoff".into(),
                );
            }
        }
        outgoing.extend(
            term.normal_edges(&unit.pools)
                .into_iter()
                .map(|e| (e, state)),
        );
        outgoing.extend(
            term.failure_edges(&unit.tables)
                .into_iter()
                .map(|f| (unit.tables.failures[f.index()].cleanup, State::Failed)),
        );
        for (edge, state) in outgoing {
            let target = unit.tables.edges[edge.index()].target;
            match incoming.get(&target) {
                Some(previous) if *previous != state => {
                    return Err("adapter gate custody disagrees at a CFG join".into());
                }
                Some(_) => (),
                None => {
                    incoming.insert(target, state);
                    pending.push_back(target);
                }
            }
        }
    }
    Ok(true)
}

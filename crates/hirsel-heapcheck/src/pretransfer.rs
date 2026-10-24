//! H reconstructs private adapter custody independently of C's protocol audit.
use crate::violation::{Checked, ViolationKind as V, err};
use hirsel_lir::{LayoutTable, ir::*, pretransfer::AdapterReference, stage::Stage};
use std::collections::BTreeSet;

pub(crate) fn audit<S: Stage>(
    u: &LirUnit<S>,
    table: &LayoutTable,
    references: &[AdapterReference],
) -> Checked<()> {
    let bad = |text: &str| err(V::FrameImbalance, text);
    // Backward code provenance: all reaching definitions must prove ordinary
    // before an opaque callable can discard the possible adapter inventory.
    fn ordinary<S>(
        u: &LirUnit<S>,
        v: ValueId,
        refs: &[AdapterReference],
        seen: &mut BTreeSet<ValueId>,
    ) -> bool {
        if !seen.insert(v) {
            return true;
        }
        if u.parameters.contains(&v) || u.captures.contains(&v) {
            return false;
        }
        for i in &u.instructions {
            match *i {
                Inst::FunctionRef { result, symbol } if result == v => {
                    return !refs
                        .iter()
                        .any(|r| r.adapter == u.tables.symbols[symbol.index()]);
                }
                Inst::Closure { result, .. } if result == v => return true,
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
                } if result == v => {
                    if u.addresses[address.index()].projection.len != 0 {
                        return false;
                    }
                    return ordinary(u, reaching, refs, seen);
                }
                Inst::Store {
                    version,
                    value,
                    address,
                }
                | Inst::Replace {
                    version,
                    value,
                    address,
                    ..
                } if version == v => {
                    if u.addresses[address.index()].projection.len != 0 {
                        return false;
                    }
                    return ordinary(u, value, refs, seen);
                }
                Inst::Weaken { result, value } | Inst::CopyValue { result, value }
                    if result == v =>
                {
                    return ordinary(u, value, refs, seen);
                }
                _ => (),
            }
        }
        let mut found = false;
        for edge in &u.tables.edges {
            let block = &u.blocks[edge.target.index()];
            for (&p, &value) in block
                .parameters
                .get(&u.pools.values)
                .unwrap_or(&[])
                .iter()
                .zip(edge.arguments.get(&u.pools.values).unwrap_or(&[]))
            {
                if p == v {
                    found = true;
                    if !ordinary(u, value, refs, seen) {
                        return false;
                    }
                }
            }
        }
        found
    }
    for call in &u.tables.calls {
        let possible = match call.callee {
            Callee::Operation(_) => false,
            Callee::Direct(s) | Callee::Bound { code: s, .. } => references
                .iter()
                .any(|r| r.adapter == u.tables.symbols[s.index()]),
            Callee::Indirect(v) => {
                !ordinary(u, v, references, &mut BTreeSet::new())
                    && references.iter().any(|r| {
                        let (a, b) = (
                            &table.fn_abis[call.abi.0 as usize],
                            &table.fn_abis[r.abi.0 as usize],
                        );
                        a.parameters == b.parameters
                            && a.result == b.result
                            && a.diverges == b.diverges
                    })
            }
        };
        if possible != call.pretransfer_failure.is_some() {
            return Err(bad(
                "H private adapter dispatch has a missing or extra rejection edge",
            ));
        }
        if let Some(f) = call.pretransfer_failure {
            let f = &u.tables.failures[f.index()];
            if f.rule.is_some()
                || f.dispose
                    .get(&u.pools.values)
                    .unwrap_or(&[])
                    .iter()
                    .any(|v| u.types[u.values[v.index()].ty.index()].repr != ValueRepr::Token)
            {
                return Err(bad("H rejection edge disposes pending custody"));
            }
        }
    }
    let reference = references.iter().find(|r| {
        r.adapter.definition == u.header.definition && r.adapter.instance == Some(u.header.instance)
    });
    if reference.is_none() {
        if u.terminators.contains(&Terminator::RejectTransfer) {
            return Err(bad("H refuses private status 3 from an ordinary unit"));
        }
        return Ok(());
    }
    let reference = reference.unwrap();
    if u.header.origin != UnitOrigin::Library || reference.abi != u.header.abi {
        return Err(bad("H adapter has no independent body role"));
    }
    let key = &table.fn_abis[reference.abi.0 as usize].parameters[1];
    let mut gates = 0;
    let mut handoffs = 0;
    for inst in &u.instructions {
        if let Inst::Hash {
            value,
            layout,
            failure,
            ..
        } = *inst
        {
            let read = u
                .instructions
                .iter()
                .find_map(|i| match *i {
                    Inst::Read {
                        result,
                        address,
                        mode: ReadMode::Borrow,
                        ..
                    } if result == value => Some(address),
                    _ => None,
                })
                .ok_or_else(|| bad("H gate does not borrow a formal"))?;
            let address = &u.addresses[read.index()];
            if address.projection.len != 0
                || !matches!(address.base,AddressBase::Slot(s) if u.slots[s.index()].kind==SlotKind::Parameter(1))
                || layout != key.layout
                || u.tables.failures[failure.index()].rule.as_deref() != Some("MEM-3")
            {
                return Err(bad("H gate is not complete key admission"));
            }
            gates += 1;
        }
        if matches!(inst, Inst::FrameEnter { .. } | Inst::FrameLeave) {
            return Err(bad("H compiler adapter adds an activation"));
        }
    }
    for call in &u.tables.calls {
        if !matches!(call.callee,Callee::Direct(s) if u.tables.symbols[s.index()]==reference.primitive)
            || !call.forward_caller
            || call.pretransfer_failure.is_some()
        {
            return Err(bad("H adapter handoff target differs"));
        }
        handoffs += 1;
    }
    if gates != 1 || handoffs != 1 || !u.terminators.contains(&Terminator::RejectTransfer) {
        return Err(bad(
            "H adapter needs one gate, one handoff and pending rejection",
        ));
    }
    Ok(())
}

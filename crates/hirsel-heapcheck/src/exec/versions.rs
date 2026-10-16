//! Storage-version declarations, derived independently of C's flow analysis.
use super::*;

#[derive(Clone, Copy, Eq, PartialEq)]
enum Version {
    Unknown,
    Slot(SlotId),
    Conflict,
}

pub(super) fn slots<S>(unit: &LirUnit<S>) -> Vec<Option<SlotId>> {
    let root = |mut address: AddressId| {
        for _ in 0..unit.addresses.len() {
            match unit.addresses.get(address.index())?.base {
                AddressBase::Slot(slot) => return Some(slot),
                AddressBase::Address(parent) => address = parent,
                AddressBase::Value(_) => return None,
            }
        }
        None
    };
    let mut versions = vec![Version::Unknown; unit.values.len()];
    let mut queue = std::collections::VecDeque::new();
    let mut declare = |value: ValueId, slot: SlotId| {
        if let Some(current) = versions.get_mut(value.index()) {
            let next = match *current {
                Version::Unknown => Version::Slot(slot),
                Version::Slot(other) if other == slot => *current,
                _ => Version::Conflict,
            };
            if *current != next {
                *current = next;
                queue.push_back(value);
            }
        }
    };
    for (index, slot) in unit.slots.iter().enumerate() {
        let value = match slot.kind {
            SlotKind::Parameter(ordinal) => unit.parameters.get(ordinal as usize),
            SlotKind::Capture(ordinal) => unit.captures.get(ordinal as usize),
            _ => None,
        };
        if let Some(&value) = value {
            declare(value, SlotId(index as u32));
        }
    }
    for inst in &unit.instructions {
        match *inst {
            Inst::Store {
                address, version, ..
            }
            | Inst::SeqReserve {
                address, version, ..
            }
            | Inst::SeqSetLen {
                address, version, ..
            }
            | Inst::SeqAppend {
                address, version, ..
            }
            | Inst::FlatAppend {
                address, version, ..
            }
            | Inst::FlatField {
                address, version, ..
            }
            | Inst::FlatClose {
                address, version, ..
            } => {
                if let Some(slot) = root(address) {
                    declare(version, slot);
                }
            }
            Inst::Replace {
                address,
                version,
                displaced,
                displaced_version,
                ..
            } => {
                if let Some(slot) = root(address) {
                    declare(version, slot);
                }
                declare(displaced_version, displaced);
            }
            _ => {}
        }
    }
    for call in &unit.tables.calls {
        for argument in call
            .arguments
            .get(&unit.pools.call_arguments)
            .unwrap_or(&[])
        {
            if argument.passing == hirsel_core_contract::Passing::Mut
                && let ArgumentValue::Address { address, version } = argument.value
                && let Some(slot) = root(address)
            {
                declare(version, slot);
            }
        }
    }
    drop(declare);
    let mut dependents = vec![Vec::new(); unit.values.len()];
    for edge in &unit.tables.edges {
        let Some(block) = unit.blocks.get(edge.target.index()) else {
            continue;
        };
        for (&arg, &param) in edge
            .arguments
            .get(&unit.pools.values)
            .unwrap_or(&[])
            .iter()
            .zip(block.parameters.get(&unit.pools.values).unwrap_or(&[]))
        {
            if let Some(children) = dependents.get_mut(arg.index()) {
                children.push(param);
            }
        }
    }
    // A declaration changes at most twice (unknown -> slot -> conflict), so
    // each edge is visited at most twice even with loop-carried versions.
    while let Some(arg) = queue.pop_front() {
        let incoming = versions[arg.index()];
        for &param in &dependents[arg.index()] {
            let Some(current) = versions.get_mut(param.index()) else {
                continue;
            };
            let next = match (*current, incoming) {
                (Version::Unknown, other) => other,
                (own, Version::Unknown) => own,
                (Version::Slot(a), Version::Slot(b)) if a == b => *current,
                _ => Version::Conflict,
            };
            if *current != next {
                *current = next;
                queue.push_back(param);
            }
        }
    }
    versions
        .into_iter()
        .map(|version| match version {
            Version::Slot(slot) => Some(slot),
            _ => None,
        })
        .collect()
}

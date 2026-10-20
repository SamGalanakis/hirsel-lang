//! A String's read-only Bytes interval can keep the String's existing storage.
//! Escaping/mutated results and temporary or mutable owners retain an owning copy.
use super::*;
use hirsel_types::{Primitive, TypeKind};

pub(super) fn aliases(
    body: &tir::Body,
    types: &hirsel_types::TypeInterner,
    aliases: &mut [Option<tir::PlaceId>],
) {
    use tir::InstructionKind as K;
    let n = body.locals.len();
    let root = |p: tir::PlaceId| body.places[p.0 as usize].root.0 as usize;
    let whole = |p: tir::PlaceId| body.places[p.0 as usize].projection.is_empty();
    let primitive = |l: usize, want| matches!(types.get(body.locals[l].ty).map(|t| &t.kind), Some(TypeKind::Primitive(p)) if *p == want);
    let mut parent: Vec<usize> = (0..n).collect();
    fn find(parent: &mut [usize], l: usize) -> usize {
        if parent[l] != l {
            parent[l] = find(parent, parent[l]);
        }
        parent[l]
    }
    for inst in body.blocks.iter().flat_map(|b| &b.instructions) {
        if let K::Assign { destination, value } = inst.kind
            && let tir::Rvalue::Use(op) = &body.rvalues[value.0 as usize]
            && op.usage == UseKind::Take
            && whole(destination)
            && whole(op.place)
            && primitive(root(destination), Primitive::Bytes)
            && primitive(root(op.place), Primitive::Bytes)
        {
            let (a, b) = (
                find(&mut parent, root(destination)),
                find(&mut parent, root(op.place)),
            );
            parent[a] = b;
        }
    }
    for l in 0..n {
        find(&mut parent, l);
    }
    let adjacent: Vec<Vec<usize>> = body
        .blocks
        .iter()
        .map(|b| {
            body.successors(&b.terminator)
                .iter()
                .map(|b| b.0 as usize)
                .collect()
        })
        .collect();
    let mut cyclic = vec![false; body.blocks.len()];
    for component in hirsel_graph::components(&adjacent) {
        if component.len() > 1 || adjacent[component[0]].contains(&component[0]) {
            for b in component {
                cyclic[b] = true;
            }
        }
    }
    let mut invalid = vec![false; n];
    let mut owner_invalid = vec![false; n];
    let mut definitions = vec![0; n];
    let mut producers: Vec<Vec<tir::PlaceId>> = vec![Vec::new(); n];
    let mut loans = HashMap::new();
    let operand = |op: &tir::Operand, invalid: &mut [bool], owner_invalid: &mut [bool]| {
        if matches!(op.usage, UseKind::Take | UseKind::Mut) {
            invalid[parent[root(op.place)]] = true;
            owner_invalid[root(op.place)] = true;
        }
    };
    // First collect only non-moving uses. Whole Bytes moves are storage aliases.
    for (b, block) in body.blocks.iter().enumerate() {
        for inst in &block.instructions {
            match &inst.kind {
                K::Assign { destination, value } => {
                    let d = root(*destination);
                    definitions[d] += 1;
                    owner_invalid[d] |= !whole(*destination) || cyclic[b];
                    let moved = matches!(&body.rvalues[value.0 as usize], tir::Rvalue::Use(op) if op.usage == UseKind::Take && whole(*destination) && whole(op.place) && primitive(d, Primitive::Bytes) && parent[d] == parent[root(op.place)]);
                    if !moved {
                        invalid[parent[d]] = true;
                        for op in rvalue_operands(&body.rvalues[value.0 as usize]) {
                            operand(op, &mut invalid, &mut owner_invalid);
                        }
                    }
                }
                K::BeginLoan {
                    id, place, mutable, ..
                } => {
                    loans.insert(id.0, *place);
                    if *mutable {
                        invalid[parent[root(*place)]] = true;
                        owner_invalid[root(*place)] = true;
                    }
                }
                K::Replace {
                    destination,
                    value,
                    displaced,
                    ..
                } => {
                    invalid[parent[root(*destination)]] = true;
                    owner_invalid[root(*destination)] = true;
                    owner_invalid[root(*displaced)] = true;
                    operand(value, &mut invalid, &mut owner_invalid);
                }
                K::Snapshot { destination, value } => {
                    invalid[parent[root(*destination)]] = true;
                    owner_invalid[root(*destination)] = true;
                    operand(value, &mut invalid, &mut owner_invalid);
                }
                K::Check { condition, .. } => operand(condition, &mut invalid, &mut owner_invalid),
                _ => {}
            }
        }
        match &block.terminator {
            tir::Terminator::Call {
                callee,
                arguments,
                returns,
                ..
            } => {
                if let Some(ret) = returns {
                    let d = root(ret.destination);
                    definitions[d] += 1;
                    owner_invalid[d] |= cyclic[b];
                    if matches!(callee, tir::Callee::Definition { path, .. } if path.is_core("string.as_bytes"))
                        && whole(ret.destination)
                        && let Some(source) = arguments.iter().find(|a| a.parameter.0 == 0)
                        && source.value.usage == UseKind::Borrow
                    {
                        producers[parent[d]].push(source.value.place);
                    } else {
                        invalid[parent[d]] = true;
                    }
                }
                for a in arguments {
                    operand(&a.value, &mut invalid, &mut owner_invalid);
                }
                if let tir::Callee::Indirect { value, .. } = callee {
                    operand(value, &mut invalid, &mut owner_invalid);
                }
            }
            tir::Terminator::Return { value, .. }
            | tir::Terminator::Branch {
                condition: value, ..
            }
            | tir::Terminator::Switch { value, .. } => {
                operand(value, &mut invalid, &mut owner_invalid)
            }
            _ => {}
        }
    }
    let scopes = super::flat_views::scope_intervals(body);
    let mut selected = vec![None; n];
    for group in 0..n {
        let [source] = producers[group].as_slice() else {
            continue;
        };
        if invalid[group] {
            continue;
        }
        let mut source = *source;
        while let Some(loan) = body.places[source.0 as usize].loan {
            let Some(parent) = loans.get(&loan.0) else {
                break;
            };
            if *parent == source {
                break;
            }
            source = *parent;
        }
        let l = root(source);
        if !whole(source) || !primitive(l, Primitive::String) || owner_invalid[l] {
            continue;
        }
        if body.locals[l].storage != tir::Storage::Parameter
            && (body.locals[l].storage != tir::Storage::Local || definitions[l] != 1)
        {
            continue;
        }
        selected[group] = Some(source);
    }
    for l in 0..n {
        let group = parent[l];
        if let Some(source) = selected[group] {
            let outer = scopes[body.locals[root(source)].scope.0 as usize];
            let inner = scopes[body.locals[l].scope.0 as usize];
            if !view_storage(body, l) || !(outer.0 <= inner.0 && inner.1 <= outer.1) {
                selected[group] = None;
            }
        }
    }
    for l in 0..n {
        if let Some(source) = selected[parent[l]] {
            aliases[l] = Some(source);
        }
    }
}

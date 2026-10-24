//! Abstract value declarations for hand-built ledger fixtures. These fixtures
//! have no TIR source provider; they explicitly use ordinary fields and boxes,
//! without alias/distinct erasure. Production access uses independent source
//! types instead of this fixture-only physical construction vocabulary.
use hirsel_lir::structural::{Shape, ShapeGraph};
use hirsel_lir::structural_access::AccessPlan;
use hirsel_lir::{LayoutId, LayoutTable, ir::*, layout_types::LayoutKind as K};
use std::collections::{BTreeMap, BTreeSet};

fn plan(table: &LayoutTable, root: LayoutId) -> Option<AccessPlan> {
    let mut known = BTreeMap::new();
    let mut pending = vec![];
    let mut nodes = vec![];
    fn node(
        id: LayoutId,
        known: &mut BTreeMap<LayoutId, u32>,
        pending: &mut Vec<LayoutId>,
        nodes: &mut Vec<Shape>,
    ) -> u32 {
        if let Some(&n) = known.get(&id) {
            return n;
        }
        let n = nodes.len() as u32;
        known.insert(id, n);
        pending.push(id);
        nodes.push(Shape::Leaf);
        n
    }
    let root_node = node(root, &mut known, &mut pending, &mut nodes);
    while let Some(id) = pending.pop() {
        let row = table.layout(id)?;
        let mut child = |id| node(id, &mut known, &mut pending, &mut nodes);
        let shape = match row.kind {
            K::Scalar(_) | K::Path | K::Slice { .. } => Shape::Leaf,
            K::Unit | K::Aggregate | K::Flat | K::Resource => {
                Shape::Fields(row.fields.iter().map(|f| child(f.layout)).collect())
            }
            K::Variant | K::Packed => Shape::Variant(
                row.tag
                    .as_ref()?
                    .variants
                    .iter()
                    .map(|arm| arm.fields.iter().map(|f| child(f.layout)).collect())
                    .collect(),
            ),
            K::Frozen { value } => Shape::Transparent(child(value)),
            K::Shared { payload } => Shape::Transparent(child(payload)),
            K::Block { payload } => match table.layout(payload)?.kind {
                K::Sequence { element } | K::Sequential { element } | K::SoaColumns { element } => {
                    Shape::Sequence(child(element))
                }
                K::Flat => Shape::Transparent(child(payload)),
                _ => Shape::Box(child(payload)),
            },
            K::Box { payload } => Shape::Box(child(payload)),
            K::Inline { element, .. } | K::Simd { element, .. } => Shape::Sequence(child(element)),
            _ => return None,
        };
        nodes[known[&id] as usize] = shape;
    }
    AccessPlan::derive(
        hirsel_lir::operation::NativeType::Tuple(vec![]),
        ShapeGraph {
            root: root_node,
            nodes,
        },
        table,
        root,
    )
    .ok()
}

pub(super) fn attach<S>(unit: &mut LirUnit<S>, table: &LayoutTable) {
    let roots: BTreeSet<_> = unit
        .instructions
        .iter()
        .filter_map(|i| match *i {
            Inst::Compare { layout, .. } | Inst::Hash { layout, .. } => Some(layout),
            _ => None,
        })
        .collect();
    for root in roots {
        if unit
            .tables
            .structural
            .iter()
            .all(|p| p.root_layout() != root)
            && let Some(plan) = plan(table, root)
        {
            unit.tables.structural.push(plan);
        }
    }
}

//! Independent MEM-3 admission over the H ledger. Runtime programs and asserted
//! edge totals are not executed here; offsets come from checked layouts and
//! child costs from the independently validated logical graph correspondence.
use super::compare::At;
use super::*;
use hirsel_lir::codec::{CodecNode as N, CodecNodeId as Id};
use hirsel_lir::structural::Shape;
use hirsel_lir::structural_access::AccessPlan;

const STRUCTURAL_LIMIT: u64 = 1088;

impl<S: Stage> Machine<'_, S> {
    fn structural_node<'p>(&self, plan: &'p AccessPlan, id: Id) -> Checked<&'p N> {
        plan.nodes
            .get(id.index())
            .ok_or_else(|| err(V::Malformed, "structural access node"))
    }

    fn structural_delta(&self, plan: &AccessPlan, id: Id) -> Checked<u64> {
        let node = self.structural_node(plan, id)?;
        let layout = self.record(node.layout())?;
        if matches!(layout.kind, LayoutKind::Frozen { .. }) || matches!(node, N::Framed { .. }) {
            return Ok(0);
        }
        if let LayoutKind::Block { payload } = layout.kind
            && self.record(payload)?.kind == LayoutKind::Flat
            && matches!(node, N::Box { .. })
        {
            return Ok(0);
        }
        let logical = *plan
            .logical_nodes
            .get(id.index())
            .ok_or_else(|| err(V::Malformed, "structural logical correspondence"))?;
        Ok(
            match plan
                .graph
                .nodes
                .get(logical as usize)
                .ok_or_else(|| err(V::Malformed, "structural shape"))?
            {
                Shape::Leaf | Shape::Transparent(_) => 0,
                Shape::Fields(_)
                | Shape::Variant(_)
                | Shape::Box(_)
                | Shape::Sequence(_)
                | Shape::Map { .. }
                | Shape::Set(_)
                | Shape::Distinct(_) => 1,
            },
        )
    }

    pub(super) fn structural_preflight(&self, plan: &AccessPlan, value: &Val) -> Checked<bool> {
        use super::projection::{Location, Projection, SharedTraversal};
        let image = self.projection_image(plan, value)?;
        let root = Projection {
            node: plan.root,
            location: Location::Stored(At::Image(&image, 0)),
        };
        let mut work = vec![(root, 0u64)];
        while let Some((projection, depth)) = work.pop() {
            if depth > STRUCTURAL_LIMIT {
                return Ok(false);
            }
            let next = depth + self.structural_delta(plan, projection.node)?;
            let view = self.projection_view(plan, projection, SharedTraversal::Follow)?;
            work.extend(view.children.into_iter().rev().map(|child| (child, next)));
        }
        Ok(true)
    }
}

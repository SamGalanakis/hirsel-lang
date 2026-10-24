//! H value operations on independent ledger projections, after MEM-3 admission.
//! No runtime/interpreter traversal or serialization output is reused here.
use super::compare::At;
use super::projection::{Location, Projection, SharedTraversal};
use super::*;
use hirsel_lir::codec::CodecNode as Node;
use hirsel_lir::structural_access::AccessPlan;
use std::cmp::Ordering;

fn stored(location: Location<'_>) -> Checked<At<'_>> {
    match location {
        Location::Stored(at) => Ok(at),
        Location::Columns { .. } => fail(V::Malformed, "scalar in virtual row"),
    }
}
impl<S: Stage> Machine<'_, S> {
    pub(super) fn projection_order(
        &self,
        plan: &AccessPlan,
        a: &Val,
        b: &Val,
    ) -> Checked<Option<Ordering>> {
        let a = self.projection_image(plan, a)?;
        let b = self.projection_image(plan, b)?;
        self.projection_order_at(
            plan,
            Projection {
                node: plan.root,
                location: Location::Stored(At::Image(&a, 0)),
            },
            Projection {
                node: plan.root,
                location: Location::Stored(At::Image(&b, 0)),
            },
        )
    }

    fn projection_sort(&self, plan: &AccessPlan, values: &mut [Projection<'_>]) -> Checked<()> {
        let mut failure = None;
        values.sort_by(|a, b| match self.projection_order_at(plan, *a, *b) {
            Ok(Some(order)) => order,
            Ok(None) => {
                failure = Some(err(V::TypeConfusion, "unordered Set key"));
                Ordering::Equal
            }
            Err(error) => {
                failure = Some(error);
                Ordering::Equal
            }
        });
        failure.map_or(Ok(()), Err)
    }

    fn projection_order_at(
        &self,
        plan: &AccessPlan,
        a: Projection<'_>,
        b: Projection<'_>,
    ) -> Checked<Option<Ordering>> {
        enum Item<'a> {
            Pair(Projection<'a>, Projection<'a>),
            Length(u64, u64),
        }
        let mut work = vec![Item::Pair(a, b)];
        while let Some(item) = work.pop() {
            let (a, b) = match item {
                Item::Length(a, b) => {
                    if a != b {
                        return Ok(Some(a.cmp(&b)));
                    }
                    continue;
                }
                Item::Pair(a, b) => (a, b),
            };
            let a_view = self.projection_view(plan, a, SharedTraversal::Follow)?;
            let b_view = self.projection_view(plan, b, SharedTraversal::Follow)?;
            if a_view.ordinal != b_view.ordinal {
                return Ok(Some(a_view.ordinal.cmp(&b_view.ordinal)));
            }
            if let (Some(a), Some(b)) = (a_view.bytes, b_view.bytes) {
                let order = self.projection_bytes(a)?.cmp(self.projection_bytes(b)?);
                if order != Ordering::Equal {
                    return Ok(Some(order));
                }
                continue;
            }
            match a_view.node {
                Node::Scalar(layout) | Node::Char(layout) => {
                    let kind = match self.record(*layout)?.kind {
                        LayoutKind::Scalar(kind) => kind,
                        _ => return fail(V::Malformed, "projection scalar layout"),
                    };
                    let size = self.size(*layout)?;
                    let a = scalar::canonical(kind, self.bits_at(stored(a.location)?, size)?);
                    let b = scalar::canonical(kind, self.bits_at(stored(b.location)?, size)?);
                    let order = scalar::order(kind, a, b).map_err(|m| err(V::TypeConfusion, m))?;
                    if order != Some(Ordering::Equal) {
                        return Ok(order);
                    }
                }
                Node::Path(_) => {
                    let a = self.bits_at(stored(a.location)?, 8)? as u64;
                    let b = self.bits_at(stored(b.location)?, 8)? as u64;
                    let a = self
                        .ops
                        .path_text(a)
                        .ok_or_else(|| err(V::Malformed, "unknown Path ID"))?;
                    let b = self
                        .ops
                        .path_text(b)
                        .ok_or_else(|| err(V::Malformed, "unknown Path ID"))?;
                    let order = a.as_bytes().cmp(b.as_bytes());
                    if order != Ordering::Equal {
                        return Ok(Some(order));
                    }
                }
                _ => {
                    if let (Some(a), Some(b)) = (a_view.count, b_view.count) {
                        work.push(Item::Length(a, b));
                    }
                    let (mut a, mut b) = (a_view.children, b_view.children);
                    if matches!(a_view.node, Node::Set { .. }) {
                        self.projection_sort(plan, &mut a)?;
                        self.projection_sort(plan, &mut b)?;
                    }
                    for (a, b) in a.into_iter().zip(b).rev() {
                        work.push(Item::Pair(a, b));
                    }
                }
            }
        }
        Ok(Some(Ordering::Equal))
    }

    pub(super) fn projection_hash(&self, plan: &AccessPlan, value: &Val) -> Checked<u64> {
        let image = self.projection_image(plan, value)?;
        let root = Projection {
            node: plan.root,
            location: Location::Stored(At::Image(&image, 0)),
        };
        let mut work = vec![root];
        let mut hash = 0xcbf2_9ce4_8422_2325u64;
        let mut feed = |bytes: &[u8]| {
            for &byte in bytes {
                hash = (hash ^ u64::from(byte)).wrapping_mul(0x100000001b3);
            }
        };
        while let Some(projection) = work.pop() {
            let view = self.projection_view(plan, projection, SharedTraversal::Follow)?;
            if let Some(bytes) = view.bytes {
                feed(&bytes.len.to_le_bytes());
                feed(self.projection_bytes(bytes)?);
                continue;
            }
            match view.node {
                Node::Scalar(layout) | Node::Char(layout) => {
                    let kind = match self.record(*layout)?.kind {
                        LayoutKind::Scalar(kind) => kind,
                        _ => return fail(V::Malformed, "projection scalar layout"),
                    };
                    let size = self.size(*layout)?;
                    let bits =
                        scalar::canonical(kind, self.bits_at(stored(projection.location)?, size)?);
                    let bits = match kind {
                        ScalarKind::F32 if f32::from_bits(bits as u32) == 0.0 => 0,
                        ScalarKind::F64 if f64::from_bits(bits as u64) == 0.0 => 0,
                        _ => bits,
                    };
                    feed(&bits.to_le_bytes()[..size as usize]);
                }
                Node::Path(_) => {
                    let id = self.bits_at(stored(projection.location)?, 8)? as u64;
                    let text = self
                        .ops
                        .path_text(id)
                        .ok_or_else(|| err(V::Malformed, "unknown Path ID"))?;
                    feed(&(text.len() as u64).to_le_bytes());
                    feed(text.as_bytes());
                }
                _ => {
                    if let Some(tag) = view.ordinal {
                        feed(&tag.to_le_bytes());
                    }
                    if let Some(count) = view.count {
                        feed(&count.to_le_bytes());
                    }
                    let mut children = view.children;
                    if matches!(view.node, Node::Set { .. }) {
                        self.projection_sort(plan, &mut children)?;
                    }
                    work.extend(children.into_iter().rev());
                }
            }
        }
        Ok(hash)
    }
}

//! Seal entry authority is derived from actual H scope and Take state.
use super::*;
use crate::ledger::sealed::StorageCustody;

impl<S: Stage> Machine<'_, S> {
    pub(super) fn seal_root(
        &mut self,
        f: &mut Frame,
        result: ValueId,
        grant: ValueId,
        root: ValueId,
        site: (SealDescriptorId, SourceAnchorId),
    ) -> Checked<Option<usize>> {
        let (shape, origin) = site;
        let unit = self.unit(f.unit);
        if unit.header.origin != UnitOrigin::Library
            || unit.header.definition != hirsel_syntax::DefPath::core("arena.seal")
        {
            return fail(
                V::Malformed,
                "sealed producer is not the arena.seal library body",
            );
        }
        let descriptor = unit
            .tables
            .seal_descriptors
            .get(shape.index())
            .ok_or_else(|| err(V::Malformed, "seal descriptor is missing"))?;
        descriptor
            .check(&self.program.layouts)
            .map_err(|e| err(V::TypeConfusion, e))?;
        let root_layout = descriptor.root_layout;
        let owner_ty = self.value_type(f.unit, result)?;
        let owner_layout = owner_ty.layout;
        let root_ty = self.value_type(f.unit, root)?;
        if owner_ty.repr != ValueRepr::Handle
            || self.value_info(f.unit, result)?.holding != Holding::Owned
            || !matches!(self.record(owner_layout)?.kind,
                LayoutKind::SealedRegion { root } if root == root_layout)
            || root_ty.layout != root_layout
            || self.value_type(f.unit, grant)?.repr != ValueRepr::Arena
        {
            return fail(
                V::TypeConfusion,
                "seal root, grant or owner has another type",
            );
        }
        if f.values.get(result.index()).is_none_or(Option::is_some) {
            return fail(V::Malformed, "seal result is not fresh");
        }
        if self.get(f, grant)?.borrow.is_some() || self.get(f, root)?.borrow.is_some() {
            return fail(
                V::BorrowedStore,
                "seal requires a consumable grant and root",
            );
        }
        let Val::Arena(Some(region)) = self.read_use(f, grant)? else {
            return fail(V::TypeConfusion, "heap is not a consumable arena grant");
        };
        let scope = self
            .ledger
            .regions
            .get(region)
            .ok_or_else(|| err(V::ForgedPointer, "seal grant names an unknown scope"))?;
        if !scope.live {
            return fail(V::LiveRegion, "seal grant names a closed scope");
        }
        if scope.storage != StorageCustody::Scope {
            return fail(V::CreditUnderflow, "seal grant was already consumed");
        }
        // Authority was recorded by H's actual Take read from incoming
        // parameter storage. That slot is now empty in the real library body.
        if !f.seal_grants.contains(&(grant, region)) {
            return fail(
                V::ForgedPointer,
                "seal grant has no actual Take parameter read",
            );
        }
        for (index, slot) in unit.slots.iter().enumerate() {
            if unit.types[slot.ty.index()].repr == ValueRepr::Arena
                && let Some(SlotStore::Own(ptr) | SlotStore::Alias(ptr)) = f.slots.get(index)
                && f.loans.iter().flatten().any(|loan| loan.alloc == ptr.alloc)
            {
                return fail(V::BorrowExpired, "seal grant still has an active loan");
            }
        }
        let kind = self.record(root_layout)?.kind.clone();
        let bits = matches!((&kind, root_ty.repr), (LayoutKind::Unit, ValueRepr::Unit))
            || matches!((&kind, root_ty.repr),
                (LayoutKind::Scalar(a), ValueRepr::Scalar(b)) if *a == b && b != ScalarKind::Pointer);
        let empty = scope.allocations.is_empty()
            && !self
                .ledger
                .allocations
                .indexed()
                .any(|(_, a)| a.owner == Owner::Region(region) && a.state == AllocState::Live);
        let expected_retained = self.config.seal_sources.iter().any(|s| {
            s.events.iter().any(|e| {
                matches!(
                    e.kind,
                    hirsel_lir::sealed::SourceEventKind::Allocate
                        | hirsel_lir::sealed::SourceEventKind::Replace
                )
            })
        });
        let (id, staged) = if bits && empty && !expected_retained {
            let size = self.size(root_layout)?;
            let mut image = Extent::new(size);
            match (kind, self.read_use(f, root)?) {
                (LayoutKind::Unit, Val::Unit) => {}
                (LayoutKind::Scalar(expected), Val::Bits { scalar, bits })
                    if expected == scalar =>
                {
                    image.write_bytes(0, &bits.to_le_bytes()[..size as usize]);
                }
                _ => return fail(V::TypeConfusion, "seal root bits have another runtime type"),
            }
            let mut staged = self.ledger.clone();
            let id = staged.transfer_closed_scope_with_bit_root(
                region,
                self.word,
                &self.program.layouts,
                root_layout,
                image,
            )?;
            (id, staged)
        } else {
            let Some(prepared) = self.prepare_seal_graph(f, region, root, shape, origin)? else {
                return Ok(Some(region));
            };
            prepared
        };
        let align = self.record(owner_layout)?.align;
        let control_size = self.size(owner_layout)?;
        // Preparation mutated only a private ledger. Publish once all refusal
        // checks, charge arithmetic and source-credit drains have succeeded.
        self.take_value(f, grant)?;
        self.take_value(f, root)?;
        if !bits {
            f.values[root.index()] = None;
        }
        self.ledger = staged;
        let control = self.ledger.alloc(
            Owner::Heap,
            Some(owner_layout),
            control_size,
            control_size,
            align,
            Some(Count::Finite(1)),
        );
        self.ledger.bind_sealed_control(control, id)?;
        self.seal_inventory.remove(&region);
        f.seal_grants.retain(|(_, scope)| *scope != region);
        self.define_fresh(f, result, Val::Ptr(control))?;
        Ok(None)
    }
}

//! Empty-scope bit-root sealing derives custody from actual H scope state.
//! Retained allocations need semantic inventory and relocation, still refused.
use super::*;
use crate::ledger::sealed::StorageCustody;

impl<S: Stage> Machine<'_, S> {
    pub(super) fn seal_bit_root(
        &mut self,
        f: &mut Frame,
        result: ValueId,
        grant: ValueId,
        root: ValueId,
        shape: SealDescriptorId,
    ) -> Checked<()> {
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
        let kind = self.record(root_layout)?.kind.clone();
        if !matches!((&kind, root_ty.repr), (LayoutKind::Unit, ValueRepr::Unit))
            && !matches!((&kind, root_ty.repr),
                (LayoutKind::Scalar(a), ValueRepr::Scalar(b)) if *a == b && b != ScalarKind::Pointer)
        {
            return fail(
                V::Unsupported,
                "sealed retained-root inventory is not linked",
            );
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
        // Check both the listing and independently recorded live allocations.
        // Even unreachable storage needs its inventory before it can transfer.
        if !scope.allocations.is_empty()
            || self
                .ledger
                .allocations
                .indexed()
                .any(|(_, a)| a.owner == Owner::Region(region) && a.state == AllocState::Live)
        {
            return fail(V::Unsupported, "sealed scope inventory is not linked");
        }
        let size = self.size(root_layout)?;
        let mut image = Extent::new(size);
        match (kind, self.read_use(f, root)?) {
            (LayoutKind::Unit, Val::Unit) => {}
            (LayoutKind::Scalar(expected), Val::Bits { scalar, bits }) if expected == scalar => {
                image.write_bytes(0, &bits.to_le_bytes()[..size as usize]);
            }
            _ => return fail(V::TypeConfusion, "seal root bits have another runtime type"),
        }
        let align = self.record(owner_layout)?.align;
        let control_size = self.size(owner_layout)?;
        // Metadata does not debit the logical arena budget. All refusal checks
        // precede custody transfer; the lexical guard and budget remain live.
        let id = self.ledger.transfer_closed_scope_with_bit_root(
            region,
            self.word,
            &self.program.layouts,
            root_layout,
            image,
        )?;
        let control = self.ledger.alloc(
            Owner::Heap,
            Some(owner_layout),
            control_size,
            control_size,
            align,
            Some(Count::Finite(1)),
        );
        self.ledger.bind_sealed_control(control, id)?;
        f.seal_grants.retain(|(_, scope)| *scope != region);
        self.take_value(f, grant)?;
        self.take_value(f, root)?;
        self.define_fresh(f, result, Val::Ptr(control))
    }
}

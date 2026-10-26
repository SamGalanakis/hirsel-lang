//! H-owned storage custody and affine owner/loan transitions. The language
//! SealArena/BorrowSealedRoot adapters establish source authority and SSA
//! borrow exclusion before using these transitions.
use super::*;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct SealedId(u32);

/// A local loan number alone is not an identity across two sealed owners.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct SealedLoan {
    owner: SealedId,
    index: u64,
}

#[derive(Clone)]
struct StoredRoot {
    layout: LayoutId,
    value: Extent,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum StorageCustody {
    Scope,
    Sealed(SealedId),
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Phase {
    Live,
    Dropping,
    Dead,
}

#[derive(Clone)]
pub(crate) struct SealedOwner {
    region: usize,
    allocations: Vec<Ptr>,
    phase: Phase,
    loans: BTreeSet<u64>,
    next_loan: u64,
    root: Option<StoredRoot>,
    control: Option<Ptr>,
}

impl Ledger {
    pub(crate) fn transfer_closed_scope(&mut self, region: usize, word: u64) -> Checked<SealedId> {
        let scope = self
            .regions
            .get(region)
            .ok_or_else(|| err(V::Malformed, "sealed transfer has no scope"))?;
        if scope.storage != StorageCustody::Scope {
            return fail(V::CreditUnderflow, "scope storage was already transferred");
        }
        self.sealed_cells(region, word)?;
        let allocations: Vec<Ptr> = self.regions[region]
            .allocations
            .iter()
            .copied()
            .filter(|id| self.get(*id).state == AllocState::Live)
            .map(|alloc| Ptr {
                alloc,
                generation: self.get(alloc).generation,
                offset: 0,
                path: 0,
            })
            .collect();
        let id = SealedId(
            u32::try_from(self.sealed_owners.len())
                .map_err(|_| err(V::Malformed, "sealed owner identity overflow"))?,
        );
        for ptr in &allocations {
            self.get_mut(ptr.alloc).published = true;
        }
        self.sealed_owners.push(SealedOwner {
            region,
            allocations,
            phase: Phase::Live,
            loans: BTreeSet::new(),
            next_loan: 0,
            root: None,
            control: None,
        });
        self.regions[region].storage = StorageCustody::Sealed(id);
        Ok(id)
    }

    /// Snapshot a fully initialized bit root. Data-bearing roots need a
    /// separate cell relocation/ownership proof and cannot enter this path.
    pub(crate) fn transfer_closed_scope_with_bit_root(
        &mut self,
        region: usize,
        word: u64,
        table: &hirsel_lir::LayoutTable,
        layout: LayoutId,
        value: Extent,
    ) -> Checked<SealedId> {
        use hirsel_lir::layout_types::{LayoutKind, ScalarKind, SizeSpec};
        let record = table
            .layout(layout)
            .ok_or_else(|| err(V::TypeConfusion, "unknown sealed bit root layout"))?;
        if !matches!(record.kind, LayoutKind::Unit)
            && !matches!(record.kind, LayoutKind::Scalar(kind) if kind != ScalarKind::Pointer)
            || record.size != SizeSpec::Fixed(value.len())
            || !value.cells.is_empty()
            || !value.resources.is_empty()
        {
            return fail(
                V::TypeConfusion,
                "sealed bit root contains storage or has another extent",
            );
        }
        if !value.all_init(0, value.len()) {
            return fail(V::UninitializedRead, "sealed bit root is not initialized");
        }
        // All fallible root checks precede the sole storage-custody transition.
        let id = self.transfer_closed_scope(region, word)?;
        self.sealed_owners[id.0 as usize].root = Some(StoredRoot { layout, value });
        Ok(id)
    }

    /// Called only after the H graph adapter has checked the typed image,
    /// source events, evacuation and credits in its private staging ledger.
    pub(crate) fn transfer_closed_scope_with_graph_root(
        &mut self,
        region: usize,
        word: u64,
        layout: LayoutId,
        value: Extent,
    ) -> Checked<SealedId> {
        let id = self.transfer_closed_scope(region, word)?;
        self.sealed_owners[id.0 as usize].root = Some(StoredRoot { layout, value });
        Ok(id)
    }

    pub(crate) fn sealed_bit_root(
        &self,
        id: SealedId,
        loan: SealedLoan,
        layout: LayoutId,
    ) -> Checked<&Extent> {
        self.check_sealed_ownership()?;
        let owner = self
            .sealed_owners
            .get(id.0 as usize)
            .ok_or_else(|| err(V::ForgedPointer, "unknown sealed root owner"))?;
        if loan.owner != id || owner.phase != Phase::Live || !owner.loans.contains(&loan.index) {
            return fail(
                V::BorrowExpired,
                "sealed root has no active loan of this owner",
            );
        }
        let root = owner
            .root
            .as_ref()
            .ok_or_else(|| err(V::TypeConfusion, "sealed owner has no typed root"))?;
        if root.layout != layout {
            return fail(V::TypeConfusion, "sealed root layout differs from its read");
        }
        Ok(&root.value)
    }

    /// Associate a private, affine checked allocation with an actual H owner.
    /// Integer handle bits and ordinary run-context handles supply no binding.
    pub(crate) fn bind_sealed_control(&mut self, control: Ptr, id: SealedId) -> Checked<()> {
        let allocation = self.check(control)?;
        if control.offset != 0
            || allocation.owner != Owner::Heap
            || allocation.count != Some(Count::Finite(1))
            || self.sealed_controls.get(&control).is_some()
        {
            return fail(
                V::TypeConfusion,
                "sealed control is not a fresh affine allocation",
            );
        }
        let owner = self
            .sealed_owners
            .get_mut(id.0 as usize)
            .ok_or_else(|| err(V::ForgedPointer, "sealed control names an unknown owner"))?;
        if owner.phase != Phase::Live || owner.control.is_some() || owner.root.is_none() {
            return fail(V::CreditUnderflow, "sealed owner has no unbound typed root");
        }
        owner.control = Some(control);
        self.sealed_controls.insert(control, id);
        Ok(())
    }

    pub(crate) fn sealed_control(&self, control: Ptr) -> Checked<SealedId> {
        let allocation = self.check(control)?;
        if allocation.owner != Owner::Heap || allocation.count != Some(Count::Finite(1)) {
            return fail(V::CreditUnderflow, "sealed control lost its affine credit");
        }
        let id = self
            .sealed_controls
            .get(&control)
            .copied()
            .ok_or_else(|| err(V::ForgedPointer, "pointer has no sealed owner binding"))?;
        let owner = &self.sealed_owners[id.0 as usize];
        if owner.phase != Phase::Live || owner.control != Some(control) {
            return fail(V::BorrowExpired, "sealed control owner is not live");
        }
        Ok(id)
    }

    pub(crate) fn begin_sealed_loan(&mut self, id: SealedId) -> Checked<SealedLoan> {
        self.check_sealed_ownership()?;
        let owner = self
            .sealed_owners
            .get_mut(id.0 as usize)
            .ok_or_else(|| err(V::ForgedPointer, "unknown sealed owner"))?;
        if owner.phase != Phase::Live {
            return fail(V::BorrowExpired, "sealed owner is not live");
        }
        let loan = owner.next_loan;
        owner.next_loan = loan
            .checked_add(1)
            .ok_or_else(|| err(V::Malformed, "sealed loan identity overflow"))?;
        owner.loans.insert(loan);
        Ok(SealedLoan {
            owner: id,
            index: loan,
        })
    }
    pub(crate) fn end_sealed_loan(&mut self, id: SealedId, loan: SealedLoan) -> Checked<()> {
        let owner = self
            .sealed_owners
            .get_mut(id.0 as usize)
            .ok_or_else(|| err(V::ForgedPointer, "unknown sealed owner"))?;
        if loan.owner != id || owner.phase != Phase::Live || !owner.loans.remove(&loan.index) {
            return fail(V::BorrowExpired, "sealed loan is no longer live");
        }
        Ok(())
    }

    /// Begin the mechanical drain once. The returned allocation IDs are work,
    /// not additional owners or Shared credits. Existing H drains must release
    /// cells and free storage before finish_sealed_drop may discharge the row.
    pub(crate) fn begin_sealed_drop(&mut self, id: SealedId) -> Checked<Vec<AllocId>> {
        self.check_sealed_ownership()?;
        let owner = self
            .sealed_owners
            .get_mut(id.0 as usize)
            .ok_or_else(|| err(V::ForgedPointer, "unknown sealed owner"))?;
        if owner.phase != Phase::Live {
            return fail(V::CreditUnderflow, "sealed owner was already consumed");
        }
        if !owner.loans.is_empty() {
            return fail(
                V::LoanOutlivesFrame,
                "sealed owner dropped during a live loan",
            );
        }
        owner.phase = Phase::Dropping;
        Ok(owner.allocations.iter().map(|ptr| ptr.alloc).collect())
    }
    pub(crate) fn finish_sealed_drop(&mut self, id: SealedId) -> Checked<()> {
        self.check_sealed_ownership()?;
        let owner = self
            .sealed_owners
            .get(id.0 as usize)
            .ok_or_else(|| err(V::ForgedPointer, "unknown sealed owner"))?;
        if owner.phase != Phase::Dropping {
            return fail(
                V::CreditUnderflow,
                "sealed drain was not entered exactly once",
            );
        }
        if owner
            .allocations
            .iter()
            .any(|id| self.get(id.alloc).state != AllocState::Dead)
        {
            return fail(V::UnexplainedAllocation, "sealed drain left storage live");
        }
        let owner = &mut self.sealed_owners[id.0 as usize];
        owner.root = None;
        owner.phase = Phase::Dead;
        Ok(())
    }
}

impl Ledger {
    /// Ordinary runs have no sealed rows. Avoid scanning every old lexical
    /// scope at each instruction; scope close and terminal audit still run the
    /// full tag/row check, including dangling tags without an owner record.
    pub(crate) fn check_sealed_owners_after_step(&self) -> Checked<()> {
        if self.sealed_owners.is_empty() {
            return Ok(());
        }
        self.check_sealed_ownership()
    }
    /// Independent rows, not a count derived from candidate release commands.
    pub(crate) fn check_sealed_ownership(&self) -> Checked<()> {
        let mut members = BTreeSet::new();
        for (index, owner) in self.sealed_owners.indexed() {
            if owner.phase != Phase::Live && !owner.loans.is_empty() {
                return fail(V::LoanOutlivesFrame, "consumed sealed owner retains loans");
            }
            let scope = self
                .regions
                .get(owner.region)
                .ok_or_else(|| err(V::Malformed, "sealed owner lost its source scope"))?;
            if scope.storage != StorageCustody::Sealed(SealedId(index as u32)) {
                return fail(V::RegionEscape, "scope and sealed storage custody disagree");
            }
            if let Some(control) = owner.control {
                let allocation = self
                    .lookup(control.alloc)
                    .ok_or_else(|| err(V::ForgedPointer, "sealed control storage is missing"))?;
                if allocation.generation != control.generation {
                    return fail(V::StaleGeneration, "sealed control storage was reused");
                }
                if owner.phase == Phase::Live
                    && (allocation.state != AllocState::Live
                        || allocation.count != Some(Count::Finite(1)))
                {
                    return fail(
                        V::CreditUnderflow,
                        "live sealed control lost its affine holder",
                    );
                }
                if owner.phase == Phase::Dead && allocation.state != AllocState::Dead {
                    return fail(
                        V::UnexplainedAllocation,
                        "dead sealed owner retained its control",
                    );
                }
            }
            for ptr in &owner.allocations {
                if !members.insert(ptr.alloc) {
                    return fail(V::RegionEscape, "sealed storage belongs to two owner rows");
                }
                let allocation = self
                    .lookup(ptr.alloc)
                    .ok_or_else(|| err(V::ForgedPointer, "sealed owner lost its storage"))?;
                if allocation.generation != ptr.generation {
                    return fail(V::StaleGeneration, "sealed owner storage was reused");
                }
                if allocation.owner != Owner::Region(owner.region) {
                    return fail(V::RegionEscape, "sealed storage changed owner domains");
                }
                if owner.phase == Phase::Live && !allocation.published {
                    return fail(
                        V::WritePublished,
                        "sealed storage lost its read-only publication",
                    );
                }
                if owner.phase == Phase::Live && allocation.state != AllocState::Live {
                    return fail(V::UseAfterFree, "live sealed owner lost storage");
                }
                if owner.phase == Phase::Dead && allocation.state != AllocState::Dead {
                    return fail(
                        V::UnexplainedAllocation,
                        "dead sealed owner retains live storage",
                    );
                }
            }
        }
        if !self.sealed_owners.is_empty() {
            for (index, allocation) in self.allocations.indexed() {
                if allocation.state == AllocState::Dead {
                    continue;
                }
                if let Owner::Region(region) = allocation.owner
                    && self
                        .regions
                        .get(region)
                        .is_some_and(|r| matches!(r.storage, StorageCustody::Sealed(_)))
                    && !members.contains(&AllocId(index as u32))
                {
                    return fail(
                        V::RegionEscape,
                        "sealed scope contains storage missing from its owner",
                    );
                }
            }
        }
        // A dangling scope-side tag must not bypass ordinary region cleanup.
        for (region, scope) in self.regions.indexed() {
            if let StorageCustody::Sealed(id) = scope.storage {
                if self
                    .sealed_owners
                    .get(id.0 as usize)
                    .is_none_or(|owner| owner.region != region)
                {
                    return fail(V::RegionEscape, "scope names a foreign sealed owner");
                }
            }
        }
        Ok(())
    }
    pub(crate) fn audit_sealed_owners(&self) -> Checked<()> {
        self.check_sealed_ownership()?;
        if self
            .sealed_owners
            .iter()
            .any(|owner| owner.phase != Phase::Dead)
        {
            return fail(V::OpenResource, "sealed storage owner was not drained");
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> (Ledger, Ptr) {
        let mut ledger = Ledger::default();
        ledger.regions.push(Region {
            storage: StorageCustody::Scope,
            instance: RegionInstanceId(0),
            live: true,
            allocations: Snapshot::default(),
            budget: None,
        });
        let data = ledger.alloc(Owner::Region(0), None, 8, 8, 8, None);
        ledger.regions[0].allocations.push(data.alloc);
        (ledger, data)
    }
    #[test]
    fn sealed_custody_transfers_once_and_outlives_lexical_scope_metadata() {
        let (mut ledger, data) = fixture();
        let owner = ledger.transfer_closed_scope(0, 8).unwrap();
        assert_eq!(
            ledger.transfer_closed_scope(0, 8).unwrap_err().kind,
            V::CreditUnderflow
        );
        ledger.regions[0].live = false;
        ledger.check_sealed_ownership().unwrap();
        ledger.check(data).unwrap();
        assert_eq!(
            ledger.audit_sealed_owners().unwrap_err().kind,
            V::OpenResource
        );
        let work = ledger.begin_sealed_drop(owner).unwrap();
        assert_eq!(work, vec![data.alloc]);
        assert_eq!(
            ledger.finish_sealed_drop(owner).unwrap_err().kind,
            V::UnexplainedAllocation
        );
        for id in work {
            ledger.free(id).unwrap();
        }
        ledger.finish_sealed_drop(owner).unwrap();
        ledger.audit_sealed_owners().unwrap();
        assert_eq!(
            ledger.begin_sealed_drop(owner).unwrap_err().kind,
            V::CreditUnderflow
        );
        assert_eq!(
            ledger.finish_sealed_drop(owner).unwrap_err().kind,
            V::CreditUnderflow
        );
    }
    #[test]
    fn sealed_custody_loans_pin_the_owner_and_cannot_be_ended_twice() {
        let (mut ledger, _) = fixture();
        let owner = ledger.transfer_closed_scope(0, 8).unwrap();
        let a = ledger.begin_sealed_loan(owner).unwrap();
        let b = ledger.begin_sealed_loan(owner).unwrap();
        assert_ne!(a, b);
        assert_eq!(
            ledger.begin_sealed_drop(owner).unwrap_err().kind,
            V::LoanOutlivesFrame
        );
        ledger.end_sealed_loan(owner, a).unwrap();
        assert_eq!(
            ledger.end_sealed_loan(owner, a).unwrap_err().kind,
            V::BorrowExpired
        );
        assert_eq!(
            ledger.begin_sealed_drop(owner).unwrap_err().kind,
            V::LoanOutlivesFrame
        );
        ledger.end_sealed_loan(owner, b).unwrap();
        ledger.begin_sealed_drop(owner).unwrap();
        assert_eq!(
            ledger.begin_sealed_loan(owner).unwrap_err().kind,
            V::BorrowExpired
        );
    }
    #[test]
    fn sealed_custody_rejects_dual_ownership_missing_storage_and_foreign_scope_tags() {
        let (mut ledger, data) = fixture();
        let owner = ledger.transfer_closed_scope(0, 8).unwrap();
        for mutation in 0..4 {
            let mut bad = ledger.clone();
            match mutation {
                0 => bad.regions[0].storage = StorageCustody::Scope,
                1 => bad.get_mut(data.alloc).state = AllocState::Dead,
                2 => bad.get_mut(data.alloc).owner = Owner::Heap,
                3 => bad.regions[0].storage = StorageCustody::Sealed(SealedId(owner.0 + 1)),
                _ => unreachable!(),
            }
            assert!(bad.check_sealed_ownership().is_err(), "mutation {mutation}");
        }
        ledger.check_sealed_ownership().unwrap();
    }
    #[test]
    fn sealed_custody_failed_transfer_preserves_the_scopes_storage_obligation() {
        let (mut ledger, data) = fixture();
        let heap = ledger.alloc(Owner::Heap, None, 8, 8, 8, Some(Count::Finite(1)));
        ledger.get_mut(data.alloc).extent.put_cell(
            0,
            8,
            Cell::Data {
                ptr: heap,
                counted: true,
                borrow: None,
            },
        );
        assert_eq!(
            ledger.transfer_closed_scope(0, 8).unwrap_err().kind,
            V::RegionEscape
        );
        assert_eq!(ledger.regions[0].storage, StorageCustody::Scope);
        assert!(ledger.sealed_owners.is_empty());
        assert_eq!(ledger.get(heap.alloc).count, Some(Count::Finite(1)));
        ledger.check(data).unwrap();
    }
    #[test]
    fn sealed_custody_audits_generation_membership_and_loans_from_actual_storage() {
        let (mut ledger, data) = fixture();
        let owner = ledger.transfer_closed_scope(0, 8).unwrap();
        for mutation in 0..4 {
            let mut bad = ledger.clone();
            let expected = match mutation {
                0 => {
                    bad.get_mut(data.alloc).generation += 1;
                    V::StaleGeneration
                }
                1 => {
                    bad.alloc(Owner::Region(0), None, 8, 8, 8, None);
                    V::RegionEscape
                }
                2 => {
                    bad.sealed_owners[owner.0 as usize].phase = Phase::Dropping;
                    bad.sealed_owners[owner.0 as usize].loans.insert(0);
                    V::LoanOutlivesFrame
                }
                3 => {
                    bad.sealed_owners[owner.0 as usize].allocations.clear();
                    V::RegionEscape
                }
                _ => unreachable!(),
            };
            assert_eq!(
                bad.check_sealed_ownership().unwrap_err().kind,
                expected,
                "mutation {mutation}"
            );
        }
        // Closing lexical/recovery metadata does not end an owner's storage.
        ledger.regions[0].live = false;
        let loan = ledger.begin_sealed_loan(owner).unwrap();
        ledger.check(data).unwrap();
        ledger.end_sealed_loan(owner, loan).unwrap();
        for id in ledger.begin_sealed_drop(owner).unwrap() {
            ledger.free(id).unwrap();
        }
        ledger.finish_sealed_drop(owner).unwrap();
        ledger.audit_sealed_owners().unwrap();
    }
    #[test]
    fn sealed_custody_publishes_storage_read_only_and_detects_mutability_restoration() {
        let (mut ledger, data) = fixture();
        assert!(!ledger.get(data.alloc).published);
        let owner = ledger.transfer_closed_scope(0, 8).unwrap();
        assert!(ledger.get(data.alloc).published);
        let mut bad = ledger.clone();
        bad.get_mut(data.alloc).published = false;
        assert_eq!(
            bad.check_sealed_ownership().unwrap_err().kind,
            V::WritePublished
        );
        ledger.check_sealed_ownership().unwrap();
        for id in ledger.begin_sealed_drop(owner).unwrap() {
            ledger.free(id).unwrap();
        }
        ledger.finish_sealed_drop(owner).unwrap();
        ledger.audit_sealed_owners().unwrap();
    }
}

#[cfg(test)]
mod bit_root_tests {
    use super::*;
    use crate::build::Layouts;
    use hirsel_lir::layout_types::{ScalarKind, TargetSpec};

    fn fixture() -> (Ledger, hirsel_lir::LayoutTable, LayoutId, LayoutId) {
        let mut layouts = Layouts::new(TargetSpec::x86_64_unknown_linux_gnu());
        let unit = layouts.unit();
        let scalar = layouts.scalar(ScalarKind::U64);
        let (_, table) = layouts.finish();
        let mut ledger = Ledger::default();
        for i in 0..2 {
            ledger.regions.push(Region {
                storage: StorageCustody::Scope,
                instance: RegionInstanceId(i),
                live: true,
                allocations: Snapshot::default(),
                budget: None,
            });
        }
        (ledger, table, unit, scalar)
    }
    fn bits() -> Extent {
        let mut e = Extent::new(8);
        e.write_bytes(0, &73u64.to_le_bytes());
        e
    }
    fn drain(ledger: &mut Ledger, owner: SealedId) {
        for alloc in ledger.begin_sealed_drop(owner).unwrap() {
            ledger.free(alloc).unwrap();
        }
        ledger.finish_sealed_drop(owner).unwrap();
    }

    #[test]
    fn sealed_bit_roots_snapshot_unit_and_scalar_after_scope_exit() {
        let (mut ledger, table, unit, scalar) = fixture();
        let mut original = bits();
        let a = ledger
            .transfer_closed_scope_with_bit_root(0, 8, &table, unit, Extent::new(0))
            .unwrap();
        let b = ledger
            .transfer_closed_scope_with_bit_root(1, 8, &table, scalar, original.clone())
            .unwrap();
        original.write_bytes(0, &99u64.to_le_bytes());
        for region in 0..2 {
            ledger.regions[region].live = false;
        }
        let la = ledger.begin_sealed_loan(a).unwrap();
        let lb = ledger.begin_sealed_loan(b).unwrap();
        assert_eq!(ledger.sealed_bit_root(a, la, unit).unwrap().len(), 0);
        assert_eq!(
            ledger
                .sealed_bit_root(b, lb, scalar)
                .unwrap()
                .read_bytes(0, 8),
            &73u64.to_le_bytes()
        );
        assert_eq!(
            ledger.begin_sealed_drop(b).unwrap_err().kind,
            V::LoanOutlivesFrame
        );
        ledger.end_sealed_loan(a, la).unwrap();
        ledger.end_sealed_loan(b, lb).unwrap();
        drain(&mut ledger, a);
        drain(&mut ledger, b);
        assert_eq!(
            ledger.sealed_bit_root(b, lb, scalar).unwrap_err().kind,
            V::BorrowExpired
        );
        ledger.audit_sealed_owners().unwrap();
    }

    #[test]
    fn sealed_bit_root_loans_cannot_substitute_another_owner_with_the_same_local_number() {
        let (mut ledger, table, unit, scalar) = fixture();
        let a = ledger
            .transfer_closed_scope_with_bit_root(0, 8, &table, scalar, bits())
            .unwrap();
        let b = ledger
            .transfer_closed_scope_with_bit_root(1, 8, &table, scalar, bits())
            .unwrap();
        let la = ledger.begin_sealed_loan(a).unwrap();
        let lb = ledger.begin_sealed_loan(b).unwrap();
        assert_eq!(la.index, lb.index);
        assert_ne!(la, lb);
        assert_eq!(
            ledger.sealed_bit_root(b, la, scalar).unwrap_err().kind,
            V::BorrowExpired
        );
        assert_eq!(
            ledger.end_sealed_loan(b, la).unwrap_err().kind,
            V::BorrowExpired
        );
        assert_eq!(
            ledger.sealed_bit_root(b, lb, unit).unwrap_err().kind,
            V::TypeConfusion
        );
        assert!(ledger.sealed_bit_root(b, lb, scalar).is_ok());
        ledger.end_sealed_loan(a, la).unwrap();
        ledger.end_sealed_loan(b, lb).unwrap();
        assert_eq!(
            ledger.sealed_bit_root(b, lb, scalar).unwrap_err().kind,
            V::BorrowExpired
        );
        drain(&mut ledger, a);
        drain(&mut ledger, b);
        ledger.audit_sealed_owners().unwrap();
    }

    #[test]
    fn malformed_sealed_bit_roots_leave_scope_custody_unchanged() {
        for mutation in 0..5 {
            let (mut ledger, table, unit, scalar) = fixture();
            let mut e = bits();
            let (layout, expected) = match mutation {
                0 => (unit, V::TypeConfusion),
                1 => {
                    e = Extent::new(8);
                    (scalar, V::UninitializedRead)
                }
                2 => {
                    let p = ledger.alloc(Owner::Heap, None, 8, 8, 8, Some(Count::Finite(1)));
                    e.put_cell(
                        0,
                        8,
                        Cell::Data {
                            ptr: p,
                            counted: true,
                            borrow: None,
                        },
                    );
                    (scalar, V::TypeConfusion)
                }
                3 => {
                    e.resources.insert(vec![], ResourceId(0));
                    (scalar, V::TypeConfusion)
                }
                _ => (LayoutId(999), V::TypeConfusion),
            };
            assert_eq!(
                ledger
                    .transfer_closed_scope_with_bit_root(0, 8, &table, layout, e)
                    .unwrap_err()
                    .kind,
                expected
            );
            assert_eq!(ledger.regions[0].storage, StorageCustody::Scope);
            assert!(ledger.sealed_owners.is_empty());
        }
    }
}

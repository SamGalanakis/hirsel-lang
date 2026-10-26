//! The sealed read adapter checks the actual owner place and H's private root.
//! No candidate descriptor, integer handle or loan annotation creates custody.
use super::*;

impl<S: Stage> Machine<'_, S> {
    pub(super) fn read_sealed_root(
        &mut self,
        f: &mut Frame,
        result: ValueId,
        owner: ValueId,
        loan: LoanId,
        shape: SealDescriptorId,
    ) -> Checked<()> {
        let unit = self.unit(f.unit);
        let descriptor = unit
            .tables
            .seal_descriptors
            .get(shape.index())
            .ok_or_else(|| err(V::Malformed, "sealed read descriptor is missing"))?;
        descriptor
            .check(&self.program.layouts)
            .map_err(|e| err(V::TypeConfusion, e))?;
        let root_layout = descriptor.root_layout;
        let owner_layout = self.value_type(f.unit, owner)?.layout;
        let ty = self.value_type(f.unit, result)?.clone();
        if !matches!(self.record(owner_layout)?.kind,
            LayoutKind::SealedRegion { root } if root == root_layout)
            || ty.layout != root_layout
        {
            return fail(
                V::TypeConfusion,
                "sealed read result or owner has another root layout",
            );
        }
        let bits = matches!(ty.repr, ValueRepr::Unit | ValueRepr::Scalar(_));
        let expected = if bits {
            Holding::Plain
        } else {
            Holding::Borrowed(loan)
        };
        if self.value_info(f.unit, result)?.holding != expected {
            return fail(
                V::TypeConfusion,
                "sealed root read has incorrect loan holding",
            );
        }
        let info = self
            .unit(f.unit)
            .tables
            .loans
            .get(loan.index())
            .ok_or_else(|| err(V::Malformed, "sealed owner loan is missing"))?;
        if !info.mutable || self.address_layout(f.unit, info.address)? != owner_layout {
            return fail(
                V::BorrowExpired,
                "sealed owner loan is not exclusive for this layout",
            );
        }
        self.check_borrow(f, Borrow::Loan(loan))?;
        let backing = f.loans[loan.index()].expect("checked active owner loan");
        let Val::Ptr(control) = self.read_use(f, owner)? else {
            return fail(
                V::ForgedPointer,
                "sealed owner is not a checked control pointer",
            );
        };
        if self.read_ptr(backing)? != control {
            return fail(
                V::ViewDetached,
                "sealed owner differs from the actual loaned place",
            );
        }
        if self.ledger.check(control)?.layout != Some(owner_layout) {
            return fail(
                V::TypeConfusion,
                "sealed control has another physical owner layout",
            );
        }
        let id = self.ledger.sealed_control(control)?;
        let pinned = match f.sealed_loans[loan.index()] {
            Some((old, pinned)) if old == id => pinned,
            Some(_) => {
                return fail(
                    V::ViewDetached,
                    "owner loan was rebound to another sealed owner",
                );
            }
            None => {
                let pinned = self.ledger.begin_sealed_loan(id)?;
                f.sealed_loans[loan.index()] = Some((id, pinned));
                pinned
            }
        };
        let image = self
            .ledger
            .sealed_bit_root(id, pinned, root_layout)?
            .clone();
        let (value, _, _) = self.val_from(image, ty.repr, ty.layout)?;
        self.define_fresh(f, result, value)
    }
}

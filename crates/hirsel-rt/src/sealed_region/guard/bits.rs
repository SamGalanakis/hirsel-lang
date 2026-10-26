//! Complete pointer-free plans derive from a checked native descriptor and
//! actual demanded scope state. No caller supplies a charge or physical graph.
use super::*;
use crate::sealed_region::receipt::semantic::Origin;
use crate::sealed_region::{Charge, Root};

impl DemandedGuard<'_> {
    /// Seal a pointer-free root from an empty demanded scope. Both semantic
    /// retained entries and physical backing must be absent; a dead entry or
    /// elided allocation cannot silently turn into an empty plan.
    ///
    /// This Rust boundary does not activate public native seal. Original
    /// source/type correspondence and emitted receipt transport remain checked
    /// compiler obligations, including exclusion of pointer-valued scalars.
    ///
    /// # Safety
    /// The entered invocation owns this grant and `value`, and no grant/root
    /// loan remains. `value` is readable and fully initialized, including any
    /// padding, at `index`'s exact pointer-free layout (null only for Unit).
    /// `root` is its independently validated
    /// original source snapshot, not candidate-invented provenance. All source
    /// allocation events have reached this guard's journal, even if physical
    /// allocation was elided. The existing seal/drop lifetime contract applies.
    pub unsafe fn seal_bits(
        &mut self,
        index: u32,
        value: *mut u8,
        root: &Receipt<Record>,
    ) -> Result<*mut SealedRegion, Error> {
        let inventory = self.inventory()?;
        let charge = inventory
            .seal_charge(root)
            .map_err(|error| Error::Seal(journal::SealError::Semantic(error)))?;
        // Check semantic history independently of physical storage. Empty or
        // zero-cost retained blocks still require the general graph producer.
        if inventory.entries().len() != 0
            || unsafe { !(*self.base()).segment.is_null() || !(*self.base()).cleanup.is_null() }
        {
            return Err(Error::InventoryRequired);
        }
        let layout = unsafe { &*(*self.context()).descriptor(index) };
        if layout.finalizer.is_some()
            || layout.step_count != 0
            || layout.scan_words != 0
            || layout.flags != 0
            || !layout.logical_count.is_null()
        {
            return Err(Error::RootLayout);
        }
        let extent = usize::try_from(layout.size).map_err(|_| Error::RootLayout)?;
        let align = usize::try_from(layout.align).map_err(|_| Error::RootLayout)?;
        if !align.is_power_of_two()
            || (extent != 0 && (value.is_null() || !value.addr().is_multiple_of(align)))
        {
            return Err(Error::RootLayout);
        }
        let record = root.record();
        let Some(image) = root.bytes() else {
            return Err(Error::RootImage);
        };
        if record.origin() != Origin::Inline
            || record.logical_charge() != Charge::Bytes(0)
            || record.heap_charge() != Charge::Bytes(0)
            || record.outgoing_heap_charge() != Charge::Bytes(0)
            || root.children().len() != 0
            || image.len() != extent
            || root.alignment().is_none_or(|alignment| alignment < align)
            || charge != Charge::Bytes(0)
            || (extent != 0 && image != unsafe { std::slice::from_raw_parts(value, extent) })
        {
            return Err(Error::RootImage);
        }
        let plan = Plan {
            root: Root {
                descriptor: index,
                source: value,
                extent,
                align,
                patches: Vec::new(),
            },
            blocks: Vec::new(),
            shared: Vec::new(),
            charge,
        };
        unsafe { self.seal(root, &plan) }
    }
}

#[cfg(all(test, feature = "diagnostic"))]
mod tests;

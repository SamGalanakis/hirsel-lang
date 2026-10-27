//! Private semantic snapshots derived from H's checked cells. Snapshot images
//! carry no ledger holders and never keep language storage alive.
use super::*;
use hirsel_lir::operation::NativeType;
use hirsel_lir::sealed_payload_source::{PayloadProducer, SourcePayload};

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct SnapshotValue {
    root: NativeType,
    value: Val,
    cells: BTreeMap<AllocId, SnapshotAllocation>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct SnapshotAllocation {
    generation: u32,
    layout: Option<LayoutId>,
    owner: Owner,
    shared: bool,
    image: Extent,
}

impl<S: Stage> Machine<'_, S> {
    pub(super) fn receipt_snapshot(
        &self,
        root: NativeType,
        mut value: Val,
    ) -> Checked<SnapshotValue> {
        self.check_val(&value)?;
        fn normalize(image: &mut Extent) {
            for cell in image.cells.values_mut() {
                if let Cell::Data {
                    counted, borrow, ..
                } = cell
                {
                    *counted = false;
                    *borrow = None;
                }
            }
        }
        if let Val::Image(image) = &mut value {
            normalize(image);
        }
        let mut pending = Vec::new();
        let children = |value: &Val| match value {
            Val::Ptr(ptr) => vec![*ptr],
            Val::Image(image) => image
                .cells
                .values()
                .filter_map(|cell| match cell {
                    Cell::Data { ptr, .. } => Some(*ptr),
                    _ => None,
                })
                .collect(),
            _ => Vec::new(),
        };
        pending.extend(children(&value));
        let mut cells = BTreeMap::new();
        while let Some(ptr) = pending.pop() {
            let allocation = self.ledger.check(ptr)?;
            if cells.contains_key(&ptr.alloc) {
                continue;
            }
            let mut image = allocation.extent.copy(0, allocation.extent.len(), None);
            normalize(&mut image);
            for cell in image.cells.values() {
                if let Cell::Data { ptr, .. } = cell {
                    pending.push(*ptr);
                }
            }
            cells.insert(
                ptr.alloc,
                SnapshotAllocation {
                    generation: ptr.generation,
                    layout: allocation.layout,
                    owner: allocation.owner,
                    shared: matches!(allocation.count, Some(Count::Root(_))),
                    image,
                },
            );
        }
        Ok(SnapshotValue { root, value, cells })
    }

    /// Recount historical semantic cells in a private ledger image. No source
    /// storage is revived, no holders are acquired, and the real ledger is
    /// restored before returning an error or a charge.
    pub(super) fn receipt_occurrences(
        &mut self,
        snapshot: &SnapshotValue,
        descriptor: &hirsel_lir::sealed::SealDescriptor,
        region: usize,
        arena_seen: &mut std::collections::BTreeSet<AllocId>,
        shared: &mut BTreeMap<Ptr, u64>,
    ) -> Checked<Option<u64>> {
        if snapshot.root != descriptor.root_type {
            return fail(
                V::TypeConfusion,
                "snapshot charge has another semantic root",
            );
        }
        let mut historical = self.ledger.clone();
        let mut seen = arena_seen.clone();
        for (&id, saved) in &snapshot.cells {
            let row = historical.get_mut(id);
            row.generation = saved.generation;
            row.state = AllocState::Live;
            row.layout = saved.layout;
            row.owner = saved.owner;
            row.count = saved.shared.then_some(Count::Root(1));
            row.extent = saved.image.clone();
            row.size = row.extent.len();
            if saved.owner == Owner::Region(region) {
                seen.insert(id);
            }
        }
        // Each arena allocation owns only its own heap edges. A nested arena
        // block has a separate, possibly newer, inventory snapshot.
        if let Val::Ptr(root) = snapshot.value
            && snapshot
                .cells
                .get(&root.alloc)
                .is_some_and(|saved| saved.owner == Owner::Region(region))
        {
            seen.remove(&root.alloc);
        }
        let physical = std::mem::replace(&mut self.ledger, historical);
        let charge = self.seal_occurrences(descriptor, &snapshot.value, region, &mut seen, shared);
        self.ledger = physical;
        arena_seen.extend(seen);
        charge
    }

    pub(super) fn receipt_control(
        &self,
        control: Ptr,
        layout: LayoutId,
    ) -> Checked<&SnapshotValue> {
        let allocation = self.ledger.check(control)?;
        if control.offset != 0
            || allocation.layout != Some(layout)
            || !matches!(self.record(layout)?.kind, LayoutKind::PayloadReceipt { .. })
        {
            return fail(
                V::TypeConfusion,
                "receipt has another private control layout",
            );
        }
        self.receipts
            .get(&control.alloc)
            .ok_or_else(|| err(V::ForgedPointer, "receipt has no H semantic snapshot"))
    }

    pub(super) fn check_receipt_cell(&self, control: Ptr, layout: LayoutId) -> Checked<()> {
        self.receipt_control(control, layout).map(|_| ())
    }

    pub(super) fn check_receipt_pair(
        &self,
        control: Ptr,
        layout: LayoutId,
        root: &NativeType,
        value: Val,
    ) -> Checked<()> {
        let expected = self.receipt_control(control, layout)?;
        let actual = self.receipt_snapshot(root.clone(), value)?;
        if *expected != actual {
            return fail(V::TypeConfusion, "receipt differs from the paired H cells");
        }
        Ok(())
    }

    pub(super) fn payload_receipt(
        &mut self,
        f: &mut Frame,
        result: ValueId,
        value: ValueId,
        shape: SealDescriptorId,
        binding: PayloadBindingId,
        inputs: PoolRange<ReceiptInput>,
    ) -> Checked<()> {
        let unit = self.unit(f.unit);
        let descriptor = unit
            .tables
            .seal_descriptors
            .get(shape.index())
            .ok_or_else(|| err(V::Malformed, "receipt descriptor is missing"))?;
        descriptor
            .check(&self.program.layouts)
            .map_err(|e| err(V::TypeConfusion, e))?;
        if !self.config.seal_descriptors.contains(descriptor) {
            return fail(
                V::TypeConfusion,
                "receipt descriptor differs from validated input",
            );
        }
        let (selected, binding) = unit
            .tables
            .payload_binding(binding)
            .ok_or_else(|| err(V::Malformed, "receipt binding is missing"))?;
        if selected != shape {
            return fail(
                V::TypeConfusion,
                "receipt binding selects another descriptor",
            );
        }
        let source = self
            .config
            .seal_sources
            .iter()
            .find(|source| {
                source.reference.definition == unit.header.definition
                    && source.reference.instance == unit.header.instance
            })
            .ok_or_else(|| err(V::Malformed, "receipt has no independent source payload"))?;
        source.check().map_err(|e| err(V::Malformed, e))?;
        let expected = SourcePayload {
            root_type: descriptor.root_type.clone(),
            recipe: binding.recipe.clone(),
        };
        if source.witness.implementation_revision != unit.header.implementation_revision
            || source
                .payloads
                .iter()
                .filter(|payload| **payload == expected)
                .count()
                != 1
        {
            return fail(
                V::Malformed,
                "receipt differs from independent source payload",
            );
        }
        let sites = unit
            .tables
            .constructs
            .iter()
            .filter(|site| {
                unit.tables.source_anchors.get(site.anchor.index()) == Some(&binding.recipe.source)
            })
            .collect::<Vec<_>>();
        if !matches!(sites.as_slice(), [site] if
            site.fields.get(&unit.pools.fields).is_some_and(|fields|
                fields.iter().map(|field| field.value).eq(binding.values.iter().copied())))
        {
            return fail(
                V::Malformed,
                "receipt does not preserve actual construction operands",
            );
        }
        let inputs = self.pool(inputs, &unit.pools.receipt_inputs)?;
        binding
            .check_receipt_inputs(inputs)
            .map_err(|e| err(V::Malformed, e))?;
        for (input, original) in inputs.iter().zip(&binding.recipe.inputs) {
            if let Some(receipt) = input.receipt {
                let Val::Ptr(control) = self.read_use(f, receipt)? else {
                    return fail(V::ForgedPointer, "receipt child has no private control");
                };
                let layout = self.value_type(f.unit, receipt)?.layout;
                let snapshot = self.receipt_control(control, layout)?;
                // Original operands may have been consumed by construction.
                // Inspect their H identity without rereading their storage.
                if snapshot.root != original.ty
                    || snapshot.value != self.get(f, input.original)?.val
                {
                    return fail(
                        V::TypeConfusion,
                        "receipt child names another original value",
                    );
                }
                self.check_receipt_pair(
                    control,
                    layout,
                    &original.ty,
                    self.get(f, input.original)?.val.clone(),
                )?;
            } else {
                let plain = self.config.seal_descriptors.iter().any(|d| {
                    d.root_type == original.ty && d.bits_root(&self.program.layouts).is_ok()
                });
                if !plain && !matches!(original.producer, PayloadProducer::Literal(_)) {
                    return fail(V::Malformed, "receipt omits original child provenance");
                }
            }
        }
        let root = descriptor.root_type.clone();
        let root_layout = descriptor.root_layout;
        let source_anchor = binding.recipe.source.clone();
        let layout = self.value_type(f.unit, result)?.layout;
        if self.value_type(f.unit, result)?.repr != ValueRepr::PayloadReceipt
            || !matches!(&self.record(layout)?.kind, LayoutKind::PayloadReceipt { root: ty } if *ty == root)
            || self.value_type(f.unit, value)?.layout != root_layout
        {
            return fail(V::TypeConfusion, "receipt root and private layout differ");
        }
        let value = self.read_use(f, value)?;
        let Val::Ptr(ptr) = &value else {
            return fail(
                V::Unsupported,
                "receipt needs H inline construction correspondence",
            );
        };
        if self.seal_allocations.get(&ptr.alloc) != Some(&(f.unit, source_anchor)) {
            return fail(V::Malformed, "receipt has no actual H construction event");
        }
        self.check_receipt_construction(f, &sites, binding, root_layout, &value)?;
        let snapshot = self.receipt_snapshot(root, value)?;
        let control = self.heap_alloc(layout, self.word, self.record(layout)?.align)?;
        self.receipts.insert(control.alloc, snapshot);
        self.define_fresh(f, result, Val::Ptr(control))
    }

    fn check_receipt_construction(
        &self,
        f: &Frame,
        sites: &[&ConstructSite],
        binding: &hirsel_lir::sealed_payload::PayloadBinding,
        layout: LayoutId,
        value: &Val,
    ) -> Checked<()> {
        use hirsel_lir::sealed_payload_source::{PayloadDestination, PayloadOperation};
        let [site] = sites else {
            return fail(V::Malformed, "receipt construction is ambiguous");
        };
        if !matches!(
            binding.recipe.operation,
            PayloadOperation::Allocate {
                destination: PayloadDestination::Heap
            }
        ) || !matches!(
            site.destination,
            DestinationClass::Heap | DestinationClass::Inferred
        ) || site.layout != layout
        {
            return fail(
                V::Malformed,
                "receipt needs its original ordinary heap construction",
            );
        }
        let (Val::Ptr(ptr), LayoutKind::Block { payload }) = (value, &self.record(layout)?.kind)
        else {
            return fail(
                V::Unsupported,
                "H receipt needs sequence construction correspondence",
            );
        };
        let LayoutKind::Sequence { element } = self.record(*payload)?.kind else {
            return fail(
                V::Unsupported,
                "H receipt needs sequence construction correspondence",
            );
        };
        if site.constructor != ConstructorKind::Sequence
            || ptr.offset != 0
            || self.ledger.check(*ptr)?.layout != Some(*payload)
        {
            return fail(
                V::TypeConfusion,
                "receipt construction has another physical payload",
            );
        }
        let image = &self.ledger.check(*ptr)?.extent;
        let count = self.scan_scalar(*ptr, 8)? as u64;
        if count != binding.values.len() as u64 {
            return fail(
                V::TypeConfusion,
                "receipt construction length differs from original operands",
            );
        }
        let mut scan = self.scan_cell_offsets(image, *payload, true)?;
        scan.extend(self.scan_cell_offsets(image, *payload, false)?);
        let scanned = scan.into_iter().collect::<std::collections::BTreeSet<_>>();
        let actual = image
            .cells
            .keys()
            .copied()
            .collect::<std::collections::BTreeSet<_>>();
        if scanned != actual || !image.resources.is_empty() {
            return fail(
                V::TypeConfusion,
                "receipt construction cells differ from its complete scan",
            );
        }
        let (start, stride) = self.sequence_shape(element)?;
        let size = self.size(element)?;
        for (index, (value, original)) in binding
            .values
            .iter()
            .zip(&binding.recipe.inputs)
            .enumerate()
        {
            let ty = self.value_type(f.unit, *value)?;
            if ty.layout != element {
                return fail(V::TypeConfusion, "receipt input has another element layout");
            }
            let offset = start
                .checked_add(
                    (index as u64)
                        .checked_mul(stride)
                        .ok_or_else(|| err(V::OutOfExtent, "receipt element offset overflow"))?,
                )
                .ok_or_else(|| err(V::OutOfExtent, "receipt element offset overflow"))?;
            image.check_range(offset, size)?;
            let child = self
                .val_from(image.copy(offset, size, None), ty.repr, element)?
                .0;
            let expected =
                self.receipt_snapshot(original.ty.clone(), self.get(f, *value)?.val.clone())?;
            let actual = self.receipt_snapshot(original.ty.clone(), child)?;
            if expected != actual {
                return fail(
                    V::TypeConfusion,
                    "receipt construction differs from original operand cells",
                );
            }
        }
        Ok(())
    }

    pub(super) fn bind_receipt_call(
        &mut self,
        caller: &mut Frame,
        callee: &mut Frame,
        site: &CallSite,
    ) -> Checked<()> {
        let unit = self.unit(callee.unit);
        let abi = &self.program.layouts.fn_abis[unit.header.abi.0 as usize];
        hirsel_lir::receipt_transport::check_call_contract(
            abi,
            site,
            &self.unit(caller.unit).pools,
        )
        .map_err(|e| err(V::Malformed, e))?;
        let Some(contract) = &abi.receipts else {
            return Ok(());
        };
        if site.pretransfer_failure.is_some() {
            return fail(
                V::Unsupported,
                "H receipt rejection custody is not implemented",
            );
        }
        let source = self
            .config
            .receipt_sources
            .iter()
            .find(|source| {
                source.original.definition == unit.header.definition
                    && source.instance == unit.header.instance
            })
            .ok_or_else(|| {
                err(
                    V::Malformed,
                    "receipt entry lacks independent source declaration",
                )
            })?;
        source.check_structure().map_err(|e| err(V::Malformed, e))?;
        if source.contract != *contract
            || source.implementation_revision != unit.header.implementation_revision
        {
            return fail(
                V::Malformed,
                "receipt entry differs from independent source declaration",
            );
        }
        let entry = unit
            .tables
            .receipt_entry
            .as_ref()
            .ok_or_else(|| err(V::Malformed, "receipt callee has no entry bindings"))?;
        let receipts = site.receipts.as_ref().unwrap();
        let arguments = self.pool(site.arguments, &self.unit(caller.unit).pools.call_arguments)?;
        for ((binding, parameter), carrier) in entry
            .parameters
            .iter()
            .zip(&contract.parameters)
            .zip(&receipts.arguments)
        {
            let ordinary = arguments
                .iter()
                .find(|argument| {
                    abi.parameters[argument.parameter as usize].source == parameter.source
                })
                .ok_or_else(|| err(V::Malformed, "receipt lacks ordinary argument"))?;
            let value = match ordinary.value {
                ArgumentValue::Value(value) => self.read_use(caller, value)?,
                ArgumentValue::Address { address, .. } => {
                    let resolved = self.resolve_use(caller, address)?;
                    let physical = &abi.parameters[ordinary.parameter as usize];
                    let repr = self
                        .value_type(callee.unit, unit.parameters[ordinary.parameter as usize])?
                        .repr;
                    let image = self.ledger.check(resolved.ptr)?.extent.copy(
                        resolved.ptr.offset,
                        self.size(physical.layout)?,
                        None,
                    );
                    self.val_from(image, repr, physical.layout)?.0
                }
            };
            let (receipt, take) = match *carrier {
                ReceiptArgument::Borrow { receipt, .. } => (receipt, false),
                ReceiptArgument::Take { receipt, .. } => (receipt, true),
                ReceiptArgument::Mut { .. } => {
                    return fail(
                        V::Unsupported,
                        "H mutable receipt custody is not implemented",
                    );
                }
            };
            let Val::Ptr(control) = self.read_use(caller, receipt)? else {
                return fail(V::ForgedPointer, "receipt call has no private control");
            };
            let layout = self.value_type(caller.unit, receipt)?.layout;
            let profile = source
                .profile(hirsel_lir::sealed_payload_source::ReceiptPort::Parameter(
                    parameter.source,
                ))
                .ok_or_else(|| {
                    err(
                        V::Malformed,
                        "receipt entry lacks complete parameter profile",
                    )
                })?;
            self.check_receipt_pair(control, layout, &profile.root.ty, value)?;
            let target_layout = unit.types[unit.slots[binding.slot.index()].ty.index()].layout;
            if layout != target_layout {
                return fail(V::TypeConfusion, "receipt call and entry layouts differ");
            }
            let moved = if take {
                self.take_owned(caller, receipt, "take receipt argument")?
            } else {
                Moved {
                    val: Val::Ptr(control),
                    credited: false,
                    borrow: Some(Borrow::Parameter(ordinary.parameter)),
                }
            };
            let storage = self.ledger.alloc(
                Owner::Frame,
                Some(layout),
                self.word,
                self.word,
                self.record(layout)?.align,
                None,
            );
            self.write_value_as(storage, layout, moved, !take)?;
            self.ledger.get_mut(storage.alloc).published = !take;
            callee.slots[binding.slot.index()] = SlotStore::Own(storage);
            callee.versions[binding.slot.index()] = Some(binding.version);
            self.define_version(callee, binding.version)?;
        }
        Ok(())
    }

    pub(super) fn check_receipt_slot_lifetime(
        &self,
        f: &Frame,
        slot: SlotId,
        initializing: bool,
    ) -> Checked<()> {
        let unit = self.unit(f.unit);
        let SlotKind::ReceiptParameter(source) = unit.slots[slot.index()].kind else {
            return Ok(());
        };
        let mode = self.program.layouts.fn_abis[unit.header.abi.0 as usize]
            .receipts
            .as_ref()
            .and_then(|contract| contract.parameters.iter().find(|p| p.source == source))
            .ok_or_else(|| err(V::Malformed, "receipt slot lacks entry custody"))?
            .mode;
        if initializing || mode == hirsel_lir::sealed_payload_source::ReceiptMode::Borrow {
            return fail(
                V::BorrowExpired,
                "receipt entry storage must survive to frame exit",
            );
        }
        Ok(())
    }

    pub(super) fn audit_receipts(&self) -> Checked<()> {
        for &id in self.receipts.keys() {
            let allocation = self.ledger.get(id);
            if allocation.state != AllocState::Live
                || !matches!(allocation.count, Some(Count::Finite(1..)))
            {
                return fail(
                    V::CreditUnderflow,
                    "private receipt control escaped its release protocol",
                );
            }
        }
        Ok(())
    }

    pub(super) fn release_receipt(&mut self, at: Ptr, layout: LayoutId) -> Checked<()> {
        let Cell::Data {
            ptr,
            counted: true,
            borrow: None,
        } = self.read_cell(at)?
        else {
            return fail(
                V::CreditUnderflow,
                "receipt release requires an owning metadata cell",
            );
        };
        self.receipt_control(ptr, layout)?;
        let word = self.word;
        self.ledger
            .get_mut(at.alloc)
            .extent
            .take(at.offset, word, FREE_POISON, Some((&[], &[])));
        if self.count_down(ptr)? {
            self.receipts.remove(&ptr.alloc);
            self.ledger.free(ptr.alloc)?;
        }
        Ok(())
    }
}

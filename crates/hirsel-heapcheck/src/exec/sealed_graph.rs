//! Direct graph sealing over H's cells. Source inputs are supplied separately
//! from candidate units. Physical relocation deduplicates allocations; the
//! logical walk visits heap occurrences and stops at Shared boundaries.
use super::compare::At;
use super::projection::{Location, Projection, SharedTraversal};
use super::*;
use crate::ledger::sealed::SealedId;
use hirsel_lir::codec::CodecNode as Node;
use hirsel_lir::logical_storage::{LogicalBytes, StorageClass};
use hirsel_lir::sealed::SealDescriptor;
use hirsel_lir::sealed_payload_source::SourceEventKind;
use std::collections::BTreeSet;

#[derive(Clone)]
pub(super) struct Entry {
    value: Val,
    descriptor: SealDescriptor,
    snapshot: super::receipts::SnapshotValue,
    events: Vec<(usize, SourceAnchor)>,
}

impl<S: Stage> Machine<'_, S> {
    /// Check the complete static event correspondence once, including bodies
    /// that this run never enters. Runtime inventory belongs to actual scopes;
    /// an uncalled function or another arena cannot contribute to that scope.
    pub(super) fn check_seal_sources(&self) -> Checked<()> {
        let mut selected = BTreeSet::new();
        for source in &self.config.seal_sources {
            source.check().map_err(|e| err(V::Malformed, e))?;
            if !selected.insert((
                source.reference.definition.clone(),
                source.reference.instance,
            )) {
                return fail(
                    V::Malformed,
                    "seal source inventory is ambiguous or invalid",
                );
            }
            let units = self
                .program
                .units
                .iter()
                .filter(|unit| {
                    unit.header.definition == source.reference.definition
                        && unit.header.instance == source.reference.instance
                })
                .collect::<Vec<_>>();
            let [unit] = units.as_slice() else {
                return fail(V::Malformed, "seal source does not select one linked body");
            };
            if unit.header.implementation_revision != source.witness.implementation_revision {
                return fail(V::Malformed, "seal source implementation revision differs");
            }
            let mut observed = Vec::new();
            for block in &unit.blocks {
                for inst in block
                    .instructions
                    .get(&unit.instructions)
                    .ok_or_else(|| err(V::Malformed, "seal inventory block range"))?
                {
                    let (shape, anchor, kind) = match *inst {
                        Inst::ArenaInventory {
                            shape,
                            allocation,
                            previous,
                            ..
                        } => (
                            shape,
                            allocation,
                            if previous.is_some() {
                                SourceEventKind::Replace
                            } else {
                                SourceEventKind::Allocate
                            },
                        ),
                        Inst::SealArena { shape, origin, .. } => {
                            (shape, origin, SourceEventKind::Seal)
                        }
                        Inst::BorrowSealedRoot { shape, .. } => {
                            (shape, block.anchor, SourceEventKind::BorrowRoot)
                        }
                        _ => continue,
                    };
                    let descriptor = unit
                        .tables
                        .seal_descriptors
                        .get(shape.index())
                        .ok_or_else(|| err(V::Malformed, "seal inventory descriptor is missing"))?;
                    descriptor
                        .check(&self.program.layouts)
                        .map_err(|e| err(V::TypeConfusion, e))?;
                    if !self.config.seal_descriptors.contains(descriptor) {
                        return fail(
                            V::TypeConfusion,
                            "seal descriptor differs from validated module input",
                        );
                    }
                    let anchor = unit
                        .tables
                        .source_anchors
                        .get(anchor.index())
                        .ok_or_else(|| err(V::Malformed, "seal inventory anchor is missing"))?;
                    observed.push(hirsel_lir::sealed::SourceEvent {
                        anchor: anchor.clone(),
                        kind,
                        root_type: descriptor.root_type.clone(),
                    });
                }
            }
            let mut payloads = Vec::new();
            for table in &unit.tables.seal_payloads {
                let descriptor = unit
                    .tables
                    .seal_descriptors
                    .get(table.descriptor.index())
                    .ok_or_else(|| err(V::Malformed, "source payload descriptor is missing"))?;
                for binding in &table.bindings {
                    payloads.push(hirsel_lir::sealed_payload_source::SourcePayload {
                        root_type: descriptor.root_type.clone(),
                        recipe: binding.recipe.clone(),
                    });
                }
            }
            if payloads != source.payloads {
                return fail(
                    V::Malformed,
                    "independent source payload correspondence differs",
                );
            }
            if observed != source.events {
                return fail(
                    V::Malformed,
                    "independent retained allocation event correspondence differs",
                );
            }
        }
        Ok(())
    }

    fn seal_source(
        &self,
        f: &Frame,
        anchor: SourceAnchorId,
        kind: SourceEventKind,
        descriptor: &SealDescriptor,
    ) -> Checked<SourceAnchor> {
        let unit = self.unit(f.unit);
        let at = unit
            .tables
            .source_anchors
            .get(anchor.index())
            .ok_or_else(|| err(V::Malformed, "seal event has no source anchor"))?;
        let mut inputs = self.config.seal_sources.iter().filter(|input| {
            input.reference.definition == unit.header.definition
                && input.reference.instance == unit.header.instance
        });
        let input = inputs.next().ok_or_else(|| {
            err(
                V::Unsupported,
                "seal graph has no independent source inventory",
            )
        })?;
        if inputs.next().is_some()
            || input.check().is_err()
            || input.witness.implementation_revision != unit.header.implementation_revision
        {
            return fail(
                V::Malformed,
                "seal source inventory is ambiguous or invalid",
            );
        }
        if input
            .events
            .iter()
            .filter(|e| e.anchor == *at && e.kind == kind && e.root_type == descriptor.root_type)
            .count()
            != 1
        {
            return fail(V::Malformed, "seal event differs from independent source");
        }
        descriptor
            .check(&self.program.layouts)
            .map_err(|e| err(V::TypeConfusion, e))?;
        if !self.config.seal_descriptors.contains(descriptor) {
            return fail(
                V::TypeConfusion,
                "seal descriptor differs from validated module input",
            );
        }
        Ok(at.clone())
    }

    pub(super) fn record_seal_inventory(
        &mut self,
        f: &Frame,
        arena: ValueId,
        allocation: SourceAnchorId,
        value: ValueId,
        shape: SealDescriptorId,
        previous: Option<ValueId>,
    ) -> Checked<()> {
        let descriptor = self
            .unit(f.unit)
            .tables
            .seal_descriptors
            .get(shape.index())
            .ok_or_else(|| err(V::Malformed, "inventory descriptor is missing"))?
            .clone();
        let kind = if previous.is_some() {
            SourceEventKind::Replace
        } else {
            SourceEventKind::Allocate
        };
        let anchor = self.seal_source(f, allocation, kind, &descriptor)?;
        let Val::Arena(Some(region)) = self.read_use(f, arena)? else {
            return fail(V::ForgedPointer, "inventory has no explicit arena");
        };
        let scope = self
            .ledger
            .regions
            .get(region)
            .ok_or_else(|| err(V::ForgedPointer, "inventory names an unknown scope"))?;
        if !scope.live || scope.storage != crate::ledger::sealed::StorageCustody::Scope {
            return fail(V::CreditUnderflow, "inventory uses a consumed arena");
        }
        if self.value_type(f.unit, value)?.layout != descriptor.root_layout {
            return fail(V::TypeConfusion, "inventory value differs from descriptor");
        }
        let val = self.read_use(f, value)?;
        let Val::Ptr(ptr) = val else {
            return fail(
                V::Unsupported,
                "inventory requires a materialized semantic allocation",
            );
        };
        if self.ledger.check(ptr)?.owner != Owner::Region(region) || ptr.offset != 0 {
            return fail(
                V::RegionEscape,
                "inventory allocation belongs to another owner",
            );
        }
        if previous.is_none()
            && self.seal_allocations.get(&ptr.alloc) != Some(&(f.unit, anchor.clone()))
        {
            return fail(
                V::Malformed,
                "inventory does not name the actual H construction event",
            );
        }
        let mut events = vec![(f.unit, anchor)];
        let old = if let Some(previous) = previous {
            // Growth may have retired its old physical backing already; its
            // recorded semantic identity, not a fresh data read, is required.
            let Val::Ptr(old) = self.get(f, previous)?.val else {
                return fail(
                    V::TypeConfusion,
                    "inventory replacement has no previous storage",
                );
            };
            let entry = self
                .seal_inventory
                .get(&region)
                .and_then(|e| e.get(&old.alloc))
                .ok_or_else(|| {
                    err(
                        V::Malformed,
                        "inventory replacement has no semantic allocation",
                    )
                })?;
            let Val::Ptr(recorded) = entry.value else {
                unreachable!()
            };
            if recorded != old || self.ledger.get(old.alloc).generation != old.generation {
                return fail(
                    V::StaleGeneration,
                    "inventory replacement changed allocation generation",
                );
            }
            events.extend(entry.events.clone());
            Some(old.alloc)
        } else {
            None
        };
        let snapshot = self.receipt_snapshot(descriptor.root_type.clone(), Val::Ptr(ptr))?;
        let entries = self.seal_inventory.entry(region).or_default();
        if entries.contains_key(&ptr.alloc) && old != Some(ptr.alloc) {
            return fail(
                V::Malformed,
                "inventory repeats an allocation without replacement",
            );
        }
        if let Some(old) = old {
            entries.remove(&old);
        }
        entries.insert(
            ptr.alloc,
            Entry {
                value: Val::Ptr(ptr),
                descriptor,
                snapshot,
                events,
            },
        );
        Ok(())
    }

    /// Tree charge over logical projection, with arena allocations visited once
    /// across inventory and root. A repeated heap allocation is visited again.
    pub(super) fn seal_occurrences(
        &self,
        descriptor: &SealDescriptor,
        value: &Val,
        region: usize,
        arena_seen: &mut BTreeSet<AllocId>,
        shared: &mut BTreeMap<Ptr, u64>,
    ) -> Checked<Option<u64>> {
        let image = self.projection_image(&descriptor.projection, value)?;
        let mut pending = vec![Projection {
            node: descriptor.projection.root,
            location: Location::Stored(At::Image(&image, 0)),
        }];
        let mut total = Some(0u64);
        while let Some(p) = pending.pop() {
            let view = self.projection_view(&descriptor.projection, p, SharedTraversal::Stop)?;
            let at = match p.location {
                Location::Stored(at) => Some(at),
                _ => None,
            };
            if view.shared {
                let ptr =
                    self.ptr_at(at.ok_or_else(|| err(V::Malformed, "Shared column root"))?)?;
                let allocation = self.ledger.check(ptr)?;
                let LayoutKind::Shared { payload } = self.record(view.node.layout())?.kind else {
                    return fail(V::TypeConfusion, "Shared projection has no root layout");
                };
                if ptr.offset != 0
                    || allocation.layout != Some(payload)
                    || !matches!(allocation.count, Some(Count::Root(_)))
                {
                    return fail(V::CountMismatch, "semantic Shared has no root count");
                }
                let count = shared.entry(ptr).or_default();
                *count = count
                    .checked_add(1)
                    .ok_or_else(|| err(V::CountMismatch, "Shared multiplicity overflow"))?;
                continue;
            }
            let fact = descriptor.logical.facts[view.logical as usize];
            let mut heap = false;
            if matches!(fact.class, StorageClass::Block | StorageClass::Bytes) {
                let at = at.ok_or_else(|| err(V::Unsupported, "noncontiguous storage root"))?;
                let target = match view.node {
                    Node::Text { .. } => self.ptr_at(at)?,
                    Node::FlatText { .. } => match at {
                        At::Heap(p) => p,
                        _ => continue,
                    },
                    _ if self.record(view.node.layout())?.kind == LayoutKind::Flat => match at {
                        At::Heap(p) => p,
                        _ => continue,
                    },
                    _ => self.ptr_at(at)?,
                };
                let allocation = self.ledger.check(target)?;
                let expected = match view.node {
                    Node::Text { storage, .. } => Some(*storage),
                    _ => match self.record(view.node.layout())?.kind {
                        LayoutKind::Block { payload } | LayoutKind::Box { payload } => {
                            Some(payload)
                        }
                        LayoutKind::Flat => Some(view.node.layout()),
                        _ => None,
                    },
                };
                if expected.is_some() && allocation.layout != expected {
                    return fail(
                        V::TypeConfusion,
                        "sealed pointer has another payload layout",
                    );
                }
                match allocation.owner {
                    Owner::Region(r) if r == region => {
                        if !arena_seen.insert(target.alloc) {
                            continue;
                        }
                    }
                    Owner::Heap | Owner::Static => heap = true,
                    _ => {
                        return fail(
                            V::RegionEscape,
                            "semantic seal edge belongs to another arena or frame",
                        );
                    }
                }
            }
            if heap {
                let charge = match fact.class {
                    StorageClass::Bytes => view.bytes.map(|s| s.len),
                    StorageClass::Block => {
                        let shape = &descriptor.logical.shape.nodes[view.logical as usize];
                        let children: Vec<_> = match shape {
                            hirsel_lir::structural::Shape::Box(child) => vec![*child],
                            hirsel_lir::structural::Shape::Sequence(child)
                            | hirsel_lir::structural::Shape::Set(child) => vec![*child],
                            hirsel_lir::structural::Shape::Map { key, value } => vec![*key, *value],
                            _ => return fail(V::Malformed, "logical block shape"),
                        };
                        let each =
                            children.into_iter().try_fold(0u64, |n, child| {
                                match descriptor.logical.facts[child as usize].bytes {
                                    LogicalBytes::Bytes(bytes) => n.checked_add(bytes),
                                    LogicalBytes::Overflow => None,
                                }
                            });
                        each.and_then(|n| n.checked_mul(view.count.unwrap_or(1)))
                    }
                    _ => Some(0),
                };
                total = total.zip(charge).and_then(|(a, b)| a.checked_add(b));
            }
            pending.extend(view.children.into_iter().rev());
        }
        Ok(total)
    }

    fn seal_initialized(&self, image: &Extent, layout: LayoutId) -> Checked<bool> {
        // Capacity, record padding and inactive variant bytes are not payload.
        // The ordinary initialized() helper handles inline fields/variants;
        // sequences select only their actual initialized elements here.
        match self.record(layout)?.kind {
            LayoutKind::Sequence { element } => {
                image.check_range(0, 16)?;
                if !image.all_init(0, 16) {
                    return Ok(false);
                }
                let count = u64::from_le_bytes(image.read_bytes(0, 8).try_into().unwrap());
                let (start, stride) = self.sequence_shape(element)?;
                for i in 0..count {
                    if !self.initialized(image, start + i * stride, element)?.0 {
                        return Ok(false);
                    }
                }
                Ok(true)
            }
            _ => Ok(self.initialized(image, 0, layout)?.0),
        }
    }

    pub(super) fn prepare_seal_graph(
        &mut self,
        f: &Frame,
        region: usize,
        root: ValueId,
        shape: SealDescriptorId,
        origin: SourceAnchorId,
    ) -> Checked<Option<(SealedId, Ledger)>> {
        let descriptor = self.unit(f.unit).tables.seal_descriptors[shape.index()].clone();
        self.seal_source(f, origin, SourceEventKind::Seal, &descriptor)?;
        let entries = self
            .seal_inventory
            .get(&region)
            .cloned()
            .unwrap_or_default();
        // Source expectations survive deletion of both physical storage and its
        // candidate event. Direct module inputs describe the entered path.
        // Raw instruction fixtures enter here without start's static check.
        // Their explicit expectations describe that one entered component.
        // Full programs checked the entire source set before their first step.
        for (unit_index, unit) in self
            .program
            .units
            .iter()
            .enumerate()
            .filter(|_| !self.seal_sources_checked)
        {
            for source in self.config.seal_sources.iter().filter(|s| {
                s.reference.definition == unit.header.definition
                    && s.reference.instance == unit.header.instance
            }) {
                for event in source.events.iter().filter(|e| {
                    matches!(e.kind, SourceEventKind::Allocate | SourceEventKind::Replace)
                }) {
                    if !entries
                        .values()
                        .any(|e| e.events.contains(&(unit_index, event.anchor.clone())))
                    {
                        return fail(
                            V::Malformed,
                            "independent retained allocation event is missing",
                        );
                    }
                }
            }
        }
        let live: BTreeSet<_> = self
            .ledger
            .allocations
            .indexed()
            .filter(|(_, a)| a.owner == Owner::Region(region) && a.state == AllocState::Live)
            .map(|(i, _)| AllocId(i as u32))
            .collect();
        let listed: BTreeSet<_> = self.ledger.regions[region]
            .allocations
            .iter()
            .copied()
            .collect();
        if live.iter().any(|id| !listed.contains(id))
            || live.iter().any(|id| !entries.contains_key(id))
        {
            return fail(
                V::RegionEscape,
                "physical retained storage has no semantic allocation event",
            );
        }
        let value = self.read_use(f, root)?;
        // Collect the physical DAG independently from every retained allocation.
        let mut root_image = self.projection_image(&descriptor.projection, &value)?;
        if let Val::Ptr(_) = value {
            if let Some(Cell::Data { counted, .. }) = root_image.cells.get_mut(&0) {
                *counted = self.get(f, root)?.credits > 0;
            }
        }
        if matches!(self.record(descriptor.root_layout)?.size, SizeSpec::Fixed(size) if size != root_image.len())
        {
            return fail(V::TypeConfusion, "sealed root has another inline extent");
        }
        if let Val::Bits { scalar, .. } = value {
            if self.record(descriptor.root_layout)?.kind != LayoutKind::Scalar(scalar) {
                return fail(
                    V::TypeConfusion,
                    "sealed root bits have another scalar kind",
                );
            }
        }
        for cell in root_image.cells.values() {
            if let Cell::Data {
                ptr,
                counted,
                borrow,
            } = cell
            {
                if borrow.is_some() || (self.counted_target(*ptr) && !*counted) {
                    return fail(
                        V::BorrowedStore,
                        "sealed root contains a non-owning heap field",
                    );
                }
            }
        }
        if !root_image.resources.is_empty()
            || !self.initialized(&root_image, 0, descriptor.root_layout)?.0
        {
            return fail(
                V::TypeConfusion,
                "sealed root image is uninitialized or contains a resource",
            );
        }
        let mut images = BTreeMap::<AllocId, (Ptr, LayoutId, Extent)>::new();
        let mut pending: Vec<_> = live
            .iter()
            .map(|id| Ptr {
                alloc: *id,
                generation: self.ledger.get(*id).generation,
                offset: 0,
                path: 0,
            })
            .collect();
        pending.extend(root_image.cells.values().filter_map(|c| {
            if let Cell::Data { ptr, .. } = c {
                Some(*ptr)
            } else {
                None
            }
        }));
        let mut allowed: BTreeSet<LayoutId> = descriptor.access.iter().map(|a| a.layout).collect();
        for e in entries.values() {
            allowed.extend(e.descriptor.access.iter().map(|a| a.layout));
        }
        while let Some(ptr) = pending.pop() {
            let a = self.ledger.check(ptr)?;
            if matches!(a.count, Some(Count::Root(_))) {
                continue;
            }
            if images.contains_key(&ptr.alloc) {
                continue;
            }
            if !matches!(a.owner, Owner::Heap | Owner::Static) && a.owner != Owner::Region(region) {
                return fail(
                    V::RegionEscape,
                    "physical seal edge escapes the source domain",
                );
            }
            let layout = a
                .layout
                .ok_or_else(|| err(V::TypeConfusion, "sealed allocation has no layout"))?;
            if !allowed.contains(&layout)
                || !a.extent.resources.is_empty()
                || self.record(layout)?.finalizer.is_some()
            {
                return fail(
                    V::TypeConfusion,
                    "sealed physical payload lacks descriptor coverage",
                );
            }
            let size = self.extent_of(layout, Ptr { offset: 0, ..ptr })?;
            a.extent.check_range(0, size)?;
            let image = a.extent.copy(0, size, None);
            if !self.seal_initialized(&image, layout)? {
                return fail(
                    V::UninitializedRead,
                    "sealed physical payload is not initialized",
                );
            }
            let mut offsets = self.scan_cell_offsets(&image, layout, true)?;
            offsets.extend(self.scan_cell_offsets(&image, layout, false)?);
            let actual: BTreeSet<_> = a
                .extent
                .cells
                .iter()
                .filter_map(|(o, c)| matches!(c, Cell::Data { .. }).then_some(*o))
                .collect();
            if offsets.into_iter().collect::<BTreeSet<_>>() != actual {
                return fail(
                    V::TypeConfusion,
                    "sealed physical child cells differ from complete scan",
                );
            }
            for cell in a.extent.cells.values() {
                match cell {
                    Cell::Data {
                        ptr,
                        counted,
                        borrow: None,
                    } => {
                        if self.counted_target(*ptr) && !*counted {
                            return fail(
                                V::CreditUnderflow,
                                "sealed payload has an uncredited outgoing cell",
                            );
                        }
                        pending.push(*ptr);
                    }
                    _ => {
                        return fail(
                            V::BorrowedStore,
                            "sealed payload retains a borrow or capability",
                        );
                    }
                }
            }
            images.insert(ptr.alloc, (Ptr { offset: 0, ..ptr }, layout, image));
        }
        // Reject cycles before the occurrence walk; no helper depth or step
        // budget is allowed to turn a corrupt physical cycle into a hang.
        let mut done = BTreeSet::new();
        for id in images.keys() {
            let mut path = BTreeSet::new();
            let mut work = vec![(*id, false)];
            while let Some((id, exit)) = work.pop() {
                if exit {
                    path.remove(&id);
                    done.insert(id);
                    continue;
                }
                if done.contains(&id) {
                    continue;
                }
                if !path.insert(id) {
                    return fail(V::TypeConfusion, "sealed physical graph is cyclic");
                }
                work.push((id, true));
                for cell in images[&id].2.cells.values() {
                    if let Cell::Data { ptr, .. } = cell {
                        if images.contains_key(&ptr.alloc) {
                            work.push((ptr.alloc, false));
                        }
                    }
                }
            }
        }
        let mut arena_seen = BTreeSet::new();
        let mut shared = BTreeMap::new();
        let mut charge = Some(0u64);
        for entry in entries.values() {
            charge = charge
                .zip(self.receipt_occurrences(
                    &entry.snapshot,
                    &entry.descriptor,
                    region,
                    &mut arena_seen,
                    &mut shared,
                )?)
                .and_then(|(a, b)| a.checked_add(b));
        }
        charge = charge
            .zip(self.seal_occurrences(
                &descriptor,
                &value,
                region,
                &mut arena_seen,
                &mut shared,
            )?)
            .and_then(|(a, b)| a.checked_add(b));
        // No outside cell or SSA loan may observe adopted arena storage.
        for (i, a) in self.ledger.allocations.indexed() {
            if a.state == AllocState::Live
                && !live.contains(&AllocId(i as u32))
                && a.extent
                    .cells
                    .values()
                    .any(|c| matches!(c, Cell::Data { ptr, .. } if live.contains(&ptr.alloc)))
            {
                return fail(
                    V::RegionEscape,
                    "outside physical cell reaches sealing storage",
                );
            }
        }
        if f.values.iter().enumerate().any(|(i, h)| {
            i != root.index()
                && h.as_ref().is_some_and(|h| {
                    (h.credits > 0 || h.borrow.is_some()) && self.seal_value_reaches(&h.val, &live)
                })
        }) {
            return fail(
                V::BorrowExpired,
                "outside SSA value reaches sealing storage",
            );
        }
        for activity in &self.stack {
            if let Activity::Frame(frame) = activity {
                if frame.values.iter().flatten().any(|h| {
                    (h.credits > 0 || h.borrow.is_some()) && self.seal_value_reaches(&h.val, &live)
                }) || frame
                    .loans
                    .iter()
                    .flatten()
                    .any(|p| live.contains(&p.alloc))
                {
                    return fail(
                        V::BorrowExpired,
                        "caller retains a live view of sealing storage",
                    );
                }
            }
        }
        if f.loans.iter().flatten().any(|p| live.contains(&p.alloc)) {
            return fail(V::BorrowExpired, "seal retains a live storage loan");
        }
        self.stack.push(Activity::Frame(Box::new(f.clone())));
        let counts = self.recount_counts();
        self.stack.pop();
        counts?;
        if charge.is_none()
            || self.ledger.regions[region]
                .budget
                .is_some_and(|b| charge.unwrap() > b.remaining)
        {
            return Ok(None);
        }
        // Stage exclusively in a ledger snapshot. Even malformed release or
        // closed-cell audit refusals leave the entered root and grant intact.
        let root_releases = match &value {
            Val::Ptr(p) if self.get(f, root)?.credits > 0 && self.counted_target(*p) => vec![*p],
            Val::Image(e) => e
                .cells
                .values()
                .filter_map(|c| match c {
                    Cell::Data {
                        ptr, counted: true, ..
                    } => Some(*ptr),
                    _ => None,
                })
                .collect(),
            _ => vec![],
        };
        let original = self.ledger.clone();
        let staged = self.commit_seal_graph(
            region,
            (root_image.clone(), descriptor.root_layout),
            &images,
            &shared,
            charge.unwrap(),
            root_releases,
        );
        let staged_ledger = std::mem::replace(&mut self.ledger, original);
        let id = staged?;
        Ok(Some((id, staged_ledger)))
    }

    fn seal_value_reaches(&self, value: &Val, live: &BTreeSet<AllocId>) -> bool {
        match value {
            Val::Ptr(p) | Val::Callable { env: Some(p), .. } => live.contains(&p.alloc),
            Val::Image(e) => e
                .cells
                .values()
                .any(|c| matches!(c, Cell::Data { ptr, .. } if live.contains(&ptr.alloc))),
            _ => false,
        }
    }

    fn commit_seal_graph(
        &mut self,
        region: usize,
        root_image: (Extent, LayoutId),
        images: &BTreeMap<AllocId, (Ptr, LayoutId, Extent)>,
        semantic_shared: &BTreeMap<Ptr, u64>,
        charge: u64,
        root_releases: Vec<Ptr>,
    ) -> Checked<SealedId> {
        let (mut root, root_layout) = root_image;
        let mut relocated = BTreeMap::new();
        for (id, (ptr, layout, image)) in images {
            let destination = if self.ledger.check(*ptr)?.owner == Owner::Region(region) {
                *ptr
            } else {
                self.region_alloc(region, *layout, image.len())?
            };
            relocated.insert(*id, destination);
        }
        let mut release = root_releases;
        let mut physical_shared = BTreeMap::<Ptr, u64>::new();
        let mut patch = |image: &mut Extent, retained: bool| -> Checked<()> {
            for cell in image.cells.values_mut() {
                if let Cell::Data {
                    ptr,
                    counted,
                    borrow,
                } = cell
                {
                    if let Some(new) = relocated.get(&ptr.alloc) {
                        if retained && *counted {
                            release.push(*ptr);
                        }
                        *ptr = Ptr {
                            offset: ptr.offset,
                            path: ptr.path,
                            ..*new
                        };
                        *counted = false;
                        *borrow = None;
                    } else {
                        if !matches!(self.ledger.check(*ptr)?.count, Some(Count::Root(_)))
                            || !*counted
                        {
                            return fail(
                                V::CountMismatch,
                                "sealed Shared edge has no owning credit",
                            );
                        }
                        *physical_shared.entry(*ptr).or_default() += 1;
                        *counted = true;
                        *borrow = None;
                    }
                }
            }
            Ok(())
        };
        let mut patched = Vec::new();
        for (id, (ptr, layout, image)) in images {
            let mut image = image.clone();
            let retained = self.ledger.check(*ptr)?.owner == Owner::Region(region);
            patch(&mut image, retained)?;
            patched.push((relocated[id], *layout, image, retained));
        }
        patch(&mut root, false)?;
        // Root's source holder is released along with arena-held outgoing
        // credits, after destination Shared acquisitions have been installed.
        // Its exact owning cells are selected by the typed input image.
        let root_storage = self.region_alloc(region, root_layout, root.len())?;
        for (ptr, _, image, retained) in patched {
            if !retained {
                for cell in image.cells.values() {
                    if let Cell::Data {
                        ptr, counted: true, ..
                    } = cell
                    {
                        self.dup(*ptr)?;
                    }
                }
            }
            self.ledger.get_mut(ptr.alloc).extent.place(0, image);
        }
        for cell in root.cells.values() {
            if let Cell::Data {
                ptr, counted: true, ..
            } = cell
            {
                self.dup(*ptr)?;
            }
        }
        self.ledger
            .get_mut(root_storage.alloc)
            .extent
            .place(0, root.clone());
        for (ptr, count) in semantic_shared {
            let physical = physical_shared.get(ptr).copied().unwrap_or(0);
            if physical > *count {
                return fail(
                    V::CountMismatch,
                    "physical Shared multiplicity exceeds semantic inventory",
                );
            }
            for _ in physical..*count {
                let payload = self
                    .ledger
                    .check(*ptr)?
                    .layout
                    .ok_or_else(|| err(V::TypeConfusion, "Shared payload layout"))?;
                let layout = self
                    .program
                    .layouts
                    .layouts
                    .iter()
                    .position(
                        |row| matches!(row.kind, LayoutKind::Shared { payload: p } if p == payload),
                    )
                    .map(|i| LayoutId(i as u32))
                    .ok_or_else(|| err(V::TypeConfusion, "Shared credit has no root layout"))?;
                // Extra semantic roots have real checked cells, independent of
                // deduplicated ordinary payload cells and release recipes.
                let backing = self.region_alloc(region, layout, self.word)?;
                self.dup(*ptr)?;
                self.ledger.get_mut(backing.alloc).extent.put_cell(
                    0,
                    self.word,
                    Cell::Data {
                        ptr: *ptr,
                        counted: true,
                        borrow: None,
                    },
                );
            }
        }
        let mut drain = Drain {
            caller: None,
            operation: None,
            work: vec![],
            failed: false,
        };
        for ptr in release {
            self.decrement(ptr, None, &mut drain.work)?;
        }
        while let Some(work) = drain.work.pop() {
            self.step_drain(&mut drain, work)?;
        }
        if !self.charge(region, charge) {
            return fail(V::InternalAudit, "preflight charge changed during seal");
        }
        for cell in root.cells.values_mut() {
            if let Cell::Data { counted, .. } = cell {
                *counted = false;
            }
        }
        self.ledger
            .transfer_closed_scope_with_graph_root(region, self.word, root_layout, root)
    }
}

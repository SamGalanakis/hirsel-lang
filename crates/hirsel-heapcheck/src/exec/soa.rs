//! Logical rows over independently checked physical columns.
use super::*;
use hirsel_lir::layout_types::FieldLayout;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct ColumnRow {
    pub(super) base: Ptr,
    pub(super) capacity: u64,
    pub(super) index: u64,
}

impl<S: Stage> Machine<'_, S> {
    /// SoA columns use the stored capacity, including over-reserved sources.
    pub(super) fn column_shape(&self, element: LayoutId) -> Checked<(u64, u64, Vec<(u64, u64)>)> {
        let r = self.record(element)?;
        let quantum = r
            .fields
            .iter()
            .map(|f| self.record(f.layout).map(|r| u64::from(r.align)))
            .collect::<Checked<Vec<_>>>()?
            .into_iter()
            .max()
            .unwrap_or(1);
        let data = hirsel_lir::layout::soa_data_offset(quantum)
            .map_err(|e| err(V::Malformed, format!("{e:?}")))?;
        let mut prefix = 0;
        let mut columns = Vec::new();
        for f in &r.fields {
            let stride = self.size(f.layout)?;
            columns.push((prefix, stride));
            prefix += stride;
        }
        Ok((data, quantum, columns))
    }

    pub(super) fn columns_capacity(&self, base: Ptr, element: LayoutId) -> Checked<u64> {
        let (data, quantum, columns) = self.column_shape(element)?;
        let width: u64 = columns.iter().map(|&(_, stride)| stride).sum();
        let capacity = self.read_scalar(base.at(8), 8)? as u64;
        if !capacity.is_multiple_of(quantum) {
            return fail(V::Malformed, "column capacity is not aligned");
        }
        let bytes = capacity
            .checked_mul(width)
            .and_then(|b| data.checked_add(b))
            .ok_or_else(|| err(V::OutOfExtent, "column capacity overflow"))?;
        self.ledger
            .check(base)?
            .extent
            .check_range(base.offset, bytes)?;
        Ok(capacity)
    }

    /// Zeroed backing bytes do not construct spare rows. Keep their fields
    /// unwritten, as a fresh reserve does, while retaining the live row bytes.
    pub(super) fn clear_column_spare(&mut self, base: Ptr, element: LayoutId) -> Checked<()> {
        let count = self.read_scalar(base, 8)? as u64;
        let capacity = self.columns_capacity(base, element)?;
        if count > capacity {
            return fail(V::OutOfExtent, "column count exceeds capacity");
        }
        let (data, _, columns) = self.column_shape(element)?;
        for (prefix, stride) in columns {
            let at = base.offset + data + capacity * prefix + count * stride;
            let bytes = (capacity - count) * stride;
            self.ledger
                .get_mut(base.alloc)
                .extent
                .take(at, bytes, UNINIT_POISON, Some((&[], &[])));
        }
        Ok(())
    }

    pub(super) fn column_fields(&mut self, r: &Resolved) -> Checked<Vec<(FieldLayout, Ptr)>> {
        let row = r
            .columns
            .ok_or_else(|| err(V::Malformed, "missing column row"))?;
        let (data, _, columns) = self.column_shape(r.layout)?;
        let fields = self.record(r.layout)?.fields.clone();
        fields
            .into_iter()
            .zip(columns)
            .map(|(field, (prefix, stride))| {
                let offset = data + row.capacity * prefix + row.index * stride;
                let row_at = self
                    .ledger
                    .project(row.base, 0, ProjectionStep::Index(row.index));
                let at = self
                    .ledger
                    .project(row_at, offset, ProjectionStep::Field(field.ordinal));
                self.ledger
                    .check(at)?
                    .extent
                    .check_range(at.offset, stride)?;
                Ok((field, at))
            })
            .collect()
    }

    pub(super) fn column_resolved(
        &mut self,
        base: Ptr,
        element: LayoutId,
        index: u64,
    ) -> Checked<Resolved> {
        let capacity = self.columns_capacity(base, element)?;
        if index >= capacity {
            return fail(V::OutOfExtent, "column slot exceeds capacity");
        }
        Ok(Resolved {
            ptr: base,
            layout: element,
            columns: Some(ColumnRow {
                base,
                capacity,
                index,
            }),
            path: vec![base.alloc],
            root_slot: None,
            flat_content: None,
            flat_end: None,
        })
    }

    /// Gather without importing any producer's ownership or initialization facts.
    pub(super) fn read_resolved(
        &mut self,
        r: &Resolved,
        layout: LayoutId,
        take: bool,
    ) -> Checked<Extent> {
        if r.columns.is_some() {
            let mut image = Extent::new(self.size(r.layout)?);
            for (field, at) in self.column_fields(r)? {
                let size = self.size(field.layout)?;
                let owned = self.resources_of(field.layout)?;
                let part = if take {
                    self.take_part(at, size, &owned)
                } else {
                    self.ledger.check(at)?.extent.copy(
                        at.offset,
                        size,
                        Some((&self.ledger.path(at), &owned)),
                    )
                };
                image.place_at(field.offset, &[ProjectionStep::Field(field.ordinal)], part);
            }
            Ok(image)
        } else {
            let size = self.bounded_size(r.ptr, layout)?;
            self.ledger
                .check(r.ptr)?
                .extent
                .check_range(r.ptr.offset, size)?;
            let owned = self.resources_of(layout)?;
            Ok(if take {
                self.take_part(r.ptr, size, &owned)
            } else {
                self.ledger.check(r.ptr)?.extent.copy(
                    r.ptr.offset,
                    size,
                    Some((&self.ledger.path(r.ptr), &owned)),
                )
            })
        }
    }

    pub(super) fn write_resolved(
        &mut self,
        r: &Resolved,
        layout: LayoutId,
        m: Moved,
    ) -> Checked<()> {
        if r.columns.is_none() {
            return self.write_value(r.ptr, layout, m);
        }
        let Val::Image(mut image) = m.val else {
            return fail(V::TypeConfusion, "column row is not an aggregate image");
        };
        for (field, at) in self.column_fields(r)? {
            let size = self.size(field.layout)?;
            let owned = self.resources_of(field.layout)?;
            let prefix = [ProjectionStep::Field(field.ordinal)];
            let part = image.take(field.offset, size, UNINIT_POISON, Some((&prefix, &owned)));
            self.write_value(
                at,
                field.layout,
                Moved {
                    val: Val::Image(part),
                    credited: m.credited,
                    borrow: m.borrow,
                },
            )?;
        }
        Ok(())
    }

    pub(super) fn holds_columns(&self, f: &Frame, address: AddressId) -> Checked<Option<LayoutId>> {
        let layout = self.address_layout(f.unit, address)?;
        Ok(match self.record(layout)?.kind {
            LayoutKind::Block { payload } => match self.record(payload)?.kind {
                LayoutKind::SoaColumns { element } => Some(element),
                _ => None,
            },
            _ => None,
        })
    }

    pub(super) fn reserve_columns(
        &mut self,
        f: &mut Frame,
        address: AddressId,
        capacity: ValueId,
        version: ValueId,
        element: LayoutId,
    ) -> Checked<()> {
        let r = self.resolve_use(f, address)?;
        self.check_writable(&r)?;
        let block = self.read_ptr(r.ptr)?;
        let old_capacity = self.columns_capacity(block, element)?;
        let Val::Bits { bits, .. } = self.read_use(f, capacity)? else {
            return fail(V::TypeConfusion, "column capacity is not an integer");
        };
        let count = self.read_scalar(block, 8)? as u64;
        // MEM-6: an unshared tracked arena block is written in place too, and a
        // tracked one regrows in its arena, uncharged.
        let region = self.tracked_region(block)?;
        let old = self.ledger.check(block)?;
        let private =
            (old.count == Some(Count::Finite(1)) && old.owner == Owner::Heap && !old.published)
                || (region.is_some() && old.count == Some(Count::Arena { shared: false }));
        if !private || bits > u128::from(old_capacity) {
            let (data, quantum, columns) = self.column_shape(element)?;
            let width: u64 = columns.iter().map(|&(_, stride)| stride).sum();
            let wanted = u64::try_from(bits)
                .map_err(|_| err(V::OutOfExtent, "column capacity exceeds u64"))?
                .max(count);
            let cap = hirsel_lir::layout::align_up(wanted, quantum)
                .map_err(|_| err(V::OutOfExtent, "column capacity overflow"))?;
            let bytes = cap
                .checked_mul(width)
                .and_then(|b| data.checked_add(b))
                .ok_or_else(|| err(V::OutOfExtent, "column allocation overflow"))?;
            let (payload, align) = (
                old.layout
                    .ok_or_else(|| err(V::Malformed, "column block without layout"))?,
                old.align,
            );
            let grown = match region {
                Some(region) => {
                    let grown = self.arena_alloc(region, payload, bytes)?;
                    self.ledger.get_mut(grown.alloc).count = Some(Count::Arena { shared: false });
                    grown
                }
                None => self.heap_alloc(payload, bytes, align)?,
            };
            let extent = &mut self.ledger.get_mut(grown.alloc).extent;
            extent.write_bytes(0, &count.to_le_bytes());
            extent.write_bytes(8, &cap.to_le_bytes());
            for index in 0..count {
                let from = self.column_resolved(block, element, index)?;
                let image = self.read_resolved(&from, element, private)?;
                let val = if private {
                    Val::Image(image)
                } else {
                    self.copy_value(Val::Image(image), element)?.0
                };
                let to = self.column_resolved(grown, element, index)?;
                self.write_resolved(
                    &to,
                    element,
                    Moved {
                        val,
                        credited: true,
                        borrow: None,
                    },
                )?;
            }
            let Cell::Data { counted, .. } = self.read_cell(r.ptr)? else {
                return fail(V::TypeConfusion, "column root is not a data cell");
            };
            self.ledger.get_mut(r.ptr.alloc).extent.put_cell(
                r.ptr.offset,
                self.word,
                Cell::Data {
                    ptr: grown,
                    counted: true,
                    borrow: None,
                },
            );
            if private && region.is_some() {
                // The abandoned storage stays in its arena, emptied: its exit
                // drops no row there.
                self.ledger
                    .get_mut(block.alloc)
                    .extent
                    .write_bytes(block.offset, &0u64.to_le_bytes());
            } else if private {
                self.ledger.get_mut(block.alloc).count = None;
                self.ledger.free(block.alloc)?;
            } else if counted {
                if self.count_down(block)? {
                    return fail(V::Malformed, "shared column block died");
                }
            } else if self.counted_target(block) {
                return fail(
                    V::CreditUnderflow,
                    "column reserve replaces an uncredited holder",
                );
            }
        }
        if let Some(s) = r.root_slot {
            f.versions[s.index()] = Some(version);
        }
        self.define_version(f, version)
    }

    pub(super) fn set_columns_len(
        &mut self,
        f: &mut Frame,
        address: AddressId,
        len: ValueId,
        version: ValueId,
        element: LayoutId,
    ) -> Checked<()> {
        let r = self.resolve_use(f, address)?;
        self.check_writable(&r)?;
        let block = self.read_ptr(r.ptr)?;
        self.check_private(block)?;
        let capacity = self.columns_capacity(block, element)?;
        let Val::Bits { bits, .. } = self.read_use(f, len)? else {
            return fail(V::TypeConfusion, "column length is not an integer");
        };
        if bits > u128::from(capacity) {
            return fail(V::OutOfExtent, "column length exceeds capacity");
        }
        let len = bits as u64;
        for index in 0..capacity {
            let row = self.column_resolved(block, element, index)?;
            let image = self.read_resolved(&row, element, false)?;
            let (initialized, live) = self.initialized(&image, 0, element)?;
            if index < len && !initialized {
                return fail(V::UninitializedRead, "column length covers unwritten row");
            }
            if index >= len && live {
                return fail(V::LeakedSlot, "column length abandons written row");
            }
        }
        self.ledger
            .get_mut(block.alloc)
            .extent
            .write_bytes(block.offset, &len.to_le_bytes());
        if let Some(s) = r.root_slot {
            f.versions[s.index()] = Some(version);
        }
        self.define_version(f, version)
    }
}

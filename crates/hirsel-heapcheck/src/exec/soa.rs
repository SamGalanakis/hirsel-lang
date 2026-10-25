//! Logical rows over independently checked physical columns.
use super::*;
use hirsel_lir::layout::{SoaPart, SoaShape};
use hirsel_lir::layout_types::ScanStep;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct ColumnRow {
    pub(super) base: Ptr,
    pub(super) capacity: u64,
    pub(super) index: u64,
}

/// One column's share of a row: the row image's `size` bytes at `row_offset`, stored at
/// `at` and named by `step`; `field` is a record field's layout.
pub(super) struct ColumnPart {
    pub(super) row_offset: u64,
    pub(super) size: u64,
    pub(super) at: Ptr,
    pub(super) step: ProjectionStep<u64>,
    pub(super) part: SoaPart,
}

impl<S: Stage> Machine<'_, S> {
    /// The columns of an `@soa` sequence of `element`, derived from its row layout alone:
    /// never from the payload record's own column claims.
    pub(super) fn column_shape(&self, element: LayoutId) -> Checked<SoaShape> {
        self.program
            .layouts
            .soa_shape(element)
            .map_err(|e| err(V::Malformed, format!("{e:?}")))
    }

    /// A record element's field columns as (data offset, quantum, (prefix, stride) per
    /// field), for walks that visit fields; a variant's columns have no fields.
    pub(super) fn field_columns(&self, element: LayoutId) -> Checked<(u64, u64, Vec<(u64, u64)>)> {
        let shape = self.column_shape(element)?;
        if shape.tag().is_some() {
            return fail(V::Unsupported, "a field walk of a variant's columns");
        }
        let columns = shape.spans.iter().map(|s| (s.prefix, s.stride)).collect();
        Ok((shape.data_offset, shape.quantum, columns))
    }

    /// SoA columns use the stored capacity, including over-reserved sources.
    pub(super) fn columns_capacity(&self, base: Ptr, element: LayoutId) -> Checked<u64> {
        let shape = self.column_shape(element)?;
        let capacity = self.read_scalar(base.at(8), 8)? as u64;
        if !capacity.is_multiple_of(shape.quantum) {
            return fail(V::Malformed, "column capacity is not aligned");
        }
        let bytes = capacity
            .checked_mul(shape.width)
            .and_then(|b| shape.data_offset.checked_add(b))
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
        let shape = self.column_shape(element)?;
        for span in shape.spans {
            let at = base.offset + shape.data_offset + capacity * span.prefix + count * span.stride;
            let bytes = (capacity - count) * span.stride;
            self.ledger
                .get_mut(base.alloc)
                .extent
                .take(at, bytes, UNINIT_POISON, Some((&[], &[])));
        }
        Ok(())
    }

    /// The parts of the row `r` names, one per column.
    pub(super) fn column_parts(&mut self, r: &Resolved) -> Checked<Vec<ColumnPart>> {
        let row = r
            .columns
            .ok_or_else(|| err(V::Malformed, "missing column row"))?;
        let shape = self.column_shape(r.layout)?;
        let row_at = self
            .ledger
            .project(row.base, 0, ProjectionStep::Index(row.index));
        let mut parts = Vec::new();
        for (position, span) in shape.spans.iter().enumerate() {
            let offset = shape.data_offset + row.capacity * span.prefix + row.index * span.stride;
            let step = match span.part {
                SoaPart::Field { ordinal, .. } => ProjectionStep::Field(ordinal),
                _ => ProjectionStep::Field(position as u32),
            };
            let at = self.ledger.project(row_at, offset, step);
            self.ledger
                .check(at)?
                .extent
                .check_range(at.offset, span.stride)?;
            parts.push(ColumnPart {
                row_offset: span.row_offset,
                size: span.stride,
                at,
                step,
                part: span.part,
            });
        }
        Ok(parts)
    }

    /// The active ordinal of the variant row `r` names, read from its tag column.
    pub(super) fn column_variant(&mut self, r: &Resolved) -> Checked<u32> {
        let tag = self
            .column_parts(r)?
            .into_iter()
            .find(|p| matches!(p.part, SoaPart::Tag(_)))
            .ok_or_else(|| err(V::TypeConfusion, "a variant read of a record row"))?;
        Ok(self.read_scalar(tag.at, tag.size)? as u32)
    }

    /// The place of variant field `field` of arm `variant` in the payload column of the row
    /// `r` names. Reading a payload through an arm the tag column does not name is a
    /// failure, or a checker violation when the projection claims a proof.
    pub(super) fn column_variant_field(
        &mut self,
        r: &Resolved,
        variant: u32,
        field: u32,
        unchecked: bool,
    ) -> Checked<Result<(Ptr, LayoutId), LangFailure>> {
        let active = self.column_variant(r)?;
        if active != variant {
            if unchecked {
                return fail(V::TypeConfusion, "invalid proved variant projection");
            }
            return Ok(Err(LangFailure));
        }
        let tag = self
            .record(r.layout)?
            .tag
            .as_ref()
            .ok_or_else(|| err(V::Malformed, "a variant row without a tag"))?;
        let fl = tag
            .variants
            .iter()
            .find(|v| v.ordinal == variant)
            .and_then(|v| v.fields.iter().find(|fl| fl.ordinal == field))
            .ok_or_else(|| err(V::Malformed, format!("no variant field {field}")))?
            .clone();
        let payload = self
            .column_parts(r)?
            .into_iter()
            .find(|p| p.part == SoaPart::Payload)
            .ok_or_else(|| err(V::Malformed, "a variant row without a payload column"))?;
        let within = fl
            .offset
            .checked_sub(payload.row_offset)
            .ok_or_else(|| err(V::Malformed, "a variant field before its payload"))?;
        Ok(Ok((
            self.ledger.project(
                payload.at,
                within,
                ProjectionStep::VariantField { variant, field },
            ),
            fl.layout,
        )))
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

    /// Gather without importing any producer's ownership or initialization facts. A
    /// variant row's tag widens to its u32 tag word; its payload bytes come whole.
    pub(super) fn read_resolved(
        &mut self,
        r: &Resolved,
        layout: LayoutId,
        take: bool,
    ) -> Checked<Extent> {
        if r.columns.is_some() {
            let mut image = Extent::new(self.size(r.layout)?);
            for part in self.column_parts(r)? {
                let owned = match part.part {
                    SoaPart::Field { layout, .. } => self.resources_of(layout)?,
                    _ => Vec::new(),
                };
                let piece = if take {
                    self.take_part(part.at, part.size, &owned)
                } else {
                    self.ledger.check(part.at)?.extent.copy(
                        part.at.offset,
                        part.size,
                        Some((&self.ledger.path(part.at), &owned)),
                    )
                };
                if let SoaPart::Tag(_) = part.part {
                    image.write_bytes(part.row_offset, &[0; 4]);
                }
                image.place_at(part.row_offset, &[part.step], piece);
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
        // A variant row's payload is written through the arm its own tag names.
        let active = match self.record(r.layout)?.tag {
            Some(_) => Some(image.active(0, TagForm::Ordinal, self.word)?),
            None => None,
        };
        for part in self.column_parts(r)? {
            let prefix = [part.step];
            match part.part {
                SoaPart::Field { layout, .. } => {
                    let owned = self.resources_of(layout)?;
                    let piece = image.take(
                        part.row_offset,
                        part.size,
                        UNINIT_POISON,
                        Some((&prefix, &owned)),
                    );
                    self.write_value(
                        part.at,
                        layout,
                        Moved {
                            val: Val::Image(piece),
                            credited: m.credited,
                            borrow: m.borrow,
                        },
                    )?;
                }
                SoaPart::Tag(_) => {
                    let ordinal = u64::from(active.unwrap_or(0));
                    if part.size < 8 && ordinal >> (8 * part.size) != 0 {
                        return fail(V::Malformed, "a variant ordinal wider than its tag column");
                    }
                    let piece = image.take(0, 4, UNINIT_POISON, Some((&prefix, &[])));
                    self.write_raw(part.at, piece.copy(0, part.size, None))?;
                }
                SoaPart::Payload => {
                    let mut piece = image.take(
                        part.row_offset,
                        part.size,
                        UNINIT_POISON,
                        Some((&prefix, &[])),
                    );
                    match self.arm_view(r, active.unwrap_or(0))? {
                        Some(arm) => {
                            let held = self.size(arm)?;
                            let rest = piece.take(held, part.size - held, UNINIT_POISON, None);
                            let head = piece.take(0, held, UNINIT_POISON, None);
                            self.write_value(
                                part.at,
                                arm,
                                Moved {
                                    val: Val::Image(head),
                                    credited: m.credited,
                                    borrow: m.borrow,
                                },
                            )?;
                            self.write_raw(part.at.at(held), rest)?;
                        }
                        None => self.write_raw(part.at, piece)?,
                    }
                }
            }
        }
        Ok(())
    }

    /// The arm view a variant row's payload column has for `variant`, when that arm holds
    /// anything: the column of the payload block's own scan with that arm.
    fn arm_view(&self, r: &Resolved, variant: u32) -> Checked<Option<LayoutId>> {
        let row = r
            .columns
            .ok_or_else(|| err(V::Malformed, "missing column row"))?;
        let Some(payload) = self.ledger.check(row.base)?.layout else {
            return fail(V::Malformed, "column block without layout");
        };
        let record = self.record(payload)?;
        let views = record.scan.iter().filter_map(|step| match step {
            ScanStep::Columns(c) => Some(&c.columns),
            _ => None,
        });
        Ok(views
            .flatten()
            .find(|c| c.arm == Some(variant))
            .map(|c| c.layout))
    }

    /// Store bytes that hold no reference into unwritten storage.
    fn write_raw(&mut self, at: Ptr, piece: Extent) -> Checked<()> {
        if !piece.cells.is_empty() || !piece.resources.is_empty() {
            return fail(
                V::TypeConfusion,
                "a reference stored through an arm with no holders",
            );
        }
        self.ledger
            .check(at)?
            .extent
            .check_range(at.offset, piece.len())?;
        if self.has_content(at, piece.len())? {
            return fail(
                V::StoreOverInitialized,
                format!(
                    "store over initialized bytes at {:?}+{}",
                    at.alloc, at.offset
                ),
            );
        }
        self.ledger
            .get_mut(at.alloc)
            .extent
            .place_at(at.offset, &[], piece);
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
            let shape = self.column_shape(element)?;
            let (data, quantum, width) = (shape.data_offset, shape.quantum, shape.width);
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

#[cfg(test)]
mod tests;

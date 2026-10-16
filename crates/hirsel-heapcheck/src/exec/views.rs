//! Checked marshalling of borrowed Unicode views. Models see bytes and relative
//! offsets; only this executor touches owner cells, loans and holder credits.
use super::*;
use hirsel_core_contract::{OperationId, Passing};

pub(super) fn is_view_operation(id: &OperationId) -> bool {
    let p = &id.definition.0;
    p.package.name == "core"
        && p.package.version == "1"
        && p.modules == ["$intrinsic"]
        && matches!(
            p.name.as_str(),
            "UnicodeScalars_open"
                | "UnicodeScalars_next"
                | "ScalarPositions_open"
                | "ScalarPositions_next"
        )
}

impl<S: Stage> Machine<'_, S> {
    pub(super) fn view_operation(
        &mut self,
        f: &mut Frame,
        site: &CallSite,
        op: OperationRef,
        normal: EdgeId,
        failure: FailureId,
    ) -> Checked<Control> {
        let u = self.unit(f.unit);
        let id = u.tables.operations[op.index()].clone();
        let args = self.pool(site.arguments, &u.pools.call_arguments)?.to_vec();
        let [argument] = args.as_slice() else {
            return fail(V::Malformed, "Unicode cursor arity");
        };
        let result = site
            .result
            .ok_or_else(|| err(V::Malformed, "Unicode cursor without output"))?;
        let result_layout = self.value_type(f.unit, result)?.layout;
        let open = id.definition.0.name.ends_with("_open");
        let (mut image, cursor_layout, address, source_borrow) = if open {
            let (Passing::Borrow, ArgumentValue::Value(v)) = (argument.passing, argument.value)
            else {
                return fail(V::Malformed, "Unicode open must borrow a view");
            };
            let Val::Image(image) = self.read_use(f, v)? else {
                return fail(V::TypeConfusion, "Unicode source slice");
            };
            (image, result_layout, None, self.get(f, v)?.borrow)
        } else {
            let (Passing::Mut, ArgumentValue::Address { address, version }) =
                (argument.passing, argument.value)
            else {
                return fail(V::Malformed, "Unicode next must have an exclusive cursor");
            };
            let r = self.resolve_use(f, address)?;
            self.check_writable(&r)?;
            let a = self.ledger.check(r.ptr)?;
            let size = self.size(r.layout)?;
            a.extent.check_range(r.ptr.offset, size)?;
            if !a.extent.all_init(r.ptr.offset, size) {
                return fail(V::UninitializedRead, "Unicode cursor bytes");
            }
            (
                a.extent.copy(r.ptr.offset, size, Some((&[], &[]))),
                r.layout,
                Some((r, version)),
                None,
            )
        };
        let fields = self.record(cursor_layout)?.fields.clone();
        let [view, position] = fields.as_slice() else {
            return fail(V::Malformed, "Unicode cursor layout");
        };
        if view.offset != 0 || !matches!(self.record(view.layout)?.kind, LayoutKind::Slice { .. }) {
            return fail(V::TypeConfusion, "Unicode cursor source is not a slice");
        }
        let bytes = self.slice_content(&image, 0, view.layout)?;
        let current = if open {
            0
        } else {
            u64::from_le_bytes(image.read_bytes(position.offset, 8).try_into().unwrap())
        };
        let operand = if open {
            OpValue::Bytes(bytes.clone())
        } else {
            OpValue::ViewCursor {
                bytes: bytes.clone(),
                position: current,
            }
        };
        let outcome = self.ops.call(&id, &[operand], &mut self.output);
        let (value, new_position) = match outcome {
            OpOutcome::Value(OpValue::ViewCursor {
                bytes: returned,
                position,
            }) if open => {
                if returned != bytes || position != 0 {
                    return fail(V::Malformed, "Unicode open changed its borrowed source");
                }
                (None, position)
            }
            OpOutcome::Updated { value, arguments } if !open => {
                let [
                    (
                        0,
                        OpValue::ViewCursor {
                            bytes: returned,
                            position,
                        },
                    ),
                ] = arguments.as_slice()
                else {
                    return fail(V::Malformed, "Unicode next update");
                };
                if returned != &bytes || *position < current || *position > bytes.len() as u64 + 1 {
                    return fail(
                        V::Malformed,
                        "Unicode next changed its source or position range",
                    );
                }
                (Some(value), *position)
            }
            OpOutcome::Fail(rule) => {
                if let Some((r, version)) = address {
                    self.view_version(f, r.root_slot, version)?;
                }
                self.record_failure(f, &rule, site.anchor);
                self.take_failure(f, failure, false)?;
                return Ok(Control::Continue);
            }
            OpOutcome::Unsupported => {
                return fail(V::Unsupported, format!("operation {:?}", id.definition));
            }
            _ => return fail(V::Malformed, "Unicode operation outcome"),
        };
        let val = if open {
            for cell in image.cells.values_mut() {
                if let Cell::Data {
                    counted, borrow, ..
                } = cell
                {
                    *counted = false;
                    *borrow = source_borrow.or(*borrow);
                }
            }
            let mut cursor = Extent::new(self.size(cursor_layout)?);
            cursor.write_bytes(0, &vec![0; self.size(cursor_layout)? as usize]);
            cursor.place(view.offset, image);
            cursor.write_bytes(position.offset, &new_position.to_le_bytes());
            Val::Image(cursor)
        } else {
            let (r, version) = address.unwrap();
            self.ledger
                .get_mut(r.ptr.alloc)
                .extent
                .write_bytes(r.ptr.offset + position.offset, &new_position.to_le_bytes());
            self.view_version(f, r.root_slot, version)?;
            match value.unwrap() {
                OpValue::Plain(bytes) => {
                    if bytes.len() as u64 != self.size(result_layout)? {
                        return fail(V::Malformed, "Unicode result size");
                    }
                    let mut image = Extent::new(bytes.len() as u64);
                    image.write_bytes(0, &bytes);
                    Val::Image(image)
                }
                OpValue::SomeSlice {
                    argument: 0,
                    start,
                    length,
                } => {
                    if start
                        .checked_add(length)
                        .is_none_or(|end| end > bytes.len() as u64)
                    {
                        return fail(V::OutOfExtent, "Unicode result outside its borrowed view");
                    }
                    let slice_fields = self.record(view.layout)?.fields.clone();
                    let owner = match image.cells.get(&slice_fields[0].offset) {
                        Some(Cell::Data { ptr, .. }) => *ptr,
                        _ => return fail(V::ForgedPointer, "Unicode cursor owner"),
                    };
                    let origin = u64::from_le_bytes(
                        image
                            .read_bytes(slice_fields[1].offset, 8)
                            .try_into()
                            .unwrap(),
                    );
                    let record = self.record(result_layout)?.clone();
                    let tag = record
                        .tag
                        .as_ref()
                        .ok_or_else(|| err(V::Malformed, "Unicode option tag"))?;
                    let field = &tag.variants[1].fields[0];
                    let mut output = Extent::new(self.size(result_layout)?);
                    output.write_bytes(0, &vec![0; self.size(result_layout)? as usize]);
                    output.write_bytes(tag.offset, &1u32.to_le_bytes());
                    output.place(
                        field.offset,
                        self.slice_image(
                            field.layout,
                            owner,
                            origin.checked_add(start).ok_or_else(|| {
                                err(V::OutOfExtent, "Unicode slice origin overflow")
                            })?,
                            length,
                        )?,
                    );
                    let (val, _) = self.copy_value(Val::Image(output), result_layout)?;
                    val
                }
                _ => return fail(V::Malformed, "Unicode next result"),
            }
        };
        self.define_fresh(f, result, val)?;
        self.jump(f, normal)?;
        Ok(Control::Continue)
    }

    fn view_version(
        &mut self,
        f: &mut Frame,
        slot: Option<SlotId>,
        version: ValueId,
    ) -> Checked<()> {
        if let Some(slot) = slot {
            f.versions[slot.index()] = Some(version);
        }
        self.define_version(f, version)
    }
}

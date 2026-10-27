use super::*;

impl<'m, 'a> FnEmitter<'m, 'a> {
    /// Binds each capture slot to its field of the environment the body was entered with.
    /// The environment is B's `environment()` record of the captures in ordinal order, each
    /// field at the layout of its capture slot (a borrowed capture's view, a `mut` capture's
    /// address), placed as an aggregate places fields, behind the counted block header.
    pub(super) fn bind_captures(&mut self) -> Emitted {
        let unit = self.unit;
        let mut captures: Vec<(u32, usize, LayoutId)> = Vec::new();
        for (s, slot) in unit.slots.iter().enumerate() {
            if let SlotKind::Capture(i) = slot.kind {
                captures.push((i, s, self.lir_type(slot.ty)?.layout));
            }
        }
        if captures.is_empty() {
            return Ok(());
        }
        captures.sort_by_key(|c| c.0);
        if captures.iter().enumerate().any(|(n, c)| c.0 as usize != n)
            || captures.len() != unit.captures.len()
        {
            return self.invalid("capture slots that are not one per environment field");
        }
        let Some(environment) = self.environment else {
            return self.invalid("capture slots in a unit without an environment");
        };
        if let Some(entry) = &unit.tables.receipt_entry
            && let Some(environment_layout) = entry.environment
        {
            entry
                .check_transport(self.signature.abi, unit, self.inputs.layouts)
                .map_err(|what| crate::CodegenError::Invalid {
                    definition: self.definition().clone(),
                    what: what.into(),
                })?;
            let (base, _) = hirsel_lir::receipt_closure::transport_environment(
                self.inputs.layouts,
                environment_layout,
                self.signature.abi.receipts.as_ref().unwrap(),
            )
            .map_err(|what| crate::CodegenError::Invalid {
                definition: self.definition().clone(),
                what: what.into(),
            })?;
            let record = self.layout(base)?;
            let payload = self.payload_of(environment, environment_layout)?;
            if record.fields.len() != captures.len() {
                return self.invalid("receipt wrapper base differs from capture storage");
            }
            for ((ordinal, slot, layout), field) in captures.iter().zip(&record.fields) {
                if *ordinal != field.ordinal || *layout != field.layout {
                    return self.invalid("receipt wrapper field differs from capture slot");
                }
                self.slots[*slot] =
                    Some(self.field_place(payload, field.offset, field.layout, field.unaligned)?);
            }
            return Ok(());
        }
        let mut offset = 0u64;
        let mut align = 1u32;
        let mut fields = Vec::new();
        for &(_, s, layout) in &captures {
            let field_align = self.layout(layout)?.align;
            offset = match align_up(offset, u64::from(field_align)) {
                Ok(offset) => offset,
                Err(_) => return self.invalid("an environment beyond u64"),
            };
            fields.push((s, offset, layout));
            offset += self.fixed_size(layout)?;
            align = align.max(field_align);
        }
        let start = match payload_offset(align) {
            Ok(start) => start,
            Err(_) => return self.invalid("an environment payload offset beyond u64"),
        };
        let payload = Place {
            columns: None,
            ptr: self.builder.offset(environment, self.const_u64(start)),
            layout: LayoutId(u32::MAX),
            align,
            block: Some(environment),
        };
        for (s, offset, layout) in fields {
            let ptr = self.builder.offset(payload.ptr, self.const_u64(offset));
            let align = align.min(low_bit(offset)).min(self.layout(layout)?.align);
            self.slots[s] = Some(Place {
                columns: None,
                ptr,
                layout,
                align,
                block: payload.block,
            });
        }
        Ok(())
    }

    /// Binds each declared parameter: its value, or its slot's storage.
    pub(super) fn bind_parameters(&mut self) -> Emitted {
        if self.resume.is_some() {
            return self.resume_parameters();
        }
        let unit = self.unit;
        let abi = self.signature.abi;
        if unit.parameters.len() != abi.parameters.len() {
            return self.invalid("parameter count differs from the FnAbi");
        }
        self.builder.position(self.prologue_block);
        for (i, parameter) in abi.parameters.iter().enumerate() {
            let value = unit.parameters[i];
            let slot = unit
                .slots
                .iter()
                .position(|s| s.kind == SlotKind::Parameter(parameter.source));
            let first = self.signature.first[i];
            let layout = parameter.layout;
            let align = self.layout(layout)?.align;
            if self.signature.roots.contains(&i) {
                let Some(s) = slot else {
                    return self.invalid("a root parameter without storage");
                };
                let place = self.stack(layout)?;
                self.store(
                    Shape::Scalar(ScalarKind::Pointer),
                    self.module.parameter(self.function, first),
                    place,
                );
                self.slots[s] = Some(place);
                continue;
            }
            match parameter.passing {
                ParameterPassing::ByValue => {
                    let args: Vec<_> = (0..parameter.slots.len() as u32)
                        .map(|k| self.module.parameter(self.function, first + k))
                        .collect();
                    let assembled = self.assemble(layout, &args)?;
                    match slot {
                        Some(s) => {
                            let place = self.stack(layout)?;
                            self.store(self.layout_shape(layout)?, assembled, place);
                            self.slots[s] = Some(place);
                        }
                        None => self.define(value, assembled)?,
                    }
                }
                ParameterPassing::ReadOnlyAddress
                | ParameterPassing::TransferAddress
                | ParameterPassing::ExclusiveAddress => {
                    let place = Place {
                        columns: None,
                        ptr: self.module.parameter(self.function, first),
                        layout,
                        align,
                        block: None,
                    };
                    match slot {
                        Some(s) => self.slots[s] = Some(place),
                        None if parameter.convention == Passing::Mut => {
                            return self.invalid("a mut parameter without its slot");
                        }
                        None => {
                            let loaded = self.load(self.layout_shape(layout)?, place);
                            self.define(value, loaded)?;
                        }
                    }
                }
                ParameterPassing::PointerLength => {
                    // The ABI pointer addresses the logical flat bytes. A variable flat
                    // value's LIR representation is its owner, whose header precedes them;
                    // a fixed flat value is inline. The length is the logical extent, not
                    // the owner's allocation capacity. Reads use the layout's size program.
                    let pointer = self.module.parameter(self.function, first);
                    match self.layout(layout)?.kind {
                        LayoutKind::Block { payload } | LayoutKind::Box { payload } => {
                            let pointer = if self.value_type(value)?.repr == ValueRepr::Uncounted {
                                pointer
                            } else {
                                self.uncounted_payload(pointer, payload)?.block.ok_or_else(
                                    || CodegenError::Invalid {
                                        definition: self.definition().clone(),
                                        what: "a flat extent parameter without its owner".into(),
                                    },
                                )?
                            };
                            if let Some(s) = slot {
                                let place = self.stack(layout)?;
                                self.store(Shape::Scalar(ScalarKind::Pointer), pointer, place);
                                self.slots[s] = Some(place);
                            } else {
                                self.define(value, pointer)?;
                            }
                        }
                        LayoutKind::Flat => {
                            let place = Place {
                                columns: None,
                                ptr: pointer,
                                layout,
                                align,
                                block: None,
                            };
                            if let Some(s) = slot {
                                self.slots[s] = Some(place);
                            } else {
                                self.define(value, self.load(self.layout_shape(layout)?, place))?;
                            }
                        }
                        _ => {
                            return self.invalid("a pointer/length parameter of a non-flat layout");
                        }
                    }
                }
            }
        }
        self.bind_captures()?;
        self.bind_receipt_entry()
    }

    /// A by-value parameter from its physical slots: none for unit, one scalar, or the
    /// scalar leaves of a multi-slot value at their layout offsets.
    pub(super) fn assemble(&self, layout: LayoutId, args: &[Value<'m>]) -> Emitted<Value<'m>> {
        match (self.layout_shape(layout)?, args) {
            (Shape::Unit, []) => Ok(self.module.const_zero(self.llvm_type(Shape::Unit))),
            (Shape::Scalar(ScalarKind::Bool), [byte]) => {
                let zero = self.module.const_int(self.module.int_type(8), 0);
                Ok(self.builder.icmp(IntPredicate::Ne, *byte, zero))
            }
            (Shape::Scalar(_), [arg]) => Ok(*arg),
            (Shape::Callable, [code, environment]) => {
                let value = self.module.const_zero(self.llvm_type(Shape::Callable));
                let value = self.builder.insert(value, *code, 0);
                Ok(self.builder.insert(value, *environment, 1))
            }
            (shape @ Shape::Bytes { .. }, args) => {
                let leaves = &self.layout(layout)?.abi.scalars;
                if leaves.len() != args.len() {
                    return self
                        .unsupported("a by-value aggregate whose slots are not its scalar leaves");
                }
                let place = self.stack(layout)?;
                self.builder
                    .memset(place.ptr, 0, self.fixed_size(layout)?, place.align);
                for (&(offset, kind), &arg) in leaves.iter().zip(args) {
                    let at = self.field_place(place, offset, layout, false)?;
                    self.store(Shape::Scalar(kind), arg, at);
                }
                Ok(self.load(shape, place))
            }
            _ => self.invalid("a by-value parameter's slots do not match its layout"),
        }
    }

    /// The physical slots of a by-value argument (the inverse of `assemble`).
    pub(super) fn disassemble(
        &self,
        layout: LayoutId,
        value: Value<'m>,
    ) -> Emitted<Vec<Value<'m>>> {
        Ok(match self.layout_shape(layout)? {
            Shape::Unit => Vec::new(),
            Shape::Scalar(ScalarKind::Bool) => {
                vec![
                    self.builder
                        .cast(Cast::ZExt, value, self.module.int_type(8)),
                ]
            }
            Shape::Scalar(_) => vec![value],
            Shape::Callable => vec![
                self.builder.extract(value, 0),
                self.builder.extract(value, 1),
            ],
            shape @ Shape::Bytes { .. } => {
                let place = self.stack(layout)?;
                self.store(shape, value, place);
                let mut slots = Vec::new();
                for &(offset, kind) in &self.layout(layout)?.abi.scalars {
                    let at = self.field_place(place, offset, layout, false)?;
                    let leaf = self.load(Shape::Scalar(kind), at);
                    slots.push(if kind == ScalarKind::Bool {
                        self.builder.cast(Cast::ZExt, leaf, self.module.int_type(8))
                    } else {
                        leaf
                    });
                }
                slots
            }
        })
    }

    /// The place `offset` bytes into `place`, holding a value of `layout`.
    pub(super) fn field_place(
        &self,
        place: Place<'m>,
        offset: u64,
        layout: LayoutId,
        unaligned: bool,
    ) -> Emitted<Place<'m>> {
        if place.columns.is_some() {
            return self.column_field(place, offset, layout);
        }
        let ptr = if offset == 0 {
            place.ptr
        } else {
            self.builder.offset(place.ptr, self.const_u64(offset))
        };
        let align = if unaligned {
            1
        } else {
            place
                .align
                .min(low_bit(offset))
                .min(self.layout(layout)?.align)
        };
        Ok(Place {
            columns: None,
            ptr,
            layout,
            align,
            block: place.block,
        })
    }
}

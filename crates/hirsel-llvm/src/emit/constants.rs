use super::*;

impl<'m, 'a> FnEmitter<'m, 'a> {
    pub(super) fn constant(&mut self, constant: ConstantId) -> Emitted<Value<'m>> {
        let Some(data) = self.unit.tables.constants.get(constant.index()) else {
            return self.invalid("a constant outside the constant table");
        };
        let shape = self.shape(self.lir_type(data.ty)?)?;
        let module = self.module;
        Ok(match (&data.value, shape) {
            (ConstantValue::Unit, Shape::Unit) => module.const_zero(self.llvm_type(shape)),
            (ConstantValue::Bool(b), Shape::Scalar(ScalarKind::Bool)) => {
                module.const_int(module.int_type(1), u128::from(*b))
            }
            (ConstantValue::Integer(bits), Shape::Scalar(kind))
                if scalar::integer(kind).is_some() =>
            {
                let width = scalar::integer(kind).unwrap().0;
                if width < 128 && *bits >> width != 0 {
                    return self.invalid("an integer constant wider than its type");
                }
                module.const_int(module.int_type(width), *bits)
            }
            (ConstantValue::F32(bits), Shape::Scalar(ScalarKind::F32)) => {
                let int = module.const_int(module.int_type(32), u128::from(*bits));
                self.builder.cast(Cast::Bits, int, module.f32_type())
            }
            (ConstantValue::F64(bits), Shape::Scalar(ScalarKind::F64)) => {
                let int = module.const_int(module.int_type(64), u128::from(*bits));
                self.builder.cast(Cast::Bits, int, module.f64_type())
            }
            (ConstantValue::String(text), _) => {
                return self.text_literal(data.ty, text.as_bytes());
            }
            (ConstantValue::Bytes(bytes), _) => return self.text_literal(data.ty, bytes),
            _ => return self.invalid("a constant of another type"),
        })
    }

    /// A String or Bytes literal over a static immortal block: a Plain slice (owner, start 0,
    /// length) for a `Slice` layout, else the block reference itself.
    pub(super) fn text_literal(&mut self, ty: LirTypeId, bytes: &[u8]) -> Emitted<Value<'m>> {
        let ty = self.lir_type(ty)?;
        let record = self.layout(ty.layout)?;
        match (ty.repr, &record.kind) {
            (ValueRepr::Aggregate, &LayoutKind::Slice { element }) => {
                let field = |ordinal: u32| record.fields.iter().find(|f| f.ordinal == ordinal);
                let (Some(owner), Some(start), Some(length)) = (field(0), field(1), field(2))
                else {
                    return self.invalid("a slice layout without owner, start and length");
                };
                // The owner is the sequence block of the slice's element the table derived
                // with it.
                let Some(payload) = slice_storage(self.inputs.layouts, element) else {
                    return self.invalid("a slice without its element's sequence layout");
                };
                let block = self.literal_block(payload, bytes)?;
                let whole = match sequence_slice_start(self.layout(element)?) {
                    Ok(whole) => whole,
                    Err(error) => return self.invalid(format!("{error:?}")),
                };
                let place = self.stack(ty.layout)?;
                let size = self.fixed_size(ty.layout)?;
                self.builder.memset(place.ptr, 0, size, place.align);
                let at = self.field_place(place, owner.offset, owner.layout, false)?;
                self.store(Shape::Scalar(ScalarKind::Pointer), block, at);
                let at = self.field_place(place, start.offset, start.layout, false)?;
                self.store(Shape::Scalar(ScalarKind::U64), self.const_u64(whole), at);
                let at = self.field_place(place, length.offset, length.layout, false)?;
                let length = self.const_u64(bytes.len() as u64);
                self.store(Shape::Scalar(ScalarKind::U64), length, at);
                Ok(self.load(self.layout_shape(ty.layout)?, place))
            }
            (
                ValueRepr::Block | ValueRepr::Uncounted,
                LayoutKind::Block { payload } | LayoutKind::Box { payload },
            ) => {
                let payload = *payload;
                let block = self.literal_block(payload, bytes)?;
                if ty.repr == ValueRepr::Block {
                    return Ok(block);
                }
                Ok(self.payload_of(block, payload)?.ptr)
            }
            _ => self.invalid("a text constant of another type"),
        }
    }

    /// The payload of a counted block, behind its header.
    pub(super) fn payload_of(&self, block: Value<'m>, payload: LayoutId) -> Emitted<Place<'m>> {
        let align = self.layout(payload)?.align;
        let offset = match payload_offset(align) {
            Ok(offset) => offset,
            Err(error) => return self.invalid(format!("{error:?}")),
        };
        Ok(Place {
            ptr: self.builder.offset(block, self.const_u64(offset)),
            layout: payload,
            align,
            block: Some(block),
        })
    }

    /// `ReadStatic`: the static immortal blocks of the symbol's data. A reference to the
    /// constant's block names its header (an uncounted reference its payload); any other
    /// value is read from that payload.
    pub(super) fn read_static(&mut self, result: ValueId, symbol: SymbolId) -> Emitted<Value<'m>> {
        let Some(symbol) = self.unit.tables.symbols.get(symbol.index()) else {
            return self.invalid("a static outside the symbol table");
        };
        let Some(data) = self.inputs.statics.iter().find(|d| d.symbol == *symbol) else {
            return self.invalid("a static without its data");
        };
        let Some(root) = data.blocks.first() else {
            return self.invalid("static data without a block");
        };
        let held = root.layout;
        let name = format!(
            "hirsel.static.{}",
            symbol_name(&symbol.definition, symbol.instance.as_ref())
        );
        // A block names only blocks after it, so each is emitted after its targets.
        let mut emitted: Vec<Option<Value<'m>>> = vec![None; data.blocks.len()];
        for (index, block) in data.blocks.iter().enumerate().rev() {
            if let SizeSpec::Fixed(size) = self.layout(block.layout)?.size
                && size != block.bytes.len() as u64
            {
                return self.invalid("static data of another size than its layout");
            }
            let mut references = Vec::with_capacity(block.references.len());
            for r in &block.references {
                let target = r.target as usize;
                let Some(Some(target)) = emitted.get(target).filter(|_| target > index) else {
                    return self.invalid("a static reference to no later block");
                };
                references.push((r.offset, *target));
            }
            let descriptor = self.descriptor(block.layout)?;
            let align = self.layout(block.layout)?.align;
            let block_name = match index {
                0 => name.clone(),
                _ => format!("{name}.{index}"),
            };
            emitted[index] = Some(self.static_block(
                &block_name,
                descriptor,
                align,
                &block.bytes,
                &references,
            )?);
        }
        let Some(block) = emitted[0] else {
            return self.invalid("static data without a block");
        };
        let ty = self.value_type(result)?;
        match (ty.repr, &self.layout(ty.layout)?.kind) {
            (
                ValueRepr::Block,
                LayoutKind::Block { payload: p } | LayoutKind::Box { payload: p },
            ) if *p == held => Ok(block),
            (ValueRepr::Uncounted, LayoutKind::Block { .. } | LayoutKind::Box { .. }) => {
                Ok(self.payload_of(block, held)?.ptr)
            }
            _ if ty.layout == held => {
                let place = self.payload_of(block, held)?;
                Ok(self.load(self.layout_shape(held)?, place))
            }
            _ => self.invalid("a static read at another layout than its data"),
        }
    }
}

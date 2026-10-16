//! Static data (CT-2): a materialized constant encoded as immortal static
//! blocks at its layouts (`hirsel_lir::statics`), for every constant a unit
//! reads with `ReadStatic`.
//!
//! The encoding writes each value at its layout record exactly as a
//! construction would: scalars little-endian at their size, an aggregate's
//! and an active variant's fields at their offsets with the variant's tag
//! form, inline repeats and Simd lanes at the element size, a sequence
//! payload as its u64 count, its u64 capacity (the count) and then its
//! elements from their alignment (runtime ABI 5). A block reference becomes a
//! cell naming another static block, and a String or Bytes slice names a whole
//! byte sequence block (its `sequence_slice_start`) and its length.
//! Flat data is one pointer-free block, sized from the materialized value.
//! A value this cannot lay out (a callable, a non-flat Packed extent, a handle) is a `StaticError`, never a guessed image. Only a
//! library constant holds a function today (`u32.order`'s comparator, an
//! operation): user constants of function type are not data.
use hirsel_lir::ir::Symbol;
use hirsel_lir::layout::{
    LayoutBuilder, SEQUENCE_CAPACITY_OFFSET, sequence_elements_offset, sequence_slice_start,
};
use hirsel_lir::layout_types::{LayoutId, LayoutKind, ScalarKind, SizeSpec, TagForm};
use hirsel_lir::statics::{StaticBlock, StaticData, StaticReference};
use hirsel_tir::{Constant, Constructor, Materialized, ValueKind, ValueNode};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StaticError(pub String);

fn unsupported<T>(message: impl Into<String>) -> Result<T, StaticError> {
    Err(StaticError(message.into()))
}

/// Encode `value`, the constant `symbol` names, at its type's layout.
pub fn encode(
    layouts: &mut LayoutBuilder<'_>,
    symbol: Symbol,
    value: &Materialized,
) -> Result<StaticData, StaticError> {
    let layout = layouts
        .layout(&value.types, value.root.ty)
        .map_err(|e| StaticError(format!("the constant's layout: {e:?}")))?;
    let mut encoder = Encoder {
        layouts,
        blocks: Vec::new(),
    };
    match encoder.kind(layout) {
        // A reference constant is its block: block 0 is that payload.
        LayoutKind::Block { payload } | LayoutKind::Box { payload } => {
            encoder.block(payload, &value.root)?;
        }
        _ => {
            let size = encoder.fixed(layout)?;
            encoder.reserve(layout, size);
            encoder.write(layout, &value.root, 0, 0)?;
        }
    }
    Ok(StaticData {
        symbol,
        blocks: encoder.blocks,
    })
}

struct Encoder<'a, 'o> {
    layouts: &'a mut LayoutBuilder<'o>,
    blocks: Vec<StaticBlock>,
}

impl Encoder<'_, '_> {
    fn kind(&self, layout: LayoutId) -> LayoutKind {
        self.layouts.record(layout).kind.clone()
    }

    fn fixed(&self, layout: LayoutId) -> Result<u64, StaticError> {
        match self.layouts.record(layout).size {
            SizeSpec::Fixed(size) => Ok(size),
            _ => unsupported("static data of a dynamic extent"),
        }
    }

    fn reserve(&mut self, layout: LayoutId, size: u64) -> u32 {
        self.blocks.push(StaticBlock {
            layout,
            bytes: vec![0; size as usize],
            references: Vec::new(),
        });
        (self.blocks.len() - 1) as u32
    }

    /// A new static block of `payload` holding `node` (a Box's content, a
    /// sequence, a String or Bytes); its index.
    fn block(&mut self, payload: LayoutId, node: &ValueNode) -> Result<u32, StaticError> {
        let content = match &node.kind {
            ValueKind::Construct {
                constructor: Constructor::Box,
                fields,
            } => match fields.as_slice() {
                [content] => content,
                _ => return unsupported("a box without exactly one field"),
            },
            _ => node,
        };
        if self.kind(payload) == LayoutKind::Flat {
            let bytes = self.flat_image(payload, content)?;
            let index = self.reserve(payload, bytes.len() as u64);
            self.put(index, 0, &bytes);
            return Ok(index);
        }
        let size = match self.kind(payload) {
            LayoutKind::Sequence { element } => {
                let (start, stride) = self.elements(element)?;
                let count = sequence_len(content)?;
                stride
                    .checked_mul(count)
                    .and_then(|n| n.checked_add(start))
                    .ok_or_else(|| StaticError("a static sequence beyond u64".into()))?
            }
            _ => self.fixed(payload)?,
        };
        let index = self.reserve(payload, size);
        self.write(payload, content, index, 0)?;
        Ok(index)
    }

    /// A sequence payload's element offset and stride.
    fn elements(&self, element: LayoutId) -> Result<(u64, u64), StaticError> {
        let start = sequence_elements_offset(self.layouts.record(element))
            .map_err(|e| StaticError(format!("{e:?}")))?;
        Ok((start, self.fixed(element)?))
    }

    fn put(&mut self, block: u32, offset: u64, bytes: &[u8]) {
        let at = offset as usize;
        self.blocks[block as usize].bytes[at..at + bytes.len()].copy_from_slice(bytes);
    }

    fn reference(&mut self, block: u32, offset: u64, target: u32) {
        self.blocks[block as usize]
            .references
            .push(StaticReference { offset, target });
    }

    /// Write `node` at `layout` into `block` at `offset`.
    fn write(
        &mut self,
        layout: LayoutId,
        node: &ValueNode,
        block: u32,
        offset: u64,
    ) -> Result<(), StaticError> {
        let record = self.layouts.record(layout).clone();
        if record.kind == LayoutKind::Flat {
            let bytes = self.flat_image(layout, node)?;
            self.put(block, offset, &bytes);
            return Ok(());
        }
        match (&record.kind, &node.kind) {
            (LayoutKind::Unit, _) => Ok(()),
            (LayoutKind::Scalar(kind), ValueKind::Constant(c)) => {
                let size = self.fixed(layout)? as usize;
                let bits: u128 = match (kind, c) {
                    (ScalarKind::Bool, Constant::Bool(b)) => u128::from(*b),
                    (ScalarKind::F32, Constant::F32(x)) => u128::from(*x),
                    (ScalarKind::F64, Constant::F64(x)) => u128::from(*x),
                    (ScalarKind::Pointer | ScalarKind::F32 | ScalarKind::F64, _)
                    | (ScalarKind::Bool, _) => {
                        return unsupported(format!("constant {c:?} at a {kind:?} scalar"));
                    }
                    (_, Constant::Integer(n)) => *n,
                    _ => return unsupported(format!("constant {c:?} at a {kind:?} scalar")),
                };
                self.put(block, offset, &bits.to_le_bytes()[..size]);
                Ok(())
            }
            (
                LayoutKind::Aggregate,
                ValueKind::Construct {
                    constructor: Constructor::Tuple | Constructor::Record(_),
                    fields,
                },
            ) => {
                for field in &record.fields {
                    let Some(value) = fields.get(field.ordinal as usize) else {
                        return unsupported("a constant without a field its layout has");
                    };
                    self.write(field.layout, value, block, offset + field.offset)?;
                }
                Ok(())
            }
            (
                LayoutKind::Variant,
                ValueKind::Construct {
                    constructor: Constructor::Variant { variant, .. },
                    fields,
                },
            ) => {
                let Some(tag) = &record.tag else {
                    return unsupported("a variant layout without a tag");
                };
                let Some(active) = tag.variants.iter().find(|v| v.ordinal == *variant) else {
                    return unsupported("a variant its layout does not have");
                };
                // A niche's tag word is the dataful field's reference itself,
                // and zero (as reserved) for the nullary variant.
                if tag.form == TagForm::Ordinal {
                    self.put(block, offset + tag.offset, &variant.to_le_bytes());
                }
                for field in &active.fields {
                    let Some(value) = fields.get(field.ordinal as usize) else {
                        return unsupported("a variant without a field its layout has");
                    };
                    self.write(field.layout, value, block, offset + field.offset)?;
                }
                Ok(())
            }
            (
                LayoutKind::Inline { element, .. } | LayoutKind::Simd { element, .. },
                ValueKind::Construct {
                    constructor: Constructor::Sequence,
                    fields,
                },
            ) => {
                let stride = self.fixed(*element)?;
                if stride * fields.len() as u64 != self.fixed(layout)? {
                    return unsupported("an inline constant of another length than its layout");
                }
                for (i, value) in fields.iter().enumerate() {
                    self.write(*element, value, block, offset + stride * i as u64)?;
                }
                Ok(())
            }
            (LayoutKind::Sequence { element }, _) => {
                let (start, stride) = self.elements(*element)?;
                let count = sequence_len(node)?.to_le_bytes();
                self.put(block, offset, &count);
                self.put(block, offset + SEQUENCE_CAPACITY_OFFSET, &count);
                match &node.kind {
                    ValueKind::Constant(Constant::String(s)) => {
                        self.put(block, offset + start, s.as_bytes());
                    }
                    ValueKind::Constant(Constant::Bytes(b)) => {
                        self.put(block, offset + start, b);
                    }
                    ValueKind::Construct { fields, .. } => {
                        for (i, value) in fields.iter().enumerate() {
                            let at = offset + start + stride * i as u64;
                            self.write(*element, value, block, at)?;
                        }
                    }
                    _ => unreachable!("sequence_len accepted it"),
                }
                Ok(())
            }
            (LayoutKind::Block { payload } | LayoutKind::Box { payload }, _) => {
                let target = self.block(*payload, node)?;
                self.reference(block, offset, target);
                Ok(())
            }
            (LayoutKind::Slice { element }, ValueKind::Constant(c)) => {
                let length = match c {
                    Constant::String(s) => s.len() as u64,
                    Constant::Bytes(b) => b.len() as u64,
                    _ => return unsupported(format!("constant {c:?} at a slice")),
                };
                let Some(owner) = self.sequence_of(*element) else {
                    return unsupported("a static slice whose sequence layout is not derived");
                };
                let target = self.block(owner, node)?;
                let start = sequence_slice_start(self.layouts.record(*element))
                    .map_err(|e| StaticError(format!("{e:?}")))?;
                for field in &record.fields {
                    match field.ordinal {
                        0 => self.reference(block, offset + field.offset, target),
                        1 => self.put(block, offset + field.offset, &start.to_le_bytes()),
                        2 => self.put(block, offset + field.offset, &length.to_le_bytes()),
                        _ => return unsupported("a slice layout with a fourth word"),
                    }
                }
                Ok(())
            }
            (LayoutKind::Callable { .. }, ValueKind::FunctionReference(function)) => unsupported(
                format!("a function value ({}) in static data", function.0.name),
            ),
            (
                LayoutKind::Opaque { owner },
                ValueKind::Construct {
                    constructor,
                    fields,
                },
            ) if owner.0.package.name == "core"
                && matches!(
                    (owner.0.name.as_str(), constructor),
                    ("Map", Constructor::Map) | ("Set", Constructor::Set)
                ) =>
            {
                // Materialization has already applied duplicate/upsert semantics.
                // The immutable image keeps that order and remains unindexed.
                if record.fields.len() != 2 {
                    return unsupported("a keyed container without entries and index");
                }
                for field in &record.fields {
                    let (LayoutKind::Block { payload } | LayoutKind::Box { payload }) =
                        self.kind(field.layout)
                    else {
                        return unsupported("a keyed container field without a sequence block");
                    };
                    let mut sequence = node.clone();
                    sequence.kind = ValueKind::Construct {
                        constructor: Constructor::Sequence,
                        fields: if field.ordinal == 0 {
                            fields.clone()
                        } else {
                            Vec::new()
                        },
                    };
                    let target = self.block(payload, &sequence)?;
                    self.reference(block, offset + field.offset, target);
                }
                Ok(())
            }
            (LayoutKind::Opaque { owner }, _) => {
                unsupported(format!("static data of the library owner {owner:?}"))
            }
            (kind, value) => unsupported(format!("static data of {value:?} at a {kind:?} layout")),
        }
    }

    /// Encode a materialized flat value at the already-derived in-memory layout.
    /// Prefixes measure the bytes following them; variable sequences include their
    /// count in that extent. No reference cells or separate child blocks are created.
    fn flat_image(&self, layout: LayoutId, node: &ValueNode) -> Result<Vec<u8>, StaticError> {
        let record = self.layouts.record(layout);
        if record.kind == LayoutKind::Flat
            && let [content] = record.fields.as_slice()
            && matches!(content.offset, 8 | 16)
            && matches!(record.size, SizeSpec::Dynamic(_))
        {
            let mut bytes = Vec::new();
            match &node.kind {
                ValueKind::Constant(Constant::String(text)) => bytes.extend(text.as_bytes()),
                ValueKind::Constant(Constant::Bytes(data)) => bytes.extend(data),
                ValueKind::Construct {
                    constructor: Constructor::Sequence,
                    fields,
                } => {
                    if content.offset == 16 {
                        bytes.extend((fields.len() as u64).to_le_bytes());
                    }
                    for value in fields {
                        bytes.extend(self.flat_image(content.layout, value)?);
                    }
                }
                _ if content.offset == 8 => bytes = self.flat_image(content.layout, node)?,
                _ => return unsupported("a flat variable sequence without sequence contents"),
            }
            let mut prefixed = (bytes.len() as u64).to_le_bytes().to_vec();
            prefixed.extend(bytes);
            return Ok(prefixed);
        }
        match (&record.kind, &node.kind) {
            (LayoutKind::Unit, _) => Ok(Vec::new()),
            (LayoutKind::Scalar(kind), ValueKind::Constant(value)) => {
                let bits = match (kind, value) {
                    (ScalarKind::Bool, Constant::Bool(b)) => u128::from(*b),
                    (ScalarKind::F32, Constant::F32(b)) => u128::from(*b),
                    (ScalarKind::F64, Constant::F64(b)) => u128::from(*b),
                    (
                        ScalarKind::Pointer | ScalarKind::Bool | ScalarKind::F32 | ScalarKind::F64,
                        _,
                    ) => return unsupported("a flat scalar constant of another kind"),
                    (_, Constant::Integer(bits)) => *bits,
                    _ => return unsupported("a flat scalar without scalar bits"),
                };
                Ok(bits.to_le_bytes()[..self.fixed(layout)? as usize].to_vec())
            }
            (
                LayoutKind::Flat,
                ValueKind::Construct {
                    constructor: Constructor::Record(_) | Constructor::Tuple,
                    fields,
                },
            ) => {
                let mut bytes = Vec::new();
                for field in &record.fields {
                    let value = fields
                        .get(field.ordinal as usize)
                        .ok_or_else(|| StaticError("a flat constant without its field".into()))?;
                    bytes.extend(self.flat_image(field.layout, value)?);
                }
                Ok(bytes)
            }
            (
                LayoutKind::Inline { element, count },
                ValueKind::Construct {
                    constructor: Constructor::Sequence,
                    fields,
                },
            ) => {
                if *count != fields.len() as u64 {
                    return unsupported("a flat inline constant of another length");
                }
                let mut bytes = Vec::new();
                for value in fields {
                    bytes.extend(self.flat_image(*element, value)?);
                }
                Ok(bytes)
            }
            (
                LayoutKind::Packed,
                ValueKind::Construct {
                    constructor: Constructor::Variant { variant, .. },
                    fields,
                },
            ) => {
                let active = record
                    .tag
                    .as_ref()
                    .and_then(|tag| tag.variants.iter().find(|v| v.ordinal == *variant))
                    .ok_or_else(|| StaticError("a flat constant without its variant".into()))?;
                let mut bytes = variant.to_le_bytes().to_vec();
                for field in &active.fields {
                    let value = fields
                        .get(field.ordinal as usize)
                        .ok_or_else(|| StaticError("a flat variant without its field".into()))?;
                    bytes.extend(self.flat_image(field.layout, value)?);
                }
                Ok(bytes)
            }
            (kind, value) => unsupported(format!("flat static data of {value:?} at {kind:?}")),
        }
    }

    /// The byte sequence payload layout a slice of `element` views.
    fn sequence_of(&self, element: LayoutId) -> Option<LayoutId> {
        self.layouts
            .table()
            .layouts
            .iter()
            .position(|r| r.kind == LayoutKind::Sequence { element })
            .map(|i| LayoutId(i as u32))
    }
}

/// The element count of a sequence constant.
fn sequence_len(node: &ValueNode) -> Result<u64, StaticError> {
    match &node.kind {
        ValueKind::Constant(Constant::String(s)) => Ok(s.len() as u64),
        ValueKind::Constant(Constant::Bytes(b)) => Ok(b.len() as u64),
        ValueKind::Construct {
            constructor: Constructor::Sequence,
            fields,
        } => Ok(fields.len() as u64),
        kind => unsupported(format!("{kind:?} as a sequence")),
    }
}

//! Observable values contain bits and declaration ordinals, never host addresses.
use hirsel_syntax::DefPath;
use hirsel_tir::BodyId;
use hirsel_types::{BuiltinType, Primitive};

/// Concrete lexical size bindings, keyed by the declaration rather than its ordinal alone.
pub type SizeBindings =
    std::collections::BTreeMap<(hirsel_types::NameSite, hirsel_types::SizeParamId), u64>;

/// A named callable has no environment. Generic type rows retain their owner.
#[derive(Clone, Debug)]
pub struct NamedFunction {
    pub definition: DefPath,
    pub arguments: Vec<hirsel_types::GenericArgument>,
    pub types: std::sync::Arc<hirsel_types::TypeStore>,
    pub ty: hirsel_types::TypeId,
}
impl PartialEq for NamedFunction {
    fn eq(&self, other: &Self) -> bool {
        self.definition == other.definition
            && self.arguments.len() == other.arguments.len()
            && self.arguments.iter().zip(&other.arguments).all(|(a, b)| {
                match (
                    hirsel_types::encoding::canonical_argument_encoding(&self.types, a),
                    hirsel_types::encoding::canonical_argument_encoding(&other.types, b),
                ) {
                    (Ok(a), Ok(b)) => a == b,
                    _ => match (a, b) {
                        (
                            hirsel_types::GenericArgument::Type(a),
                            hirsel_types::GenericArgument::Type(b),
                        ) => hirsel_types::semantic_equal(&self.types, *a, &other.types, *b)
                            .unwrap_or(false),
                        _ => a == b,
                    },
                }
            })
    }
}
impl Eq for NamedFunction {}

#[derive(Debug, Eq, PartialEq)]
pub enum Value {
    FunctionReference(NamedFunction),
    /// Opaque simulation identity. Tags come from the library registry, never addresses.
    Handle {
        kind: String,
        id: u64,
    },
    Unit,
    Bool(bool),
    Integer(Integer),
    F32(u32),
    F64(u64),
    String(String),
    Bytes(Vec<u8>),
    Map(Vec<Option<Value>>),
    Set(Vec<Option<Value>>),
    Tuple(Vec<Option<Value>>),
    Record {
        definition: DefPath,
        fields: Vec<Option<Value>>,
    },
    Sequence {
        kind: BuiltinType,
        elements: Vec<Option<Value>>,
    },
    Box(Vec<Option<Value>>),
    Distinct {
        definition: DefPath,
        fields: Vec<Option<Value>>,
    },
    ResourceVariant {
        sizes: Vec<Option<u64>>,
        definition: DefPath,
        variant: u32,
        fields: Vec<Option<Value>>,
    },
    Resource {
        /// Concrete type-size bindings retained until the finalizer runs.
        sizes: Vec<Option<u64>>,
        definition: DefPath,
        fields: Vec<Option<Value>>,
    },
    Closure {
        /// Once-environment snapshots share the fact that invocation moved its
        /// owners into a frame. Many-use environments have no invocation state.
        invoked: Option<std::sync::Arc<std::sync::OnceLock<()>>>,
        definition: DefPath,
        body: BodyId,
        captures: Vec<Option<Value>>,
        /// Borrow/mut captures are snapshots owned by the enclosing frame.
        owned_captures: Vec<bool>,
        /// Lexical size bindings travel with the environment, including callbacks.
        sizes: SizeBindings,
    },
    Variant {
        variant: u32,
        fields: Vec<Option<Value>>,
    },
}

impl Value {
    pub(crate) fn begin_call(&self) {
        if let Self::Closure {
            invoked: Some(invoked),
            ..
        } = self
        {
            let _ = invoked.set(());
        }
    }
    pub(crate) fn fields_mut(&mut self) -> Option<&mut Vec<Option<Value>>> {
        match self {
            Self::Map(f)
            | Self::Set(f)
            | Self::Tuple(f)
            | Self::Box(f)
            | Self::Record { fields: f, .. }
            | Self::Variant { fields: f, .. }
            | Self::Resource { fields: f, .. }
            | Self::ResourceVariant { fields: f, .. }
            | Self::Distinct { fields: f, .. }
            | Self::Sequence { elements: f, .. }
            | Self::Closure { captures: f, .. } => Some(f),
            _ => None,
        }
    }

    /// Normalize external inputs and operation results as well as TIR literals.
    pub(crate) fn canonicalize(&mut self) {
        let mut pending = vec![self];
        while let Some(value) = pending.pop() {
            match value {
                Value::F32(bits) if f32::from_bits(*bits).is_nan() => *bits = 0x7fc0_0000,
                Value::F64(bits) if f64::from_bits(*bits).is_nan() => *bits = 0x7ff8_0000_0000_0000,
                Value::Map(fields)
                | Value::Set(fields)
                | Value::Tuple(fields)
                | Value::Variant { fields, .. }
                | Value::Record { fields, .. }
                | Value::Distinct { fields, .. }
                | Value::Resource { fields, .. }
                | Value::ResourceVariant { fields, .. }
                | Value::Box(fields)
                | Value::Sequence {
                    elements: fields, ..
                }
                | Value::Closure {
                    captures: fields, ..
                } => {
                    pending.extend(fields.iter_mut().filter_map(Option::as_mut));
                }
                _ => {}
            }
        }
    }
}

/// A sized two's-complement bit vector; construction rejects out-of-width bits.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Integer {
    kind: Primitive,
    bits: u128,
}
impl Integer {
    pub fn new(kind: Primitive, bits: u128) -> Option<Self> {
        let (width, _) = integer_layout(kind)?;
        (bits & !mask(width) == 0).then_some(Self { kind, bits })
    }
    pub fn kind(self) -> Primitive {
        self.kind
    }
    pub fn bits(self) -> u128 {
        self.bits
    }
    pub fn width(self) -> u32 {
        integer_layout(self.kind).unwrap().0
    }
    pub fn is_signed(self) -> bool {
        integer_layout(self.kind).unwrap().1
    }
    pub fn signed(self) -> i128 {
        let shift = 128 - self.width();
        ((self.bits << shift) as i128) >> shift
    }
    pub(crate) fn masked(self, bits: u128) -> Self {
        Self {
            bits: bits & mask(self.width()),
            ..self
        }
    }
}
pub fn integer_layout(kind: Primitive) -> Option<(u32, bool)> {
    use Primitive::*;
    Some(match kind {
        U8 => (8, false),
        U16 => (16, false),
        U32 => (32, false),
        U64 => (64, false),
        U128 => (128, false),
        I8 => (8, true),
        I16 => (16, true),
        I32 => (32, true),
        I64 => (64, true),
        I128 => (128, true),
        _ => return None,
    })
}
pub(crate) fn mask(width: u32) -> u128 {
    u128::MAX >> (128 - width)
}
pub(crate) fn signed_bounds(width: u32) -> (i128, i128) {
    let max = (mask(width) >> 1) as i128;
    (-max - 1, max)
}
/// SEM-2: canonical positive quiet NaN, including constants and unary results.
pub fn f32_value(value: f32) -> Value {
    Value::F32(if value.is_nan() {
        0x7fc0_0000
    } else {
        value.to_bits()
    })
}
pub fn f64_value(value: f64) -> Value {
    Value::F64(if value.is_nan() {
        0x7ff8_0000_0000_0000
    } else {
        value.to_bits()
    })
}

impl Clone for Value {
    fn clone(&self) -> Self {
        fn shallow(v: &Value) -> Value {
            match v {
                Value::FunctionReference(reference) => Value::FunctionReference(reference.clone()),
                Value::Unit => Value::Unit,
                Value::Bool(x) => Value::Bool(*x),
                Value::Integer(x) => Value::Integer(*x),
                Value::F32(x) => Value::F32(*x),
                Value::F64(x) => Value::F64(*x),
                Value::String(x) => Value::String(x.clone()),
                Value::Bytes(x) => Value::Bytes(x.clone()),
                Value::Handle { kind, id } => Value::Handle {
                    kind: kind.clone(),
                    id: *id,
                },
                Value::Map(f) => Value::Map(vec![None; f.len()]),
                Value::Set(f) => Value::Set(vec![None; f.len()]),
                Value::Tuple(f) => Value::Tuple(vec![None; f.len()]),
                Value::Box(f) => Value::Box(vec![None; f.len()]),
                Value::Record { definition, fields } => Value::Record {
                    definition: definition.clone(),
                    fields: vec![None; fields.len()],
                },
                Value::Distinct { definition, fields } => Value::Distinct {
                    definition: definition.clone(),
                    fields: vec![None; fields.len()],
                },
                Value::ResourceVariant {
                    sizes,
                    definition,
                    variant,
                    fields,
                } => Value::ResourceVariant {
                    sizes: sizes.clone(),
                    definition: definition.clone(),
                    variant: *variant,
                    fields: vec![None; fields.len()],
                },
                Value::Resource {
                    definition,
                    fields,
                    sizes,
                } => Value::Resource {
                    sizes: sizes.clone(),
                    definition: definition.clone(),
                    fields: vec![None; fields.len()],
                },
                Value::Variant { variant, fields } => Value::Variant {
                    variant: *variant,
                    fields: vec![None; fields.len()],
                },
                Value::Sequence { kind, elements } => Value::Sequence {
                    kind: *kind,
                    elements: vec![None; elements.len()],
                },
                Value::Closure {
                    invoked,
                    definition,
                    body,
                    captures,
                    owned_captures,
                    sizes,
                } => Value::Closure {
                    invoked: invoked.clone(),
                    definition: definition.clone(),
                    body: *body,
                    captures: vec![None; captures.len()],
                    owned_captures: owned_captures.clone(),
                    sizes: sizes.clone(),
                },
            }
        }
        let mut result = shallow(self);
        let mut work = vec![(self, &mut result)];
        while let Some((source, target)) = work.pop() {
            if let (Some(src), Some(dst)) = (source.fields(), target.fields_mut()) {
                for (src, dst) in src.iter().zip(dst.iter_mut()) {
                    if let Some(src) = src {
                        *dst = Some(shallow(src));
                        work.push((src, dst.as_mut().unwrap()));
                    }
                }
            }
        }
        result
    }
}
impl Value {
    pub(crate) fn fields(&self) -> Option<&[Option<Value>]> {
        match self {
            Self::Closure {
                invoked: Some(invoked),
                ..
            } if invoked.get().is_some() => Some(&[]),
            Self::Map(f)
            | Self::Set(f)
            | Self::Tuple(f)
            | Self::Box(f)
            | Self::Record { fields: f, .. }
            | Self::Distinct { fields: f, .. }
            | Self::Resource { fields: f, .. }
            | Self::ResourceVariant { fields: f, .. }
            | Self::Variant { fields: f, .. }
            | Self::Sequence { elements: f, .. }
            | Self::Closure { captures: f, .. } => Some(f),
            _ => None,
        }
    }
    /// Free a simulation tree without running finalizers or recursing in Rust.
    /// Engines use the cleanup worklist; this is for consuming tooling snapshots.
    pub fn discard(self) {
        let mut work = vec![self];
        while let Some(mut value) = work.pop() {
            if let Some(fields) = value.fields_mut() {
                work.extend(fields.iter_mut().filter_map(Option::take));
            }
        }
    }
}

impl Value {
    pub(crate) fn inline_bits(&self) -> bool {
        let mut work = vec![self];
        while let Some(v) = work.pop() {
            match v {
                Self::Unit
                | Self::Bool(_)
                | Self::Integer(_)
                | Self::F32(_)
                | Self::F64(_)
                | Self::Handle { .. } => (),
                Self::Tuple(f)
                | Self::Record { fields: f, .. }
                | Self::Distinct { fields: f, .. }
                | Self::Variant { fields: f, .. } => {
                    work.extend(f.iter().filter_map(Option::as_ref))
                }
                _ => return false,
            }
        }
        true
    }
}

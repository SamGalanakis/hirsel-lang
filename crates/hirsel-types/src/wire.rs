//! SER-2 version 1. This is a portable data codec, never a native layout decoder.
//! Values use a flat arena so traversal and destruction cannot overflow the stack.
use crate::serialization::{SchemaArgument, SchemaId, SchemaNode, SerializableSchema};
use crate::{BuiltinType, Primitive, Size};
use std::collections::BTreeSet;

mod schema;

pub const VERSION: u32 = 1;
pub const MAGIC: &[u8; 4] = b"HSC1";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ValueId(pub usize);

/// Engine-neutral data. Scalar floats retain bits; encoding canonicalizes NaNs.
/// Wrappers represent nominal, Box and Packed boundaries, never live identities.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ValueNode {
    Bool(bool),
    Unsigned(u128),
    Signed(i128),
    F32(u32),
    F64(u64),
    String(String),
    Bytes(Vec<u8>),
    Fields(Vec<ValueId>),
    Variant { ordinal: u32, fields: Vec<ValueId> },
    Sequence(Vec<ValueId>),
    Map(Vec<(ValueId, ValueId)>),
    Set(Vec<ValueId>),
    Wrapper(ValueId),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Value {
    pub root: ValueId,
    pub nodes: Vec<ValueNode>,
}

/// Host budgets are operational refusal, not language schema or value limits.
#[derive(Clone, Copy, Debug)]
pub struct Limits {
    pub bytes: usize,
    pub values: usize,
    pub work: usize,
}
impl Default for Limits {
    fn default() -> Self {
        Self {
            bytes: 64 * 1024 * 1024,
            values: 1_000_000,
            work: 4_000_000,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    Schema,
    Version,
    SchemaMismatch,
    Truncated,
    Trailing,
    Scalar,
    Utf8,
    Tag,
    Length,
    Duplicate,
    Order,
    Value,
    Limit,
}

#[derive(Clone, Debug)]
enum Node {
    Scalar(Primitive),
    Fields(Vec<usize>),
    Variant(Vec<Vec<usize>>),
    Wrapper(usize),
    Sequence { element: usize, count: Option<u64> },
    Map { key: usize, value: usize },
    Set(usize),
}

/// An executable plan compiled from a concrete checked schema. It does not prove
/// source opt-in: the caller obtains that proof through serializable_schema.
pub struct Codec {
    nodes: Vec<Node>,
    root: usize,
    descriptor: Vec<u8>,
    fixed_flat: Option<usize>,
}

struct Budget {
    limits: Limits,
    work: usize,
    values: usize,
}
impl Budget {
    fn step(&mut self) -> Result<(), Error> {
        self.work = self.work.checked_add(1).ok_or(Error::Limit)?;
        if self.work > self.limits.work {
            return Err(Error::Limit);
        }
        Ok(())
    }
    fn count(&mut self, count: usize) -> Result<(), Error> {
        self.values = self.values.checked_add(count).ok_or(Error::Limit)?;
        if self.values > self.limits.values {
            return Err(Error::Limit);
        }
        Ok(())
    }
}
fn put(out: &mut Vec<u8>, bytes: &[u8], budget: &Budget) -> Result<(), Error> {
    if bytes.len() > budget.limits.bytes.saturating_sub(out.len()) {
        return Err(Error::Limit);
    }
    out.try_reserve(bytes.len()).map_err(|_| Error::Limit)?;
    out.extend_from_slice(bytes);
    Ok(())
}
fn count(out: &mut Vec<u8>, n: usize, budget: &Budget) -> Result<(), Error> {
    put(
        out,
        &u64::try_from(n).map_err(|_| Error::Length)?.to_le_bytes(),
        budget,
    )
}

impl Codec {
    pub fn descriptor(&self) -> &[u8] {
        &self.descriptor
    }

    /// Exact packed extent for the conservative common subset of private
    /// FlatFormatV0 and SER-2: scalars, records, tuples and fixed Inline arrays.
    /// Box, Simd, variants and variable containers have no layout-equality proof.
    pub fn fixed_flat_extent(&self) -> Option<usize> {
        self.fixed_flat
    }

    pub fn encode_fixed_flat(&self, bytes: &[u8], limits: Limits) -> Result<Vec<u8>, Error> {
        if self.fixed_flat != Some(bytes.len()) {
            return Err(Error::Schema);
        }
        self.decode_payload_inner(bytes, limits, false)?;
        self.envelope(bytes, limits)
    }

    pub fn encode_payload(&self, value: &Value, limits: Limits) -> Result<Vec<u8>, Error> {
        let mut budget = Budget {
            limits,
            work: 0,
            values: 0,
        };
        self.encode_part(value, self.root, value.root, &mut budget)
    }

    fn encode_part(
        &self,
        value: &Value,
        schema: usize,
        root: ValueId,
        budget: &mut Budget,
    ) -> Result<Vec<u8>, Error> {
        enum Job {
            Visit(usize, ValueId),
            Finish(usize, usize, Option<u32>),
        }
        let mut work = vec![Job::Visit(schema, root)];
        let mut parts: Vec<Vec<u8>> = vec![];
        while let Some(job) = work.pop() {
            budget.step()?;
            match job {
                Job::Finish(t, n, tag) => {
                    let start = parts.len().checked_sub(n).ok_or(Error::Value)?;
                    let children: Vec<_> = parts.drain(start..).collect();
                    let mut bytes = vec![];
                    match &self.nodes[t] {
                        Node::Wrapper(_) => {
                            parts.push(children.into_iter().next().ok_or(Error::Value)?);
                            continue;
                        }
                        Node::Variant(_) => {
                            put(&mut bytes, &tag.ok_or(Error::Value)?.to_le_bytes(), budget)?
                        }
                        Node::Sequence { count: None, .. } | Node::Set(_) => {
                            count(&mut bytes, n, budget)?
                        }
                        Node::Map { .. } => count(&mut bytes, n / 2, budget)?,
                        _ => (),
                    }
                    if matches!(self.nodes[t], Node::Map { .. }) {
                        let mut seen = BTreeSet::new();
                        for pair in children.as_chunks::<2>().0.iter() {
                            if !seen.insert(&pair[0]) {
                                return Err(Error::Duplicate);
                            }
                        }
                    }
                    let mut children = children;
                    if matches!(self.nodes[t], Node::Set(_)) {
                        children.sort_unstable();
                        if children.windows(2).any(|p| p[0] == p[1]) {
                            return Err(Error::Duplicate);
                        }
                    }
                    for child in children {
                        put(&mut bytes, &child, budget)?;
                    }
                    parts.push(bytes);
                }
                Job::Visit(t, id) => {
                    budget.count(1)?;
                    let v = value.nodes.get(id.0).ok_or(Error::Value)?;
                    let mut children = vec![];
                    let mut tag = None;
                    match (&self.nodes[t], v) {
                        (Node::Scalar(p), v) => {
                            let mut bytes = vec![];
                            encode_scalar(*p, v, &mut bytes, budget)?;
                            parts.push(bytes);
                            continue;
                        }
                        (Node::Wrapper(child), ValueNode::Wrapper(v)) => {
                            children.push((*child, *v))
                        }
                        (Node::Fields(types), ValueNode::Fields(fields)) => {
                            if types.len() != fields.len() {
                                return Err(Error::Value);
                            }
                            children.extend(types.iter().zip(fields).map(|(t, v)| (*t, *v)));
                        }
                        (Node::Variant(arms), ValueNode::Variant { ordinal, fields }) => {
                            let types = arms.get(*ordinal as usize).ok_or(Error::Tag)?;
                            if types.len() != fields.len() {
                                return Err(Error::Value);
                            }
                            tag = Some(*ordinal);
                            children.extend(types.iter().zip(fields).map(|(t, v)| (*t, *v)));
                        }
                        (
                            Node::Sequence {
                                element,
                                count: fixed,
                            },
                            ValueNode::Sequence(items),
                        ) => {
                            if fixed.is_some_and(|n| u64::try_from(items.len()).ok() != Some(n)) {
                                return Err(Error::Length);
                            }
                            children.extend(items.iter().map(|v| (*element, *v)));
                        }
                        (Node::Map { key, value }, ValueNode::Map(entries)) => {
                            for (k, v) in entries {
                                children.push((*key, *k));
                                children.push((*value, *v));
                            }
                        }
                        (Node::Set(element), ValueNode::Set(items)) => {
                            children.extend(items.iter().map(|v| (*element, *v)))
                        }
                        _ => return Err(Error::Value),
                    }
                    if children.len() > budget.limits.values.saturating_sub(budget.values) {
                        return Err(Error::Limit);
                    }
                    work.push(Job::Finish(t, children.len(), tag));
                    work.extend(children.into_iter().rev().map(|(t, v)| Job::Visit(t, v)));
                }
            }
            if work.len() > budget.limits.work {
                return Err(Error::Limit);
            }
        }
        if parts.len() != 1 {
            return Err(Error::Value);
        }
        parts.pop().ok_or(Error::Value)
    }

    pub fn encode(&self, value: &Value, limits: Limits) -> Result<Vec<u8>, Error> {
        let payload = self.encode_payload(value, limits)?;
        self.envelope(&payload, limits)
    }

    fn envelope(&self, payload: &[u8], limits: Limits) -> Result<Vec<u8>, Error> {
        let budget = Budget {
            limits,
            work: 0,
            values: 0,
        };
        let mut out = vec![];
        put(&mut out, MAGIC, &budget)?;
        put(&mut out, &VERSION.to_le_bytes(), &budget)?;
        count(&mut out, self.descriptor.len(), &budget)?;
        put(&mut out, &self.descriptor, &budget)?;
        count(&mut out, payload.len(), &budget)?;
        put(&mut out, payload, &budget)?;
        Ok(out)
    }

    /// Returns a borrowed canonical payload after full structural validation.
    /// Copying this slice is a valid flat fast path; arbitrary native bytes have
    /// no such proof and must first match this schema and validation exactly.
    pub fn validated_payload<'a>(
        &self,
        bytes: &'a [u8],
        limits: Limits,
    ) -> Result<&'a [u8], Error> {
        if bytes.len() > limits.bytes {
            return Err(Error::Limit);
        }
        let mut input = Input { bytes, at: 0 };
        if input.take(4)? != MAGIC {
            return Err(Error::Version);
        }
        if input.number(4)? != u128::from(VERSION) {
            return Err(Error::Version);
        }
        let len = input.length()?;
        if input.take(len)? != self.descriptor {
            return Err(Error::SchemaMismatch);
        }
        let len = input.length()?;
        let payload = input.take(len)?;
        if input.at != bytes.len() {
            return Err(Error::Trailing);
        }
        self.decode_payload_inner(payload, limits, false)?;
        Ok(payload)
    }

    pub fn decode(&self, bytes: &[u8], limits: Limits) -> Result<Value, Error> {
        // The validation walk is also the flat path without value materialization. Decode
        // constructs fresh data only after the envelope has been checked.
        if bytes.len() > limits.bytes {
            return Err(Error::Limit);
        }
        let mut input = Input { bytes, at: 0 };
        if input.take(4)? != MAGIC || input.number(4)? != u128::from(VERSION) {
            return Err(Error::Version);
        }
        let len = input.length()?;
        if input.take(len)? != self.descriptor {
            return Err(Error::SchemaMismatch);
        }
        let len = input.length()?;
        let payload = input.take(len)?;
        if input.at != bytes.len() {
            return Err(Error::Trailing);
        }
        self.decode_payload(payload, limits)
    }

    pub fn decode_payload(&self, bytes: &[u8], limits: Limits) -> Result<Value, Error> {
        self.decode_payload_inner(bytes, limits, true)
    }

    fn decode_payload_inner(
        &self,
        bytes: &[u8],
        limits: Limits,
        materialize: bool,
    ) -> Result<Value, Error> {
        if bytes.len() > limits.bytes {
            return Err(Error::Limit);
        }
        let mut budget = Budget {
            limits,
            work: 0,
            values: 0,
        };
        let mut input = Input { bytes, at: 0 };
        let mut nodes = vec![];
        let mut values = vec![];
        enum Build {
            Fields,
            Variant(u32),
            Sequence,
            Wrapper,
            Map,
            Set,
        }
        enum Job {
            Visit(usize),
            Finish(Build, usize),
            KeyStart { schema: usize, set: bool },
            KeyEnd { at: usize, set: bool },
            CollectionStart,
            CollectionEnd,
        }
        let mut work = vec![Job::Visit(self.root)];
        #[derive(Default)]
        struct Collection<'a> {
            keys: BTreeSet<&'a [u8]>,
            previous: Option<&'a [u8]>,
        }
        let mut collections: Vec<Collection<'_>> = vec![];
        while let Some(job) = work.pop() {
            budget.step()?;
            match job {
                Job::CollectionStart => collections.push(Collection::default()),
                Job::CollectionEnd => {
                    collections.pop().ok_or(Error::Value)?;
                }
                Job::KeyStart { schema, set } => {
                    work.push(Job::KeyEnd { at: input.at, set });
                    work.push(Job::Visit(schema));
                }
                Job::KeyEnd { at, set } => {
                    let key = &input.bytes[at..input.at];
                    let collection = collections.last_mut().ok_or(Error::Value)?;
                    if set {
                        if let Some(previous) = collection.previous {
                            if previous == key {
                                return Err(Error::Duplicate);
                            }
                            if previous > key {
                                return Err(Error::Order);
                            }
                        }
                        collection.previous = Some(key);
                    } else if !collection.keys.insert(key) {
                        return Err(Error::Duplicate);
                    }
                }
                Job::Finish(kind, n) => {
                    if !materialize {
                        continue;
                    }
                    let start = values.len().checked_sub(n).ok_or(Error::Value)?;
                    let children: Vec<_> = values.drain(start..).collect();
                    let node = match kind {
                        Build::Fields => ValueNode::Fields(children),
                        Build::Variant(ordinal) => ValueNode::Variant {
                            ordinal,
                            fields: children,
                        },
                        Build::Sequence => ValueNode::Sequence(children),
                        Build::Wrapper => ValueNode::Wrapper(children[0]),
                        Build::Set => ValueNode::Set(children),
                        Build::Map => ValueNode::Map(
                            children
                                .as_chunks::<2>()
                                .0
                                .iter()
                                .map(|p| (p[0], p[1]))
                                .collect(),
                        ),
                    };
                    let id = ValueId(nodes.len());
                    nodes.push(node);
                    values.push(id);
                }
                Job::Visit(t) => {
                    budget.count(1)?;
                    match &self.nodes[t] {
                        Node::Scalar(p) => {
                            let node = decode_scalar(*p, &mut input, materialize)?;
                            if materialize {
                                let id = ValueId(nodes.len());
                                nodes.push(node);
                                values.push(id);
                            }
                        }
                        Node::Fields(fields) => {
                            work.push(Job::Finish(Build::Fields, fields.len()));
                            work.extend(fields.iter().rev().map(|t| Job::Visit(*t)));
                        }
                        Node::Variant(arms) => {
                            let tag = input.number(4)? as u32;
                            let fields = arms.get(tag as usize).ok_or(Error::Tag)?;
                            work.push(Job::Finish(Build::Variant(tag), fields.len()));
                            work.extend(fields.iter().rev().map(|t| Job::Visit(*t)));
                        }
                        Node::Wrapper(child) => {
                            work.push(Job::Finish(Build::Wrapper, 1));
                            work.push(Job::Visit(*child));
                        }
                        Node::Sequence { element, count } => {
                            let n = match count {
                                Some(n) => usize::try_from(*n).map_err(|_| Error::Length)?,
                                None => input.length()?,
                            };
                            if n > budget.limits.values.saturating_sub(budget.values) {
                                return Err(Error::Limit);
                            }
                            work.push(Job::Finish(Build::Sequence, n));
                            work.extend((0..n).map(|_| Job::Visit(*element)));
                        }
                        Node::Map { key, value } => {
                            let n = input.length()?;
                            if n > budget.limits.values.saturating_sub(budget.values) / 2 {
                                return Err(Error::Limit);
                            }
                            work.push(Job::CollectionEnd);
                            work.push(Job::Finish(Build::Map, n * 2));
                            for _ in 0..n {
                                work.push(Job::Visit(*value));
                                work.push(Job::KeyStart {
                                    schema: *key,
                                    set: false,
                                });
                            }
                            work.push(Job::CollectionStart);
                        }
                        Node::Set(element) => {
                            let n = input.length()?;
                            if n > budget.limits.values.saturating_sub(budget.values) {
                                return Err(Error::Limit);
                            }
                            // Each start must be captured at actual visit time.
                            work.push(Job::CollectionEnd);
                            work.push(Job::Finish(Build::Set, n));
                            for _ in 0..n {
                                work.push(Job::KeyStart {
                                    schema: *element,
                                    set: true,
                                });
                            }
                            work.push(Job::CollectionStart);
                        }
                    }
                }
            }
            if work.len() > budget.limits.work {
                return Err(Error::Limit);
            }
        }
        if input.at != bytes.len() {
            return Err(Error::Trailing);
        }
        if materialize && values.len() != 1 {
            return Err(Error::Value);
        }
        Ok(Value {
            root: values.pop().unwrap_or(ValueId(0)),
            nodes,
        })
    }
}

struct Input<'a> {
    bytes: &'a [u8],
    at: usize,
}
impl<'a> Input<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], Error> {
        let end = self.at.checked_add(n).ok_or(Error::Length)?;
        let bytes = self.bytes.get(self.at..end).ok_or(Error::Truncated)?;
        self.at = end;
        Ok(bytes)
    }
    fn number(&mut self, n: usize) -> Result<u128, Error> {
        let mut bytes = [0; 16];
        bytes[..n].copy_from_slice(self.take(n)?);
        Ok(u128::from_le_bytes(bytes))
    }
    fn length(&mut self) -> Result<usize, Error> {
        usize::try_from(self.number(8)?).map_err(|_| Error::Length)
    }
}
fn integer(p: Primitive) -> Option<(usize, bool)> {
    use Primitive::*;
    Some(match p {
        U8 => (1, false),
        U16 => (2, false),
        U32 => (4, false),
        U64 => (8, false),
        U128 => (16, false),
        I8 => (1, true),
        I16 => (2, true),
        I32 => (4, true),
        I64 => (8, true),
        I128 => (16, true),
        _ => return None,
    })
}
fn nan32(bits: u32) -> bool {
    bits & 0x7fff_ffff > 0x7f80_0000
}
fn nan64(bits: u64) -> bool {
    bits & 0x7fff_ffff_ffff_ffff > 0x7ff0_0000_0000_0000
}
fn encode_scalar(
    p: Primitive,
    v: &ValueNode,
    out: &mut Vec<u8>,
    budget: &Budget,
) -> Result<(), Error> {
    if let Some((width, signed)) = integer(p) {
        let bits = match (signed, v) {
            (false, ValueNode::Unsigned(n)) if width == 16 || *n < 1u128 << (width * 8) => *n,
            (true, ValueNode::Signed(n))
                if width == 16
                    || (*n >= -(1i128 << (width * 8 - 1)) && *n < 1i128 << (width * 8 - 1)) =>
            {
                *n as u128
            }
            _ => return Err(Error::Scalar),
        };
        return put(out, &bits.to_le_bytes()[..width], budget);
    }
    match (p, v) {
        (Primitive::Bool, ValueNode::Bool(b)) => put(out, &[u8::from(*b)], budget),
        (Primitive::F32, ValueNode::F32(b)) => put(
            out,
            &(if nan32(*b) { 0x7fc0_0000 } else { *b }).to_le_bytes(),
            budget,
        ),
        (Primitive::F64, ValueNode::F64(b)) => put(
            out,
            &(if nan64(*b) { 0x7ff8_0000_0000_0000 } else { *b }).to_le_bytes(),
            budget,
        ),
        (Primitive::String, ValueNode::String(s)) => {
            count(out, s.len(), budget)?;
            put(out, s.as_bytes(), budget)
        }
        (Primitive::Bytes, ValueNode::Bytes(b)) => {
            count(out, b.len(), budget)?;
            put(out, b, budget)
        }
        _ => Err(Error::Value),
    }
}
fn decode_scalar(
    p: Primitive,
    input: &mut Input<'_>,
    materialize: bool,
) -> Result<ValueNode, Error> {
    if let Some((width, signed)) = integer(p) {
        let n = input.number(width)?;
        return Ok(if signed {
            ValueNode::Signed(if width == 16 {
                n as i128
            } else {
                ((n << (128 - width * 8)) as i128) >> (128 - width * 8)
            })
        } else {
            ValueNode::Unsigned(n)
        });
    }
    Ok(match p {
        Primitive::Bool => match input.number(1)? {
            0 => ValueNode::Bool(false),
            1 => ValueNode::Bool(true),
            _ => return Err(Error::Scalar),
        },
        Primitive::F32 => {
            let b = input.number(4)? as u32;
            if nan32(b) && b != 0x7fc0_0000 {
                return Err(Error::Scalar);
            }
            ValueNode::F32(b)
        }
        Primitive::F64 => {
            let b = input.number(8)? as u64;
            if nan64(b) && b != 0x7ff8_0000_0000_0000 {
                return Err(Error::Scalar);
            }
            ValueNode::F64(b)
        }
        Primitive::String => {
            let n = input.length()?;
            let s = std::str::from_utf8(input.take(n)?).map_err(|_| Error::Utf8)?;
            ValueNode::String(if materialize {
                s.to_owned()
            } else {
                String::new()
            })
        }
        Primitive::Bytes => {
            let n = input.length()?;
            let bytes = input.take(n)?;
            ValueNode::Bytes(if materialize { bytes.to_vec() } else { vec![] })
        }
        _ => return Err(Error::Schema),
    })
}

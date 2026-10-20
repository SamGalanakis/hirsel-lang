//! Codec data without a program: a value tree shaped like a schema's structure, converted
//! to and from the codec's engine-neutral [`wire::Value`] by walking a canonical schema.
//! Nominal, box and packed boundaries are the schema's business, so frame code writes
//! records, variants and lists and never a wrapper.
use hirsel_types::Primitive;
use hirsel_types::serialization::{SchemaArgument, SchemaId, SchemaNode, SerializableSchema};
use hirsel_types::wire::{self, Codec, ValueId, ValueNode};

/// A value of a serializable schema with its wrappers erased.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Data {
    Bool(bool),
    Unsigned(u128),
    Text(String),
    Bytes(Vec<u8>),
    /// A record's or tuple's fields in declaration order.
    Record(Vec<Data>),
    /// A variant's ordinal and payload.
    Variant(u32, Vec<Data>),
    List(Vec<Data>),
}

/// Why data and a schema disagree, or the codec refused.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DataError {
    Shape(&'static str),
    Codec(wire::Error),
}

/// One schema compiled for encoding and decoding [`Data`].
pub struct DataCodec {
    schema: SerializableSchema,
    codec: Codec,
}
impl DataCodec {
    pub fn new(schema: &SerializableSchema) -> Result<Self, DataError> {
        let schema = schema
            .clone()
            .canonicalized()
            .ok_or(DataError::Shape("a schema that does not canonicalize"))?;
        let codec = Codec::new(&schema).map_err(DataError::Codec)?;
        Ok(Self { schema, codec })
    }
    /// `data` as one canonical codec envelope.
    pub fn encode(&self, data: &Data, limits: wire::Limits) -> Result<Vec<u8>, DataError> {
        let value = to_wire(&self.schema, data)?;
        self.codec.encode(&value, limits).map_err(DataError::Codec)
    }
    /// The data one canonical codec envelope holds.
    pub fn decode(&self, bytes: &[u8], limits: wire::Limits) -> Result<Data, DataError> {
        let value = self.codec.decode(bytes, limits).map_err(DataError::Codec)?;
        from_wire(&self.schema, self.schema.root, &value, value.root)
    }
}

fn node(schema: &SerializableSchema, id: SchemaId) -> Result<&SchemaNode, DataError> {
    let mut id = id;
    // A transparent reference names the node it stands for.
    for _ in 0..=schema.nodes.len() {
        match schema.nodes.get(id.0 as usize) {
            Some(SchemaNode::Reference(to)) => id = *to,
            Some(node) => return Ok(node),
            None => return Err(DataError::Shape("schema node")),
        }
    }
    Err(DataError::Shape("a reference cycle"))
}
fn element(arguments: &[SchemaArgument]) -> Result<SchemaId, DataError> {
    match arguments.first() {
        Some(SchemaArgument::Type(t)) => Ok(*t),
        _ => Err(DataError::Shape("a collection's element")),
    }
}

fn to_wire(schema: &SerializableSchema, root: &Data) -> Result<wire::Value, DataError> {
    let mut nodes = vec![ValueNode::Fields(vec![])];
    let mut pending = vec![(schema.root, root, ValueId(0))];
    while let Some((ty, data, id)) = pending.pop() {
        let mut children: Vec<(SchemaId, &Data)> = vec![];
        let mut value = match (node(schema, ty)?, data) {
            (SchemaNode::Scalar(Primitive::Bool), Data::Bool(b)) => ValueNode::Bool(*b),
            (
                SchemaNode::Scalar(
                    Primitive::U8
                    | Primitive::U16
                    | Primitive::U32
                    | Primitive::U64
                    | Primitive::U128,
                ),
                Data::Unsigned(n),
            ) => ValueNode::Unsigned(*n),
            (SchemaNode::Scalar(Primitive::String), Data::Text(t)) => ValueNode::String(t.clone()),
            (SchemaNode::Scalar(Primitive::Bytes), Data::Bytes(b)) => ValueNode::Bytes(b.clone()),
            (SchemaNode::Tuple(parts), Data::Record(fields)) if parts.len() == fields.len() => {
                children.extend(parts.iter().copied().zip(fields));
                ValueNode::Fields(vec![])
            }
            (SchemaNode::Record(parts), Data::Record(fields)) if parts.len() == fields.len() => {
                children.extend(parts.iter().map(|p| p.1).zip(fields));
                ValueNode::Fields(vec![])
            }
            (SchemaNode::Variant(arms), Data::Variant(ordinal, fields)) => {
                let (_, parts) = arms
                    .get(*ordinal as usize)
                    .ok_or(DataError::Shape("variant ordinal"))?;
                if parts.len() != fields.len() {
                    return Err(DataError::Shape("variant arity"));
                }
                children.extend(parts.iter().map(|p| p.1).zip(fields));
                ValueNode::Variant {
                    ordinal: *ordinal,
                    fields: vec![],
                }
            }
            (SchemaNode::Nominal { representation, .. }, _)
            | (SchemaNode::Box(representation), _) => {
                children.push((*representation, data));
                ValueNode::Wrapper(ValueId(0))
            }
            (
                SchemaNode::Builtin {
                    kind: hirsel_types::BuiltinType::Packed,
                    arguments,
                },
                _,
            ) => {
                children.push((element(arguments)?, data));
                ValueNode::Wrapper(ValueId(0))
            }
            (
                SchemaNode::Builtin {
                    kind: hirsel_types::BuiltinType::List,
                    arguments,
                },
                Data::List(items),
            ) => {
                let t = element(arguments)?;
                children.extend(items.iter().map(|item| (t, item)));
                ValueNode::Sequence(vec![])
            }
            _ => return Err(DataError::Shape("data does not fit its schema")),
        };
        let ids: Vec<_> = children
            .into_iter()
            .map(|(t, d)| {
                let child = ValueId(nodes.len());
                nodes.push(ValueNode::Fields(vec![]));
                pending.push((t, d, child));
                child
            })
            .collect();
        match &mut value {
            ValueNode::Fields(fs)
            | ValueNode::Sequence(fs)
            | ValueNode::Variant { fields: fs, .. } => *fs = ids,
            ValueNode::Wrapper(t) => *t = ids[0],
            _ => (),
        }
        nodes[id.0] = value;
    }
    Ok(wire::Value {
        root: ValueId(0),
        nodes,
    })
}

fn from_wire(
    schema: &SerializableSchema,
    ty: SchemaId,
    value: &wire::Value,
    id: ValueId,
) -> Result<Data, DataError> {
    let at = |id: ValueId| value.nodes.get(id.0).ok_or(DataError::Shape("value node"));
    let all = |types: &mut dyn Iterator<Item = SchemaId>, ids: &[ValueId]| {
        ids.iter()
            .zip(types)
            .map(|(id, t)| from_wire(schema, t, value, *id))
            .collect::<Result<Vec<_>, _>>()
    };
    Ok(match (node(schema, ty)?, at(id)?) {
        (SchemaNode::Scalar(_), ValueNode::Bool(b)) => Data::Bool(*b),
        (SchemaNode::Scalar(_), ValueNode::Unsigned(n)) => Data::Unsigned(*n),
        (SchemaNode::Scalar(_), ValueNode::String(t)) => Data::Text(t.clone()),
        (SchemaNode::Scalar(_), ValueNode::Bytes(b)) => Data::Bytes(b.clone()),
        (SchemaNode::Tuple(parts), ValueNode::Fields(ids)) if parts.len() == ids.len() => {
            Data::Record(all(&mut parts.iter().copied(), ids)?)
        }
        (SchemaNode::Record(parts), ValueNode::Fields(ids)) if parts.len() == ids.len() => {
            Data::Record(all(&mut parts.iter().map(|p| p.1), ids)?)
        }
        (SchemaNode::Variant(arms), ValueNode::Variant { ordinal, fields }) => {
            let (_, parts) = arms
                .get(*ordinal as usize)
                .ok_or(DataError::Shape("variant ordinal"))?;
            if parts.len() != fields.len() {
                return Err(DataError::Shape("variant arity"));
            }
            Data::Variant(*ordinal, all(&mut parts.iter().map(|p| p.1), fields)?)
        }
        (SchemaNode::Nominal { representation, .. }, ValueNode::Wrapper(inner))
        | (SchemaNode::Box(representation), ValueNode::Wrapper(inner)) => {
            from_wire(schema, *representation, value, *inner)?
        }
        (
            SchemaNode::Builtin {
                kind: hirsel_types::BuiltinType::Packed,
                arguments,
            },
            ValueNode::Wrapper(inner),
        ) => from_wire(schema, element(arguments)?, value, *inner)?,
        (
            SchemaNode::Builtin {
                kind: hirsel_types::BuiltinType::List,
                arguments,
            },
            ValueNode::Sequence(ids),
        ) => {
            let t = element(arguments)?;
            Data::List(all(&mut std::iter::repeat(t), ids)?)
        }
        _ => return Err(DataError::Shape("a value that does not fit its schema")),
    })
}

//! Source values cross SER-2 through the shared, schema-directed Rust codec.
use crate::{Error, Integer, OperationCall, OperationResult, Value, invalid};
use hirsel_types::serialization::{
    SchemaArgument, SchemaId, SchemaNode, SerializableSchema, serializable_schema,
};
use hirsel_types::wire::{self, Codec, ValueId, ValueNode};
use hirsel_types::{BuiltinType, GenericArgument, GenericDeclarations, Primitive};

type R<T> = Result<T, Error>;

/// A decoded checked opaque value awaiting its check (SER-1): the check and the value.
type Checked = (hirsel_syntax::DefPath, Value);

pub(super) fn call(
    library: &mut super::Library,
    name: &str,
    call: &OperationCall<'_>,
) -> R<OperationResult> {
    let [GenericArgument::Type(ty)] = call.generics else {
        return Err(invalid("codec type actual"));
    };
    let schema = serializable_schema(
        call.types,
        *ty,
        &GenericDeclarations::default(),
        &mut |path| Ok(call.runtime.serialization.get(path).cloned()),
    )
    .map_err(Error::Query)?
    .ok_or_else(|| invalid("codec requires a checked concrete serializable schema"))?;
    let codec = Codec::new(&schema).map_err(|e| invalid(format!("codec schema: {e:?}")))?;
    let [value] = &*call.arguments else {
        return Err(invalid("codec arity"));
    };
    if name == "codec.encode" {
        let value = to_wire(&schema, value)?;
        return codec
            .encode(&value, wire::Limits::default())
            .map(|bytes| OperationResult::returned(Value::Bytes(bytes)))
            .map_err(codec_error);
    }
    let Value::Bytes(bytes) = value else {
        return Err(invalid("codec.decode expects Bytes"));
    };
    match codec.decode(bytes, wire::Limits::default()) {
        Ok(value) => {
            let (value, checks) = from_wire(&schema, value, call.runtime)?;
            let checks = checks
                .into_iter()
                .map(|(check, occurrence)| Ok((check_function(&check, call.runtime)?, occurrence)))
                .collect::<R<_>>()?;
            library.check_decoded(value, checks)
        }
        Err(error @ (wire::Error::Schema | wire::Error::Value | wire::Error::Limit)) => {
            Err(codec_error(error))
        }
        Err(error) => Ok(OperationResult::returned(decode_error(error_ordinal(
            error,
        )?))),
    }
}

/// The checked serializable schema of `ty` at `call`.
pub(super) fn schema(call: &OperationCall<'_>, ty: hirsel_types::TypeId) -> R<SerializableSchema> {
    serializable_schema(
        call.types,
        ty,
        &GenericDeclarations::default(),
        &mut |path| Ok(call.runtime.serialization.get(path).cloned()),
    )
    .map_err(Error::Query)?
    .ok_or_else(|| invalid("a runner call requires checked concrete serializable schemas"))
}
/// `value` in canonical codec bytes, or `Limit` when the codec's budgets refuse it.
pub(super) fn encode(
    schema: &SerializableSchema,
    value: &Value,
) -> R<Result<Vec<u8>, wire::Error>> {
    let codec = Codec::new(schema).map_err(|e| invalid(format!("codec schema: {e:?}")))?;
    match codec.encode(&to_wire(schema, value)?, wire::Limits::default()) {
        Ok(bytes) => Ok(Ok(bytes)),
        Err(wire::Error::Limit) => Ok(Err(wire::Error::Limit)),
        Err(error) => Err(codec_error(error)),
    }
}
/// The fresh value canonical bytes of `schema` hold.
pub(super) fn decode(
    schema: &SerializableSchema,
    bytes: &[u8],
    runtime: &crate::aggregate::RuntimeTypes,
) -> R<Value> {
    let codec = Codec::new(schema).map_err(|e| invalid(format!("codec schema: {e:?}")))?;
    let value = codec
        .decode(bytes, wire::Limits::default())
        .map_err(codec_error)?;
    // A runner call's bytes encode a live value of this run, not outside input, so
    // they hold no occurrence a check (SER-1) has not already seen constructed.
    Ok(from_wire(schema, value, runtime)?.0)
}
/// `Error(e)` of a decode, for the `DecodeError` ordinal `e`.
pub(super) fn decode_error(ordinal: u32) -> Value {
    super::error(Value::Variant {
        variant: ordinal,
        fields: vec![],
    })
}
/// `DecodeError.Invalid`: a check rejected a decoded representation.
pub(super) const INVALID: u32 = 10;

/// The check `path` as a callable value, from its declared header.
fn check_function(
    path: &hirsel_syntax::DefPath,
    runtime: &crate::aggregate::RuntimeTypes,
) -> R<Value> {
    let signature = runtime
        .serialization
        .get(path)
        .ok_or_else(|| invalid("codec check without its signature"))?;
    let hirsel_types::SignatureKind::Function(f) = &signature.kind else {
        return Err(invalid("codec check that is not a function"));
    };
    let mut types = signature.types.clone();
    let ty = types.push(hirsel_types::Type::with_kind(
        hirsel_types::TypeKind::Function(f.clone()),
    ));
    Ok(Value::FunctionReference(Box::new(
        crate::value::NamedFunction {
            definition: path.clone(),
            arguments: vec![],
            types: std::sync::Arc::new(types),
            ty,
        },
    )))
}

fn codec_error(error: wire::Error) -> Error {
    if error == wire::Error::Limit {
        Error::Environmental(hirsel_run_schema::Environmental::CodecLimit)
    } else {
        invalid(format!("invalid checked codec value: {error:?}"))
    }
}
fn error_ordinal(error: wire::Error) -> R<u32> {
    Ok(match error {
        wire::Error::Version => 0,
        wire::Error::SchemaMismatch => 1,
        wire::Error::Truncated => 2,
        wire::Error::Trailing => 3,
        wire::Error::Scalar => 4,
        wire::Error::Utf8 => 5,
        wire::Error::Tag => 6,
        wire::Error::Length => 7,
        wire::Error::Duplicate => 8,
        wire::Error::Order => 9,
        other => return Err(codec_error(other)),
    })
}
fn fields(value: &Value) -> R<Vec<&Value>> {
    if matches!(value, Value::Unit) {
        return Ok(vec![]);
    }
    value
        .fields()
        .ok_or_else(|| invalid("codec aggregate"))?
        .iter()
        .map(|v| v.as_ref().ok_or_else(|| invalid("codec moved field")))
        .collect()
}
fn element(arguments: &[SchemaArgument], ordinal: usize) -> R<SchemaId> {
    match arguments.get(ordinal) {
        Some(SchemaArgument::Type(t)) => Ok(*t),
        _ => Err(invalid("codec schema argument")),
    }
}

fn to_wire(schema: &SerializableSchema, root: &Value) -> R<wire::Value> {
    let mut nodes = vec![ValueNode::Fields(vec![])];
    let mut pending = vec![(schema.root, root, ValueId(0))];
    while let Some((ty, value, id)) = pending.pop() {
        let mut children: Vec<(SchemaId, &Value)> = vec![];
        let mut node = match (&schema.nodes[ty.0 as usize], value) {
            (SchemaNode::Scalar(Primitive::Bool), Value::Bool(v)) => ValueNode::Bool(*v),
            (SchemaNode::Scalar(Primitive::Char), Value::Char(v)) => {
                ValueNode::Unsigned(u128::from(u32::from(*v)))
            }
            (SchemaNode::Scalar(_), Value::Integer(v)) if v.is_signed() => {
                ValueNode::Signed(v.signed())
            }
            (SchemaNode::Scalar(_), Value::Integer(v)) => ValueNode::Unsigned(v.bits()),
            (SchemaNode::Scalar(Primitive::F32), Value::F32(v)) => ValueNode::F32(*v),
            (SchemaNode::Scalar(Primitive::F64), Value::F64(v)) => ValueNode::F64(*v),
            (SchemaNode::Scalar(Primitive::String), Value::String(v)) => {
                ValueNode::String(v.clone())
            }
            (SchemaNode::Scalar(Primitive::Bytes), Value::Bytes(v)) => ValueNode::Bytes(v.clone()),
            (SchemaNode::Tuple(ts), _) => {
                let values = fields(value)?;
                if values.len() != ts.len() {
                    return Err(invalid("codec tuple arity"));
                }
                children.extend(ts.iter().copied().zip(values));
                ValueNode::Fields(vec![])
            }
            (SchemaNode::Record(fs), _) => {
                let values = fields(value)?;
                if values.len() != fs.len() {
                    return Err(invalid("codec record arity"));
                }
                children.extend(fs.iter().map(|f| f.1).zip(values));
                ValueNode::Fields(vec![])
            }
            (SchemaNode::Variant(arms), Value::Variant { variant, .. }) => {
                let (_, fs) = arms
                    .get(*variant as usize)
                    .ok_or_else(|| invalid("codec variant ordinal"))?;
                let values = fields(value)?;
                if values.len() != fs.len() {
                    return Err(invalid("codec variant arity"));
                }
                children.extend(fs.iter().map(|f| f.1).zip(values));
                ValueNode::Variant {
                    ordinal: *variant,
                    fields: vec![],
                }
            }
            (
                SchemaNode::Nominal {
                    definition,
                    representation,
                    ..
                },
                _,
            ) if definition.is_core("Path") => {
                let Value::Record { fields, .. } = value else {
                    return Err(invalid("codec Path value"));
                };
                let [Some(text)] = fields.as_slice() else {
                    return Err(invalid("codec Path payload"));
                };
                children.push((*representation, text));
                ValueNode::Wrapper(ValueId(0))
            }
            (SchemaNode::Nominal { representation, .. }, _) => {
                let inner = if let Value::Distinct { fields, .. } = value
                    && !matches!(
                        schema.nodes[representation.0 as usize],
                        SchemaNode::Record(_) | SchemaNode::Variant(_)
                    ) {
                    if fields.len() != 1 {
                        return Err(invalid("codec distinct payload"));
                    }
                    fields[0]
                        .as_ref()
                        .ok_or_else(|| invalid("codec distinct moved"))?
                } else {
                    value
                };
                children.push((*representation, inner));
                ValueNode::Wrapper(ValueId(0))
            }
            (SchemaNode::Box(t), Value::Box(_)) => {
                let values = fields(value)?;
                if values.len() != 1 {
                    return Err(invalid("codec box arity"));
                }
                children.push((*t, values[0]));
                ValueNode::Wrapper(ValueId(0))
            }
            (
                SchemaNode::Builtin {
                    kind: BuiltinType::Packed,
                    arguments,
                },
                _,
            ) => {
                children.push((element(arguments, 0)?, value));
                ValueNode::Wrapper(ValueId(0))
            }
            (
                SchemaNode::Builtin {
                    kind: BuiltinType::Map,
                    arguments,
                },
                Value::Map { .. },
            ) => {
                let key = element(arguments, 0)?;
                let val = element(arguments, 1)?;
                for entry in fields(value)? {
                    let pair = fields(entry)?;
                    if pair.len() != 2 {
                        return Err(invalid("codec map entry"));
                    }
                    children.extend([(key, pair[0]), (val, pair[1])]);
                }
                ValueNode::Map(vec![])
            }
            (
                SchemaNode::Builtin {
                    kind: BuiltinType::Set,
                    arguments,
                },
                Value::Set { .. },
            ) => {
                children.extend(
                    fields(value)?
                        .into_iter()
                        .map(|v| Ok((element(arguments, 0)?, v)))
                        .collect::<R<Vec<_>>>()?,
                );
                ValueNode::Set(vec![])
            }
            (SchemaNode::Builtin { kind, arguments }, Value::Sequence { kind: actual, .. })
                if kind == actual =>
            {
                let t = element(arguments, 0)?;
                children.extend(fields(value)?.into_iter().map(|v| (t, v)));
                ValueNode::Sequence(vec![])
            }
            _ => return Err(invalid("codec schema/value mismatch")),
        };
        let ids: Vec<_> = children
            .into_iter()
            .map(|(t, v)| {
                let child = ValueId(nodes.len());
                nodes.push(ValueNode::Fields(vec![]));
                pending.push((t, v, child));
                child
            })
            .collect();
        match &mut node {
            ValueNode::Fields(fs)
            | ValueNode::Sequence(fs)
            | ValueNode::Set(fs)
            | ValueNode::Variant { fields: fs, .. } => *fs = ids,
            ValueNode::Map(fs) => {
                *fs = ids
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .map(|p| (p[0], p[1]))
                    .collect()
            }
            ValueNode::Wrapper(t) => *t = ids[0],
            _ => (),
        }
        nodes[id.0] = node;
    }
    Ok(wire::Value {
        root: ValueId(0),
        nodes,
    })
}

/// The decoded value and every checked occurrence in it, an inner occurrence before the
/// one holding it and otherwise in encoding order.
fn from_wire(
    schema: &SerializableSchema,
    value: wire::Value,
    runtime: &crate::aggregate::RuntimeTypes,
) -> R<(Value, Vec<Checked>)> {
    enum Job {
        Visit(SchemaId, ValueId),
        Finish(SchemaId, usize, Option<u32>),
    }
    let mut pending = vec![Job::Visit(schema.root, value.root)];
    let mut values: Vec<Value> = vec![];
    let mut checks = vec![];
    while let Some(job) = pending.pop() {
        match job {
            Job::Finish(ty, n, ordinal) => {
                let mut fs: Vec<_> = values
                    .split_off(values.len() - n)
                    .into_iter()
                    .map(Some)
                    .collect();
                let node = match &schema.nodes[ty.0 as usize] {
                    SchemaNode::Tuple(_) | SchemaNode::Record(_) => {
                        if fs.is_empty() {
                            Value::Unit
                        } else {
                            Value::Tuple(fs)
                        }
                    }
                    SchemaNode::Variant(_) => Value::Variant {
                        variant: ordinal.unwrap(),
                        fields: fs,
                    },
                    SchemaNode::Box(_) => Value::Box(fs),
                    SchemaNode::Nominal {
                        definition,
                        representation,
                        check,
                    } => {
                        let child = fs
                            .pop()
                            .flatten()
                            .ok_or_else(|| invalid("codec nominal payload"))?;
                        let value = if definition.is_core("Path") {
                            let Value::String(text) = child else {
                                return Err(invalid("codec Path text"));
                            };
                            super::dispatch::path(&text)
                        } else if runtime.get(definition).is_some_and(|n| n.distinct) {
                            let fields = if matches!(
                                schema.nodes[representation.0 as usize],
                                SchemaNode::Record(_)
                            ) {
                                match child {
                                    Value::Tuple(fields) => fields,
                                    Value::Unit => vec![],
                                    _ => return Err(invalid("codec distinct record")),
                                }
                            } else {
                                vec![Some(child)]
                            };
                            Value::Distinct {
                                definition: definition.into(),
                                fields,
                            }
                        } else {
                            match child {
                                Value::Tuple(fields) => Value::Record {
                                    definition: definition.into(),
                                    fields,
                                },
                                Value::Unit => Value::Record {
                                    definition: definition.into(),
                                    fields: vec![],
                                },
                                other => other,
                            }
                        };
                        if let Some(check) = check {
                            checks.push((check.clone(), value.clone()));
                        }
                        value
                    }
                    SchemaNode::Builtin {
                        kind: BuiltinType::Packed,
                        ..
                    } => fs.pop().flatten().unwrap(),
                    SchemaNode::Builtin {
                        kind: BuiltinType::Map,
                        ..
                    } => Value::map(
                        fs.chunks_mut(2)
                            .map(|p| Some(Value::Tuple(p.iter_mut().map(Option::take).collect())))
                            .collect(),
                    ),
                    SchemaNode::Builtin {
                        kind: BuiltinType::Set,
                        ..
                    } => Value::set(fs),
                    SchemaNode::Builtin { kind, .. } => Value::Sequence {
                        kind: *kind,
                        elements: fs.into(),
                        arena: None,
                    },
                    _ => return Err(invalid("codec finish schema")),
                };
                values.push(node);
            }
            Job::Visit(ty, id) => {
                let node = &value.nodes[id.0];
                let shape = &schema.nodes[ty.0 as usize];
                let mut children = vec![];
                let mut ordinal = None;
                match (shape, node) {
                    (SchemaNode::Scalar(p), node) => {
                        let scalar = match node {
                            ValueNode::Bool(v) => Value::Bool(*v),
                            // The reference decoder admitted only scalar values.
                            ValueNode::Unsigned(v) if *p == Primitive::Char => Value::Char(
                                u32::try_from(*v)
                                    .ok()
                                    .and_then(char::from_u32)
                                    .ok_or_else(|| invalid("codec char node"))?,
                            ),
                            ValueNode::Unsigned(v) => Value::Integer(Integer::new(*p, *v).unwrap()),
                            ValueNode::Signed(v) => {
                                let width = crate::value::integer_layout(*p).unwrap().0;
                                Value::Integer(
                                    Integer::new(*p, *v as u128 & crate::value::mask(width))
                                        .unwrap(),
                                )
                            }
                            ValueNode::F32(v) => crate::value::f32_value(f32::from_bits(*v)),
                            ValueNode::F64(v) => crate::value::f64_value(f64::from_bits(*v)),
                            ValueNode::String(v) => Value::String(v.clone()),
                            ValueNode::Bytes(v) => Value::Bytes(v.clone()),
                            _ => return Err(invalid("codec scalar node")),
                        };
                        values.push(scalar);
                        continue;
                    }
                    (SchemaNode::Tuple(ts), ValueNode::Fields(fs)) => {
                        children.extend(ts.iter().copied().zip(fs.iter().copied()))
                    }
                    (SchemaNode::Record(ts), ValueNode::Fields(fs)) => {
                        children.extend(ts.iter().map(|f| f.1).zip(fs.iter().copied()))
                    }
                    (
                        SchemaNode::Variant(arms),
                        ValueNode::Variant {
                            ordinal: arm,
                            fields,
                        },
                    ) => {
                        ordinal = Some(*arm);
                        children.extend(
                            arms[*arm as usize]
                                .1
                                .iter()
                                .map(|f| f.1)
                                .zip(fields.iter().copied()),
                        );
                    }
                    (SchemaNode::Nominal { representation, .. }, ValueNode::Wrapper(t))
                    | (SchemaNode::Box(representation), ValueNode::Wrapper(t)) => {
                        children.push((*representation, *t))
                    }
                    (
                        SchemaNode::Builtin {
                            kind: BuiltinType::Packed,
                            arguments,
                        },
                        ValueNode::Wrapper(t),
                    ) => children.push((element(arguments, 0)?, *t)),
                    (
                        SchemaNode::Builtin {
                            kind: BuiltinType::Map,
                            arguments,
                        },
                        ValueNode::Map(entries),
                    ) => {
                        for (k, v) in entries {
                            children.extend([
                                (element(arguments, 0)?, *k),
                                (element(arguments, 1)?, *v),
                            ]);
                        }
                    }
                    (
                        SchemaNode::Builtin { arguments, .. },
                        ValueNode::Sequence(fs) | ValueNode::Set(fs),
                    ) => children.extend(
                        fs.iter()
                            .map(|t| Ok((element(arguments, 0)?, *t)))
                            .collect::<R<Vec<_>>>()?,
                    ),
                    _ => return Err(invalid("codec decoded shape")),
                }
                pending.push(Job::Finish(ty, children.len(), ordinal));
                pending.extend(children.into_iter().rev().map(|(t, v)| Job::Visit(t, v)));
            }
        }
    }
    let value = values.pop().ok_or_else(|| invalid("codec empty value"))?;
    Ok((value, checks))
}

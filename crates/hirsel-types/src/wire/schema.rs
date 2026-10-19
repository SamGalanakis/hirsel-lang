use super::*;

impl Codec {
    pub fn new(schema: &SerializableSchema) -> Result<Self, Error> {
        if schema.version != 1
            || schema.root.0 as usize >= schema.nodes.len()
            || schema.nodes.len() > u32::MAX as usize
        {
            return Err(Error::Schema);
        }
        // Public descriptors are forgeable. Check all references before either
        // canonicalization or the executable plan reads their indexed children.
        for node in &schema.nodes {
            for child in edges(node) {
                if child.0 as usize >= schema.nodes.len() {
                    return Err(Error::Schema);
                }
            }
        }
        let schema = schema.clone().canonicalized().ok_or(Error::Schema)?;
        let schema = &schema;
        let mut nodes = Vec::with_capacity(schema.nodes.len());
        for node in &schema.nodes {
            let t = match node {
                SchemaNode::Scalar(Primitive::Never)
                | SchemaNode::Parameter(_)
                | SchemaNode::Reference(_) => return Err(Error::Schema),
                SchemaNode::Scalar(p) => Node::Scalar(*p),
                SchemaNode::Tuple(parts) => {
                    Node::Fields(parts.iter().map(|t| t.0 as usize).collect())
                }
                SchemaNode::Record(fields) => {
                    unique(fields.iter().map(|f| f.0.as_str()))?;
                    Node::Fields(fields.iter().map(|f| f.1.0 as usize).collect())
                }
                SchemaNode::Variant(arms) => {
                    unique(arms.iter().map(|a| a.0.as_str()))?;
                    for (_, fs) in arms {
                        unique(fs.iter().filter_map(|f| f.0.as_deref()))?;
                    }
                    Node::Variant(
                        arms.iter()
                            .map(|a| a.1.iter().map(|f| f.1.0 as usize).collect())
                            .collect(),
                    )
                }
                SchemaNode::Box(t)
                | SchemaNode::Nominal {
                    representation: t, ..
                } => Node::Wrapper(t.0 as usize),
                SchemaNode::Builtin { kind, arguments } => {
                    use BuiltinType::*;
                    use SchemaArgument::{Size as N, Type};
                    match (kind, arguments.as_slice()) {
                        (Array | List, [Type(t)]) => Node::Sequence {
                            element: t.0 as usize,
                            count: None,
                        },
                        (Inline | Simd, [Type(t), N(Size::Constant(n))]) => {
                            if *kind == Simd && (*n == 0 || !is_scalar(schema, *t)) {
                                return Err(Error::Schema);
                            }
                            Node::Sequence {
                                element: t.0 as usize,
                                count: Some(*n),
                            }
                        }
                        (Packed, [Type(t)]) if is_variant(schema, *t) => {
                            Node::Wrapper(t.0 as usize)
                        }
                        (Map, [Type(k), Type(v)]) => {
                            key_tree(schema, *k)?;
                            Node::Map {
                                key: k.0 as usize,
                                value: v.0 as usize,
                            }
                        }
                        (Set, [Type(t)]) => {
                            key_tree(schema, *t)?;
                            Node::Set(t.0 as usize)
                        }
                        _ => return Err(Error::Schema),
                    }
                }
            };
            nodes.push(t);
        }
        Ok(Self {
            nodes,
            root: schema.root.0 as usize,
            descriptor: descriptor(schema)?,
            fixed_flat: fixed_flat(schema),
        })
    }
}
fn unique<'a>(names: impl Iterator<Item = &'a str>) -> Result<(), Error> {
    let mut seen = BTreeSet::new();
    for name in names {
        if !seen.insert(name) {
            return Err(Error::Schema);
        }
    }
    Ok(())
}
fn edges(node: &SchemaNode) -> Vec<SchemaId> {
    match node {
        SchemaNode::Scalar(_) | SchemaNode::Parameter(_) => vec![],
        SchemaNode::Tuple(ts) => ts.clone(),
        SchemaNode::Record(fs) => fs.iter().map(|f| f.1).collect(),
        SchemaNode::Variant(vs) => vs.iter().flat_map(|v| v.1.iter().map(|f| f.1)).collect(),
        SchemaNode::Box(t)
        | SchemaNode::Nominal {
            representation: t, ..
        }
        | SchemaNode::Reference(t) => vec![*t],
        SchemaNode::Builtin { arguments, .. } => arguments
            .iter()
            .filter_map(|a| {
                if let SchemaArgument::Type(t) = a {
                    Some(*t)
                } else {
                    None
                }
            })
            .collect(),
    }
}
fn unwrap(schema: &SerializableSchema, mut t: SchemaId) -> Option<&SchemaNode> {
    let mut seen = BTreeSet::new();
    while seen.insert(t.0) {
        match &schema.nodes[t.0 as usize] {
            SchemaNode::Nominal { representation, .. } => t = *representation,
            node => return Some(node),
        }
    }
    None
}
fn is_variant(schema: &SerializableSchema, t: SchemaId) -> bool {
    matches!(unwrap(schema, t), Some(SchemaNode::Variant(_)))
}
fn is_scalar(schema: &SerializableSchema, t: SchemaId) -> bool {
    matches!(unwrap(schema,t),Some(SchemaNode::Scalar(p)) if integer(*p).is_some() || matches!(p,Primitive::Bool | Primitive::F32 | Primitive::F64))
}
fn key_tree(schema: &SerializableSchema, root: SchemaId) -> Result<(), Error> {
    let mut seen = BTreeSet::new();
    let mut work = vec![root];
    while let Some(t) = work.pop() {
        if !seen.insert(t.0) {
            continue;
        }
        if matches!(
            schema.nodes[t.0 as usize],
            SchemaNode::Scalar(Primitive::F32 | Primitive::F64 | Primitive::Never)
                | SchemaNode::Parameter(_)
        ) {
            return Err(Error::Schema);
        }
        work.extend(edges(&schema.nodes[t.0 as usize]));
    }
    Ok(())
}
fn scalar_tag(p: Primitive) -> Result<u8, Error> {
    use Primitive::*;
    Ok(match p {
        Bool => 0,
        U8 => 1,
        U16 => 2,
        U32 => 3,
        U64 => 4,
        U128 => 5,
        I8 => 6,
        I16 => 7,
        I32 => 8,
        I64 => 9,
        I128 => 10,
        F32 => 11,
        F64 => 12,
        String => 13,
        Bytes => 14,
        Char => 15,
        Never => return Err(Error::Schema),
    })
}
fn builtin_tag(p: BuiltinType) -> Result<u8, Error> {
    use BuiltinType::*;
    Ok(match p {
        Array => 0,
        List => 1,
        Inline => 2,
        Simd => 3,
        Packed => 4,
        Map => 5,
        Set => 6,
        _ => return Err(Error::Schema),
    })
}
fn text(out: &mut Vec<u8>, s: &str) {
    out.extend_from_slice(&(s.len() as u64).to_le_bytes());
    out.extend_from_slice(s.as_bytes());
}
fn number(out: &mut Vec<u8>, n: usize) -> Result<(), Error> {
    out.extend_from_slice(&u32::try_from(n).map_err(|_| Error::Schema)?.to_le_bytes());
    Ok(())
}
fn id(out: &mut Vec<u8>, t: SchemaId) {
    out.extend_from_slice(&t.0.to_le_bytes());
}
fn descriptor(schema: &SerializableSchema) -> Result<Vec<u8>, Error> {
    let mut out = vec![];
    out.extend_from_slice(&schema.version.to_le_bytes());
    id(&mut out, schema.root);
    number(&mut out, schema.nodes.len())?;
    for node in &schema.nodes {
        match node {
            SchemaNode::Scalar(p) => {
                out.extend_from_slice(&[0, scalar_tag(*p)?]);
            }
            SchemaNode::Tuple(ts) => {
                out.push(1);
                number(&mut out, ts.len())?;
                for t in ts {
                    id(&mut out, *t);
                }
            }
            SchemaNode::Record(fs) => {
                out.push(2);
                number(&mut out, fs.len())?;
                for (name, t) in fs {
                    text(&mut out, name);
                    id(&mut out, *t);
                }
            }
            SchemaNode::Variant(vs) => {
                out.push(3);
                number(&mut out, vs.len())?;
                for (name, fs) in vs {
                    text(&mut out, name);
                    number(&mut out, fs.len())?;
                    for (name, t) in fs {
                        out.push(u8::from(name.is_some()));
                        if let Some(name) = name {
                            text(&mut out, name);
                        }
                        id(&mut out, *t);
                    }
                }
            }
            SchemaNode::Box(t) => {
                out.push(4);
                id(&mut out, *t);
            }
            SchemaNode::Builtin { kind, arguments } => {
                out.extend_from_slice(&[5, builtin_tag(*kind)?]);
                number(&mut out, arguments.len())?;
                for a in arguments {
                    match a {
                        SchemaArgument::Type(t) => {
                            out.push(0);
                            id(&mut out, *t);
                        }
                        SchemaArgument::Size(Size::Constant(n)) => {
                            out.push(1);
                            out.extend_from_slice(&n.to_le_bytes());
                        }
                        _ => return Err(Error::Schema),
                    }
                }
            }
            SchemaNode::Nominal {
                definition,
                representation,
                ..
            } => {
                out.push(6);
                text(&mut out, &definition.0.package.name);
                text(&mut out, &definition.0.package.version);
                number(&mut out, definition.0.modules.len())?;
                for name in &definition.0.modules {
                    text(&mut out, name);
                }
                text(&mut out, &definition.0.name);
                id(&mut out, *representation);
            }
            _ => return Err(Error::Schema),
        }
    }
    Ok(out)
}

// A least fixed point in dependency order; cycles have no fixed-byte proof.
fn fixed_flat(schema: &SerializableSchema) -> Option<usize> {
    let mut widths = vec![None; schema.nodes.len()];
    let mut remaining = vec![0; schema.nodes.len()];
    let mut parents = vec![vec![]; schema.nodes.len()];
    let mut ready = vec![];
    for (i, node) in schema.nodes.iter().enumerate() {
        let children = edges(node);
        remaining[i] = children.len();
        for child in children {
            parents[child.0 as usize].push(i);
        }
        if remaining[i] == 0 {
            ready.push(i);
        }
    }
    while let Some(i) = ready.pop() {
        widths[i] = match &schema.nodes[i] {
            SchemaNode::Scalar(p) => integer(*p).map(|p| p.0).or(match p {
                Primitive::Bool => Some(1),
                Primitive::F32 => Some(4),
                Primitive::F64 => Some(8),
                _ => None,
            }),
            SchemaNode::Tuple(ts) => ts
                .iter()
                .try_fold(0usize, |n, t| n.checked_add(widths[t.0 as usize]?)),
            SchemaNode::Record(fs) => fs
                .iter()
                .try_fold(0usize, |n, f| n.checked_add(widths[f.1.0 as usize]?)),
            SchemaNode::Nominal { representation, .. } => widths[representation.0 as usize],
            SchemaNode::Builtin {
                kind: BuiltinType::Inline,
                arguments,
            } => match arguments.as_slice() {
                [
                    SchemaArgument::Type(t),
                    SchemaArgument::Size(Size::Constant(n)),
                ] => widths[t.0 as usize].and_then(|w| w.checked_mul(usize::try_from(*n).ok()?)),
                _ => None,
            },
            _ => None,
        };
        for parent in &parents[i] {
            remaining[*parent] -= 1;
            if remaining[*parent] == 0 {
                ready.push(*parent);
            }
        }
    }
    widths[schema.root.0 as usize]
}

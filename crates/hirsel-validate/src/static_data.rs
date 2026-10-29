//! Independently recover immutable literal values for materialized collection keys.
//! Interning structural nodes keeps shared constant graphs shared and compares
//! order-sensitive Map and unordered Set contents without an interpreter or producer proof flag.
use super::*;

#[derive(Clone, Eq, PartialEq, Ord, PartialOrd)]
enum Atom {
    Unit,
    Bool(bool),
    Integer(u128),
    F32(u32),
    F64(u64),
    String(String),
    Bytes(Vec<u8>),
}
#[derive(Clone, Eq, PartialEq, Ord, PartialOrd)]
enum Node {
    Atom(Atom),
    Tuple(Vec<u32>),
    Record(DefPath, Vec<u32>),
    Variant(DefPath, u32, Vec<u32>),
    Sequence(Vec<u32>),
    Box(u32),
    Map(Vec<(u32, u32)>),
    Set(Vec<u32>),
}
#[derive(Clone, Copy)]
enum Producer {
    Value(RvalueId),
    Snapshot(PlaceId),
}
pub(super) struct Values {
    nodes: Vec<Node>,
    values: Vec<Option<u32>>,
}
impl Values {
    pub fn new(body: &ReadBody<'_>) -> Self {
        let mut producers = vec![None; body.locals.len()];
        let mut written = vec![false; body.locals.len()];
        let mut invalid = vec![false; body.locals.len()];
        for block in &body.blocks {
            for instruction in &block.instructions {
                let output = match instruction.kind {
                    InstructionKind::Assign { destination, value } => {
                        Some((destination, Producer::Value(value)))
                    }
                    InstructionKind::Snapshot {
                        destination,
                        ref value,
                    } => Some((destination, Producer::Snapshot(value.place))),
                    InstructionKind::Replace {
                        destination,
                        displaced,
                        ..
                    } => {
                        invalid[body.places[destination.0 as usize].root.0 as usize] = true;
                        invalid[body.places[displaced.0 as usize].root.0 as usize] = true;
                        None
                    }
                    _ => None,
                };
                if let Some((place, producer)) = output {
                    let access = &body.places[place.0 as usize];
                    let root = access.root.0 as usize;
                    invalid[root] |= written[root] || !access.projection.is_empty();
                    written[root] = true;
                    producers[root] = Some(producer);
                }
            }
            if let Terminator::Call {
                arguments, returns, ..
            } = &block.terminator
            {
                if let Some(r) = returns {
                    invalid[body.places[r.destination.0 as usize].root.0 as usize] = true;
                }
                for argument in arguments {
                    if argument.value.usage == UseKind::Mut {
                        invalid[body.places[argument.value.place.0 as usize].root.0 as usize] =
                            true;
                    }
                }
            }
        }
        let mut result = Self {
            nodes: vec![],
            values: vec![None; body.locals.len()],
        };
        let mut intern = BTreeMap::new();
        let mut colors = vec![0; body.locals.len()];
        for root in 0..body.locals.len() {
            let mut stack = vec![(root, false)];
            while let Some((root, leaving)) = stack.pop() {
                if colors[root] == 2 {
                    continue;
                }
                if invalid[root] || producers[root].is_none() || !leaving && colors[root] == 1 {
                    colors[root] = 2;
                    continue;
                }
                let producer = producers[root].unwrap();
                if !leaving {
                    colors[root] = 1;
                    stack.push((root, true));
                    let dependencies: Vec<_> = match producer {
                        Producer::Snapshot(place) => vec![place],
                        Producer::Value(value) => match &body.rvalues[value.0 as usize] {
                            Rvalue::Use(value) => vec![value.place],
                            Rvalue::Construct { fields, .. } => {
                                fields.iter().map(|f| f.value.place).collect()
                            }
                            _ => vec![],
                        },
                    };
                    for place in dependencies.into_iter().rev() {
                        let input = body.places[place.0 as usize].root.0 as usize;
                        if colors[input] != 2 {
                            stack.push((input, false));
                        }
                    }
                    continue;
                }
                colors[root] = 2;
                let node = match producer {
                    Producer::Snapshot(place) => {
                        result.values[root] = result.place(body, place);
                        continue;
                    }
                    Producer::Value(value) => match &body.rvalues[value.0 as usize] {
                        Rvalue::Use(value) => {
                            result.values[root] = result.place(body, value.place);
                            continue;
                        }
                        Rvalue::Constant(value) => Some(Node::Atom(match value {
                            Constant::Unit => Atom::Unit,
                            Constant::Bool(v) => Atom::Bool(*v),
                            Constant::Integer(v) => Atom::Integer(*v),
                            Constant::F32(v) => Atom::F32(*v),
                            Constant::F64(v) => Atom::F64(*v),
                            Constant::String(v) => Atom::String(v.clone()),
                            Constant::Bytes(v) => Atom::Bytes(v.clone()),
                        })),
                        Rvalue::Construct {
                            constructor,
                            fields,
                            ..
                        } => {
                            let mut ordered: Vec<_> = fields.iter().collect();
                            ordered.sort_by_key(|field| field.field);
                            let children: Option<Vec<_>> = ordered
                                .iter()
                                .enumerate()
                                .map(|(ordinal, field)| {
                                    (field.field == ordinal as u32)
                                        .then(|| result.place(body, field.value.place))
                                        .flatten()
                                })
                                .collect();
                            children.and_then(|children| result.construct(constructor, children))
                        }
                        _ => None,
                    },
                };
                if let Some(node) = node {
                    let id = *intern.entry(node.clone()).or_insert_with(|| {
                        let id = result.nodes.len() as u32;
                        result.nodes.push(node);
                        id
                    });
                    result.values[root] = Some(id);
                }
            }
        }
        result
    }
    fn construct(&self, constructor: &Constructor, mut children: Vec<u32>) -> Option<Node> {
        Some(match constructor {
            Constructor::Tuple if children.is_empty() => Node::Atom(Atom::Unit),
            Constructor::Tuple => Node::Tuple(children),
            Constructor::Record(path) => Node::Record(path.clone(), children),
            Constructor::Variant {
                definition,
                variant,
            } => Node::Variant(definition.clone(), *variant, children),
            Constructor::Sequence => Node::Sequence(children),
            Constructor::Box if children.len() == 1 => Node::Box(children[0]),
            Constructor::Box => return None,
            Constructor::SortedMap { .. } => return None,
            Constructor::Map => {
                let entries = children
                    .into_iter()
                    .map(|child| match &self.nodes[child as usize] {
                        Node::Tuple(fields) if fields.len() == 2 => Some((fields[0], fields[1])),
                        _ => None,
                    })
                    .collect::<Option<Vec<_>>>()?;
                Node::Map(entries)
            }
            Constructor::Set => {
                children.sort_unstable();
                Node::Set(children)
            }
        })
    }
    fn place(&self, body: &ReadBody<'_>, place: PlaceId) -> Option<u32> {
        let access = &body.places[place.0 as usize];
        let mut value = self.values[access.root.0 as usize]?;
        for projection in &access.projection {
            value = match (projection, &self.nodes[value as usize]) {
                (Projection::Field(field), Node::Tuple(fields) | Node::Record(_, fields)) => {
                    *fields.get(*field as usize)?
                }
                (Projection::VariantField { variant, field }, Node::Variant(_, tag, fields))
                    if variant == tag =>
                {
                    *fields.get(*field as usize)?
                }
                _ => return None,
            };
        }
        Some(value)
    }
    pub fn distinct_keys(
        &self,
        body: &ReadBody<'_>,
        constructor: &Constructor,
        fields: &[ConstructField],
    ) -> bool {
        let mut seen = BTreeSet::new();
        fields.iter().all(|field| {
            let Some(mut key) = self.place(body, field.value.place) else {
                return false;
            };
            if *constructor == Constructor::Map {
                let Node::Tuple(entry) = &self.nodes[key as usize] else {
                    return false;
                };
                if entry.len() != 2 {
                    return false;
                }
                key = entry[0];
            }
            seen.insert(key)
        })
    }
}

//! Physical access reconstructed from an independently validated logical graph.
use crate::codec::{CodecNode as N, CodecNodeId as Id};
use crate::layout_types::{LayoutId, LayoutKind as K, LayoutTable};
use crate::structural::{Shape, ShapeGraph};
use hirsel_codec::snapshot_struct;
use std::collections::BTreeMap;

#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct AccessPlan {
    pub source: crate::operation::NativeType,
    pub graph: ShapeGraph,
    pub root: Id,
    pub nodes: Vec<N>,
    pub logical_nodes: Vec<u32>,
    /// Logical child-edge cost of each physical access. Representation
    /// unwrapping costs zero; a distinct payload costs one even when empty.
    pub edges: Vec<u8>,
}

snapshot_struct!(AccessPlan {
    source,
    graph,
    root,
    nodes,
    logical_nodes,
    edges
});

impl AccessPlan {
    /// Representations which the legacy layout-only value walkers cannot read.
    /// The access program supplies their logical field/element ordering.
    pub fn needs_value_program(&self, table: &LayoutTable) -> bool {
        self.nodes.iter().any(|node| {
            let Some(row) = table.layout(node.layout()) else {
                return true;
            };
            match row.kind {
                K::Flat
                | K::Packed
                | K::Sequential { .. }
                | K::SoaColumns { .. }
                | K::Shared { .. }
                | K::Frozen { .. } => true,
                K::Block { payload } => table.layout(payload).is_some_and(|row| {
                    matches!(
                        row.kind,
                        K::Flat | K::Sequential { .. } | K::SoaColumns { .. }
                    )
                }),
                _ => false,
            }
        })
    }

    pub fn root_layout(&self) -> LayoutId {
        self.nodes[self.root.index()].layout()
    }
    pub fn layouts(&self) -> impl Iterator<Item = LayoutId> + '_ {
        self.nodes.iter().flat_map(|node| match node {
            N::Text {
                layout, storage, ..
            } => vec![*layout, *storage],
            _ => vec![node.layout()],
        })
    }
    pub fn check(&self, table: &LayoutTable) -> Result<(), String> {
        self.graph.check().map_err(str::to_owned)?;
        if self.nodes.get(self.root.index()).is_none() {
            return Err("structural access root outside graph".into());
        }
        let expected = Builder::new(&self.graph, table).build(self.root_layout())?;
        if self.root != expected.0
            || self.nodes != expected.1
            || self.edges != expected.2
            || self.logical_nodes != expected.3
        {
            return Err("physical access differs from logical graph and layouts".into());
        }
        Ok(())
    }
    /// The codec's existing physical node emitter supplies offsets for Flat,
    /// Packed, sequential and SoA storage. No serialization eligibility or
    /// wire descriptor is requested or changed by this access-only program.
    pub fn program(
        &self,
        table: &LayoutTable,
        index: &dyn Fn(LayoutId) -> Option<u32>,
    ) -> Result<Vec<u64>, String> {
        let mut physical = table.clone();
        for row in &mut physical.layouts {
            if let K::Shared { payload } = row.kind {
                row.kind = K::Block { payload };
            }
        }
        crate::codec::CodecPlan {
            descriptor: vec![],
            nodes: self.nodes.clone(),
            root: self.root,
            bytes: self.root,
            result: None,
            checks: vec![],
        }
        .access_program(&physical, index)
    }
    pub fn derive(
        source: crate::operation::NativeType,
        graph: ShapeGraph,
        table: &LayoutTable,
        root: LayoutId,
    ) -> Result<Self, String> {
        let (root, nodes, edges, logical_nodes) = Builder::new(&graph, table).build(root)?;
        Ok(Self {
            source,
            graph,
            root,
            nodes,
            logical_nodes,
            edges,
        })
    }
}

struct Builder<'a> {
    graph: &'a ShapeGraph,
    table: &'a LayoutTable,
    nodes: Vec<N>,
    edges: Vec<u8>,
    logical_nodes: Vec<u32>,
    known: BTreeMap<(u32, LayoutId), Id>,
    pending: Vec<(u32, LayoutId, Id)>,
}
impl<'a> Builder<'a> {
    fn new(graph: &'a ShapeGraph, table: &'a LayoutTable) -> Self {
        Self {
            graph,
            table,
            nodes: vec![],
            edges: vec![],
            logical_nodes: vec![],
            known: BTreeMap::new(),
            pending: vec![],
        }
    }
    fn record(&self, id: LayoutId) -> Result<&'a crate::layout_types::LayoutRecord, String> {
        self.table
            .layout(id)
            .ok_or_else(|| "structural layout outside table".into())
    }
    fn node(&mut self, s: u32, layout: LayoutId) -> Id {
        if let Some(&id) = self.known.get(&(s, layout)) {
            return id;
        }
        let id = Id(self.nodes.len() as u32);
        self.known.insert((s, layout), id);
        self.nodes.push(N::Fields {
            layout,
            fields: vec![],
        });
        self.edges.push(0);
        self.logical_nodes.push(s);
        self.pending.push((s, layout, id));
        id
    }
    fn build(mut self, layout: LayoutId) -> Result<(Id, Vec<N>, Vec<u8>, Vec<u32>), String> {
        self.graph.check().map_err(str::to_owned)?;
        let root = self.node(self.graph.root, layout);
        while let Some((s, layout, id)) = self.pending.pop() {
            let (node, edge) = self.describe(s, layout)?;
            self.nodes[id.index()] = node;
            self.edges[id.index()] = edge;
        }
        Ok((root, self.nodes, self.edges, self.logical_nodes))
    }
    fn field(&self, l: LayoutId, n: u32) -> Result<LayoutId, String> {
        self.record(l)?
            .fields
            .iter()
            .find(|f| f.ordinal == n)
            .map(|f| f.layout)
            .ok_or_else(|| "missing structural field".into())
    }
    fn payload(&self, l: LayoutId) -> Result<LayoutId, String> {
        match self.record(l)?.kind {
            K::Block { payload } | K::Box { payload } | K::Shared { payload } => Ok(payload),
            _ => Err("structural block lacks payload".into()),
        }
    }
    fn shape(&self, mut s: u32) -> &'a Shape {
        while let Shape::Transparent(child) = self.graph.nodes[s as usize] {
            s = child;
        }
        &self.graph.nodes[s as usize]
    }
    fn flattened(&self, s: u32, layout: LayoutId) -> Result<bool, String> {
        Ok(match self.shape(s) {
            Shape::Fields(fields) => {
                let row = self.record(layout)?;
                matches!(row.kind, K::Aggregate | K::Flat | K::Unit)
                    && fields.len() == row.fields.len()
            }
            Shape::Variant(_) => self.record(layout)?.tag.is_some(),
            _ => false,
        })
    }
    fn describe(&mut self, s: u32, l: LayoutId) -> Result<(N, u8), String> {
        let r = self.record(l)?;
        if let K::Block { payload } = r.kind
            && self.record(payload)?.kind == K::Flat
        {
            return Ok((
                N::Box {
                    layout: l,
                    payload: self.node(s, payload),
                },
                0,
            ));
        }
        if let K::Frozen { value } = r.kind {
            return Ok((
                N::Fields {
                    layout: l,
                    fields: vec![self.node(s, value)],
                },
                0,
            ));
        }
        let shape = self.graph.nodes[s as usize].clone();
        if r.kind == K::Flat
            && !matches!(shape, Shape::Leaf | Shape::Sequence(_))
            && let [field] = r.fields.as_slice()
            && field.offset == 8
            && matches!(r.size, crate::layout_types::SizeSpec::Dynamic(_))
        {
            return Ok((
                N::Framed {
                    layout: l,
                    value: self.node(s, field.layout),
                },
                0,
            ));
        }
        Ok(match shape {
            Shape::Leaf => (
                match r.kind {
                    K::Scalar(_) => N::Scalar(l),
                    K::Path => N::Path(l),
                    K::Flat => N::FlatText {
                        layout: l,
                        utf8: false,
                    },
                    K::Slice { element } => {
                        let storage = self
                            .table
                            .layouts
                            .iter()
                            .position(|r| r.kind == K::Sequence { element })
                            .ok_or("text storage missing")?;
                        N::Text {
                            layout: l,
                            storage: LayoutId(storage as u32),
                            utf8: false,
                        }
                    }
                    _ => return Err("non-leaf physical representation for logical leaf".into()),
                },
                0,
            ),
            Shape::Transparent(child) | Shape::Distinct(child) => {
                let edge = u8::from(matches!(self.graph.nodes[s as usize], Shape::Distinct(_)));
                if matches!(r.kind, K::Shared { .. }) {
                    (
                        N::Box {
                            layout: l,
                            payload: self.node(child, self.payload(l)?),
                        },
                        0,
                    )
                } else if self.flattened(child, l)? {
                    (
                        N::Checked {
                            layout: l,
                            check: 0,
                            value: self.node(child, l),
                        },
                        edge,
                    )
                } else {
                    (
                        N::Fields {
                            layout: l,
                            fields: vec![self.node(child, self.field(l, 0)?)],
                        },
                        edge,
                    )
                }
            }
            Shape::Fields(children) => {
                if children.len() != r.fields.len() {
                    return Err("logical fields differ from physical representation".into());
                }
                let fields = children
                    .into_iter()
                    .enumerate()
                    .map(|(n, s)| Ok(self.node(s, self.field(l, n as u32)?)))
                    .collect::<Result<_, String>>()?;
                (N::Fields { layout: l, fields }, 1)
            }
            Shape::Variant(children) => {
                let tag = r.tag.as_ref().ok_or("logical variant lacks physical tag")?;
                if children.len() != tag.variants.len() {
                    return Err("logical arms differ from physical representation".into());
                }
                let mut arms = vec![];
                for (ordinal, children) in children.into_iter().enumerate() {
                    let arm = tag
                        .variants
                        .iter()
                        .find(|v| v.ordinal == ordinal as u32)
                        .ok_or("logical arm lacks physical representation")?;
                    if children.len() != arm.fields.len() {
                        return Err(
                            "logical payload fields differ from physical representation".into()
                        );
                    }
                    let mut fields = vec![];
                    for (n, s) in children.into_iter().enumerate() {
                        let layout = arm
                            .fields
                            .iter()
                            .find(|f| f.ordinal == n as u32)
                            .ok_or("logical payload field absent")?
                            .layout;
                        fields.push(self.node(s, layout));
                    }
                    arms.push(fields);
                }
                (N::Variant { layout: l, arms }, 1)
            }
            Shape::Box(child) => (
                N::Box {
                    layout: l,
                    payload: self.node(child, self.payload(l)?),
                },
                1,
            ),
            Shape::Sequence(child) => match r.kind {
                K::Inline { element, .. } | K::Simd { element, .. } => (
                    N::Repeat {
                        layout: l,
                        element: self.node(child, element),
                    },
                    1,
                ),
                K::Flat => (
                    N::Sequence {
                        layout: l,
                        element: self.node(child, self.field(l, 0)?),
                    },
                    1,
                ),
                _ => {
                    let payload = self.record(self.payload(l)?)?;
                    let element = match payload.kind {
                        K::Sequence { element }
                        | K::Sequential { element }
                        | K::SoaColumns { element } => element,
                        _ => return Err("logical sequence lacks physical storage".into()),
                    };
                    (
                        N::Sequence {
                            layout: l,
                            element: self.node(child, element),
                        },
                        1,
                    )
                }
            },
            Shape::Map { key, .. } | Shape::Set(key) => {
                let value = match self.graph.nodes[s as usize] {
                    Shape::Map { value, .. } => Some(value),
                    _ => None,
                };
                let container = self.payload(l)?;
                let entries = self.field(container, 0)?;
                let sequence = self.record(self.payload(entries)?)?;
                let K::Sequence { element } = sequence.kind else {
                    return Err("logical map lacks entry sequence".into());
                };
                let entry = self
                    .record(element)?
                    .tag
                    .as_ref()
                    .and_then(|t| t.variants.iter().find(|v| v.ordinal == 1))
                    .and_then(|v| v.fields.first())
                    .ok_or("logical map lacks live entry payload")?
                    .layout;
                let k = self.node(key, self.field(entry, 0)?);
                match value {
                    Some(value) => (
                        N::Map {
                            layout: l,
                            key: k,
                            value: self.node(value, self.field(entry, 1)?),
                        },
                        1,
                    ),
                    None => (
                        N::Set {
                            layout: l,
                            element: k,
                        },
                        1,
                    ),
                }
            }
        })
    }
}

/// A keyed primitive's preflight must occur after admission, before the first
/// taken actual, and read the actual key's caller storage. Canonical operation
/// identity comes from the call's independently checked declaration.
pub fn check_key_order<S: crate::stage::Stage>(unit: &crate::ir::LirUnit<S>) -> Result<(), String> {
    use crate::ir::*;
    fn origin<S>(
        unit: &LirUnit<S>,
        mut value: ValueId,
    ) -> Option<(AddressBase, Vec<ProjectionStep>)> {
        for _ in 0..=unit.values.len() {
            let instruction = unit
                .instructions
                .iter()
                .find(|inst| inst.defs().contains(&value))?;
            match *instruction {
                Inst::Read { address, .. } | Inst::Copy { address, .. } => {
                    let info = unit.addresses.get(address.index())?;
                    return Some((
                        info.base,
                        info.projection.get(&unit.pools.projections)?.to_vec(),
                    ));
                }
                Inst::Weaken { value: input, .. } | Inst::CopyValue { value: input, .. } => {
                    value = input
                }
                _ => return None,
            }
        }
        None
    }
    // RC may split a prefix to detach an aliased container. This independent
    // must-analysis retains only facts proved on every path, including each
    // instruction's failure edge before its successful effect.
    #[derive(Clone, Default, Eq, PartialEq)]
    struct Gate {
        admitted: bool,
        transferred: bool,
        keys: Vec<(AddressBase, Vec<ProjectionStep>)>,
    }
    let edge = |id: EdgeId| {
        unit.tables
            .edges
            .get(id.index())
            .map(|edge| edge.target)
            .ok_or("keyed edge outside table")
    };
    let failure_edge = |id: FailureId| {
        unit.tables
            .failures
            .get(id.index())
            .ok_or("keyed failure outside table")
            .and_then(|failure| edge(failure.cleanup))
    };
    let mut incoming = BTreeMap::from([(unit.entry, Gate::default())]);
    let mut pending = std::collections::VecDeque::from([unit.entry]);
    while let Some(block_id) = pending.pop_front() {
        let mut gate = incoming[&block_id].clone();
        let block = unit
            .blocks
            .get(block_id.index())
            .ok_or("keyed block outside table")?;
        let mut outgoing = Vec::new();
        for instruction in block
            .instructions
            .get(&unit.instructions)
            .ok_or("keyed instruction range outside table")?
        {
            if let Some(failure) = instruction.failure() {
                outgoing.push((failure_edge(failure)?, gate.clone()));
            }
            match instruction {
                Inst::FrameEnter { .. } => {
                    gate = Gate {
                        admitted: true,
                        ..Gate::default()
                    }
                }
                Inst::FrameLeave => gate = Gate::default(),
                Inst::Read {
                    mode: ReadMode::Take,
                    ..
                } => gate.transferred = true,
                Inst::Hash { value, .. } if gate.admitted && !gate.transferred => {
                    if let Some(storage) = origin(unit, *value) {
                        if !gate.keys.contains(&storage) {
                            gate.keys.push(storage);
                        }
                    }
                }
                _ => (),
            }
        }
        let terminator = &unit.terminators[block_id.index()];
        if let Terminator::Call { call, .. } = *terminator {
            let call = unit
                .tables
                .calls
                .get(call.index())
                .ok_or("keyed call outside table")?;
            let definition = match call.callee {
                Callee::Operation(operation) => Some(
                    &unit
                        .tables
                        .operations
                        .get(operation.index())
                        .ok_or("keyed primitive declaration outside table")?
                        .definition,
                ),
                Callee::Direct(symbol) | Callee::Bound { code: symbol, .. } => Some(
                    &unit
                        .tables
                        .symbols
                        .get(symbol.index())
                        .ok_or("keyed library symbol outside table")?
                        .definition,
                ),
                Callee::Indirect(_) => None,
            };
            if definition.is_some_and(|definition| {
                ["map.insert", "set.insert", "map.get", "set.contains"]
                    .iter()
                    .any(|name| definition.is_core(name))
            }) {
                let key = call
                    .arguments
                    .get(&unit.pools.call_arguments)
                    .and_then(|args| args.iter().find(|a| a.parameter == 1))
                    .ok_or("keyed primitive lacks key actual")?;
                let ArgumentValue::Value(value) = key.value else {
                    return Err("key is not a value actual".into());
                };
                let storage =
                    origin(unit, value).ok_or("key actual has no attributable caller storage")?;
                if !gate.admitted || !gate.keys.contains(&storage) {
                    return Err("key preflight is missing or follows argument transfer".into());
                }
            }
        }
        for id in terminator.normal_edges(&unit.pools) {
            outgoing.push((edge(id)?, gate.clone()));
        }
        for id in terminator.failure_edges(&unit.tables) {
            outgoing.push((failure_edge(id)?, gate.clone()));
        }
        for (target, state) in outgoing {
            if let Some(previous) = incoming.get_mut(&target) {
                let before = previous.clone();
                previous.admitted &= state.admitted;
                previous.transferred |= state.transferred;
                previous.keys.retain(|key| state.keys.contains(key));
                if *previous != before {
                    pending.push_back(target);
                }
            } else {
                incoming.insert(target, state);
                pending.push_back(target);
            }
        }
    }
    Ok(())
}

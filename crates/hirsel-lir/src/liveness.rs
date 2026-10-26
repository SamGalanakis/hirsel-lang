//! Backward value liveness over a LIR unit's normal and failure edges: ordinary
//! successors and expanded cleanup blocks. A block argument is a use on its incoming edge, not in the
//! target; a value an operation's failure successor needs (an argument of its
//! failure edge, or live into its cleanup) is live at that operation, so a
//! value used only by failure cleanup is live.
//!
//! This is the producer-side analysis RC insertion reads. Validators never
//! import it: they replay ownership independently.
use crate::ir::{
    AddressBase, ArgumentValue, BlockId, EdgeId, FailureId, Inst, LirUnit, MaskRef, ProjectionStep,
    Terminator, ValueId,
};
use crate::stage::Stage;

/// A dense set of value ids.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ValueSet(Vec<u64>);

impl ValueSet {
    pub fn with_capacity(values: usize) -> Self {
        Self(vec![0; values.div_ceil(64)])
    }
    pub fn insert(&mut self, v: ValueId) -> bool {
        let (w, b) = (v.index() / 64, v.index() % 64);
        if w >= self.0.len() {
            self.0.resize(w + 1, 0);
        }
        let had = self.0[w] & (1 << b) != 0;
        self.0[w] |= 1 << b;
        !had
    }
    pub fn remove(&mut self, v: ValueId) {
        if let Some(w) = self.0.get_mut(v.index() / 64) {
            *w &= !(1 << (v.index() % 64));
        }
    }
    pub fn contains(&self, v: ValueId) -> bool {
        self.0
            .get(v.index() / 64)
            .is_some_and(|w| w & (1 << (v.index() % 64)) != 0)
    }
    /// Add every member of `other`; whether anything was added.
    pub fn union(&mut self, other: &ValueSet) -> bool {
        if self.0.len() < other.0.len() {
            self.0.resize(other.0.len(), 0);
        }
        let mut changed = false;
        for (a, b) in self.0.iter_mut().zip(&other.0) {
            let next = *a | *b;
            changed |= next != *a;
            *a = next;
        }
        changed
    }
    pub fn iter(&self) -> impl Iterator<Item = ValueId> + '_ {
        self.0.iter().enumerate().flat_map(|(w, bits)| {
            (0..64)
                .filter(move |b| bits & (1u64 << b) != 0)
                .map(move |b| ValueId((w * 64 + b) as u32))
        })
    }
}

/// Live values at each block's entry (after its parameters are defined) and
/// exit (before its terminator's edges pass arguments).
#[derive(Clone, Debug)]
pub struct Liveness {
    pub live_in: Vec<ValueSet>,
    pub live_out: Vec<ValueSet>,
}

impl Liveness {
    /// Whether `value` is live after instruction `index` of `block`: some
    /// later instruction, the terminator, a failure successor or a
    /// successor block still needs it.
    pub fn live_after<S: Stage>(
        &self,
        unit: &LirUnit<S>,
        block: BlockId,
        index: usize,
        value: ValueId,
    ) -> bool {
        let insts = unit.blocks[block.index()]
            .instructions
            .get(&unit.instructions)
            .unwrap_or(&[]);
        let mut live = self.exit(unit, block);
        for inst in insts[index + 1..].iter().rev() {
            self.step(unit, inst, &mut live);
        }
        live.contains(value)
    }

    /// Values retained by normal and every failure successor, including
    /// call-defined write-back versions. A saved caller owns their storage
    /// until the selected return or pretransfer rejection resumes it.
    pub fn successor_needs<S: Stage>(&self, unit: &LirUnit<S>, block: BlockId) -> ValueSet {
        let mut live = self.live_out[block.index()].clone();
        for f in unit.terminators[block.index()].failure_edges(&unit.tables) {
            live.union(&self.failure_needs(unit, f));
        }
        live
    }

    /// Live just before the terminator. A call's failure edge may name the
    /// call's own write-back versions, so its needs join before the call's
    /// definitions are removed.
    fn exit<S: Stage>(&self, unit: &LirUnit<S>, block: BlockId) -> ValueSet {
        let mut live = self.successor_needs(unit, block);
        let term = &unit.terminators[block.index()];
        for d in terminator_defs(unit, term) {
            live.remove(d);
        }
        for u in terminator_uses(unit, term) {
            live.insert(u);
        }
        live
    }

    /// Transfer backwards over one instruction.
    fn step<S: Stage>(&self, unit: &LirUnit<S>, inst: &Inst, live: &mut ValueSet) {
        for d in Inst::defs(inst) {
            live.remove(d);
        }
        for u in inst_uses(unit, inst) {
            live.insert(u);
        }
        if let Some(f) = Inst::failure(inst) {
            live.union(&self.failure_needs(unit, f));
        }
    }

    /// What a failure successor needs at its source: its edge's arguments and
    /// what is live into its cleanup block other than that block's parameters.
    fn failure_needs<S: Stage>(&self, unit: &LirUnit<S>, f: FailureId) -> ValueSet {
        let failure = &unit.tables.failures[f.index()];
        let mut out = self.edge_needs(unit, failure.cleanup);
        for &v in failure.dispose.get(&unit.pools.values).unwrap_or(&[]) {
            out.insert(v);
        }
        out
    }

    fn edge_needs<S: Stage>(&self, unit: &LirUnit<S>, e: EdgeId) -> ValueSet {
        let edge = &unit.tables.edges[e.index()];
        let mut out = self.live_in[edge.target.index()].clone();
        for &p in unit.blocks[edge.target.index()]
            .parameters
            .get(&unit.pools.values)
            .unwrap_or(&[])
        {
            out.remove(p);
        }
        for &a in edge.arguments.get(&unit.pools.values).unwrap_or(&[]) {
            out.insert(a);
        }
        out
    }
}

/// Compute liveness to a fixpoint.
pub fn liveness<S: Stage>(unit: &LirUnit<S>) -> Liveness {
    let n = unit.blocks.len();
    let values = unit.values.len();
    let mut l = Liveness {
        live_in: vec![ValueSet::with_capacity(values); n],
        live_out: vec![ValueSet::with_capacity(values); n],
    };
    let mut changed = true;
    while changed {
        changed = false;
        for b in (0..n).rev() {
            let block = BlockId(b as u32);
            let mut out = ValueSet::with_capacity(values);
            for e in Terminator::normal_edges(&unit.terminators[b], &unit.pools) {
                out.union(&l.edge_needs(unit, e));
            }
            changed |= l.live_out[b].union(&out);
            let mut live = l.exit(unit, block);
            let insts = unit.blocks[b]
                .instructions
                .get(&unit.instructions)
                .unwrap_or(&[]);
            for inst in insts.iter().rev() {
                l.step(unit, inst, &mut live);
            }
            for &p in unit.blocks[b]
                .parameters
                .get(&unit.pools.values)
                .unwrap_or(&[])
            {
                live.remove(p);
            }
            changed |= l.live_in[b].union(&live);
        }
    }
    l
}

/// The values an instruction reads, including those named through its
/// side-table rows (construction fields, captures) and the index operands
/// and value bases of an address it prepares.
pub fn inst_uses<S: Stage>(unit: &LirUnit<S>, inst: &Inst) -> Vec<ValueId> {
    let defs = Inst::defs(inst);
    let mut out = Vec::new();
    inst.visit(&mut |r| {
        if let crate::ir::Ref::Value(v) = r
            && !defs.contains(&v)
        {
            out.push(v);
        }
    });
    match *inst {
        Inst::PayloadReceipt { inputs, .. } => {
            for input in inputs.get(&unit.pools.receipt_inputs).unwrap_or(&[]) {
                out.extend(input.receipt);
            }
        }
        Inst::Address { address, .. } => {
            let info = &unit.addresses[address.index()];
            if let AddressBase::Value(v) = info.base {
                out.push(v);
            }
            for step in info.projection.get(&unit.pools.projections).unwrap_or(&[]) {
                match *step {
                    ProjectionStep::Payload
                    | ProjectionStep::Field(_)
                    | ProjectionStep::VariantField { .. } => {}
                    ProjectionStep::Index(v) | ProjectionStep::Slot(v) => out.push(v),
                    ProjectionStep::FlatAt { index, offset } => out.extend([index, offset]),
                }
            }
        }
        Inst::Construct { construct, .. }
        | Inst::Reuse { construct, .. }
        | Inst::UniqueCopy { construct, .. }
        | Inst::FlatAppend { construct, .. }
        | Inst::FlatClose { construct, .. } => {
            let site = &unit.tables.constructs[construct.index()];
            out.extend(
                site.fields
                    .get(&unit.pools.fields)
                    .unwrap_or(&[])
                    .iter()
                    .map(|f| f.value),
            );
        }
        Inst::Closure { closure, .. } => {
            let site = &unit.tables.closures[closure.index()];
            if let Some(receipts) = &site.receipts {
                out.extend(receipts.captures.iter().map(|c| c.receipt));
            }
            out.extend(
                site.captures
                    .get(&unit.pools.captures)
                    .unwrap_or(&[])
                    .iter()
                    .map(|c| c.value),
            );
        }
        Inst::Release { live, .. } | Inst::Reset { live, .. } => {
            if let MaskRef::Dynamic(v) = live {
                out.push(v);
            }
        }
        _ => {}
    }
    out
}

/// Values a terminator defines: a call's result and its `mut` write-back
/// versions.
pub fn terminator_defs<S: Stage>(unit: &LirUnit<S>, term: &Terminator) -> Vec<ValueId> {
    let Terminator::Call { call, .. } = *term else {
        return Vec::new();
    };
    let site = &unit.tables.calls[call.index()];
    let mut out: Vec<ValueId> = site.normal_results().collect();
    out.extend(site.writebacks(&unit.pools).map(|(_, version)| version));
    out
}

/// Values a terminator reads before its edges.
pub fn terminator_uses<S: Stage>(unit: &LirUnit<S>, term: &Terminator) -> Vec<ValueId> {
    match *term {
        Terminator::Branch { condition, .. } => vec![condition],
        Terminator::Switch { value, .. } => vec![value],
        Terminator::Isolate { start, .. } => vec![start],
        Terminator::Return(v) => v.into_iter().collect(),
        Terminator::Call { call, .. } => {
            let site = &unit.tables.calls[call.index()];
            let mut out = Vec::new();
            if let Some(v) = site.callee.value() {
                out.push(v);
            }
            out.extend(site.receipts.iter().flat_map(|receipt| receipt.inputs()));
            for a in site
                .arguments
                .get(&unit.pools.call_arguments)
                .unwrap_or(&[])
            {
                if let ArgumentValue::Value(v) = a.value {
                    out.push(v);
                }
            }
            out
        }
        _ => Vec::new(),
    }
}

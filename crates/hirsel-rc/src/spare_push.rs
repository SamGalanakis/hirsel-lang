//! Pushes into reserved storage (FACT-3): a `list.push` that the compiler
//! proves has spare capacity calls `$native.list_push_spare`, which appends
//! without reading the capacity or growing. Removing a check that cannot fail
//! changes cost only.
//!
//! The proof is about one counted loop over a fresh list:
//!
//! ```text
//! let mut xs = []                // the static empty sequence: count 0
//! list.reserve(mut xs, m)        // capacity >= m, unique
//! for j in 0..m { ... list.push(mut xs, v) ... }
//! ```
//!
//! The list's version entering the loop is exactly the reserve's, the reserve's
//! input is the empty sequence, the loop's index starts at zero and grows by one
//! on its only feedback edge, the push is guarded by `j < m` with the reserve's
//! `m` defined outside the loop, and the push is the loop's only reference to the
//! list and lies on no inner cycle. At most one push runs per iteration, so the
//! count before the push is at most `j < m <= capacity`. A detach preserves
//! capacity, so sharing the list elsewhere cannot invalidate the bound.
//! Discovery and proof are read-only; only a complete proof edits a unit.
//!
//! A byte field built in place (LIR schema revision 24) is proved the same
//! way: its opening is the empty list, its `FlatField` reservation leaves
//! room for `m` bytes past its length word, and a push of the loop marks
//! itself `spare`.
use crate::flat_walk::{Graph, Values, code, definitions, integer, scalar_source};
use crate::liveness::inst_defs;
use hirsel_lir::ir::*;
use hirsel_lir::layout_types::{LayoutId, LayoutKind, ScalarKind};
use hirsel_lir::stage::Value;
use std::collections::BTreeSet;

/// The library path of the spare-capacity append.
pub fn spare_path() -> hirsel_syntax::DefPath {
    crate::library::native_path("list_push_spare")
}

/// A proven push: the call to retarget.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Proven {
    pub block: usize,
    pub call: CallId,
}

/// Every `list.push` call of `u` proven to append into spare capacity.
/// `layout` answers the record of a layout id (the table may be unsealed).
pub fn prove(u: &LirUnit<Value>, layout: &dyn Fn(LayoutId) -> LayoutKind) -> Vec<Proven> {
    let push = crate::library::core_path("list.push");
    let pushes: Vec<(usize, CallId)> = u
        .terminators
        .iter()
        .enumerate()
        .filter_map(|(b, t)| match *t {
            Terminator::Call { call, .. } if callee_is(u, call, &push) => Some((b, call)),
            _ => None,
        })
        .collect();
    if pushes.is_empty() {
        return Vec::new();
    }
    let g = Graph::new(u);
    let forward = forwards(u, &g);
    let cx = Proof {
        u,
        g,
        defs: definitions(u),
        def_block: def_blocks(u),
        forward,
        reserve: crate::library::core_path("list.reserve"),
        empty: crate::import::empty_sequence(),
    };
    pushes
        .into_iter()
        .filter(|&(block, call)| cx.spare(block, call, layout).is_some())
        .map(|(block, call)| Proven { block, call })
        .collect()
}

/// Every open field push of `u` proven to write into its reservation: the
/// instruction's index in `u.instructions`.
pub fn prove_fields(u: &LirUnit<Value>) -> Vec<usize> {
    let mut pushes = Vec::new();
    for (b, block) in u.blocks.iter().enumerate() {
        for (i, inst) in code(u, b).iter().enumerate() {
            if let Inst::FlatField {
                op: FlatFieldOp::Push { spare: false, .. },
                ..
            } = inst
            {
                pushes.push((b, i, block.instructions.start as usize + i));
            }
        }
    }
    if pushes.is_empty() {
        return Vec::new();
    }
    let g = Graph::new(u);
    let forward = forwards(u, &g);
    let cx = Proof {
        u,
        g,
        defs: definitions(u),
        def_block: def_blocks(u),
        forward,
        reserve: crate::library::core_path("list.reserve"),
        empty: crate::import::empty_sequence(),
    };
    pushes
        .into_iter()
        .filter(|&(b, i, _)| cx.field_spare(b, i).is_some())
        .map(|(_, _, at)| at)
        .collect()
}

/// Mark each proven open field push of `u` spare.
pub fn commit_fields(u: &mut LirUnit<Value>, proven: &[usize]) {
    for &at in proven {
        if let Inst::FlatField {
            op: FlatFieldOp::Push { spare, .. },
            ..
        } = &mut u.instructions[at]
        {
            *spare = true;
        }
    }
}

fn callee_is(u: &LirUnit<Value>, call: CallId, path: &hirsel_syntax::DefPath) -> bool {
    match u.tables.calls[call.index()].callee {
        Callee::Direct(symbol) => u.tables.symbols[symbol.index()].definition == *path,
        _ => false,
    }
}

/// The block defining each value: its parameter's block, its instruction's
/// block, or the block of the call that defines it.
fn def_blocks(u: &LirUnit<Value>) -> Vec<Option<usize>> {
    let mut at = vec![None; u.values.len()];
    for (b, block) in u.blocks.iter().enumerate() {
        for &p in block.parameters.get(&u.pools.values).unwrap_or(&[]) {
            at[p.index()] = Some(b);
        }
        for inst in code(u, b) {
            for v in inst_defs(inst) {
                at[v.index()] = Some(b);
            }
        }
        if let Terminator::Call { call, .. } = u.terminators[b] {
            let site = &u.tables.calls[call.index()];
            if let Some(r) = site.result {
                at[r.index()] = Some(b);
            }
            for a in site.arguments.get(&u.pools.call_arguments).unwrap_or(&[]) {
                if let ArgumentValue::Address { version, .. } = a.value {
                    at[version.index()] = Some(b);
                }
            }
        }
    }
    at
}

/// Trivial block parameters, as LLVM removes trivial phis: one whose
/// incoming arguments, after forwarding, are a single value besides itself
/// forwards to that value. Iterated to a fixpoint, so a cycle of parameters
/// passing one value around a loop collapses onto it.
fn forwards(u: &LirUnit<Value>, g: &Graph) -> Vec<ValueId> {
    let mut rep: Vec<ValueId> = (0..u.values.len() as u32).map(ValueId).collect();
    let find = |rep: &[ValueId], mut v: ValueId| {
        while rep[v.index()] != v {
            v = rep[v.index()];
        }
        v
    };
    let mut changed = true;
    while changed {
        changed = false;
        for (b, block) in u.blocks.iter().enumerate() {
            let params = block.parameters.get(&u.pools.values).unwrap_or(&[]);
            for (ordinal, &p) in params.iter().enumerate() {
                if rep[p.index()] != p {
                    continue;
                }
                let mut only = None;
                let mut trivial = !g.pred[b].is_empty();
                for &(_, e) in &g.pred[b] {
                    let Some(&a) = u.tables.edges[e.index()]
                        .arguments
                        .get(&u.pools.values)
                        .and_then(|a| a.get(ordinal))
                    else {
                        trivial = false;
                        break;
                    };
                    let a = find(&rep, a);
                    if a == p {
                        continue;
                    }
                    match only {
                        None => only = Some(a),
                        Some(o) if o == a => {}
                        Some(_) => {
                            trivial = false;
                            break;
                        }
                    }
                }
                if trivial && let Some(o) = only {
                    rep[p.index()] = o;
                    changed = true;
                }
            }
        }
    }
    rep
}

/// The push a proof is about: a library call, or the open field push at an
/// index of its block.
#[derive(Clone, Copy, Eq, PartialEq)]
enum Push {
    Call(CallId),
    Field(usize),
}

struct Proof<'u> {
    u: &'u LirUnit<Value>,
    g: Graph,
    defs: Values,
    def_block: Vec<Option<usize>>,
    /// Each value's representative: a block parameter whose incoming
    /// arguments are one value or parameters forwarding to itself names
    /// that value.
    forward: Vec<ValueId>,
    reserve: hirsel_syntax::DefPath,
    empty: hirsel_syntax::DefPath,
}

impl Proof<'_> {
    /// `v` through scalar copies and loop-invariant block parameters: one
    /// whose incoming arguments are all one value or the parameter itself.
    fn resolve(&self, mut v: ValueId) -> ValueId {
        let mut seen = BTreeSet::new();
        while seen.insert(v) {
            v = scalar_source(self.u, &self.defs, v);
            match self.invariant(v) {
                Some(next) => v = next,
                None => break,
            }
        }
        v
    }

    fn invariant(&self, p: ValueId) -> Option<ValueId> {
        let mut v = p;
        while self.forward[v.index()] != v {
            v = self.forward[v.index()];
        }
        (v != p).then_some(v)
    }

    /// The root slot of an address, through nested address bases.
    fn root(&self, mut a: AddressId) -> Option<SlotId> {
        loop {
            match self.u.addresses[a.index()].base {
                AddressBase::Slot(s) => return Some(s),
                AddressBase::Address(b) => a = b,
                AddressBase::Value(_) => return None,
            }
        }
    }

    fn whole(&self, a: AddressId) -> bool {
        self.u.addresses[a.index()].projection.len == 0
    }

    /// The `mut` argument of a call: its whole root slot and new version.
    fn mut_root(&self, call: CallId) -> Option<(AddressId, SlotId, ValueId)> {
        let site = &self.u.tables.calls[call.index()];
        let arguments = site.arguments.get(&self.u.pools.call_arguments)?;
        let first = arguments.iter().find(|a| a.parameter == 0)?;
        let ArgumentValue::Address { address, version } = first.value else {
            return None;
        };
        (self.whole(address)).then_some(())?;
        Some((address, self.root(address)?, version))
    }

    fn spare(
        &self,
        block: usize,
        call: CallId,
        layout: &dyn Fn(LayoutId) -> LayoutKind,
    ) -> Option<()> {
        let u = self.u;
        let (_, list, _) = self.mut_root(call)?;
        // Only element-strided storage: capacity counts elements.
        let LayoutKind::Block { payload } =
            layout(u.types[u.slots[list.index()].ty.index()].layout)
        else {
            return None;
        };
        if !matches!(layout(payload), LayoutKind::Sequence { .. }) {
            return None;
        }
        self.counted(block, list, Push::Call(call), |v| self.reserve_of(v, list))
    }

    /// The open field push at instruction `at` of block `block`.
    fn field_spare(&self, block: usize, at: usize) -> Option<()> {
        let Inst::FlatField { address, field, .. } = code(self.u, block)[at] else {
            return None;
        };
        if !self.whole(address) {
            return None;
        }
        let list = self.root(address)?;
        self.counted(block, list, Push::Field(at), |v| {
            self.field_reserve_of(v, list, field)
        })
    }

    /// Whether the push in `block` runs at most once per iteration of a
    /// counted loop `j < m` over the list's reserved version, `reserve_of`
    /// naming the reservation's `m` and block.
    fn counted(
        &self,
        block: usize,
        list: SlotId,
        push: Push,
        reserve_of: impl Fn(ValueId) -> Option<(ValueId, usize)>,
    ) -> Option<()> {
        let u = self.u;
        let (header, members) = self.innermost(block)?;
        if self.inner_cycle(block, header, &members) {
            return None;
        }
        // The push is the loop's only reference to the list.
        for &b in &members {
            for (i, inst) in code(u, b).iter().enumerate() {
                if self.references(inst, list) && !(b == block && push == Push::Field(i)) {
                    return None;
                }
            }
            if let Terminator::Call { call: other, .. } = u.terminators[b] {
                let site = &u.tables.calls[other.index()];
                for a in site.arguments.get(&u.pools.call_arguments).unwrap_or(&[]) {
                    if let ArgumentValue::Address { address, .. } = a.value
                        && self.root(address) == Some(list)
                        && push != Push::Call(other)
                    {
                        return None;
                    }
                }
            }
        }
        // The list's version entering the loop is a reserve's of a fresh list.
        let params = u.blocks[header].parameters.get(&u.pools.values)?;
        let entries: Vec<EdgeId> = self.g.pred[header]
            .iter()
            .filter(|(p, _)| !members.contains(p))
            .map(|&(_, e)| e)
            .collect();
        if entries.is_empty() {
            return None;
        }
        let entering = |ordinal: usize| -> Option<Vec<ValueId>> {
            entries
                .iter()
                .map(|e| {
                    u.tables.edges[e.index()]
                        .arguments
                        .get(&u.pools.values)?
                        .get(ordinal)
                        .copied()
                })
                .collect()
        };
        let mut reserved = None;
        for ordinal in 0..params.len() {
            let versions = entering(ordinal)?;
            let resolved: BTreeSet<ValueId> = versions.iter().map(|&v| self.follow(v)).collect();
            let [version] = resolved.into_iter().collect::<Vec<_>>()[..] else {
                continue;
            };
            if let Some(found) = reserve_of(version) {
                if reserved.replace((ordinal, found)).is_some() {
                    return None;
                }
            }
        }
        let (_, (capacity, reserve_block)) = reserved?;
        if members.contains(&reserve_block) {
            return None;
        }
        let bound = self.resolve(capacity);
        // A unit parameter has no defining block and is invariant.
        if self.def_block[bound.index()].is_some_and(|b| members.contains(&b)) {
            return None;
        }
        // A zero-based unit-stride index whose `j < m` guards the push.
        let index = self.guard(block, header, &members, bound)?;
        if u.types[u.values[index.index()].ty.index()].repr != ValueRepr::Scalar(ScalarKind::U64) {
            return None;
        }
        let ordinal = params.iter().position(|&p| p == index)?;
        for v in entering(ordinal)? {
            if integer(u, &self.defs, v) != Some(0) {
                return None;
            }
        }
        for &(p, e) in &self.g.pred[header] {
            if !members.contains(&p) {
                continue;
            }
            let next = *u.tables.edges[e.index()]
                .arguments
                .get(&u.pools.values)?
                .get(ordinal)?;
            let Some(Inst::Binary {
                op: BinaryOp::Add,
                left,
                right,
                ..
            }) = self.defs.inst[scalar_source(u, &self.defs, next).index()]
            else {
                return None;
            };
            let one = |v| integer(u, &self.defs, v) == Some(1);
            let at = |v| scalar_source(u, &self.defs, v) == index;
            if !(at(left) && one(right) || at(right) && one(left)) {
                return None;
            }
        }
        Some(())
    }

    /// When `version` is the list's version produced by a `FlatField`
    /// reservation of `field` whose reaching version is that field's
    /// opening: the reserved capacity and the reservation's block.
    fn field_reserve_of(
        &self,
        version: ValueId,
        list: SlotId,
        field: u32,
    ) -> Option<(ValueId, usize)> {
        let b = self.def_block[version.index()]?;
        let (at, capacity) =
            code(self.u, b)
                .iter()
                .enumerate()
                .find_map(|(i, inst)| match *inst {
                    Inst::FlatField {
                        address,
                        field: f,
                        op: FlatFieldOp::Reserve { capacity },
                        version: v,
                    } if v == version
                        && f == field
                        && self.whole(address)
                        && self.root(address) == Some(list) =>
                    {
                        Some((i, capacity))
                    }
                    _ => None,
                })?;
        let before = self.reaching(b, at, list)?;
        match self.defs.inst[before.index()]? {
            Inst::FlatField {
                field: f,
                op: FlatFieldOp::Open,
                version: v,
                ..
            } if v == before && f == field => Some((capacity, b)),
            _ => None,
        }
    }

    /// A version through block parameters that only forward one value.
    fn follow(&self, mut v: ValueId) -> ValueId {
        let mut seen = BTreeSet::new();
        while seen.insert(v) {
            match self.invariant(v) {
                Some(next) => v = next,
                None => break,
            }
        }
        v
    }

    /// When `version` is the list's version produced by `list.reserve` of a
    /// list whose reaching content was the static empty sequence: the
    /// reserved capacity and the reserve's block.
    fn reserve_of(&self, version: ValueId, list: SlotId) -> Option<(ValueId, usize)> {
        let u = self.u;
        let b = self.def_block[version.index()]?;
        let Terminator::Call { call, .. } = u.terminators[b] else {
            return None;
        };
        if !callee_is(u, call, &self.reserve) {
            return None;
        }
        let (_, root, out) = self.mut_root(call)?;
        if root != list || out != version {
            return None;
        }
        let site = &u.tables.calls[call.index()];
        let capacity = site
            .arguments
            .get(&u.pools.call_arguments)?
            .iter()
            .find(|a| a.parameter == 1)?;
        let ArgumentValue::Value(capacity) = capacity.value else {
            return None;
        };
        let before = self.reaching(b, code(u, b).len(), list)?;
        let content = scalar_source(u, &self.defs, before);
        match self.defs.inst[content.index()]? {
            Inst::ReadStatic { symbol, .. }
                if u.tables.symbols[symbol.index()].definition == self.empty =>
            {
                Some((capacity, b))
            }
            _ => None,
        }
    }

    /// The version of `slot` reaching instruction `at` of block `b`: the last
    /// write before it in the block, else the block's join parameter, else,
    /// along a single predecessor, the version reaching its end.
    fn reaching(&self, mut b: usize, mut at: usize, slot: SlotId) -> Option<ValueId> {
        let u = self.u;
        let mut seen = BTreeSet::new();
        loop {
            if !seen.insert(b) {
                return None;
            }
            for inst in code(u, b)[..at].iter().rev() {
                let written = match *inst {
                    Inst::Store {
                        address, version, ..
                    }
                    | Inst::Replace {
                        address, version, ..
                    }
                    | Inst::FlatField {
                        address, version, ..
                    } => (self.root(address) == Some(slot)).then_some(version),
                    _ => None,
                };
                if let Some(v) = written {
                    return self.whole_write(inst).then_some(v);
                }
                if self.references(inst, slot)
                    && !matches!(
                        inst,
                        Inst::BeginLoan { .. } | Inst::BindOrigin { .. } | Inst::Address { .. }
                    )
                {
                    return None;
                }
            }
            let params = u.blocks[b].parameters.get(&u.pools.values).unwrap_or(&[]);
            if let Some(&p) = params.iter().find(|&&p| self.version_of(p) == Some(slot)) {
                return Some(self.follow(p));
            }
            let [(p, _)] = self.g.pred[b][..] else {
                return None;
            };
            if let Terminator::Call { call, .. } = u.terminators[p] {
                let site = &u.tables.calls[call.index()];
                for a in site.arguments.get(&u.pools.call_arguments).unwrap_or(&[]) {
                    if let ArgumentValue::Address { address, version } = a.value
                        && self.root(address) == Some(slot)
                    {
                        return Some(version);
                    }
                }
            }
            b = p;
            at = code(u, p).len();
        }
    }

    fn whole_write(&self, inst: &Inst) -> bool {
        match *inst {
            Inst::Store { address, .. }
            | Inst::Replace { address, .. }
            | Inst::FlatField { address, .. } => self.whole(address),
            _ => false,
        }
    }

    /// The slot a version value names, from the writes and joins that define it.
    fn version_of(&self, v: ValueId) -> Option<SlotId> {
        let u = self.u;
        let b = self.def_block[v.index()]?;
        let params = u.blocks[b].parameters.get(&u.pools.values)?;
        let ordinal = params.iter().position(|&p| p == v)?;
        for &(_, e) in &self.g.pred[b] {
            let a = *u.tables.edges[e.index()]
                .arguments
                .get(&u.pools.values)?
                .get(ordinal)?;
            if a == v {
                continue;
            }
            return self.defined_version(a);
        }
        None
    }

    fn defined_version(&self, v: ValueId) -> Option<SlotId> {
        let u = self.u;
        match self.defs.inst[v.index()] {
            Some(Inst::Store {
                address, version, ..
            }) if version == v => self.root(address),
            Some(Inst::Replace {
                address, version, ..
            }) if version == v => self.root(address),
            Some(Inst::FlatField {
                address, version, ..
            }) if version == v => self.root(address),
            Some(_) => None,
            None => {
                let b = self.def_block[v.index()]?;
                if let Terminator::Call { call, .. } = u.terminators[b] {
                    let site = &u.tables.calls[call.index()];
                    for a in site.arguments.get(&u.pools.call_arguments)? {
                        if let ArgumentValue::Address { address, version } = a.value
                            && version == v
                        {
                            return self.root(address);
                        }
                    }
                }
                self.version_of(v)
            }
        }
    }

    /// Whether `inst` names the slot or an address rooted at it, other than
    /// a mutable loan or the preparation of its whole address.
    fn references(&self, inst: &Inst, slot: SlotId) -> bool {
        if let Inst::BeginLoan {
            address,
            mutable: true,
            ..
        }
        | Inst::Address { address, .. } = *inst
            && self.whole(address)
            && self.root(address) == Some(slot)
        {
            return false;
        }
        let mut hit = false;
        inst.visit(&mut |r| match r {
            Ref::Slot(s) if s == slot => hit = true,
            Ref::Address(a) if self.root(a) == Some(slot) => hit = true,
            _ => {}
        });
        hit
    }

    /// The innermost natural loop containing `b`: its header and members,
    /// the union over every feedback edge, which must be exactly one.
    fn innermost(&self, b: usize) -> Option<(usize, BTreeSet<usize>)> {
        let mut latches: std::collections::BTreeMap<usize, Vec<usize>> = Default::default();
        for (latch, outs) in self.g.succ.iter().enumerate() {
            for &(header, _) in outs {
                if self.g.dominates(header, latch) {
                    latches.entry(header).or_default().push(latch);
                }
            }
        }
        let mut best: Option<(usize, Vec<usize>, BTreeSet<usize>)> = None;
        for (header, latches) in latches {
            let mut members = BTreeSet::from([header]);
            let mut q = latches.clone();
            while let Some(x) = q.pop() {
                if members.insert(x) {
                    q.extend(self.g.pred[x].iter().map(|p| p.0));
                }
            }
            if members.contains(&b)
                && best
                    .as_ref()
                    .is_none_or(|(_, _, m)| members.len() < m.len())
            {
                best = Some((header, latches, members));
            }
        }
        let (header, latches, members) = best?;
        (latches.len() == 1).then_some((header, members))
    }

    /// Whether `b` lies on a cycle of the loop that avoids its header.
    fn inner_cycle(&self, b: usize, header: usize, members: &BTreeSet<usize>) -> bool {
        let mut seen = BTreeSet::new();
        let mut q: Vec<usize> = self.g.succ[b].iter().map(|s| s.0).collect();
        while let Some(x) = q.pop() {
            if x == b {
                return true;
            }
            if x == header || !members.contains(&x) || !seen.insert(x) {
                continue;
            }
            q.extend(self.g.succ[x].iter().map(|s| s.0));
        }
        false
    }

    /// Whether `limit` is at most the reserved `bound`: the same value, the
    /// lengths of one slice, or integer constants in that order.
    fn within(&self, limit: ValueId, bound: ValueId) -> bool {
        let limit = self.resolve(limit);
        limit == bound
            || self
                .slice_length(limit)
                .is_some_and(|s| self.slice_length(bound) == Some(s))
            || matches!(
                (integer(self.u, &self.defs, limit), integer(self.u, &self.defs, bound)),
                (Some(l), Some(b)) if l <= b
            )
    }

    /// The slice whose length `v` is, through whole reads of its place's
    /// version: `bytes.len(data)` read twice is one length, since a slice is
    /// an immutable value. A sequence block's count can change between two
    /// reads, so only a slice qualifies.
    fn slice_length(&self, v: ValueId) -> Option<ValueId> {
        let u = self.u;
        let Some(Inst::SeqLen { value, .. }) = self.defs.inst[v.index()] else {
            return None;
        };
        (u.types[u.values[value.index()].ty.index()].repr != ValueRepr::Block)
            .then(|| self.resolve(value))
    }

    /// The header parameter `j` of a branch on `j < limit`, `limit` within
    /// the reserved `bound`, whose true
    /// successor, entered only from that branch, dominates `b`.
    fn guard(
        &self,
        b: usize,
        header: usize,
        members: &BTreeSet<usize>,
        bound: ValueId,
    ) -> Option<ValueId> {
        let u = self.u;
        let params = u.blocks[header].parameters.get(&u.pools.values)?;
        for &t in members {
            let Terminator::Branch { condition, yes, .. } = u.terminators[t] else {
                continue;
            };
            let target = u.tables.edges[yes.index()].target.index();
            if self.g.pred[target].len() != 1 || !self.g.dominates(target, b) {
                continue;
            }
            let Some(Inst::Binary {
                op: BinaryOp::Lt,
                left,
                right,
                ..
            }) = self.defs.inst[scalar_source(u, &self.defs, condition).index()]
            else {
                continue;
            };
            let index = scalar_source(u, &self.defs, left);
            if params.contains(&index) && self.within(right, bound) {
                return Some(index);
            }
        }
        None
    }
}

/// Retarget each proven push of `u` to the spare-capacity body. `requests`
/// holds the unit's library requests; the push's request supplies the
/// instance actuals, and the new instance joins `requests`.
pub fn commit(
    u: &mut LirUnit<Value>,
    proven: &[Proven],
    requests: &mut Vec<crate::library::Request>,
    layouts: &dyn crate::library::builder::Layouts,
) -> Result<(), crate::instance::InstanceError> {
    let spare = spare_path();
    let body = crate::library::body_index(&spare).expect("registered spare push");
    for p in proven {
        let Callee::Direct(symbol) = u.tables.calls[p.call.index()].callee else {
            unreachable!("a proven push is a direct call")
        };
        let instance = u.tables.symbols[symbol.index()].instance;
        let mut request = None;
        for r in requests.iter() {
            if Some(r.key(layouts)?.digest()) == instance {
                request = Some(r.clone());
                break;
            }
        }
        let push = request.expect("a push's library request");
        let request = crate::library::Request::native(
            body,
            &spare,
            push.actuals.store.clone(),
            push.actuals.arguments.clone(),
            push.callable,
        );
        let target = Symbol {
            kind: SymbolKind::Function,
            definition: spare.clone(),
            instance: Some(request.key(layouts)?.digest()),
        };
        let id = match u.tables.symbols.iter().position(|s| *s == target) {
            Some(i) => SymbolId(i as u32),
            None => {
                u.tables.symbols.push(target);
                SymbolId(u.tables.symbols.len() as u32 - 1)
            }
        };
        u.tables.calls[p.call.index()].callee = Callee::Direct(id);
        requests.push(request);
        // A push instance no call names any more is not built.
        let mut named = u
            .tables
            .calls
            .iter()
            .any(|c| c.callee == Callee::Direct(symbol));
        for inst in &u.instructions {
            inst.visit(&mut |r| named |= r == Ref::Symbol(symbol));
        }
        if !named {
            let mut kept = Vec::with_capacity(requests.len());
            for r in requests.drain(..) {
                if Some(r.key(layouts)?.digest()) != instance {
                    kept.push(r);
                }
            }
            *requests = kept;
        }
    }
    Ok(())
}

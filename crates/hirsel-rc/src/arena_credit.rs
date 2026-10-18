//! Infer block result placement across the complete call graph, then omit
//! construction credit walks only for payloads whose children share its token.
//! Unknown operations, mutable token storage and projected loads prove nothing.
use hirsel_lir::{
    abi_types::FnAbiId,
    ir::*,
    layout_types::{CopyRecipe, LayoutKind, LayoutTable},
    stage::{Placed, Value},
};
use std::collections::{BTreeMap, BTreeSet, VecDeque};

#[derive(Clone)]
enum Source {
    Unknown,
    Flow(Vec<ValueId>),
    Made(ConstructId),
    Call(usize),
}
struct Discovery<'a, S> {
    u: &'a LirUnit<S>,
    table: &'a LayoutTable,
    source: Vec<Source>,
    reads: Vec<Option<SlotId>>,
    stable: Vec<bool>,
    parameters: BTreeMap<ValueId, u32>,
}
impl<'a, S> Discovery<'a, S> {
    fn new(u: &'a LirUnit<S>, table: &'a LayoutTable) -> Self {
        fn whole<S>(u: &LirUnit<S>, a: AddressId) -> Option<SlotId> {
            let a = &u.addresses[a.index()];
            if a.projection.len != 0 {
                return None;
            }
            match a.base {
                AddressBase::Slot(s) => Some(s),
                AddressBase::Address(a) => whole(u, a),
                _ => None,
            }
        }
        // A mutable loan can change storage behind a reaching name.
        let mut aliased = BTreeSet::new();
        for loan in &u.tables.loans {
            if loan.mutable
                && let Some(s) = whole(u, loan.address)
            {
                aliased.insert(s);
            }
        }
        for arg in &u.pools.call_arguments {
            if let ArgumentValue::Address { address, .. } = arg.value
                && let Some(s) = whole(u, address)
            {
                aliased.insert(s);
            }
        }
        let safe = |a| whole(u, a).filter(|s| !aliased.contains(s));
        let mut writes: Vec<_> = u
            .slots
            .iter()
            .map(|s| usize::from(matches!(s.kind, SlotKind::Parameter(_))))
            .collect();
        for inst in &u.instructions {
            if let Inst::Store { address, .. } | Inst::Replace { address, .. } = *inst
                && let Some(s) = whole(u, address)
            {
                writes[s.index()] += 1;
            }
        }
        let mut initialized_at_entry = BTreeSet::new();
        if !u.tables.edges.iter().any(|edge| edge.target == u.entry) {
            for inst in u.blocks[u.entry.index()]
                .instructions
                .get(&u.instructions)
                .unwrap_or(&[])
            {
                if let Inst::Store { address, .. } = *inst
                    && let Some(slot) = whole(u, address)
                {
                    initialized_at_entry.insert(slot);
                }
            }
        }
        let stable = writes
            .iter()
            .enumerate()
            .map(|(i, &n)| {
                n == 1
                    && !aliased.contains(&SlotId(i as u32))
                    && (matches!(u.slots[i].kind, SlotKind::Parameter(_))
                        || initialized_at_entry.contains(&SlotId(i as u32)))
            })
            .collect();
        let mut source = vec![Source::Unknown; u.values.len()];
        let mut reads = vec![None; u.values.len()];
        for edge in &u.tables.edges {
            for (&to, &from) in u.blocks[edge.target.index()]
                .parameters
                .get(&u.pools.values)
                .unwrap_or(&[])
                .iter()
                .zip(edge.arguments.get(&u.pools.values).unwrap_or(&[]))
            {
                if !matches!(source[to.index()], Source::Flow(_)) {
                    source[to.index()] = Source::Flow(Vec::new());
                }
                if let Source::Flow(xs) = &mut source[to.index()] {
                    xs.push(from);
                }
            }
        }
        for inst in &u.instructions {
            match *inst {
                Inst::Read {
                    result,
                    address,
                    reaching,
                    ..
                }
                | Inst::Copy {
                    result,
                    address,
                    reaching,
                    ..
                } if safe(address).is_some() => {
                    let layout = u.types[u.values[result.index()].ty.index()].layout;
                    if table
                        .layout(layout)
                        .is_some_and(|r| r.copy != CopyRecipe::FlatBytes)
                    {
                        source[result.index()] = Source::Flow(vec![reaching]);
                        reads[result.index()] = safe(address);
                    }
                }
                Inst::Store {
                    address,
                    value,
                    version,
                }
                | Inst::Replace {
                    address,
                    value,
                    version,
                    ..
                } if safe(address).is_some() => source[version.index()] = Source::Flow(vec![value]),
                Inst::Recast { result, value } => {
                    source[result.index()] = Source::Flow(vec![value])
                }
                Inst::Construct {
                    result, construct, ..
                } => source[result.index()] = Source::Made(construct),
                _ => {}
            }
        }
        for (i, call) in u.tables.calls.iter().enumerate() {
            if let Some(v) = call.result {
                source[v.index()] = Source::Call(i);
            }
        }
        Self {
            u,
            table,
            source,
            reads,
            stable,
            parameters: u
                .parameters
                .iter()
                .enumerate()
                .map(|(i, &v)| (v, i as u32))
                .collect(),
        }
    }
    fn arena(&self, site: &ConstructSite) -> Option<SlotId> {
        match site.placement {
            Some(Placement::Fixed(StorageOwner::Arena(a))) => {
                Some(self.u.tables.arenas[a.index()].slot)
            }
            None => match site.destination {
                DestinationClass::Arena(s) => Some(s),
                _ => None,
            },
            _ => None,
        }
    }
    fn proves(
        &self,
        slot: SlotId,
        targets: &BTreeMap<(hirsel_syntax::DefPath, hirsel_syntax::ContentHash), usize>,
        claims: &[BTreeSet<u32>],
        requests: &[(ValueId, usize)],
    ) -> BTreeMap<usize, bool> {
        // Three independent questions per value: root, inline credits, token.
        struct Query<'a, 'b, S> {
            d: &'a Discovery<'b, S>,
            slot: SlotId,
            targets: &'a BTreeMap<(hirsel_syntax::DefPath, hirsel_syntax::ContentHash), usize>,
            claims: &'a [BTreeSet<u32>],
            memo: BTreeMap<usize, bool>,
            active: BTreeSet<usize>,
        }
        impl<S> Query<'_, '_, S> {
            fn test(&mut self, v: ValueId, p: usize) -> bool {
                let at = v.index() * 3 + p;
                if let Some(&b) = self.memo.get(&at) {
                    return b;
                }
                if !self.active.insert(at) {
                    return false;
                }
                let d = self.d;
                let ty = &d.u.types[d.u.values[v.index()].ty.index()];
                let no_drop = d
                    .table
                    .layout(ty.layout)
                    .is_some_and(|r| r.drop.is_empty() && r.finalizer.is_none());
                let answer = if p == 1 && ty.repr == ValueRepr::Block {
                    self.test(v, 0)
                } else if p == 1 && no_drop {
                    true
                } else if p == 2
                    && (d.reads[v.index()] == Some(self.slot)
                        || d.parameters.get(&v).is_some_and(|&p| {
                            d.u.slots[self.slot.index()].kind == SlotKind::Parameter(p)
                        }))
                {
                    true
                } else {
                    match &d.source[v.index()] {
                        Source::Flow(xs) => !xs.is_empty() && xs.iter().all(|&x| self.test(x, p)),
                        Source::Made(k) => {
                            let site = &d.u.tables.constructs[k.index()];
                            if p == 0 {
                                ty.repr == ValueRepr::Block && d.arena(site) == Some(self.slot)
                            } else if p == 1 && ty.repr == ValueRepr::Aggregate {
                                d.table
                                    .layout(site.layout)
                                    .is_some_and(|r| r.finalizer.is_none())
                                    && site
                                        .fields
                                        .get(&d.u.pools.fields)
                                        .unwrap_or(&[])
                                        .iter()
                                        .all(|f| self.test(f.value, 1))
                            } else {
                                false
                            }
                        }
                        Source::Call(i) if p == 0 => {
                            let call = &d.u.tables.calls[*i];
                            let params: Vec<_> = if let Callee::Direct(s) = call.callee {
                                let s = &d.u.tables.symbols[s.index()];
                                s.instance
                                    .and_then(|hash| {
                                        self.targets.get(&(s.definition.clone(), hash))
                                    })
                                    .map(|&i| self.claims[i].iter().copied().collect())
                                    .unwrap_or_else(|| {
                                        d.table.fn_abis[call.abi.0 as usize]
                                            .result_arena
                                            .into_iter()
                                            .collect()
                                    })
                            } else {
                                Vec::new()
                            };
                            params.into_iter().any(|parameter| {
                                call.arguments
                                    .get(&d.u.pools.call_arguments)
                                    .unwrap_or(&[])
                                    .iter()
                                    .find(|a| a.parameter == parameter)
                                    .is_some_and(|a| match a.value {
                                        ArgumentValue::Value(value) => self.test(value, 2),
                                        ArgumentValue::Address { .. } => false,
                                    })
                            })
                        }
                        _ => false,
                    }
                };
                self.active.remove(&at);
                self.memo.insert(at, answer);
                answer
            }
        }
        let mut q = Query {
            d: self,
            slot,
            targets,
            claims,
            memo: BTreeMap::new(),
            active: BTreeSet::new(),
        };
        for &(value, property) in requests {
            q.test(value, property);
        }
        q.memo
    }
}

/// Derive every function's claim by assume-then-verify. Removing an invalid
/// claim revisits its callers, including recursive strongly connected groups.
pub fn summarize(units: &mut [LirUnit<Value>], table: &mut LayoutTable) {
    let targets: BTreeMap<_, _> = units
        .iter()
        .enumerate()
        .map(|(i, u)| ((u.header.definition.clone(), u.header.instance), i))
        .collect();
    let discovery: Vec<_> = units.iter().map(|u| Discovery::new(u, table)).collect();
    let mut claims: Vec<BTreeSet<u32>> = discovery
        .iter()
        .map(|d| {
            let abi = &table.fn_abis[d.u.header.abi.0 as usize];
            if !abi.result.as_ref().is_some_and(|r| {
                table.layout(r.layout).is_some_and(|r| {
                    matches!(r.kind, LayoutKind::Block { .. } | LayoutKind::Box { .. })
                })
            }) {
                return BTreeSet::new();
            }
            d.u.slots
                .iter()
                .enumerate()
                .filter_map(|(i, s)| match s.kind {
                    SlotKind::Parameter(p)
                        if d.stable[i]
                            && table
                                .layout(d.u.types[s.ty.index()].layout)
                                .is_some_and(|r| r.kind == LayoutKind::Arena)
                            && abi.parameters.iter().any(|a| {
                                a.source == p
                                    && a.convention == hirsel_core_contract::Passing::Borrow
                            }) =>
                    {
                        Some(p)
                    }
                    _ => None,
                })
                .collect()
        })
        .collect();
    let mut callers = vec![BTreeSet::new(); units.len()];
    for (i, u) in units.iter().enumerate() {
        for call in &u.tables.calls {
            if let Callee::Direct(s) = call.callee {
                let s = &u.tables.symbols[s.index()];
                if let Some(&callee) = s
                    .instance
                    .and_then(|h| targets.get(&(s.definition.clone(), h)))
                {
                    callers[callee].insert(i);
                }
            }
        }
    }
    for choose in [false, true] {
        if choose {
            for xs in &mut claims {
                if let Some(p) = xs.first().copied() {
                    *xs = BTreeSet::from([p]);
                }
            }
        }
        let mut queue: VecDeque<_> = (0..units.len()).collect();
        let mut queued = vec![true; units.len()];
        while let Some(i) = queue.pop_front() {
            queued[i] = false;
            let d = &discovery[i];
            let invalid: Vec<_> = claims[i]
                .iter()
                .copied()
                .filter(|&p| {
                    let slot = SlotId(
                        d.u.slots
                            .iter()
                            .position(|s| s.kind == SlotKind::Parameter(p))
                            .unwrap() as u32,
                    );
                    let requests: Vec<_> =
                        d.u.terminators
                            .iter()
                            .filter_map(|t| match t {
                                Terminator::Return(Some(v)) => Some((*v, 0)),
                                _ => None,
                            })
                            .collect();
                    let proof = d.proves(slot, &targets, &claims, &requests);
                    d.u.terminators.iter().any(|t| match t {
                        Terminator::Return(Some(v)) => proof.get(&(v.index() * 3)) != Some(&true),
                        Terminator::Return(None) => true,
                        _ => false,
                    })
                })
                .collect();
            if !invalid.is_empty() {
                for p in invalid {
                    claims[i].remove(&p);
                }
                for &caller in &callers[i] {
                    if !queued[caller] {
                        queue.push_back(caller);
                        queued[caller] = true;
                    }
                }
            }
        }
    }
    drop(discovery);
    let abis: Vec<_> = units
        .iter_mut()
        .enumerate()
        .map(|(i, u)| {
            if claims[i].is_empty() {
                return u.header.abi;
            }
            let mut abi = table.fn_abis[u.header.abi.0 as usize].clone();
            abi.result_arena = claims[i].first().copied();
            let id = FnAbiId(table.fn_abis.len() as u32);
            table.fn_abis.push(abi);
            u.header.abi = id;
            id
        })
        .collect();
    for u in units.iter_mut() {
        for call in &mut u.tables.calls {
            if let Callee::Direct(s) = call.callee {
                let s = &u.tables.symbols[s.index()];
                if let Some(&i) = s
                    .instance
                    .and_then(|h| targets.get(&(s.definition.clone(), h)))
                    && !claims[i].is_empty()
                {
                    call.abi = abis[i];
                }
            }
        }
    }
    let digest = table.digest();
    for u in units {
        u.header.layouts = digest;
    }
}

/// Placement is final before the construction flag is selected.
pub fn elide(unit: &mut LirUnit<Placed>, table: &LayoutTable) {
    let d = Discovery::new(unit, table);
    let mut requests: BTreeMap<SlotId, Vec<(ValueId, usize)>> = BTreeMap::new();
    for site in &unit.tables.constructs {
        if let Some(slot) = d.arena(site).filter(|s| d.stable[s.index()]) {
            requests.entry(slot).or_default().extend(
                site.fields
                    .get(&unit.pools.fields)
                    .unwrap_or(&[])
                    .iter()
                    .map(|f| (f.value, 1)),
            );
        }
    }
    let proofs: BTreeMap<_, _> = requests
        .into_iter()
        .map(|(s, requests)| (s, d.proves(s, &BTreeMap::new(), &[], &requests)))
        .collect();
    let mut flags = vec![false; unit.tables.constructs.len()];
    for inst in &unit.instructions {
        if let Inst::Construct {
            result, construct, ..
        } = *inst
            && unit.types[unit.values[result.index()].ty.index()].repr == ValueRepr::Block
        {
            let site = &unit.tables.constructs[construct.index()];
            if let Some(slot) = d.arena(site)
                && d.stable[slot.index()]
            {
                let proof = &proofs[&slot];
                // A tracked block stays writable (MEM-6): a later write, such
                // as a Map's rebuilt index, stores a child no proof covers.
                flags[construct.index()] = !site.tracked
                    && table
                        .layout(site.layout)
                        .is_some_and(|r| r.finalizer.is_none())
                    && site
                        .fields
                        .get(&unit.pools.fields)
                        .unwrap_or(&[])
                        .iter()
                        .all(|f| proof.get(&(f.value.index() * 3 + 1)) == Some(&true));
            }
        }
    }
    for (site, flag) in unit.tables.constructs.iter_mut().zip(flags) {
        if !matches!(
            site.placement,
            Some(Placement::Fixed(StorageOwner::LocalRegion(_)))
        ) {
            site.arena_no_drop = flag;
        }
    }
}

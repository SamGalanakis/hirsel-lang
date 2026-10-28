//! Descriptive custody at every possible physical save. Slots are may-live;
//! Ready's masks govern initialization. Runtime custody binds allocation
//! generations and never dereferences an uninitialized slot from this map.
use super::host_return::LiveOwners;
use super::*;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CpuCaptureRoot {
    pub slot: SlotId,
    pub layout: LayoutId,
    pub offset: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub enum CpuSavePoint {
    Instruction(usize),
    Terminator(BlockId),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CpuSaveRoots {
    pub input: CpuCaptureRoot,
    pub mapper: CpuCaptureRoot,
    pub result: (ValueId, LayoutId),
    pub frames: BTreeMap<ContentHash, BTreeMap<CpuSavePoint, LiveOwners>>,
    pub parents: BTreeMap<(ContentHash, CallId), ContentHash>,
}

pub(super) fn propose(
    units: &[&ValidatedUnit<Ready>],
    table: &LayoutTable,
    graph: &ScalarGraph,
    protocol: &CpuWorkerProtocol,
) -> Option<CpuSaveRoots> {
    let worker = units
        .iter()
        .find(|u| u.unit().header.instance == protocol.worker)?
        .unit();
    let wrapper = table.layout(protocol.environment)?;
    let root = |slot, field: usize| CpuCaptureRoot {
        slot,
        layout: wrapper.fields[field].layout,
        offset: wrapper.fields[field].offset,
    };
    let mut frames = BTreeMap::new();
    let mut parents = BTreeMap::new();
    for id in graph.bodies.keys() {
        let u = units
            .iter()
            .find(|u| u.unit().header.instance == *id)?
            .unit();
        frames.insert(*id, frame(u));
        for term in &u.terminators {
            if let Terminator::Call { call, .. } = *term {
                let callee = if *id == protocol.worker && call == protocol.mapper {
                    *graph.bodies[id].calls.iter().next()?
                } else {
                    let site = &u.tables.calls[call.index()];
                    match site.callee {
                        Callee::Direct(s) | Callee::Bound { code: s, .. } => {
                            u.tables.symbols[s.index()].instance?
                        }
                        Callee::Indirect(v) => super::plan::known_code(u, v, &[], false)?,
                        // Worker protocol operations run on the canonical stack.
                        Callee::Operation(_) if *id == protocol.worker => continue,
                        _ => return None,
                    }
                };
                if !graph.bodies[id].calls.contains(&callee) {
                    return None;
                }
                parents.insert((*id, call), callee);
            }
        }
    }
    Some(CpuSaveRoots {
        input: root(protocol.jobs, 1),
        mapper: root(protocol.shared_mapper, 0),
        result: (
            worker.tables.calls[protocol.mapper.index()].result?,
            protocol.result_layout,
        ),
        frames,
        parents,
    })
}

fn frame(u: &LirUnit<Ready>) -> BTreeMap<CpuSavePoint, LiveOwners> {
    let live = crate::liveness::liveness(u);
    let storage = storage(u);
    let mut out = BTreeMap::new();
    for (b, block) in u.blocks.iter().enumerate() {
        let at = BlockId(b as u32);
        let mut values = live.successor_needs(u, at);
        for d in crate::liveness::terminator_defs(u, &u.terminators[b]) {
            values.remove(d);
        }
        for v in crate::liveness::terminator_uses(u, &u.terminators[b]) {
            values.insert(v);
        }
        let mut record = |point, values: &crate::liveness::ValueSet| {
            let (slots, loans) = &storage[&point];
            out.insert(
                point,
                LiveOwners {
                    at,
                    slots: slots
                        .iter()
                        .map(|s| (*s, u.types[u.slots[s.index()].ty.index()].layout))
                        .collect(),
                    values: values
                        .iter()
                        .map(|v| {
                            (
                                v,
                                u.types[u.values[v.index()].ty.index()].layout,
                                u.values[v.index()].holding,
                            )
                        })
                        .collect(),
                    loans: loans.iter().copied().collect(),
                },
            );
        };
        record(CpuSavePoint::Terminator(at), &values);
        let insts = block.instructions.get(&u.instructions).unwrap_or(&[]);
        for (i, inst) in insts.iter().enumerate().rev() {
            for d in inst.defs() {
                values.remove(d);
            }
            for v in crate::liveness::inst_uses(u, inst) {
                values.insert(v);
            }
            if let Some(f) = inst.failure() {
                let failure = &u.tables.failures[f.index()];
                let edge = &u.tables.edges[failure.cleanup.index()];
                let mut needed = live.live_in[edge.target.index()].clone();
                for p in u.blocks[edge.target.index()]
                    .parameters
                    .get(&u.pools.values)
                    .unwrap_or(&[])
                {
                    needed.remove(*p);
                }
                for v in edge
                    .arguments
                    .get(&u.pools.values)
                    .unwrap_or(&[])
                    .iter()
                    .chain(failure.dispose.get(&u.pools.values).unwrap_or(&[]))
                {
                    needed.insert(*v);
                }
                values.union(&needed);
            }
            record(
                CpuSavePoint::Instruction(block.instructions.start as usize + i),
                &values,
            );
        }
    }
    out
}

type Storage = (BTreeSet<SlotId>, BTreeSet<LoanId>);

/// Forward union over normal and instruction/call failure edges. Separate
/// from the import checker's storage walk in continuation::validate.
fn storage(u: &LirUnit<Ready>) -> BTreeMap<CpuSavePoint, Storage> {
    let mut entries = vec![Storage::default(); u.blocks.len()];
    entries[u.entry.index()]
        .0
        .extend(u.slots.iter().enumerate().filter_map(|(i, s)| {
            matches!(
                s.kind,
                SlotKind::Parameter(_) | SlotKind::Capture(_) | SlotKind::ReturnSlot
            )
            .then_some(SlotId(i as u32))
        }));
    let mut reached = BTreeSet::from([u.entry]);
    let mut work = vec![u.entry];
    let mut out: BTreeMap<CpuSavePoint, Storage> = BTreeMap::new();
    while let Some(b) = work.pop() {
        let mut state = entries[b.index()].clone();
        let mut edges = Vec::new();
        let mut record = |point, state: &Storage| {
            let row = out.entry(point).or_default();
            row.0.extend(&state.0);
            row.1.extend(&state.1);
        };
        for (i, inst) in u.blocks[b.index()]
            .instructions
            .get(&u.instructions)
            .unwrap_or(&[])
            .iter()
            .enumerate()
        {
            record(
                CpuSavePoint::Instruction(u.blocks[b.index()].instructions.start as usize + i),
                &state,
            );
            if let Some(f) = inst.failure() {
                edges.push((u.tables.failures[f.index()].cleanup, state.clone()));
            }
            match *inst {
                Inst::StorageLive { slot } => {
                    state.0.insert(slot);
                }
                Inst::StorageDead { slot, .. } => {
                    state.0.remove(&slot);
                }
                Inst::BeginLoan { loan, .. } => {
                    state.1.insert(loan);
                }
                Inst::EndLoan { loan, .. } => {
                    state.1.remove(&loan);
                }
                _ => {}
            }
        }
        record(CpuSavePoint::Terminator(b), &state);
        let term = &u.terminators[b.index()];
        edges.extend(
            term.normal_edges(&u.pools)
                .into_iter()
                .map(|e| (e, state.clone())),
        );
        edges.extend(
            term.failure_edges(&u.tables)
                .into_iter()
                .map(|f| (u.tables.failures[f.index()].cleanup, state.clone())),
        );
        for (edge, state) in edges {
            let next = u.tables.edges[edge.index()].target;
            let row = &mut entries[next.index()];
            let old = (row.0.len(), row.1.len());
            row.0.extend(state.0);
            row.1.extend(state.1);
            if reached.insert(next) || old != (row.0.len(), row.1.len()) {
                work.push(next);
            }
        }
    }
    out
}

pub(super) fn check(
    units: &[&ValidatedUnit<Ready>],
    table: &LayoutTable,
    graph: &ScalarGraph,
    protocol: &CpuWorkerProtocol,
    roots: &CpuSaveRoots,
) -> bool {
    let Some(wrapper) = table.layout(protocol.environment) else {
        return false;
    };
    if roots.input
        != (CpuCaptureRoot {
            slot: protocol.jobs,
            layout: wrapper.fields[1].layout,
            offset: wrapper.fields[1].offset,
        })
        || roots.mapper
            != (CpuCaptureRoot {
                slot: protocol.shared_mapper,
                layout: wrapper.fields[0].layout,
                offset: wrapper.fields[0].offset,
            })
        || roots.frames.len() != graph.bodies.len()
    {
        return false;
    }
    let Some(worker) = units
        .iter()
        .find(|u| u.unit().header.instance == protocol.worker)
    else {
        return false;
    };
    if Some(roots.result.0) != worker.unit().tables.calls[protocol.mapper.index()].result
        || roots.result.1 != protocol.result_layout
    {
        return false;
    }
    let mut parents = BTreeMap::new();
    for id in graph.bodies.keys() {
        let Some(validated) = units.iter().find(|u| u.unit().header.instance == *id) else {
            return false;
        };
        let u = validated.unit();
        if roots.frames.get(id) != Some(&crate::continuation::cpu_saved_frames(u)) {
            return false;
        }
        for (b, term) in u.terminators.iter().enumerate() {
            let Terminator::Call { call, .. } = *term else {
                continue;
            };
            let site = &u.tables.calls[call.index()];
            if *id == protocol.worker && call != protocol.mapper {
                continue;
            }
            let callee = if *id == protocol.worker {
                graph.bodies[id].calls.iter().next().copied()
            } else {
                match site.callee {
                    Callee::Direct(s) | Callee::Bound { code: s, .. } => {
                        u.tables.symbols[s.index()].instance
                    }
                    Callee::Indirect(v) => super::plan::known_code(u, v, &[], false),
                    _ => None,
                }
            };
            let Some(callee) = callee else { return false };
            if !graph.bodies[id].calls.contains(&callee)
                || !roots.frames[id].contains_key(&CpuSavePoint::Terminator(BlockId(b as u32)))
            {
                return false;
            }
            parents.insert((*id, call), callee);
        }
    }
    roots.parents == parents
}

//! Cleanup lowering: the semantic `Drop`s and `Copy`s of an Rc
//! or Reuse unit become Ready operations.
//!
//! A drop keeps its place in TIR's cleanup chain: the import already expanded
//! each chain into cleanup blocks in the nested `CleanupOrder` (scopes
//! innermost first, temporaries before names, kept temporaries as named
//! locals declared just before their binding), so lowering a drop in place
//! reorders nothing observable. Each drop becomes a sequence of actions over
//! the content still in its slot:
//!
//! - **Release.** Content with no finalizer reachable inline is taken whole and
//!   released by its layout's drop program (`Release` of a block, or of an
//!   inline value at its own layout).
//! - **Finalize.** An inline resource follows `Live -> Finalizing ->
//!   FieldsPending -> Dead`: `FinalizeBegin`, a call of its drop body with the
//!   resource borrowed, `FinalizeEnd` on the normal and the failure edge,
//!   then its owning fields released high to low; the slot then holds nothing
//!   owned and its storage ends.
//! - An aggregate holding a resource inline, or content with parts already
//!   moved out, is decomposed into its fields in drop order. A variant with a
//!   part taken is decomposed into the fields of the variant the take showed
//!   active, through `VariantField` addresses prepared beside the take's own
//!   (they check the same tag while the content is still whole).
//!
//! Failure is accumulated, never early-returned: when an action fails, a
//! failure-cleanup chain runs every remaining action of the drop (each in its
//! own block, so the chain is linear) and then continues into the drop's own
//! failure successor, the rest of TIR's chain. The primary failure stays
//! primary; the runtime extends the cause chain.
//!
//! A `Copy` whose recipe is Bits becomes `Read Bits` and one whose recipe is
//! FlatBytes a `FlatCopy`; a Dup or Fields copy is the Ready primitive and
//! stays.
//!
//! The slot states this module reads (initialization, moved parts, reaching
//! versions) come from insertion's forward analysis; the Ready validator
//! replays ownership independently.
use crate::PassError;
use crate::insert::{Editor, Facts, Flow, Lay, Place, Root, State, Step, Tri, Versions, type_of};
use crate::instance::Instances;
use crate::paths::{Paths, Tris};
use hirsel_core_contract::Passing;
use hirsel_intern::work::{self, Counter};
use hirsel_lir::abi_types::FnAbiId;
use hirsel_lir::ir::{
    AddressBase, AddressId, AddressInfo, ArgumentValue, BlockId, BlockKind, CallArgument, CallId,
    CallSite, Callee, CleanupAnchorId, EdgeId, FailureEdge, FailureId, Holding, Inst, LirUnit,
    LoanId, LoanInfo, MaskRef, PoolRange, ProjectionStep, ReadMode, SlotId, Symbol, SymbolId,
    SymbolKind, Terminator, ValueId, ValueRepr,
};
use hirsel_lir::layout_types::{CopyRecipe, LayoutId, LayoutKind, LayoutTable};
use hirsel_lir::stage::{Ready, Stage, StageKind};
use hirsel_syntax::{ContentHash, DefPath};
use std::collections::{BTreeMap, BTreeSet};

mod observation;
use observation::{NoObservations, Reporter};

/// The drop body of one resource layout: its instance and `FnAbi`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Finalizer {
    pub origin: hirsel_lir::ir::UnitOrigin,
    pub definition: DefPath,
    pub instance: ContentHash,
    pub abi: FnAbiId,
}

/// Every resource layout's drop body, keyed by the resource's layout.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Finalizers(pub BTreeMap<LayoutId, Finalizer>);

impl Finalizers {
    /// The finalizer instances of a program, from its plan alone (design §1e): each takes
    /// its resource borrowed as its first parameter, whose `FnAbi` layout names the
    /// resource. No unit is read, so a unit's cleanup needs no other unit's RC output.
    pub fn planned(instances: &Instances, table: &LayoutTable) -> Self {
        let mut out = BTreeMap::new();
        for id in instances.ids() {
            let role = &instances.roles[id.index()];
            if !role.is_finalizer() {
                continue;
            }
            let abi = instances.abis[id.index()];
            let Some(parameter) = table.fn_abis[abi.0 as usize].parameters.first() else {
                continue;
            };
            let view = instances.view(id);
            out.insert(
                parameter.layout,
                Finalizer {
                    origin: if role.is_library() {
                        hirsel_lir::ir::UnitOrigin::Library
                    } else {
                        hirsel_lir::ir::UnitOrigin::Source
                    },
                    definition: view.key.definition().clone(),
                    instance: view.key.digest(),
                    abi,
                },
            );
        }
        Self(out)
    }

    /// The finalizers cleanup lowering of `unit` can call: those of the layouts it
    /// reaches through the table, by layout.
    pub fn reached<S: Clone>(
        &self,
        unit: &LirUnit<S>,
        table: &LayoutTable,
    ) -> Vec<(LayoutId, &Finalizer)> {
        let (_, roots, _) = hirsel_lir::objects::erased(unit);
        let mut seen = BTreeSet::new();
        let mut pending = roots;
        while let Some(id) = pending.pop() {
            if !seen.insert(id) {
                continue;
            }
            if let Some(record) = table.layout(id) {
                pending.extend(record.references());
            }
        }
        seen.into_iter()
            .filter_map(|id| self.0.get(&id).map(|f| (id, f)))
            .collect()
    }

    /// The finalizer facts cleanup lowering of `unit` can read, by content, for the object
    /// stage's key: per finalizer it `reached`, the layout's `LayoutKey`, the finalizer's
    /// instance and its FnAbi key, in key order.
    pub fn facts<S: Clone>(
        &self,
        unit: &LirUnit<S>,
        table: &LayoutTable,
        keys: &hirsel_lir::LayoutKeys,
    ) -> Vec<u8> {
        let mut facts: Vec<[u8; 96]> = self
            .reached(unit, table)
            .into_iter()
            .filter_map(|(id, finalizer)| {
                let abi = keys.fn_abi_of(table, finalizer.abi)?;
                let mut fact = [0; 96];
                fact[..32].copy_from_slice(&keys.key(id).0.0);
                fact[32..64].copy_from_slice(&finalizer.instance.0);
                fact[64..].copy_from_slice(&abi.0);
                Some(fact)
            })
            .collect();
        facts.sort_unstable();
        facts.concat()
    }

    /// The finalizer instances of a program read from its units: `planned` must equal it.
    pub fn new<S: Stage>(instances: &Instances, units: &[LirUnit<S>]) -> Self {
        let mut out = BTreeMap::new();
        for id in instances.ids() {
            if !instances.roles[id.index()].is_finalizer() {
                continue;
            }
            let unit = &units[id.index()];
            let Some(&self_value) = unit.parameters.first() else {
                continue;
            };
            let ty = unit.values[self_value.index()].ty;
            out.insert(
                unit.types[ty.index()].layout,
                Finalizer {
                    origin: unit.header.origin,
                    definition: unit.header.definition.clone(),
                    instance: unit.header.instance,
                    abi: instances.abis[id.index()],
                },
            );
        }
        Self(out)
    }
}

/// Lower an Rc or Reuse unit's semantic drops and copies; the result still
/// has to pass the Ready validator.
pub fn lower<S: Stage>(
    unit: LirUnit<S>,
    table: &LayoutTable,
    finalizers: &Finalizers,
) -> Result<LirUnit<Ready>, PassError> {
    lower_indexed(unit, finalizers, &hirsel_lir::LayoutIndex::new(table))
}

pub fn lower_indexed<S: Stage>(
    unit: LirUnit<S>,
    finalizers: &Finalizers,
    index: &hirsel_lir::LayoutIndex<'_>,
) -> Result<LirUnit<Ready>, PassError> {
    lower_reporting(unit, finalizers, index, NoObservations).map(|(unit, ())| unit)
}

/// Checked cleanup lowering with pass-authored splice locations. Unsupported
/// compound drops fail; coordinate-preserving version repairs use a checked
/// identity recipe rather than rebuilding observations.
pub fn lower_observed<S: Stage>(
    unit: LirUnit<S>,
    finalizers: &Finalizers,
    index: &hirsel_lir::LayoutIndex<'_>,
    input: &hirsel_lir::ownership_trace::Plan,
) -> Result<
    (
        LirUnit<Ready>,
        hirsel_lir::ownership_trace::Plan,
        hirsel_lir::ownership_trace::Recipe,
    ),
    String,
> {
    hirsel_lir::ownership_trace::check(&unit, input).map_err(|error| error.to_string())?;
    let before = unit.clone();
    let (after, mut output) = lower_reporting(
        unit,
        finalizers,
        index,
        observation::Observations::new(input),
    )
    .map_err(|error| error.to_string())?;
    output.unit = after.digest();
    let recipe = hirsel_lir::ownership_trace::Recipe {
        before: input.unit,
        after: output.unit,
        relocations: output
            .observations
            .iter()
            .map(|observation| hirsel_lir::ownership_trace::Relocation {
                id: observation.marker.id,
                point: observation.point,
                token: observation.marker.token,
                owner: observation.marker.owner,
            })
            .collect(),
    };
    hirsel_lir::ownership_trace::check_transform(&before, input, &after, &output, Some(&recipe))
        .map_err(|error| error.to_string())?;
    Ok((after, output, recipe))
}

fn lower_reporting<S: Stage, R: Reporter>(
    unit: LirUnit<S>,
    finalizers: &Finalizers,
    index: &hirsel_lir::LayoutIndex<'_>,
    mut report: R,
) -> Result<(LirUnit<Ready>, R::Output), PassError> {
    if !matches!(S::KIND, StageKind::Rc | StageKind::Reuse) {
        return Err(PassError::Malformed(format!(
            "cleanup lowering reads Rc or Reuse LIR, not {:?}",
            S::KIND
        )));
    }
    let table = index.table();
    let lay = Lay(table);
    let unit = unit.into_stage::<Ready>();
    let mut facts = Facts::new(&unit, table);
    let mut ed = Editor::new(unit);
    // Reuse the fixed point until flag insertion changes the unit, as rustc's
    // ElaborateDrops retains ResultsCursors while constructing its MirPatch
    // (compiler/rustc_mir_transform/src/elaborate_drops.rs:64-94,123-125).
    // Hirsel also repairs SSA versions; each repair reruns its transfer before
    // sharing the final result. Ready validation still re-derives its own facts.
    let inputs = join_versions(&mut ed, &mut facts, lay, false)?;
    let before_flags = instruction_states_from(&ed, &facts, lay, false, inputs);
    let drops: Vec<_> = match join_flags(&mut ed, &facts, lay, index, &before_flags, &mut report)? {
        None => before_flags
            .into_iter()
            .map(|(b, i, st)| (b, i, st, Flags::new()))
            .collect(),
        Some(flags) => drop_states(&ed, &facts, lay)
            .into_iter()
            .map(|(b, i, st)| (b, i, st, flags.get(&(b, i)).cloned().unwrap_or_default()))
            .collect(),
    };
    lower_copies(&mut ed, lay)?;
    report.prelude(&ed)?;
    let mut lowering = Lowering {
        index,
        ed: &mut ed,
        facts: &mut facts,
        lay,
        finalizers,
        symbols: BTreeMap::new(),
        inserted: Vec::new(),
        prepared: Vec::new(),
        flags: BTreeMap::new(),
        retire_flags: BTreeMap::new(),
        drop_at: BlockId(0),
    };
    // Later drops of a block first, so splitting a block leaves the
    // indices of its earlier drops in place; a preparation inserted before
    // an earlier drop moves it one place on.
    let mut drops = drops;
    while let Some((b, i, st, flag)) = drops.pop() {
        lowering.drop(b, i, &st, flag, &mut report)?;
        for (block, at) in lowering.inserted.drain(..) {
            for d in drops.iter_mut().filter(|d| d.0 == block && d.1 >= at) {
                d.1 += 1;
            }
        }
    }
    {
        drop(lowering);
        let mut snapshot = ed.unit.clone();
        snapshot.instructions.clear();
        for (block, code) in snapshot.blocks.iter_mut().zip(&ed.code) {
            block.instructions = PoolRange::push(&mut snapshot.instructions, code.iter().cloned());
        }
        let mut facts = Facts::new(&snapshot, table);
        report.versions(&ed, true)?;
        join_read_versions(&mut ed, &mut facts, lay)?;
        retie_versions(&mut ed, &facts, lay);
        report.versions(&ed, false)?;
    }
    let (mut unit, output) = report.finish(ed)?;
    prune_unused_flags(&mut unit);
    Ok((unit, output))
}

/// Whole-value releases can make a semantic drop's field flags unnecessary.
/// Keep flags read by code or forwarded to a flag it reads, including cycles.
/// Unused metadata must not assert an unobserved payload's presence on an edge.
fn prune_unused_flags(unit: &mut LirUnit<Ready>) {
    let flags: BTreeSet<ValueId> = unit.tables.init_flags.iter().map(|f| f.value).collect();
    if flags.is_empty() {
        return;
    }
    let mut used: BTreeSet<ValueId> = unit
        .instructions
        .iter()
        .flat_map(|i| crate::liveness::inst_uses(unit, i))
        .chain(
            unit.terminators
                .iter()
                .flat_map(|t| crate::liveness::terminator_uses(unit, t)),
        )
        .chain(unit.parameters.iter().copied())
        .chain(unit.captures.iter().copied())
        .chain(unit.tables.failures.iter().flat_map(|f| {
            f.dispose
                .get(&unit.pools.values)
                .unwrap_or(&[])
                .iter()
                .copied()
        }))
        .filter(|v| flags.contains(v))
        .collect();
    let mut inputs: BTreeMap<ValueId, Vec<ValueId>> = BTreeMap::new();
    for edge in &unit.tables.edges {
        let params = unit.blocks[edge.target.index()]
            .parameters
            .get(&unit.pools.values)
            .unwrap_or(&[]);
        let args = edge.arguments.get(&unit.pools.values).unwrap_or(&[]);
        for (&param, &arg) in params.iter().zip(args) {
            if flags.contains(&arg) {
                if flags.contains(&param) {
                    inputs.entry(param).or_default().push(arg);
                } else {
                    used.insert(arg);
                }
            }
        }
    }
    let mut pending: Vec<ValueId> = used.iter().copied().collect();
    while let Some(flag) = pending.pop() {
        for &arg in inputs.get(&flag).into_iter().flatten() {
            if used.insert(arg) {
                pending.push(arg);
            }
        }
    }
    let removed: BTreeSet<ValueId> = flags.difference(&used).copied().collect();
    if removed.is_empty() {
        return;
    }
    let keep: Vec<Vec<bool>> = unit
        .blocks
        .iter()
        .map(|b| {
            b.parameters
                .get(&unit.pools.values)
                .unwrap_or(&[])
                .iter()
                .map(|v| !removed.contains(v))
                .collect()
        })
        .collect();
    for edge in &mut unit.tables.edges {
        let args: Vec<ValueId> = edge
            .arguments
            .get(&unit.pools.values)
            .unwrap_or(&[])
            .iter()
            .zip(&keep[edge.target.index()])
            .filter_map(|(&v, &keep)| keep.then_some(v))
            .collect();
        edge.arguments = PoolRange::push(&mut unit.pools.values, args);
    }
    for (block, keep) in unit.blocks.iter_mut().zip(keep) {
        let params: Vec<ValueId> = block
            .parameters
            .get(&unit.pools.values)
            .unwrap_or(&[])
            .iter()
            .zip(keep)
            .filter_map(|(&v, keep)| keep.then_some(v))
            .collect();
        block.parameters = PoolRange::push(&mut unit.pools.values, params);
    }
    unit.tables
        .init_flags
        .retain(|f| !removed.contains(&f.value));
    crate::insert::compact_values(unit);
}

/// A completed inline finalization may retain plain bytes and their version.
/// Transport that actual version, rather than the semantic Drop's empty marker.
/// Empty storage retains revision 8's arbitrary declared version marker.
fn retie_versions(ed: &mut Editor<Ready>, facts: &Facts, lay: Lay<'_>) {
    let flow = Flow {
        unit: &ed.unit,
        code: &ed.code,
        lay,
        facts,
    };
    let states = flow.solve();
    let mut replacements = BTreeMap::new();
    for (b, state) in states.into_iter().enumerate() {
        let Some(state) = state else { continue };
        let block = BlockId(b as u32);
        let mut record = |edge: EdgeId, state: &State| {
            let target = ed.unit.tables.edges[edge.index()].target;
            for &(ordinal, slot) in facts.phis_of(target) {
                if let Versions::One(version) = state.slots[slot.index()].versions {
                    replacements.insert((edge, ordinal), version);
                }
            }
        };
        flow.block(block, state, |i, pre, post| {
            if let Some(inst) = ed.code[b].get(i) {
                if let Some(failure) = Inst::failure(inst) {
                    let state = if matches!(
                        inst,
                        Inst::Drop { .. }
                            | Inst::Release { .. }
                            | Inst::Reset { .. }
                            | Inst::RegionExit { .. }
                    ) {
                        post
                    } else {
                        pre
                    };
                    record(ed.unit.tables.failures[failure.index()].cleanup, state);
                }
            } else {
                let term = &ed.unit.terminators[b];
                for edge in Terminator::normal_edges(term, &ed.unit.pools) {
                    record(edge, post);
                }
                for failure in term.failure_edges(&ed.unit.tables) {
                    let state = if let Terminator::Call { call, .. } = term {
                        if ed.unit.tables.calls[call.index()].pretransfer_failure == Some(failure) {
                            pre
                        } else {
                            post
                        }
                    } else {
                        post
                    };
                    record(ed.unit.tables.failures[failure.index()].cleanup, state);
                }
            }
        });
    }
    let mut arguments = BTreeMap::new();
    for ((edge, ordinal), version) in replacements {
        let values = arguments.entry(edge).or_insert_with(|| {
            ed.unit.tables.edges[edge.index()]
                .arguments
                .get(&ed.unit.pools.values)
                .unwrap()
                .to_vec()
        });
        values[ordinal] = version;
    }
    for (edge, values) in arguments {
        ed.unit.tables.edges[edge.index()].arguments =
            PoolRange::push(&mut ed.unit.pools.values, values);
    }
}

/// Give every drop one reaching version of its slot. A drop names no
/// version, so the import added no join where versions meet only at a drop
/// (several `mut` calls' write-backs into one failure cleanup, say); the
/// take that releases the content needs one. Each such block gets a
/// version-join parameter, and each edge into it passes the version that
/// reaches along it, or its source block's own join where several do.
///
/// A drop of content there on some paths only (a drop flag decides it)
/// joins its versions too. An edge holding no content passes any version of
/// the slot that dominates it, as nothing reads it (LIR schema revision 8);
/// where one version reaches alone it need not dominate the drop, since a
/// read's reaching version is a name the validator pins, not an operand.
fn join_versions(
    ed: &mut Editor<Ready>,
    facts: &mut Facts,
    lay: Lay<'_>,
    reads: bool,
) -> Result<Vec<Option<State>>, PassError> {
    // Parameters and arguments change below, but the graph does not.
    let adjacent = Flow {
        unit: &ed.unit,
        code: &ed.code,
        lay,
        facts,
    }
    .successors()
    .into_iter()
    .map(|targets| targets.into_iter().map(|b| b.index()).collect())
    .collect::<Vec<Vec<usize>>>();
    let dom = hirsel_graph::Dominators::new(&adjacent, ed.unit.entry.index());
    let mut incoming = vec![Vec::new(); ed.unit.blocks.len()];
    for (index, edge) in ed.unit.tables.edges.iter().enumerate() {
        incoming[edge.target.index()].push(EdgeId(index as u32));
    }
    let mut defs = version_defs(ed, facts);
    let mut versions = vec![Vec::new(); ed.unit.slots.len()];
    for &value in defs.keys() {
        versions[facts.version_of[value.index()].unwrap().index()].push(value);
    }
    let bound = ed.unit.slots.len() * ed.unit.blocks.len() + 1;
    for _ in 0..bound {
        let inputs = VersionInputs::new(ed, facts, lay, reads);
        if inputs.joins.is_empty() {
            return Ok(inputs.ins);
        }
        // A plain version phi changes only its slot's version transfer, not
        // any other slot's liveness, initialization or version. Distinct slots
        // therefore share this solve and replay. Recompute before joining the
        // same slot again, since its transfer function has now changed.
        for &(block, slot) in &inputs.joins {
            let reaches = |v: ValueId, b: BlockId, point: usize| {
                defs.get(&v).is_some_and(|&(x, at)| {
                    (x == b && at <= point)
                        || (x != b
                            && (!dom.reachable(b.index()) || dom.dominates(x.index(), b.index())))
                })
            };
            let usable = |edge: &VersionEdge| {
                let state = &edge.slots[slot.index()];
                let empty = content_of(state.live, state.init) == Tri::No;
                match state.versions {
                    Versions::One(value) => empty || reaches(value, edge.source, edge.point),
                    Versions::None => empty,
                    Versions::Many => false,
                }
            };
            if !inputs.ins[block.index()]
                .as_ref()
                .is_some_and(|state| state.slots[slot.index()].versions == Versions::Many)
            {
                return Err(PassError::Unsupported(
                    "versions that meet inside a block".into(),
                ));
            }
            let ty = ed.unit.slots[slot.index()].ty;
            let mut joins: BTreeMap<BlockId, ValueId> = BTreeMap::new();
            let mut created = Vec::new();
            let mut work = vec![block];
            while let Some(source) = work.pop() {
                if joins.contains_key(&source) {
                    continue;
                }
                let value = ed.value(ty, Holding::Plain);
                joins.insert(source, value);
                created.push((source, value));
                for &edge in &incoming[source.index()] {
                    if let Some(edge) = &inputs.edges[edge.index()]
                        && !usable(edge)
                    {
                        work.push(edge.source);
                    }
                }
            }
            for (&target, &parameter) in &joins {
                let params = ed.unit.blocks[target.index()]
                    .parameters
                    .get(&ed.unit.pools.values)
                    .unwrap_or(&[])
                    .to_vec();
                let ordinal = params.len();
                ed.unit.blocks[target.index()].parameters = PoolRange::push(
                    &mut ed.unit.pools.values,
                    params.into_iter().chain([parameter]),
                );
                if facts.phis.len() <= target.index() {
                    facts.phis.resize(target.index() + 1, Vec::new());
                }
                facts.phis[target.index()].push((ordinal, slot));
                if facts.version_of.len() <= parameter.index() {
                    facts.version_of.resize(parameter.index() + 1, None);
                    facts.backing.resize(parameter.index() + 1, None);
                    facts.statics.resize(parameter.index() + 1, false);
                }
                facts.version_of[parameter.index()] = Some(slot);
                for &edge in &incoming[target.index()] {
                    // Unused edges are pruned by Editor::finish, but need a
                    // correctly typed argument until then.
                    let argument = match &inputs.edges[edge.index()] {
                        Some(along) if usable(along) => {
                            match along.slots[slot.index()].versions {
                                Versions::One(value) => value,
                                Versions::None => versions[slot.index()]
                                    .iter()
                                    .copied()
                                    .find(|&v| reaches(v, along.source, along.point))
                                    .or_else(|| joins.get(&along.source).copied())
                                    // Revision 8 permits a future marker on empty
                                    // storage. Initialized inputs pin its version.
                                    .or_else(|| versions[slot.index()].first().copied())
                                    .ok_or_else(|| {
                                        PassError::Malformed(
                                            "initialized storage has no version declaration".into(),
                                        )
                                    })?,
                                Versions::Many => unreachable!("usable version"),
                            }
                        }
                        Some(along) => joins.get(&along.source).copied().unwrap_or(parameter),
                        None => parameter,
                    };
                    let args = ed.unit.tables.edges[edge.index()]
                        .arguments
                        .get(&ed.unit.pools.values)
                        .unwrap_or(&[])
                        .to_vec();
                    ed.unit.tables.edges[edge.index()].arguments = PoolRange::push(
                        &mut ed.unit.pools.values,
                        args.into_iter().chain([argument]),
                    );
                }
            }
            // Creation order is ValueId order, preserving the fallback's
            // preference for the earliest declared version of this slot.
            for (block, value) in created {
                defs.insert(value, (block, 0));
                versions[slot.index()].push(value);
            }
        }
    }
    Err(PassError::Malformed("version joins did not settle".into()))
}

struct VersionEdge {
    source: BlockId,
    point: usize,
    slots: hirsel_lir::persistent::PVec<crate::insert::SlotState>,
}

/// One solved input and one replay for every version consumer and edge.
/// Snapshots share persistent state rows; no state is cloned at each drop/read.
struct VersionInputs {
    ins: Vec<Option<State>>,
    edges: Vec<Option<VersionEdge>>,
    joins: Vec<(BlockId, SlotId)>,
}
impl VersionInputs {
    fn new(ed: &Editor<Ready>, facts: &Facts, lay: Lay<'_>, reads: bool) -> Self {
        let flow = Flow {
            unit: &ed.unit,
            code: &ed.code,
            lay,
            facts,
        };
        let ins = version_states(&flow);
        let mut edges = (0..ed.unit.tables.edges.len())
            .map(|_| None)
            .collect::<Vec<_>>();
        let mut order = Vec::new();
        let mut joins = Vec::new();
        let mut slots = std::collections::BTreeSet::new();
        for (b, state) in ins.iter().enumerate() {
            let Some(state) = state else {
                continue;
            };
            let block = BlockId(b as u32);
            let mut record = |edge: EdgeId, point: usize, state: &State| {
                edges[edge.index()] = Some(VersionEdge {
                    source: block,
                    point,
                    slots: state.slots.clone(),
                });
                order.push(edge);
            };
            work::count_by(Counter::RcCleanupVersionRows, (ed.code[b].len() + 1) as u64);
            flow.block(block, state.clone(), |i, pre, post| {
                if let Some(inst) = ed.code[b].get(i) {
                    let address = match inst {
                        Inst::Drop { address, .. } => Some(*address),
                        Inst::Read { address, .. } | Inst::Copy { address, .. } if reads => {
                            Some(*address)
                        }
                        _ => None,
                    };
                    if let Some(address) = address
                        && let Some((slot, _)) = facts.slot_of(address)
                    {
                        let state = &pre.slots[slot.index()];
                        if content_of(state.live, state.init) != Tri::No
                            && state.versions == Versions::Many
                            && slots.insert(slot)
                        {
                            joins.push((block, slot));
                        }
                    }
                    if let Some(failure) = inst.failure() {
                        let finishes = matches!(
                            inst,
                            Inst::Drop { .. }
                                | Inst::Release { .. }
                                | Inst::Reset { .. }
                                | Inst::RegionExit { .. }
                        );
                        record(
                            ed.unit.tables.failures[failure.index()].cleanup,
                            i + usize::from(finishes),
                            if finishes { post } else { pre },
                        );
                    }
                } else {
                    let term = &ed.unit.terminators[b];
                    for edge in term.normal_edges(&ed.unit.pools) {
                        record(edge, i, post);
                    }
                    for failure in term.failure_edges(&ed.unit.tables) {
                        let state = if let Terminator::Call { call, .. } = term {
                            if ed.unit.tables.calls[call.index()].pretransfer_failure
                                == Some(failure)
                            {
                                pre
                            } else {
                                post
                            }
                        } else {
                            post
                        };
                        record(ed.unit.tables.failures[failure.index()].cleanup, i, state);
                    }
                }
            });
        }
        // After all direct consumers have versions, repair transports of
        // initialized plain bytes retained by finalized shells as before.
        if reads && joins.is_empty() {
            for edge in order {
                let along = edges[edge.index()].as_ref().unwrap();
                let target = ed.unit.tables.edges[edge.index()].target;
                for &(_, slot) in facts.phis_of(target) {
                    if along.slots[slot.index()].versions == Versions::Many && slots.insert(slot) {
                        joins.push((along.source, slot));
                    }
                }
            }
        }
        Self { ins, edges, joins }
    }
}

/// Where each version of a slot is defined: (block, the instruction index
/// it is available from; 0 for a parameter, the instruction count for a
/// terminator's write-back).
fn version_defs(ed: &Editor<Ready>, facts: &Facts) -> BTreeMap<ValueId, (BlockId, usize)> {
    let is_version = |v: ValueId| facts.version_of.get(v.index()).copied().flatten().is_some();
    let mut out = BTreeMap::new();
    for (x, block) in ed.unit.blocks.iter().enumerate() {
        let b = BlockId(x as u32);
        for &p in block.parameters.get(&ed.unit.pools.values).unwrap_or(&[]) {
            if is_version(p) {
                out.insert(p, (b, 0));
            }
        }
        for (i, inst) in ed.code[x].iter().enumerate() {
            for v in Inst::defs(inst) {
                if is_version(v) {
                    out.insert(v, (b, i + 1));
                }
            }
        }
        for v in crate::liveness::terminator_defs(&ed.unit, &ed.unit.terminators[x]) {
            if is_version(v) {
                out.insert(v, (b, ed.code[x].len()));
            }
        }
    }
    for &v in ed.unit.parameters.iter().chain(&ed.unit.captures) {
        if is_version(v) {
            out.insert(v, (ed.unit.entry, 0));
        }
    }
    out
}

/// Drop flags (explicit initialized masks, LIR schema revision 8). A drop that
/// meets content initialized on some paths only (a resource
/// taken in a loop and assigned again, say) is decided at run time: every
/// block where its slot may or may not hold content gets a bool parameter
/// that is true exactly when it does, each edge into such a block passes
/// `true`, `false` or the source's own flag, and a whole `Replace` that may
/// displace nothing hands the flag to the displaced slot. Returns each such
/// drop's flag by position; `Lowering::drop` branches on it.
type Part = (SlotId, Vec<Step>);
type Flags = BTreeMap<Part, ValueId>;

/// Presence of an inline part. Descendant holes do not remove the containing
/// part; a recursive drop uses their own flags in the original drop order.
fn presence(s: &crate::insert::SlotState, paths: &Paths, path: &[Step]) -> Tri {
    if content_of(s.live, s.init) == Tri::No {
        return Tri::No;
    }
    let mut value = content_of(s.live, s.init);
    for (p, moved) in s.moved.iter() {
        if path.starts_with(paths.steps(p)) {
            if moved == Tri::Yes {
                return Tri::No;
            }
            if moved == Tri::Maybe {
                value = Tri::Maybe;
            }
        }
    }
    value
}

pub(crate) fn absent(
    s: &crate::insert::SlotState,
    paths: &Paths,
    path: &mut Vec<Step>,
    layout: LayoutId,
    lay: Lay<'_>,
) -> bool {
    if presence(s, paths, path) == Tri::No || lay.trivial(layout) {
        return true;
    }
    let Some(record) = lay.get(layout) else {
        return false;
    };
    if record.finalizer.is_some() && s.fields_pending.at(paths, path) != Tri::Yes {
        return false;
    }
    if let LayoutKind::Inline { element, count } = record.kind {
        return (0..count).all(|index| {
            path.push(Step::Inline(index));
            let empty = absent(s, paths, path, element, lay);
            path.pop();
            empty
        });
    }
    if record.tag.is_some() {
        return record.tag.as_ref().is_some_and(|tag| {
            tag.variants.iter().all(|variant| {
                variant.fields.iter().all(|field| {
                    path.push(Step::Variant(variant.ordinal, field.ordinal));
                    let empty = absent(s, paths, path, field.layout, lay);
                    path.pop();
                    empty
                })
            })
        });
    }
    if matches!(record.kind, LayoutKind::Aggregate | LayoutKind::Resource) {
        return record.fields.iter().all(|field| {
            path.push(Step::Field(field.ordinal));
            let empty = absent(s, paths, path, field.layout, lay);
            path.pop();
            empty
        });
    }
    false
}

fn owner_presence(
    s: &crate::insert::SlotState,
    paths: &Paths,
    path: &[Step],
    layout: LayoutId,
    lay: Lay<'_>,
) -> Tri {
    // A slot proved empty owns nothing at any part with owners (C's
    // `flag_content`).
    if s.owned_empty && (path.is_empty() || !lay.trivial(layout)) {
        Tri::No
    } else if !lay.trivial(layout) && absent(s, paths, &mut path.to_vec(), layout, lay) {
        Tri::No
    } else {
        presence(s, paths, path)
    }
}

/// A finite part of frame-owned inline storage. Counted payloads and
/// dynamic elements are released by their owning container, not frame flags.
fn inline_part_layout(
    lay: Lay<'_>,
    mut layout: LayoutId,
    path: &[Step],
) -> Result<Option<LayoutId>, PassError> {
    for step in path {
        if lay.payload(layout).is_some() {
            return Ok(None);
        }
        let record = lay.get(layout);
        let found = match *step {
            Step::Payload => return Ok(None),
            Step::Field(f) => record.and_then(|r| r.fields.iter().find(|x| x.ordinal == f)),
            Step::Variant(v, f) => record
                .and_then(|r| r.tag.as_ref())
                .and_then(|t| t.variants.iter().find(|x| x.ordinal == v))
                .and_then(|x| x.fields.iter().find(|x| x.ordinal == f)),
            Step::Inline(_) => {
                layout = match record.map(|r| &r.kind) {
                    Some(LayoutKind::Inline { element, .. }) => *element,
                    _ => {
                        return Err(PassError::Malformed(
                            "an inline path outside inline storage".into(),
                        ));
                    }
                };
                continue;
            }
            Step::Index | Step::Slot => return Ok(None),
        };
        layout = found
            .map(|x| x.layout)
            .ok_or_else(|| PassError::Malformed(format!("no {step:?} of {layout:?}")))?;
    }
    Ok(Some(layout))
}

fn join_flags<R: Reporter>(
    ed: &mut Editor<Ready>,
    facts: &Facts,
    lay: Lay<'_>,
    index: &hirsel_lir::LayoutIndex<'_>,
    drops: &[(BlockId, usize, State)],
    report: &mut R,
) -> Result<Option<BTreeMap<(BlockId, usize), Flags>>, PassError> {
    // Each flagged part with its layout. A flag is true exactly when its part
    // still owns content (LIR revision 13): a part whose remaining fields are
    // all absent or plain, an inactive variant's included, owns nothing.
    let mut wanted: BTreeMap<Part, LayoutId> = BTreeMap::new();
    for &(b, i, ref state) in drops {
        let Inst::Drop { address, .. } = ed.code[b.index()][i] else {
            unreachable!()
        };
        let Some((slot, _)) = facts.slot_of(address) else {
            continue;
        };
        let s = &state.slots[slot.index()];
        let layout = ed.unit.types[ed.unit.slots[slot.index()].ty.index()].layout;
        let paths = facts.paths();
        if !lay.trivial(layout) && owner_presence(s, &paths, &[], layout, lay) == Tri::Maybe {
            wanted.insert((slot, Vec::new()), layout);
        }
        for (path, moved) in s.moved.sorted(&paths) {
            if moved == Tri::Maybe
                && let Some(part) = inline_part_layout(lay, layout, path)?
                && !lay.trivial(part)
            {
                wanted.insert((slot, path.to_vec()), part);
            }
        }
    }
    if wanted.is_empty() {
        return Ok(None);
    }
    let boolean = index
        .scalar(hirsel_lir::layout_types::ScalarKind::Bool)
        .ok_or_else(|| PassError::Malformed("drop flags require the bool layout".into()))?;
    let bool_ty = type_of(
        &mut ed.unit,
        ValueRepr::Scalar(hirsel_lir::layout_types::ScalarKind::Bool),
        boolean,
    );
    let mut constant = |value: bool| {
        let id = hirsel_lir::ir::ConstantId(ed.unit.tables.constants.len() as u32);
        ed.unit.tables.constants.push(hirsel_lir::ir::ConstantData {
            ty: bool_ty,
            value: hirsel_lir::ir::ConstantValue::Bool(value),
        });
        let result = ed.value(bool_ty, Holding::Plain);
        ed.code[ed.unit.entry.index()].insert(
            0,
            Inst::Const {
                result,
                constant: id,
            },
        );
        result
    };
    let yes = constant(true);
    let no = constant(false);
    report.inserted(&[(ed.unit.entry, 0), (ed.unit.entry, 0)])?;
    type Shape = BTreeMap<Part, Tri>;
    let shape = |st: &State| -> Shape {
        wanted
            .iter()
            .map(|(p, &layout)| {
                (
                    p.clone(),
                    owner_presence(&st.slots[p.0.index()], &facts.paths(), &p.1, layout, lay),
                )
            })
            .collect()
    };
    let flow = Flow {
        unit: &ed.unit,
        code: &ed.code,
        lay,
        facts,
    };
    let ins = flow.solve();
    let mut walks = BTreeMap::new();
    for (b, st) in ins.iter().enumerate() {
        let Some(st) = st else { continue };
        let block = BlockId(b as u32);
        let mut steps = Vec::new();
        let outs = flow.block(block, st.clone(), |i, pre, post| {
            if i < flow.code[b].len() {
                steps.push((shape(pre), shape(post)));
            }
        });
        walks.insert(
            block,
            (
                shape(st),
                steps,
                outs.into_iter()
                    .map(|(e, st)| (e, shape(&st)))
                    .collect::<BTreeMap<_, _>>(),
            ),
        );
    }
    let mut params: BTreeMap<BlockId, Flags> = BTreeMap::new();
    for (&block, (entry, _, _)) in &walks {
        for (part, &value) in entry {
            if block != ed.unit.entry && value == Tri::Maybe {
                let value = ed.value(bool_ty, Holding::Plain);
                let path = flag_projection(ed, &part.1)?;
                ed.unit.tables.init_flags.push(hirsel_lir::ir::InitFlag {
                    value,
                    slot: part.0,
                    path,
                    environment: None,
                });
                params.entry(block).or_default().insert(part.clone(), value);
            }
        }
    }
    for (&block, added) in &params {
        let old = ed.unit.blocks[block.index()]
            .parameters
            .get(&ed.unit.pools.values)
            .unwrap_or(&[])
            .to_vec();
        ed.unit.blocks[block.index()].parameters = PoolRange::push(
            &mut ed.unit.pools.values,
            old.into_iter().chain(added.values().copied()),
        );
    }
    let argument = |part: &Part, state: &Shape, current: &Flags| match state[part] {
        Tri::Yes => Ok(yes),
        Tri::No => Ok(no),
        Tri::Maybe => current
            .get(part)
            .copied()
            .ok_or_else(|| PassError::Malformed(format!("presence flag lost for {part:?}"))),
    };
    let mut flags = BTreeMap::new();
    for (&block, (entry, steps, outs)) in &walks {
        let b = block.index();
        let mut current = params.get(&block).cloned().unwrap_or_default();
        for (i, (pre, post)) in steps.iter().enumerate() {
            let inst = ed.code[b][i].clone();
            if matches!(inst, Inst::Drop { .. }) {
                flags.insert((block, i), current.clone());
            }
            let before = current.clone();
            for part in wanted.keys() {
                if pre[part] != post[part] {
                    current.remove(part);
                }
            }
            if let Inst::Replace {
                address, displaced, ..
            } = inst
                && let Some((target, path)) = facts.slot_of(address)
                && path.is_empty()
            {
                for ((slot, path), &value) in &before {
                    if *slot == target {
                        current.insert((displaced, path.clone()), value);
                    }
                }
            }
            if let Some(failure) = Inst::failure(&inst)
                && failure != crate::insert::PENDING_FAILURE
            {
                let cleanup = ed.unit.tables.failures[failure.index()].cleanup;
                let target = ed.unit.tables.edges[cleanup.index()].target;
                if let Some(added) = params.get(&target) {
                    let finishes = matches!(
                        inst,
                        Inst::Release { .. }
                            | Inst::Reset { .. }
                            | Inst::Drop { .. }
                            | Inst::RegionExit { .. }
                    );
                    let state = &outs[&cleanup];
                    let here = if finishes { &current } else { &before };
                    let extra = added
                        .keys()
                        .map(|p| argument(p, state, here))
                        .collect::<Result<Vec<_>, _>>()?;
                    let extended = extended(ed, cleanup, &extra);
                    report.edge(cleanup, extended)?;
                    let cleanup = extended;
                    let mut copy = ed.unit.tables.failures[failure.index()].clone();
                    copy.cleanup = cleanup;
                    ed.unit.tables.failures.push(copy);
                    *Inst::failure_mut(&mut ed.code[b][i]).unwrap() =
                        FailureId(ed.unit.tables.failures.len() as u32 - 1);
                }
            }
        }
        let term = ed.unit.terminators[b].clone();
        ed.unit.terminators[b] = reroute(ed, term, &mut |ed, e| {
            let target = ed.unit.tables.edges[e.index()].target;
            let Some(added) = params.get(&target) else {
                return Ok(e);
            };
            let state = outs.get(&e).unwrap_or(entry);
            let extra = added
                .keys()
                .map(|p| argument(p, state, &current))
                .collect::<Result<Vec<_>, _>>()?;
            let extended = extended(ed, e, &extra);
            report.edge(e, extended)?;
            Ok(extended)
        })?;
    }
    Ok(Some(flags))
}

fn flag_projection(
    ed: &mut Editor<Ready>,
    path: &[Step],
) -> Result<PoolRange<ProjectionStep>, PassError> {
    let mut steps = Vec::new();
    for &step in path {
        steps.push(match step {
            Step::Payload => ProjectionStep::Payload,
            Step::Field(f) => ProjectionStep::Field(f),
            Step::Variant(variant, field) => ProjectionStep::VariantField { variant, field },
            Step::Inline(index) => {
                let value = ed
                    .code
                    .iter()
                    .flatten()
                    .find_map(|inst| match *inst {
                        Inst::Const { result, constant }
                            if ed.unit.types[ed.unit.values[result.index()].ty.index()].repr
                                == ValueRepr::Scalar(hirsel_lir::layout_types::ScalarKind::U64)
                                && ed.unit.tables.constants[constant.index()].value
                                    == hirsel_lir::ir::ConstantValue::Integer(index.into()) =>
                        {
                            Some(result)
                        }
                        _ => None,
                    })
                    .ok_or_else(|| {
                        PassError::Malformed("an inline move has no constant index".into())
                    })?;
                ProjectionStep::Index(value)
            }
            Step::Index | Step::Slot => {
                return Err(PassError::Malformed(
                    "drop flags describe inline fields only".into(),
                ));
            }
        });
    }
    Ok(PoolRange::push(&mut ed.unit.pools.projections, steps))
}

fn content_of(live: Tri, init: Tri) -> Tri {
    match (live, init) {
        (Tri::Yes, Tri::Yes) => Tri::Yes,
        (Tri::No, _) | (_, Tri::No) => Tri::No,
        _ => Tri::Maybe,
    }
}

/// `term` with each edge it names (a failure's cleanup edge through a copy
/// of the failure) replaced by `edge`'s choice.
fn reroute(
    ed: &mut Editor<Ready>,
    term: Terminator,
    edge: &mut dyn FnMut(&mut Editor<Ready>, EdgeId) -> Result<EdgeId, PassError>,
) -> Result<Terminator, PassError> {
    fn failure(
        ed: &mut Editor<Ready>,
        f: FailureId,
        edge: &mut dyn FnMut(&mut Editor<Ready>, EdgeId) -> Result<EdgeId, PassError>,
    ) -> Result<FailureId, PassError> {
        let mut copy = ed.unit.tables.failures[f.index()].clone();
        let cleanup = edge(ed, copy.cleanup)?;
        if cleanup == copy.cleanup {
            return Ok(f);
        }
        copy.cleanup = cleanup;
        ed.unit.tables.failures.push(copy);
        Ok(FailureId(ed.unit.tables.failures.len() as u32 - 1))
    }
    Ok(match term {
        Terminator::Jump(e) => Terminator::Jump(edge(ed, e)?),
        Terminator::Branch { condition, yes, no } => Terminator::Branch {
            condition,
            yes: edge(ed, yes)?,
            no: edge(ed, no)?,
        },
        Terminator::Switch {
            value,
            arms,
            otherwise,
        } => {
            let old = arms.get(&ed.unit.pools.switch_arms).unwrap_or(&[]).to_vec();
            let mut new = Vec::with_capacity(old.len());
            for arm in old {
                new.push(hirsel_lir::ir::SwitchArm {
                    value: arm.value,
                    edge: edge(ed, arm.edge)?,
                });
            }
            Terminator::Switch {
                value,
                arms: PoolRange::push(&mut ed.unit.pools.switch_arms, new),
                otherwise: edge(ed, otherwise)?,
            }
        }
        Terminator::Call {
            call,
            normal,
            failure: f,
        } => {
            let normal = normal.map(|normal| edge(ed, normal)).transpose()?;
            Terminator::Call {
                call,
                normal,
                failure: failure(ed, f, edge)?,
            }
        }
        Terminator::Raise(f) => Terminator::Raise(failure(ed, f, edge)?),
        other => other,
    })
}

/// A new edge to `edge`'s target with its arguments and then `extra`.
fn extended(ed: &mut Editor<Ready>, edge: EdgeId, extra: &[ValueId]) -> EdgeId {
    let e = &ed.unit.tables.edges[edge.index()];
    let target = e.target;
    let args: Vec<ValueId> = e
        .arguments
        .get(&ed.unit.pools.values)
        .unwrap_or(&[])
        .iter()
        .copied()
        .chain(extra.iter().copied())
        .collect();
    ed.edge(target, &args)
}

/// The state before every semantic drop, in block and instruction order.
fn drop_states(ed: &Editor<Ready>, facts: &Facts, lay: Lay<'_>) -> Vec<(BlockId, usize, State)> {
    instruction_states(ed, facts, lay, false)
}

fn version_states(flow: &Flow<'_, Ready>) -> Vec<Option<State>> {
    flow.solve_counted(|rows| work::count_by(Counter::RcCleanupVersionRows, rows as u64))
}

fn instruction_states(
    ed: &Editor<Ready>,
    facts: &Facts,
    lay: Lay<'_>,
    reads: bool,
) -> Vec<(BlockId, usize, State)> {
    let flow = Flow {
        unit: &ed.unit,
        code: &ed.code,
        lay,
        facts,
    };
    let ins = version_states(&flow);
    instruction_states_from(ed, facts, lay, reads, ins)
}

fn instruction_states_from(
    ed: &Editor<Ready>,
    facts: &Facts,
    lay: Lay<'_>,
    reads: bool,
    ins: Vec<Option<State>>,
) -> Vec<(BlockId, usize, State)> {
    let flow = Flow {
        unit: &ed.unit,
        code: &ed.code,
        lay,
        facts,
    };
    let mut out = Vec::new();
    for (b, state) in ins.into_iter().enumerate() {
        let Some(state) = state else {
            continue;
        };
        let block = BlockId(b as u32);
        let code = &ed.code[b];
        work::count_by(Counter::RcCleanupVersionRows, (code.len() + 1) as u64);
        flow.block(block, state, |i, pre, _| {
            if matches!(code.get(i), Some(Inst::Drop { .. }))
                || reads && matches!(code.get(i), Some(Inst::Read { .. } | Inst::Copy { .. }))
            {
                out.push((block, i, pre.clone()));
            }
        });
    }
    out
}

/// Library finalizers mutate the finalized root on both call edges. Thread
/// those write-back versions into the field releases that follow the call,
/// including joins in the shared failure cleanup.
fn join_read_versions(
    ed: &mut Editor<Ready>,
    facts: &mut Facts,
    lay: Lay<'_>,
) -> Result<(), PassError> {
    let inputs = join_versions(ed, facts, lay, true)?;
    for (block, i, state) in instruction_states_from(ed, facts, lay, true, inputs) {
        let address = match ed.code[block.index()][i] {
            Inst::Read { address, .. } | Inst::Copy { address, .. } => address,
            _ => continue,
        };
        let Some((slot, _)) = facts.slot_of(address) else {
            continue;
        };
        let version = match state.slots[slot.index()].versions {
            crate::insert::Versions::None => continue,
            crate::insert::Versions::One(version) => version,
            crate::insert::Versions::Many => {
                return Err(PassError::Malformed(
                    "a finalizer field read has several versions".into(),
                ));
            }
        };
        match &mut ed.code[block.index()][i] {
            Inst::Read { reaching, .. } | Inst::Copy { reaching, .. } => *reaching = version,
            _ => unreachable!(),
        }
    }
    Ok(())
}

/// Bits copies read bits and FlatBytes copies copy bytes; Dup and Fields
/// copies are the Ready primitive.
fn lower_copies(ed: &mut Editor<Ready>, lay: Lay<'_>) -> Result<(), PassError> {
    for b in 0..ed.code.len() {
        for i in 0..ed.code[b].len() {
            let Inst::Copy {
                result,
                address,
                reaching,
                failure,
            } = ed.code[b][i]
            else {
                continue;
            };
            let layout = ed.unit.types[ed.unit.values[result.index()].ty.index()].layout;
            ed.code[b][i] = match lay.get(layout).map(|r| r.copy) {
                Some(CopyRecipe::Bits) => Inst::Read {
                    result,
                    address,
                    reaching,
                    mode: ReadMode::Bits,
                },
                Some(CopyRecipe::FlatBytes) => Inst::FlatCopy {
                    result,
                    address,
                    layout,
                    failure,
                },
                Some(CopyRecipe::Dup | CopyRecipe::Fields) => continue,
                Some(CopyRecipe::NotCopyable) | None => {
                    return Err(PassError::Malformed(format!(
                        "a copy of {layout:?}, which has no copy recipe"
                    )));
                }
            };
        }
    }
    Ok(())
}

/// One step of a lowered drop, over a field path of the dropped slot.
#[derive(Clone, Debug)]
enum Action {
    Conditional {
        condition: ValueId,
        actions: Vec<Action>,
    },
    /// Take the content whole and release it by its layout.
    Release {
        path: Vec<Step>,
        layout: LayoutId,
        moved: Vec<u32>,
    },
    /// Run an inline resource's drop body between `FinalizeBegin` and
    /// `FinalizeEnd`; its activation remains live through owned field cleanup.
    Finalize {
        path: Vec<Step>,
        layout: LayoutId,
        fields: Vec<Action>,
    },
    /// An intact variant containing inline finalizers needs an active-arm plan.
    Retire {
        slot: SlotId,
    },
    Variant {
        path: Vec<Step>,
        layout: LayoutId,
    },
    EmptyVariant {
        path: Vec<Step>,
        layout: LayoutId,
    },
    Inline {
        path: Vec<Step>,
        layout: LayoutId,
    },
}

struct Lowering<'e, 'a> {
    ed: &'e mut Editor<Ready>,
    facts: &'e mut Facts,
    lay: Lay<'a>,
    finalizers: &'a Finalizers,
    index: &'a hirsel_lir::LayoutIndex<'a>,
    symbols: BTreeMap<LayoutId, SymbolId>,
    /// Instructions inserted outside the drop being lowered, as (block,
    /// index), for the positions of the drops still to lower.
    inserted: Vec<(BlockId, usize)>,
    prepared: Vec<(SlotId, Vec<Step>, AddressId)>,
    flags: BTreeMap<Vec<Step>, ValueId>,
    retire_flags: BTreeMap<(SlotId, Vec<Step>), (SlotId, AddressId, ValueId)>,
    /// The block of the drop being lowered.
    drop_at: BlockId,
}

/// What every action of one drop shares.
struct Site {
    slot: SlotId,
    version: Option<ValueId>,
    anchor: CleanupAnchorId,
    /// The drop's own failure successor, where an accumulated failure goes.
    failure: FailureEdge,
}

impl Lowering<'_, '_> {
    /// Lower the drop at `b`'s instruction `i`; `flag` decides content that
    /// is there on some paths only.
    fn drop<R: Reporter>(
        &mut self,
        b: BlockId,
        i: usize,
        st: &State,
        flags: Flags,
        report: &mut R,
    ) -> Result<(), PassError> {
        let Inst::Drop {
            address,
            if_live: _,
            anchor,
            failure,
        } = self.ed.code[b.index()][i]
        else {
            return Err(PassError::Malformed("not a drop".into()));
        };
        self.drop_at = b;
        let Some((slot, path)) = self.facts.slot_of(address).map(|(s, p)| (s, p.to_vec())) else {
            return Err(PassError::Unsupported("a drop through a block".into()));
        };
        self.flags = flags
            .iter()
            .filter(|(p, _)| p.0 == slot)
            .map(|((_, p), &v)| (p.clone(), v))
            .collect();
        let s = &st.slots[slot.index()];
        let mut actions = Vec::new();
        let layout = self.place_layout(slot, &path)?;
        // A committed whole destructure has transferred even its Plain fields.
        // End that empty storage before a guard/cleanup join; erasing its Drop
        // alone leaves conditional moved parts on an otherwise live parent.
        let retire_shell = path.is_empty()
            && s.live == Tri::Yes
            && s.init == Tri::Yes
            && s.fields_pending.at(&self.facts.paths(), &[]) == Tri::Yes
            && self.lay.get(layout).is_some_and(|record| {
                record.finalizer.is_some()
                    && record.fields.iter().all(|field| {
                        s.moved
                            .at(&self.facts.paths(), &[Step::Field(field.ordinal)])
                            == Tri::Yes
                    })
                    && record
                        .tag
                        .iter()
                        .flat_map(|tag| &tag.variants)
                        .all(|variant| {
                            variant.fields.iter().all(|field| {
                                s.moved.at(
                                    &self.facts.paths(),
                                    &[Step::Variant(variant.ordinal, field.ordinal)],
                                ) == Tri::Yes
                            })
                        })
            });
        if !self.lay.trivial(layout)
            && owner_presence(s, &self.facts.paths(), &path, layout, self.lay) != Tri::No
        {
            if s.live == Tri::Maybe || s.init == Tri::Maybe {
                let Some(&flag) = self.flags.get(&Vec::new()) else {
                    return Err(PassError::Unsupported(format!(
                        "a drop of conditionally initialized {slot:?}"
                    )));
                };
                return self.flagged(b, i, st, slot, flag, flags, report);
            }
            if path
                .iter()
                .any(|step| !matches!(step, Step::Field(_) | Step::Variant(_, _) | Step::Inline(_)))
            {
                return Err(PassError::Unsupported(
                    "a drop through dynamic element storage".into(),
                ));
            }
            let layout = self.place_layout(slot, &path)?;
            self.plan(s, path, layout, &mut actions)?;
        }
        let whole_release = matches!(actions.as_slice(), [Action::Release { path: released, moved, .. }]
            if self.facts.slot_of(address).is_some_and(|(_, original)| released == original) && moved.is_empty());
        let whole_finalizer = matches!(actions.as_slice(), [Action::Finalize { path: finalized, .. }]
            if self.facts.slot_of(address).is_some_and(|(_, original)| finalized == original));
        let finalizer_fields = matches!(actions.as_slice(), [Action::Finalize { fields, .. }]
            if !fields.is_empty());
        let version = match s.versions {
            Versions::One(v) => Some(v),
            _ if actions.is_empty() => None,
            versions => {
                return Err(PassError::Unsupported(format!(
                    "{versions:?} versions of a {:?} slot reach its drop",
                    self.ed.unit.slots[slot.index()].kind
                )));
            }
        };
        let site = Site {
            slot,
            version,
            anchor,
            failure: self.ed.unit.tables.failures[failure.index()].clone(),
        };
        let tail = self.ed.code[b.index()].split_off(i + 1);
        self.ed.code[b.index()].pop();
        let terminator = std::mem::replace(
            &mut self.ed.unit.terminators[b.index()],
            Terminator::Unreachable,
        );
        let at = self.sequence(b, &actions, &site)?;
        if retire_shell && actions.is_empty() {
            self.ed.code[at.index()].push(Inst::StorageDead {
                slot,
                if_live: false,
            });
        }
        let release = (whole_release && at == b)
            .then(|| hirsel_lir::ownership_trace::Point::ReleaseDrop {
                block: at,
                index: self.ed.code[at.index()].len() as u32 - 1,
            })
            .or_else(|| {
                (whole_finalizer && !finalizer_fields)
                    .then_some(hirsel_lir::ownership_trace::Point::FinalizerDrop { block: b })
            });
        let release = release.or_else(|| {
            (!actions.is_empty() && !whole_release && (!whole_finalizer || finalizer_fields))
                .then_some(hirsel_lir::ownership_trace::Point::CompoundDrop {
                    block: b,
                    normal: at,
                    index: self.ed.code[at.index()].len() as u32,
                    failure: self.ed.unit.tables.edges[site.failure.cleanup.index()].target,
                })
        });
        report.splice(observation::Splice {
            block: b,
            index: i,
            tail: at,
            start: self.ed.code[at.index()].len(),
            replacement: release,
            insertions: &self.inserted,
        })?;
        if whole_finalizer {
            report.finalizer_edge(site.failure.cleanup, b)?;
        } else if let Some(hirsel_lir::ownership_trace::Point::ReleaseDrop { block, index }) =
            release
        {
            if let Inst::Release { failure, .. } = self.ed.code[block.index()][index as usize] {
                report.edge(
                    site.failure.cleanup,
                    self.ed.unit.tables.failures[failure.index()].cleanup,
                )?;
            }
        }
        self.ed.code[at.index()].extend(tail);
        self.ed.unit.terminators[at.index()] = terminator;
        Ok(())
    }

    /// A drop of content there on some paths only: `b` branches on the slot's
    /// flag into a block of its own that drops the content, then both go on
    /// to the rest of `b`.
    fn flagged<R: Reporter>(
        &mut self,
        b: BlockId,
        i: usize,
        st: &State,
        slot: SlotId,
        flag: ValueId,
        flags: Flags,
        report: &mut R,
    ) -> Result<(), PassError> {
        let block = &self.ed.unit.blocks[b.index()];
        let (kind, anchor) = (block.kind, block.anchor);
        let mut tail = self.ed.code[b.index()].split_off(i);
        let rest = tail.split_off(1);
        let terminator = std::mem::replace(
            &mut self.ed.unit.terminators[b.index()],
            Terminator::Unreachable,
        );
        let (full, after) = (
            self.ed.block(kind, &[], anchor),
            self.ed.block(kind, &[], anchor),
        );
        self.ed.code[full.index()] = tail;
        self.ed.code[after.index()] = rest;
        self.ed.unit.terminators[after.index()] = terminator;
        let (yes, no) = (self.ed.edge(full, &[]), self.ed.edge(after, &[]));
        self.ed.unit.terminators[b.index()] = Terminator::Branch {
            condition: flag,
            yes,
            no,
        };
        let on = self.ed.edge(after, &[]);
        self.ed.unit.terminators[full.index()] = Terminator::Jump(on);
        let mut there = st.clone();
        there.slots[slot.index()].live = Tri::Yes;
        there.slots[slot.index()].init = Tri::Yes;
        report.splice(observation::Splice {
            block: b,
            index: i,
            tail: after,
            start: 0,
            // This is the existing live branch of the same conditional
            // semantic drop. Its next splice transports the original marker
            // to the checked release/finalizer commit, never the bypass.
            replacement: Some(hirsel_lir::ownership_trace::Point::Instruction {
                block: full,
                index: 0,
                moment: hirsel_lir::ownership_trace::Moment::IfLive,
            }),
            insertions: &[],
        })?;
        self.drop(full, 0, &there, flags, report)
    }

    /// Emit a plan with a suffix cleanup after each potentially failing action.
    fn sequence(
        &mut self,
        at: BlockId,
        actions: &[Action],
        site: &Site,
    ) -> Result<BlockId, PassError> {
        // Keep finalizer bodies, descendant actions and retirement in one
        // flat plan. Every failure goes to the shared remaining suffix rather
        // than recursively cloning the complete descendant tree per outcome.
        let mut flat = Vec::new();
        self.flatten(actions, site, &mut flat)?;
        let actions = flat.as_slice();
        // Each chain block runs one action and goes on to the next; the last
        // continues into the drop's own failure successor.
        let mut chain: Vec<BlockId> = vec![BlockId(u32::MAX); actions.len() + 1];
        let cleanup = &self.ed.unit.tables.edges[site.failure.cleanup.index()];
        let (exit, exit_args) = (
            cleanup.target,
            cleanup
                .arguments
                .get(&self.ed.unit.pools.values)
                .unwrap_or(&[])
                .to_vec(),
        );
        chain[actions.len()] = exit;
        for j in (1..actions.len()).rev() {
            let c = self
                .ed
                .block(BlockKind::FailureCleanup, &[], site.failure.anchor);
            chain[j] = c;
            let end = self.action(c, &actions[j], site, &mut |l, _| {
                l.next(&chain, j + 1, &exit_args, site, false)
            })?;
            let e = self.next_edge(&chain, j + 1, &exit_args);
            self.ed.unit.terminators[end.index()] = Terminator::Jump(e);
        }
        let mut at = at;
        for (j, action) in actions.iter().enumerate() {
            at = self.action(at, action, site, &mut |l, _| {
                l.next(&chain, j + 1, &exit_args, site, true)
            })?;
        }
        Ok(at)
    }

    fn flatten(
        &mut self,
        actions: &[Action],
        site: &Site,
        out: &mut Vec<Action>,
    ) -> Result<(), PassError> {
        for action in actions {
            if let Action::Finalize {
                path,
                layout,
                fields,
            } = action
            {
                let source = self
                    .finalizers
                    .0
                    .get(layout)
                    .is_some_and(|f| f.origin == hirsel_lir::ir::UnitOrigin::Source);
                let guard = source.then(|| self.retire_flag(site, path)).transpose()?;
                out.push(Action::Finalize {
                    path: path.clone(),
                    layout: *layout,
                    fields: vec![],
                });
                self.flatten(fields, site, out)?;
                if let Some((slot, _, _)) = guard {
                    out.push(Action::Retire { slot });
                }
            } else {
                out.push(action.clone());
            }
        }
        Ok(())
    }

    /// Record the editor's exact entry insertion alongside cleanup's splices.
    fn whole(&mut self, slot: SlotId) -> AddressId {
        let before = self.ed.code[self.ed.unit.entry.index()].len();
        let insertion = self.ed.code[self.ed.unit.entry.index()]
            .iter()
            .position(|inst| matches!(inst, Inst::Address { .. }))
            .unwrap_or(0);
        let address = self.ed.whole(self.facts, slot);
        if self.ed.code[self.ed.unit.entry.index()].len() > before {
            self.inserted.push((self.ed.unit.entry, insertion));
        }
        address
    }

    fn retire_flag(
        &mut self,
        site: &Site,
        path: &[Step],
    ) -> Result<(SlotId, AddressId, ValueId), PassError> {
        let key = (site.slot, path.to_vec());
        if let Some(&flag) = self.retire_flags.get(&key) {
            return Ok(flag);
        }
        let layout = self
            .index
            .scalar(hirsel_lir::layout_types::ScalarKind::Bool)
            .ok_or_else(|| PassError::Malformed("finalizer retirement needs bool layout".into()))?;
        let ty = type_of(
            &mut self.ed.unit,
            ValueRepr::Scalar(hirsel_lir::layout_types::ScalarKind::Bool),
            layout,
        );
        let slot = SlotId(self.ed.unit.slots.len() as u32);
        self.ed.unit.slots.push(hirsel_lir::ir::SlotInfo {
            ty,
            kind: hirsel_lir::ir::SlotKind::Temporary,
            scope: self.ed.unit.slots[site.slot.index()].scope,
        });
        let address = self.whole(slot);
        let reaching = self.ed.value(ty, Holding::Plain);
        let flag = (slot, address, reaching);
        self.retire_flags.insert(key, flag);
        Ok(flag)
    }

    fn store_retirement_flag(
        &mut self,
        at: BlockId,
        flag: (SlotId, AddressId, ValueId),
        value: bool,
    ) {
        let ty = self.ed.unit.slots[flag.0.index()].ty;
        let constant = hirsel_lir::ir::ConstantId(self.ed.unit.tables.constants.len() as u32);
        self.ed
            .unit
            .tables
            .constants
            .push(hirsel_lir::ir::ConstantData {
                ty,
                value: hirsel_lir::ir::ConstantValue::Bool(value),
            });
        let result = self.ed.value(ty, Holding::Plain);
        let version = self.ed.value(ty, Holding::Plain);
        if value {
            let old = self.ed.value(ty, Holding::Plain);
            self.ed.code[at.index()].push(Inst::Read {
                result: old,
                address: flag.1,
                reaching: flag.2,
                mode: ReadMode::Take,
            });
        }
        self.ed.code[at.index()].extend([
            Inst::Const { result, constant },
            Inst::Store {
                address: flag.1,
                value: result,
                version,
            },
        ]);
    }

    /// The edge into chain position `j`: the next chain block, or the drop's
    /// own failure successor with its arguments.
    fn next_edge(&mut self, chain: &[BlockId], j: usize, exit_args: &[ValueId]) -> EdgeId {
        let args = if j + 1 == chain.len() { exit_args } else { &[] };
        self.ed.edge(chain[j], args)
    }

    /// A failure edge into chain position `j`. On the main path it carries
    /// the drop's rule and token disposals; inside the chain those already
    /// happened.
    fn next(
        &mut self,
        chain: &[BlockId],
        j: usize,
        exit_args: &[ValueId],
        site: &Site,
        main: bool,
    ) -> FailureId {
        let cleanup = self.next_edge(chain, j, exit_args);
        let id = FailureId(self.ed.unit.tables.failures.len() as u32);
        self.ed.unit.tables.failures.push(FailureEdge {
            cleanup,
            rule: if main {
                site.failure.rule.clone()
            } else {
                None
            },
            anchor: site.failure.anchor,
            dispose: if main {
                site.failure.dispose
            } else {
                PoolRange::EMPTY
            },
        });
        id
    }

    /// Emit `action` at the end of block `at`, its failures through `fail`;
    /// returns the block where the path continues.
    fn action(
        &mut self,
        at: BlockId,
        action: &Action,
        site: &Site,
        fail: &mut dyn FnMut(&mut Self, ()) -> FailureId,
    ) -> Result<BlockId, PassError> {
        let version = site
            .version
            .ok_or_else(|| PassError::Malformed("an action without a version".into()))?;
        match action {
            Action::Conditional { condition, actions } => {
                let block = &self.ed.unit.blocks[at.index()];
                let (kind, anchor) = (block.kind, block.anchor);
                let full = self.ed.block(kind, &[], anchor);
                let joined = self.ed.block(kind, &[], anchor);
                let yes = self.ed.edge(full, &[]);
                let no = self.ed.edge(joined, &[]);
                self.ed.unit.terminators[at.index()] = Terminator::Branch {
                    condition: *condition,
                    yes,
                    no,
                };
                let failure = fail(self, ());
                let inner = Site {
                    slot: site.slot,
                    version: site.version,
                    anchor: site.anchor,
                    failure: self.ed.unit.tables.failures[failure.index()].clone(),
                };
                let retry = self.ed.block(BlockKind::FailureCleanup, &[], anchor);
                let base = self.prepare_actions(retry, actions, &inner, retry)?;
                let failed = self.sequence(retry, actions, &inner)?;
                self.ed.unit.terminators[failed.index()] = Terminator::Jump(inner.failure.cleanup);
                self.prepared.truncate(base);
                let base = self.prepare_actions(full, actions, &inner, retry)?;
                let end = self.sequence(full, actions, &inner)?;
                self.ed.unit.terminators[end.index()] = Terminator::Jump(self.ed.edge(joined, &[]));
                self.prepared.truncate(base);
                Ok(joined)
            }
            Action::Retire { slot } => {
                let (_, address, reaching) = self
                    .retire_flags
                    .values()
                    .find(|(candidate, _, _)| candidate == slot)
                    .copied()
                    .unwrap();
                let ty = self.ed.unit.slots[slot.index()].ty;
                let condition = self.ed.value(ty, Holding::Plain);
                self.ed.code[at.index()].push(Inst::Read {
                    result: condition,
                    address,
                    reaching,
                    mode: ReadMode::Bits,
                });
                let block = &self.ed.unit.blocks[at.index()];
                let (kind, anchor) = (block.kind, block.anchor);
                let retire = self.ed.block(kind, &[], anchor);
                let joined = self.ed.block(kind, &[], anchor);
                let yes = self.ed.edge(retire, &[]);
                let no = self.ed.edge(joined, &[]);
                self.ed.unit.terminators[at.index()] = Terminator::Branch { condition, yes, no };
                self.ed.code[retire.index()].push(Inst::FrameLeave);
                self.ed.unit.terminators[retire.index()] =
                    Terminator::Jump(self.ed.edge(joined, &[]));
                self.ed.code[joined.index()].push(Inst::StorageDead {
                    slot: *slot,
                    if_live: false,
                });
                Ok(joined)
            }
            Action::EmptyVariant { path, layout } => {
                let whole = self.address(site.slot, path, *layout)?;
                let ty = type_of(&mut self.ed.unit, ValueRepr::Aggregate, *layout);
                let value = self.ed.value(ty, Holding::Owned);
                self.ed.code[at.index()].extend([
                    Inst::Read {
                        result: value,
                        address: whole,
                        reaching: version,
                        mode: ReadMode::Take,
                    },
                    Inst::Forget { value },
                ]);
                Ok(at)
            }
            Action::Variant { path, layout } => self.variant(at, path, *layout, site, fail),
            Action::Inline { path, layout } => self.inline(at, path, *layout, site, fail),
            Action::Release {
                path,
                layout,
                moved,
            } => {
                let address = self.address(site.slot, path, *layout)?;
                let repr = crate::import::repr_of(&self.layout_kind(*layout)?);
                let ty = type_of(&mut self.ed.unit, repr, *layout);
                let value = self.ed.value(ty, Holding::Owned);
                let failure = fail(self, ());
                let released = self.lay.payload(*layout).unwrap_or(*layout);
                let live = if moved.is_empty() {
                    MaskRef::All
                } else {
                    let record = self.lay.get(released).unwrap();
                    let fields = record
                        .fields
                        .iter()
                        .chain(
                            record
                                .tag
                                .iter()
                                .flat_map(|tag| &tag.variants)
                                .flat_map(|variant| &variant.fields),
                        )
                        .map(|field| field.ordinal + 1)
                        .max()
                        .unwrap_or(0);
                    let mut words = vec![u64::MAX; fields.div_ceil(64) as usize];
                    for &field in moved {
                        words[field as usize / 64] &= !(1 << (field % 64));
                    }
                    let words = PoolRange::push(&mut self.ed.unit.pools.mask_words, words);
                    let mask = hirsel_lir::ir::MaskId(self.ed.unit.tables.masks.len() as u32);
                    self.ed
                        .unit
                        .tables
                        .masks
                        .push(hirsel_lir::ir::FieldMask { fields, words });
                    MaskRef::Static(mask)
                };
                let code = &mut self.ed.code[at.index()];
                code.push(Inst::Read {
                    result: value,
                    address,
                    reaching: version,
                    mode: ReadMode::Take,
                });
                code.push(Inst::Release {
                    value,
                    layout: released,
                    live,
                    anchor: site.anchor,
                    failure,
                });
                Ok(at)
            }
            Action::Finalize {
                path,
                layout,
                fields: _,
            } => {
                let finalizer = self.finalizers.0.get(layout).cloned().ok_or_else(|| {
                    PassError::Unsupported(format!("no drop body instance for {layout:?}"))
                })?;
                let address = self.address(site.slot, path, *layout)?;
                let abi = &self.lay.0.fn_abis[finalizer.abi.0 as usize];
                let [parameter] = abi.parameters.as_slice() else {
                    return Err(PassError::Malformed("a finalizer takes self".into()));
                };
                let mutable = finalizer.origin == hirsel_lir::ir::UnitOrigin::Library;
                if parameter.convention
                    != if mutable {
                        Passing::Mut
                    } else {
                        Passing::Borrow
                    }
                {
                    return Err(PassError::Malformed(
                        "a library finalizer takes mut self; a source finalizer borrows self"
                            .into(),
                    ));
                }
                self.ed.code[at.index()].push(Inst::FinalizeBegin {
                    address,
                    anchor: site.anchor,
                });
                let (argument, loan) = if mutable {
                    let ty = self.ed.unit.slots[site.slot.index()].ty;
                    let version = self.ed.value(ty, Holding::Plain);
                    (ArgumentValue::Address { address, version }, None)
                } else {
                    let loan = LoanId(self.ed.unit.tables.loans.len() as u32);
                    self.ed.unit.tables.loans.push(LoanInfo {
                        address,
                        mutable: false,
                    });
                    let ty = type_of(&mut self.ed.unit, ValueRepr::Aggregate, *layout);
                    let this = self.ed.value(ty, Holding::Borrowed(loan));
                    self.ed.code[at.index()].extend([
                        Inst::BeginLoan {
                            loan,
                            address,
                            mutable: false,
                            if_inactive: false,
                        },
                        Inst::Read {
                            result: this,
                            address,
                            reaching: version,
                            mode: ReadMode::Borrow,
                        },
                    ]);
                    (ArgumentValue::Value(this), Some(loan))
                };
                let call = self.finalizer_call(*layout, &finalizer, argument, site)?;
                let kind = self.ed.unit.blocks[at.index()].kind;
                let anchor = self.ed.unit.blocks[at.index()].anchor;
                let normal = self.ed.block(kind, &[], anchor);
                let failed = self.ed.block(BlockKind::FailureCleanup, &[], anchor);
                let rejected =
                    (!mutable).then(|| self.ed.block(BlockKind::FailureCleanup, &[], anchor));
                for b in [normal, failed] {
                    let code = &mut self.ed.code[b.index()];
                    if let Some(loan) = loan {
                        code.push(Inst::EndLoan {
                            loan,
                            if_active: false,
                        });
                    }
                    code.push(Inst::FinalizeEnd {
                        address,
                        anchor: site.anchor,
                    });
                }
                // The drop body's failure is the primary one: the call
                // propagates it (no rule), `FinalizeEnd` lowers the shield and
                // the remaining fields are released after it, in the chain.
                let rest = fail(self, ());
                let rest = self.ed.unit.tables.failures[rest.index()].clone();
                if let Some(rejected) = rejected {
                    if let Some(loan) = loan {
                        self.ed.code[rejected.index()].push(Inst::EndLoan {
                            loan,
                            if_active: false,
                        });
                    }
                    self.ed.code[rejected.index()].push(Inst::FinalizeEnd {
                        address,
                        anchor: site.anchor,
                    });
                    let cleanup = extended(self.ed, rest.cleanup, &[]);
                    self.ed.unit.terminators[rejected.index()] = Terminator::Jump(cleanup);
                    let cleanup = self.ed.edge(rejected, &[]);
                    let failure = FailureId(self.ed.unit.tables.failures.len() as u32);
                    self.ed.unit.tables.failures.push(FailureEdge {
                        cleanup,
                        rule: Some("LANG-10".into()),
                        anchor: self.ed.unit.tables.calls[call.index()].anchor,
                        dispose: rest.dispose,
                    });
                    let flag = self.retire_flag(site, path)?;
                    self.ed.code[at.index()].push(Inst::StorageLive { slot: flag.0 });
                    self.store_retirement_flag(at, flag, false);
                    self.ed.code[at.index()].push(Inst::FrameEnter { failure });
                    self.store_retirement_flag(at, flag, true);
                }
                let into_failed = self.ed.edge(failed, &[]);
                let call_failure = FailureId(self.ed.unit.tables.failures.len() as u32);
                self.ed.unit.tables.failures.push(FailureEdge {
                    cleanup: into_failed,
                    rule: None,
                    anchor: site.failure.anchor,
                    dispose: rest.dispose,
                });
                let into_normal = self.ed.edge(normal, &[]);
                self.ed.unit.terminators[at.index()] = Terminator::Call {
                    call,
                    normal: Some(into_normal),
                    failure: call_failure,
                };
                let cleanup = extended(self.ed, rest.cleanup, &[]);
                self.ed.unit.terminators[failed.index()] = Terminator::Jump(cleanup);
                Ok(normal)
            }
        }
    }

    /// Keep a completed projected owner absent across an arm join. Each
    /// incoming false flag is checked against that arm's actual ownership;
    /// the false successor preserves the subtree proof before phase maps
    /// from inactive arms meet. No payload is read or transferred here.
    fn completed_part(
        &mut self,
        slot: SlotId,
        path: &[Step],
        destination: EdgeId,
        kind: BlockKind,
        anchor: hirsel_lir::ir::SourceAnchorId,
    ) -> Result<EdgeId, PassError> {
        let boolean = self
            .index
            .scalar(hirsel_lir::layout_types::ScalarKind::Bool)
            .ok_or_else(|| PassError::Malformed("cleanup completion needs bool layout".into()))?;
        let ty = type_of(
            &mut self.ed.unit,
            ValueRepr::Scalar(hirsel_lir::layout_types::ScalarKind::Bool),
            boolean,
        );
        let present = self.ed.value(ty, Holding::Plain);
        let joined = self.ed.block(kind, &[present], anchor);
        let path = flag_projection(self.ed, path)?;
        self.ed
            .unit
            .tables
            .init_flags
            .push(hirsel_lir::ir::InitFlag {
                value: present,
                slot,
                path,
                environment: None,
            });
        let invalid = self.ed.block(kind, &[], anchor);
        self.ed.unit.terminators[invalid.index()] = Terminator::InvariantFault {
            reason: "completed variant still owns cleanup content".into(),
        };
        let yes = self.ed.edge(invalid, &[]);
        self.ed.unit.terminators[joined.index()] = Terminator::Branch {
            condition: present,
            yes,
            no: destination,
        };
        let absent = self.ed.value(ty, Holding::Plain);
        let constant = hirsel_lir::ir::ConstantId(self.ed.unit.tables.constants.len() as u32);
        self.ed
            .unit
            .tables
            .constants
            .push(hirsel_lir::ir::ConstantData {
                ty,
                value: hirsel_lir::ir::ConstantValue::Bool(false),
            });
        self.ed.code[self.ed.unit.entry.index()].insert(
            0,
            Inst::Const {
                result: absent,
                constant,
            },
        );
        self.inserted.push((self.ed.unit.entry, 0));
        Ok(self.ed.edge(joined, &[absent]))
    }

    /// Select an intact variant's active arm. Ordinary variants use a
    /// temporary that each arm ends; resources retain their original slot
    /// so their own finalizer preserves the slot's cleanup order.
    fn variant(
        &mut self,
        at: BlockId,
        path: &[Step],
        layout: LayoutId,
        site: &Site,
        fail: &mut dyn FnMut(&mut Self, ()) -> FailureId,
    ) -> Result<BlockId, PassError> {
        let record = self.lay.get(layout).unwrap().clone();
        // A resource's own finalizer participates in its original slot's
        // cleanup order. Keep that slot and its phase through arm selection.
        // Projected payload remains an owned descendant of its original
        // slot while a containing source finalizer is live. Moving it into a
        // new temporary would break that cleanup invocation's custody.
        let held = path.is_empty() && record.finalizer.is_none();
        let kind = self.ed.unit.blocks[at.index()].kind;
        let anchor = self.ed.unit.blocks[at.index()].anchor;
        let (local, whole, joined, retry) = if held {
            self.hold_action(at, path, layout, site, fail)?
        } else {
            let whole = self.address(site.slot, path, layout)?;
            let outer = fail(self, ());
            let local = Site {
                slot: site.slot,
                version: site.version,
                anchor: site.anchor,
                failure: self.ed.unit.tables.failures[outer.index()].clone(),
            };
            (
                local,
                whole,
                self.ed.block(kind, &[], anchor),
                self.ed.block(BlockKind::FailureCleanup, &[], anchor),
            )
        };
        let base_path = if held { Vec::new() } else { path.to_vec() };
        let slot = local.slot;
        let ty = type_of(&mut self.ed.unit, ValueRepr::Aggregate, layout);
        let version = local.version.unwrap();
        let exit = self.ed.unit.tables.edges[local.failure.cleanup.index()].clone();
        let exit_args = exit
            .arguments
            .get(&self.ed.unit.pools.values)
            .unwrap_or(&[])
            .to_vec();
        let mut normal = self.ed.edge(joined, &[]);
        let mut failure = self.ed.edge(exit.target, &exit_args);
        if !held && !path.is_empty() {
            normal = self.completed_part(slot, path, normal, kind, anchor)?;
            failure =
                self.completed_part(slot, path, failure, BlockKind::FailureCleanup, anchor)?;
        }
        let variants = record.tag.as_ref().unwrap().variants.clone();
        let state = crate::insert::SlotState {
            live: Tri::Yes,
            init: Tri::Yes,
            moved: Tris::EMPTY,
            taken: Tris::EMPTY,
            fields_pending: Tris::EMPTY,
            versions: crate::insert::Versions::One(version),
            owned_empty: false,
        };
        for (start, failed) in [(retry, true), (at, false)] {
            let loan = LoanId(self.ed.unit.tables.loans.len() as u32);
            self.ed.unit.tables.loans.push(LoanInfo {
                address: whole,
                mutable: false,
            });
            let tag = self.ed.value(ty, Holding::Borrowed(loan));
            self.ed.code[start.index()].extend([
                Inst::BeginLoan {
                    loan,
                    address: whole,
                    mutable: false,
                    if_inactive: false,
                },
                Inst::Read {
                    result: tag,
                    address: whole,
                    reaching: version,
                    mode: ReadMode::Borrow,
                },
            ]);
            let mut arms = Vec::new();
            for variant in &variants {
                let arm = self.ed.block(
                    if failed {
                        BlockKind::FailureCleanup
                    } else {
                        kind
                    },
                    &[],
                    anchor,
                );
                self.ed.code[arm.index()].push(Inst::EndLoan {
                    loan,
                    if_active: false,
                });
                let mut actions = Vec::new();
                self.plan_variant_fields(
                    &state,
                    base_path.clone(),
                    &record,
                    variant.ordinal,
                    &mut actions,
                )?;
                if record.finalizer.is_some() {
                    actions = vec![Action::Finalize {
                        path: base_path.clone(),
                        layout,
                        fields: actions,
                    }];
                }
                // A nullary active arm owns no payload; taking and releasing it
                // empties the storage without reading a nonexistent field.
                if actions.is_empty() {
                    actions.push(Action::EmptyVariant {
                        path: base_path.clone(),
                        layout,
                    });
                }
                let base = self.prepare_actions(arm, &actions, &local, retry)?;
                let dead = self.ed.block(BlockKind::FailureCleanup, &[], anchor);
                if held {
                    self.ed.code[dead.index()].push(Inst::StorageDead {
                        slot,
                        if_live: false,
                    });
                }
                self.ed.unit.terminators[dead.index()] =
                    Terminator::Jump(extended(self.ed, failure, &[]));
                let edge = self.ed.edge(dead, &[]);
                let sub = Site {
                    slot,
                    version: Some(version),
                    anchor: site.anchor,
                    failure: FailureEdge {
                        cleanup: edge,
                        rule: local.failure.rule.clone(),
                        anchor,
                        dispose: local.failure.dispose,
                    },
                };
                let end = self.sequence(arm, &actions, &sub)?;
                if held {
                    self.ed.code[end.index()].push(Inst::StorageDead {
                        slot,
                        if_live: false,
                    });
                }
                self.ed.unit.terminators[end.index()] = Terminator::Jump(extended(
                    self.ed,
                    if failed { failure } else { normal },
                    &[],
                ));
                self.prepared.truncate(base);
                arms.push(hirsel_lir::ir::SwitchArm {
                    value: variant.ordinal,
                    edge: self.ed.edge(arm, &[]),
                });
            }
            let invalid = self.ed.block(
                if failed {
                    BlockKind::FailureCleanup
                } else {
                    kind
                },
                &[],
                anchor,
            );
            self.ed.unit.terminators[invalid.index()] = Terminator::Unreachable;
            let otherwise = self.ed.edge(invalid, &[]);
            let arms = PoolRange::push(&mut self.ed.unit.pools.switch_arms, arms);
            // Switch extracts the tag via the layout's TagForm, including niches.
            self.ed.unit.terminators[start.index()] = Terminator::Switch {
                value: tag,
                arms,
                otherwise,
            };
        }
        Ok(joined)
    }

    /// Intact inline owners share the same temporary and failure suffix
    /// machinery as variant owners. No payload is published outside cleanup.
    fn hold_action(
        &mut self,
        at: BlockId,
        path: &[Step],
        layout: LayoutId,
        site: &Site,
        fail: &mut dyn FnMut(&mut Self, ()) -> FailureId,
    ) -> Result<(Site, AddressId, BlockId, BlockId), PassError> {
        let source = self.address(site.slot, path, layout)?;
        let ty = type_of(&mut self.ed.unit, ValueRepr::Aggregate, layout);
        let slot = SlotId(self.ed.unit.slots.len() as u32);
        self.ed.unit.slots.push(hirsel_lir::ir::SlotInfo {
            ty,
            kind: hirsel_lir::ir::SlotKind::Temporary,
            scope: self.ed.unit.slots[site.slot.index()].scope,
        });
        let whole = self.whole(slot);
        let value = self.ed.value(ty, Holding::Owned);
        let version = self.ed.value(ty, Holding::Plain);
        self.ed.code[at.index()].extend([
            Inst::Read {
                result: value,
                address: source,
                reaching: site.version.unwrap(),
                mode: ReadMode::Take,
            },
            Inst::StorageLive { slot },
            Inst::Store {
                address: whole,
                value,
                version,
            },
        ]);
        let outer = fail(self, ());
        let failure = self.ed.unit.tables.failures[outer.index()].clone();
        let local = Site {
            slot,
            version: Some(version),
            anchor: site.anchor,
            failure,
        };
        let kind = self.ed.unit.blocks[at.index()].kind;
        let anchor = self.ed.unit.blocks[at.index()].anchor;
        let joined = self.ed.block(kind, &[], anchor);
        let retry = self.ed.block(BlockKind::FailureCleanup, &[], anchor);
        Ok((local, whole, joined, retry))
    }

    fn inline(
        &mut self,
        at: BlockId,
        path: &[Step],
        layout: LayoutId,
        site: &Site,
        fail: &mut dyn FnMut(&mut Self, ()) -> FailureId,
    ) -> Result<BlockId, PassError> {
        let (local, _, joined, retry) = self.hold_action(at, path, layout, site, fail)?;
        let LayoutKind::Inline { element, count } = self.layout_kind(layout)? else {
            unreachable!()
        };
        let state = crate::insert::SlotState {
            live: Tri::Yes,
            init: Tri::Yes,
            moved: Tris::EMPTY,
            taken: Tris::EMPTY,
            fields_pending: Tris::EMPTY,
            versions: crate::insert::Versions::One(local.version.unwrap()),
            owned_empty: false,
        };
        let mut actions = Vec::new();
        for index in (0..count).rev() {
            self.plan(&state, vec![Step::Inline(index)], element, &mut actions)?;
        }
        let exit = self.ed.unit.tables.edges[local.failure.cleanup.index()].clone();
        let args = exit
            .arguments
            .get(&self.ed.unit.pools.values)
            .unwrap_or(&[])
            .to_vec();
        for (start, failed) in [(retry, true), (at, false)] {
            let base = self.prepare_actions(start, &actions, &local, retry)?;
            let anchor = self.ed.unit.blocks[start.index()].anchor;
            let dead = self.ed.block(BlockKind::FailureCleanup, &[], anchor);
            self.ed.code[dead.index()].push(Inst::StorageDead {
                slot: local.slot,
                if_live: false,
            });
            self.ed.unit.terminators[dead.index()] =
                Terminator::Jump(self.ed.edge(exit.target, &args));
            let cleanup = self.ed.edge(dead, &[]);
            let sub = Site {
                slot: local.slot,
                version: local.version,
                anchor: local.anchor,
                failure: FailureEdge {
                    cleanup,
                    ..local.failure.clone()
                },
            };
            let end = self.sequence(start, &actions, &sub)?;
            self.ed.code[end.index()].push(Inst::StorageDead {
                slot: local.slot,
                if_live: false,
            });
            self.ed.unit.terminators[end.index()] = Terminator::Jump(if failed {
                self.ed.edge(exit.target, &args)
            } else {
                self.ed.edge(joined, &[])
            });
            self.prepared.truncate(base);
        }
        Ok(joined)
    }

    fn prepare_actions(
        &mut self,
        at: BlockId,
        actions: &[Action],
        local: &Site,
        retry: BlockId,
    ) -> Result<usize, PassError> {
        let base = self.prepared.len();
        for action in actions {
            if let Action::Finalize { fields, .. } = action {
                self.prepare_actions(at, fields, local, retry)?;
            }
            let (path, layout) = match action {
                Action::Conditional { .. } => continue,
                Action::EmptyVariant { .. } | Action::Retire { .. } => continue,
                Action::Release { path, layout, .. }
                | Action::Finalize { path, layout, .. }
                | Action::Variant { path, layout }
                | Action::Inline { path, layout } => (path, *layout),
            };
            if path.is_empty() {
                continue;
            }
            let steps = self.projections(path)?;
            let address = self.fresh_address(local.slot, path, steps, layout);
            let anchor = self.ed.unit.blocks[at.index()].anchor;
            let edge = self.ed.edge(retry, &[]);
            let failure = self.ed.failure(edge, anchor);
            self.ed.code[at.index()].push(Inst::Address { address, failure });
            self.prepared.push((local.slot, path.clone(), address));
        }
        Ok(base)
    }

    fn projections(&mut self, path: &[Step]) -> Result<Vec<ProjectionStep>, PassError> {
        let mut steps = Vec::new();
        for step in path {
            steps.push(match *step {
                Step::Payload => ProjectionStep::Payload,
                Step::Field(field) => ProjectionStep::Field(field),
                Step::Variant(variant, field) => ProjectionStep::VariantField { variant, field },
                Step::Inline(index) => {
                    let layout = self
                        .index
                        .scalar(hirsel_lir::layout_types::ScalarKind::U64)
                        .ok_or_else(|| {
                            PassError::Malformed("no u64 layout for an inline cleanup index".into())
                        })?;
                    let ty = type_of(
                        &mut self.ed.unit,
                        ValueRepr::Scalar(hirsel_lir::layout_types::ScalarKind::U64),
                        layout,
                    );
                    let result = self.ed.value(ty, Holding::Plain);
                    let constant =
                        hirsel_lir::ir::ConstantId(self.ed.unit.tables.constants.len() as u32);
                    self.ed
                        .unit
                        .tables
                        .constants
                        .push(hirsel_lir::ir::ConstantData {
                            ty,
                            value: hirsel_lir::ir::ConstantValue::Integer(u128::from(index)),
                        });
                    self.ed.code[self.ed.unit.entry.index()]
                        .insert(0, Inst::Const { result, constant });
                    self.inserted.push((self.ed.unit.entry, 0));
                    ProjectionStep::Index(result)
                }
                _ => {
                    return Err(PassError::Unsupported(
                        "a dynamic element cleanup path".into(),
                    ));
                }
            });
        }
        Ok(steps)
    }

    /// The call of `finalizer` with the resource borrowed.
    fn finalizer_call(
        &mut self,
        layout: LayoutId,
        finalizer: &Finalizer,
        argument: ArgumentValue,
        site: &Site,
    ) -> Result<CallId, PassError> {
        let symbol = match self.symbols.get(&layout) {
            Some(&s) => s,
            None => {
                let wanted = Symbol {
                    kind: SymbolKind::Function,
                    definition: finalizer.definition.clone(),
                    instance: Some(finalizer.instance),
                };
                let symbols = &mut self.ed.unit.tables.symbols;
                let s = match symbols.iter().position(|s| *s == wanted) {
                    Some(i) => SymbolId(i as u32),
                    None => {
                        symbols.push(wanted);
                        SymbolId(symbols.len() as u32 - 1)
                    }
                };
                self.symbols.insert(layout, s);
                s
            }
        };
        let abi = self
            .lay
            .0
            .fn_abis
            .get(finalizer.abi.0 as usize)
            .ok_or_else(|| PassError::Malformed("a drop body ABI outside the table".into()))?;
        let [parameter] = abi.parameters.as_slice() else {
            return Err(PassError::Malformed(
                "a drop body takes exactly its borrowed self".into(),
            ));
        };
        let arguments = PoolRange::push(
            &mut self.ed.unit.pools.call_arguments,
            [CallArgument {
                parameter: parameter.source,
                passing: parameter.convention,
                value: argument,
            }],
        );
        // This call implements a cleanup action, rather than the source
        // invocation whose failure eventually reached that action. Keep the
        // cleanup's body and diagnostic span, but use the non-source block
        // namespace so independent invocation coverage cannot confuse them.
        let cleanup = self
            .ed
            .unit
            .tables
            .cleanup_anchors
            .get(site.anchor.index())
            .ok_or_else(|| PassError::Malformed("drop body has no cleanup anchor".into()))?;
        let origin = hirsel_lir::ir::SourceAnchor {
            body: cleanup.body,
            block: u32::MAX,
            statement: self.ed.unit.tables.source_anchors.len() as u32,
            site: cleanup.site,
        };
        let anchor = match self
            .ed
            .unit
            .tables
            .source_anchors
            .iter()
            .position(|a| a == &origin)
        {
            Some(index) => hirsel_lir::ir::SourceAnchorId(index as u32),
            None => {
                let anchor =
                    hirsel_lir::ir::SourceAnchorId(self.ed.unit.tables.source_anchors.len() as u32);
                self.ed.unit.tables.source_anchors.push(origin);
                anchor
            }
        };
        let id = CallId(self.ed.unit.tables.calls.len() as u32);
        self.ed.unit.tables.calls.push(CallSite {
            receipts: None,
            pretransfer_failure: None,
            forward_caller: true,
            operation_arguments: Vec::new(),
            callee: Callee::Direct(symbol),
            abi: finalizer.abi,
            arguments,
            result: None,
            destination: None,
            declared: crate::import::CALL_EFFECTS,
            anchor,
        });
        Ok(id)
    }

    /// The actions that destroy the content at `path` still in the slot.
    fn plan(
        &self,
        s: &crate::insert::SlotState,
        path: Vec<Step>,
        layout: LayoutId,
        out: &mut Vec<Action>,
    ) -> Result<(), PassError> {
        let paths = self.facts.paths();
        let moved = s.moved.at(&paths, &path);
        if moved == Tri::Yes || self.lay.trivial(layout) {
            return Ok(());
        }
        // Whether any part under `path` is taken or a pending shell.
        let under = s.taken.iter().chain(s.fields_pending.iter()).any(|(q, _)| {
            let q = paths.steps(q);
            q.len() > path.len() && q.starts_with(&path)
        });
        if moved == Tri::Maybe {
            let condition = self
                .flags
                .get(&path)
                .copied()
                .ok_or_else(|| PassError::Malformed(format!("no drop flag for {path:?}")))?;
            let mut present = s.clone();
            present
                .moved
                .set(paths.get(&path).expect("a moved path is interned"), Tri::No);
            let mut actions = Vec::new();
            self.plan(&present, path, layout, &mut actions)?;
            out.push(Action::Conditional { condition, actions });
            return Ok(());
        }
        // Inline finalizers use the explicit phase/call/field cleanup below.
        // A whole release is only valid when destruction reaches no inline
        // finalizer; counted payloads still run their own drop programs.
        if !under
            && s.fields_pending.at(&paths, &path) == Tri::No
            && !self.lay.inline_finalizer(layout)
        {
            out.push(Action::Release {
                path,
                layout,
                moved: Vec::new(),
            });
            return Ok(());
        }
        let record = self
            .lay
            .get(layout)
            .ok_or_else(|| PassError::Malformed(format!("{layout:?} outside the table")))?;
        if let LayoutKind::Box { .. } = record.kind {
            let mut moved = Vec::new();
            for (part, state) in s.taken.sorted(&paths) {
                if !part.starts_with(&path) {
                    continue;
                }
                match (&part[path.len()..], state) {
                    ([Step::Payload, Step::Field(field) | Step::Variant(_, field)], Tri::Yes) => {
                        moved.push(*field)
                    }
                    _ => {
                        return Err(PassError::Unsupported(
                            "a conditional or nested box partial transfer".into(),
                        ));
                    }
                }
            }
            out.push(Action::Release {
                path,
                layout,
                moved,
            });
            return Ok(());
        }
        let pending = s.fields_pending.at(&paths, &path);
        if record.finalizer.is_some() && pending == Tri::Maybe {
            return Err(PassError::Malformed(
                "a retained defused shell crossed its straight-line region".into(),
            ));
        }
        // Select an intact resource's active arm while it is still live.
        // The arm finalizes the resource in place before its payload.
        if record.tag.is_some() && !under && pending != Tri::Yes {
            out.push(Action::Variant { path, layout });
            return Ok(());
        }
        if record.finalizer.is_some() && pending != Tri::Yes {
            if under {
                return Err(PassError::Malformed(
                    "a field moved out of a live resource".into(),
                ));
            }
            let mut fields = Vec::new();
            self.plan_fields(s, path.clone(), layout, under, &mut fields)?;
            out.push(Action::Finalize {
                path,
                layout,
                fields,
            });
            return Ok(());
        }
        self.plan_fields(s, path, layout, under, out)
    }

    fn plan_fields(
        &self,
        s: &crate::insert::SlotState,
        path: Vec<Step>,
        layout: LayoutId,
        under: bool,
        out: &mut Vec<Action>,
    ) -> Result<(), PassError> {
        let record = self
            .lay
            .get(layout)
            .ok_or_else(|| PassError::Malformed(format!("{layout:?} outside the table")))?;
        if record.tag.is_some() {
            return self.plan_variant(s, path, record, out);
        }
        if !under && matches!(record.kind, LayoutKind::Inline { .. }) {
            out.push(Action::Inline { path, layout });
            return Ok(());
        }
        if let LayoutKind::Inline { element, count } = record.kind {
            for index in (0..count).rev() {
                let mut part = path.clone();
                part.push(Step::Inline(index));
                self.plan(s, part, element, out)?;
            }
            return Ok(());
        }
        if !matches!(record.kind, LayoutKind::Aggregate | LayoutKind::Resource) {
            return Err(PassError::Malformed(format!(
                "a partial transfer from non-composite inline {:?} storage",
                record.kind
            )));
        }
        // Layout validation fixes this order. Byte offsets cannot identify a
        // declaration: zero-sized resources and their siblings may coincide.
        for field in record.fields.iter().rev() {
            if hirsel_lir::layout::field_is_borrowed(self.lay.0, record, field) {
                continue;
            }
            let mut inner = path.clone();
            inner.push(Step::Field(field.ordinal));
            self.plan(s, inner, field.layout, out)?;
        }
        Ok(())
    }

    /// Absent masks include inactive variants. Plan every variant's fields;
    /// per-part guards select the live payload at a correlated join.
    fn plan_variant(
        &self,
        s: &crate::insert::SlotState,
        path: Vec<Step>,
        record: &hirsel_lir::layout_types::LayoutRecord,
        out: &mut Vec<Action>,
    ) -> Result<(), PassError> {
        for variant in &record
            .tag
            .as_ref()
            .ok_or_else(|| PassError::Malformed("variant without tag".into()))?
            .variants
        {
            self.plan_variant_fields(s, path.clone(), record, variant.ordinal, out)?;
        }
        Ok(())
    }

    fn plan_variant_fields(
        &self,
        s: &crate::insert::SlotState,
        path: Vec<Step>,
        record: &hirsel_lir::layout_types::LayoutRecord,
        v: u32,
        out: &mut Vec<Action>,
    ) -> Result<(), PassError> {
        let fields = &record
            .tag
            .as_ref()
            .and_then(|t| t.variants.iter().find(|x| x.ordinal == v))
            .ok_or_else(|| PassError::Malformed(format!("no variant {v}")))?
            .fields;
        for field in fields.iter().rev() {
            let mut inner = path.clone();
            inner.push(Step::Variant(v, field.ordinal));
            self.plan(s, inner, field.layout, out)?;
        }
        Ok(())
    }

    fn layout_kind(&self, layout: LayoutId) -> Result<LayoutKind, PassError> {
        self.lay
            .get(layout)
            .map(|r| r.kind.clone())
            .ok_or_else(|| PassError::Malformed(format!("{layout:?} outside the table")))
    }

    /// The layout of a field path inside a slot.
    fn place_layout(&self, slot: SlotId, path: &[Step]) -> Result<LayoutId, PassError> {
        let unit = &self.ed.unit;
        inline_part_layout(
            self.lay,
            unit.types[unit.slots[slot.index()].ty.index()].layout,
            path,
        )?
        .ok_or_else(|| PassError::Unsupported("a drop through counted or dynamic storage".into()))
    }

    /// A field-path address of `slot`, prepared at entry beside the slot's
    /// own whole address: a slot root with field steps checks nothing, so
    /// the entry preparation dominates every use.
    fn address(
        &mut self,
        slot: SlotId,
        path: &[Step],
        layout: LayoutId,
    ) -> Result<AddressId, PassError> {
        if let Some((_, _, address)) = self
            .prepared
            .iter()
            .rev()
            .find(|(s, p, _)| *s == slot && p == path)
        {
            return Ok(*address);
        }
        let whole = self.whole(slot);
        if path.is_empty() {
            return Ok(whole);
        }
        let steps = self.projections(path)?;
        if in_place(path) {
            let id = self.fresh_address(slot, path, steps.clone(), layout);
            self.beside_variant_preparations(slot, &steps, id)?;
            return Ok(id);
        }
        let entry = self.ed.unit.entry.index();
        let unit = &self.ed.unit;
        if let Some(existing) = unit.addresses.iter().enumerate().find_map(|(a, info)| {
            let id = AddressId(a as u32);
            (info.base == AddressBase::Slot(slot)
                && info.projection.get(&unit.pools.projections) == Some(steps.as_slice())
                && self.ed.code[entry]
                    .iter()
                    .any(|i| matches!(*i, Inst::Address { address, .. } if address == id)))
            .then_some(id)
        }) {
            return Ok(existing);
        }
        let id = self.fresh_address(slot, path, steps, layout);
        // Beside the whole address's preparation, failing as it does.
        let (at, failure) = self.ed.code[entry]
            .iter()
            .enumerate()
            .find_map(|(i, inst)| match *inst {
                Inst::Address { address, failure } if address == whole => Some((i, failure)),
                _ => None,
            })
            .expect("the whole address is prepared at entry");
        let f = self.ed.unit.tables.failures[failure.index()].clone();
        let e = &self.ed.unit.tables.edges[f.cleanup.index()];
        let (target, args) = (
            e.target,
            e.arguments
                .get(&self.ed.unit.pools.values)
                .unwrap_or(&[])
                .to_vec(),
        );
        let edge = self.ed.edge(target, &args);
        let failure = self.ed.failure(edge, f.anchor);
        self.ed.code[entry].insert(
            at + 1,
            Inst::Address {
                address: id,
                failure,
            },
        );
        self.inserted.push((self.ed.unit.entry, at + 1));
        Ok(id)
    }

    /// Prepare `id` (a path through `VariantField`) beside every
    /// preparation of another address of `slot` through the same variant
    /// step: those are what the takes proving the variant active used, so
    /// they dominate the cleanup, check the same tag, and fail while the
    /// content is still whole, with their own failure edges cloned.
    fn beside_variant_preparations(
        &mut self,
        slot: SlotId,
        steps: &[ProjectionStep],
        id: AddressId,
    ) -> Result<(), PassError> {
        let n = steps
            .iter()
            // Every enclosing checked projection must already hold. A sibling
            // sharing just Ok's tag may precede the nested Some tag test.
            .rposition(|s| !matches!(s, ProjectionStep::Field(_)))
            .expect("an in-place path has a checked step");
        let selected = steps[n];
        let unit = &self.ed.unit;
        let sibling = |a: AddressId| {
            let info = &unit.addresses[a.index()];
            let p = info.projection.get(&unit.pools.projections).unwrap_or(&[]);
            a != id
                && info.base == AddressBase::Slot(slot)
                && p.len() > n
                && p[..n] == steps[..n]
                && match (selected, p[n]) {
                    (
                        ProjectionStep::VariantField { variant, .. },
                        ProjectionStep::VariantField { variant: v, .. },
                    ) => v == variant,
                    (ProjectionStep::Index(_), ProjectionStep::Index(_)) => true,
                    _ => false,
                }
        };
        let sites: Vec<(usize, usize, FailureId)> = self
            .ed
            .code
            .iter()
            .enumerate()
            .flat_map(|(b, code)| {
                code.iter()
                    .enumerate()
                    .filter_map(move |(i, inst)| match *inst {
                        Inst::Address { address, failure } if sibling(address) => {
                            Some((b, i, failure))
                        }
                        _ => None,
                    })
            })
            .collect();
        if sites.is_empty() {
            return Err(PassError::Unsupported(
                "a variant part with no prepared sibling address".into(),
            ));
        }
        // One preparation. The sibling preparations dominating the drop
        // being lowered form a chain: take its first, as a loop makes every
        // site reach every other. Otherwise drop a site another site reaches.
        let succ = self.successors();
        let dom = hirsel_graph::Dominators::new(&succ, self.ed.unit.entry.index());
        let reach: Vec<std::collections::BTreeSet<usize>> = (0..succ.len())
            .map(|b| {
                hirsel_graph::postorder(&succ, succ[b].iter().copied())
                    .into_iter()
                    .collect()
            })
            .collect();
        let before = |t: &(usize, usize, FailureId), u: &(usize, usize, FailureId)| {
            if t.0 == u.0 {
                t.1 < u.1
            } else {
                dom.dominates(t.0, u.0)
            }
        };
        let covering: Vec<_> = sites
            .iter()
            .filter(|t| dom.dominates(t.0, self.drop_at.index()))
            .copied()
            .collect();
        let sites: Vec<_> = match covering
            .iter()
            .find(|t| covering.iter().all(|u| u == *t || before(t, u)))
        {
            Some(first) => vec![*first],
            None => {
                let reaches = |t: &(usize, usize, FailureId), u: &(usize, usize, FailureId)| {
                    (t.0 == u.0 && t.1 < u.1) || (t.0 != u.0 && reach[t.0].contains(&u.0))
                };
                sites
                    .iter()
                    .filter(|u| !sites.iter().any(|t| t != *u && reaches(t, u)))
                    .copied()
                    .collect()
            }
        };
        // Later sites first, so earlier indices stay put.
        for &(b, i, failure) in sites.iter().rev() {
            let f = self.ed.unit.tables.failures[failure.index()].clone();
            let e = self.ed.unit.tables.edges[f.cleanup.index()].clone();
            let args = e
                .arguments
                .get(&self.ed.unit.pools.values)
                .unwrap_or(&[])
                .to_vec();
            let cleanup = self.ed.edge(e.target, &args);
            let dispose = f
                .dispose
                .get(&self.ed.unit.pools.values)
                .unwrap_or(&[])
                .to_vec();
            let dispose = PoolRange::push(&mut self.ed.unit.pools.values, dispose);
            let clone = FailureId(self.ed.unit.tables.failures.len() as u32);
            self.ed.unit.tables.failures.push(FailureEdge {
                cleanup,
                rule: f.rule.clone(),
                anchor: f.anchor,
                dispose,
            });
            self.ed.code[b].insert(
                i + 1,
                Inst::Address {
                    address: id,
                    failure: clone,
                },
            );
            self.inserted.push((BlockId(b as u32), i + 1));
        }
        Ok(())
    }

    /// Each block's successors, through normal and failure edges alike.
    fn successors(&self) -> Vec<Vec<usize>> {
        Flow {
            unit: &self.ed.unit,
            code: &self.ed.code,
            lay: self.lay,
            facts: self.facts,
        }
        .successors()
        .into_iter()
        .map(|targets| targets.into_iter().map(|b| b.index()).collect())
        .collect()
    }

    /// A new unprepared address of `slot` at `path`.
    fn fresh_address(
        &mut self,
        slot: SlotId,
        path: &[Step],
        steps: Vec<ProjectionStep>,
        layout: LayoutId,
    ) -> AddressId {
        let repr = self
            .lay
            .get(layout)
            .map_or(ValueRepr::Aggregate, |r| crate::import::repr_of(&r.kind));
        let ty = type_of(&mut self.ed.unit, repr, layout);
        let projection = PoolRange::push(&mut self.ed.unit.pools.projections, steps);
        let id = AddressId(self.ed.unit.addresses.len() as u32);
        self.ed.unit.addresses.push(AddressInfo {
            source_place: None,
            unchecked: Vec::new(),
            base: AddressBase::Slot(slot),
            projection,
            ty,
        });
        self.facts.places.push(Some(Place {
            root: Root::Slot(slot),
            path: path.to_vec(),
            crossings: Vec::new(),
        }));
        id
    }
}

/// Whether a plan's path cannot be prepared at entry: a `VariantField`
/// step checks the tag, which holds only once the content is there (and
/// here, only for the variant a taken part showed active).
fn in_place(path: &[Step]) -> bool {
    path.iter().any(|s| !matches!(s, Step::Field(_)))
}

#[cfg(test)]
mod pending_tests {
    use super::*;
    use hirsel_heapcheck::build::{Layouts, fixture_unit, path};
    use hirsel_lir::ir::{AddressBase, SlotKind};
    use hirsel_lir::layout_types::{ScalarKind, TargetSpec};
    use hirsel_lir::testing::UnitBuilder;

    /// Defuse's FieldsPending state plans normal and local failure cleanup
    /// without changing the version or the remaining field obligations.
    #[test]
    fn a_pending_shell_skips_its_parent_and_drops_only_remaining_fields() {
        let mut layouts = Layouts::new(TargetSpec::x86_64_unknown_linux_gnu());
        let number = layouts.scalar(ScalarKind::U64);
        let block = layouts.block(number);
        let resource = layouts.resource(&[block, block], Some(path("drop_parent")));
        let (_, table) = layouts.finish();
        let lay = Lay(&table);
        let mut builder = UnitBuilder::<Ready>::new(fixture_unit(path("fixture")));
        let entry = builder.block(BlockKind::Body, &[]);
        builder.unit.entry = entry;
        builder.term(entry, Terminator::Return(None));
        let slot = builder.slot(ValueRepr::Aggregate, resource, SlotKind::Local);
        let address = builder.address(AddressBase::Slot(slot), &[], ValueRepr::Aggregate, resource);
        let version = builder.plain(ValueRepr::Aggregate, resource);
        let unit = builder.finish_with_layouts(&table);
        let mut facts = Facts::new(&unit, &table);
        let mut ed = Editor::new(unit);
        let flow = Flow {
            unit: &ed.unit,
            code: &ed.code,
            lay,
            facts: &facts,
        };
        let mut state = flow.entry_state();
        state.slots[slot.index()].live = Tri::Yes;
        state.slots[slot.index()].init = Tri::Yes;
        state.slots[slot.index()].versions = Versions::One(version);
        let field = facts.intern(&[Step::Field(0)]);
        state.slots[slot.index()].moved.set(field, Tri::Yes);
        state.slots[slot.index()].taken.set(field, Tri::Yes);
        let before = state.slots[slot.index()].clone();
        flow.fields_pending(&mut state, address);
        let slot_state = &state.slots[slot.index()];
        assert_eq!(slot_state.versions, before.versions);
        assert_eq!(slot_state.moved, before.moved);
        assert_eq!(slot_state.taken, before.taken);
        let finalizers = Finalizers::default();
        let index = hirsel_lir::LayoutIndex::new(&table);
        let lowering = Lowering {
            index: &index,
            ed: &mut ed,
            facts: &mut facts,
            lay,
            finalizers: &finalizers,
            symbols: BTreeMap::new(),
            inserted: vec![],
            prepared: vec![],
            flags: BTreeMap::new(),
            retire_flags: BTreeMap::new(),
            drop_at: BlockId(0),
        };
        let mut actions = Vec::new();
        lowering
            .plan(slot_state, vec![], resource, &mut actions)
            .unwrap();
        assert!(
            matches!(actions.as_slice(), [Action::Release { path, layout, .. }] if *path == [Step::Field(1)] && *layout == block)
        );
        assert!(
            lowering
                .plan(&before, vec![], resource, &mut vec![])
                .is_err(),
            "a live parent cannot already have a field taken"
        );
        let mut empty = slot_state.clone();
        let field = lowering.facts.intern(&[Step::Field(1)]);
        empty.moved.set(field, Tri::Yes);
        empty.taken.set(field, Tri::Yes);
        assert!(absent(
            &empty,
            &lowering.facts.paths(),
            &mut vec![],
            resource,
            lay
        ));
        let mut actions = vec![];
        lowering
            .plan(&empty, vec![], resource, &mut actions)
            .unwrap();
        assert!(actions.is_empty());
    }
    #[test]
    fn zero_sized_children_keep_their_declaration_order_and_individual_obligations() {
        let mut layouts = Layouts::new(TargetSpec::x86_64_unknown_linux_gnu());
        let child = layouts.resource(&[], Some(path("drop_child")));
        let parent = layouts.resource(&[child, child], Some(path("drop_parent")));
        let variant = layouts.variant(&[&[child, child], &[]]);
        let (_, table) = layouts.finish();
        assert_eq!(
            table.layout(parent).unwrap().fields[0].offset,
            table.layout(parent).unwrap().fields[1].offset
        );
        let mut builder = UnitBuilder::<Ready>::new(fixture_unit(path("zero_sized_cleanup")));
        builder.unit.header.stage = StageKind::Ready;
        let entry = builder.block(BlockKind::Body, &[]);
        builder.term(entry, Terminator::Return(None));
        let unit = builder.finish_with_layouts(&table);
        let mut facts = Facts::new(&unit, &table);
        let mut ed = Editor::new(unit);
        let finalizers = Finalizers::default();
        let index = hirsel_lir::LayoutIndex::new(&table);
        let lowering = Lowering {
            index: &index,
            ed: &mut ed,
            facts: &mut facts,
            lay: Lay(&table),
            finalizers: &finalizers,
            symbols: BTreeMap::new(),
            inserted: vec![],
            prepared: vec![],
            flags: BTreeMap::new(),
            retire_flags: BTreeMap::new(),
            drop_at: BlockId(0),
        };
        let mut state = crate::insert::SlotState {
            live: Tri::Yes,
            init: Tri::Yes,
            moved: Tris::EMPTY,
            taken: Tris::EMPTY,
            fields_pending: Tris::EMPTY,
            versions: Versions::None,
            owned_empty: false,
        };
        let variant_state = state.clone();
        fn paths_of(actions: &[Action]) -> Vec<Vec<Step>> {
            let mut paths = Vec::new();
            for action in actions {
                match action {
                    Action::Release { path, .. } => paths.push(path.clone()),
                    Action::Finalize { path, fields, .. } => {
                        paths.push(path.clone());
                        paths.extend(paths_of(fields));
                    }
                    _ => panic!("unexpected {action:?}"),
                }
            }
            paths
        }
        // Inline finalizers are explicit phases: the parent finalizes in place,
        // then each zero-sized child keeps its own obligation in reverse order.
        let mut actions = vec![];
        lowering.plan(&state, vec![], parent, &mut actions).unwrap();
        assert!(
            actions
                .iter()
                .all(|action| matches!(action, Action::Finalize { .. }))
        );
        assert_eq!(
            paths_of(&actions),
            [vec![], vec![Step::Field(1)], vec![Step::Field(0)]]
        );
        state
            .fields_pending
            .set(lowering.facts.intern(&[]), Tri::Yes);
        let mut actions = vec![];
        lowering.plan(&state, vec![], parent, &mut actions).unwrap();
        assert_eq!(
            paths_of(&actions),
            [vec![Step::Field(1)], vec![Step::Field(0)]]
        );
        let field = lowering.facts.intern(&[Step::Field(1)]);
        state.moved.set(field, Tri::Yes);
        state.taken.set(field, Tri::Yes);
        let mut actions = vec![];
        lowering.plan(&state, vec![], parent, &mut actions).unwrap();
        assert_eq!(paths_of(&actions), [vec![Step::Field(0)]]);
        let mut actions = vec![];
        lowering
            .plan_variant_fields(
                &variant_state,
                vec![],
                table.layout(variant).unwrap(),
                0,
                &mut actions,
            )
            .unwrap();
        assert_eq!(
            paths_of(&actions),
            [vec![Step::Variant(0, 1)], vec![Step::Variant(0, 0)]]
        );
    }
}

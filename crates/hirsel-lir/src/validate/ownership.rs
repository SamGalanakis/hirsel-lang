//! Ownership: slot liveness, initialization masks and reaching versions, loans
//! and borrowed views, prepared places, compiler-local regions, the cleanup
//! nesting order, and the exact holder-credit replay over normal and failure
//! edges.
//!
//! Two passes. The lattice pass solves slot, loan, region, stale-view,
//! unplaced-address and cleanup-order states to a fixpoint with `flow::solve`. The check pass then
//! walks every block once in reverse postorder from its converged state,
//! reporting violations and replaying holder credits. Credits are exact, not
//! a lattice: a join's predecessors must agree on every credit (must-ownership;
//! a join never invents a holder), so the first predecessor in reverse
//! postorder fixes a block's credits and every other edge is compared to it.
use super::flow;
use super::structure::{
    self, Backing, Facts, Root, Step, invalidates, overlap, terminator_edges, through_slot,
};
use super::tokens;
use super::{Cx, Report, Site, ViolationKind as K};
use crate::ir::{
    AddressId, ArgumentValue, BlockId, BlockKind, CaptureUse, CleanupAnchorId, EdgeId, FailureId,
    FlatFieldOp, Holding, INSTRUCTION_DROP, Inst, LOOP_RELEASE, LoanId, Placement,
    SCRATCH_DISPOSAL, STATE_RELEASE, ScopeId, ScopeInfo, SlotId, SlotKind, StorageOwner,
    Terminator, UnitOrigin, ValueId, ValueRepr,
};
use crate::layout_types::LayoutId;
use crate::persistent::{Element, PVec};
use crate::stage::{Stage, StageKind};
use hirsel_core_contract::Passing;
use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Tri {
    No,
    Yes,
    Maybe,
}

impl Tri {
    fn join(self, other: Self) -> Self {
        if self == other { self } else { Tri::Maybe }
    }
    fn may(self) -> bool {
        self != Tri::No
    }
}

/// `Element` flags. Dead storage, an inactive loan, a region not entered:
/// what most of a state is.
const DEAD: u8 = 1;
/// Joined with a `DEAD` element, this one stays as it is.
const ABSORBS: u8 = 2;
/// Storage holding nothing: uninitialized on every path.
const EMPTY: u8 = 4;

impl Element for Tri {
    fn flags(&self) -> u8 {
        match self {
            Tri::No => DEAD | ABSORBS,
            Tri::Maybe => ABSORBS,
            Tri::Yes => 0,
        }
    }
    fn absorbs(own: u8, other: u8) -> bool {
        own & ABSORBS != 0 && other & DEAD != 0
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct SlotState {
    live: Tri,
    init: Tri,
    /// Absent owners: transfers, inactive variants, and completed finalizers.
    moved: BTreeMap<Vec<Step>, Tri>,
    /// Actual transfers, separate from fields absent on inactive variant edges.
    taken: BTreeMap<Vec<Step>, Tri>,
    /// Versions that may reach here; a read needs exactly one.
    versions: Versions,
    /// The possible finalization phases of each inline resource place
    /// finalized in this content (`LIVE | FINALIZING | PENDING` bits); a
    /// place not listed is Live.
    finalize: BTreeMap<Vec<Step>, u8>,
    /// Every initialized incoming path has transferred or destroyed all
    /// owners. This proof survives joins that lose moved/finalized correlation.
    /// It does not say that the slot's remaining plain bytes are uninitialized.
    owned_empty: bool,
}

/// The versions of a slot that may reach a point. A read needs exactly one,
/// so every set of two or more is one state, `Many`: a join stays the same
/// size however many paths meet, and every check reads the same verdict.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Versions {
    None,
    One(ValueId),
    Many,
}

impl Versions {
    fn join(self, other: Self) -> Self {
        match (self, other) {
            (Versions::None, x) | (x, Versions::None) => x,
            (Versions::One(a), Versions::One(b)) if a == b => self,
            _ => Versions::Many,
        }
    }
}

impl From<Option<ValueId>> for Versions {
    fn from(v: Option<ValueId>) -> Self {
        v.map_or(Versions::None, Versions::One)
    }
}

/// Finalization phases of an inline resource.
const LIVE: u8 = 1;
const FINALIZING: u8 = 2;
const PENDING: u8 = 4;

impl SlotState {
    const DEAD: Self = Self {
        live: Tri::No,
        init: Tri::No,
        moved: BTreeMap::new(),
        taken: BTreeMap::new(),
        versions: Versions::None,
        finalize: BTreeMap::new(),
        owned_empty: false,
    };
    fn phase(&self, path: &[Step]) -> u8 {
        self.finalize.get(path).copied().unwrap_or(LIVE)
    }
    /// New content at `path`: every resource inside it is Live again.
    fn renew(&mut self, path: &[Step]) {
        self.finalize.retain(|q, _| !q.starts_with(path));
    }
}

impl Element for SlotState {
    fn flags(&self) -> u8 {
        let bare = self.moved.is_empty()
            && self.taken.is_empty()
            && self.finalize.is_empty()
            && !self.owned_empty;
        let dead =
            self.live == Tri::No && self.init == Tri::No && bare && self.versions == Versions::None;
        // `join_slot` with dead storage: `Yes` becomes `Maybe`, and an
        // uninitialized side takes the other's (empty) moved parts and phases.
        let absorbs =
            self.live != Tri::Yes && self.init != Tri::Yes && (self.init != Tri::No || bare);
        u8::from(dead) * DEAD + u8::from(absorbs) * ABSORBS + u8::from(self.init == Tri::No) * EMPTY
    }
    fn absorbs(own: u8, other: u8) -> bool {
        own & ABSORBS != 0 && other & DEAD != 0
    }
}

/// The state at one program point. Every part is shared with the states it
/// was cloned from until it is written, so a clone costs nothing and a join
/// of two states visits only where they differ.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct State {
    /// Explicit Defuse regions still retaining owned fields.
    defusing: Rc<BTreeSet<(SlotId, Vec<Step>)>>,
    slots: PVec<SlotState>,
    loans: PVec<Tri>,
    regions: PVec<Tri>,
    /// Views whose backing was written, moved or dropped on some path.
    stale: Rc<BTreeSet<ValueId>>,
    /// Dynamic addresses whose root a write may have changed above one of
    /// their checks or block dereferences on some path since their
    /// preparation: the location prepared may no longer be the place.
    unplaced: Rc<BTreeSet<AddressId>>,
    /// Within a cleanup sequence, the possible last root releases as
    /// (scope, temporary, slot).
    order: Rc<BTreeSet<(ScopeId, bool, u32)>>,
    /// Fields moved out of owned block values (`read take` through
    /// `Value(d)`, LIR schema revision 5), Yes or on some path.
    parts: Rc<BTreeMap<ValueId, BTreeMap<Vec<Step>, Tri>>>,
    /// Each slot's current drop flag (LIR schema revision 8): a bool that is
    /// true exactly when the slot holds content here. A flag parameter
    /// becomes current on entry; any change to the slot's liveness or
    /// initialization ends it, and a `Replace` that may displace nothing
    /// hands it to the displaced slot.
    flags: Rc<BTreeMap<(SlotId, Vec<Step>), ValueId>>,
    /// Resource phases on alternatives where each current flag is true.
    /// None means no true input; missing phase rows mean Live.
    flag_phases: Rc<BTreeMap<(SlotId, Vec<Step>), Option<BTreeMap<Vec<Step>, u8>>>>,
}

/// Whether a slot holds content: live and initialized.
fn content(s: &SlotState) -> Tri {
    match (s.live, s.init) {
        (Tri::Yes, Tri::Yes) => Tri::Yes,
        (Tri::No, _) | (_, Tri::No) => Tri::No,
        _ => Tri::Maybe,
    }
}

/// Independently reconstruct a flag's exact initialized part, including
/// ancestor moves. A descendant hole leaves the containing part present.
fn part_content(s: &SlotState, path: &[Step]) -> Tri {
    let mut state = content(s);
    if state == Tri::No {
        return state;
    }
    for (prefix, &moved) in &s.moved {
        if path.starts_with(prefix) {
            if moved == Tri::Yes {
                return Tri::No;
            }
            if moved == Tri::Maybe {
                state = Tri::Maybe;
            }
        }
    }
    // A live variant is not evidence that a particular payload is active.
    // Switches and successful variant preparations explicitly record every
    // selected field, including Tri::No for an intact active field.
    for (n, step) in path.iter().enumerate() {
        if matches!(step, Step::Variant(_, _)) && !s.moved.contains_key(&path[..=n]) {
            state = Tri::Maybe;
        }
    }
    state
}

/// A part's state in one side's moved or taken rows: a part under a part
/// moved out on that side is absent there too, so a join keeps a part
/// absent on every path where some enclosing part is.
fn absence(rows: &BTreeMap<Vec<Step>, Tri>, path: &[Step], unknown: Tri) -> Tri {
    if rows
        .iter()
        .any(|(prefix, &t)| t == Tri::Yes && path.starts_with(prefix))
    {
        return Tri::Yes;
    }
    rows.get(path).copied().unwrap_or(unknown)
}

fn join_slot(x: &mut SlotState, y: &SlotState) {
    {
        x.live = x.live.join(y.live);
        // Moved parts describe the paths where the slot is initialized: an
        // uninitialized side contributes nothing, so a part moved out on every
        // initialized path stays moved (a loop's first entry, a branch that
        // never stored).
        if x.init == Tri::No || (x.owned_empty && y.init != Tri::No) {
            x.moved = y.moved.clone();
        } else if y.init != Tri::No && !y.owned_empty && x.moved != y.moved {
            let keys: BTreeSet<_> = x.moved.keys().chain(y.moved.keys()).cloned().collect();
            let (l, r) = (x.moved.clone(), y.moved.clone());
            for key in keys {
                let unknown = if matches!(key.last(), Some(Step::Variant(_, _))) {
                    Tri::Maybe
                } else {
                    Tri::No
                };
                let joined = absence(&l, &key, unknown).join(absence(&r, &key, unknown));
                x.moved.insert(key, joined);
            }
        }
        if x.init == Tri::No {
            x.taken = y.taken.clone();
        } else if y.init != Tri::No && x.taken != y.taken {
            let keys: BTreeSet<_> = x.taken.keys().chain(y.taken.keys()).cloned().collect();
            let (l, r) = (x.taken.clone(), y.taken.clone());
            for key in keys {
                let joined = absence(&l, &key, Tri::No).join(absence(&r, &key, Tri::No));
                x.taken.insert(key, joined);
            }
        }
        x.versions = x.versions.join(y.versions);
        // Finalization phases likewise describe initialized content.
        if x.init == Tri::No || (x.owned_empty && y.init != Tri::No) {
            x.finalize = y.finalize.clone();
        } else if y.init != Tri::No && !y.owned_empty && x.finalize != y.finalize {
            let keys: BTreeSet<_> = x
                .finalize
                .keys()
                .chain(y.finalize.keys())
                .cloned()
                .collect();
            for key in keys {
                let phase = x.phase(&key) | y.phase(&key);
                x.finalize.insert(key, phase);
            }
        }
        if x.init == Tri::No {
            x.owned_empty = y.owned_empty;
        } else if y.init != Tri::No {
            x.owned_empty &= y.owned_empty;
        }
        x.init = x.init.join(y.init);
    }
}

fn join(a: &mut State, b: &State) {
    if !Rc::ptr_eq(&a.defusing, &b.defusing) && !b.defusing.is_subset(&a.defusing) {
        Rc::make_mut(&mut a.defusing).extend(b.defusing.iter().cloned());
    }
    a.slots.join(&b.slots, join_slot);
    a.loans.join(&b.loans, |x, y| *x = x.join(*y));
    a.regions.join(&b.regions, |x, y| *x = x.join(*y));
    if !Rc::ptr_eq(&a.stale, &b.stale) && !b.stale.is_subset(&a.stale) {
        Rc::make_mut(&mut a.stale).extend(b.stale.iter().copied());
    }
    if !Rc::ptr_eq(&a.unplaced, &b.unplaced) && !b.unplaced.is_subset(&a.unplaced) {
        Rc::make_mut(&mut a.unplaced).extend(b.unplaced.iter().copied());
    }
    if !Rc::ptr_eq(&a.order, &b.order) && !b.order.is_subset(&a.order) {
        Rc::make_mut(&mut a.order).extend(b.order.iter().copied());
    }
    if !Rc::ptr_eq(&a.parts, &b.parts) && a.parts != b.parts {
        let mut parts = (*a.parts).clone();
        let values: BTreeSet<_> = a.parts.keys().chain(b.parts.keys()).copied().collect();
        for v in values {
            let x = a.parts.get(&v).cloned().unwrap_or_default();
            let y = b.parts.get(&v).cloned().unwrap_or_default();
            let keys: BTreeSet<_> = x.keys().chain(y.keys()).cloned().collect();
            let joined = keys
                .into_iter()
                .map(|k| {
                    let l = x.get(&k).copied().unwrap_or(Tri::No);
                    let r = y.get(&k).copied().unwrap_or(Tri::No);
                    (k, l.join(r))
                })
                .collect();
            parts.insert(v, joined);
        }
        a.parts = Rc::new(parts);
    }
    if !Rc::ptr_eq(&a.flags, &b.flags) && a.flags != b.flags {
        Rc::make_mut(&mut a.flags).retain(|s, v| b.flags.get(s) == Some(v));
    }
    let mut phases = (*a.flag_phases).clone();
    phases.retain(|part, own| {
        if !a.flags.contains_key(part) {
            return false;
        }
        let Some(other) = b.flag_phases.get(part) else {
            return false;
        };
        match (&mut *own, other) {
            (None, other) => *own = other.clone(),
            (Some(own), Some(other)) => {
                let paths: BTreeSet<_> = own.keys().chain(other.keys()).cloned().collect();
                for path in paths {
                    let phase = own.get(&path).copied().unwrap_or(LIVE)
                        | other.get(&path).copied().unwrap_or(LIVE);
                    own.insert(path, phase);
                }
            }
            _ => {}
        }
        true
    });
    if phases != *a.flag_phases {
        a.flag_phases = Rc::new(phases);
    }
}

pub(super) type Credits = BTreeMap<ValueId, u32>;

/// Whether some field of `value` is or may be moved out.
fn moved_out(st: &State, value: ValueId) -> bool {
    st.parts
        .get(&value)
        .is_some_and(|m| m.values().any(|t| t.may()))
}

pub(super) fn check<S: Stage>(cx: &Cx<'_, S>, facts: &Facts, report: &mut Report) {
    let unit = cx.unit;
    tokens::check(cx, facts, report);
    let mut views: BTreeMap<SlotId, Vec<(ValueId, ValueId)>> = BTreeMap::new();
    for (v, backing) in facts.backing.iter().enumerate() {
        if let Backing::Slot { slot, .. } = backing {
            let v = ValueId(v as u32);
            views.entry(*slot).or_default().push((v, v));
        }
    }
    // Edge transfer snapshots validity before installing each parameter.
    // Retain its actual storage/owned backing, not an alternate borrowed
    // parameter's current incarnation (which may since have been rebound).
    let mut bases: BTreeMap<ValueId, Vec<ValueId>> = BTreeMap::new();
    for (b, block) in unit.blocks.iter().enumerate() {
        for &p in block.parameters.get(&unit.pools.values).unwrap_or(&[]) {
            let Some(join) = &facts.joins[p.index()] else {
                continue;
            };
            let mut pending: Vec<_> = join.sources.iter().copied().collect();
            let mut seen = BTreeSet::from([p]);
            while let Some(source) = pending.pop() {
                if !seen.insert(source) {
                    continue;
                }
                match &facts.backing[source.index()] {
                    Backing::Value(base) => pending.push(*base),
                    Backing::Slot { slot, .. } => {
                        views.entry(*slot).or_default().push((p, source));
                    }
                    Backing::None => {}
                }
                if let Some(join) = &facts.joins[source.index()] {
                    pending.extend(join.sources.iter().copied());
                }
                if unit.values[source.index()].holding == Holding::Owned
                    && structure::in_scope_at(cx, facts, source, BlockId(b as u32))
                {
                    bases.entry(p).or_default().push(source);
                }
            }
        }
    }
    // Per slot, the addresses rooted there whose place a write may move.
    let mut placed: BTreeMap<SlotId, Vec<(AddressId, Vec<usize>)>> = BTreeMap::new();
    for (a, addr) in facts.addresses.iter().enumerate() {
        if let Some(addr) = addr
            && let Root::Slot(slot) = addr.root
        {
            let anchors = addr.anchors();
            if !anchors.is_empty() {
                placed
                    .entry(slot)
                    .or_default()
                    .push((AddressId(a as u32), anchors));
            }
        }
    }
    let mut joined: BTreeMap<LoanId, Vec<ValueId>> = BTreeMap::new();
    for (v, join) in facts.joins.iter().enumerate() {
        for &loan in join.iter().filter(|j| j.joined()).flat_map(|j| &j.loans) {
            joined.entry(loan).or_default().push(ValueId(v as u32));
        }
    }
    let init = State {
        defusing: Rc::default(),
        slots: unit
            .slots
            .iter()
            .enumerate()
            .map(|(s, slot)| {
                let entered = matches!(slot.kind, SlotKind::Parameter(_) | SlotKind::Capture(_));
                let versions = match slot.kind {
                    SlotKind::Capture(ordinal) => unit.captures.get(ordinal as usize),
                    _ => facts
                        .parameters
                        .iter()
                        .position(|&(_, p)| p == Some(SlotId(s as u32)))
                        .and_then(|i| unit.parameters.get(i)),
                }
                .copied()
                .into();
                SlotState {
                    live: if entered || slot.kind == SlotKind::ReturnSlot {
                        Tri::Yes
                    } else {
                        Tri::No
                    },
                    init: if entered { Tri::Yes } else { Tri::No },
                    moved: BTreeMap::new(),
                    taken: BTreeMap::new(),
                    versions,
                    finalize: BTreeMap::new(),
                    owned_empty: false,
                }
            })
            .collect(),
        loans: unit.tables.loans.iter().map(|_| Tri::No).collect(),
        regions: unit.tables.regions.iter().map(|_| Tri::No).collect(),
        stale: Rc::default(),
        unplaced: Rc::default(),
        order: Rc::default(),
        parts: Rc::default(),
        flags: Rc::default(),
        flag_phases: Rc::default(),
    };
    let cleanup_retirements =
        match crate::invocations::cleanup_retirements(unit, cx.operations, cx.layouts.0) {
            Ok(retirements) => retirements,
            Err(message) => {
                report.add(Site::UNIT, K::Schema(message));
                BTreeMap::new()
            }
        };
    let mut walker = Walker {
        cx,
        facts,
        views: &views,
        placed: &placed,
        bases: &bases,
        joined: &joined,
        cleanup_retirements: &cleanup_retirements,
        report: None,
        discharged: BTreeSet::new(),
        flat_reaching: BTreeMap::new(),
        open_reaching: BTreeMap::new(),
    };
    let ins = flow::solve(
        &facts.cfg,
        init,
        |b, st| {
            walker
                .block(b, st.clone(), Credits::new())
                .into_iter()
                .map(|(e, s, _)| (e, s))
                .collect()
        },
        join,
    );
    let mut walker = Walker {
        cx,
        facts,
        views: &views,
        placed: &placed,
        bases: &bases,
        joined: &joined,
        cleanup_retirements: &cleanup_retirements,
        report: Some(report),
        discharged: BTreeSet::new(),
        flat_reaching: BTreeMap::new(),
        open_reaching: BTreeMap::new(),
    };
    let mut credits_in: Vec<Option<Credits>> = vec![None; unit.blocks.len()];
    credits_in[unit.entry.index()] = Some(
        unit.parameters
            .iter()
            .filter(|&&p| unit.values[p.index()].holding == Holding::Owned && cx.owns(p))
            .map(|&p| (p, 1))
            .collect(),
    );
    for &b in &facts.cfg.rpo {
        let Some(state) = ins[b.index()].clone() else {
            continue;
        };
        let credits = credits_in[b.index()].clone().unwrap_or_default();
        for (edge, _, out) in walker.block(b, state, credits) {
            let target = unit.tables.edges[edge.index()].target;
            match &credits_in[target.index()] {
                None => credits_in[target.index()] = Some(out),
                Some(first) => {
                    let keys: BTreeSet<_> = first.keys().chain(out.keys()).copied().collect();
                    for value in keys {
                        if first.get(&value) != out.get(&value) {
                            walker.err(
                                Site::block(target),
                                K::JoinCredits {
                                    block: target,
                                    value,
                                },
                            );
                        }
                    }
                }
            }
        }
    }
    super::flat_walk::check(
        cx,
        facts,
        &walker.flat_reaching,
        walker.report.as_deref_mut().unwrap(),
    );
    super::flat_open::check(
        cx,
        facts,
        &walker.open_reaching,
        walker.report.as_deref_mut().unwrap(),
    );
    for (a, _) in unit.tables.cleanup_anchors.iter().enumerate() {
        let anchor = CleanupAnchorId(a as u32);
        if !walker.discharged.contains(&anchor) {
            walker.err(Site::UNIT, K::SkippedAnchor(anchor));
        }
    }
}

/// Within one scope temporaries release before names, each kind in descending
/// slot order, and a scope releases only after its descendant scopes, never
/// after an unrelated one (TIR `cleanup_order::releases_after`, DROP-2).
fn releases_after(
    scopes: &[ScopeInfo],
    previous: (ScopeId, bool, u32),
    next: (ScopeId, bool, u32),
) -> bool {
    if next.0 != previous.0 {
        return strict_ancestor(scopes, next.0, previous.0);
    }
    match (previous.1, next.1) {
        (false, true) => false,
        (true, false) => true,
        _ => next.2 < previous.2,
    }
}

/// Whether `outer` encloses `inner`; the structure phase rejects cycles, and
/// the walk is bounded regardless.
fn strict_ancestor(scopes: &[ScopeInfo], outer: ScopeId, inner: ScopeId) -> bool {
    let mut at = scopes.get(inner.index()).and_then(|s| s.parent);
    for _ in 0..scopes.len() {
        match at {
            Some(p) if p == outer => return true,
            Some(p) => at = scopes.get(p.index()).and_then(|s| s.parent),
            None => return false,
        }
    }
    false
}

struct Walker<'w, 'a, S> {
    cx: &'w Cx<'a, S>,
    facts: &'w Facts,
    /// Per slot, each view a write to it may make stale, with the value
    /// whose backing names the part (itself, or a joined view's source).
    views: &'w BTreeMap<SlotId, Vec<(ValueId, ValueId)>>,
    /// Per slot, its dynamic addresses with their `Addr::anchors`.
    placed: &'w BTreeMap<SlotId, Vec<(AddressId, Vec<usize>)>>,
    /// Per borrowed join parameter, values in scope at the join whose
    /// payloads its views are.
    bases: &'w BTreeMap<ValueId, Vec<ValueId>>,
    /// Per loan, the joined views it covers: they go stale when it ends.
    joined: &'w BTreeMap<LoanId, Vec<ValueId>>,
    cleanup_retirements: &'w BTreeMap<(BlockId, u32), AddressId>,
    report: Option<&'w mut Report>,
    discharged: BTreeSet<CleanupAnchorId>,
    flat_reaching: BTreeMap<Site, Option<ValueId>>,
    /// The version reaching each write of an open element (`flat_open`).
    open_reaching: BTreeMap<Site, Option<ValueId>>,
}

type Outs = Vec<(EdgeId, State, Credits)>;

impl<S: Stage> Walker<'_, '_, S> {
    fn err(&mut self, site: Site, kind: K) {
        if let Some(report) = self.report.as_deref_mut() {
            report.add(site, kind);
        }
    }

    fn holding(&self, value: ValueId) -> Holding {
        self.cx.unit.values[value.index()].holding
    }

    fn block(&mut self, b: BlockId, mut st: State, mut cr: Credits) -> Outs {
        let unit = self.cx.unit;
        let block = &unit.blocks[b.index()];
        let insts = block.instructions.get(&unit.instructions).unwrap_or(&[]);
        let mut outs = Vec::new();
        for (i, inst) in insts.iter().enumerate() {
            let site = Site::at(b, i as u32);
            // Release, reset, drop and region exit finish their cleanup
            // before failing. Any other failing operation has disposed of
            // the owned operands it takes, as call entry transfers Take
            // actuals, and commits nothing else.
            let failure = Inst::failure(inst);
            let finishes = matches!(
                inst,
                Inst::Release { .. }
                    | Inst::Reset { .. }
                    | Inst::Drop { .. }
                    | Inst::RegionExit { .. }
            );
            let pre = (failure.is_some() && !finishes).then(|| (st.clone(), cr.clone()));
            let flagged: Vec<_> = st
                .flags
                .iter()
                .map(|(p, &v)| (p.clone(), v, self.flag_content(&st, p.0, &p.1)))
                .collect();
            self.inst(site, b, inst, &mut st, &mut cr);
            if !flagged.is_empty() {
                self.carry_flags(inst, &flagged, &mut st);
            }
            if let Some(failure) = failure {
                let (fst, mut fcr) = match pre {
                    None => (st.clone(), cr.clone()),
                    Some((pst, mut pcr)) => {
                        for v in self.taken(inst) {
                            match pcr.get(&v).copied() {
                                Some(n) if n > 1 => {
                                    pcr.insert(v, n - 1);
                                }
                                Some(_) => {
                                    pcr.remove(&v);
                                }
                                None => {}
                            }
                        }
                        (pst, pcr)
                    }
                };
                if let Inst::Reset { result, .. } = *inst {
                    fcr.remove(&result);
                }
                outs.push(self.failure(site, b, failure, fst, fcr));
            }
        }
        let site = Site::at(b, insts.len() as u32);
        let term = &unit.terminators[b.index()];
        match *term {
            Terminator::Jump(_) | Terminator::Unreachable | Terminator::InvariantFault { .. } => {}
            Terminator::Branch { condition, .. } => self.borrow(site, &st, &cr, condition),
            Terminator::Switch { value, .. } => self.borrow(site, &st, &cr, value),
            Terminator::Recover { arena, .. } => self.borrow(site, &st, &cr, arena),
            Terminator::Isolate { start, .. } => self.borrow(site, &st, &cr, start),
            Terminator::Call {
                call,
                normal,
                failure,
            } => {
                let site_data = &unit.tables.calls[call.index()];
                if let Some(callee) = site_data.callee.value() {
                    self.borrow(site, &st, &cr, callee);
                }
                let args = site_data
                    .arguments
                    .get(&unit.pools.call_arguments)
                    .unwrap_or(&[]);
                // Borrowed arguments are checked before any Take transfers,
                // and `mut` places before any write-back.
                for arg in args {
                    match (arg.passing, arg.value) {
                        (Passing::Borrow, ArgumentValue::Value(v)) => {
                            self.borrow(site, &st, &cr, v)
                        }
                        (_, ArgumentValue::Address { address, .. }) => {
                            self.still_placed(site, &st, address)
                        }
                        _ => {}
                    }
                }
                if let Some(rejected) = site_data.pretransfer_failure {
                    outs.push(self.failure(site, b, rejected, st.clone(), cr.clone()));
                }
                for arg in args {
                    match (arg.passing, arg.value) {
                        (Passing::Take, ArgumentValue::Value(v)) => {
                            self.consume(site, &st, &mut cr, v)
                        }
                        (Passing::Mut, ArgumentValue::Address { address, version }) => {
                            if let Some((Root::Slot(slot), path)) = self.place(address) {
                                self.require(site, &st, slot, &path, false);
                                self.holder_loans(site, &st, &cr, slot);
                                self.borrowed_storage(site, slot);
                                self.invalidate(&mut st, slot, &path);
                                Rc::make_mut(&mut st.flags).retain(|(s, p), _| {
                                    *s != slot || p.is_empty() || !p.starts_with(&path)
                                });
                                // The write-back version reaches both edges.
                                let state = &mut st.slots[slot.index()];
                                state.moved.retain(|p, _| !overlap(p, &path));
                                state.moved.extend(
                                    state
                                        .taken
                                        .iter()
                                        .filter(|(p, _)| overlap(p, &path))
                                        .map(|(p, &t)| (p.clone(), t)),
                                );
                                state.owned_empty = false;
                                state.versions = Versions::One(version);
                            }
                        }
                        _ => {}
                    }
                }
                // Call entry has transferred Take actuals; callee failure
                // excludes them from caller cleanup.
                outs.push(self.failure(site, b, failure, st.clone(), cr.clone()));
                // A diverging call ends this frame's normal path here.
                let Some(normal) = normal else {
                    return outs;
                };
                let mut ncr = cr;
                if let Some(result) = site_data.result {
                    self.create(site, &mut st, &mut ncr, result);
                }
                let (s, c) = self.leave(site, b, normal, st, ncr);
                outs.push((normal, s, c));
                return outs;
            }
            Terminator::Return(value) => {
                if let Some(v) = value {
                    if !self.facts.regions[v.index()].is_empty() {
                        self.err(site, K::RegionEscape(v));
                    }
                    self.consume(site, &st, &mut cr, v);
                    let allowed = self.cx.layouts.0.fn_abis[unit.header.abi.0 as usize]
                        .result
                        .as_ref()
                        .map(|r| r.borrowed_parameters.as_slice())
                        .unwrap_or(&[]);
                    for &loan in &self.facts.borrowed_holders[v.index()] {
                        let root = self.facts.addresses
                            [unit.tables.loans[loan.index()].address.index()]
                        .as_ref()
                        .map(|a| a.root);
                        let parameter = self.facts.parameters.iter().enumerate().find_map(
                            |(i, &(passing, slot))| {
                                (passing == Passing::Borrow && slot.map(Root::Slot) == root)
                                    .then_some(i as u32)
                            },
                        );
                        if !parameter.is_some_and(|p| allowed.contains(&p)) {
                            self.err(site, K::BorrowedReturn(v));
                        } else {
                            st.loans[loan.index()] = Tri::No;
                        }
                    }
                }
                self.exit(site, &st, &cr);
            }
            Terminator::Propagate => self.exit(site, &st, &cr),
            Terminator::RejectTransfer => {
                // The protocol audit establishes the adapter role. This replay
                // independently requires every pending formal to be untouched.
                for (ordinal, &(passing, slot)) in self.facts.parameters.iter().enumerate() {
                    if passing != Passing::Take {
                        continue;
                    }
                    if let Some(slot) = slot {
                        let s = &st.slots[slot.index()];
                        if content(s) != Tri::Yes
                            || !s.taken.is_empty()
                            || !s.moved.is_empty()
                            || !s.finalize.is_empty()
                            || s.versions != Versions::One(unit.parameters[ordinal])
                        {
                            self.err(site, K::LeakedStorage(slot));
                        }
                        st.slots[slot.index()].init = Tri::No;
                    }
                }
                self.exit(site, &st, &cr);
            }
            Terminator::Raise(failure) => {
                outs.push(self.failure(site, b, failure, st, cr));
                return outs;
            }
        }
        // A branch on a slot's current flag decides its content per edge.
        let refined = match *term {
            Terminator::Branch { condition, yes, no } if yes != no => st
                .flags
                .iter()
                .find(|&(_, &v)| v == condition)
                .map(|(part, _)| (part.clone(), yes)),
            _ => None,
        };
        for edge in terminator_edges(self.cx, term) {
            let mut est = st.clone();
            self.switch_parts(term, edge, &mut est);
            if let Some(((slot, ref path), yes)) = refined {
                let s = &mut est.slots[slot.index()];
                if edge == yes {
                    s.live = Tri::Yes;
                    s.init = Tri::Yes;
                    s.owned_empty = false;
                    if let Some(Some(phases)) = st.flag_phases.get(&(slot, path.clone())) {
                        s.finalize.retain(|p, _| !p.starts_with(path));
                        s.finalize.extend(
                            phases
                                .iter()
                                .filter(|(p, _)| p.starts_with(path))
                                .map(|(p, &phase)| (p.clone(), phase)),
                        );
                    }
                    if !path.is_empty() {
                        s.moved.retain(|p, _| !path.starts_with(p));
                        s.taken.retain(|p, _| !path.starts_with(p));
                        // A true payload flag proves its enclosing variants
                        // active, even before an address is prepared.
                        for (n, step) in path.iter().enumerate() {
                            if matches!(step, Step::Variant(_, _)) {
                                s.moved.insert(path[..=n].to_vec(), Tri::No);
                            }
                        }
                    }
                } else if path.is_empty() {
                    s.init = Tri::No;
                } else {
                    s.taken.insert(path.clone(), Tri::Yes);
                    s.moved.insert(path.clone(), Tri::Yes);
                }
                self.remember_empty(&mut est, slot);
            }
            let (s, c) = self.leave(site, b, edge, est, cr.clone());
            outs.push((edge, s, c));
        }
        outs
    }

    fn switch_parts(&self, term: &Terminator, edge: EdgeId, state: &mut State) {
        let Terminator::Switch {
            value,
            arms,
            otherwise,
        } = term
        else {
            return;
        };
        let structure::Def::Inst(block, index) = self.facts.defs[value.index()] else {
            return;
        };
        let unit = self.cx.unit;
        let Some(Inst::Read {
            address,
            reaching: version,
            mode: crate::ir::ReadMode::Borrow,
            ..
        }) = unit.blocks[block.index()]
            .instructions
            .get(&unit.instructions)
            .and_then(|insts| insts.get(index as usize))
        else {
            return;
        };
        let Some((Root::Slot(slot), path)) = self.place(*address) else {
            return;
        };
        if state.slots[slot.index()].versions != Versions::One(*version) {
            return;
        }
        let layout = self.cx.value_layout(*value);
        if self.inline_layout(slot, &path) != Some(layout) {
            return;
        }
        let Some(record) = self.cx.layouts.get(layout) else {
            return;
        };
        let Some(tag) = &record.tag else {
            return;
        };
        let arms = arms.get(&unit.pools.switch_arms).unwrap_or(&[]);
        for variant in &tag.variants {
            let selected = arms
                .iter()
                .find(|a| a.value == variant.ordinal)
                .map_or(*otherwise, |a| a.edge);
            if selected != edge {
                for field in &variant.fields {
                    let mut part = path.clone();
                    part.push(Step::Variant(variant.ordinal, field.ordinal));
                    state.slots[slot.index()].moved.insert(part, Tri::Yes);
                }
            } else {
                for field in &variant.fields {
                    let mut part = path.clone();
                    part.push(Step::Variant(variant.ordinal, field.ordinal));
                    let actual = state.slots[slot.index()]
                        .taken
                        .get(&part)
                        .copied()
                        .unwrap_or(Tri::No);
                    state.slots[slot.index()].moved.insert(part, actual);
                }
            }
        }
        self.remember_empty(state, slot);
    }

    /// After an instruction: a flag whose slot's liveness or initialization
    /// changed is no longer current, except that a whole `Replace` of a live
    /// slot hands the slot's flag to the displaced slot, which now holds
    /// content exactly when the slot did.
    fn carry_flags(
        &self,
        inst: &Inst,
        before: &[((SlotId, Vec<Step>), ValueId, Tri)],
        st: &mut State,
    ) {
        if matches!(inst, Inst::Address { .. }) {
            return;
        }
        let mut flags = (*st.flags).clone();
        for (part, &value, &state) in before.iter().map(|(p, v, s)| (p, v, s)) {
            let (slot, path) = part;
            if self.flag_content(st, *slot, path) != state {
                flags.remove(part);
            }
            if let Inst::Replace {
                address, displaced, ..
            } = *inst
                && let Some((Root::Slot(target), written)) = self.place(address)
                && target == *slot
                && written.is_empty()
            {
                flags.insert((displaced, path.clone()), value);
            }
        }
        let mut phases = (*st.flag_phases).clone();
        if let Inst::FinalizeBegin { address, .. } | Inst::FinalizeEnd { address, .. } = *inst
            && let Some((Root::Slot(slot), path)) = self.place(address)
        {
            for ((target, part), summary) in &mut phases {
                if *target == slot
                    && path.starts_with(part)
                    && let Some(summary) = summary
                {
                    summary.insert(path.clone(), st.slots[slot.index()].phase(&path));
                }
            }
        }
        if let Inst::Replace {
            address, displaced, ..
        } = *inst
            && let Some((Root::Slot(target), written)) = self.place(address)
            && written.is_empty()
        {
            for (part, _, _) in before {
                if part.0 == target
                    && let Some(summary) = phases.get(part).cloned()
                {
                    phases.insert((displaced, part.1.clone()), summary);
                }
            }
        }
        phases.retain(|part, _| flags.contains_key(part));
        if phases != *st.flag_phases {
            st.flag_phases = Rc::new(phases);
        }
        if flags != *st.flags {
            st.flags = Rc::new(flags);
        }
    }

    /// A bool constant's value, if `value` is one.
    fn constant_bool(&self, value: ValueId) -> Option<bool> {
        let unit = self.cx.unit;
        let structure::Def::Inst(b, i) = self.facts.defs[value.index()] else {
            return None;
        };
        let insts = unit.blocks[b.index()]
            .instructions
            .get(&unit.instructions)
            .unwrap_or(&[]);
        match insts.get(i as usize) {
            Some(&Inst::Const { constant, .. }) => {
                match unit.tables.constants[constant.index()].value {
                    crate::ir::ConstantValue::Bool(x) => Some(x),
                    _ => None,
                }
            }
            _ => None,
        }
    }

    /// A failure successor: dispose the listed tokens, require that no token
    /// is left, then take the cleanup edge.
    fn failure(
        &mut self,
        site: Site,
        b: BlockId,
        failure: FailureId,
        st: State,
        mut cr: Credits,
    ) -> (EdgeId, State, Credits) {
        let unit = self.cx.unit;
        let f = &unit.tables.failures[failure.index()];
        for &t in f.dispose.get(&unit.pools.values).unwrap_or(&[]) {
            if self.cx.repr(t) != ValueRepr::Token {
                self.err(site, K::TokenUse(t));
            }
            self.consume(site, &st, &mut cr, t);
        }
        for (&v, &n) in &cr {
            if n > 0 && self.cx.repr(v) == ValueRepr::Token {
                self.err(site, K::TokenFailure(v));
            }
        }
        let (s, c) = self.leave(site, b, f.cleanup, st, cr);
        (f.cleanup, s, c)
    }

    /// Leave along `edge`: version joins, stale and order bookkeeping, credit
    /// transfer into parameters, scope exits and token feedback.
    fn leave(
        &mut self,
        site: Site,
        b: BlockId,
        edge: EdgeId,
        mut st: State,
        mut cr: Credits,
    ) -> (State, Credits) {
        let unit = self.cx.unit;
        let e = &unit.tables.edges[edge.index()];
        let target = e.target;
        if self.facts.cfg.preds[target.index()].len() > 1 {
            for (slot, _) in st.defusing.iter() {
                self.err(site, K::FinalizeState(*slot));
            }
        }
        let args = e.arguments.get(&unit.pools.values).unwrap_or(&[]);
        let params = unit.blocks[target.index()]
            .parameters
            .get(&unit.pools.values)
            .unwrap_or(&[]);
        for &(j, slot) in &self.facts.phis[target.index()] {
            let arg = args[j as usize];
            // Where the slot holds nothing the argument names no content and
            // nothing reads it; the structure check still requires a version
            // of this slot (LIR schema revision 8).
            if st.slots[slot.index()].versions != Versions::One(arg)
                && content(&st.slots[slot.index()]) != Tri::No
            {
                self.err(site, K::VersionJoin { slot, value: arg });
            }
            st.slots[slot.index()].versions = Versions::One(params[j as usize]);
        }
        let kinds = (
            unit.blocks[b.index()].kind,
            unit.blocks[target.index()].kind,
        );
        // A statement cleanup can leave a newer temporary retained by live
        // content. If that cleanup fails, frame unwinding starts a distinct
        // release sequence. Its remaining owners need not follow the already
        // completed statement drop's ordinal. Failure-chain edges still keep
        // their order history, including failed drops within that chain.
        let starts_unwind = kinds == (BlockKind::Cleanup, BlockKind::FailureCleanup);
        if (kinds.0 == BlockKind::Body || kinds.1 == BlockKind::Body || starts_unwind)
            && !st.order.is_empty()
        {
            st.order = Rc::default();
        }
        let mut incoming = Credits::new();
        // Snapshot before resetting target parameters: a back edge may forward
        // a parameter to itself, or permute several parameters.
        let invalid: Vec<bool> = args
            .iter()
            .map(|&a| self.validity(&st, &cr, a).is_some())
            .collect();
        for (index, (&a, &p)) in args.iter().zip(params).enumerate() {
            if st.stale.contains(&p) {
                Rc::make_mut(&mut st.stale).remove(&p);
            }
            // A reaching-version phi transports storage identity, not a read
            // of its content. Expired views may remain stored for silent
            // destruction; actual reads and calls still validate their loans.
            if self.facts.phis[target.index()]
                .iter()
                .any(|(j, _)| *j as usize == index)
            {
                continue;
            }
            // Non-owning transport is not a read. Preserve invalidity so a
            // later read, call or return cannot revive an expired view.
            if !self.cx.owns(p) {
                if invalid[index] {
                    Rc::make_mut(&mut st.stale).insert(p);
                }
                continue;
            }
            match self.holding(p) {
                Holding::Owned => {
                    if cr.get(&a).copied().unwrap_or(0) == 0 && self.holding(a) != Holding::Owned {
                        self.err(site, K::BorrowIntoOwned(a));
                    } else {
                        self.consume(site, &st, &mut cr, a);
                    }
                    incoming.insert(p, 1);
                }
                Holding::Borrowed(loan) => {
                    if self.holding(a) == Holding::Owned {
                        self.err(site, K::OwnedIntoBorrowed(a));
                    } else {
                        // A joined view takes views under any of its loans.
                        let joined = self.facts.joins[p.index()]
                            .as_ref()
                            .is_some_and(|j| j.joined());
                        if !joined && self.holding(a) != Holding::Borrowed(loan) {
                            self.err(site, K::Holding(p));
                        }
                        if invalid[index] {
                            Rc::make_mut(&mut st.stale).insert(p);
                        }
                    }
                }
                Holding::Plain => {
                    if self.holding(a) == Holding::Owned {
                        self.err(site, K::OwnedIntoBorrowed(a));
                    }
                }
            }
        }
        cr.retain(|&v, n| {
            if *n == 0 {
                return false;
            }
            if structure::in_scope_at(self.cx, self.facts, v, target) {
                return true;
            }
            if let Some(report) = self.report.as_deref_mut() {
                report.add(site, K::Leak(v));
            }
            false
        });
        cr.extend(incoming);
        // Each drop flag of the target: a constant matching the slot's
        // content on this edge, or the slot's current flag.
        for (j, slot, path) in &self.facts.flags[target.index()] {
            let (j, slot) = (*j, *slot);
            let part = (slot, path.clone());
            let arg = args[j as usize];
            let ok = st.flags.get(&part) == Some(&arg)
                || match (self.constant_bool(arg), self.flag_content(&st, slot, path)) {
                    (Some(true), Tri::Yes) | (Some(false), Tri::No) => true,
                    _ => false,
                };
            if !ok {
                self.err(site, K::InitFlag { slot, value: arg });
            }
            let summary = if st.flags.get(&part) == Some(&arg) {
                st.flag_phases.get(&part).cloned().unwrap_or(None)
            } else if self.constant_bool(arg) == Some(true) {
                Some(st.slots[slot.index()].finalize.clone())
            } else {
                None
            };
            Rc::make_mut(&mut st.flag_phases).insert(part.clone(), summary);
            Rc::make_mut(&mut st.flags).insert(part, params[j as usize]);
        }
        if self.facts.cfg.feedback.contains(&edge)
            && cr
                .iter()
                .any(|(&v, &n)| n > 0 && self.cx.repr(v) == ValueRepr::Token)
        {
            self.err(site, K::TokenFeedback(edge));
        }
        (st, cr)
    }

    /// At a return or propagation: no credit, owned storage, open loan or
    /// entered region remains, and every caller `mut` parameter is whole.
    fn exit(&mut self, site: Site, st: &State, cr: &Credits) {
        let unit = self.cx.unit;
        for (&v, &n) in cr {
            if n > 0 {
                self.err(site, K::Leak(v));
            }
        }
        // Dead storage owns nothing, so only a caller's `mut` parameter is
        // checked whatever its state; every other slot is checked where it
        // holds something.
        let passing = |id: SlotId| {
            self.facts
                .parameters
                .iter()
                .find(|&&(_, p)| p == Some(id))
                .map(|&(passing, _)| passing)
        };
        let written: BTreeSet<SlotId> = self
            .facts
            .parameters
            .iter()
            .filter_map(|&(_, p)| p)
            .filter(|&id| passing(id) == Some(Passing::Mut))
            .collect();
        for id in written {
            let state = &st.slots[id.index()];
            // A part is missing when it was transferred out or its resource
            // was finalized; an inactive variant's fields are absent from a
            // whole value, not moved out of it.
            let missing = state.taken.values().any(|t| t.may())
                || state
                    .moved
                    .iter()
                    .any(|(path, t)| t.may() && state.phase(path) != LIVE);
            if state.init != Tri::Yes || missing {
                self.err(site, K::MutExit(id));
            }
        }
        for (s, state) in st.slots.lacking(EMPTY) {
            let id = SlotId(s as u32);
            let slot = &unit.slots[s];
            match (passing(id), slot.kind) {
                (Some(Passing::Mut | Passing::Borrow), _)
                | (_, SlotKind::Capture(_) | SlotKind::ReturnSlot) => {}
                _ => {
                    if self.cx.owns_type(slot.ty) && !self.empty(id, state) {
                        self.err(site, K::LeakedStorage(id));
                    }
                }
            }
        }
        for (l, _) in st.loans.lacking(DEAD) {
            self.err(site, K::LoanOpenAtExit(LoanId(l as u32)));
        }
        for (r, _) in st.regions.lacking(DEAD) {
            self.err(site, K::RegionState(crate::ir::RegionInstanceId(r as u32)));
        }
    }

    // --- values ---

    /// Why a use of `value` here is invalid, if it is. A holder is valid; a
    /// borrowed view needs its loan definitely active, an unwritten backing
    /// and a valid base; a region value needs its region entered.
    fn validity(&self, st: &State, cr: &Credits, value: ValueId) -> Option<K> {
        if self.facts.borrowed_sources[value.index()]
            .iter()
            .any(|&source| source != value && self.view_validity(st, cr, source).is_some())
        {
            return Some(K::OutsideLoan(value));
        }
        self.view_validity(st, cr, value)
    }

    fn inherited_loan(&self, value: ValueId, loan: LoanId) -> bool {
        matches!(self.facts.backing[value.index()], Backing::Value(base)
            if self.holding(base) == Holding::Borrowed(loan))
    }

    fn view_validity(&self, st: &State, cr: &Credits, value: ValueId) -> Option<K> {
        // A projected loop value can feed its own phi. Visit every distinct
        // backing once, retaining checks on all real roots of that cycle.
        let mut pending = vec![value];
        let mut seen = BTreeSet::new();
        while let Some(current) = pending.pop() {
            if !seen.insert(current) {
                continue;
            }
            if !self.cx.owns(current) {
                if st.stale.contains(&current) {
                    return Some(K::StaleView(value));
                }
                if self.facts.regions[current.index()]
                    .iter()
                    .any(|r| st.regions[r.index()] != Tri::Yes)
                {
                    return Some(if current == value {
                        K::UseAfterRegionExit(value)
                    } else {
                        K::StaleView(value)
                    });
                }
                continue;
            }
            if cr.get(&current).copied().unwrap_or(0) > 0 {
                continue;
            }
            match self.holding(current) {
                Holding::Owned => {
                    return Some(if current == value {
                        K::NoCredit(value)
                    } else {
                        K::StaleView(value)
                    });
                }
                Holding::Borrowed(loan) => {
                    let joined = self.facts.joins[current.index()]
                        .as_ref()
                        .is_some_and(|j| j.joined());
                    let inherited = self.inherited_loan(current, loan);
                    if !joined && !inherited && st.loans[loan.index()] != Tri::Yes {
                        return Some(if current == value {
                            K::OutsideLoan(value)
                        } else {
                            K::StaleView(value)
                        });
                    }
                }
                Holding::Plain => {}
            }
            if st.stale.contains(&current) {
                return Some(K::StaleView(value));
            }
            pending.extend(self.bases.get(&current).into_iter().flatten().copied());
            if let Backing::Value(base) = self.facts.backing[current.index()] {
                pending.push(base);
            }
        }
        None
    }

    fn borrow(&mut self, site: Site, st: &State, cr: &Credits, value: ValueId) {
        if let Some(kind) = self.validity(st, cr, value) {
            self.err(site, kind);
        }
    }

    /// Consume one credit. The last credit of a holder under an active loan
    /// cannot be given up before that loan's `EndLoan`.
    fn consume(&mut self, site: Site, st: &State, cr: &mut Credits, value: ValueId) {
        if !self.cx.owns(value) {
            self.borrow(site, st, cr, value);
            return;
        }
        if moved_out(st, value) {
            self.err(site, K::MovedValuePart(value));
        }
        let held = cr.get(&value).copied().unwrap_or(0);
        if held == 0 && self.immortal(value) {
            return;
        }
        if held == 0 {
            self.err(site, K::NoCredit(value));
            return;
        }
        if held == 1 {
            for loan in self.loans_on(st, Root::Value(value), &[]) {
                self.err(site, K::HolderUnderLoan(loan));
            }
        }
        if held == 1 {
            cr.remove(&value);
        } else {
            cr.insert(value, held - 1);
        }
    }

    /// A Plain static (a literal slice over program-lifetime storage): its
    /// backing block carries the immortal count, so consuming it needs no
    /// credit and the storage receiving it releases nothing.
    fn immortal(&self, value: ValueId) -> bool {
        self.facts.statics[value.index()] && self.holding(value) == Holding::Plain
    }

    /// A release or reset of a value with fields moved out must say exactly
    /// which stay: a static live mask clearing the moved fields and no other
    /// (LIR schema revision 5). The value's parts end with it.
    fn release_mask(
        &mut self,
        site: Site,
        st: &mut State,
        value: ValueId,
        layout: crate::layout_types::LayoutId,
        live: crate::ir::MaskRef,
    ) {
        if !st.parts.contains_key(&value) {
            return;
        }
        let Some(parts) = Rc::make_mut(&mut st.parts).remove(&value) else {
            return;
        };
        if !parts.values().any(|t| t.may()) {
            return;
        }
        let mut moved = BTreeSet::new();
        let mut exact = true;
        for (path, t) in &parts {
            match (
                path.strip_prefix(&[Step::Payload])
                    .unwrap_or(path.as_slice()),
                t,
            ) {
                (_, Tri::No) => {}
                ([Step::Field(f) | Step::Variant(_, f)], Tri::Yes) => {
                    moved.insert(*f);
                }
                _ => exact = false,
            }
        }
        let crate::ir::MaskRef::Static(mask) = live else {
            self.err(site, K::ReleaseMask(value));
            return;
        };
        let unit = self.cx.unit;
        let mask = &unit.tables.masks[mask.index()];
        let words = mask.words.get(&unit.pools.mask_words).unwrap_or(&[]);
        let bit = |i: u32| {
            words
                .get(i as usize / 64)
                .is_some_and(|w| w >> (i % 64) & 1 == 1)
        };
        let record = self.cx.layouts.get(layout);
        let mut fields: BTreeSet<u32> = record
            .iter()
            .flat_map(|r| r.fields.iter().map(|f| f.ordinal))
            .collect();
        fields.extend(
            record
                .iter()
                .flat_map(|r| r.tag.iter())
                .flat_map(|t| &t.variants)
                .flat_map(|v| v.fields.iter().map(|f| f.ordinal)),
        );
        let cleared: BTreeSet<u32> = (0..mask.fields).filter(|&i| !bit(i)).collect();
        let cleared: BTreeSet<u32> = cleared.intersection(&fields).copied().collect();
        if !exact || cleared != moved {
            self.err(site, K::ReleaseMask(value));
        }
    }

    /// The owned operands a failing operation has already disposed of.
    fn taken(&self, inst: &Inst) -> Vec<ValueId> {
        let unit = self.cx.unit;
        let mut out = Vec::new();
        match *inst {
            Inst::Construct { construct, .. } => {
                out.extend(structure::consumed_values(self.cx, construct))
            }
            Inst::Reuse {
                token, construct, ..
            } => {
                out.push(token);
                out.extend(structure::consumed_values(self.cx, construct));
            }
            Inst::Closure { closure, .. } => out.extend(
                unit.tables.closures[closure.index()]
                    .captures
                    .get(&unit.pools.captures)
                    .unwrap_or(&[])
                    .iter()
                    .filter(|c| c.usage == CaptureUse::Take)
                    .map(|c| c.value),
            ),
            Inst::Replace { value, .. } => out.push(value),
            _ => {}
        }
        out.retain(|&v| self.cx.owns(v));
        out
    }

    fn create(&mut self, site: Site, st: &mut State, cr: &mut Credits, value: ValueId) {
        if st.stale.contains(&value) {
            Rc::make_mut(&mut st.stale).remove(&value);
        }
        if self.cx.owns(value)
            && self.holding(value) == Holding::Owned
            && cr.insert(value, 1).is_some()
        {
            self.err(site, K::Leak(value));
        }
    }

    // --- storage ---

    fn place(&self, address: crate::ir::AddressId) -> Option<(Root, Vec<Step>)> {
        let a = self.facts.addresses[address.index()].as_ref()?;
        Some((a.root, a.path.clone()))
    }

    /// Loans that may be active on a place overlapping `path` of `root`.
    fn loans_on(&self, st: &State, root: Root, path: &[Step]) -> Vec<LoanId> {
        let unit = self.cx.unit;
        st.loans
            .lacking(DEAD)
            .into_iter()
            .filter_map(|(l, _)| {
                let info = &unit.tables.loans[l];
                let a = self.facts.addresses[info.address.index()].as_ref()?;
                (a.root == root && overlap(&a.path, path)).then_some(LoanId(l as u32))
            })
            .collect()
    }

    fn holder_released(&mut self, site: Site, st: &State, slot: SlotId, path: &[Step]) {
        for loan in self.loans_on(st, Root::Slot(slot), path) {
            self.err(site, K::HolderUnderLoan(loan));
        }
    }

    fn borrowed_storage(&mut self, site: Site, slot: SlotId) {
        if self
            .facts
            .parameters
            .iter()
            .any(|&(passing, p)| passing == Passing::Borrow && p == Some(slot))
        {
            self.err(site, K::BorrowedStorage(slot));
        }
    }

    /// The place must hold initialized content: live storage, initialized,
    /// and no overlapping part moved out. `maybe` admits a conditional use.
    fn require(&mut self, site: Site, st: &State, slot: SlotId, path: &[Step], maybe: bool) {
        let s = &st.slots[slot.index()];
        if !(s.live == Tri::Yes || maybe && s.live == Tri::Maybe) {
            self.err(site, K::DeadStorage(slot));
            return;
        }
        if !(s.init == Tri::Yes || maybe && s.init == Tri::Maybe) {
            self.err(site, K::Uninitialized(slot));
            return;
        }
        if s.taken
            .iter()
            .any(|(q, t)| t.may() && overlap(q, path) && !(maybe && *t == Tri::Maybe))
        {
            self.err(site, K::MovedPart(slot));
        }
    }

    /// A record may hold uncounted views even though it owns no credits.
    /// Reads and Mut calls use those holders; silent destruction does not.
    fn holder_loans(&mut self, site: Site, st: &State, cr: &Credits, slot: SlotId) {
        if let Versions::One(version) = st.slots[slot.index()].versions {
            if self.facts.borrowed_sources[version.index()]
                .iter()
                .any(|&source| self.view_validity(st, cr, source).is_some())
            {
                self.err(site, K::OutsideLoan(version));
            }
        }
    }

    /// Views of `slot` that a write to `path` may detach or overwrite go
    /// stale, as do the prepared places it may move.
    fn invalidate(&self, st: &mut State, slot: SlotId, path: &[Step]) {
        self.unplace(st, slot, path);
        for &(view, backed) in self.views.get(&slot).into_iter().flatten() {
            if let Backing::Slot {
                path: viewed,
                crossings,
                ..
            } = &self.facts.backing[backed.index()]
                && invalidates(path, viewed, crossings)
                && !st.stale.contains(&view)
            {
                Rc::make_mut(&mut st.stale).insert(view);
            }
        }
    }

    /// A sequence write rewrites the block at `address`: its count, its
    /// capacity and, when it grows or detaches, the block itself. Views of its
    /// element slots go stale, and so does every place located through the
    /// block (its dereference and bounds checks anchor at the place).
    fn write_block(&mut self, site: Site, st: &mut State, address: AddressId, version: ValueId) {
        let Some((Root::Slot(slot), path)) = self.place(address) else {
            return;
        };
        self.borrowed_storage(site, slot);
        self.require(site, st, slot, &path, false);
        self.unplace(st, slot, &path);
        let mut inside = path;
        inside.push(Step::Slot);
        self.invalidate(st, slot, &inside);
        st.slots[slot.index()].owned_empty = false;
        st.slots[slot.index()].versions = Versions::One(version);
    }

    /// A write to `path` of `slot` at or above an anchor of a prepared address
    /// (a counted block dereference, a tag or a bounds check) may move its
    /// place: a released block, another variant's bytes, a shorter sequence.
    fn unplace(&self, st: &mut State, slot: SlotId, path: &[Step]) {
        for (address, anchors) in self.placed.get(&slot).into_iter().flatten() {
            let Some(addr) = &self.facts.addresses[address.index()] else {
                continue;
            };
            if anchors
                .iter()
                .any(|&k| path.len() <= k && overlap(path, &addr.path[..k]))
                && !st.unplaced.contains(address)
            {
                Rc::make_mut(&mut st.unplaced).insert(*address);
            }
        }
    }

    /// A use of an address its preparation located, before any write since
    /// may have moved its place.
    fn still_placed(&mut self, site: Site, st: &State, address: AddressId) {
        if st.unplaced.contains(&address) {
            let slot = match self.facts.addresses[address.index()]
                .as_ref()
                .map(|a| a.root)
            {
                Some(Root::Slot(slot)) => slot,
                _ => return,
            };
            self.err(site, K::StaleAddress { address, slot });
        }
    }

    fn version(&mut self, site: Site, st: &State, slot: SlotId, reaching: ValueId) {
        if st.slots[slot.index()].versions != Versions::One(reaching) {
            self.err(site, K::StaleVersion { slot, reaching });
        }
    }

    /// A read of a place: storage checks for a slot, validity for a block.
    fn read(
        &mut self,
        site: Site,
        st: &State,
        cr: &Credits,
        address: crate::ir::AddressId,
        transfer_box: bool,
    ) -> Option<(Root, Vec<Step>)> {
        let (root, path) = self.place(address)?;
        match root {
            Root::Slot(slot) => {
                if transfer_box
                    && path.is_empty()
                    && matches!(
                        self.cx
                            .layouts
                            .get(
                                self.cx.unit.types[self.cx.unit.slots[slot.index()].ty.index()]
                                    .layout
                            )
                            .map(|r| &r.kind),
                        Some(crate::layout_types::LayoutKind::Box { .. })
                    )
                {
                    // Taking the pointer transfers its partially moved payload; release must carry the exact live mask.
                    let mut pointer = st.clone();
                    pointer.slots[slot.index()]
                        .taken
                        .retain(|part, _| !part.starts_with(&[Step::Payload]));
                    self.require(site, &pointer, slot, &path, false);
                } else {
                    self.require(site, st, slot, &path, false);
                }
                self.holder_loans(site, st, cr, slot);
            }
            Root::Value(base) => {
                self.borrow(site, st, cr, base);
                if st
                    .parts
                    .get(&base)
                    .is_some_and(|m| m.iter().any(|(q, t)| t.may() && overlap(q, &path)))
                {
                    self.err(site, K::MovedValuePart(base));
                }
            }
        }
        Some((root, path))
    }

    /// The layout of the inline place `path` of `slot`, or None past a
    /// variant, an element or a block (another storage's content).
    fn inline_layout(&self, slot: SlotId, path: &[Step]) -> Option<crate::layout_types::LayoutId> {
        let unit = self.cx.unit;
        let mut layout = unit.types[unit.slots[slot.index()].ty.index()].layout;
        for step in path {
            let record = self.cx.layouts.get(layout)?;
            if self.cx.layouts.payload(layout).is_some() {
                return None;
            }
            let fields = match *step {
                Step::Field(_) => &record.fields,
                Step::Variant(v, _) => {
                    &record
                        .tag
                        .as_ref()?
                        .variants
                        .iter()
                        .find(|x| x.ordinal == v)?
                        .fields
                }
                Step::Inline(index) => {
                    let crate::layout_types::LayoutKind::Inline { element, count } = record.kind
                    else {
                        return None;
                    };
                    if index >= count {
                        return None;
                    }
                    layout = element;
                    continue;
                }
                Step::Payload | Step::Index | Step::Slot => return None,
            };
            let (Step::Field(f) | Step::Variant(_, f)) = *step else {
                return None;
            };
            layout = fields.iter().find(|x| x.ordinal == f)?.layout;
        }
        Some(layout)
    }

    /// Whether `slot` holds nothing owned: its content is uninitialized or
    /// moved out whole, or on every path where it is initialized every
    /// owning part of it, through inline fields and the active variant's
    /// fields, is moved out and each resource among them has finished
    /// `FinalizeEnd`.
    fn empty(&self, slot: SlotId, s: &SlotState) -> bool {
        if s.init == Tri::No || s.owned_empty || s.moved.get(&Vec::new()) == Some(&Tri::Yes) {
            return true;
        }
        self.covered(slot, s, &mut Vec::new())
    }

    /// A transfer or finalization removes owners monotonically until a write.
    /// Save the complete proof before a join can lose its path correlation.
    fn flag_content(&self, st: &State, slot: SlotId, path: &[Step]) -> Tri {
        let s = &st.slots[slot.index()];
        let trivial = self
            .inline_layout(slot, path)
            .is_some_and(|l| self.cx.layouts.trivial(l));
        if s.owned_empty && (path.is_empty() || !trivial) {
            return Tri::No;
        }
        let presence = part_content(s, path);
        if presence != Tri::No
            && !path.is_empty()
            && self
                .inline_layout(slot, path)
                .is_some_and(|l| !self.cx.layouts.trivial(l))
            && self.covered(slot, s, &mut path.to_vec())
        {
            Tri::No
        } else {
            presence
        }
    }

    fn remember_empty(&self, st: &mut State, slot: SlotId) {
        let completed: Vec<_> = st
            .defusing
            .iter()
            .filter(|(s, path)| {
                *s == slot
                    && (st.slots[slot.index()].init == Tri::No
                        || self.parts_absent(
                            slot,
                            &st.slots[slot.index()],
                            &mut path.clone(),
                            true,
                        ))
            })
            .cloned()
            .collect();
        if !completed.is_empty() {
            let regions = Rc::make_mut(&mut st.defusing);
            for place in completed {
                regions.remove(&place);
            }
        }
        let s = &st.slots[slot.index()];
        if self
            .inline_layout(slot, &[])
            .is_some_and(|l| self.cx.layouts.trivial(l))
        {
            return;
        }
        if s.init != Tri::No && !s.owned_empty && self.empty(slot, s) {
            st.slots[slot.index()].owned_empty = true;
        }
    }

    fn covered(&self, slot: SlotId, s: &SlotState, path: &mut Vec<Step>) -> bool {
        self.parts_absent(slot, s, path, false)
    }

    /// Defuse distribution empties Plain fields too. Ordinary cleanup only
    /// needs owning parts absent; both proofs use the same physical walk.
    fn parts_absent(
        &self,
        slot: SlotId,
        s: &SlotState,
        path: &mut Vec<Step>,
        all_fields: bool,
    ) -> bool {
        if s.moved.get(path.as_slice()) == Some(&Tri::Yes) {
            return true;
        }
        let Some(layout) = self.inline_layout(slot, path) else {
            return false;
        };
        let layouts = self.cx.layouts;
        if !all_fields && layouts.trivial(layout) {
            return true;
        }
        let Some(record) = layouts.get(layout) else {
            return false;
        };
        if record.finalizer.is_some() && s.phase(path) != PENDING {
            return false;
        }
        if let crate::layout_types::LayoutKind::Inline { count, .. } = record.kind {
            return (0..count).all(|index| {
                path.push(Step::Inline(index));
                let covered = self.parts_absent(slot, s, path, all_fields);
                path.pop();
                covered
            });
        }
        if record.tag.is_some() {
            return record.tag.as_ref().is_some_and(|tag| {
                tag.variants.iter().all(|variant| {
                    variant.fields.iter().all(|field| {
                        path.push(Step::Variant(variant.ordinal, field.ordinal));
                        let covered = self.parts_absent(slot, s, path, all_fields);
                        path.pop();
                        covered
                    })
                })
            });
        }

        if !matches!(
            record.kind,
            crate::layout_types::LayoutKind::Aggregate | crate::layout_types::LayoutKind::Resource
        ) {
            return false;
        }
        record.fields.iter().all(|f| {
            path.push(Step::Field(f.ordinal));
            let covered = self.parts_absent(slot, s, path, all_fields);
            path.pop();
            covered
        })
    }

    /// A take of `path` out of `slot`: no enclosing resource with a
    /// finalizer may still be Live or Finalizing.
    fn unfinalized_owner(&mut self, site: Site, st: &State, slot: SlotId, path: &[Step]) {
        // Removing a sequence element mutates inside its counted storage;
        // it does not move an inline field of the enclosing resource.
        if path.last() == Some(&Step::Slot)
            && (0..path.len()).any(|n| {
                self.inline_layout(slot, &path[..n])
                    .and_then(|layout| self.cx.layouts.get(layout))
                    .is_some_and(|r| {
                        matches!(
                            r.kind,
                            crate::layout_types::LayoutKind::Block { .. }
                                | crate::layout_types::LayoutKind::Box { .. }
                        )
                    })
            })
        {
            return;
        }
        let s = &st.slots[slot.index()];
        for n in 0..path.len() {
            let prefix = &path[..n];
            let finalized = self
                .inline_layout(slot, prefix)
                .and_then(|l| self.cx.layouts.get(l))
                .is_some_and(|r| r.finalizer.is_some());
            if finalized && s.phase(prefix) != PENDING {
                self.err(site, K::FinalizeState(slot));
            }
        }
    }

    fn observe(&mut self, site: Site, b: BlockId, st: &mut State, slot: SlotId) {
        if self.cx.unit.blocks[b.index()].kind == BlockKind::Body {
            return;
        }
        let info = &self.cx.unit.slots[slot.index()];
        let next = (info.scope, info.kind.is_temporary(), slot.0);
        if st
            .order
            .iter()
            .any(|&previous| !releases_after(&self.cx.unit.scopes, previous, next))
        {
            self.err(site, K::CleanupOrder(slot));
        }
        st.order = Rc::new(BTreeSet::from([next]));
    }

    /// A region placement needs its region entered; an arena placement reads
    /// the token in its instance's slot, which must hold one here.
    fn region_placement(&mut self, site: Site, st: &State, placement: Option<Placement>) {
        match placement {
            Some(Placement::Fixed(StorageOwner::LocalRegion(r)))
                if st.regions[r.index()] != Tri::Yes =>
            {
                self.err(site, K::RegionState(r));
            }
            Some(Placement::Fixed(StorageOwner::Arena(arena))) => {
                let Some(instance) = self.cx.unit.tables.arenas.get(arena.index()) else {
                    return;
                };
                let slot = instance.slot;
                let state = &st.slots[slot.index()];
                if state.live != Tri::Yes {
                    self.err(site, K::DeadStorage(slot));
                } else if state.init != Tri::Yes {
                    self.err(site, K::Uninitialized(slot));
                }
            }
            _ => {}
        }
    }

    /// RC-ROOT (S-14): content that reaches a `Shared` root
    /// ends at its cleanup anchor. A TIR cleanup action runs in a `Cleanup`
    /// or `FailureCleanup` block, so a drop or release under a TIR anchor
    /// elsewhere ended its holder early (at a last use, say). An instruction's
    /// own drop and a loop release sit where TIR states them; a disposal or
    /// state release cleans up after a failure only. A displaced slot's drop is not
    /// checked: its content left its place when displaced. A library body has no TIR cleanup
    /// chain: its drops sit where its operation's contract places them (a spawned task's
    /// callable after its body returns, say).
    fn root_anchor(&mut self, site: Site, b: BlockId, anchor: CleanupAnchorId, layout: LayoutId) {
        if self.cx.unit.header.origin == UnitOrigin::Library
            || !self.cx.layouts.root_bearing(layout)
        {
            return;
        }
        if !self.at_cleanup_anchor(b, anchor) {
            self.err(site, K::RootOutsideAnchor(anchor));
        }
    }

    /// Cleanup lowering preserves the block kind and anchor while splitting
    /// drops, so the lifetime witness remains available at Ready too.
    fn at_cleanup_anchor(&self, b: BlockId, anchor: CleanupAnchorId) -> bool {
        let Some(cleanup) = self.cx.unit.tables.cleanup_anchors.get(anchor.index()) else {
            return false;
        };
        let kind = self.cx.unit.blocks[b.index()].kind;
        match cleanup.cleanup {
            INSTRUCTION_DROP | LOOP_RELEASE => true,
            SCRATCH_DISPOSAL | STATE_RELEASE => kind == BlockKind::FailureCleanup,
            _ => kind != BlockKind::Body,
        }
    }

    fn anchor_release(&mut self, site: Site, b: BlockId, value: ValueId, anchor: CleanupAnchorId) {
        self.discharged.insert(anchor);
        // A block destroys its payload; an inline value its own content.
        let own = self.cx.value_layout(value);
        let destroyed = if self.cx.repr(value) == ValueRepr::Callable {
            // Erasure hides the environment type, not its actual SSA origin.
            // Reconstruct the closure's descriptor with C's token provenance
            // check; the Reset's claimed layout grants no authority here.
            tokens::environment(self.cx, self.facts, value, &mut BTreeSet::new()).unwrap_or(own)
        } else {
            self.cx.layouts.released(own).unwrap_or(own)
        };
        if self.cx.layouts.destructible(destroyed)
            && (self.cx.stage < StageKind::Ready
                || self.cx.unit.header.origin != UnitOrigin::Library
                    && !self.at_cleanup_anchor(b, anchor))
        {
            self.err(site, K::ResourceOutsideAnchor(value));
        }
    }

    fn inst(&mut self, site: Site, b: BlockId, inst: &Inst, st: &mut State, cr: &mut Credits) {
        if self.report.is_some() {
            if let Inst::Address { address, .. } | Inst::FlatNext { address, .. } = *inst {
                let version = match self.facts.addresses[address.index()]
                    .as_ref()
                    .map(|a| a.root)
                {
                    Some(Root::Slot(slot)) => match st.slots[slot.index()].versions {
                        Versions::One(v) => Some(v),
                        _ => None,
                    },
                    _ => None,
                };
                self.flat_reaching.insert(site, version);
            }
            if let Inst::FlatField { address, .. } | Inst::FlatClose { address, .. } = *inst {
                let version = match self.facts.addresses[address.index()]
                    .as_ref()
                    .map(|a| a.root)
                {
                    Some(Root::Slot(slot)) => match st.slots[slot.index()].versions {
                        Versions::One(v) => Some(v),
                        _ => None,
                    },
                    _ => None,
                };
                self.open_reaching.insert(site, version);
            }
        }

        // A preparation locates its place afresh from the root, running its
        // base's checks again, so it uses no earlier location.
        if let Inst::Address { address, .. } = *inst {
            if st.unplaced.contains(&address) {
                Rc::make_mut(&mut st.unplaced).remove(&address);
            }
        } else {
            for address in structure::inst_addresses(self.cx, inst) {
                self.still_placed(site, st, address);
            }
        }
        let unit = self.cx.unit;
        match *inst {
            Inst::Const { result, .. }
            | Inst::FunctionRef { result, .. }
            | Inst::ReadStatic { result, .. }
            | Inst::TokenNone { result } => self.create(site, st, cr, result),
            Inst::Unary {
                result, operand, ..
            } => {
                self.borrow(site, st, cr, operand);
                self.create(site, st, cr, result);
            }
            Inst::Binary {
                result,
                left,
                right,
                ..
            } => {
                self.borrow(site, st, cr, left);
                self.borrow(site, st, cr, right);
                self.create(site, st, cr, result);
            }
            Inst::Convert {
                result,
                flag,
                operand,
                ..
            } => {
                self.borrow(site, st, cr, operand);
                self.create(site, st, cr, result);
                if let Some(flag) = flag {
                    self.create(site, st, cr, flag);
                }
            }
            Inst::IntArith {
                result,
                flag,
                left,
                right,
                ..
            } => {
                self.borrow(site, st, cr, left);
                self.borrow(site, st, cr, right);
                self.create(site, st, cr, result);
                if let Some(flag) = flag {
                    self.create(site, st, cr, flag);
                }
            }
            Inst::MulAdd {
                result,
                left,
                right,
                addend,
            } => {
                for v in [left, right, addend] {
                    self.borrow(site, st, cr, v);
                }
                self.create(site, st, cr, result);
            }
            Inst::ScalarUnary {
                result, operand, ..
            } => {
                self.borrow(site, st, cr, operand);
                self.create(site, st, cr, result);
            }
            // Structural comparison borrows both operands for the instruction only.
            Inst::Compare {
                result,
                left,
                right,
                ..
            } => {
                self.borrow(site, st, cr, left);
                self.borrow(site, st, cr, right);
                self.create(site, st, cr, result);
            }
            Inst::IsUnique { result, value }
            | Inst::IsWritable { result, value }
            | Inst::SeqLen { result, value }
            | Inst::SeqCapacity { result, value }
            | Inst::Hash { result, value, .. }
            | Inst::Encode { result, value, .. }
            | Inst::Decode { result, value, .. } => {
                self.borrow(site, st, cr, value);
                self.create(site, st, cr, result);
            }
            Inst::SealArena {
                result,
                grant,
                root,
                ..
            } => {
                self.borrow(site, st, cr, grant);
                self.consume(site, st, cr, root);
                self.create(site, st, cr, result);
            }
            Inst::BorrowSealedRoot {
                result,
                owner,
                loan,
                ..
            } => {
                if st.loans[loan.index()] != Tri::Yes {
                    self.err(site, K::OutsideLoan(result));
                }
                self.borrow(site, st, cr, owner);
                self.create(site, st, cr, result);
            }
            Inst::PayloadReceipt {
                result,
                value,
                inputs,
                ..
            } => {
                self.borrow(site, st, cr, value);
                for input in inputs
                    .get(&self.cx.unit.pools.receipt_inputs)
                    .unwrap_or(&[])
                {
                    if let Some(receipt) = input.receipt {
                        self.borrow(site, st, cr, receipt);
                    }
                }
                self.create(site, st, cr, result);
            }
            Inst::ArenaInventory {
                arena,
                value,
                previous,
                ..
            } => {
                self.borrow(site, st, cr, arena);
                self.borrow(site, st, cr, value);
                if let Some(previous) = previous {
                    self.borrow(site, st, cr, previous);
                }
            }
            // Both move the whole owned value into a new representation.
            Inst::Freeze { result, value } | Inst::Thaw { result, value } => {
                self.consume(site, st, cr, value);
                self.create(site, st, cr, result);
            }
            // The same view at another payload layout: it happens inside
            // the loan it carries, as a borrowed read does.
            Inst::CopyValue { result, value } => {
                self.borrow(site, st, cr, value);
                self.create(site, st, cr, result);
            }
            Inst::Recast { result, value } => {
                if self.holding(result) == Holding::Owned {
                    self.consume(site, st, cr, value);
                } else {
                    self.borrow(site, st, cr, value);
                }
                if let Holding::Borrowed(loan) = self.holding(result)
                    && st.loans[loan.index()] != Tri::Yes
                {
                    self.err(site, K::OutsideLoan(result));
                }
                self.create(site, st, cr, result);
            }
            // A write through the block at the place (LIR schema revision
            // 7): the place holds initialized content, every view through
            // the block goes stale, and the root has a new version. Which
            // element slots hold a value is the heap checker's to prove.
            Inst::SeqReserve {
                address,
                capacity: amount,
                version,
            }
            | Inst::SeqSetLen {
                address,
                len: amount,
                version,
            }
            | Inst::SeqAppend {
                address,
                value: amount,
                version,
            } => {
                // An appended element moves into the block (LIR schema revision 19).
                if matches!(inst, Inst::SeqAppend { .. }) {
                    self.consume(site, st, cr, amount);
                } else {
                    self.borrow(site, st, cr, amount);
                }
                self.write_block(site, st, address, version);
            }
            // A push's preparation may copy or relocate the block at the
            // place (LIR schema revision 32): a write, as a reserve's. Its
            // count is read (revision 34).
            Inst::ArenaPush {
                address,
                count,
                version,
                ..
            } => {
                if let Some(count) = count {
                    self.borrow(site, st, cr, count);
                }
                self.write_block(site, st, address, version)
            }
            // A semantic copy's mark reads the value, which keeps its credits.
            Inst::Share { value } => {
                // RC-ROOT (S-14): a semantic copy of root-bearing content is a
                // Dup; one made by a move cancelled that Dup against its
                // holder's release, ending the holder away from its anchor.
                if self.facts.moved[value.index()]
                    && self.cx.layouts.root_bearing(self.cx.value_layout(value))
                {
                    self.err(site, K::RootCopyCancelled(value));
                }
                self.borrow(site, st, cr, value)
            }
            // A write of a builder's open element reads its operand (LIR
            // schema revision 24).
            Inst::FlatField {
                address,
                op,
                version,
                ..
            } => {
                match op {
                    FlatFieldOp::Open => {}
                    FlatFieldOp::Reserve { capacity: value } | FlatFieldOp::Push { value, .. } => {
                        self.borrow(site, st, cr, value)
                    }
                }
                let Some((Root::Slot(slot), path)) = self.place(address) else {
                    return;
                };
                self.borrowed_storage(site, slot);
                self.require(site, st, slot, &path, false);
                let mut inside = path;
                inside.push(Step::Slot);
                self.invalidate(st, slot, &inside);
                st.slots[slot.index()].owned_empty = false;
                st.slots[slot.index()].versions = Versions::One(version);
            }
            // An element built in place reads the values it encodes and
            // consumes the others (LIR schema revision 22); an open element
            // is completed so (revision 24).
            Inst::FlatAppend {
                address,
                construct,
                version,
            }
            | Inst::FlatClose {
                address,
                construct,
                version,
                ..
            } => {
                for v in structure::encoded_values(self.cx, construct) {
                    self.borrow(site, st, cr, v);
                }
                for v in structure::consumed_values(self.cx, construct) {
                    self.consume(site, st, cr, v);
                }
                self.write_block(site, st, address, version);
            }
            Inst::Weaken { result, value } => {
                // A borrowed view weakens to a view; only an owned operand is consumed.
                if self.holding(value) == Holding::Owned {
                    self.consume(site, st, cr, value);
                } else {
                    self.borrow(site, st, cr, value);
                }
                self.create(site, st, cr, result);
            }
            Inst::Check { condition, .. } => self.borrow(site, st, cr, condition),
            Inst::UpdateMask { result, mask, .. } => {
                self.borrow(site, st, cr, mask);
                self.create(site, st, cr, result);
            }
            Inst::StorageLive { slot } => {
                if self.cx.owns_type(unit.slots[slot.index()].ty)
                    && !self.empty(slot, &st.slots[slot.index()])
                {
                    self.err(site, K::LeakedStorage(slot));
                }
                // A fresh incarnation cannot be ordered against its own earlier release.
                Rc::make_mut(&mut st.order).retain(|&(_, _, previous)| previous != slot.0);
                st.slots[slot.index()] = SlotState {
                    live: Tri::Yes,
                    ..SlotState::DEAD
                };
                Rc::make_mut(&mut st.defusing).retain(|(s, _)| *s != slot);
            }
            Inst::StorageDead { slot, if_live } => {
                let s = &st.slots[slot.index()];
                if !(s.live == Tri::Yes || if_live) {
                    self.err(site, K::DeadStorage(slot));
                }
                if self.cx.owns_type(unit.slots[slot.index()].ty) && !self.empty(slot, s) {
                    self.err(site, K::LeakedStorage(slot));
                }
                if s.finalize.values().any(|p| p & FINALIZING != 0) {
                    self.err(site, K::FinalizeState(slot));
                }
                self.holder_released(site, st, slot, &[]);
                self.invalidate(st, slot, &[]);
                st.slots[slot.index()] = SlotState::DEAD;
                Rc::make_mut(&mut st.defusing).retain(|(s, _)| *s != slot);
            }
            Inst::EndScope { .. }
            | Inst::BindOrigin { .. }
            | Inst::BindFields { .. }
            | Inst::EndOrigin { .. }
            | Inst::FrameEnter { .. }
            | Inst::Unavailable { .. } => {}
            Inst::FrameLeave => {
                if let Some(&address) = self
                    .cleanup_retirements
                    .get(&(b, site.index.unwrap_or(u32::MAX)))
                {
                    if let Some((Root::Slot(slot), mut path)) = self.place(address) {
                        if !self.covered(slot, &st.slots[slot.index()], &mut path) {
                            self.err(
                                site,
                                K::Schema("finalizer retires before owned descendants".into()),
                            );
                        }
                    }
                }
            }
            Inst::Address { address, .. } => {
                let info = &unit.addresses[address.index()];
                if let crate::ir::AddressBase::Value(v) = info.base {
                    self.borrow(site, st, cr, v);
                }
                if let Some((Root::Slot(slot), path)) = self.place(address) {
                    for (n, step) in path.iter().enumerate() {
                        if let Step::Variant(selected, _) = *step
                            && let Some(layout) = self.inline_layout(slot, &path[..n])
                            && let Some(tag) =
                                self.cx.layouts.get(layout).and_then(|r| r.tag.as_ref())
                        {
                            for variant in &tag.variants {
                                if variant.ordinal != selected {
                                    for field in &variant.fields {
                                        let mut absent = path[..n].to_vec();
                                        absent.push(Step::Variant(variant.ordinal, field.ordinal));
                                        st.slots[slot.index()].moved.insert(absent, Tri::Yes);
                                    }
                                } else {
                                    for field in &variant.fields {
                                        let mut present = path[..n].to_vec();
                                        present.push(Step::Variant(selected, field.ordinal));
                                        let actual = st.slots[slot.index()]
                                            .taken
                                            .get(&present)
                                            .copied()
                                            .unwrap_or(Tri::No);
                                        st.slots[slot.index()].moved.insert(present, actual);
                                    }
                                }
                            }
                        }
                    }
                    self.remember_empty(st, slot);
                }
                for step in info.projection.get(&unit.pools.projections).unwrap_or(&[]) {
                    use crate::ir::ProjectionStep as P;
                    match *step {
                        P::Payload | P::Field(_) | P::VariantField { .. } => {}
                        P::Index(v) | P::Slot(v) => self.borrow(site, st, cr, v),
                        P::FlatAt { index, offset } => {
                            self.borrow(site, st, cr, index);
                            self.borrow(site, st, cr, offset);
                        }
                    }
                }
            }
            Inst::Read {
                result,
                address,
                reaching,
                mode,
            } => {
                let Some((root, path)) =
                    self.read(site, st, cr, address, mode == crate::ir::ReadMode::Take)
                else {
                    return;
                };
                // A borrowed read happens inside its loan.
                if mode == crate::ir::ReadMode::Borrow
                    && let Holding::Borrowed(loan) = self.holding(result)
                    && !self.inherited_loan(result, loan)
                    && st.loans[loan.index()] != Tri::Yes
                {
                    self.err(site, K::OutsideLoan(result));
                }
                if let Root::Value(base) = root
                    && mode == crate::ir::ReadMode::Take
                {
                    Rc::make_mut(&mut st.parts)
                        .entry(base)
                        .or_default()
                        .insert(path.clone(), Tri::Yes);
                }
                if let Root::Slot(slot) = root {
                    if st
                        .defusing
                        .iter()
                        .any(|(s, p)| *s == slot && p.starts_with(&path))
                        || st.slots[slot.index()].phase(&path) == PENDING
                            && self
                                .inline_layout(slot, &path)
                                .and_then(|l| self.cx.layouts.get(l))
                                .is_some_and(|layout| layout.finalizer.is_some())
                    {
                        self.err(site, K::FinalizeState(slot));
                    }
                    self.version(site, st, slot, reaching);
                    if mode == crate::ir::ReadMode::Take {
                        self.borrowed_storage(site, slot);
                        self.holder_released(site, st, slot, &path);
                        self.unfinalized_owner(site, st, slot, &path);
                        self.invalidate(st, slot, &path);
                        let s = &mut st.slots[slot.index()];
                        s.renew(&path);
                        if path.is_empty() {
                            if !s.taken.is_empty()
                                && matches!(
                                    self.cx
                                        .layouts
                                        .get(
                                            unit.types[unit.values[result.index()].ty.index()]
                                                .layout
                                        )
                                        .map(|r| &r.kind),
                                    Some(crate::layout_types::LayoutKind::Box { .. })
                                )
                            {
                                Rc::make_mut(&mut st.parts).insert(result, s.taken.clone());
                            }
                            s.init = Tri::No;
                            s.moved.clear();
                            s.taken.clear();
                            s.versions = Versions::None;
                        } else if !through_slot(&path) {
                            s.moved
                                .retain(|q, _| !(q.len() > path.len() && overlap(q, &path)));
                            s.taken.insert(path.clone(), Tri::Yes);
                            s.moved.insert(path, Tri::Yes);
                        }
                        if s.init == Tri::No {
                            s.owned_empty = false;
                        }
                        self.remember_empty(st, slot);
                    }
                }
                self.create(site, st, cr, result);
            }
            Inst::Copy {
                result,
                address,
                reaching,
                ..
            } => {
                if let Some((Root::Slot(slot), _)) = self.read(site, st, cr, address, false) {
                    self.version(site, st, slot, reaching);
                }
                self.create(site, st, cr, result);
            }
            Inst::FlatCopy {
                result, address, ..
            } => {
                self.read(site, st, cr, address, false);
                self.create(site, st, cr, result);
            }
            // A view of a flat variable part happens inside its loan, as a
            // borrowed read does; a flat length is a plain read.
            Inst::SliceView {
                result,
                value,
                start,
                end,
                ..
            } => {
                for v in [value, start, end] {
                    self.borrow(site, st, cr, v);
                }
                self.create(site, st, cr, result);
            }
            Inst::FlatView { result, address } => {
                self.read(site, st, cr, address, false);
                if let Holding::Borrowed(loan) = self.holding(result)
                    && st.loans[loan.index()] != Tri::Yes
                {
                    self.err(site, K::OutsideLoan(result));
                }
                self.create(site, st, cr, result);
            }
            Inst::FlatLength { result, address } | Inst::FlatNext { result, address } => {
                self.read(site, st, cr, address, false);
                self.create(site, st, cr, result);
            }
            Inst::UniqueCopy {
                result,
                value,
                construct,
                ..
            } => {
                self.borrow(site, st, cr, value);
                self.region_placement(
                    site,
                    st,
                    unit.tables.constructs[construct.index()].placement,
                );
                self.create(site, st, cr, result);
            }
            Inst::Store {
                address,
                value,
                version,
            } => {
                self.consume(site, st, cr, value);
                let Some((Root::Slot(slot), path)) = self.place(address) else {
                    return;
                };
                self.borrowed_storage(site, slot);
                let s = &st.slots[slot.index()];
                if s.live != Tri::Yes {
                    self.err(site, K::DeadStorage(slot));
                } else if path.is_empty() {
                    if s.init.may()
                        && !s.moved.get(&path).is_some_and(|t| *t == Tri::Yes)
                        && !self.facts.capture_mask_stores.contains(&site)
                    {
                        self.err(site, K::StoreToLive(slot));
                    }
                } else if s.init != Tri::Yes
                    || s.taken
                        .iter()
                        .any(|(q, t)| t.may() && q.len() < path.len() && overlap(q, &path))
                {
                    self.err(site, K::Uninitialized(slot));
                } else if !through_slot(&path) && s.taken.get(&path) != Some(&Tri::Yes) {
                    // An element slot named without a count check may be
                    // empty (LIR schema revision 7); every other place must
                    // have been moved out.
                    self.err(site, K::StoreToLive(slot));
                }
                self.invalidate(st, slot, &path);
                let s = &mut st.slots[slot.index()];
                s.owned_empty = false;
                s.renew(&path);
                if path.is_empty() {
                    s.init = Tri::Yes;
                    s.moved.clear();
                    s.taken.clear();
                } else if !through_slot(&path) {
                    s.moved.retain(|q, _| !overlap(q, &path));
                }
                s.taken
                    .retain(|q, _| !(q.len() >= path.len() && overlap(q, &path)));
                s.versions = Versions::One(version);
            }
            Inst::Replace {
                address,
                value,
                displaced,
                version,
                displaced_version,
                if_live,
                ..
            } => {
                self.consume(site, st, cr, value);
                let Some((Root::Slot(slot), path)) = self.place(address) else {
                    return;
                };
                self.borrowed_storage(site, slot);
                self.require(site, st, slot, &path, if_live);
                let before = st.slots[slot.index()].init;
                let displaced_phases: BTreeMap<_, _> = st.slots[slot.index()]
                    .finalize
                    .iter()
                    .filter_map(|(q, p)| q.strip_prefix(path.as_slice()).map(|r| (r.to_vec(), *p)))
                    .collect();
                let d = &st.slots[displaced.index()];
                if d.live != Tri::Yes {
                    self.err(site, K::DeadStorage(displaced));
                } else if d.init.may() {
                    self.err(site, K::StoreToLive(displaced));
                }
                self.invalidate(st, slot, &path);
                // The displaced slot's new content moves its prepared places.
                self.unplace(st, displaced, &[]);
                let s = &mut st.slots[slot.index()];
                s.owned_empty = false;
                s.renew(&path);
                if path.is_empty() {
                    s.init = Tri::Yes;
                    s.moved.clear();
                    s.taken.clear();
                } else {
                    s.moved.retain(|q, _| !overlap(q, &path));
                }
                s.taken
                    .retain(|q, _| !(q.len() >= path.len() && overlap(q, &path)));
                s.versions = Versions::One(version);
                let d = &mut st.slots[displaced.index()];
                d.init = if path.is_empty() { before } else { Tri::Yes };
                d.moved.clear();
                d.taken.clear();
                d.versions = Versions::One(displaced_version);
                d.finalize = displaced_phases;
                d.owned_empty = false;
            }
            Inst::Construct {
                result, construct, ..
            } => {
                // A flat construction reads the values it encodes.
                for v in structure::encoded_values(self.cx, construct) {
                    self.borrow(site, st, cr, v);
                }
                for v in structure::consumed_values(self.cx, construct) {
                    self.consume(site, st, cr, v);
                }
                self.region_placement(
                    site,
                    st,
                    unit.tables.constructs[construct.index()].placement,
                );
                self.create(site, st, cr, result);
            }
            Inst::Reuse {
                result,
                token,
                construct,
                ..
            } => {
                self.consume(site, st, cr, token);
                // A flat construction reads the values it encodes.
                for v in structure::encoded_values(self.cx, construct) {
                    self.borrow(site, st, cr, v);
                }
                for v in structure::consumed_values(self.cx, construct) {
                    self.consume(site, st, cr, v);
                }
                self.region_placement(
                    site,
                    st,
                    unit.tables.constructs[construct.index()].placement,
                );
                self.create(site, st, cr, result);
            }
            Inst::Closure {
                result, closure, ..
            } => {
                let c = &unit.tables.closures[closure.index()];
                for capture in c.captures.get(&unit.pools.captures).unwrap_or(&[]) {
                    match capture.usage {
                        CaptureUse::Take => self.consume(site, st, cr, capture.value),
                        CaptureUse::Copy | CaptureUse::Borrow | CaptureUse::Mut => {
                            self.borrow(site, st, cr, capture.value)
                        }
                    }
                }
                if let Some(receipts) = &c.receipts {
                    for capture in &receipts.captures {
                        self.borrow(site, st, cr, capture.receipt);
                    }
                }
                self.region_placement(site, st, c.placement);
                self.create(site, st, cr, result);
            }
            Inst::BeginLoan {
                loan,
                address,
                mutable,
                if_inactive,
            } => {
                if !if_inactive && st.loans[loan.index()] != Tri::No {
                    self.err(site, K::LoanState(loan));
                }
                st.loans[loan.index()] = Tri::Yes;
                if mutable && let Some((Root::Slot(slot), path)) = self.place(address) {
                    let state = &mut st.slots[slot.index()];
                    state.moved.retain(|p, _| !overlap(p, &path));
                    state.moved.extend(
                        state
                            .taken
                            .iter()
                            .filter(|(p, _)| overlap(p, &path))
                            .map(|(p, &t)| (p.clone(), t)),
                    );
                    state.owned_empty = false;
                }
            }
            Inst::EndLoan { loan, if_active } => {
                if !if_active && st.loans[loan.index()] != Tri::Yes {
                    self.err(site, K::LoanState(loan));
                }
                st.loans[loan.index()] = Tri::No;
                if let Some(joined) = self.joined.get(&loan) {
                    Rc::make_mut(&mut st.stale).extend(joined.iter().copied());
                }
            }
            Inst::Drop {
                address,
                if_live,
                anchor,
                ..
            } => {
                self.discharged.insert(anchor);
                let dropped = unit.types[unit.addresses[address.index()].ty.index()].layout;
                // A displaced slot's content left its place in the step that displaced it
                // (an overwrite, or a unique write's copy that now holds what it held).
                let displaced = matches!(
                    self.place(address),
                    Some((Root::Slot(slot), path))
                        if path.is_empty() && unit.slots[slot.index()].kind == SlotKind::Displaced
                );
                if !displaced {
                    self.root_anchor(site, b, anchor, dropped);
                }
                let Some((Root::Slot(slot), path)) = self.place(address) else {
                    return;
                };
                self.borrowed_storage(site, slot);
                if path.is_empty() {
                    let s = &st.slots[slot.index()];
                    if !(s.live == Tri::Yes || if_live && s.live == Tri::Maybe) {
                        self.err(site, K::DeadStorage(slot));
                    } else if !(s.init == Tri::Yes || if_live && s.init.may()) {
                        self.err(site, K::Uninitialized(slot));
                    }
                    if st.slots[slot.index()].init.may() {
                        self.observe(site, b, st, slot);
                    }
                } else {
                    self.require(site, st, slot, &path, if_live);
                }
                self.holder_released(site, st, slot, &path);
                self.invalidate(st, slot, &path);
                let s = &mut st.slots[slot.index()];
                s.renew(&path);
                if path.is_empty() {
                    s.init = Tri::No;
                    s.moved.clear();
                    s.taken.clear();
                    s.versions = Versions::None;
                } else if !through_slot(&path) {
                    s.moved
                        .retain(|q, _| !(q.len() > path.len() && overlap(q, &path)));
                    s.taken.insert(path.clone(), Tri::Yes);
                    s.moved.insert(path, Tri::Yes);
                }
                if s.init == Tri::No {
                    s.owned_empty = false;
                }
                self.remember_empty(st, slot);
            }
            Inst::Defuse { address } => {
                let Some((Root::Slot(slot), path)) = self.read(site, st, cr, address, false) else {
                    self.err(site, K::Type("defuse requires owned slot storage"));
                    return;
                };
                self.require(site, st, slot, &path, false);
                self.borrowed_storage(site, slot);
                self.holder_released(site, st, slot, &path);
                let s = &mut st.slots[slot.index()];
                if s.phase(&path) != LIVE {
                    self.err(site, K::FinalizeState(slot));
                }
                s.finalize.insert(path.clone(), PENDING);
                Rc::make_mut(&mut st.defusing).insert((slot, path));
                self.remember_empty(st, slot);
            }
            Inst::FinalizeBegin { address, anchor } | Inst::FinalizeEnd { address, anchor } => {
                self.discharged.insert(anchor);
                let begin = matches!(inst, Inst::FinalizeBegin { .. });
                let Some((Root::Slot(slot), path)) = self.place(address) else {
                    return;
                };
                self.require(site, st, slot, &path, false);
                let (from, to) = if begin {
                    (LIVE, FINALIZING)
                } else {
                    (FINALIZING, PENDING)
                };
                // Finalizing a whole slot's resource begins its release, in
                // the cleanup order.
                if begin && path.is_empty() {
                    self.observe(site, b, st, slot);
                }
                let s = &mut st.slots[slot.index()];
                if s.phase(&path) != from {
                    self.err(site, K::FinalizeState(slot));
                }
                s.finalize.insert(path, to);
                if !begin {
                    let path = self.place(address).unwrap().1;
                    let state = &st.slots[slot.index()];
                    if self.covered(slot, state, &mut path.clone()) {
                        // Finalized bytes may remain readable, but this part
                        // has no owner. Join that same absence with a take.
                        st.slots[slot.index()].moved.insert(path, Tri::Yes);
                    }
                    self.remember_empty(st, slot);
                }
            }
            Inst::Dup { value } => {
                self.borrow(site, st, cr, value);
                if moved_out(st, value) {
                    self.err(site, K::MovedValuePart(value));
                }
                *cr.entry(value).or_insert(0) += 1;
            }
            Inst::Release {
                value,
                layout,
                live,
                anchor,
                ..
            } => {
                self.anchor_release(site, b, value, anchor);
                self.root_anchor(site, b, anchor, self.cx.value_layout(value));
                if let Some(slot) = self.facts.taken_from[value.index()] {
                    self.observe(site, b, st, slot);
                }
                self.release_mask(site, st, value, layout, live);
                self.consume(site, st, cr, value);
            }
            Inst::Reset {
                result,
                value,
                layout,
                live,
                anchor,
                ..
            } => {
                self.release_mask(site, st, value, layout, live);
                self.anchor_release(site, b, value, anchor);
                self.root_anchor(site, b, anchor, self.cx.value_layout(value));
                if let Some(slot) = self.facts.taken_from[value.index()] {
                    self.observe(site, b, st, slot);
                }
                if self.immortal(value) {
                    // Static storage is never a reuse donor.
                    self.err(site, K::NoCredit(value));
                }
                self.consume(site, st, cr, value);
                self.create(site, st, cr, result);
            }
            Inst::Dispose { token } => self.consume(site, st, cr, token),
            Inst::HoleOpen { result, hole, .. } => {
                self.create(site, st, cr, result);
                self.create(site, st, cr, hole);
            }
            Inst::HoleFill { hole, value } => {
                self.consume(site, st, cr, hole);
                self.consume(site, st, cr, value);
            }
            Inst::HoleAbandon { hole, root } => {
                self.consume(site, st, cr, hole);
                if let Some(root) = root {
                    self.consume(site, st, cr, root);
                }
            }
            Inst::RegionEnter { result, region } => {
                if st.regions[region.index()] != Tri::No {
                    self.err(site, K::RegionState(region));
                }
                st.regions[region.index()] = Tri::Yes;
                self.create(site, st, cr, result);
            }
            Inst::RegionReserve {
                result,
                mark,
                extent,
                ..
            } => {
                self.borrow(site, st, cr, mark);
                self.borrow(site, st, cr, extent);
                for &r in &self.facts.regions[result.index()] {
                    if st.regions[r.index()] != Tri::Yes {
                        self.err(site, K::RegionState(r));
                    }
                }
                self.create(site, st, cr, result);
            }
            Inst::RegionPublish { value } => self.borrow(site, st, cr, value),
            Inst::RegionExit { mark, region, .. } => {
                self.consume(site, st, cr, mark);
                if st.regions[region.index()] != Tri::Yes {
                    self.err(site, K::RegionState(region));
                }
                st.regions[region.index()] = Tri::No;
            }
            Inst::ArenaOpen { result, budget } => {
                if let Some(budget) = budget {
                    self.borrow(site, st, cr, budget);
                }
                self.create(site, st, cr, result);
            }
            Inst::ArenaToken {
                result,
                mark,
                budget,
            } => {
                self.borrow(site, st, cr, mark);
                if let Some(budget) = budget {
                    self.borrow(site, st, cr, budget);
                }
                self.create(site, st, cr, result);
            }
            Inst::Forget { value } => self.consume(site, st, cr, value),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `PVec::join` skips dead storage meeting a state that absorbs it: the
    /// flags must say exactly when `join_slot` would change nothing, which
    /// storage is dead and which holds nothing.
    #[test]
    fn a_slot_absorbs_dead_storage_exactly_when_the_join_changes_nothing() {
        let tris = [Tri::No, Tri::Yes, Tri::Maybe];
        let path = vec![Step::Field(0)];
        let mut seen = [0, 0];
        for live in tris {
            for init in tris {
                for moved in [None, Some(Tri::Yes), Some(Tri::Maybe)] {
                    for taken in [None, Some(Tri::Yes), Some(Tri::Maybe)] {
                        for version in [None, Some(ValueId(3))] {
                            for phase in [None, Some(PENDING)] {
                                for owned_empty in [false, true] {
                                    let state = SlotState {
                                        live,
                                        init,
                                        moved: moved
                                            .into_iter()
                                            .map(|t| (path.clone(), t))
                                            .collect(),
                                        taken: taken
                                            .into_iter()
                                            .map(|t| (path.clone(), t))
                                            .collect(),
                                        versions: version.into(),
                                        finalize: phase
                                            .into_iter()
                                            .map(|p| (path.clone(), p))
                                            .collect(),
                                        owned_empty,
                                    };
                                    let mut joined = state.clone();
                                    join_slot(&mut joined, &SlotState::DEAD);
                                    let absorbs =
                                        SlotState::absorbs(state.flags(), SlotState::DEAD.flags());
                                    assert_eq!(absorbs, joined == state, "{state:?}");
                                    assert_eq!(state.flags() & DEAD != 0, state == SlotState::DEAD);
                                    assert_eq!(state.flags() & EMPTY != 0, state.init == Tri::No);
                                    seen[usize::from(joined == state)] += 1;
                                    let mut same = state.clone();
                                    join_slot(&mut same, &state);
                                    assert_eq!(same, state, "a join is idempotent");
                                }
                            }
                        }
                    }
                }
            }
        }
        assert!(seen[0] > 0 && seen[1] > 0);
        for t in tris {
            assert_eq!(
                Tri::absorbs(t.flags(), Tri::No.flags()),
                t.join(Tri::No) == t
            );
            assert_eq!(t.flags() & DEAD != 0, t == Tri::No);
        }
    }
}

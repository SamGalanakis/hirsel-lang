//! Linear raw reuse tokens: provenance, donor eligibility,
//! compatibility of each reuse with its token, and where tokens may appear.
//!
//! A token's identity is `(usable bytes, alignment, task heap domain)` derived
//! from its donor's layout through the runtime size-class table; a `None`
//! token (from `TokenNone`) is compatible with every reuse and allocates. The
//! linear accounting itself (copied, disposed twice, leaked, missing from a
//! failure's disposal list, live across a feedback edge) is the credit replay
//! in `ownership`, which counts tokens like any other holder.
use super::structure::{Def, Facts, Root, Step, construct_values, propagate};
use super::{Cx, Report, Site, ViolationKind as K};
use crate::ir::{
    ArgumentValue, BlockId, CaptureValue, ConstructorKind, FlatFieldOp, Inst, MaskRef, Placement,
    ReadMode, StorageOwner, TaskDomainId, Terminator, ValueId, ValueRepr,
};
use crate::layout_types::LayoutId;
use crate::runtime_abi::{SizeClass, size_class};
use crate::stage::Stage;
use std::collections::BTreeSet;

/// What a token value may hold.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Tok {
    /// No provenance reaches it (yet).
    Bottom,
    /// Explicit None: the reuse allocates.
    None,
    Raw {
        capacity: u64,
        align: u32,
        domain: TaskDomainId,
    },
    /// Join arguments of different provenance.
    Conflict,
}

/// The heap domain of a counted block, where it is known.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Domain {
    Unset,
    Known(TaskDomainId),
    Unknown,
}

pub(super) fn check<S: Stage>(cx: &Cx<'_, S>, facts: &Facts, report: &mut Report) {
    let unit = cx.unit;
    let mut domains = vec![Domain::Unset; unit.values.len()];
    let executing = (unit.tables.task_domains > 0).then_some(TaskDomainId(0));
    for (v, info) in unit.values.iter().enumerate() {
        // A closure's environment is a counted block too (LIR schema revision 5).
        if matches!(
            unit.types[info.ty.index()].repr,
            ValueRepr::Block | ValueRepr::Callable
        ) {
            domains[v] = match executing {
                Some(d) => Domain::Known(d),
                None => Domain::Unknown,
            };
        }
    }
    let placed = |placement: Option<Placement>| match placement {
        Some(Placement::Fixed(StorageOwner::Heap(d))) => Domain::Known(d),
        _ => Domain::Unknown,
    };
    for block in &unit.blocks {
        for inst in block.instructions.get(&unit.instructions).unwrap_or(&[]) {
            match *inst {
                Inst::Construct {
                    result, construct, ..
                }
                | Inst::Reuse {
                    result, construct, ..
                }
                | Inst::UniqueCopy {
                    result, construct, ..
                } => {
                    domains[result.index()] =
                        placed(unit.tables.constructs[construct.index()].placement)
                }
                Inst::Closure {
                    result, closure, ..
                } => {
                    domains[result.index()] =
                        placed(unit.tables.closures[closure.index()].placement)
                }
                Inst::Const { result, .. }
                | Inst::ReadStatic { result, .. }
                | Inst::FunctionRef { result, .. } => domains[result.index()] = Domain::Unknown,
                _ => {}
            }
        }
    }
    // Block parameters hold a known domain only if every argument agrees.
    for (b, block) in unit.blocks.iter().enumerate() {
        let _ = b;
        for &p in block.parameters.get(&unit.pools.values).unwrap_or(&[]) {
            if domains[p.index()] != Domain::Unknown {
                domains[p.index()] = Domain::Unset;
            }
        }
    }
    propagate(cx, &facts.cfg, &mut domains, |param, arg| {
        match (*param, *arg) {
            (Domain::Unset, a) => a,
            (p, Domain::Unset) => p,
            (p, a) if p == a => p,
            _ => Domain::Unknown,
        }
    });

    let mut info = vec![Tok::Bottom; unit.values.len()];
    for (b, block) in unit.blocks.iter().enumerate() {
        let insts = block.instructions.get(&unit.instructions).unwrap_or(&[]);
        for (i, inst) in insts.iter().enumerate() {
            let site = Site::at(BlockId(b as u32), i as u32);
            match *inst {
                Inst::TokenNone { result } => info[result.index()] = Tok::None,
                Inst::Reset {
                    result,
                    value,
                    layout,
                    ..
                } => {
                    // Only owned counted heap blocks of a known domain donate:
                    // never static, region, arena, frozen or caller storage.
                    let domain = match domains[value.index()] {
                        Domain::Known(d) if !facts.statics[value.index()] => Some(d),
                        _ => None,
                    };
                    // A closure donates by the environment layout of the
                    // closure that built it, whatever its static (possibly
                    // erased) callable type says (LIR schema revision 5).
                    let domain = domain.filter(|_| {
                        cx.repr(value) != ValueRepr::Callable
                            || environment(cx, facts, value, &mut BTreeSet::new()) == Some(layout)
                    });
                    match (domain, capacity(cx, layout)) {
                        (Some(domain), Some((capacity, align))) => {
                            info[result.index()] = Tok::Raw {
                                capacity,
                                align,
                                domain,
                            }
                        }
                        _ => report.add(site, K::TokenDonor(value)),
                    }
                }
                _ => {}
            }
        }
    }
    propagate(cx, &facts.cfg, &mut info, |param, arg| {
        match (*param, *arg) {
            (Tok::Bottom, a) | (a, Tok::Bottom) => a,
            (Tok::None, a) | (a, Tok::None) => a,
            (p, a) if p == a => p,
            _ => Tok::Conflict,
        }
    });
    for (v, tok) in info.iter().enumerate() {
        if *tok == Tok::Conflict && cx.repr(ValueId(v as u32)) == ValueRepr::Token {
            report.add(Site::UNIT, K::TokenJoin(ValueId(v as u32)));
        }
    }

    for (b, block) in unit.blocks.iter().enumerate() {
        let b = BlockId(b as u32);
        let insts = block.instructions.get(&unit.instructions).unwrap_or(&[]);
        for (i, inst) in insts.iter().enumerate() {
            let site = Site::at(b, i as u32);
            if let Inst::Reuse {
                construct,
                token,
                kept: Some(kept),
                ..
            } = *inst
            {
                kept_fields(cx, facts, site, token, construct, kept, report);
            }
            if let Inst::Reuse {
                result,
                token,
                construct,
                ..
            } = *inst
                && let Tok::Raw {
                    capacity: held,
                    align,
                    domain,
                } = info[token.index()]
            {
                let c = &unit.tables.constructs[construct.index()];
                if c.placement != Some(Placement::Fixed(StorageOwner::Heap(domain))) {
                    report.add(site, K::TokenOwner(result));
                }
                match capacity(cx, c.layout) {
                    Some((requested, wanted)) => {
                        if wanted != align {
                            report.add(site, K::TokenAlignment(result));
                        }
                        if requested != held {
                            report.add(site, K::TokenCapacity(result));
                        }
                    }
                    None => report.add(site, K::TokenCapacity(result)),
                }
            }
            for v in token_operands(cx, inst) {
                if cx.repr(v) == ValueRepr::Token {
                    report.add(site, K::TokenUse(v));
                }
            }
        }
        let site = Site::at(b, insts.len() as u32);
        for v in terminator_values(cx, &unit.terminators[b.index()]) {
            if cx.repr(v) == ValueRepr::Token {
                report.add(site, K::TokenUse(v));
            }
        }
    }
}

/// Every kept field of a reuse (LIR schema revision 5) has physical-slot
/// evidence: wherever the token holds storage, it was reset from a donor of
/// the construction's own layout whose live mask cleared that field, and the
/// field's operand is the value taken out of that same field of that donor
/// (`read take` at `Value(donor).f`, or `.VariantField(v, f)` for the
/// construction's variant `v`, which the take's preparation proved active).
/// Through joins, every edge passing a reset token passes that take too; an
/// edge passing an explicit None proves nothing is skipped and needs nothing.
fn kept_fields<S>(
    cx: &Cx<'_, S>,
    facts: &Facts,
    site: Site,
    token: ValueId,
    construct: crate::ir::ConstructId,
    kept: crate::ir::MaskId,
    report: &mut Report,
) {
    let unit = cx.unit;
    let c = &unit.tables.constructs[construct.index()];
    let mask = &unit.tables.masks[kept.index()];
    let words = mask.words.get(&unit.pools.mask_words).unwrap_or(&[]);
    let fields = c.fields.get(&unit.pools.fields).unwrap_or(&[]);
    for f in 0..mask.fields {
        if !words
            .get(f as usize / 64)
            .is_some_and(|w| w >> (f % 64) & 1 == 1)
        {
            continue;
        }
        let step = match c.constructor {
            ConstructorKind::Record(_) | ConstructorKind::Tuple => Some(Step::Field(f)),
            ConstructorKind::Variant { variant, .. } => Some(Step::Variant(variant, f)),
            _ => None,
        };
        let operand = fields.iter().find(|x| x.field == f).map(|x| x.value);
        let proved = match (step, operand) {
            (Some(step), Some(value)) => evidence(
                cx,
                facts,
                token,
                value,
                c.layout,
                step,
                &mut BTreeSet::new(),
            ),
            _ => false,
        };
        if !proved {
            let value = operand.unwrap_or(token);
            report.add(site, K::KeptField { value, field: f });
        }
    }
}

fn evidence<S>(
    cx: &Cx<'_, S>,
    facts: &Facts,
    token: ValueId,
    value: ValueId,
    layout: LayoutId,
    step: Step,
    seen: &mut BTreeSet<(ValueId, ValueId)>,
) -> bool {
    let unit = cx.unit;
    if !seen.insert((token, value)) {
        return false;
    }
    let inst = |v: ValueId| match facts.defs.get(v.index()) {
        Some(&Def::Inst(b, i)) => unit.blocks[b.index()]
            .instructions
            .get(&unit.instructions)
            .and_then(|insts| insts.get(i as usize))
            .copied(),
        _ => None,
    };
    match (facts.defs.get(token.index()), facts.defs.get(value.index())) {
        (Some(&Def::Param(tb, ti)), Some(&Def::Param(vb, vi))) if tb == vb => {
            let preds = &facts.cfg.preds[tb.index()];
            !preds.is_empty()
                && preds.iter().all(|&(_, edge)| {
                    let args = unit.tables.edges[edge.index()]
                        .arguments
                        .get(&unit.pools.values)
                        .unwrap_or(&[]);
                    match (args.get(ti as usize), args.get(vi as usize)) {
                        (Some(&t), Some(&v)) => evidence(cx, facts, t, v, layout, step, seen),
                        _ => false,
                    }
                })
        }
        _ => match inst(token) {
            Some(Inst::TokenNone { .. }) => true,
            Some(Inst::Reset {
                value: donor,
                layout: reset,
                live: MaskRef::Static(live),
                ..
            }) if reset == layout => {
                let mask = &unit.tables.masks[live.index()];
                let words = mask.words.get(&unit.pools.mask_words).unwrap_or(&[]);
                let field = match step {
                    Step::Field(f) | Step::Variant(_, f) => f,
                    Step::Payload | Step::Index | Step::Inline(_) | Step::Slot => return false,
                };
                let cleared = !words
                    .get(field as usize / 64)
                    .is_some_and(|w| w >> (field % 64) & 1 == 1);
                let taken = match inst(value) {
                    Some(Inst::Read {
                        address,
                        mode: ReadMode::Take,
                        ..
                    }) => facts.addresses[address.index()].as_ref().is_some_and(|a| {
                        a.root == Root::Value(donor)
                            && a.path.strip_prefix(&[Step::Payload]).unwrap_or(&a.path) == [step]
                    }),
                    _ => false,
                };
                cleared && taken
            }
            _ => false,
        },
    }
}

/// The environment layout of the closure a callable value was built as:
/// through `Weaken`, whole-slot stores and takes (a take names the stored
/// version it reads) and block parameters whose arguments all agree. None
/// when any source is not a closure (a parameter, a call result, a function
/// reference).
pub(super) fn environment<S>(
    cx: &Cx<'_, S>,
    facts: &Facts,
    value: ValueId,
    seen: &mut BTreeSet<ValueId>,
) -> Option<LayoutId> {
    let unit = cx.unit;
    if !seen.insert(value) {
        return None;
    }
    match *facts.defs.get(value.index())? {
        Def::Inst(b, i) => {
            let inst = *unit.blocks[b.index()]
                .instructions
                .get(&unit.instructions)?
                .get(i as usize)?;
            match inst {
                Inst::Closure { closure, .. } => {
                    Some(unit.tables.closures[closure.index()].environment)
                }
                Inst::Weaken { value, .. } => environment(cx, facts, value, seen),
                Inst::Store {
                    value: stored,
                    version,
                    ..
                } if version == value => environment(cx, facts, stored, seen),
                Inst::Read {
                    address,
                    reaching,
                    mode: ReadMode::Take,
                    ..
                } => {
                    let whole = facts.addresses[address.index()]
                        .as_ref()
                        .is_some_and(|a| matches!(a.root, Root::Slot(_)) && a.path.is_empty());
                    if whole {
                        environment(cx, facts, reaching, seen)
                    } else {
                        None
                    }
                }
                _ => None,
            }
        }
        Def::Param(b, i) => {
            let mut found = None;
            for &(_, edge) in &facts.cfg.preds[b.index()] {
                let arg = *unit.tables.edges[edge.index()]
                    .arguments
                    .get(&unit.pools.values)?
                    .get(i as usize)?;
                let env = environment(cx, facts, arg, &mut seen.clone())?;
                if found.is_some_and(|f| f != env) {
                    return None;
                }
                found = Some(env);
            }
            found
        }
        _ => None,
    }
}

/// `(usable bytes, alignment)` of a fixed-size payload: the size class's
/// capacity, or the request itself for a large block.
fn capacity<S>(cx: &Cx<'_, S>, layout: LayoutId) -> Option<(u64, u32)> {
    let record = cx.layouts.get(layout)?;
    let size = cx.layouts.fixed_size(layout)?;
    let capacity = match size_class(size, u64::from(record.align)) {
        SizeClass::Small { capacity, .. } | SizeClass::Large { capacity } => capacity,
    };
    Some((capacity, record.align.max(8)))
}

/// Operands that may never be tokens: everything but a reuse's token and a
/// disposal (block arguments and failure disposal lists are accounted by
/// the credit replay).
fn token_operands<S>(cx: &Cx<'_, S>, inst: &Inst) -> Vec<ValueId> {
    let unit = cx.unit;
    match *inst {
        Inst::Reuse { construct, .. }
        | Inst::Construct { construct, .. }
        | Inst::FlatAppend { construct, .. }
        | Inst::FlatClose { construct, .. } => construct_values(cx, construct).collect(),
        Inst::FlatField { op, .. } => match op {
            FlatFieldOp::Open => Vec::new(),
            FlatFieldOp::Reserve { capacity: value } | FlatFieldOp::Push { value, .. } => {
                vec![value]
            }
        },
        Inst::Closure { closure, .. } => unit.tables.closures[closure.index()]
            .captures
            .get(&unit.pools.captures)
            .unwrap_or(&[])
            .iter()
            .map(|c: &CaptureValue| c.value)
            .collect(),
        Inst::Store { value, .. }
        | Inst::Replace { value, .. }
        | Inst::UniqueCopy { value, .. }
        | Inst::Freeze { value, .. }
        | Inst::Thaw { value, .. }
        | Inst::Dup { value }
        | Inst::Share { value }
        | Inst::IsUnique { value, .. }
        | Inst::IsWritable { value, .. }
        | Inst::Weaken { value, .. }
        | Inst::SeqLen { value, .. }
        | Inst::SeqCapacity { value, .. }
        | Inst::Hash { value, .. }
        | Inst::Encode { value, .. }
        | Inst::Decode { value, .. }
        | Inst::Recast { value, .. }
        | Inst::SeqReserve {
            capacity: value, ..
        }
        | Inst::SeqSetLen { len: value, .. }
        | Inst::SeqAppend { value, .. }
        | Inst::ArenaPush {
            count: Some(value), ..
        }
        | Inst::Release { value, .. }
        | Inst::Reset { value, .. }
        | Inst::Forget { value }
        | Inst::RegionPublish { value } => vec![value],
        Inst::Unary { operand, .. }
        | Inst::Convert { operand, .. }
        | Inst::ScalarUnary { operand, .. } => vec![operand],
        Inst::Binary { left, right, .. }
        | Inst::Compare { left, right, .. }
        | Inst::IntArith { left, right, .. } => vec![left, right],
        Inst::MulAdd {
            left,
            right,
            addend,
            ..
        } => vec![left, right, addend],
        Inst::Check { condition, .. } => vec![condition],
        Inst::Read { reaching, .. } | Inst::Copy { reaching, .. } => vec![reaching],
        _ => Vec::new(),
    }
}

fn terminator_values<S>(cx: &Cx<'_, S>, term: &Terminator) -> Vec<ValueId> {
    let unit = cx.unit;
    match *term {
        Terminator::Branch { condition, .. } => vec![condition],
        Terminator::Switch { value, .. } => vec![value],
        Terminator::Return(Some(value)) => vec![value],
        Terminator::Call { call, .. } => {
            let site = &unit.tables.calls[call.index()];
            let mut out: Vec<_> = site
                .arguments
                .get(&unit.pools.call_arguments)
                .unwrap_or(&[])
                .iter()
                .filter_map(|a| match a.value {
                    ArgumentValue::Value(v) => Some(v),
                    ArgumentValue::Address { .. } => None,
                })
                .collect();
            if let Some(v) = site.callee.value() {
                out.push(v);
            }
            out
        }
        _ => Vec::new(),
    }
}

//! Drop specialization: a known block whose counted children are
//! copied out right before its release.
//!
//! ```text
//! a1 = address d.f1; ..; an = address d.fn
//! x1 = copy a1; .. xn = copy an; release d
//! ```
//!
//! copies each child (a `Dup`) and then, when `d` was unique, drops those
//! same children again inside the release. Specialized, the block (whose
//! address preparations stay before the test) tests its count first:
//!
//! ```text
//! u = is_unique d; branch u
//!   unique: x1 = take d.f1; ..; release d [live: all but f1..fn]  -- no Dup
//!   shared: x1 = copy d.f1; ..; release d                          -- a decrement
//! join(x1, .., xn): the rest of the block
//! ```
//!
//! Both paths use the layout's own drop program: the unique release runs it
//! under the mask of the children that stay, the shared one decrements. The
//! token pass later turns the unique release into a `Reset` when a
//! construction of its bucket follows, and recognizes the shared path's
//! release as never donating. Copies of plain or flat children, and copies
//! not immediately before the release, are left alone, as is a run whose
//! addresses are prepared between its copies.
//!
//! Each path's children are values of their own, so a failure cleanup that
//! releases a copied child by name (as the release's own cleanup does) is
//! cloned per path with the child renamed. Only a cleanup of releases, drops
//! and region exits with no parameters or results is cloned; any other leaves
//! the run alone.
use crate::PassError;
use crate::insert::{Editor, Facts, Place, Root, Step};
use crate::reuse::type_of;
use hirsel_lir::ir::{
    BlockId, EdgeId, FailureId, FieldMask, Holding, Inst, LirUnit, MaskId, MaskRef, PoolRange,
    ReadMode, Ref, Terminator, ValueId, ValueRepr,
};
use hirsel_lir::layout_types::{CopyRecipe, LayoutId, LayoutKind, LayoutTable, ScalarKind};
use hirsel_lir::stage::Reuse;
use std::collections::{BTreeMap, BTreeSet};

pub fn specialize(unit: LirUnit<Reuse>, table: &LayoutTable) -> Result<LirUnit<Reuse>, PassError> {
    let Some(bool_layout) = table
        .layouts
        .iter()
        .position(|l| l.kind == LayoutKind::Scalar(ScalarKind::Bool))
        .map(|i| LayoutId(i as u32))
    else {
        return Ok(unit);
    };
    let facts = Facts::new(&unit, table);
    let mut ed = Editor::new(unit);
    let bool_ty = type_of(
        &mut ed.unit,
        ValueRepr::Scalar(ScalarKind::Bool),
        bool_layout,
    );
    let mut work: Vec<usize> = (0..ed.code.len()).collect();
    while let Some(b) = work.pop() {
        let Some(found) = candidate(&ed, &facts, table, b) else {
            continue;
        };
        let join = rewrite(&mut ed, b, found, bool_ty, table);
        work.push(join.index());
    }
    Ok(ed.finish())
}

/// The run of child copies `first..release` and the release at `release`.
struct Candidate {
    first: usize,
    release: usize,
}

fn candidate(
    ed: &Editor<Reuse>,
    facts: &Facts,
    table: &LayoutTable,
    b: usize,
) -> Option<Candidate> {
    let code = &ed.code[b];
    for (r, inst) in code.iter().enumerate() {
        let Inst::Release {
            value: d,
            live: MaskRef::All,
            failure,
            ..
        } = *inst
        else {
            continue;
        };
        let unit = &ed.unit;
        if unit.types[unit.values[d.index()].ty.index()].repr != ValueRepr::Block
            || unit.values[d.index()].holding != Holding::Owned
            || facts.statics.get(d.index()).copied().unwrap_or(false)
        {
            continue;
        }
        let mut first = r;
        let mut fields = BTreeSet::new();
        while first > 0 {
            let Inst::Copy {
                result,
                address,
                reaching,
                ..
            } = code[first - 1]
            else {
                break;
            };
            let child = facts.places.get(address.index()).cloned().flatten();
            let Some(f) = child_field(&child, d) else {
                break;
            };
            let layout = unit.types[unit.values[result.index()].ty.index()].layout;
            let counted = table.layout(layout).is_some_and(|l| {
                l.copy == CopyRecipe::Dup
                    && matches!(l.kind, LayoutKind::Block { .. } | LayoutKind::Box { .. })
            });
            if reaching != d || !counted || !fields.insert(f) {
                break;
            }
            first -= 1;
        }
        if first == r {
            continue;
        }
        let copied: BTreeSet<ValueId> = code[first..r]
            .iter()
            .filter_map(|i| match *i {
                Inst::Copy { result, .. } => Some(result),
                _ => None,
            })
            .collect();
        let mut failures = vec![failure];
        failures.extend(code[first..r].iter().filter_map(Inst::failure));
        if !failures.iter().all(|&f| clonable(ed, f, &copied)) {
            continue;
        }
        return Some(Candidate { first, release: r });
    }
    None
}

/// The field of `d`'s payload a one-step place through `d` names.
fn child_field(place: &Option<Place>, d: ValueId) -> Option<u32> {
    let place = place.as_ref()?;
    if place.root != Root::Value(d) || place.crossings != [0] {
        return None;
    }
    match place.path[..] {
        [Step::Field(f)] | [Step::Variant(_, f)] => Some(f),
        _ => None,
    }
}

/// The blocks reachable from failure edge `f` that mention one of `values`
/// or lead to one that does: what a renamed clone must copy.
fn mentioning(ed: &Editor<Reuse>, f: FailureId, values: &BTreeSet<ValueId>) -> BTreeSet<BlockId> {
    let unit = &ed.unit;
    let succ = |b: BlockId| -> Vec<EdgeId> {
        let mut out: Vec<EdgeId> = ed.code[b.index()]
            .iter()
            .filter_map(Inst::failure)
            .map(|f| unit.tables.failures[f.index()].cleanup)
            .collect();
        out.extend(crate::insert::terminator_edges(
            unit,
            &unit.terminators[b.index()],
        ));
        out
    };
    let names = |b: BlockId| {
        let mut hit = false;
        for inst in &ed.code[b.index()] {
            inst.visit(&mut |r| {
                if let Ref::Value(v) = r {
                    hit |= values.contains(&v);
                }
            });
        }
        hit || crate::liveness::terminator_uses(unit, &unit.terminators[b.index()])
            .iter()
            .any(|v| values.contains(v))
    };
    let start = unit.tables.edges[unit.tables.failures[f.index()].cleanup.index()].target;
    let mut order = Vec::new();
    let mut seen = BTreeSet::from([start]);
    let mut stack = vec![start];
    while let Some(b) = stack.pop() {
        order.push(b);
        for e in succ(b) {
            let t = unit.tables.edges[e.index()].target;
            if seen.insert(t) {
                stack.push(t);
            }
        }
    }
    let mut out: BTreeSet<BlockId> = order.iter().copied().filter(|&b| names(b)).collect();
    loop {
        let before = out.len();
        for &b in &order {
            if succ(b)
                .iter()
                .any(|e| out.contains(&unit.tables.edges[e.index()].target))
            {
                out.insert(b);
            }
        }
        if out.len() == before {
            return out;
        }
    }
}

/// Whether the cleanup of `f` can be cloned with `values` renamed.
fn clonable(ed: &Editor<Reuse>, f: FailureId, values: &BTreeSet<ValueId>) -> bool {
    let unit = &ed.unit;
    mentioning(ed, f, values).into_iter().all(|b| {
        let plain = unit.blocks[b.index()].parameters.len == 0
            && matches!(
                unit.terminators[b.index()],
                Terminator::Jump(_) | Terminator::Propagate | Terminator::Unreachable
            );
        plain
            && ed.code[b.index()].iter().all(|inst| {
                Inst::defs(inst).is_empty()
                    && match inst {
                        Inst::Release { .. }
                        | Inst::Drop { .. }
                        | Inst::RegionExit { .. }
                        | Inst::Dup { .. } => true,
                        _ => {
                            Inst::failure(inst).is_none() && {
                                let mut hit = false;
                                inst.visit(&mut |r| {
                                    if let Ref::Value(v) = r {
                                        hit |= values.contains(&v);
                                    }
                                });
                                !hit
                            }
                        }
                    }
            })
    })
}

/// A new failure edge like `f` for another site, its cleanup cloned with
/// `map` applied wherever it names a renamed value.
fn clone_failure(
    ed: &mut Editor<Reuse>,
    f: FailureId,
    map: &BTreeMap<ValueId, ValueId>,
) -> FailureId {
    let keys = map.keys().copied().collect();
    let cloned = mentioning(ed, f, &keys);
    let mut memo = BTreeMap::new();
    clone_failure_in(ed, f, map, &cloned, &mut memo)
}

fn clone_failure_in(
    ed: &mut Editor<Reuse>,
    f: FailureId,
    map: &BTreeMap<ValueId, ValueId>,
    cloned: &BTreeSet<BlockId>,
    memo: &mut BTreeMap<BlockId, BlockId>,
) -> FailureId {
    let old = ed.unit.tables.failures[f.index()].clone();
    let cleanup = clone_edge(ed, old.cleanup, map, cloned, memo);
    let id = ed.failure(cleanup, old.anchor);
    let new = &mut ed.unit.tables.failures[id.index()];
    new.rule = old.rule;
    new.dispose = old.dispose;
    id
}

fn clone_edge(
    ed: &mut Editor<Reuse>,
    e: EdgeId,
    map: &BTreeMap<ValueId, ValueId>,
    cloned: &BTreeSet<BlockId>,
    memo: &mut BTreeMap<BlockId, BlockId>,
) -> EdgeId {
    let edge = ed.unit.tables.edges[e.index()].clone();
    let rename = |v: ValueId| map.get(&v).copied().unwrap_or(v);
    let args: Vec<ValueId> = edge
        .arguments
        .get(&ed.unit.pools.values)
        .unwrap_or(&[])
        .iter()
        .map(|&v| rename(v))
        .collect();
    let target = if cloned.contains(&edge.target) {
        clone_block(ed, edge.target, map, cloned, memo)
    } else {
        edge.target
    };
    ed.edge(target, &args)
}

fn clone_block(
    ed: &mut Editor<Reuse>,
    b: BlockId,
    map: &BTreeMap<ValueId, ValueId>,
    cloned: &BTreeSet<BlockId>,
    memo: &mut BTreeMap<BlockId, BlockId>,
) -> BlockId {
    if let Some(&c) = memo.get(&b) {
        return c;
    }
    let (kind, anchor) = (
        ed.unit.blocks[b.index()].kind,
        ed.unit.blocks[b.index()].anchor,
    );
    let c = ed.block(kind, &[], anchor);
    memo.insert(b, c);
    let rename = |v: ValueId| map.get(&v).copied().unwrap_or(v);
    let mut code = Vec::new();
    for mut inst in ed.code[b.index()].clone() {
        match &mut inst {
            Inst::Release { value, failure, .. } => {
                *value = rename(*value);
                *failure = clone_failure_in(ed, *failure, map, cloned, memo);
            }
            Inst::Dup { value } => *value = rename(*value),
            Inst::Drop { failure, .. } | Inst::RegionExit { failure, .. } => {
                *failure = clone_failure_in(ed, *failure, map, cloned, memo);
            }
            _ => {}
        }
        code.push(inst);
    }
    ed.code[c.index()] = code;
    let term = match ed.unit.terminators[b.index()].clone() {
        Terminator::Jump(e) => Terminator::Jump(clone_edge(ed, e, map, cloned, memo)),
        other => other,
    };
    ed.unit.terminators[c.index()] = term;
    c
}

fn rewrite(
    ed: &mut Editor<Reuse>,
    b: usize,
    c: Candidate,
    bool_ty: hirsel_lir::ir::LirTypeId,
    table: &LayoutTable,
) -> BlockId {
    let Inst::Release {
        value: d,
        layout,
        anchor,
        failure,
        ..
    } = ed.code[b][c.release]
    else {
        unreachable!("candidate names a release")
    };
    let kind = ed.unit.blocks[b].kind;
    let at = ed.unit.blocks[b].anchor;
    let tail: Vec<Inst> = ed.code[b].split_off(c.release + 1);
    let run: Vec<Inst> = ed.code[b].split_off(c.first);
    let copies: Vec<Inst> = run[..run.len() - 1].to_vec();
    let children: Vec<ValueId> = copies
        .iter()
        .map(|i| match *i {
            Inst::Copy { result, .. } => result,
            _ => unreachable!("the run is copies"),
        })
        .collect();

    // The join receives the children under their original names, so every
    // later use stays as it is.
    let join = ed.block(kind, &children, at);
    ed.code[join.index()] = tail;
    ed.unit.terminators[join.index()] =
        std::mem::replace(&mut ed.unit.terminators[b], Terminator::Unreachable);

    let fresh = |ed: &mut Editor<Reuse>, v: ValueId| {
        let info = ed.unit.values[v.index()].clone();
        ed.value(info.ty, info.holding)
    };

    // Unique: move each child out; the release drops only what stays.
    let unique = ed.block(kind, &[], at);
    let mut taken = Vec::new();
    let mut moved = BTreeSet::new();
    for inst in &copies {
        let Inst::Copy {
            result, address, ..
        } = *inst
        else {
            unreachable!()
        };
        let x = fresh(ed, result);
        taken.push(x);
        ed.code[unique.index()].push(Inst::Read {
            result: x,
            address,
            reaching: d,
            mode: ReadMode::Take,
        });
        let field = field_of(ed, address, d);
        moved.insert(field);
    }
    let mask = live_mask(ed, table, layout, &moved);
    let renamed: BTreeMap<ValueId, ValueId> = children
        .iter()
        .copied()
        .zip(taken.iter().copied())
        .collect();
    let unique_failure = clone_failure(ed, failure, &renamed);
    ed.code[unique.index()].push(Inst::Release {
        value: d,
        layout,
        live: MaskRef::Static(mask),
        anchor,
        failure: unique_failure,
    });
    let to_join = ed.edge(join, &taken);
    ed.unit.terminators[unique.index()] = Terminator::Jump(to_join);

    // Shared: the copies and the plain release, as before.
    let shared = ed.block(kind, &[], at);
    let fresh_copies: Vec<ValueId> = children.iter().map(|&v| fresh(ed, v)).collect();
    let renamed: BTreeMap<ValueId, ValueId> = children
        .iter()
        .copied()
        .zip(fresh_copies.iter().copied())
        .collect();
    let mut copied = Vec::new();
    for (inst, x) in copies.into_iter().zip(fresh_copies) {
        let Inst::Copy {
            result,
            address,
            reaching,
            failure,
        } = inst
        else {
            unreachable!()
        };
        let _ = result;
        copied.push(x);
        let failure = clone_failure(ed, failure, &renamed);
        ed.code[shared.index()].push(Inst::Copy {
            result: x,
            address,
            reaching,
            failure,
        });
    }
    let failure = clone_failure(ed, failure, &renamed);
    ed.code[shared.index()].push(Inst::Release {
        value: d,
        layout,
        live: MaskRef::All,
        anchor,
        failure,
    });
    let to_join = ed.edge(join, &copied);
    ed.unit.terminators[shared.index()] = Terminator::Jump(to_join);

    let condition = ed.value(bool_ty, Holding::Plain);
    ed.code[b].push(Inst::IsUnique {
        result: condition,
        value: d,
    });
    let yes = ed.edge(unique, &[]);
    let no = ed.edge(shared, &[]);
    ed.unit.terminators[b] = Terminator::Branch { condition, yes, no };
    join
}

fn field_of(ed: &Editor<Reuse>, address: hirsel_lir::ir::AddressId, d: ValueId) -> u32 {
    let info = &ed.unit.addresses[address.index()];
    debug_assert_eq!(info.base, hirsel_lir::ir::AddressBase::Value(d));
    match info
        .projection
        .get(&ed.unit.pools.projections)
        .unwrap_or(&[])
    {
        [hirsel_lir::ir::ProjectionStep::Field(f)]
        | [hirsel_lir::ir::ProjectionStep::VariantField { field: f, .. }] => *f,
        _ => unreachable!("a candidate child is one field step"),
    }
}

/// A static mask over the payload's field ordinals, clear for `moved`.
pub(crate) fn live_mask(
    ed: &mut Editor<Reuse>,
    table: &LayoutTable,
    payload: LayoutId,
    moved: &BTreeSet<u32>,
) -> MaskId {
    let record = table.layout(payload);
    let ordinals = record
        .iter()
        .flat_map(|r| r.fields.iter().map(|f| f.ordinal))
        .chain(
            record
                .iter()
                .flat_map(|r| r.tag.iter())
                .flat_map(|t| &t.variants)
                .flat_map(|v| v.fields.iter().map(|f| f.ordinal)),
        );
    let fields = ordinals.max().map_or(0, |m| m + 1);
    mask(ed, fields, |i| !moved.contains(&i))
}

pub(crate) fn mask(ed: &mut Editor<Reuse>, fields: u32, set: impl Fn(u32) -> bool) -> MaskId {
    let mut words = vec![0u64; (fields as usize).div_ceil(64)];
    for i in 0..fields {
        if set(i) {
            words[i as usize / 64] |= 1 << (i % 64);
        }
    }
    let words = PoolRange::push(&mut ed.unit.pools.mask_words, words);
    ed.unit.tables.masks.push(FieldMask { fields, words });
    MaskId(ed.unit.tables.masks.len() as u32 - 1)
}

//! Selective writes ("Linear raw reuse tokens"): a reuse skips
//! the store of a field whose operand the token's storage already holds.
//!
//! A field is kept only with physical-slot evidence, the same evidence the
//! stage validator replays: wherever the token holds storage it was reset
//! from a donor of the construction's own payload layout whose live mask
//! cleared that field, and the field's operand is the value taken out of
//! that same field of that donor (for a variant, the construction's own
//! variant, which the take's preparation proved active). Through a join,
//! every edge passing a reset token passes that take; an edge passing an
//! explicit None token stores every field anyway. Source field names or
//! constructor argument ordinals alone are never evidence: a field that
//! moves to another slot (`P(a: y, b: x)` from `P(a: x, b: y)`) is stored.
//!
//! The operand is still evaluated (it is the take), so no evaluation is
//! skipped, only the store of bits that are already there.
use crate::reuse::type_of;
use hirsel_lir::ir::{
    AddressBase, BlockId, ConstructorKind, FieldMask, Inst, LirUnit, MaskId, MaskRef, PoolRange,
    ProjectionStep, ReadMode, ValueId,
};
use hirsel_lir::layout_types::{LayoutId, LayoutTable};
use hirsel_lir::stage::Reuse;
use std::collections::BTreeSet;

#[derive(Clone, Copy)]
enum Def {
    Inst(usize),
    Param(BlockId, usize),
}

pub fn keep(mut unit: LirUnit<Reuse>, table: &LayoutTable) -> LirUnit<Reuse> {
    let _ = (table, type_of::<Reuse>);
    let mut defs: Vec<Option<Def>> = vec![None; unit.values.len()];
    for (b, block) in unit.blocks.iter().enumerate() {
        for (i, &p) in block
            .parameters
            .get(&unit.pools.values)
            .unwrap_or(&[])
            .iter()
            .enumerate()
        {
            defs[p.index()] = Some(Def::Param(BlockId(b as u32), i));
        }
    }
    for (i, inst) in unit.instructions.iter().enumerate() {
        for v in Inst::defs(inst) {
            defs[v.index()] = Some(Def::Inst(i));
        }
    }
    let mut kept_sets: Vec<(usize, u32, BTreeSet<u32>)> = Vec::new();
    for (i, inst) in unit.instructions.iter().enumerate() {
        let Inst::Reuse {
            token,
            construct,
            kept: None,
            ..
        } = *inst
        else {
            continue;
        };
        let site = &unit.tables.constructs[construct.index()];
        let variant = match site.constructor {
            ConstructorKind::Record(_) | ConstructorKind::Tuple => None,
            ConstructorKind::Variant { variant, .. } => Some(variant),
            _ => continue,
        };
        let mut kept = BTreeSet::new();
        let mut fields = 0;
        for f in site.fields.get(&unit.pools.fields).unwrap_or(&[]) {
            fields = fields.max(f.field + 1);
            let step = match variant {
                None => ProjectionStep::Field(f.field),
                Some(variant) => ProjectionStep::VariantField {
                    variant,
                    field: f.field,
                },
            };
            if evidence(
                &unit,
                &defs,
                token,
                f.value,
                site.layout,
                step,
                &mut BTreeSet::new(),
            ) {
                kept.insert(f.field);
            }
        }
        if !kept.is_empty() {
            kept_sets.push((i, fields, kept));
        }
    }
    for (i, fields, kept) in kept_sets {
        let id = mask(&mut unit, fields, &kept);
        if let Inst::Reuse { kept, .. } = &mut unit.instructions[i] {
            *kept = Some(id);
        }
    }
    unit
}

fn mask(unit: &mut LirUnit<Reuse>, fields: u32, set: &BTreeSet<u32>) -> MaskId {
    let mut words = vec![0u64; (fields as usize).div_ceil(64)];
    for &i in set {
        words[i as usize / 64] |= 1 << (i % 64);
    }
    let words = PoolRange::push(&mut unit.pools.mask_words, words);
    unit.tables.masks.push(FieldMask { fields, words });
    MaskId(unit.tables.masks.len() as u32 - 1)
}

fn evidence(
    unit: &LirUnit<Reuse>,
    defs: &[Option<Def>],
    token: ValueId,
    value: ValueId,
    layout: LayoutId,
    step: ProjectionStep,
    seen: &mut BTreeSet<(ValueId, ValueId)>,
) -> bool {
    if !seen.insert((token, value)) {
        return false;
    }
    match (defs[token.index()], defs[value.index()]) {
        (Some(Def::Param(tb, ti)), Some(Def::Param(vb, vi))) if tb == vb => {
            let incoming: Vec<_> = unit
                .tables
                .edges
                .iter()
                .filter(|e| e.target == tb)
                .collect();
            !incoming.is_empty()
                && incoming.iter().all(|e| {
                    let args = e.arguments.get(&unit.pools.values).unwrap_or(&[]);
                    match (args.get(ti), args.get(vi)) {
                        (Some(&t), Some(&v)) => evidence(unit, defs, t, v, layout, step, seen),
                        _ => false,
                    }
                })
        }
        (Some(Def::Inst(t)), _) => match unit.instructions[t] {
            Inst::TokenNone { .. } => true,
            Inst::Reset {
                value: donor,
                layout: reset,
                live: MaskRef::Static(live),
                ..
            } if reset == layout => {
                let field = match step {
                    ProjectionStep::Field(f) | ProjectionStep::VariantField { field: f, .. } => f,
                    ProjectionStep::Payload
                    | ProjectionStep::Index(_)
                    | ProjectionStep::Slot(_)
                    | ProjectionStep::FlatAt { .. } => return false,
                };
                let m = &unit.tables.masks[live.index()];
                let words = m.words.get(&unit.pools.mask_words).unwrap_or(&[]);
                let cleared = !words
                    .get(field as usize / 64)
                    .is_some_and(|w| w >> (field % 64) & 1 == 1);
                let Some(Def::Inst(v)) = defs[value.index()] else {
                    return false;
                };
                let Inst::Read {
                    address,
                    mode: ReadMode::Take,
                    ..
                } = unit.instructions[v]
                else {
                    return false;
                };
                let a = &unit.addresses[address.index()];
                cleared
                    && a.base == AddressBase::Value(donor)
                    && a.projection.get(&unit.pools.projections) == Some(&[step][..])
            }
            _ => false,
        },
        _ => false,
    }
}

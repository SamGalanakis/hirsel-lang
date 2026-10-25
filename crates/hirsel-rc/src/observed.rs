//! Explicit ownership-plan transport through the complete Value-to-Ready ladder.
//! Ordinary compilation does not call this diagnostic entry. Each pass checks
//! its input and its own recipe before the next pass can consume the plan.
use crate::{cleanup, elide, insert, placement, ready, reuse};
use hirsel_lir::ir::LirUnit;
use hirsel_lir::ownership_trace::{Plan, Recipe};
use hirsel_lir::stage::{Ready, Value};

pub struct Options<'a> {
    pub placement: &'a placement::Options,
    pub proofs: &'a insert::Proofs,
    pub elide: bool,
    pub reuse: &'a reuse::Options,
    pub finalizers: &'a cleanup::Finalizers,
}

pub struct Lowered {
    pub unit: LirUnit<Ready>,
    pub plan: Plan,
    /// Consecutive, checked boundaries, including disabled optimization passes.
    pub recipes: Vec<Recipe>,
}

/// Transport an existing plan; never derive a replacement from rewritten LIR.
/// The caller still validates the ordinary input/output independently. A plan
/// grants no ownership, scheduling or semantic validation authority.
pub fn lower(
    unit: LirUnit<Value>,
    input: &Plan,
    index: &hirsel_lir::LayoutIndex<'_>,
    options: &Options<'_>,
) -> Result<Lowered, String> {
    let table = index.table();
    let (unit, plan, placement) = placement::place_observed(unit, table, options.placement, input)?;
    let mut recipes = vec![placement];
    let (unit, plan, insertion) = insert::insert_observed(unit, table, options.proofs, &plan)?;
    recipes.extend(insertion);
    let (unit, plan, elision) = if options.elide {
        elide::elide_observed(unit, table, &plan)?
    } else {
        crate::observation::identity(&unit, &plan, unit.clone())?
    };
    recipes.push(elision);
    let (unit, plan, reuse) = reuse::reuse_observed(unit, options.reuse, index, &plan)?;
    recipes.extend(reuse);
    let (unit, plan, ready) = ready::ready_observed(unit, options.finalizers, index, &plan)?;
    recipes.extend(ready);
    Ok(Lowered {
        unit,
        plan,
        recipes,
    })
}

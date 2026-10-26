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
    lower_with_checks(unit, input, index, options, None, None)
}

/// The diagnostic query path checks ordinary Value and intermediate stages as
/// well as each observation recipe. These checks grant the side table no C
/// authority; the enclosing producer still validates its final Ready output.
pub fn lower_validated(
    unit: LirUnit<Value>,
    input: &Plan,
    inputs: &hirsel_lir::validate::Inputs<'_>,
    options: &Options<'_>,
) -> Result<Lowered, String> {
    let validation = hirsel_lir::validate::ValidationContext::new(inputs);
    lower_with_checks(unit, input, validation.index(), options, Some(inputs), None)
}

pub fn lower_validated_with_guard(
    unit: LirUnit<Value>,
    input: &Plan,
    inputs: &hirsel_lir::validate::Inputs<'_>,
    options: &Options<'_>,
    guard: Option<&crate::seal_demand::UnitDemand>,
) -> Result<Lowered, String> {
    let validation = hirsel_lir::validate::ValidationContext::new(inputs);
    lower_with_checks(
        unit,
        input,
        validation.index(),
        options,
        Some(inputs),
        guard,
    )
}

fn checked<S: hirsel_lir::stage::Stage>(
    unit: LirUnit<S>,
    inputs: Option<&hirsel_lir::validate::Inputs<'_>>,
) -> Result<LirUnit<S>, String> {
    if let Some(inputs) = inputs {
        hirsel_lir::validate::validate(unit.clone(), inputs).map_err(|error| {
            format!(
                "ownership trace stage {:?}: {:?}",
                S::KIND,
                error.violations
            )
        })?;
    }
    Ok(unit)
}

fn lower_with_checks(
    unit: LirUnit<Value>,
    input: &Plan,
    index: &hirsel_lir::LayoutIndex<'_>,
    options: &Options<'_>,
    inputs: Option<&hirsel_lir::validate::Inputs<'_>>,
    guard: Option<&crate::seal_demand::UnitDemand>,
) -> Result<Lowered, String> {
    let unit = checked(unit, inputs)?;
    let table = index.table();
    let (unit, plan, placement) = if let Some(guard) = guard {
        hirsel_lir::ownership_trace::check(&unit, input).map_err(|e| e.to_string())?;
        let before = unit.clone();
        let mut after =
            placement::place(unit, table, options.placement).map_err(|e| e.to_string())?;
        guard.apply(&mut after)?;
        crate::observation::identity(&before, input, after)?
    } else {
        placement::place_observed(unit, table, options.placement, input)?
    };
    let unit = checked(unit, inputs)?;
    let mut recipes = vec![placement];
    let (unit, plan, insertion) = insert::insert_observed(unit, table, options.proofs, &plan)?;
    recipes.extend(insertion);
    let (unit, plan, elision) = if options.elide {
        elide::elide_observed(unit, table, &plan)?
    } else {
        crate::observation::identity(&unit, &plan, unit.clone())?
    };
    let unit = checked(unit, inputs)?;
    recipes.push(elision);
    let (unit, plan, reuse) = reuse::reuse_observed(unit, options.reuse, index, &plan)?;
    let unit = checked(unit, inputs)?;
    recipes.extend(reuse);
    let (unit, plan, ready) = ready::ready_observed(unit, options.finalizers, index, &plan)?;
    recipes.extend(ready);
    Ok(Lowered {
        unit,
        plan,
        recipes,
    })
}

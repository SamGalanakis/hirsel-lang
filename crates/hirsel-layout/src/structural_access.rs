//! Bind logical access to independent source declarations and native layout keys.
use crate::layout::{LayoutBuilder, Owners};
use hirsel_lir::layout::LayoutError;
use hirsel_lir::operation::OperationArgument;
use hirsel_lir::structural_access::AccessPlan;
use hirsel_lir::{LayoutId, LayoutKeys, LayoutTable, TargetSpec};
use hirsel_syntax::DefId;
use hirsel_types::{GenericArgument, QueryError, SignatureRef, TypeId, TypeInterner};

/// `root` and `lookup` belong to the independent declaration provider, rather
/// than to metadata copied out of the candidate access plan.
pub fn derive(
    types: &TypeInterner,
    root: TypeId,
    lookup: &mut impl FnMut(DefId) -> Result<Option<SignatureRef>, QueryError>,
    owners: &Owners,
    target: &TargetSpec,
    table: &LayoutTable,
) -> Result<AccessPlan, LayoutError> {
    let mut builder = LayoutBuilder::new(target, owners);
    let expected_root = builder.layout(types, root)?;
    let expected_table = builder.finish();
    let expected_keys = LayoutKeys::new(&expected_table);
    let key = expected_keys.key(expected_root);
    let actual_keys = LayoutKeys::new(table);
    let root_layout = (0..table.layouts.len())
        .map(|i| LayoutId(i as u32))
        .find(|&id| actual_keys.key(id) == key)
        .ok_or_else(|| {
            LayoutError::Invalid("structural layout differs from its declared type".into())
        })?;
    at_layout(types, root, lookup, table, root_layout)
}

/// Build physical projections at a producer-selected layout. Independent
/// validation additionally binds that layout's content key to the declaration.
/// Logical shapes use the same erased type identity as `AccessPlan::source`:
/// region/evidence qualifiers must not change recursive graph memoization.
/// Distinct declarations and all of their payload edges remain represented.
pub fn at_layout(
    types: &TypeInterner,
    root: TypeId,
    lookup: &mut impl FnMut(DefId) -> Result<Option<SignatureRef>, QueryError>,
    table: &LayoutTable,
    root_layout: LayoutId,
) -> Result<AccessPlan, LayoutError> {
    let mut arguments = crate::operation::collect_arguments(types, &[GenericArgument::Type(root)])?;
    let Some(OperationArgument::Type(source)) = arguments.pop() else {
        return Err(LayoutError::MissingType);
    };
    let canonical = crate::operation::intern_native(&source, types, 0)?;
    let graph = crate::structural::derive(types, canonical, lookup)
        .map_err(|e| LayoutError::Invalid(format!("structural declarations: {e:?}")))?
        .ok_or_else(|| LayoutError::Invalid("type has no complete structural view".into()))?;
    AccessPlan::derive(source, graph, table, root_layout).map_err(LayoutError::Invalid)
}

/// Reconstruct the whole association; matching producer graph and edge totals
/// alone cannot establish a source type's physical projection.
pub fn validate(
    plan: &AccessPlan,
    types: &TypeInterner,
    root: TypeId,
    lookup: &mut impl FnMut(DefId) -> Result<Option<SignatureRef>, QueryError>,
    owners: &Owners,
    target: &TargetSpec,
    table: &LayoutTable,
) -> Result<(), LayoutError> {
    plan.check(table).map_err(LayoutError::Invalid)?;
    if &derive(types, root, lookup, owners, target, table)? != plan {
        return Err(LayoutError::Invalid(
            "structural access differs from independent declarations".into(),
        ));
    }
    Ok(())
}

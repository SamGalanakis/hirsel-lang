//! Reconstruct sealed descriptors from source declarations and compare their
//! physical association through content keys, never table-local ID equality.
use crate::operation::{OperationRegistry, intern_native};
use hirsel_lir::layout::LayoutError;
use hirsel_lir::operation::NativeType;
use hirsel_lir::sealed::{SealDescriptor, StorageAccess};
use hirsel_lir::{LayoutTable, TargetSpec};
use std::collections::BTreeSet;

pub fn derive(
    registry: &OperationRegistry,
    root_type: &NativeType,
    table: &LayoutTable,
    target: &TargetSpec,
) -> Result<SealDescriptor, LayoutError> {
    let root = intern_native(root_type, &registry.types, 0)?;
    let mut lookup = |definition| Ok(registry.type_declarations.get(&definition).cloned());
    let logical = crate::logical_storage::derive(&registry.types, root, &mut lookup)
        .map_err(|e| LayoutError::Invalid(format!("sealed logical storage: {e:?}")))?
        .ok_or_else(|| {
            LayoutError::Invalid("sealed storage lacks complete logical facts".into())
        })?;
    let projection = crate::structural_access::derive(
        &registry.types,
        root,
        &mut lookup,
        &registry.owners,
        target,
        table,
    )?;
    let root_layout = projection.root_layout();
    let index = hirsel_lir::LayoutIndex::new(table);
    let mut pending: Vec<_> = projection.layouts().collect();
    let mut reached = BTreeSet::new();
    while let Some(id) = pending.pop() {
        if !reached.insert(id) {
            continue;
        }
        let row = table.layout(id).ok_or(LayoutError::MissingType)?;
        if matches!(
            row.kind,
            hirsel_lir::layout_types::LayoutKind::Shared { .. }
        ) {
            continue;
        }
        pending.extend(row.references());
        pending.extend(index.implicit(id));
    }
    let access = reached
        .into_iter()
        .map(|layout| StorageAccess {
            layout,
            scan: table.layouts[layout.0 as usize].scan.clone(),
        })
        .collect();
    let descriptor = SealDescriptor {
        root_type: root_type.clone(),
        root_layout,
        logical,
        projection,
        access,
    };
    descriptor.check(table).map_err(LayoutError::Invalid)?;
    Ok(descriptor)
}

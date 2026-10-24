//! Typed sealed-storage contracts. Logical graph identity is separate from
//! physical access. These data are untrusted until checked against independent
//! declarations and the validated layout table.
use crate::layout_types::{LayoutId, LayoutKind, LayoutTable, ScanStep};
use crate::logical_storage::LogicalStorageGraph;
use crate::operation::NativeType;
use hirsel_codec::{snapshot_enum, snapshot_struct};
use std::collections::{BTreeMap, BTreeSet};

pub use crate::sealed_payload_source::{SourceEvent, SourceEventKind};

/// Physical traversal for one layout, including uncounted arena child edges.
/// Using its drop program instead would omit borrowed/uncounted storage edges.
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct StorageAccess {
    pub layout: LayoutId,
    pub scan: Vec<ScanStep>,
}

/// A descriptor-to-declaration association. `projection` maps logical nodes
/// through the complete physical representation, including Flat framing and
/// flattened nominal fields. Access programs include private payloads too.
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct SealDescriptor {
    pub root_type: NativeType,
    pub root_layout: LayoutId,
    pub logical: LogicalStorageGraph,
    pub projection: crate::structural_access::AccessPlan,
    pub access: Vec<StorageAccess>,
}

impl SealDescriptor {
    /// Physical consistency only. The public validator must also reconstruct
    /// the logical graph and bindings from independent type declarations.
    pub fn check(&self, table: &LayoutTable) -> Result<(), String> {
        self.logical.check().map_err(str::to_owned)?;
        self.projection.check(table)?;
        if self.projection.graph != self.logical.shape
            || self.projection.source != self.root_type
            || self.projection.root_layout() != self.root_layout
        {
            return Err("sealed logical nodes lack complete physical projection".into());
        }
        let mut programs = BTreeMap::new();
        for program in &self.access {
            if programs.insert(program.layout, program).is_some() {
                return Err("duplicate sealed physical access program".into());
            }
            let layout = table
                .layout(program.layout)
                .ok_or("foreign sealed access layout")?;
            if matches!(layout.kind, LayoutKind::PayloadReceipt { .. }) {
                return Err("private receipt metadata is not sealed source payload".into());
            }
            if program.scan != layout.scan {
                return Err("sealed access omits or changes a physical owned edge".into());
            }
        }
        let mut pending = vec![self.root_layout];
        pending.extend(self.projection.layouts());
        let mut reached = BTreeSet::new();
        let index = crate::LayoutIndex::new(table);
        while let Some(id) = pending.pop() {
            if !reached.insert(id) {
                continue;
            }
            let row = table.layout(id).ok_or("foreign sealed physical layout")?;
            if row.finalizer.is_some()
                || matches!(
                    row.kind,
                    LayoutKind::PayloadReceipt { .. }
                        | LayoutKind::Resource
                        | LayoutKind::Arena
                        | LayoutKind::Handle
                        | LayoutKind::Iterator { .. }
                )
            {
                return Err("sealed descriptor contains a resource or arena capability".into());
            }
            if !programs.contains_key(&id) {
                return Err("sealed descriptor omits a physical layout".into());
            }
            // A nested Shared is an ownership boundary. Its independently
            // declared value graph remains logical metadata, not a relocation.
            if matches!(row.kind, LayoutKind::Shared { .. }) {
                continue;
            }
            pending.extend(row.references());
            pending.extend(index.implicit(id));
        }
        if programs.keys().any(|id| !reached.contains(id)) {
            return Err("unassociated sealed physical access program".into());
        }
        Ok(())
    }

    pub fn layouts(&self) -> impl Iterator<Item = LayoutId> + '_ {
        std::iter::once(self.root_layout)
            .chain(self.projection.nodes.iter().flat_map(|node| {
                // Use the codec's exhaustive reference walk, including the
                // separately named storage of a Text node. Raw receipts must
                // name it even when an untrusted access table omits it.
                let mut node = node.clone();
                node.layouts_mut()
                    .into_iter()
                    .map(|id| *id)
                    .collect::<Vec<_>>()
            }))
            .chain(self.access.iter().map(|p| p.layout))
    }

    /// Source-instance reconstruction includes independently closed environment
    /// alternatives. Candidate case symbols and layouts never authorize them.
    pub fn validate_at(
        &self,
        declarations: &dyn crate::operation::OperationDeclarations,
        definition: &hirsel_syntax::DefPath,
        instance: hirsel_syntax::ContentHash,
        table: &LayoutTable,
        target: &crate::TargetSpec,
    ) -> Result<(), String> {
        self.check(table)?;
        let expected = declarations
            .seal_descriptor_for_source(definition, instance, &self.root_type, table, target)
            .map_err(|e| format!("sealed source reconstruction: {e:?}"))?;
        if &expected != self {
            return Err("sealed descriptor disagrees with independent source".into());
        }
        Ok(())
    }

    /// Check the complete association against the declaration provider, rather
    /// than accepting matching producer-supplied logical and physical claims.
    pub fn validate(
        &self,
        declarations: &dyn crate::operation::OperationDeclarations,
        table: &LayoutTable,
        target: &crate::TargetSpec,
    ) -> Result<(), String> {
        self.check(table)?;
        let expected = declarations
            .seal_descriptor(&self.root_type, table, target)
            .map_err(|e| format!("sealed declaration reconstruction: {e:?}"))?;
        if &expected != self {
            return Err("sealed descriptor disagrees with independent declarations".into());
        }
        Ok(())
    }
}

/// Runtime control lifecycle. Prepare is private; only a complete commit
/// publishes an affine owner, and every loan retires before Dropping.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum SealedState {
    Preparing,
    Published,
    Dropping,
    Dead,
}

snapshot_struct!(StorageAccess { layout, scan });
snapshot_struct!(SealDescriptor {
    root_type,
    root_layout,
    logical,
    projection,
    access
});
snapshot_enum!(SealedState {
    Preparing,
    Published,
    Dropping,
    Dead
});

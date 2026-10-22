//! The linker's immutable descriptor-index assignment. A single instance is passed to
//! every emission partition; it binds the complete layout table, never the selected units.
use crate::{LayoutId, LayoutTable};
use hirsel_syntax::ContentHash;
use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DescriptorRegistry {
    pub table: ContentHash,
    pub target: ContentHash,
    pub identity: ContentHash,
    /// Index three onwards; index two is the target's empty environment.
    pub layouts: Vec<LayoutId>,
    /// Derived once from the complete frozen table, shared by all partitions.
    pub map_storage: crate::logical_storage::MapStorage,
    /// Every executable layout, including equal rows sharing one descriptor.
    indices: BTreeMap<LayoutId, u32>,
}

impl DescriptorRegistry {
    /// Called by the driver before partitioning. Canonical descriptor digests order
    /// the complete linked table; layouts without a selected finalizer are not executable.
    pub fn new(table: &LayoutTable, finalizers: &BTreeMap<LayoutId, ContentHash>) -> Self {
        let mut rows: Vec<_> = table
            .layouts
            .iter()
            .enumerate()
            .filter(|(i, r)| r.finalizer.is_none() || finalizers.contains_key(&LayoutId(*i as u32)))
            .map(|(i, r)| {
                (
                    crate::snapshot::digest("runtime-layout-descriptor", r),
                    LayoutId(i as u32),
                )
            })
            .collect();
        rows.sort_by_key(|(digest, id)| (*digest, *id));
        let mut layouts = Vec::new();
        let mut indices = BTreeMap::new();
        let mut equal = std::collections::HashMap::new();
        for (_, id) in rows {
            // Hashes only select candidates: equality covers the entire record and
            // selected finalizer, never just the descriptor's content key.
            let content = (&table.layouts[id.0 as usize], finalizers.get(&id));
            let index = *equal.entry(content).or_insert_with(|| {
                let index = layouts.len() as u32 + 3;
                layouts.push(id);
                index
            });
            indices.insert(id, index);
        }
        let identity = registry_identity(table.digest(), table.target, &layouts, finalizers);
        Self {
            table: table.digest(),
            target: table.target,
            identity,
            layouts,
            map_storage: crate::logical_storage::MapStorage::new(table),
            indices,
        }
    }
    pub fn index(&self, layout: LayoutId) -> Option<u32> {
        self.indices.get(&layout).copied()
    }
    pub fn external_count(&self) -> u32 {
        self.layouts.len() as u32 + 1
    }
    pub fn validates(
        &self,
        table: &LayoutTable,
        finalizers: &BTreeMap<LayoutId, ContentHash>,
    ) -> bool {
        self.layouts.len() <= (1 << 29) - 3 && *self == Self::new(table, finalizers)
    }
    pub fn symbol(&self) -> String {
        let digest = self.identity;
        format!(
            "hirsel.registry.{}",
            digest
                .0
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect::<String>()
        )
    }
}

fn registry_identity(
    table: ContentHash,
    target: ContentHash,
    layouts: &[LayoutId],
    finalizers: &BTreeMap<LayoutId, ContentHash>,
) -> ContentHash {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&table.0);
    bytes.extend_from_slice(&target.0);
    bytes.extend_from_slice(&(layouts.len() as u32).to_le_bytes());
    for id in layouts {
        bytes.extend_from_slice(&id.0.to_le_bytes());
    }
    bytes.extend_from_slice(&(finalizers.len() as u32).to_le_bytes());
    for (id, instance) in finalizers {
        bytes.extend_from_slice(&id.0.to_le_bytes());
        bytes.extend_from_slice(&instance.0);
    }
    ContentHash::of_parts("hirsel.runtime-registry.v2", [bytes.as_slice()])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout_types::*;

    fn duplicated_table() -> LayoutTable {
        let mut table = LayoutTable::new(&TargetSpec::x86_64_unknown_linux_gnu());
        let record = LayoutRecord {
            key: LayoutIdentity {
                owner_revision: None,
                instance: None,
                target: table.target,
                layout_version: table.layout_version,
            },
            kind: LayoutKind::Scalar(ScalarKind::U64),
            size: SizeSpec::Fixed(8),
            align: 8,
            fields: vec![],
            tag: None,
            initialized: InitializedBytes::Fields,
            scan: vec![],
            drop: vec![],
            copy: CopyRecipe::Bits,
            finalizer: None,
            storage: vec![],
            abi: AbiInputs {
                scalars: vec![(0, ScalarKind::U64)],
                unaligned: false,
            },
        };
        table.layouts = vec![record.clone(), record];
        table
    }

    #[test]
    fn equal_layouts_share_an_index_and_aliases_are_validated() {
        let table = duplicated_table();
        let finalizers = BTreeMap::new();
        let registry = DescriptorRegistry::new(&table, &finalizers);
        assert_eq!(registry.layouts, [LayoutId(0)]);
        assert_eq!(registry.external_count(), 2);
        assert_eq!(registry.index(LayoutId(0)), Some(3));
        assert_eq!(registry.index(LayoutId(1)), Some(3));
        assert_eq!(registry.index(LayoutId(2)), None);
        assert!(registry.validates(&table, &finalizers));
        let mut forged = registry.clone();
        forged.indices.insert(LayoutId(1), 4);
        assert!(!forged.validates(&table, &finalizers));
        let mut forged = registry;
        forged.layouts.push(LayoutId(1));
        assert!(!forged.validates(&table, &finalizers));
    }

    #[test]
    fn different_full_records_keep_distinct_indices() {
        let mut table = duplicated_table();
        table.layouts[1].copy = CopyRecipe::NotCopyable;
        let registry = DescriptorRegistry::new(&table, &BTreeMap::new());
        assert_eq!(registry.layouts.len(), 2);
        assert_ne!(registry.index(LayoutId(0)), registry.index(LayoutId(1)));
    }

    #[test]
    fn different_selected_finalizers_keep_distinct_indices() {
        let mut table = duplicated_table();
        for record in &mut table.layouts {
            record.finalizer = Some(FinalizerRef {
                definition: hirsel_syntax::DefPath::core("drop"),
                body: 0,
                unsafe_ffi_package: false,
            });
        }
        let finalizers = BTreeMap::from([
            (LayoutId(0), ContentHash::of_bytes(b"first")),
            (LayoutId(1), ContentHash::of_bytes(b"second")),
        ]);
        let registry = DescriptorRegistry::new(&table, &finalizers);
        assert_eq!(registry.layouts.len(), 2);
        assert_ne!(registry.index(LayoutId(0)), registry.index(LayoutId(1)));
        let same = ContentHash::of_bytes(b"same");
        let finalizers = BTreeMap::from([(LayoutId(0), same), (LayoutId(1), same)]);
        let registry = DescriptorRegistry::new(&table, &finalizers);
        assert_eq!(registry.index(LayoutId(0)), registry.index(LayoutId(1)));
        let registry = DescriptorRegistry::new(&table, &BTreeMap::new());
        assert!(registry.layouts.is_empty());
        assert_eq!(registry.index(LayoutId(0)), None);
        assert_eq!(registry.index(LayoutId(1)), None);
    }
}

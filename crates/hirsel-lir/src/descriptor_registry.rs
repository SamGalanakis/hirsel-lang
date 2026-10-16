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
        let layouts: Vec<_> = rows.into_iter().map(|(_, id)| id).collect();
        let identity = registry_identity(table.digest(), table.target, &layouts, finalizers);
        Self {
            table: table.digest(),
            target: table.target,
            identity,
            layouts,
        }
    }
    pub fn index(&self, layout: LayoutId) -> Option<u32> {
        self.layouts
            .iter()
            .position(|id| *id == layout)
            .and_then(|n| u32::try_from(n).ok())
            .map(|n| n + 3)
    }
    pub fn external_count(&self) -> u32 {
        self.layouts.len() as u32 + 1
    }
    pub fn validates(
        &self,
        table: &LayoutTable,
        finalizers: &BTreeMap<LayoutId, ContentHash>,
    ) -> bool {
        if self.table != table.digest()
            || self.target != table.target
            || self.layouts.len() > (1 << 29) - 3
        {
            return false;
        }
        let mut seen = std::collections::BTreeSet::new();
        let mut previous = None;
        for &id in &self.layouts {
            let Some(record) = table.layout(id) else {
                return false;
            };
            if record.finalizer.is_some() && !finalizers.contains_key(&id) {
                return false;
            }
            let key = (
                crate::snapshot::digest("runtime-layout-descriptor", record),
                id,
            );
            if previous.is_some_and(|previous| previous >= key) || !seen.insert(id) {
                return false;
            }
            previous = Some(key);
        }
        for (i, record) in table.layouts.iter().enumerate() {
            let id = LayoutId(i as u32);
            if (record.finalizer.is_none() || finalizers.contains_key(&id)) != seen.contains(&id) {
                return false;
            }
        }
        self.identity == registry_identity(self.table, self.target, &self.layouts, finalizers)
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

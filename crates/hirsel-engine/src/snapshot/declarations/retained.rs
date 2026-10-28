//! Complete declaration/manifest metadata for source-state carry. This is
//! independent of source text and type interner ownership. Shared rows are
//! counted per occurrence; inspection creates no lazy sites or frozen inputs.
use super::*;
use crate::NativeMetadataWeight as Weight;
use std::collections::BTreeSet;

fn package(weight: &mut Weight, value: &PackageId) {
    weight.add(value.name.capacity());
    weight.add(value.version.capacity());
}
fn strings(weight: &mut Weight, values: &Vec<String>) {
    weight.vec(values, |w, value| w.add(value.capacity()));
}
fn exposure(weight: &mut Weight, value: &DependencyExposure) {
    weight.add(value.head.capacity());
    package(weight, &value.package);
    strings(weight, &value.modules);
}
fn metadata(weight: &mut Weight, value: &Metadata) {
    let Metadata {
        definitions,
        declarations,
        unsafe_ffi,
        unsafe_capability,
        result_type,
        tasks_capability,
        scope_capability,
        heap,
        nominal_role_cache,
    } = value;
    weight.map(definitions, |w, path, _| w.path(path));
    weight.vec(declarations, |w, site| w.path(&site.definition));
    weight.map(unsafe_ffi, |w, key, value| {
        package(w, key);
        w.map(&value.wrappers, |w, path, _| w.path(path));
    });
    for path in [
        unsafe_capability,
        result_type,
        tasks_capability,
        scope_capability,
    ]
    .into_iter()
    .flatten()
    {
        weight.path(path);
    }
    if let Some(heap) = heap {
        weight.path(&heap.arena);
        weight.path(&heap.operation);
    }
    if let Some(roles) = nominal_role_cache.get() {
        weight.arc(roles);
        for path in [
            &roles.unsafe_capability,
            &roles.result_type,
            &roles.tasks_capability,
            &roles.scope_capability,
        ]
        .into_iter()
        .flatten()
        {
            weight.path(path);
        }
        if let Some(heap) = &roles.heap {
            weight.path(&heap.arena);
            weight.path(&heap.operation);
        }
    }
}
impl ModuleInputs {
    /// All owned manifest/declaration input buffers, excluding the inline value.
    /// This does not freeze the index, resolve a name or retain a source body.
    pub fn retained_payload_bytes(&self) -> u64 {
        let mut weight = Weight::default();
        let Self {
            exposures,
            package_exposures,
            package_dependencies,
            signature_metadata,
            manifest_revision: _,
        } = self;
        weight.vec(exposures, exposure);
        weight.map(package_exposures, |w, key, values| {
            package(w, key);
            w.vec(values, exposure);
        });
        weight.map(package_dependencies, |w, key, values| {
            package(w, key);
            w.vec(values, package);
        });
        metadata(&mut weight, signature_metadata);
        weight.bytes()
    }
}
fn row(weight: &mut Weight, value: &Arc<Row>) {
    weight.arc(value);
    weight.flat_vec(&value.site_spans);
    weight.flat_vec(&value.headers);
    if let Some(sites) = value.sites.get() {
        weight.add(crate::memo::retained_arc_bytes(&**sites) as usize);
        for site in &**sites {
            weight.path(&site.definition);
        }
    }
}
fn frozen(weight: &mut Weight, value: &Arc<FrozenRows>) {
    weight.arc(value);
    weight.add(value.index.retained_payload_bytes() as usize);
    weight.map(&value.package_indexes, |w, key, value| {
        package(w, key);
        w.add(value.retained_payload_bytes() as usize);
    });
    metadata(weight, &value.metadata);
    weight.map(&value.sites, |w, _, value| row(w, value));
}
fn hash<K, V>(weight: &mut Weight, capacity: usize) {
    // hashbrown capacity_to_buckets: even small maps have <=2 buckets per
    // usable slot. Include controls, group and alignment for the raw allocation.
    if capacity != 0 {
        let layout = std::alloc::Layout::new::<(K, V)>();
        weight.add(
            capacity
                .saturating_mul(2)
                .saturating_mul(layout.size().saturating_add(1))
                .saturating_add(16)
                .saturating_add(layout.align()),
        );
    }
}
fn files(weight: &mut Weight, values: &BTreeMap<FileId, Arc<[Arc<ItemSyntax>]>>) {
    weight.map(values, |w, _, items| {
        w.add(crate::memo::retained_arc_bytes(&**items) as usize);
        for syntax in &**items {
            w.add(crate::memo::retained::retained_syntax_bytes(syntax) as usize);
        }
    });
}
impl Declarations {
    /// Complete retained declaration backing. An opaque archived-siblings
    /// provider needs its own owner; refuse rather than omitting that payload.
    pub fn retained_payload_bytes(&self) -> Option<u64> {
        if self.archived_siblings.is_some() {
            return None;
        }
        let mut weight = Weight::default();
        frozen(&mut weight, &self.rows);
        files(&mut weight, &self.files);
        hash::<Anchor, FileId>(&mut weight, self.anchors.capacity());
        Some(weight.bytes())
    }
}
impl DeclarationIndex {
    pub(crate) fn source_carry_bytes(&self) -> u64 {
        let mut weight = Weight::default();
        weight.map(&self.rows, |w, _, value| {
            w.arc(value);
            w.add(value.site_spans.len() * size_of::<Span>());
            w.add(value.headers.len() * size_of::<ContentHash>());
        });
        hash::<std::path::PathBuf, BTreeSet<FileId>>(&mut weight, self.paths.capacity());
        for (path, values) in &self.paths {
            weight.add(path.capacity());
            weight.tree::<FileId, ()>(values.len());
        }
        weight.add(self.inputs.retained_payload_bytes() as usize);
        weight.bytes()
    }
    /// Complete existing declaration/index backing, including already frozen
    /// rows and materialized sites. Inspection performs no lazy initialization.
    /// Source text and type/session interners remain separate components.
    pub fn retained_payload_bytes(&self) -> Option<u64> {
        let mut weight = Weight::default();
        weight.map(&self.rows, |w, _, value| row(w, value));
        files(&mut weight, &self.files);
        hash::<Anchor, FileId>(&mut weight, self.anchors.capacity());
        hash::<std::path::PathBuf, BTreeSet<FileId>>(&mut weight, self.paths.capacity());
        for (path, values) in &self.paths {
            weight.add(path.capacity());
            weight.tree::<FileId, ()>(values.len());
        }
        weight.add(self.inputs.retained_payload_bytes() as usize);
        if let Some(value) = self.frozen_rows.get() {
            frozen(&mut weight, value);
        }
        if let Some(value) = self.frozen.get() {
            weight.arc(value);
            weight.add(value.retained_payload_bytes()? as usize);
        }
        Some(weight.bytes())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_projection_releases_derived_sites_with_a_complete_allocation_bound() {
        crate::with_compiler_stack(|| {
            for width in [1, 4, 16] {
                let paths: Vec<_> = (0..width)
                    .map(|i| DefPath::core(&format!("carry_row_{i}")))
                    .collect();
                for path in &paths {
                    DefId::intern(path);
                }
                let ((carried, predicted), actual) = crate::test_alloc::measured(|| {
                    let mut index = DeclarationIndex::default();
                    for (i, path) in paths.iter().enumerate() {
                        let span = Span {
                            anchor: Anchor(i as u32 + 1),
                            start: 0,
                            end: 1,
                        };
                        let mut sites = Vec::with_capacity(64);
                        sites.push(span);
                        let mut headers = Vec::with_capacity(64);
                        headers.push(ContentHash::default());
                        let row = Arc::new(Row::from(DeclarationRow {
                            definition: DefId::intern(path),
                            types: true,
                            visibility: Visibility::Public,
                            span,
                            metadata: DefinitionMetadata {
                                public: true,
                                alias: false,
                            },
                            site_spans: sites,
                            headers,
                            candidates: ContentHash::default(),
                            identity: [0; 4],
                        }));
                        let _ = row.sites();
                        index.rows.insert(DefId::intern(path), row);
                    }
                    let (predicted, inspection) =
                        crate::test_alloc::measured_independent(|| index.source_carry_bytes());
                    assert_eq!(inspection.allocations, 0);
                    (index.into_source_carry(), predicted)
                });
                assert!(
                    actual.live as u64 <= predicted,
                    "{} > {predicted}",
                    actual.live
                );
                for path in &paths {
                    let row = &carried.rows[&DefId::intern(path)];
                    assert_eq!(row.definition.path(), path);
                    assert_eq!(row.site_spans.len(), 1);
                    assert!(row.sites.get().is_none());
                }
                let (_, released) = crate::test_alloc::measured(|| drop(carried));
                assert_eq!(-released.live, actual.live);
            }
        });
    }

    #[test]
    fn declaration_backing_bound_covers_manifest_frozen_rows_sites_and_last_clone() {
        crate::with_compiler_stack(|| {
            // Registry and interned identities are process-owned, outside the
            // source-index allocation whose actual release is measured below.
            drop(DeclarationIndex::default().frozen());
            let package = PackageId {
                name: "source_index".into(),
                version: "1".into(),
            };
            for width in [1, 4, 16] {
                let paths: Vec<_> = (0..width)
                    .map(|i| {
                        DefPath(hirsel_syntax::ItemPath {
                            package: package.clone(),
                            modules: vec![format!("module_{i}")],
                            name: format!("item_{i}"),
                        })
                    })
                    .collect();
                for path in &paths {
                    DefId::intern(path);
                }
                let ((index, before), actual) = crate::test_alloc::measured(|| {
                    let mut index = DeclarationIndex::default();
                    for (i, path) in paths.iter().enumerate() {
                        let span = Span {
                            anchor: Anchor(i as u32),
                            start: 0,
                            end: 1,
                        };
                        let row = DeclarationRow {
                            definition: DefId::intern(path),
                            types: true,
                            visibility: Visibility::Public,
                            span,
                            metadata: DefinitionMetadata {
                                public: true,
                                alias: false,
                            },
                            site_spans: vec![span; width],
                            headers: vec![ContentHash::default(); width],
                            candidates: Default::default(),
                            identity: [0; 4],
                        };
                        index
                            .rows
                            .insert(DefId::intern(path), Arc::new(Row::from(row)));
                        index.anchors.insert(span.anchor, FileId(i as u32));
                        index.paths.insert(
                            std::path::PathBuf::from(format!("source_{i}.hir")),
                            [FileId(i as u32)].into(),
                        );
                        index.inputs.signature_metadata.definitions.insert(
                            path.clone(),
                            DefinitionMetadata {
                                public: true,
                                alias: false,
                            },
                        );
                        index
                            .inputs
                            .signature_metadata
                            .declarations
                            .push(DeclarationSite {
                                definition: path.clone(),
                                name_span: span,
                            });
                        index.inputs.exposures.push(DependencyExposure {
                            head: format!("head_{i}"),
                            package: package.clone(),
                            modules: path.0.modules.clone(),
                        });
                    }
                    index
                        .inputs
                        .package_exposures
                        .insert(package.clone(), index.inputs.exposures.clone());
                    index
                        .inputs
                        .package_dependencies
                        .insert(package.clone(), vec![package.clone(); width]);
                    let (before, inspection) = crate::test_alloc::measured_independent(|| {
                        index.retained_payload_bytes().unwrap()
                    });
                    assert_eq!(inspection.allocations, 0);
                    assert!(index.frozen.get().is_none() && index.frozen_rows.get().is_none());
                    let frozen = index.frozen();
                    for path in &paths {
                        drop(frozen.sites(path));
                    }
                    drop(frozen);
                    (Arc::new(index), before)
                });
                let (bound, inspection) =
                    crate::test_alloc::measured(|| index.retained_payload_bytes().unwrap());
                assert_eq!(inspection.allocations, 0);
                assert!(
                    bound > before,
                    "frozen rows and site payload must be included"
                );
                let bound = bound + crate::memo::retained_arc_bytes(&*index);
                assert!(
                    actual.live as u64 <= bound,
                    "width {width}: actual {} exceeds {bound}",
                    actual.live
                );
                let clone = index.clone();
                let (_, release) = crate::test_alloc::measured(|| drop(index));
                assert_eq!(release.live, 0);
                let (_, release) = crate::test_alloc::measured(|| drop(clone));
                assert_eq!(-release.live, actual.live);
            }
        });
    }
}

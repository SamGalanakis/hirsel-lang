//! Source signature memo backing, including syntax-backed phase products.
//! These are payload components; source maps, interners and table buckets have
//! separate owners. Repeated shared occurrences are charged conservatively.
use super::{Weight, arc_bytes, retained_signature_payload_bytes};
use hirsel_types::{ContextualName, Resolution, ResolvedName};

impl Weight<'_> {
    fn resolution(&mut self, value: &Resolution) {
        let Resolution {
            reads,
            contextual,
            paths,
            local_functions,
            drop_binding: _,
            deferred,
            generics,
            generic_declarations,
            reference_scopes,
            expressions,
            types,
            bindings,
            dependencies,
            static_arguments,
            implementation_binders,
        } = value;
        self.read_log(reads);
        self.vec(contextual, |w, (_, name)| match name {
            ContextualName::Constructor(path)
            | ContextualName::Variant { owner: path, .. }
            | ContextualName::Field { owner: path, .. } => w.def_path(path),
        });
        let name = |w: &mut Self, name: &ResolvedName| match name {
            ResolvedName::Definition(path) => w.def_path(path),
            ResolvedName::Local(_)
            | ResolvedName::Parameter(_)
            | ResolvedName::Variant(_)
            | ResolvedName::Size(_)
            | ResolvedName::TypeParameter(_)
            | ResolvedName::Primitive(_) => (),
        };
        self.vec(paths, |w, (_, value)| name(w, value));
        self.flat_vec(local_functions);
        self.vec(deferred, |w, value| w.add(value.name.capacity()));
        self.flat_vec(generics);
        generic_declarations.visit_retained_allocations(|_, bytes| {
            self.add(bytes);
            true
        });
        self.flat_vec(reference_scopes);
        self.vec(expressions, |w, (_, value)| name(w, value));
        self.vec(types, |w, (_, value)| name(w, value));
        self.flat_vec(bindings);
        self.vec(dependencies, Self::def_path);
        self.vec(static_arguments, |w, (_, arguments)| w.flat_vec(arguments));
        self.vec(implementation_binders, |w, binder| {
            w.add(binder.name.capacity());
            w.vec(&binder.members, |w, (name, _)| w.add(name.capacity()));
        });
    }
    fn resolver_snapshot(&mut self, value: &crate::ResolverSnapshot) {
        self.add(arc_bytes(&*value.syntax));
        self.syntax(&value.syntax);
        self.resolution(&value.output.value);
        self.vec(&value.output.diagnostics, Self::diagnostic);
        self.flat_vec(&value.output.dependencies);
    }
}

impl crate::ResolverSnapshot {
    /// All owned resolver payload, including its anchored syntax. Inspection
    /// allocates nothing and cannot perform source or provider lookups.
    pub fn retained_payload_bytes(&self) -> u64 {
        let mut weight = Weight {
            count_shared_repeats: true,
            ..Default::default()
        };
        weight.resolver_snapshot(self);
        weight.bytes
    }
}

pub(crate) fn retained_signature_entry_bytes(
    entry: &crate::queries::Entry<hirsel_types::SignatureRef>,
) -> Option<u64> {
    if entry.products.mode_archive.is_some() || entry.verification.lock().unwrap().is_some() {
        return None;
    }
    signature_entry_bytes(entry, None, None)
}

pub(crate) struct SourceProjection {
    pub(crate) compact: bool,
    pub(crate) detail: bool,
    pub(crate) compact_output: bool,
    pub(crate) compact_reads: bool,
    pub(crate) shared_products: bool,
}
pub(crate) fn source_projection(
    entry: &std::sync::Arc<crate::queries::Entry<hirsel_types::SignatureRef>>,
) -> SourceProjection {
    // Entry weak references belong to the engine's disposable snapshot_sources.
    // They retain no payload and cannot escape the engine. Public output
    // owners still require both strong and weak uniqueness below. A carried
    // exact read slice can detach without mutating any existing read owner.
    let compact = std::sync::Arc::strong_count(entry) == 1;
    let detail = &entry.detail;
    let detail = detail.signatures.capacity() != 0
        || detail.failed_signatures.capacity() != 0
        || detail.failed_signature_keys.capacity() != 0
        || detail.lookups.capacity() != 0
        || detail.manifest_lookups.capacity() != 0
        || detail.resolver.node_identity().is_some()
        || detail.citations.capacity() != 0;
    let compact_reads = entry.reads().flat_backing_bytes().is_some();
    let projected = compact || detail || compact_reads || entry.products.resolution.is_some();
    SourceProjection {
        compact,
        detail,
        compact_output: compact
            && entry.memo.value.as_ref().is_ok_and(|output| {
                std::sync::Arc::strong_count(output) == 1 && std::sync::Arc::weak_count(output) == 0
            }),
        compact_reads,
        shared_products: projected
            && entry.products.representation.is_none()
            && entry.products.mode_archive.is_none()
            && entry.products.mode_diagnostics.is_empty(),
    }
}
pub(crate) fn source_shared_products(
    entry: &std::sync::Arc<crate::queries::Entry<hirsel_types::SignatureRef>>,
) -> bool {
    source_projection(entry).shared_products
}
pub(crate) fn source_shared_products_bytes() -> u64 {
    std::alloc::Layout::new::<[usize; 2]>()
        .extend(std::alloc::Layout::new::<crate::PhaseProducts>())
        .expect("allocated Arc layout")
        .0
        .pad_to_align()
        .size() as u64
}

pub(crate) fn source_signature_head_bytes(
    entry: &std::sync::Arc<crate::queries::Entry<hirsel_types::SignatureRef>>,
    names: &mut dyn super::NameAllocationSet,
) -> Option<u64> {
    if entry.products.mode_archive.is_some() || entry.verification.lock().unwrap().is_some() {
        return None;
    }
    signature_entry_bytes(entry, Some(source_projection(entry)), Some(names))
}

/// Diagnostic components of the same conservative source-head bound.
pub(crate) fn source_signature_head_components(
    entry: &crate::queries::Entry<hirsel_types::SignatureRef>,
) -> [u64; 4] {
    let mut names = super::NameAllocations::<64>::default();
    let mut weight = Weight {
        count_shared_repeats: true,
        source_names: Some(&mut names),
        ..Default::default()
    };
    weight.reads(&entry.memo.reads);
    let reads = weight.bytes;
    weight.add(arc_bytes(&*entry.detail.resolver));
    if let Some(failure) = &entry.detail.operational_failure {
        weight.failure(failure);
    }
    let detail = weight.bytes - reads;
    let mut value = 0;
    let mut dependencies =
        (entry.covered.capacity() * size_of::<hirsel_types::SignatureDependency>()) as u64;
    if let Ok(output) = &entry.memo.value {
        value = retained_signature_payload_bytes(&output.value);
        dependencies += (output.dependencies.capacity()
            * size_of::<hirsel_types::SignatureDependency>()) as u64;
    }
    [reads, detail, value, dependencies]
}

fn signature_entry_bytes(
    entry: &crate::queries::Entry<hirsel_types::SignatureRef>,
    projection: Option<SourceProjection>,
    names: Option<&mut dyn super::NameAllocationSet>,
) -> Option<u64> {
    let head = projection.is_some();
    let projected = projection.as_ref().is_some_and(|p| {
        p.compact
            || p.detail
            || p.compact_reads
            || p.shared_products
            || entry.products.resolution.is_some()
    });
    let compact_output = projection.as_ref().is_some_and(|p| p.compact_output);
    let compact_reads = projection.as_ref().is_some_and(|p| p.compact_reads);
    let shared_products = projection.as_ref().is_some_and(|p| p.shared_products);
    let mut weight = Weight {
        count_shared_repeats: true,
        source_names: names,
        ..Default::default()
    };
    weight.add(arc_bytes(entry));
    if compact_reads {
        weight.add(arc_bytes(&*entry.memo.reads));
        weight.add(entry.reads().flat_backing_bytes()?);
    } else {
        weight.reads(&entry.memo.reads);
    }
    weight.cloned_vectors = projected;
    if head {
        // The exact typed read receipt is authoritative. A carried head drops
        // the normalizer's duplicate resolver/path observations, retaining only
        // an operational refusal and the empty resolver Arc's allocation.
        weight.add(arc_bytes(&*entry.detail.resolver));
        if let Some(failure) = &entry.detail.operational_failure {
            weight.failure(failure);
        }
    } else {
        weight.detail(&entry.detail);
    }
    weight.flat_vec(&entry.covered);
    if let Some(depth) = &entry.depth {
        weight.add(size_of_val(&*depth.first));
    }
    weight.vec(&entry.selections, |w, selection| {
        if let crate::comptime::Selection::Staged(_, arguments) = selection {
            w.flat_vec(arguments);
        }
    });
    weight.cloned_vectors = false;
    if let Some(receipt) = &entry.acceptance {
        weight.bytes = weight.bytes.checked_add(receipt.retained_bytes()?)?;
    }
    if !shared_products {
        weight.add(arc_bytes(&*entry.products));
    }
    if let Some(resolution) = &entry.products.resolution
        && !head
    {
        weight.resolver_snapshot(resolution);
    }
    if let Some(owner) = &entry.products.representation {
        weight.type_definition(&owner.representation.declaration);
        weight.flat_vec(&owner.dependencies);
        if let Some((view, _)) = owner.view.get() {
            weight.bytes = weight
                .bytes
                .checked_add(retained_signature_payload_bytes(view))?;
        }
    }
    weight.vec(
        &entry.products.mode_diagnostics,
        |w, (diagnostic, phase)| {
            w.diagnostic(diagnostic);
            w.add(phase.capacity());
        },
    );
    match &entry.memo.value {
        Ok(output) => {
            weight.add(arc_bytes(&**output));
            weight.bytes = weight
                .bytes
                .checked_add(retained_signature_payload_bytes(&output.value))?;
            weight.cloned_vectors = compact_output;
            weight.vec(&output.diagnostics, Weight::diagnostic);
            weight.flat_vec(&output.dependencies);
            weight.cloned_vectors = false;
        }
        Err(error) => weight.error(error),
    }
    Some(weight.bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{PhaseProducts, QueryKind, ResolverSnapshot, queries::Entry};
    use hirsel_syntax::{
        ContentHash, DefId, DefPath, ExprId, Item, ItemKind, ItemSyntax, Origin, SyntaxTree,
        TypeExprId, Visibility,
    };
    use hirsel_types::*;
    use std::sync::{Arc, Mutex, OnceLock};

    fn parameters(width: usize) -> Vec<GenericParameter> {
        (0..width)
            .map(|i| GenericParameter::Type {
                name: format!("generic_parameter_{i}"),
                bounds: vec![Bound::Copyable; width].into(),
            })
            .collect()
    }
    fn snapshot(width: usize, path: &DefPath, symbol: hirsel_intern::Symbol) -> ResolverSnapshot {
        let span = hirsel_span::Span::default();
        let mut resolution = Resolution {
            contextual: (0..width)
                .map(|_| (NameSite::Item, ContextualName::Constructor(path.clone())))
                .collect(),
            paths: (0..width)
                .map(|_| (NameSite::Item, ResolvedName::Definition(path.clone())))
                .collect(),
            local_functions: Vec::with_capacity(width),
            deferred: (0..width)
                .map(|i| DeferredName {
                    site: NameSite::Item,
                    span,
                    name: format!("missing_name_{i}"),
                })
                .collect(),
            generics: Vec::with_capacity(width),
            generic_declarations: GenericDeclarations::signature(&parameters(width)),
            reference_scopes: Vec::with_capacity(width),
            expressions: (0..width)
                .map(|i| (ExprId(i as u32), ResolvedName::Definition(path.clone())))
                .collect(),
            types: (0..width)
                .map(|i| (TypeExprId(i as u32), ResolvedName::Definition(path.clone())))
                .collect(),
            bindings: Vec::with_capacity(width),
            dependencies: vec![path.clone(); width],
            static_arguments: (0..width)
                .map(|i| {
                    (
                        ExprId(i as u32),
                        vec![
                            (
                                0,
                                StaticArgument::Actual(GenericArgument::Size(Size::Constant(3)))
                            );
                            width
                        ],
                    )
                })
                .collect(),
            implementation_binders: (0..width)
                .map(|i| ImplementationBinder {
                    name: format!("implementation_{i}"),
                    family: DefId::intern(path),
                    members: (0..width)
                        .map(|j| (format!("member_{j}"), j as u32))
                        .collect(),
                })
                .collect(),
            ..Default::default()
        };
        let reads = Arc::make_mut(&mut resolution.reads);
        reads.push(
            DependencyRead::ModuleHead {
                context: (hirsel_syntax::FileContext {
                    package: path.0.package.clone(),
                    modules: path.0.modules.clone(),
                })
                .into(),
                head: "missing_module".into(),
                manifest: Default::default(),
                exposed: false,
            }
            .shared(),
        );
        ResolverSnapshot {
            syntax: Arc::new(ItemSyntax {
                item: Item {
                    name_span: span,
                    origin: Origin {
                        span,
                        tokens: Default::default(),
                    },
                    name: symbol,
                    definition: path.clone(),
                    visibility: Visibility::Public,
                    attributes: vec![],
                    kind: ItemKind::Const {
                        ty: TypeExprId(0),
                        value: ExprId(0),
                    },
                    generated: None,
                },
                tree: SyntaxTree {
                    tokens: Vec::with_capacity(width),
                    ..Default::default()
                },
                anchor: hirsel_span::Anchor(0),
                text_hash: Default::default(),
                occurrence: 0,
            }),
            output: QueryOutput {
                value: resolution,
                diagnostics: vec![],
                dependencies: Vec::with_capacity(width),
            },
        }
    }
    fn declaration(width: usize, path: &DefPath) -> TypeDefinition {
        TypeDefinition {
            loop_source: None,
            format: Some(path.clone()),
            generics: parameters(width),
            representation: Some(TypeId::DANGLING),
            bounds: vec![Bound::Copyable; width],
            authority: AuthoritySummary::inert(),
            capability: None,
            implementation_authority: None,
            serializable: false,
            serializable_check: None,
            layout: Default::default(),
            distinct: false,
            resource: false,
            drop: Some(path.clone()),
            fact: None,
            captures: Some(vec![0; width].into()),
            destructor_allocation: Allocation::NoAlloc,
        }
    }
    #[test]
    fn source_signature_entry_bound_covers_resolver_owner_and_last_clone_without_allocating() {
        let path = DefPath::core("source.entry.payload");
        let owner = DefId::intern(&path);
        let symbol = hirsel_intern::Symbol::intern("entry");
        for width in [1, 4, 16] {
            // Warm only the process-owned weak index. Its strong name payload
            // is still created and released inside the measured region.
            drop(snapshot(width, &path, symbol));
            let mut graph = crate::memo::MemoGraph::default();
            let base = graph.adopt_value(
                crate::memo::MemoKey::new(QueryKind::Signature, owner),
                ContentHash::default(),
                Ok(Arc::new(())),
                Arc::default(),
                crate::memo::Durability::Low,
            );
            let ((entry, actual_snapshot), actual) = crate::test_alloc::measured(|| {
                let resolver = snapshot(width, &path, symbol);
                let (resolver_bound, inspection) =
                    crate::test_alloc::measured_independent(|| resolver.retained_payload_bytes());
                assert_eq!(inspection.allocations, 0);
                let view = Arc::new(Signature {
                    metadata: Default::default(),
                    definition: owner,
                    kind: SignatureKind::Type(declaration(width, &path)),
                    fingerprint: Default::default(),
                });
                let record = crate::representation::OwnerRecord {
                    text_hash: Default::default(),
                    representation: hirsel_check_protocol::header::OwnerRepresentation {
                        declaration: declaration(width, &path),
                    },
                    dependencies: Vec::with_capacity(width),
                    view: OnceLock::from((view, ContentHash::default())),
                };
                let signature = Arc::new(Signature {
                    metadata: Default::default(),
                    definition: owner,
                    kind: SignatureKind::Const {
                        generics: parameters(width).into(),
                        ty: TypeId::DANGLING,
                    },
                    fingerprint: Default::default(),
                });
                let memo = base.map_value(|_| QueryOutput {
                    value: signature,
                    diagnostics: vec![],
                    dependencies: Vec::with_capacity(width),
                });
                let entry = Arc::new(Entry {
                    memo,
                    detail: Default::default(),
                    products: Arc::new(PhaseProducts {
                        resolution: Some(resolver),
                        representation: Some(record),
                        ..Default::default()
                    }),
                    covered: Vec::with_capacity(width),
                    depth: Some(crate::memo::DepthEvidence::leaf(4)),
                    acceptance: None,
                    verification: Mutex::new(None),
                    selections: vec![],
                    consistent: true,
                });
                (entry, resolver_bound)
            });
            let (bound, inspection) =
                crate::test_alloc::measured(|| retained_signature_entry_bytes(&entry).unwrap());
            assert_eq!(inspection.allocations, 0);
            assert!(
                actual.live as u64 <= bound,
                "width {width}: actual {} exceeds {bound}",
                actual.live
            );
            assert!(bound > actual_snapshot);
            let clone = entry.clone();
            let (_, release) = crate::test_alloc::measured(|| drop(entry));
            assert_eq!(release.live, 0);
            let (_, release) = crate::test_alloc::measured(|| drop(clone));
            assert_eq!(-release.live, actual.live);
        }
    }
}

//! Source-only carry across admitted requests. Move original anchors, syntax,
//! header memos and reverse reads; never retain a LoadedProgram, body memo or
//! an S1 coordinator. A fresh engine supplies the current adapters and policy.
use super::*;
use crate::runtime::MemoryReservation;

pub struct SourceIndexState {
    sources: SourceMap,
    indexed_sources: BTreeMap<FileId, (FileContext, QueryEnvironment)>,
    items: BTreeMap<DefPath, Vec<Arc<ItemSyntax>>>,
    builtins: BTreeMap<DefPath, hirsel_types::SignatureRef>,
    syntax_keys: BTreeMap<(DefPath, QueryKind), ContentHash>,
    signatures: Table<hirsel_types::SignatureRef>,
    graph: MemoGraph,
    front_changes: frontier::FrontTracker,
    inputs: hirsel_syntax::DefMap<memos::DefInputs>,
    environment_seen: Option<(QueryEnvironment, crate::ComptimeLimits, Option<ContentHash>)>,
    published_manifest: Option<ContentHash>,
    vanished: BTreeSet<DefId>,
    declarations: crate::DeclarationIndex,
    package_manifests: BTreeMap<hirsel_syntax::PackageId, ContentHash>,
    types: Arc<hirsel_types::TypeInterner>,
    catalog: catalog::Catalog,
    compiler: ContentHash,
    snapshot_epoch: crate::Revision,
    snapshot_inputs_revision: u64,
    // The SourceMap also owns this token, including its detached output clones.
    allowance: Arc<MemoryReservation>,
}
fn invalid() -> QueryError {
    QueryError::Operational(hirsel_types::OperationalFailure {
        operation: "source index carry".into(),
        message: "source index exceeds admission, has an active transaction, or needs generated namespace carry".into(),
    })
}
fn hash<K, V>(weight: &mut crate::NativeMetadataWeight, capacity: usize) {
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
impl QueryEngine {
    /// Inspect the complete source-only carry representation. This performs no
    /// source/provider access or query. Shared core TypeBase data remains with
    /// the original process owner; custom bases refuse until independently owned.
    /// Unfinished transactions and generated namespaces refuse this S0 slice.
    pub fn source_index_bytes(&self) -> Result<u64, QueryError> {
        if !self.s1.is_empty()
            || !self.active.is_empty()
            || !self.signature_stack.is_empty()
            || !self.execution_stack.is_empty()
            || !self.generated.is_empty()
            || !self.staged_constants.is_empty()
            || !self.staged_values.is_empty()
        {
            return Err(invalid());
        }
        if self
            .types
            .base()
            .is_some_and(|base| !Arc::ptr_eq(base, &crate::registry::base()))
        {
            return Err(invalid());
        }
        let mut weight = crate::NativeMetadataWeight::default();
        weight.add(
            size_of::<SourceIndexState>() + 4 * size_of::<usize>() + size_of::<MemoryReservation>(),
        );
        hash::<DefId, Arc<Entry<hirsel_types::SignatureRef>>>(
            &mut weight,
            self.signatures.capacity(),
        );
        hash::<DefId, memos::DefInputs>(&mut weight, self.inputs.capacity());
        for value in self.inputs.values() {
            weight.add(value.retained_bytes());
        }
        weight.tree::<DefId, ()>(self.vanished.len());
        weight.map(&self.indexed_sources, |w, _, (context, environment)| {
            w.add(context.package.name.capacity());
            w.add(context.package.version.capacity());
            w.add(
                context
                    .modules
                    .capacity()
                    .saturating_mul(size_of::<String>()),
            );
            for module in &context.modules {
                w.add(module.capacity());
            }
            w.add(environment.compiler_version.capacity());
        });
        weight.map(&self.syntax_keys, |w, (path, _), _| w.path(path));
        weight.map(&self.package_manifests, |w, package, _| {
            w.add(package.name.capacity());
            w.add(package.version.capacity());
        });
        weight.map(&self.builtins, |w, path, _| w.path(path));
        if let Some((environment, _, _)) = &self.environment_seen {
            weight.add(environment.compiler_version.capacity());
        }
        // One prospective source-owner decoration covers its envelope and COW
        // file metadata; original detached views keep their own reservations.
        let mut total = weight
            .bytes()
            .checked_add(self.sources.retained_payload_bytes())
            .ok_or_else(invalid)?
            .checked_add(self.sources.retained_snapshot_metadata_bytes())
            .ok_or_else(invalid)?
            .checked_add(self.indexed_syntax_bytes().ok_or_else(invalid)?)
            .ok_or_else(invalid)?
            .checked_add(
                self.source_signature_entry_payload_bytes()
                    .ok_or_else(invalid)?,
            )
            .ok_or_else(invalid)?
            .checked_add(
                self.declarations
                    .retained_payload_bytes()
                    .ok_or_else(invalid)?,
            )
            .ok_or_else(invalid)?
            .checked_add(self.catalog.retained_payload_bytes())
            .ok_or_else(invalid)?
            .checked_add(self.types.retained_local_bytes())
            .ok_or_else(invalid)?
            .checked_add(self.graph.retained_bytes().ok_or_else(invalid)?)
            .ok_or_else(invalid)?
            .checked_add(self.front_changes.retained_bytes())
            .ok_or_else(invalid)?;
        for signature in self.builtins.values() {
            total = total
                .checked_add(crate::memo::retained::retained_signature_payload_bytes(
                    signature,
                ))
                .ok_or_else(invalid)?;
        }
        Ok(total)
    }

    /// Consume one settled source engine into bounded S0 state. Other engine
    /// products and controllers drop here. N-1 refuses before moving fields or
    /// replacing source owners; the original immutable inputs can be retried.
    pub fn into_source_index(
        mut self,
        allowance: MemoryReservation,
    ) -> Result<SourceIndexState, QueryError> {
        let required = self.source_index_bytes()?;
        if allowance.bytes() < required {
            return Err(invalid());
        }
        let allowance = Arc::new(allowance);
        let sources = std::mem::take(&mut self.sources).replace_owner(allowance.clone());
        let state = SourceIndexState {
            sources,
            indexed_sources: std::mem::take(&mut self.indexed_sources),
            items: std::mem::take(&mut self.items),
            builtins: std::mem::take(&mut self.builtins),
            syntax_keys: std::mem::take(&mut self.syntax_keys),
            signatures: std::mem::take(&mut self.signatures),
            graph: std::mem::take(&mut self.graph),
            front_changes: std::mem::take(&mut self.front_changes),
            inputs: std::mem::take(&mut self.inputs),
            environment_seen: self.environment_seen.take(),
            published_manifest: self.published_manifest,
            vanished: std::mem::take(&mut self.vanished),
            declarations: std::mem::take(&mut self.declarations),
            package_manifests: std::mem::take(&mut self.package_manifests),
            types: self.types.clone(),
            catalog: std::mem::take(&mut self.catalog),
            compiler: self.store_identity,
            snapshot_epoch: self.snapshot_epoch,
            snapshot_inputs_revision: self.snapshot_inputs_revision,
            allowance,
        };
        // Carry always owns/reclaims its source engine, independently of CLI
        // process-exit policy. No old coordinator may survive the handoff.
        self.session = false;
        Ok(state)
    }
}
impl SourceIndexState {
    pub fn reserved_bytes(&self) -> u64 {
        self.allowance.bytes()
    }
    pub fn sources(&self) -> &SourceMap {
        &self.sources
    }

    /// Restore into a fresh engine before the current loader updates sources
    /// and module inputs. Current compiler/adapters/policy remain authoritative;
    /// changed manifest/policy reads follow ordinary graph invalidation.
    pub fn restore(self, engine: &mut QueryEngine) -> Result<(), Box<Self>> {
        if self.compiler != engine.store_identity
            || !engine.graph.is_empty()
            || !engine.items.is_empty()
            || !engine.indexed_sources.is_empty()
            || engine.sources.file(hirsel_span::FileId(0)).is_some()
            || !engine.s1.is_empty()
            || !engine.active.is_empty()
        {
            return Err(Box::new(self));
        }
        let Self {
            sources,
            indexed_sources,
            items,
            builtins,
            syntax_keys,
            signatures,
            graph,
            front_changes,
            inputs,
            environment_seen,
            published_manifest,
            vanished,
            declarations,
            package_manifests,
            types,
            catalog,
            compiler: _,
            snapshot_epoch,
            snapshot_inputs_revision,
            allowance: _,
        } = self;
        engine.sources = sources;
        engine.indexed_sources = indexed_sources;
        engine.items = items;
        engine.builtins = builtins;
        engine.syntax_keys = syntax_keys;
        engine.signatures = signatures;
        engine.graph = graph;
        engine.graph.reset_stats();
        engine.front_changes = front_changes;
        engine.inputs = inputs;
        engine.environment_seen = environment_seen;
        engine.published_manifest = published_manifest;
        engine.vanished = vanished;
        engine.declarations = declarations;
        engine.package_manifests = package_manifests;
        engine.types = types;
        // Pure structural memoization is disposable; no previous body phase
        // or interpreter is installed into the fresh source engine.
        engine.catalog = catalog;
        engine.snapshot_epoch = snapshot_epoch;
        engine.snapshot_inputs_revision = snapshot_inputs_revision;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::ProcessBudget;
    fn environment() -> QueryEnvironment {
        QueryEnvironment {
            interface_version: 1,
            compiler_version: "source-carry".into(),
            spec_hash: Default::default(),
            package_manifest_hash: Default::default(),
            builtin_signatures_hash: Default::default(),
        }
    }
    fn engine(paths: &[DefPath]) -> QueryEngine {
        use hirsel_syntax::{ExprId, Item, Origin, SyntaxTree, TypeExprId, Visibility};
        let mut engine = QueryEngine::new(environment(), PhaseAdapters::default());
        for (i, path) in paths.iter().enumerate() {
            let file = engine
                .sources
                .add(
                    format!("carry-{i}.hir"),
                    "const value:u8=1u8;\n".repeat(paths.len()),
                )
                .unwrap();
            engine.indexed_sources.insert(
                file,
                (
                    FileContext {
                        package: path.0.package.clone(),
                        modules: path.0.modules.clone(),
                    },
                    engine.environment.clone(),
                ),
            );
            let anchor = engine.sources.item_anchor(file, 0);
            let span = Span {
                anchor,
                start: 0,
                end: 5,
            };
            let syntax = Arc::new(ItemSyntax {
                item: Item {
                    name_span: span,
                    origin: Origin {
                        span,
                        tokens: Default::default(),
                    },
                    name: Symbol::intern("value"),
                    definition: path.clone(),
                    visibility: Visibility::Public,
                    attributes: vec![],
                    kind: ItemKind::Const {
                        ty: TypeExprId(0),
                        value: ExprId(0),
                    },
                    generated: None,
                },
                tree: SyntaxTree::default(),
                anchor,
                text_hash: ContentHash::of_bytes(b"source"),
                occurrence: 0,
            });
            engine.items.insert(path.clone(), vec![syntax]);
            let id = DefId::intern(path);
            let ty = engine.types.intern(hirsel_types::Type::unit());
            let value = Arc::new(hirsel_types::Signature {
                metadata: Default::default(),
                definition: id,
                kind: hirsel_types::SignatureKind::Const {
                    generics: Default::default(),
                    ty,
                },
                fingerprint: Default::default(),
            });
            let memo = engine.graph.adopt_value(
                MemoKey::new(QueryKind::Signature, id),
                ContentHash::default(),
                Ok(Arc::new(output(value))),
                Arc::default(),
                crate::memo::Durability::Low,
            );
            engine.signatures.insert(
                id,
                Arc::new(Entry {
                    memo,
                    detail: Default::default(),
                    products: Default::default(),
                    covered: vec![],
                    depth: None,
                    acceptance: None,
                    verification: Default::default(),
                    selections: vec![],
                    consistent: true,
                }),
            );
        }
        engine
    }
    #[test]
    fn source_index_moves_bounded_anchors_headers_and_reverse_reads_without_a_controller() {
        crate::with_compiler_stack(|| {
            drop(engine(&[]));
            let symbol = Symbol::intern("value");
            let _ = symbol;
            for width in [1, 4, 16] {
                let paths: Vec<_> = (0..width)
                    .map(|i| DefPath::core(&format!("source.carry{i}")))
                    .collect();
                for path in &paths {
                    DefId::intern(path);
                }
                // The first unit lookup loads a process-shared core TypeBase
                // component. It belongs to the registry's original owner,
                // independently of this local source-state reservation.
                if width == 1 {
                    drop(engine(&paths));
                }
                let budget = ProcessBudget::new(64 << 20);
                let ((state, required), actual) = crate::test_alloc::measured(|| {
                    let engine = engine(&paths);
                    let (required, inspection) = crate::test_alloc::measured_independent(|| {
                        engine.source_index_bytes().unwrap()
                    });
                    assert_eq!(inspection.allocations, 0);
                    (
                        engine
                            .into_source_index(budget.reserve(required).unwrap())
                            .unwrap(),
                        required,
                    )
                });
                assert!(
                    actual.live as u64 <= required,
                    "width {width}: {} exceeds {required}",
                    actual.live
                );
                assert_eq!(budget.reserved(), required);
                let sources = state.sources().clone();
                let (_, first_drop) = crate::test_alloc::measured(|| drop(state));
                assert_eq!(
                    budget.reserved(),
                    required,
                    "detached source map keeps its complete grant"
                );
                let (_, last_drop) = crate::test_alloc::measured(|| drop(sources));
                assert_eq!(budget.reserved(), 0);
                assert_eq!(-(first_drop.live + last_drop.live), actual.live);

                let prior = engine(&paths);
                let required = prior.source_index_bytes().unwrap();
                assert!(
                    prior
                        .into_source_index(budget.reserve(required - 1).unwrap())
                        .is_err()
                );
                assert_eq!(budget.reserved(), 0);
                let prior = engine(&paths);
                let anchor = prior.items[&paths[0]][0].anchor;
                let header = prior.signatures[&DefId::intern(&paths[0])].clone();
                let required = prior.source_index_bytes().unwrap();
                let state = prior
                    .into_source_index(budget.reserve(required).unwrap())
                    .unwrap();
                let mut fresh = engine(&[]);
                state.restore(&mut fresh).ok().unwrap();
                assert_eq!(fresh.items[&paths[0]][0].anchor, anchor);
                assert!(Arc::ptr_eq(
                    &header,
                    &fresh.signatures[&DefId::intern(&paths[0])]
                ));
                assert_eq!(fresh.memo_snapshot().len(), paths.len());
                assert!(
                    fresh.resolutions.is_empty()
                        && fresh.checked.is_empty()
                        && fresh.modes.is_empty()
                        && fresh.lowered.is_empty()
                );
                assert!(fresh.s1.is_empty());
                assert_eq!(fresh.work, crate::WorkStats::default());
                drop(header);
                drop(fresh);
                assert_eq!(budget.reserved(), 0);
            }
        });
    }
    #[test]
    fn refused_source_index_restore_keeps_its_reservation_and_target_untouched() {
        crate::with_compiler_stack(|| {
            let path = DefPath::core("source.refused");
            let prior = engine(&[path.clone()]);
            let required = prior.source_index_bytes().unwrap();
            let budget = ProcessBudget::new(required);
            let state = prior
                .into_source_index(budget.reserve(required).unwrap())
                .unwrap();
            let mut target = engine(&[path.clone()]);
            let anchor = target.items[&path][0].anchor;
            let header = target.signatures[&DefId::intern(&path)].clone();
            let (refused, allocation) =
                crate::test_alloc::measured(|| state.restore(&mut target).unwrap_err());
            assert_eq!(allocation.live as usize, size_of::<SourceIndexState>());
            assert_eq!(target.items[&path][0].anchor, anchor);
            assert!(Arc::ptr_eq(
                &target.signatures[&DefId::intern(&path)],
                &header
            ));
            assert_eq!(budget.reserved(), required);
            drop(refused);
            assert_eq!(budget.reserved(), 0);
        });
    }
}

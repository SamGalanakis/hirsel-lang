//! Source-only carry across admitted requests. Move original anchors, body-free
//! heads, private proofs and reverse reads; never retain a LoadedProgram, body memo or
//! an S1 coordinator. A fresh engine supplies the current adapters and policy.
use super::*;
use crate::runtime::MemoryReservation;

pub struct SourceIndexState {
    sources: SourceMap,
    indexed_sources: BTreeMap<FileId, (FileContext, QueryEnvironment)>,
    builtins: BTreeMap<DefPath, hirsel_types::SignatureRef>,
    syntax_keys: Box<[(DefId, ContentHash)]>,
    signatures: Vec<(DefId, SourceHeader)>,
    specialization_dependencies: BTreeMap<DefId, parallel::SpecializationDependencies>,
    private_summary_reads: Box<[(DefId, Arc<Reads>)]>,
    private_summaries: Box<[(DefId, hirsel_types::UniqueSummary)]>,
    private_unique_failures: Box<[(DefId, Vec<hirsel_types::UniqueFailure>)]>,
    private_summary_revision: Option<crate::Revision>,
    graph: crate::memo::SourceHeaderGraph,
    front_changes: frontier::FrontTracker,
    inputs: Box<[(DefId, memos::DefInputs)]>,
    environment_seen: Option<(QueryEnvironment, crate::ComptimeLimits, Option<ContentHash>)>,
    published_manifest: Option<ContentHash>,
    vanished: BTreeSet<DefId>,
    declarations: crate::DeclarationIndex,
    package_manifests: BTreeMap<hirsel_syntax::PackageId, ContentHash>,
    types: Arc<hirsel_types::TypeInterner>,
    catalog: catalog::SourceCatalog,
    compiler: ContentHash,
    snapshot_epoch: crate::Revision,
    snapshot_inputs_revision: u64,
    // The SourceMap also owns this token, including its detached output clones.
    allowance: Arc<MemoryReservation>,
}
pub(crate) enum SourceHeader {
    Shared(Arc<Entry<hirsel_types::SignatureRef>>),
    Packed(PackedSourceHeader),
}
pub(crate) struct PackedSourceHeader {
    memo: crate::memo::Memo<QueryOutput<hirsel_types::SignatureRef>>,
    products: Arc<PhaseProducts>,
    details: Option<Box<SourceHeaderDetails>>,
    consistent: bool,
}
struct SourceHeaderDetails {
    failure: Option<hirsel_types::OperationalFailure>,
    covered: Option<Vec<SignatureDependency>>,
    depth: Option<crate::memo::DepthEvidence<Span>>,
    acceptance: Option<hirsel_validate::ExecutionReceipt>,
    selections: Vec<crate::comptime::Selection>,
}
impl SourceHeader {
    fn capture(entry: Arc<Entry<hirsel_types::SignatureRef>>) -> Self {
        if Arc::strong_count(&entry) > 1 {
            return Self::Shared(entry);
        }
        Self::Packed(PackedSourceHeader::capture(entry))
    }
    fn restore(self) -> Arc<Entry<hirsel_types::SignatureRef>> {
        match self {
            Self::Shared(entry) => entry,
            Self::Packed(header) => header.restore(),
        }
    }
}
impl PackedSourceHeader {
    fn capture(entry: Arc<Entry<hirsel_types::SignatureRef>>) -> Self {
        let same_coverage = entry
            .memo
            .value
            .as_ref()
            .is_ok_and(|output| output.dependencies == entry.covered);
        let details = (entry.detail.operational_failure.is_some()
            || !same_coverage
            || entry.depth.is_some()
            || entry.acceptance.is_some()
            || !entry.selections.is_empty())
        .then(|| {
            Box::new(SourceHeaderDetails {
                failure: entry.detail.operational_failure.clone(),
                covered: (!same_coverage).then(|| entry.covered.clone()),
                depth: entry.depth.clone(),
                acceptance: entry.acceptance.clone(),
                selections: entry.selections.clone(),
            })
        });
        Self {
            memo: entry.memo.clone(),
            products: entry.products.clone(),
            details,
            consistent: entry.consistent,
        }
    }
    fn restore(self) -> Arc<Entry<hirsel_types::SignatureRef>> {
        let SourceHeaderDetails {
            failure,
            covered,
            depth,
            acceptance,
            selections,
        } = self
            .details
            .map(|details| *details)
            .unwrap_or(SourceHeaderDetails {
                failure: None,
                covered: None,
                depth: None,
                acceptance: None,
                selections: vec![],
            });
        let covered = covered.unwrap_or_else(|| {
            self.memo
                .value
                .as_ref()
                .map(|output| output.dependencies.clone())
                .unwrap_or_default()
        });
        Arc::new(Entry {
            memo: self.memo,
            products: self.products,
            detail: DependencyReads {
                operational_failure: failure,
                ..Default::default()
            },
            covered,
            depth,
            acceptance,
            selections,
            consistent: self.consistent,
            verification: Default::default(),
        })
    }
}

fn invalid() -> QueryError {
    QueryError::Operational(hirsel_types::OperationalFailure {
        operation: "source index carry".into(),
        message: "source index exceeds admission, has an active transaction, or needs generated namespace carry".into(),
    })
}
impl QueryEngine {
    fn source_header_reads(&self, key: &MemoKey) -> Option<&Arc<Reads>> {
        self.signatures
            .get(&key.def)
            .filter(|entry| entry.memo.key == *key)
            .map(|entry| &entry.memo.reads)
    }

    fn source_types(&self) -> Result<hirsel_types::TypeProjection, QueryError> {
        // Immutable base rows already have their original owner. A base-only
        // context has no local closure to collect, including during inspection.
        if self.types.stats().rows == 0 {
            return Ok(self.types.project_reachable([]));
        }
        let (encoded, roots) = hirsel_types::with_table_collection(|| {
            bincode::serialize_into(std::io::sink(), &self.builtins)?;
            for entry in self.signatures.values() {
                bincode::serialize_into(std::io::sink(), &entry.memo.value)?;
                if let Some(owner) = &entry.products.representation {
                    bincode::serialize_into(std::io::sink(), &owner.representation.declaration)?;
                    bincode::serialize_into(std::io::sink(), &owner.view.get())?;
                }
            }
            Ok::<(), bincode::Error>(())
        });
        encoded.map_err(|_| invalid())?;
        Ok(self.types.project_reachable(roots))
    }

    /// Inspect the complete source-only carry representation. This performs no
    /// source/provider access or query. Shared core TypeBase data remains with
    /// the original process owner; custom bases refuse until independently owned.
    /// Unfinished transactions and generated namespaces refuse this S0 slice.
    pub fn source_index_bytes(&mut self) -> Result<u64, QueryError> {
        self.adopt_completed_headers();
        self.source_index_bytes_with_types(self.source_types()?.retained_bytes())
    }

    fn source_index_bytes_with_types(&self, type_bytes: u64) -> Result<u64, QueryError> {
        if !self.s1.is_empty()
            || !self.active.is_empty()
            || !self.signature_stack.is_empty()
            || !self.execution_stack.is_empty()
            || !self.generated.is_empty()
            || !self.staged_constants.is_empty()
            || !self.staged_values.is_empty()
            || self.private_summary_active
            || self.private_summary_failure.is_some()
            || !self.private_signature_publications.is_empty()
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
        weight.add(
            self.signatures
                .len()
                .saturating_mul(size_of::<(DefId, SourceHeader)>()),
        );
        for entry in self.signatures.values() {
            let same_coverage = entry
                .memo
                .value
                .as_ref()
                .is_ok_and(|output| output.dependencies == entry.covered);
            if entry.detail.operational_failure.is_some()
                || !same_coverage
                || entry.depth.is_some()
                || entry.acceptance.is_some()
                || !entry.selections.is_empty()
            {
                weight.add(size_of::<SourceHeaderDetails>());
            }
        }
        weight.map(&self.specialization_dependencies, |w, _, entry| {
            let mut reads = crate::memo::retained::Weight::default();
            reads.reads(&entry.reads);
            w.add(reads.bytes.try_into().unwrap_or(usize::MAX));
            w.vec(&entry.paths, |w, path| w.path(path));
        });
        weight
            .add(self.private_summaries.len() * size_of::<(DefId, hirsel_types::UniqueSummary)>());
        for summary in self.private_summaries.values() {
            weight.flat_vec(&summary.preserves_unique);
            weight.flat_vec(&summary.results);
            weight.flat_vec(&summary.nonretaining);
        }
        weight.add(
            self.private_unique_failures.len()
                * size_of::<(DefId, Vec<hirsel_types::UniqueFailure>)>(),
        );
        for failures in self.private_unique_failures.values() {
            weight.vec(failures, |w, failure| w.add(failure.message.capacity()));
        }
        weight.add(self.private_summary_reads.len() * size_of::<(DefId, Arc<Reads>)>());
        let mut previous: Option<&Arc<Reads>> = None;
        for reads in self.private_summary_reads.values() {
            // Module proofs share receipts and sort together by DefPath. Any
            // nonadjacent sharing is conservatively charged more than once.
            if previous.is_none_or(|prior| !Arc::ptr_eq(prior, reads)) {
                weight.arc(reads);
                for (_, bytes) in reads.allocations() {
                    weight.add(bytes);
                }
            }
            previous = Some(reads);
        }
        weight.add(self.inputs.len() * size_of::<(DefId, memos::DefInputs)>());
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
        weight.add(
            self.syntax_keys
                .keys()
                .filter(|(_, kind)| *kind == QueryKind::Signature)
                .count()
                * size_of::<(DefId, ContentHash)>(),
        );
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
        let mut names = crate::memo::retained::NameAllocations::<32768>::default();
        let shared_products = self
            .signatures
            .values()
            .any(crate::memo::retained::source_shared_products);
        let mut total = weight
            .bytes()
            .checked_add(if shared_products {
                crate::memo::retained::source_shared_products_bytes()
            } else {
                0
            })
            .ok_or_else(invalid)?
            .checked_add(self.sources.retained_payload_bytes())
            .ok_or_else(invalid)?
            .checked_add(self.sources.retained_snapshot_metadata_bytes())
            .ok_or_else(invalid)?
            .checked_add(
                self.signatures
                    .values()
                    .try_fold(0u64, |bytes, entry| {
                        bytes.checked_add(crate::memo::retained::source_signature_head_bytes(
                            entry, &mut names,
                        )?)
                    })
                    .ok_or_else(invalid)?,
            )
            .ok_or_else(invalid)?
            .checked_add(self.declarations.source_carry_bytes())
            .ok_or_else(invalid)?
            .checked_add(
                self.catalog
                    .source_carry_bytes(&self.items, &self.indexed_sources),
            )
            .ok_or_else(invalid)?
            .checked_add(type_bytes)
            .ok_or_else(invalid)?
            .checked_add(
                self.graph
                    .source_headers_bytes(|key| self.source_header_reads(key))
                    .ok_or_else(invalid)?,
            )
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
        if hirsel_intern::memory::metered() {
            let mut headers = if shared_products {
                crate::memo::retained::source_shared_products_bytes()
            } else {
                0
            };
            let mut parts = [0u64; 4];
            let mut names = crate::memo::retained::NameAllocations::<32768>::default();
            for entry in self.signatures.values() {
                headers += crate::memo::retained::source_signature_head_bytes(entry, &mut names)
                    .ok_or_else(invalid)?;
            }
            // Finish the global inspection before starting independent per-head
            // inspections, whose epochs may conservatively displace its marks.
            for entry in self.signatures.values() {
                for (total, bytes) in
                    parts
                        .iter_mut()
                        .zip(crate::memo::retained::source_signature_head_components(
                            entry,
                        ))
                {
                    *total += bytes;
                }
            }
            eprintln!(
                "source carry required: {total}; read-log allocation tracker: {:?}",
                names.stats()
            );
            eprintln!(
                "source carry bound: metadata={} source={} headers={} header-reads={} header-detail={} header-values={} header-dependencies={} declarations={} catalog={} types={} graph={}",
                weight.bytes(),
                self.sources.retained_payload_bytes()
                    + self.sources.retained_snapshot_metadata_bytes(),
                headers,
                parts[0],
                parts[1],
                parts[2],
                parts[3],
                self.declarations.source_carry_bytes(),
                self.catalog
                    .source_carry_bytes(&self.items, &self.indexed_sources),
                type_bytes,
                self.graph
                    .source_headers_bytes(|key| self.source_header_reads(key))
                    .ok_or_else(invalid)?,
            );
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
        self.adopt_completed_headers();
        let types = self.source_types()?;
        let required = self.source_index_bytes_with_types(types.retained_bytes())?;
        if allowance.bytes() < required {
            return Err(invalid());
        }
        let catalog = self
            .catalog
            .source_carry(&self.sources, &self.items, &self.indexed_sources);
        let shared_products = Arc::new(PhaseProducts {
            signature_persisted: true,
            ..Default::default()
        });
        let signatures: Table<hirsel_types::SignatureRef> = std::mem::take(&mut self.signatures)
            .into_iter()
            .map(|(id, entry)| {
                let projection = crate::memo::retained::source_projection(&entry);
                let share = projection.shared_products;
                let compact = projection.compact;
                let compact_output = projection.compact_output;
                if entry.products.resolution.is_none()
                    && !compact
                    && !projection.detail
                    && !projection.compact_reads
                    && !projection.shared_products
                {
                    return (id, entry);
                }
                let mut memo = entry.memo.clone();
                if projection.compact_reads {
                    memo.reads = Arc::new(entry.reads().flat_copy());
                }
                if compact_output {
                    memo.value = entry
                        .memo
                        .value
                        .as_ref()
                        .map(|output| {
                            // Clone owns only the exact vector lengths; immutable
                            // SignatureRef and read-log owners remain shared.
                            Arc::new((**output).clone())
                        })
                        .map_err(Clone::clone);
                }
                // Like a lazy durable Signature, this projection owns no resolver
                // syntax. It cannot replace the complete stored publication.
                let projected = Entry {
                    memo,
                    detail: DependencyReads {
                        operational_failure: entry.detail.operational_failure.clone(),
                        ..Default::default()
                    },
                    products: if share {
                        shared_products.clone()
                    } else if entry.products.resolution.is_some() || projection.detail {
                        Arc::new(PhaseProducts {
                            signature_persisted: true,
                            resolution: None,
                            ..(*entry.products).clone()
                        })
                    } else {
                        entry.products.clone()
                    },
                    covered: entry.covered.clone(),
                    depth: entry.depth.clone(),
                    acceptance: entry.acceptance.clone(),
                    verification: Default::default(),
                    selections: entry.selections.clone(),
                    consistent: entry.consistent,
                };
                (id, Arc::new(projected))
            })
            .collect();
        let graph = std::mem::take(&mut self.graph).into_source_headers(|key| {
            signatures
                .get(&key.def)
                .filter(|entry| entry.memo.key == *key)
                .map(|entry| &entry.memo.reads)
        });
        let signatures = signatures
            .into_iter()
            .map(|(id, entry)| (id, SourceHeader::capture(entry)))
            .collect();
        let allowance = Arc::new(allowance);
        let sources = std::mem::take(&mut self.sources).replace_owner(allowance.clone());
        let state = SourceIndexState {
            sources,
            indexed_sources: std::mem::take(&mut self.indexed_sources),
            builtins: std::mem::take(&mut self.builtins),
            syntax_keys: std::mem::take(&mut self.syntax_keys)
                .into_iter()
                .filter(|((_, kind), _)| *kind == QueryKind::Signature)
                .map(|((path, _), hash)| (DefId::intern(&path), hash))
                .collect::<Vec<_>>()
                .into_boxed_slice(),
            signatures,
            specialization_dependencies: std::mem::take(&mut self.specialization_dependencies),
            private_summary_reads: std::mem::take(&mut self.private_summary_reads)
                .into_iter()
                .collect::<Vec<_>>()
                .into_boxed_slice(),
            private_summaries: std::mem::take(&mut self.private_summaries)
                .into_iter()
                .collect::<Vec<_>>()
                .into_boxed_slice(),
            private_unique_failures: std::mem::take(&mut self.private_unique_failures)
                .into_iter()
                .collect::<Vec<_>>()
                .into_boxed_slice(),
            private_summary_revision: self.private_summary_revision,
            graph,
            front_changes: std::mem::take(&mut self.front_changes),
            inputs: std::mem::take(&mut self.inputs)
                .into_iter()
                .collect::<Vec<_>>()
                .into_boxed_slice(),
            environment_seen: self.environment_seen.take(),
            published_manifest: self.published_manifest,
            vanished: std::mem::take(&mut self.vanished),
            declarations: std::mem::take(&mut self.declarations).into_source_carry(),
            package_manifests: std::mem::take(&mut self.package_manifests),
            types: types.into_interner(),
            catalog,
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
            builtins,
            syntax_keys,
            signatures,
            specialization_dependencies,
            private_summary_reads,
            private_summaries,
            private_unique_failures,
            private_summary_revision,
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
        engine.builtins = builtins;
        engine.syntax_keys = syntax_keys
            .into_vec()
            .into_iter()
            .map(|(id, hash)| ((id.path().clone(), QueryKind::Signature), hash))
            .collect();
        engine.signatures = signatures
            .into_iter()
            .map(|(id, header)| (id, header.restore()))
            .collect();
        engine.specialization_dependencies = specialization_dependencies;
        engine.private_summary_reads = private_summary_reads.into_vec().into_iter().collect();
        engine.private_summaries = private_summaries.into_vec().into_iter().collect();
        engine.private_unique_failures = private_unique_failures.into_vec().into_iter().collect();
        engine.private_summary_revision = private_summary_revision;
        engine.graph = graph.restore();
        engine.graph.reset_stats();
        engine.front_changes = front_changes;
        engine.inputs = inputs.into_vec().into_iter().collect();
        engine.environment_seen = environment_seen;
        engine.published_manifest = published_manifest;
        engine.vanished = vanished;
        engine.declarations = declarations;
        engine.package_manifests = package_manifests;
        engine.types = types;
        // Pure structural memoization is disposable; no previous body phase
        // or interpreter is installed into the fresh source engine.
        engine.catalog = catalog.restore();
        engine
            .catalog
            .bind_ownership(engine.source_ownership.clone());
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
    fn retained_specialization_receipts_survive_body_free_registration_passes() {
        let path = DefPath::core("retained_specialization");
        let mut engine = engine(std::slice::from_ref(&path));
        let id = DefId::intern(&path);
        let text = engine.key_text(&path, QueryKind::Resolve).unwrap();
        let mut reads = crate::memo::ReadsBuilder::new();
        reads.record(ReadKey::Environment, engine.environment_value());
        engine.specialization_dependencies.insert(
            id,
            parallel::SpecializationDependencies {
                text,
                reads: Arc::new(reads.finish().unwrap()),
                paths: vec![],
            },
        );
        let before = engine.memo_stats().touched;
        engine.register_specializations(std::slice::from_ref(&path));
        assert!(engine.specialization_dependencies.contains_key(&id));
        assert_eq!(engine.memo_stats().touched, before);

        // An unchanged body hash cannot validate a changed compiler input.
        engine.environment.compiler_version.push_str("-changed");
        assert_eq!(engine.key_text(&path, QueryKind::Resolve).unwrap(), text);
        engine.register_specializations(std::slice::from_ref(&path));
        assert!(engine.memo_stats().touched > before);
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
                    let mut engine = engine(&paths);
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
                assert!(fresh.items.is_empty());
                assert_eq!(fresh.catalog.anchor(&paths[0], 0), Some(anchor));
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
    fn source_carry_drops_syntax_products_and_restores_private_proofs_with_exact_reads() {
        crate::with_compiler_stack(|| {
            let mut prior = QueryEngine::new(environment(), crate::phases::engine_only_adapters());
            let context = default_context();
            let body = (0..256)
                .map(|i| format!("let x{i} = {i}u64;\n"))
                .collect::<String>();
            let file = prior
                .sources
                .add("private.hir", format!("fn value()->u64{{{body}0u64}}\n"))
                .unwrap();
            let parsed = prior.parse_file(file, &context).unwrap();
            let syntax = parsed.value.items[0].clone();
            let path = syntax.item.definition.clone();
            let id = DefId::intern(&path);
            let anchor = syntax.anchor;
            let weak = Arc::downgrade(&syntax);
            let text = prior.cited_value(&path);
            let reads = Arc::new(Reads::from_vec(vec![(ReadKey::CitedText(id), text)]).unwrap());
            prior.private_summary_reads.insert(id, reads.clone());
            prior.private_summaries.insert(
                id,
                hirsel_types::UniqueSummary {
                    preserves_unique: vec![hirsel_types::ParamId(0)],
                    results: vec![],
                    ..Default::default()
                },
            );
            prior.private_unique_failures.insert(
                id,
                vec![hirsel_types::UniqueFailure {
                    claim: hirsel_types::UniqueClaim::Parameter(hirsel_types::ParamId(0)),
                    span: syntax.item.name_span,
                    message: "exact failure provenance".into(),
                }],
            );
            // A header's resolver product is another independent route to the
            // complete expression arena. Neither route may survive the carry.
            let signature = engine(&[path.clone()]).signatures.remove(&id).unwrap();
            let mut signature = Arc::try_unwrap(signature).ok().unwrap();
            signature.products = Arc::new(PhaseProducts {
                resolution: Some(crate::ResolverSnapshot {
                    syntax: syntax.clone(),
                    output: output(hirsel_types::Resolution::default()),
                }),
                ..Default::default()
            });
            prior.signatures.insert(id, Arc::new(signature));
            drop(syntax);
            drop(parsed);
            let required = prior.source_index_bytes().unwrap();
            assert!(required < prior.indexed_syntax_bytes().unwrap());
            let budget = ProcessBudget::new(required);
            let carry = prior
                .into_source_index(budget.reserve(required).unwrap())
                .unwrap();
            assert!(
                weak.upgrade().is_none(),
                "all syntax owners must be released"
            );
            let mut fresh = QueryEngine::new(environment(), crate::phases::engine_only_adapters());
            carry.restore(&mut fresh).ok().unwrap();
            assert!(fresh.items.is_empty());
            assert_eq!(fresh.catalog.anchor(&path, 0), Some(anchor));
            assert!(fresh.signatures[&id].products.resolution.is_none());
            assert_eq!(
                fresh.private_summaries[&id].preserves_unique,
                [hirsel_types::ParamId(0)]
            );
            assert_eq!(fresh.private_unique_failures[&id][0].span.anchor, anchor);
            assert!(Arc::ptr_eq(&fresh.private_summary_reads[&id], &reads));
            assert_eq!(fresh.observe(&path, &ReadKey::CitedText(id)), Some(text));
            fresh.materialize(&path).unwrap();
            assert_eq!(fresh.items[&path][0].anchor, anchor);
            assert!(fresh.items[&path][0].tree.expressions.len() > 256);
            drop(fresh);
            assert_eq!(budget.reserved(), 0);
        });
    }
    #[test]
    fn source_carry_compacts_unowned_buffers_and_keeps_shared_output_identity() {
        crate::with_compiler_stack(|| {
            let path = DefPath::core("source.buffers");
            for shared in [false, true] {
                let mut prior = engine(&[path.clone()]);
                let id = DefId::intern(&path);
                let mut header = Arc::try_unwrap(prior.signatures.remove(&id).unwrap())
                    .ok()
                    .unwrap();
                header.detail.signatures = Vec::with_capacity(32768);
                header.covered = Vec::with_capacity(32768);
                let exact = (*header.memo.reads).clone();
                let mut body_reads: Vec<_> = exact.iter().copied().collect();
                for i in 0..512 {
                    body_reads.push((
                        ReadKey::CitedText(DefId::intern(&DefPath::core(&format!(
                            "unretained_{i}"
                        )))),
                        ContentHash::default(),
                    ));
                }
                let complete = Reads::from_vec(body_reads).unwrap();
                header.memo.reads = Arc::new(exact.share_rows(&complete).unwrap());
                let prior_read_bytes = header.reads().allocations().next().unwrap().1;
                let exact_wire = bincode::serialize(header.reads()).unwrap();
                drop((exact, complete));
                let mut output = (*header.result().unwrap()).clone();
                output.dependencies = Vec::with_capacity(32768);
                let signature = output.value.clone();
                let output = Arc::new(output);
                let held = shared.then(|| output.clone());
                header.memo.value = Ok(output);
                prior.signatures.insert(id, Arc::new(header));
                prior
                    .snapshot_sources
                    .insert(id, Arc::downgrade(prior.signatures.get(&id).unwrap()));
                let held_reads = prior.signatures[&id].memo.reads.clone();
                let required = prior.source_index_bytes().unwrap();
                let budget = ProcessBudget::new(required);
                let carry = prior
                    .into_source_index(budget.reserve(required).unwrap())
                    .unwrap();
                let mut fresh = engine(&[]);
                carry.restore(&mut fresh).ok().unwrap();
                let header = &fresh.signatures[&id];
                assert_eq!(header.detail.signatures.capacity(), 0);
                assert_eq!(header.covered.capacity(), 0);
                assert!(header.reads().allocations().next().unwrap().1 < prior_read_bytes);
                assert_eq!(bincode::serialize(header.reads()).unwrap(), exact_wire);
                assert_eq!(bincode::serialize(&*held_reads).unwrap(), exact_wire);
                assert_eq!(held_reads.allocations().next().unwrap().1, prior_read_bytes);
                assert!(!Arc::ptr_eq(&held_reads, &header.memo.reads));
                let actual = header.result().unwrap();
                assert!(Arc::ptr_eq(&actual.value, &signature));
                if let Some(held) = held {
                    assert!(Arc::ptr_eq(&held, &actual));
                    assert!(actual.dependencies.capacity() >= 32768);
                } else {
                    assert_eq!(actual.dependencies.capacity(), 0);
                }
                drop(actual);
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

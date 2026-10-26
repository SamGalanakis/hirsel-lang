//! Snapshot-only receipts for the products an ordinary check actually required.
//! Reuse PS-9's canonical signature/TIR encoding, not transient interner ids.
use super::*;
use hirsel_proof_export::{ExportInput, Header, Location, Locator, SignatureSection};
use std::rc::Rc;

/// Current ordinary-check obligations, independent of retained cache contents.
#[derive(Default)]
pub(crate) struct CheckedScope {
    pub(crate) completed: Option<crate::memo::Revision>,
    pub(crate) front: BTreeMap<DefPath, Arc<crate::memo::Reads>>,
    pub(crate) lowered: BTreeSet<DefPath>,
    /// Settled check-time failures have diagnostics but no body to reconstruct.
    pub(crate) failed_lowered: BTreeSet<DefPath>,
}

#[derive(Clone)]
pub struct CheckedProducts {
    pub digest: ContentHash,
    /// Optional diagnostic export; the digest always commits to the full bytes.
    pub canonical: Option<Arc<str>>,
    /// Key bytes use ReadKey::encode: DefPaths and spellings, never DefIds.
    pub reads: BTreeMap<DefPath, BTreeMap<Vec<u8>, ContentHash>>,
    pub type_rows: u64,
    pub type_bytes: u64,
    owner: Arc<dyn Send + Sync>,
}
impl std::fmt::Debug for CheckedProducts {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CheckedProducts")
            .field("digest", &self.digest)
            .field("canonical", &self.canonical)
            .field("reads", &self.reads)
            .field("type_rows", &self.type_rows)
            .field("type_bytes", &self.type_bytes)
            .finish()
    }
}
impl CheckedProducts {
    /// Keep producer and output admission alive through detached receipt clones.
    /// The token controls lifetime only and grants no verification authority.
    pub fn retain_owner(mut self, owner: Arc<dyn Send + Sync>) -> Self {
        self.owner = Arc::new((self.owner, owner));
        self
    }
    /// The same receipt without canonical text. Its copied read maps are
    /// split from `grant` before the copy and stay charged while any clone
    /// of the view is held; None leaves `grant` unchanged.
    pub fn digest_view(&self, grant: &mut crate::runtime::MemoryReservation) -> Option<Arc<Self>> {
        let charge = grant.split(self.digest_view_bytes()?)?;
        let view = Self {
            digest: self.digest,
            canonical: None,
            reads: self.reads.clone(),
            type_rows: self.type_rows,
            type_bytes: self.type_bytes,
            owner: self.owner.clone(),
        };
        Some(Arc::new(view.retain_owner(Arc::new(charge))))
    }
    /// The view's own allocations: its Arc, the owner pair and charge Arc
    /// binding its grant, and the copied read maps with their paths and keys.
    fn digest_view_bytes(&self) -> Option<u64> {
        fn arc<T>() -> Option<usize> {
            Some(
                std::alloc::Layout::new::<[usize; 2]>()
                    .extend(std::alloc::Layout::new::<T>())
                    .ok()?
                    .0
                    .pad_to_align()
                    .size(),
            )
        }
        let mut weight = crate::NativeMetadataWeight::default();
        weight.add(arc::<Self>()?);
        weight.add(arc::<(Arc<dyn Send + Sync>, Arc<dyn Send + Sync>)>()?);
        weight.add(arc::<crate::runtime::MemoryReservation>()?);
        weight.tree::<DefPath, BTreeMap<Vec<u8>, ContentHash>>(self.reads.len());
        for (path, reads) in &self.reads {
            weight.path(path);
            weight.tree::<Vec<u8>, ContentHash>(reads.len());
            for key in reads.keys() {
                weight.add(key.len());
            }
        }
        Some(weight.bytes())
    }
    /// Exact owner/read keys ordered by persisted bytes, independent of intern
    /// assignment and scheduling. Includes private views and negative reads.
    pub fn canonical_reads(&self) -> BTreeMap<(Vec<u8>, Vec<u8>), ContentHash> {
        self.reads
            .iter()
            .flat_map(|(owner, reads)| {
                let mut bytes = Vec::new();
                crate::memo::encode_def_path(&mut bytes, owner);
                reads
                    .iter()
                    .map(move |(key, value)| ((bytes.clone(), key.clone()), *value))
            })
            .collect()
    }
}
impl QueryEngine {
    /// Existing accepted namespace records, in the same order used by the
    /// checked-product digest. This formats settled registrations only.
    pub(crate) fn checked_registration_records(&self) -> Vec<Vec<u8>> {
        let mut registrations = Vec::new();
        for (path, generated) in &self.generated {
            let mut key = vec![];
            crate::memo::encode_def_path(&mut key, path);
            registrations.push(key);
            registrations.push(generated.text.as_bytes().to_vec());
        }
        for declaration in &self.instance_bodies.declarations {
            let mut key = vec![];
            crate::memo::encode_def_path(&mut key, declaration.path());
            registrations.push(key);
        }
        for (declaration, instances) in &self.instance_bodies.instances {
            let mut records: Vec<_> = instances
                .iter()
                .map(|(actual, body)| {
                    let mut record = vec![];
                    crate::memo::encode_def_path(&mut record, declaration.path());
                    for spelling in actual.spellings() {
                        record.extend_from_slice(&(spelling.len() as u64).to_le_bytes());
                        record.extend_from_slice(spelling.as_bytes());
                    }
                    crate::memo::encode_def_path(&mut record, body.definition.path());
                    record
                })
                .collect();
            records.sort();
            registrations.extend(records);
        }
        registrations
    }
    /// Does not lower additional runtime definitions. Compacted artifacts
    /// already required by this check may be reconstructed through the normal
    /// eligible store path; this cost belongs to receipt production.
    pub fn checked_products(&mut self, entry: &DefPath) -> Result<CheckedProducts, QueryError> {
        if self.checked_scope.completed != Some(self.graph.revision()) {
            return Err(QueryError::NotImplemented);
        }
        let failed = self
            .checked_scope
            .failed_lowered
            .iter()
            .map(|path| {
                (
                    path.clone(),
                    self.checked_scope
                        .front
                        .get(path)
                        .cloned()
                        .unwrap_or_default(),
                )
            })
            .collect::<Vec<_>>();
        for (path, reads) in failed {
            if !self.execution_valid(&path, &reads) {
                return Err(QueryError::NotImplemented);
            }
        }
        let mut definitions = BTreeMap::new();
        let paths = self.checked_scope.lowered.clone();
        for path in paths {
            if self.checked_scope.failed_lowered.contains(&path) {
                continue;
            }
            // Failed lowerings are represented by diagnostics, never a body.
            if self
                .lowered
                .get(&DefId::intern(&path))
                .is_some_and(|e| e.result().is_err())
            {
                continue;
            }
            if self
                .lower_receipts
                .get(&DefId::intern(&path))
                .is_some_and(|e| e.result().is_err())
            {
                continue;
            }
            definitions.insert(path.clone(), self.lower_to_tir(&path)?.value);
        }
        let snapshot = self.signature_snapshot();
        let mut signatures = BTreeMap::new();
        let paths: BTreeSet<_> = self
            .source_paths()
            .into_iter()
            .chain(self.builtins.keys().cloned())
            .chain(
                self.registry
                    .signatures()
                    .map(|s| s.definition.path().clone()),
            )
            .chain(self.staged_constants.keys().cloned())
            .collect();
        for path in paths {
            let id = DefId::intern(&path);
            let public = match snapshot.signature(id) {
                Some(Ok(signature)) => signature.clone(),
                _ => match self.builtins.get(&path) {
                    Some(signature) => signature.clone(),
                    None => continue,
                },
            };
            let owner_view = snapshot
                .body_view(&path.0.package, id)
                .and_then(Result::ok)
                .filter(|view| ***view != *public)
                .map(|view| (**view).clone());
            signatures.insert(
                path.clone(),
                SignatureSection {
                    path: path.clone(),
                    dependencies: self.signature_coverage(&path)?,
                    public: (*public).clone(),
                    owner_view,
                },
            );
        }
        // Only runtime constant references (and materialized constant roots)
        // belong to this check. S1 may retain unrelated std evaluations.
        let mut required_constants = BTreeSet::new();
        for (path, definition) in &definitions {
            if self.constant_values.contains_key(path) {
                required_constants.insert(path.clone());
            }
            for value in definition.bodies.iter().flat_map(|body| &body.rvalues) {
                if let hirsel_tir::Rvalue::ReadConstant {
                    definition,
                    arguments,
                } = value
                {
                    let arguments = arguments
                        .iter()
                        .map(|size| match size {
                            hirsel_types::Size::Constant(n) => Some(*n),
                            _ => None,
                        })
                        .collect::<Option<Vec<_>>>();
                    if let Some(arguments) = arguments {
                        required_constants.insert(if arguments.is_empty() {
                            definition.clone()
                        } else {
                            hirsel_tir::constant_instance_path(definition, &arguments)
                        });
                    }
                }
            }
        }
        let sources = Rc::new(self.sources.clone());
        let locator: Locator = Rc::new(move |span| {
            let span = sources.resolve(span);
            let Some(file) = sources.file(span.file) else {
                return Location::unknown();
            };
            let (Some(start), Some(end)) = (file.location(span.start), file.location(span.end))
            else {
                return Location::unknown();
            };
            Location {
                module: file.logical_path().to_string_lossy().into_owned(),
                line: start.line,
                column: start.column,
                end_line: end.line,
                end_column: end.column,
            }
        });
        let input = ExportInput {
            types: self.types.clone(),
            header: Header {
                format: hirsel_proof_export::FORMAT.into(),
                tir_version: hirsel_tir::TIR_VERSION,
                environment: self.proof_environment(),
                entry: entry.clone(),
                program_revision: None,
            },
            definitions,
            signatures,
            constants: self
                .constant_values
                .iter()
                .map(|(path, value)| (path.clone(), value.value.clone()))
                .chain(self.staged_values.iter().map(|((path, args), value)| {
                    (
                        hirsel_tir::constant_instance_path(path, args),
                        value.value.clone(),
                    )
                }))
                .filter(|(path, _)| required_constants.contains(path))
                .collect(),
            skipped: vec![],
            annotations: vec![],
            locator,
        };
        let encoder = CheckedProductEncoder::new(&input)?;
        // The check's complete dependency edges, per definition. Intermediate
        // Resolve/Check/Modes tables have different retention lifetimes and a
        // stored Modes product can subsume them. Preserve every observed edge
        // and value, rather than make table retention part of semantic identity.
        let mut required_reads = self.checked_scope.front.clone();
        for path in &self.checked_scope.lowered {
            if let Some(lower) = self.shared_memo_reads(path, QueryKind::Lower) {
                let combined = required_reads.entry(path.clone()).or_default();
                let mut edges = combined
                    .iter()
                    .chain(lower.iter())
                    .map(|(key, value)| (*key, *value))
                    .collect::<Vec<_>>();
                edges.sort();
                edges.dedup();
                *combined = Arc::new(crate::memo::Reads::from_vec(edges).map_err(|error| {
                    QueryError::Operational(hirsel_types::OperationalFailure {
                        operation: "candidate reads".into(),
                        message: format!("conflicting complete read values for {path}: {error:?}"),
                    })
                })?);
            }
        }
        let registrations = self.checked_registration_records();
        Ok(encoder.finish(required_reads, registrations))
    }
}

/// Canonical encoding of already settled source obligations. This formats
/// ordinary immutable data and cannot demand a query or supply V authority.
pub struct CheckedProductEncoder {
    canonical: Arc<str>,
    types: Arc<hirsel_types::TypeInterner>,
}
impl CheckedProductEncoder {
    /// Already encoded bytes from the same admitted immutable builder. This
    /// constructor carries no V or native authority.
    pub(crate) fn from_canonical(
        canonical: Arc<str>,
        types: Arc<hirsel_types::TypeInterner>,
    ) -> Self {
        Self { canonical, types }
    }
    pub fn new(input: &ExportInput) -> Result<Self, QueryError> {
        let export = hirsel_proof_export::build(input);
        if !export.skipped.is_empty() {
            return Err(QueryError::Operational(hirsel_types::OperationalFailure {
                operation: "candidate products".into(),
                message: format!("incomplete canonical products: {:?}", export.skipped),
            }));
        }
        let canonical: Arc<str> = hirsel_proof_export::encode(&export, &input.locator).into();
        Ok(Self {
            canonical,
            types: input.types.clone(),
        })
    }
    /// Complete read rows and the existing canonical generated/instance
    /// registration records, in source order. None may be inferred from a
    /// semantic cache or omitted because its body was evicted.
    pub fn finish(
        self,
        required_reads: BTreeMap<DefPath, Arc<crate::memo::Reads>>,
        registrations: Vec<Vec<u8>>,
    ) -> CheckedProducts {
        let canonical = self.canonical;
        let mut reads: BTreeMap<DefPath, BTreeMap<Vec<u8>, ContentHash>> = BTreeMap::new();
        for (path, observed) in required_reads {
            let edges = reads.entry(path).or_default();
            for (key, value) in observed.iter() {
                let mut bytes = vec![];
                key.encode(&mut bytes);
                edges.insert(bytes, *value);
            }
        }
        let mut parts = registrations;
        for (path, observed) in &reads {
            let mut key = vec![];
            crate::memo::encode_def_path(&mut key, path);
            parts.push(key);
            for (key, value) in observed {
                parts.push(key.clone());
                parts.push(value.0.to_vec());
            }
        }
        let stats = self.types.stats();
        CheckedProducts {
            digest: ContentHash::of_parts(
                "hirsel.checked-products.v1",
                std::iter::once(canonical.as_bytes()).chain(parts.iter().map(Vec::as_slice)),
            ),
            canonical: Some(canonical),
            reads,
            type_rows: stats.rows,
            type_bytes: stats.row_bytes,
            owner: Arc::new(()),
        }
    }
}

#[cfg(test)]
mod registration_tests {
    use super::*;
    #[test]
    fn checked_digest_borrows_detached_canonical_text_without_copying_its_payload() {
        let types = hirsel_types::TypeInterner::new(None);
        for width in [1, 4, 16] {
            let canonical: Arc<str> = "λ🐑canonical\n".repeat(width * 1024).into();
            let registrations = vec![b"registration-one".to_vec(), b"registration-two".to_vec()];
            let expected = ContentHash::of_parts(
                "hirsel.checked-products.v1",
                std::iter::once(canonical.as_bytes())
                    .chain(registrations.iter().map(Vec::as_slice)),
            );
            let pointer = canonical.as_ptr();
            let (actual, count) = crate::test_alloc::measured(|| {
                CheckedProductEncoder::from_canonical(canonical, types.clone())
                    .finish(BTreeMap::new(), registrations)
            });
            assert_eq!(actual.digest, expected);
            assert_eq!(actual.canonical.as_ref().unwrap().as_ptr(), pointer);
            assert!(
                count.peak < 1024,
                "digest copied the canonical text: {}",
                count.peak
            );
        }
    }

    #[test]
    fn detached_checked_product_clones_keep_all_output_reservations() {
        let budget = crate::runtime::ProcessBudget::new(96);
        let products = CheckedProducts {
            digest: ContentHash::of_bytes(b"receipt"),
            canonical: Some("canonical receipt".into()),
            reads: BTreeMap::new(),
            type_rows: 0,
            type_bytes: 0,
            owner: Arc::new(()),
        }
        .retain_owner(Arc::new(budget.reserve(64).unwrap()));
        let original = products.clone();
        let products = products.retain_owner(Arc::new(budget.reserve(32).unwrap()));
        let detached = products.clone();
        drop(products);
        assert_eq!(budget.reserved(), 96);
        drop(original);
        assert_eq!(budget.reserved(), 96);
        assert_eq!(detached.canonical.as_deref(), Some("canonical receipt"));
        drop(detached);
        assert_eq!(budget.reserved(), 0);
    }

    #[test]
    fn digest_views_charge_their_copied_read_maps_until_the_last_clone_drops() {
        let budget = crate::runtime::ProcessBudget::new(1 << 20);
        let mut reads = BTreeMap::new();
        reads.insert(
            DefPath::core("main"),
            BTreeMap::from([(b"key".to_vec(), ContentHash::of_bytes(b"value"))]),
        );
        let products = CheckedProducts {
            digest: ContentHash::of_bytes(b"receipt"),
            canonical: Some("canonical receipt".into()),
            reads,
            type_rows: 1,
            type_bytes: 2,
            owner: Arc::new(()),
        };
        let needed = products.digest_view_bytes().unwrap();
        let mut short = budget.reserve(needed - 1).unwrap();
        assert!(products.digest_view(&mut short).is_none());
        assert_eq!(short.bytes(), needed - 1);
        drop(short);
        let mut grant = budget.reserve(needed + 7).unwrap();
        let view = products.digest_view(&mut grant).unwrap();
        assert_eq!(grant.bytes(), 7);
        drop(grant);
        assert_eq!(view.canonical, None);
        assert_eq!(view.digest, products.digest);
        assert_eq!(view.reads, products.reads);
        assert_eq!((view.type_rows, view.type_bytes), (1, 2));
        let held = view.clone();
        drop(view);
        assert_eq!(budget.reserved(), needed);
        drop(held);
        assert_eq!(budget.reserved(), 0);
    }

    #[test]
    fn registration_records_keep_generated_text_and_sorted_instance_actuals() {
        let mut engine = QueryEngine::new(crate::default_environment(), PhaseAdapters::default());
        let generated = DefPath::core("generated");
        let declaration = DefId::intern(&DefPath::core("factory"));
        let file = engine.sources.add("generated.hir", String::new()).unwrap();
        engine.generated.insert(
            generated.clone(),
            GeneratedSource {
                file,
                text: "fn generated(){}".into(),
                site: Span::default(),
            },
        );
        engine.instance_bodies.declarations.insert(declaration);
        let mut instances = std::collections::HashMap::new();
        for n in [2, 1] {
            let arguments = vec![hirsel_types::GenericArgument::Size(
                hirsel_types::Size::Constant(n),
            )];
            let key = hirsel_tir::InstanceBodies::key(&engine.types, &arguments).unwrap();
            instances.insert(
                key,
                hirsel_tir::InstanceBody {
                    arguments,
                    definition: DefId::intern(&DefPath::core(&format!("factory_{n}"))),
                },
            );
        }
        engine
            .instance_bodies
            .instances
            .insert(declaration, instances);
        let records = engine.checked_registration_records();
        let encode = |path: &DefPath| {
            let mut bytes = Vec::new();
            crate::memo::encode_def_path(&mut bytes, path);
            bytes
        };
        assert_eq!(records.len(), 5);
        assert_eq!(records[0], encode(&generated));
        assert_eq!(records[1], b"fn generated(){}");
        assert_eq!(records[2], encode(declaration.path()));
        assert!(records[3] < records[4]);
        for record in &records[3..] {
            assert!(record.starts_with(&encode(declaration.path())));
        }
        let entries = engine
            .instance_bodies
            .instances
            .remove(&declaration)
            .unwrap();
        let mut entries = entries.into_iter().collect::<Vec<_>>();
        entries.reverse();
        engine
            .instance_bodies
            .instances
            .insert(declaration, entries.into_iter().collect());
        assert_eq!(engine.checked_registration_records(), records);
        engine.generated.get_mut(&generated).unwrap().text.push(' ');
        let changed = engine.checked_registration_records();
        assert_ne!(
            changed, records,
            "accepted generated text participates verbatim"
        );
        assert_eq!(&changed[2..], &records[2..]);
    }
}

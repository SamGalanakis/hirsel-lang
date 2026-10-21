//! The frozen signature snapshot of one revision (design §1(b), "Snapshot").
//!
//! A revision computes its snapshot once: every signature by `DefId`, the
//! private views each opaque type's own package sees, and V's verdict on every
//! view with each package's universe errors. It is immutable, `Send + Sync`
//! and stamped with its epoch, so definitions read it without locks.
use crate::memo::Revision;
use crate::*;
use hirsel_syntax::{DefId, DefMap, PackageId};
use hirsel_types::{LookupOutput, SignatureRef, TypeInterner};
use hirsel_validate::{SignatureLookup, ValidatedSignatures, ValidationDigest};
use std::collections::BTreeMap;
use std::sync::Arc;

mod declarations;
mod provider;
mod views;
pub use declarations::{DeclarationIndex, Declarations, ModuleInputs};
pub(crate) use declarations::{RowImage, header_hash, logical};
pub use provider::FrozenProvider;

pub use views::{PackageViews, View, ViewTable};

/// Durable signature views and successful per-view validation receipts.
/// Encoders use the canonical type table and image anchor relocation scopes.
/// Digests are independently recomputed on import; fingerprints are no proof.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct SnapshotSeed {
    pub public: Vec<SignatureRef>,
    pub private: Vec<SignatureRef>,
    pub verdicts: Vec<(ValidationDigest, Vec<hirsel_validate::VRead>)>,
}
impl SnapshotSeed {
    fn verdicts(&self) -> ValidatedSignatures {
        let mut store = ValidatedSignatures::default();
        for (digest, reads) in &self.verdicts {
            store.seed_success(*digest, reads.clone());
        }
        store
    }
}

/// One revision's signatures, private views and V verdicts.
pub struct SignatureSnapshot {
    epoch: Revision,
    types: Arc<TypeInterner>,
    signatures: DefMap<Result<SignatureRef, QueryError>>,
    /// Each opaque type's private view, which only its own package's bodies see.
    private: DefMap<SignatureRef>,
    /// V's universe: every public view, with its verdict.
    views: Arc<ViewTable>,
    /// Each package with private views, as its bodies see the universe.
    packages: BTreeMap<PackageId, PackageViews>,
    validated: ValidatedSignatures,
    /// What lookups resolve against, frozen with the signatures.
    lookup: LookupInput,
}
/// A snapshot's lookup input: the provider manifest hash every lookup
/// observes, the registry a bare name falls back to, the source builtins and
/// each definition's `DeclarationCandidates`.
#[derive(Clone, Default)]
pub(crate) struct LookupInput {
    pub manifest: ContentHash,
    pub registry: crate::registry::BuiltinRegistry,
    pub builtins: std::collections::BTreeSet<DefId>,
    pub candidates: DefMap<ContentHash>,
    pub failed: DefMap<ContentHash>,
}
const _: () = {
    const fn shared<T: Send + Sync>() {}
    shared::<SignatureSnapshot>();
};
impl SignatureSnapshot {
    /// The snapshot of `signatures` (every public signature, by path, or the
    /// error that normalizing it reported) and `private` (each opaque owner's
    /// private view), reusing `previous`'s digests and verdicts that still hold.
    pub fn new(
        epoch: Revision,
        types: Arc<TypeInterner>,
        signatures: impl IntoIterator<Item = (DefId, Result<SignatureRef, QueryError>)>,
        private: impl IntoIterator<Item = SignatureRef>,
        previous: Option<&SignatureSnapshot>,
    ) -> Self {
        Self::build(epoch, types, signatures, private, previous, None)
    }
    /// Import the views, independently digesting them before any successful
    /// receipt is reused. Changed views or contextual reads are checked again.
    pub fn from_seed(epoch: Revision, types: Arc<TypeInterner>, seed: &SnapshotSeed) -> Self {
        let store = seed.verdicts();
        Self::build(
            epoch,
            types,
            seed.public.iter().map(|s| (s.definition, Ok(s.clone()))),
            seed.private.iter().cloned(),
            None,
            Some(&store),
        )
    }
    fn build(
        epoch: Revision,
        types: Arc<TypeInterner>,
        signatures: impl IntoIterator<Item = (DefId, Result<SignatureRef, QueryError>)>,
        private: impl IntoIterator<Item = SignatureRef>,
        previous: Option<&SignatureSnapshot>,
        seeds: Option<&ValidatedSignatures>,
    ) -> Self {
        let _timing = hirsel_intern::timings::phase(hirsel_intern::timings::Phase::Verify);
        let t = &*types;
        let signatures: DefMap<_> = signatures.into_iter().collect();
        let private: DefMap<SignatureRef> = private
            .into_iter()
            .map(|view| (view.definition, view))
            .collect();
        let mut combined;
        let old = if let Some(seeds) = seeds.filter(|store| !store.is_empty()) {
            combined = previous.map(|p| p.validated.clone()).unwrap_or_default();
            for (digest, reads) in seeds.successful() {
                combined.seed_success(digest, reads.to_vec());
            }
            Some(&combined)
        } else {
            previous.map(|p| &p.validated)
        };
        let mut validated = ValidatedSignatures::default();
        let views = Arc::new(ViewTable::new(
            t,
            signatures.values().filter_map(|s| s.as_ref().ok().cloned()),
            previous.map(|p| &*p.views),
            &mut validated,
            old,
        ));
        let mut owned: BTreeMap<PackageId, Vec<View>> = BTreeMap::new();
        for (d, view) in &private {
            let package = &d.path().0.package;
            let earlier = previous
                .and_then(|p| p.packages.get(package))
                .and_then(|p| p.replacement(*d));
            owned
                .entry(package.clone())
                .or_default()
                .push(View::new(t, view.clone(), earlier));
        }
        let packages = owned
            .into_iter()
            .map(|(package, replaced)| {
                let views = PackageViews::new(t, views.clone(), replaced, &mut validated, old);
                (package, views)
            })
            .collect();
        Self {
            epoch,
            types,
            signatures,
            private,
            views,
            packages,
            validated,
            lookup: LookupInput::default(),
        }
    }
    /// Retain only successful verdicts; failures are checked again on import.
    pub fn seed(&self) -> SnapshotSeed {
        let mut public: Vec<_> = self
            .signatures
            .values()
            .filter_map(|s| s.as_ref().ok().cloned())
            .collect();
        public.sort_by_key(|s| s.definition);
        let mut private: Vec<_> = self.private.values().cloned().collect();
        private.sort_by_key(|s| s.definition);
        let mut verdicts: Vec<_> = self
            .validated
            .successful()
            .map(|(digest, reads)| (digest, reads.to_vec()))
            .collect();
        verdicts.sort_unstable();
        SnapshotSeed {
            public,
            private,
            verdicts,
        }
    }
    pub(crate) fn with_lookup(mut self, lookup: LookupInput) -> Self {
        self.lookup = lookup;
        self
    }
    /// Resolve a provider lookup as the engine's provider does: a path naming
    /// a definition this snapshot holds (or a source builtin), else a bare
    /// registry name. The output carries the manifest hash it was read under.
    pub fn lookup(&self, context: &FileContext, path: &hirsel_syntax::Path) -> LookupOutput {
        let result = (|| {
            if path
                .segments
                .iter()
                .any(|segment| segment.as_str().starts_with('$'))
            {
                return Err(QueryError::NotImplemented);
            }
            let definition =
                crate::queries::lookup_path(context, path).ok_or(QueryError::NotImplemented)?;
            let d = DefId::intern(&definition);
            if self.signatures.contains_key(&d) || self.lookup.builtins.contains(&d) {
                Ok(definition)
            } else {
                self.lookup
                    .registry
                    .lookup_name(path)
                    .ok_or(QueryError::NotImplemented)
            }
        })();
        LookupOutput {
            manifest: self.lookup.manifest,
            result,
        }
    }
    /// `DeclarationCandidates(d)` at this snapshot's revision.
    pub fn declaration_candidates(&self, d: DefId) -> Option<ContentHash> {
        self.lookup.candidates.get(&d).copied()
    }
    /// The exact negative receipt captured from the failed query's complete key,
    /// or the canonical absent-query receipt. A diagnostic's debug spelling is
    /// never an input identity.
    pub fn failed_signature_receipt(&self, d: DefId) -> ContentHash {
        self.lookup
            .failed
            .get(&d)
            .copied()
            .unwrap_or_else(|| crate::queries::memos::optional(None))
    }
    pub fn epoch(&self) -> Revision {
        self.epoch
    }
    /// The session interner every view's rows live in.
    pub fn types(&self) -> &Arc<TypeInterner> {
        &self.types
    }
    /// The public signature of `d`, or the error normalizing it reported.
    pub fn signature(&self, d: DefId) -> Option<Result<&SignatureRef, &QueryError>> {
        self.signatures.get(&d).map(Result::as_ref)
    }
    /// The view of `d` a body of `package` sees: an opaque type's private view
    /// inside its own package, its public signature everywhere else.
    pub fn body_view(
        &self,
        package: &PackageId,
        d: DefId,
    ) -> Option<Result<&SignatureRef, &QueryError>> {
        match self.private.get(&d) {
            Some(view) if &d.path().0.package == package => Some(Ok(view)),
            _ => self.signature(d),
        }
    }
    /// `d`'s view in its own package (an opaque type's private view) with
    /// V's digest of it: what compile-time reflection of `d` reads.
    pub fn representation(
        &self,
        d: DefId,
    ) -> Option<Result<(&SignatureRef, ValidationDigest), &QueryError>> {
        let package = &d.path().0.package;
        if let Some(view) = self.packages.get(package).and_then(|p| p.replacement(d)) {
            return Some(Ok((&view.signature, view.digest)));
        }
        match self.signatures.get(&d)? {
            Ok(_) => self.views.get(d).map(|v| Ok((&v.signature, v.digest))),
            Err(error) => Some(Err(error)),
        }
    }
    /// V's verdicts on this snapshot's views.
    pub fn validated(&self) -> &ValidatedSignatures {
        &self.validated
    }
    /// The universe V validates `package`'s bodies against.
    pub fn validation(&self, package: &PackageId) -> &dyn SignatureLookup {
        match self.packages.get(package) {
            Some(views) => views,
            None => &*self.views,
        }
    }
    /// `package`'s universe with `signature` in place of its declared view:
    /// compile-time evaluation validates a materialized owner against it.
    pub fn validation_with(&self, package: &PackageId, signature: SignatureRef) -> PackageViews {
        let t = &*self.types;
        let mut store = ValidatedSignatures::default();
        match self.packages.get(package) {
            Some(views) => views.replace(t, signature, &mut store, Some(&self.validated)),
            None => PackageViews::new(
                t,
                self.views.clone(),
                [View::new(t, signature, None)],
                &mut store,
                Some(&self.validated),
            ),
        }
    }
}

impl QueryEngine {
    /// Prime V's receipt candidates without replacing any source or image view.
    /// Snapshot construction independently digests the actual views and verifies
    /// all contextual reads before using a receipt.
    pub fn seed_signature_verdicts(&mut self, seed: &SnapshotSeed) {
        for (digest, reads) in &seed.verdicts {
            self.snapshot_seed_verdicts
                .seed_success(*digest, reads.clone());
        }
    }
    /// This revision's snapshot, built once after its inputs changed. It is
    /// kept only when every signature normalized, so a failed one is retried.
    pub fn signature_snapshot(&mut self) -> Arc<SignatureSnapshot> {
        let stamp = self.snapshot_stamp();
        if let Some((kept, snapshot)) = &self.snapshot
            && *kept == stamp
        {
            return snapshot.clone();
        }
        let mut signatures: Vec<(DefId, Result<SignatureRef, QueryError>)> = self
            .registry
            .signatures()
            .map(|s| (s.definition, Ok(s.clone())))
            .collect();
        let mut private = vec![];
        let mut complete = true;
        for path in self.source_paths() {
            let d = DefId::intern(&path);
            let signature = self.signature(d);
            if let Ok(public) = &signature {
                match self.body_signature(&path.0.package, public.clone()) {
                    Ok((view, _)) if !Arc::ptr_eq(&view, public) => private.push(view),
                    Ok(_) => {}
                    Err(_) => complete = false,
                }
            } else {
                complete = false;
            }
            signatures.push((d, signature));
        }
        for (path, constant) in &self.staged_constants {
            signatures.push((DefId::intern(path), Ok(constant.signature.clone())));
        }
        self.snapshot_epoch = self.snapshot_epoch.next();
        let previous = self.snapshot.as_ref().map(|(_, s)| s.clone());
        let lookup = LookupInput {
            manifest: ContentHash::of_parts(
                "hirsel.provider.registry.v1",
                [
                    self.manifest_revision().0.as_slice(),
                    self.registry.revision().0.as_slice(),
                ],
            ),
            registry: self.registry.clone(),
            builtins: self.builtins.keys().map(DefId::intern).collect(),
            failed: signatures
                .iter()
                .filter(|(_, result)| result.is_err())
                .map(|(d, _)| (*d, self.failed_key_value(d.path())))
                .collect(),
            candidates: self
                .source_paths()
                .iter()
                .map(|path| (DefId::intern(path), self.candidates_value(path)))
                .collect(),
        };
        let snapshot = Arc::new(
            SignatureSnapshot::build(
                self.snapshot_epoch,
                self.types.clone(),
                signatures,
                private,
                previous.as_deref(),
                Some(&self.snapshot_seed_verdicts),
            )
            .with_lookup(lookup),
        );
        if complete {
            self.snapshot_seed_verdicts = ValidatedSignatures::default();
            self.snapshot = Some((stamp, snapshot.clone()));
        }
        snapshot
    }
    /// The inputs a snapshot built now would be built from.
    pub(crate) fn snapshot_stamp(&self) -> SnapshotStamp {
        SnapshotStamp {
            declarations: self.declarations.revision(),
            revision: self.snapshot_inputs_revision,
            environment: self.environment.clone(),
            limits: self.comptime_limits,
            manifest: Some(self.declarations.inputs().manifest_revision),
            unavailable: self.unavailable,
        }
    }
    /// The explicit inputs of module resolution, to change.
    pub fn module_inputs_mut(&mut self) -> &mut ModuleInputs {
        self.declarations.inputs_mut()
    }
    /// `DeclarationCandidates(path)`: H(count, then (file path, header hash)
    /// per candidate in source order), or None for a path with no item.
    pub fn declaration_candidates(&self, path: &DefPath) -> Option<ContentHash> {
        self.declarations.candidates(path)
    }
    /// The frozen declaration index, published to the phases that read it.
    pub(crate) fn frozen_declarations(&mut self) -> Arc<Declarations> {
        self.declarations.frozen()
    }
    /// Signatures changed outside a source revision (a staged constant).
    pub(crate) fn retire_snapshot(&mut self) {
        self.snapshot_inputs_revision += 1;
    }
    /// V's report from `validate` against `package`'s universe, or no
    /// violations under `Verify::Off`. Every V run of a session goes through here.
    pub(crate) fn run_v(
        &mut self,
        package: &PackageId,
        validate: impl FnOnce(&TypeInterner, &dyn SignatureLookup) -> hirsel_validate::Validation,
    ) -> hirsel_validate::Validation {
        match self.verify {
            Verify::On => {
                let snapshot = self.signature_snapshot();
                validate(snapshot.types(), snapshot.validation(package))
            }
            Verify::Off => hirsel_validate::Validation::default(),
        }
    }
}

/// The inputs a kept snapshot was built from.
#[derive(PartialEq)]
pub(crate) struct SnapshotStamp {
    declarations: u64,
    revision: u64,
    environment: QueryEnvironment,
    limits: hirsel_interp::Limits,
    manifest: Option<ContentHash>,
    unavailable: Option<QueryKind>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memo::{ReadKey, ReadsBuilder};
    use hirsel_types::{SignatureKind, SignatureProvider, Type, TypeKind};

    fn constant(t: &TypeInterner, name: &str, primitive: hirsel_types::Primitive) -> SignatureRef {
        let mut path = DefPath::core("snapshot_test");
        path.0.name = name.into();
        Arc::new(hirsel_types::Signature {
            metadata: Default::default(),
            definition: DefId::intern(&path),
            kind: SignatureKind::Const {
                ty: t.intern(Type::with_kind(TypeKind::Primitive(primitive))),
                generics: Default::default(),
            },
            fingerprint: ContentHash::of_bytes(name.as_bytes()),
        })
    }

    /// Eight workers read one snapshot through their own `FrozenProvider`s
    /// with no lock, each recording the same typed reads.
    #[test]
    fn eight_workers_read_one_snapshot_and_record_the_same_reads() {
        let types = TypeInterner::new(None);
        let signatures: Vec<SignatureRef> = (0..64)
            .map(|i| constant(&types, &format!("c{i}"), hirsel_types::Primitive::U64))
            .collect();
        let snapshot = Arc::new(SignatureSnapshot::new(
            Revision::from_u64(1).unwrap(),
            types.clone(),
            signatures.iter().map(|s| (s.definition, Ok(s.clone()))),
            [],
            None,
        ));
        let owner = signatures[0].definition.path().0.package.clone();
        let reads = hirsel_intern::jobs::map(
            hirsel_intern::jobs::Jobs::new(8),
            (0..8).collect::<Vec<usize>>(),
            |_| 1,
            |_| {
                let mut reads = ReadsBuilder::new();
                let mut provider = FrozenProvider::new(&snapshot, owner.clone(), &mut reads);
                for signature in &signatures {
                    let read = provider.signature(signature.definition).unwrap();
                    assert!(Arc::ptr_eq(&read, signature));
                    provider.validation_view(signature.definition).unwrap();
                }
                provider.universe_errors();
                reads.finish().unwrap()
            },
        );
        assert_eq!(reads.len(), 8);
        assert!(reads.iter().all(|r| r == &reads[0]));
        assert!(reads[0].iter().any(|(key, value)| {
            *key == ReadKey::Signature(signatures[3].definition)
                && *value == signatures[3].fingerprint
        }));
    }

    /// A missing signature is a recorded negative read, so a later
    /// revision that adds the definition invalidates the reader.
    #[test]
    fn a_missing_signature_is_a_recorded_failed_read() {
        let types = TypeInterner::new(None);
        let present = constant(&types, "present", hirsel_types::Primitive::U32);
        let absent = constant(&types, "absent", hirsel_types::Primitive::U32);
        let snapshot = SignatureSnapshot::new(
            Revision::from_u64(1).unwrap(),
            types,
            [(present.definition, Ok(present.clone()))],
            [],
            None,
        );
        let mut reads = ReadsBuilder::new();
        let package = present.definition.path().0.package.clone();
        let mut provider = FrozenProvider::new(&snapshot, package, &mut reads);
        assert!(provider.signature(absent.definition).is_err());
        let reads = reads.finish().unwrap();
        assert!(
            reads
                .iter()
                .any(|(key, _)| *key == ReadKey::FailedSignature(absent.definition))
        );
    }
    #[test]
    fn durable_verdicts_recompute_independent_digests_and_reject_changed_views() {
        use hirsel_intern::work::{self, Counter};
        let types = TypeInterner::new(None);
        let signature = constant(&types, "durable", hirsel_types::Primitive::U64);
        let snapshot = SignatureSnapshot::new(
            Revision::FIRST,
            types.clone(),
            [(signature.definition, Ok(signature.clone()))],
            [],
            None,
        );
        let seed = snapshot.seed();
        assert!(!seed.verdicts.is_empty());
        // Encode in one interner, decode in another with deliberately different
        // row numbers: the durable receipt names canonical content, never ids.
        let (_, roots) = hirsel_types::with_table_collection(|| serde_json::to_vec(&seed).unwrap());
        let (table, _) = hirsel_types::encode_table(&types, &roots);
        let bytes = hirsel_types::with_table_encoding(&types, &roots, || {
            serde_json::to_vec(&seed).unwrap()
        });
        let other = TypeInterner::new(None);
        other.intern(Type::with_kind(TypeKind::Primitive(
            hirsel_types::Primitive::U8,
        )));
        let ids = hirsel_types::decode_table(&other, &table);
        let mut decoded: SnapshotSeed =
            hirsel_types::with_table_decoding(&ids, || serde_json::from_slice(&bytes).unwrap());
        let before = work::read(Counter::ValidateSignatureRows);
        let imported = SignatureSnapshot::from_seed(Revision::FIRST, other.clone(), &decoded);
        assert_eq!(work::read(Counter::ValidateSignatureRows), before);
        assert!(
            imported
                .validation(&signature.definition.path().0.package)
                .universe_errors()
                .1
                .is_empty()
        );
        // Malformed data with the producer fingerprint unchanged must miss the
        // imported success and fail V's own check.
        let changed = Arc::make_mut(&mut decoded.public[0]);
        let fingerprint = changed.fingerprint;
        let SignatureKind::Const { ty, .. } = &mut changed.kind else {
            unreachable!()
        };
        *ty = other.intern(Type::with_kind(TypeKind::Error));
        assert_eq!(changed.fingerprint, fingerprint);
        let changed = SignatureSnapshot::from_seed(Revision::FIRST, other, &decoded);
        assert_eq!(work::read(Counter::ValidateSignatureRows), before + 1);
        assert!(
            !changed
                .validation(&signature.definition.path().0.package)
                .universe_errors()
                .1
                .is_empty()
        );
        assert!(changed.seed().verdicts.is_empty());
    }

    #[test]
    fn durable_private_views_reject_shape_changes_without_public_fingerprint_changes() {
        use hirsel_intern::work::{self, Counter};
        use hirsel_types::{Allocation, AuthoritySummary, Bound, Layout, TypeDefinition};
        let types = TypeInterner::new(None);
        let mut public = (*constant(&types, "Opaque", hirsel_types::Primitive::U64)).clone();
        public.metadata.visibility = hirsel_syntax::Visibility::PublicOpaque;
        public.kind = SignatureKind::Type(TypeDefinition {
            loop_source: None,
            format: None,
            destructor_allocation: Allocation::NoAlloc,
            generics: vec![],
            representation: None,
            bounds: vec![Bound::Plain],
            authority: AuthoritySummary::inert(),
            capability: None,
            implementation_authority: None,
            serializable: false,
            serializable_check: None,
            layout: Layout::default(),
            distinct: false,
            resource: false,
            drop: None,
            fact: None,
            captures: None,
        });
        let mut private = public.clone();
        let SignatureKind::Type(declaration) = &mut private.kind else {
            unreachable!()
        };
        declaration.representation = Some(types.intern(Type::with_kind(TypeKind::Primitive(
            hirsel_types::Primitive::U64,
        ))));
        let public = Arc::new(public);
        let snapshot = SignatureSnapshot::new(
            Revision::FIRST,
            types.clone(),
            [(public.definition, Ok(public.clone()))],
            [Arc::new(private)],
            None,
        );
        let mut seed = snapshot.seed();
        assert_eq!(seed.private.len(), 1);
        let before = work::read(Counter::ValidateSignatureRows);
        let imported = SignatureSnapshot::from_seed(Revision::FIRST, types.clone(), &seed);
        assert_eq!(work::read(Counter::ValidateSignatureRows), before);
        let package = &public.definition.path().0.package;
        assert!(imported.validation(package).universe_errors().1.is_empty());
        let private = Arc::make_mut(&mut seed.private[0]);
        let SignatureKind::Type(declaration) = &mut private.kind else {
            unreachable!()
        };
        // String fails the declared plain bound; the unchanged public view
        // still has a reusable pass, while this private view must be checked.
        declaration.representation = Some(types.intern(Type::with_kind(TypeKind::Primitive(
            hirsel_types::Primitive::String,
        ))));
        assert_eq!(private.fingerprint, public.fingerprint);
        let changed = SignatureSnapshot::from_seed(Revision::FIRST, types, &seed);
        assert_eq!(work::read(Counter::ValidateSignatureRows), before + 1);
        assert!(!changed.validation(package).universe_errors().1.is_empty());
        assert_eq!(changed.seed().verdicts.len(), 1);
    }
    #[test]
    fn engine_seed_reuses_verdicts_without_granting_declaration_authority() {
        use hirsel_intern::work::{self, Counter};
        let mut original =
            QueryEngine::new(default_environment(), crate::phases::engine_only_adapters());
        let mut seed = original.signature_snapshot().seed();
        let injected = constant(
            original.types(),
            "not_declared",
            hirsel_types::Primitive::U64,
        );
        seed.public.push(injected.clone());
        let mut imported =
            QueryEngine::new(default_environment(), crate::phases::engine_only_adapters());
        imported.seed_signature_verdicts(&seed);
        assert!(!imported.snapshot_seed_verdicts.is_empty());
        let before = work::read(Counter::ValidateSignatureRows);
        let snapshot = imported.signature_snapshot();
        assert_eq!(work::read(Counter::ValidateSignatureRows), before);
        assert!(snapshot.signature(injected.definition).is_none());
        assert!(imported.snapshot_seed_verdicts.is_empty());
    }
    #[test]
    fn frozen_negative_and_validation_receipts_match_live_observations() {
        struct Reject;
        impl SignaturePhase for Reject {
            fn signature(
                &self,
                _: &TypeInterner,
                _: &hirsel_types::TypeMemo,
                syntax: Arc<ItemSyntax>,
                _: &Declarations,
                _: &mut PhaseProducts,
                _: &mut dyn SignatureProvider,
            ) -> Result<QueryOutput<Signature>, QueryError> {
                Err(QueryError::Diagnostics(vec![Diagnostic::error(
                    const { hirsel_diag::RuleId::of("LANG-7") },
                    syntax.item.name_span,
                    "fixture rejection",
                )]))
            }
        }
        let mut adapters = crate::phases::engine_only_adapters();
        adapters.signature = Some(Arc::new(Reject));
        let mut engine = QueryEngine::new(default_environment(), adapters);
        let file = engine
            .sources
            .add("failed.hir", "fn failed() -> u64 { 1 }".into())
            .unwrap();
        let parsed = engine.parse_file(file, &default_context()).unwrap();
        let owner = parsed.value.items[0].item.definition.clone();
        let failed = DefId::intern(&owner);
        let mut missing = owner.clone();
        missing.0.name = "absent".into();
        let absent = DefId::intern(&missing);
        let snapshot = engine.signature_snapshot();
        let mut reads = ReadsBuilder::new();
        let mut provider = FrozenProvider::new(&snapshot, owner.0.package.clone(), &mut reads);
        assert!(provider.signature(failed).is_err());
        assert!(provider.signature(absent).is_err());
        provider.declaration_candidates(failed);
        provider.declaration_candidates(absent);
        provider.validation_view(failed);
        provider.universe_errors();
        provider.first_heap();
        let reads = reads.finish().unwrap();
        for (key, value) in reads.iter() {
            assert_eq!(engine.observe(&owner, key), Some(*value), "{key:?}");
        }
        // A failed header's exact complete query key changes even when the
        // phase returns byte-identical diagnostic prose.
        let old = snapshot.failed_signature_receipt(failed);
        let file = engine
            .sources
            .add("failed.hir", "fn failed(x: u8) -> u64 { 1 }".into())
            .unwrap();
        engine.parse_file(file, &default_context()).unwrap();
        let changed = engine.signature_snapshot();
        assert_ne!(old, changed.failed_signature_receipt(failed));
        assert_eq!(
            engine.observe(&owner, &ReadKey::FailedSignature(failed)),
            Some(changed.failed_signature_receipt(failed))
        );
    }
}

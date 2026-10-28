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
use std::sync::{Arc, Mutex, OnceLock};

mod checked_runtime;
mod declarations;
mod provider;
mod sources;
pub use sources::CitationSource;
pub(crate) use sources::{OwnedSourceContext, SourceContext};
mod views;
pub(crate) use declarations::{DeclarationHead, DeclarationSiblings, logical};
pub use declarations::{DeclarationIndex, Declarations, ModuleInputs};
pub use provider::FrozenProvider;

pub use views::{PackageViews, View, ViewTable};

/// One revision's signatures, private views and V verdicts.
pub struct SignatureSnapshot {
    epoch: Revision,
    types: Arc<TypeInterner>,
    signatures: DefMap<Result<SignatureRef, QueryError>>,
    headers: Option<Arc<crate::queries::header_products::HeaderProducts>>,
    /// Each opaque type's private view, which only its own package's bodies see.
    private: DefMap<SignatureRef>,
    /// Signature inputs are sufficient for P; V builds its universe on demand.
    validation: OnceLock<Arc<ValidationState>>,
    validation_inputs: Mutex<Option<ValidationCandidates>>,
    /// Each immutable product is identified once, shared by reflection and V.
    public_views: DefMap<OnceLock<View>>,
    private_views: DefMap<OnceLock<View>>,
    /// What lookups resolve against, frozen with the signatures.
    lookup: LookupInput,
    /// Local verifier authority is never part of a serialized snapshot/product.
    verifier: Option<Arc<hirsel_validate::VerifierCache>>,
    verifier_policy: ContentHash,
}
fn view_products<'a>(
    types: &TypeInterner,
    rows: impl Iterator<Item = (DefId, &'a SignatureRef)>,
    previous: Option<&DefMap<OnceLock<View>>>,
) -> DefMap<OnceLock<View>> {
    rows.map(|(d, signature)| {
        let cell = OnceLock::new();
        if let Some(view) = previous.and_then(|p| p.get(&d)).and_then(OnceLock::get)
            && view.matches(types, signature)
        {
            let _ = cell.set(view.clone());
        }
        (d, cell)
    })
    .collect()
}
/// Receipt candidates are not source authority. Keep them through deferred
/// epochs, and release the old universe once this snapshot initializes V.
#[derive(Clone, Default)]
struct ValidationCandidates {
    previous: Option<Arc<ValidationState>>,
}
struct ValidationState {
    views: Arc<ViewTable>,
    bases: BTreeMap<PackageId, Arc<ViewTable>>,
    packages: BTreeMap<PackageId, PackageViews>,
    validated: ValidatedSignatures,
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
    pub failed: DefMap<Option<ContentHash>>,
    pub package_dependencies: BTreeMap<PackageId, Vec<PackageId>>,
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
        let snapshot = Self::build(epoch, types, signatures, private, previous);
        snapshot.validation_state();
        snapshot
    }
    fn build(
        epoch: Revision,
        types: Arc<TypeInterner>,
        signatures: impl IntoIterator<Item = (DefId, Result<SignatureRef, QueryError>)>,
        private: impl IntoIterator<Item = SignatureRef>,
        previous: Option<&SignatureSnapshot>,
    ) -> Self {
        let signatures: DefMap<_> = signatures.into_iter().collect();
        let private: DefMap<SignatureRef> = private
            .into_iter()
            .map(|view| (view.definition, view))
            .collect();
        let public_views = view_products(
            &types,
            signatures
                .iter()
                .filter_map(|(d, s)| s.as_ref().ok().map(|s| (*d, s))),
            previous.map(|p| &p.public_views),
        );
        let private_views = view_products(
            &types,
            private.iter().map(|(d, s)| (*d, s)),
            previous.map(|p| &p.private_views),
        );
        let candidates = previous
            .map(|p| p.validation_candidates())
            .unwrap_or_default();
        Self {
            epoch,
            types,
            signatures,
            headers: None,
            private,
            validation: OnceLock::new(),
            validation_inputs: Mutex::new(Some(candidates)),
            public_views,
            private_views,
            verifier: None,
            verifier_policy: ContentHash::default(),
            lookup: LookupInput::default(),
        }
    }
    fn with_headers(
        mut self,
        headers: Arc<crate::queries::header_products::HeaderProducts>,
    ) -> Self {
        for d in headers.definitions() {
            self.public_views.entry(d).or_default();
            self.private_views.entry(d).or_default();
        }
        self.headers = Some(headers);
        self
    }
    pub(crate) fn header_products(
        &self,
    ) -> Option<&Arc<crate::queries::header_products::HeaderProducts>> {
        self.headers.as_ref()
    }
    fn validation_candidates(&self) -> ValidationCandidates {
        if let Some(state) = self.validation.get() {
            return ValidationCandidates {
                previous: Some(state.clone()),
            };
        }
        let pending = self.validation_inputs.lock().unwrap().clone();
        pending.unwrap_or_else(|| {
            // An initializer has taken the inputs but has not yet published.
            // Join that initializer after releasing the short input lock.
            self.validation_state();
            ValidationCandidates {
                previous: self.validation.get().cloned(),
            }
        })
    }
    fn validation_state(&self) -> &ValidationState {
        self.validation.get_or_init(|| {
            let _timing = hirsel_intern::timings::phase(hirsel_intern::timings::Phase::Verify);
            let t = &self.types;
            let ValidationCandidates { previous } = self
                .validation_inputs
                .lock()
                .unwrap()
                .take()
                .expect("OnceLock consumes validation inputs once");
            let previous = previous.as_deref();
            let old = previous.map(|p| &p.validated);
            let mut validated =
                ValidatedSignatures::with_cache(self.verifier.clone(), self.verifier_policy);
            let views = Arc::new(ViewTable::from_views(
                t,
                self.definitions().into_iter().filter_map(|d| {
                    self.signature(d).and_then(Result::ok).map(|s| {
                        self.public_views[&d]
                            .get_or_init(|| {
                                View::new(t, s.clone(), previous.and_then(|p| p.views.get(d)))
                            })
                            .clone()
                    })
                }),
                &mut validated,
                old,
            ));
            // Dependency packages cannot observe their importers. Keep every
            // selected dependency and registry row in each actual V universe,
            // including valid rows, private replacements and heap/error inputs.
            // Explicit snapshots without a complete loader graph retain the
            // full-world contract rather than guessing package visibility.
            let mut bases = BTreeMap::new();
            let builtins: std::collections::BTreeSet<_> = self
                .lookup
                .registry
                .signatures()
                .map(|s| s.definition.path().0.package.clone())
                .collect();
            for package in self.lookup.package_dependencies.keys() {
                let mut visible = builtins.clone();
                let mut pending = vec![package.clone()];
                let mut complete = true;
                while let Some(next) = pending.pop() {
                    if !visible.insert(next.clone()) {
                        continue;
                    }
                    let Some(dependencies) = self.lookup.package_dependencies.get(&next) else {
                        complete = false;
                        break;
                    };
                    pending.extend(dependencies.iter().cloned());
                }
                if !complete {
                    continue;
                }
                let base = if views
                    .views()
                    .iter()
                    .all(|v| visible.contains(&v.d.path().0.package))
                {
                    views.clone()
                } else {
                    Arc::new(ViewTable::new(
                        t,
                        views
                            .views()
                            .iter()
                            .filter(|v| visible.contains(&v.d.path().0.package))
                            .map(|v| v.signature().clone()),
                        Some(&views),
                        &mut validated,
                        old,
                    ))
                };
                bases.insert(package.clone(), base);
            }
            let mut owned: BTreeMap<PackageId, Vec<View>> = BTreeMap::new();
            for d in self.definitions() {
                let Some(view) = self.private_view(d) else {
                    continue;
                };
                let package = &d.path().0.package;
                let earlier = previous
                    .and_then(|p| p.packages.get(package))
                    .and_then(|p| p.replacement(d));
                owned.entry(package.clone()).or_default().push(
                    self.private_views[&d]
                        .get_or_init(|| View::new(t, view.clone(), earlier))
                        .clone(),
                );
            }
            let packages = owned
                .into_iter()
                .map(|(package, replaced)| {
                    let base = bases.get(&package).unwrap_or(&views).clone();
                    (
                        package,
                        PackageViews::new(t, base, replaced, &mut validated, old),
                    )
                })
                .collect();
            Arc::new(ValidationState {
                views,
                bases,
                packages,
                validated,
            })
        })
    }
    pub(crate) fn with_lookup(mut self, lookup: LookupInput) -> Self {
        self.lookup = lookup;
        self
    }
    fn with_verifier(
        mut self,
        verifier: Option<Arc<hirsel_validate::VerifierCache>>,
        policy: ContentHash,
    ) -> Self {
        self.verifier = verifier;
        self.verifier_policy = policy;
        self
    }
    pub(crate) fn source_verification_policy(&self) -> ContentHash {
        ContentHash::of_parts(
            "hirsel.validate.frozen-source.v2",
            [self.verifier_policy.0.as_slice()],
        )
    }
    pub(crate) fn validate_source_artifact(
        &self,
        definition: &hirsel_tir::DefinitionRef,
        owner: impl FnMut(hirsel_span::Anchor) -> Option<(DefPath, u32)>,
    ) -> (
        hirsel_validate::Validation,
        bool,
        Option<hirsel_validate::AcceptedArtifact>,
        Option<hirsel_validate::VerifiedDefinition>,
    ) {
        hirsel_validate::validate_definition(
            definition,
            self.types.clone(),
            owner,
            self.validation(&definition.path.path().0.package),
            self.source_verification_policy(),
            self.verifier.as_deref(),
        )
    }
    pub(crate) fn validate_private_artifact(
        &self,
        definition: &hirsel_tir::Definition,
        signature: SignatureRef,
        origin: Option<&(hirsel_span::Anchor, DefPath, u32)>,
    ) -> (
        hirsel_validate::Validation,
        Option<hirsel_validate::AcceptedArtifact>,
    ) {
        let views = self.validation_with(&definition.path.path().0.package, signature.clone());
        let artifact = hirsel_validate::Artifact::private(
            definition,
            self.types.clone(),
            signature,
            |anchor| {
                origin
                    .filter(|(expected, _, _)| *expected == anchor)
                    .map(|(_, path, occurrence)| (path.clone(), *occurrence))
            },
        );
        let (report, ran, acceptance) = hirsel_validate::validate_with_receipt(
            &artifact,
            &views,
            self.verifier_policy,
            self.verifier.as_deref(),
            &mut vec![],
        );
        if ran {
            hirsel_intern::timings::count(hirsel_intern::timings::Count::VRuns);
        }
        (report, acceptance)
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
            if self.signatures.contains_key(&d)
                || self.headers.as_ref().is_some_and(|h| h.contains(d))
                || self.lookup.builtins.contains(&d)
            {
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
        crate::queries::memos::optional(self.failed_signature_key(d))
    }
    pub(crate) fn failed_signature_key(&self, d: DefId) -> Option<ContentHash> {
        if let Some(product) = self.headers.as_ref().and_then(|headers| headers.product(d)) {
            return product.entry.as_ref().map(|entry| {
                ContentHash::of_parts(
                    "hirsel.failed-signature.v1",
                    [
                        entry.memo.text.0.as_slice(),
                        entry.memo.reads.digest().0.as_slice(),
                    ],
                )
            });
        }
        self.lookup.failed.get(&d).copied().flatten()
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
        if let Some(product) = self.headers.as_ref().and_then(|headers| headers.product(d)) {
            return Some(product.result.as_ref().map(|output| &output.value));
        }
        self.signatures.get(&d).map(Result::as_ref)
    }
    /// The view of `d` a body of `package` sees: an opaque type's private view
    /// inside its own package, its public signature everywhere else.
    pub fn body_view(
        &self,
        package: &PackageId,
        d: DefId,
    ) -> Option<Result<&SignatureRef, &QueryError>> {
        if &d.path().0.package == package
            && let Some(view) = self.private_view(d)
        {
            return Some(Ok(view));
        }
        self.signature(d)
    }
    fn private_view(&self, d: DefId) -> Option<&SignatureRef> {
        if let Some(view) = self.private.get(&d) {
            return Some(view);
        }
        let product = self.headers.as_ref()?.product(d)?;
        let public = &product.result.as_ref().ok()?.value;
        if public.metadata.visibility != hirsel_syntax::Visibility::PublicOpaque {
            return None;
        }
        let owner = product.entry.as_ref()?.products.representation.as_ref()?;
        Some(&owner.body_view(&self.types, public).0)
    }
    pub(crate) fn definitions(&self) -> Vec<DefId> {
        self.signatures
            .keys()
            .copied()
            .chain(
                self.headers
                    .iter()
                    .flat_map(|headers| headers.definitions()),
            )
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect()
    }
    /// Every public opaque definition read by a body of its defining package.
    pub(crate) fn opaque_definitions(&self) -> impl Iterator<Item = DefId> + '_ {
        self.definitions().into_iter().filter(|d| {
            self.signature(*d).is_some_and(|s| {
                s.is_ok_and(|s| s.metadata.visibility == hirsel_syntax::Visibility::PublicOpaque)
            })
        })
    }
    /// `d`'s view in its own package (an opaque type's private view) with
    /// V's digest of it: what compile-time reflection of `d` reads.
    pub fn representation(
        &self,
        d: DefId,
    ) -> Option<Result<(&SignatureRef, ValidationDigest), &QueryError>> {
        if let Some(state) = self.validation.get() {
            if let Some(view) = state
                .packages
                .get(&d.path().0.package)
                .and_then(|p| p.replacement(d))
            {
                return Some(Ok((view.signature(), view.digest())));
            }
            let view = state.views.get(d);
            return match self.signature(d)? {
                Ok(_) => view.map(|v| Ok((v.signature(), v.digest()))),
                Err(error) => Some(Err(error)),
            };
        }
        let view = if let Some(private) = self.private_view(d) {
            private
        } else {
            match self.signature(d)? {
                Ok(public) => public,
                Err(error) => return Some(Err(error)),
            }
        };
        let products = if self.private_view(d).is_some() {
            &self.private_views
        } else {
            &self.public_views
        };
        let product = products
            .get(&d)?
            .get_or_init(|| View::new(&self.types, view.clone(), None));
        Some(Ok((product.signature(), product.digest())))
    }
    /// V's verdicts on this snapshot's views.
    pub fn validated(&self) -> &ValidatedSignatures {
        &self.validation_state().validated
    }
    /// The universe V validates `package`'s bodies against.
    pub fn validation(&self, package: &PackageId) -> &dyn hirsel_validate::CheckedEnvironment {
        let state = self.validation_state();
        match state.packages.get(package) {
            Some(views) => views,
            None => &**state.bases.get(package).unwrap_or(&state.views),
        }
    }
    /// The complete input of a package's base V universe. Artifact overlays
    /// record this before replacing an owner, preserving their source context.
    pub fn validation_input_digest(&self, package: &PackageId) -> ContentHash {
        let state = self.validation_state();
        match state.packages.get(package) {
            Some(views) => views.input_digest(),
            None => state
                .bases
                .get(package)
                .unwrap_or(&state.views)
                .input_digest(),
        }
    }
    /// `package`'s universe with `signature` in place of its declared view:
    /// compile-time evaluation validates a materialized owner against it.
    pub fn validation_with(&self, package: &PackageId, signature: SignatureRef) -> PackageViews {
        let t = &self.types;
        let state = self.validation_state();
        let mut store =
            ValidatedSignatures::with_cache(self.verifier.clone(), self.verifier_policy);
        match state.packages.get(package) {
            Some(views) => views.replace(t, signature, &mut store, Some(&state.validated)),
            None => PackageViews::new(
                t,
                state.bases.get(package).unwrap_or(&state.views).clone(),
                [View::new(t, signature, None)],
                &mut store,
                Some(&state.validated),
            ),
        }
    }
}

impl hirsel_validate::ReplayEnvironment for SignatureSnapshot {
    fn base_digest(&self, package: &PackageId) -> Option<ContentHash> {
        self.definitions()
            .into_iter()
            .any(|id| id.path().0.package == *package)
            .then(|| self.validation_input_digest(package))
    }
    fn views(
        &self,
        package: &PackageId,
        overlay: Option<SignatureRef>,
    ) -> Option<Box<dyn SignatureLookup + '_>> {
        self.base_digest(package)?;
        Some(match overlay {
            Some(signature) => Box::new(self.validation_with(package, signature)),
            None => Box::new(self.validation(package)),
        })
    }
}

impl QueryEngine {
    /// This revision's snapshot, built once after its inputs changed. It is
    /// kept only when every signature normalized, so a failed one is retried.
    pub fn signature_snapshot(&mut self) -> Arc<SignatureSnapshot> {
        let snapshot = self.signature_inputs();
        snapshot.validation_state();
        self.adopt_completed_headers();
        snapshot
    }
    /// P needs immutable signatures and views, without eagerly validating V's universe.
    pub(crate) fn signature_inputs(&mut self) -> Arc<SignatureSnapshot> {
        self.adopt_completed_headers();
        self.prepare_signature_snapshot();
        let stamp = self.snapshot_stamp();
        if let Some((kept, snapshot)) = &self.snapshot
            && *kept == stamp
            && !snapshot
                .headers
                .as_ref()
                .is_some_and(|headers| headers.completed().any(|(_, p)| p.result.is_err()))
        {
            return snapshot.clone();
        }
        let headers = crate::queries::header_products::HeaderProducts::capture(self);
        let row_sources = self
            .source_paths()
            .into_iter()
            .filter_map(|path| {
                let id = DefId::intern(&path);
                self.unmarked_signature(id)?;
                self.signature_memo_source(id)
                    .map(|entry| (id, Arc::downgrade(entry)))
            })
            .collect();
        let mut signatures: Vec<_> = self
            .registry
            .signatures()
            .map(|s| (s.definition, Ok(s.clone())))
            .collect();
        for (path, constant) in &self.staged_constants {
            signatures.push((DefId::intern(path), Ok(constant.signature.clone())));
        }
        self.snapshot_epoch = self.snapshot_epoch.next();
        let previous = self.snapshot.as_ref().map(|(_, s)| s.clone());
        let lookup = LookupInput {
            package_dependencies: self.declarations.inputs().package_dependencies.clone(),
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
                .map(|(d, _)| (*d, self.failed_signature_key(d.path())))
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
                [],
                previous.as_deref(),
            )
            .with_headers(headers)
            .with_lookup(lookup)
            .with_verifier(
                self.result_store.as_ref().and_then(|s| s.verifier()),
                ContentHash::of_parts(
                    "hirsel.validate.private-evaluation.v1",
                    [
                        self.environment_value().0.as_slice(),
                        self.store_identity.0.as_slice(),
                    ],
                ),
            ),
        );
        self.snapshot = Some((stamp, snapshot.clone()));
        self.snapshot_sources = row_sources;
        snapshot
    }
    /// Observe a published signature without another phase memo query. The
    /// snapshot must describe the current universe and the immutable producer
    /// must still be the exact unmarked source that supplied its row.
    pub(crate) fn current_snapshot_signature(&self, d: DefId) -> Option<SignatureRef> {
        let (stamp, _) = self.snapshot.as_ref()?;
        if *stamp != self.snapshot_stamp() {
            return None;
        }
        self.retained_snapshot_signature(d)?.ok()
    }

    fn retained_snapshot_signature(&self, d: DefId) -> Option<Result<SignatureRef, QueryError>> {
        let source = self.snapshot_sources.get(&d)?.upgrade()?;
        let current = self.signature_memo_source(d)?;
        if !Arc::ptr_eq(&source, current)
            || !current.consistent
            || self.graph.is_dirty(&current.memo.key)
        {
            return None;
        }
        self.snapshot
            .as_ref()?
            .1
            .signature(d)
            .map(|row| row.cloned().map_err(Clone::clone))
    }

    pub(crate) fn unmarked_signature(&self, d: DefId) -> Option<Result<SignatureRef, QueryError>> {
        let current = self.signature_memo_source(d)?;
        if !current.consistent
            || self.graph.is_dirty(&current.memo.key)
            || !self.graph.memos_of(d).any(|key| *key == current.memo.key)
        {
            return None;
        }
        // A source-index carry owns the current header memo and its reverse
        // reads even when it did not retain a frozen signature universe. The
        // unmarked producer is authoritative; rebuilding a view need not query
        // or stamp every unchanged header again.
        Some(current.result().map(|output| output.value.clone()))
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
}

/// The inputs a kept snapshot was built from.
#[derive(PartialEq)]
pub(crate) struct SnapshotStamp {
    declarations: u64,
    revision: u64,
    environment: QueryEnvironment,
    limits: crate::ComptimeLimits,
    manifest: Option<ContentHash>,
    unavailable: Option<QueryKind>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memo::{ReadKey, ReadsBuilder};
    use hirsel_syntax::ItemKind;
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

    #[test]
    fn package_validation_universes_cover_selected_dependencies_without_importers() {
        let types = TypeInterner::new(None);
        let dependency = PackageId {
            name: "dependency".into(),
            version: "1".into(),
        };
        let middle = PackageId {
            name: "middle".into(),
            version: "1".into(),
        };
        let application = PackageId {
            name: "application".into(),
            version: "1".into(),
        };
        let row = |package: &PackageId, name: &str, primitive| {
            let mut row = (*constant(&types, name, primitive)).clone();
            let mut path = row.definition.path().clone();
            path.0.package = package.clone();
            row.definition = DefId::intern(&path);
            Arc::new(row)
        };
        let dep = row(&dependency, "Dep", hirsel_types::Primitive::U64);
        let mid = row(&middle, "Mid", hirsel_types::Primitive::U64);
        let app = row(&application, "App", hirsel_types::Primitive::U64);
        let core = constant(&types, "Core", hirsel_types::Primitive::U64);
        let dependencies = BTreeMap::from([
            (dependency.clone(), vec![]),
            (middle.clone(), vec![dependency.clone()]),
            (application.clone(), vec![middle.clone()]),
        ]);
        let snapshot = |dep: SignatureRef, extra: Option<SignatureRef>, dependencies| {
            let rows = [core.clone(), dep, mid.clone(), app.clone()]
                .into_iter()
                .chain(extra);
            SignatureSnapshot::build(
                Revision::FIRST,
                types.clone(),
                rows.map(|row| (row.definition, Ok(row))),
                [],
                None,
            )
            .with_lookup(LookupInput {
                package_dependencies: dependencies,
                ..Default::default()
            })
        };
        let first = snapshot(dep.clone(), None, dependencies.clone());
        assert!(first.validation(&dependency).view(app.definition).is_none());
        assert!(first.validation(&middle).view(dep.definition).is_some());
        assert!(
            first
                .validation(&dependency)
                .view(core.definition)
                .is_some()
        );
        let extra = row(&application, "Extra", hirsel_types::Primitive::U8);
        let added = snapshot(dep.clone(), Some(extra.clone()), dependencies.clone());
        assert_eq!(
            first.validation_input_digest(&dependency),
            added.validation_input_digest(&dependency)
        );
        assert_eq!(
            first.validation_input_digest(&middle),
            added.validation_input_digest(&middle)
        );
        assert_ne!(
            first.validation_input_digest(&application),
            added.validation_input_digest(&application)
        );
        let mut changed = (*dep).clone();
        changed.kind = SignatureKind::Const {
            ty: types.intern(Type::with_kind(TypeKind::Primitive(
                hirsel_types::Primitive::U8,
            ))),
            generics: Default::default(),
        };
        assert_eq!(
            changed.fingerprint, dep.fingerprint,
            "producer fingerprints do not confer V authority"
        );
        let changed = snapshot(Arc::new(changed), Some(extra), dependencies.clone());
        for package in [&dependency, &middle, &application] {
            assert_ne!(
                added.validation_input_digest(package),
                changed.validation_input_digest(package),
                "a valid changed selected row invalidates its complete universe"
            );
        }
        let mut incomplete = dependencies;
        incomplete.remove(&dependency);
        let incomplete = snapshot(dep, None, incomplete);
        assert!(
            incomplete
                .validation(&middle)
                .view(app.definition)
                .is_some(),
            "incomplete package inputs retain full-world validation"
        );
    }

    #[test]
    fn signature_inputs_demand_only_the_observed_header_until_a_universe_consumer_reads_all() {
        struct Header(Arc<Mutex<Vec<DefPath>>>);
        impl SignaturePhase for Header {
            fn signature(
                &self,
                types: &TypeInterner,
                _: &hirsel_types::TypeMemo,
                syntax: Arc<ItemSyntax>,
                _: &Declarations,
                _: &mut PhaseProducts,
                _: &mut dyn SignatureProvider,
            ) -> Result<QueryOutput<Signature>, QueryError> {
                self.0.lock().unwrap().push(syntax.item.definition.clone());
                Ok(crate::queries::output(Signature {
                    metadata: Default::default(),
                    definition: DefId::intern(&syntax.item.definition),
                    kind: SignatureKind::Const {
                        ty: types.intern(Type::unit()),
                        generics: Default::default(),
                    },
                    fingerprint: syntax.text_hash,
                }))
            }
        }
        crate::with_compiler_stack(|| {
            let trace = Arc::new(Mutex::new(Vec::new()));
            let mut adapters = crate::phases::engine_only_adapters();
            adapters.signature = Some(Arc::new(Header(trace.clone())));
            let mut engine = QueryEngine::new(default_environment(), adapters);
            let context = default_context();
            let file = engine
                .sources
                .add(
                    "demand.hir",
                    r#"extern "used" fn used()
extern "unused" fn unused()"#
                        .into(),
                )
                .unwrap();
            let parsed = engine.parse_file(file, &context).unwrap();
            let used = parsed.value.items[0].item.definition.clone();
            let unused = parsed.value.items[1].item.definition.clone();
            let snapshot = engine.signature_inputs();
            assert!(trace.lock().unwrap().is_empty());
            let first = snapshot.signature(DefId::intern(&used)).unwrap().unwrap();
            let second = snapshot.signature(DefId::intern(&used)).unwrap().unwrap();
            assert!(Arc::ptr_eq(first, second));
            assert_eq!(*trace.lock().unwrap(), vec![used]);
            snapshot.validation(&context.package);
            assert!(trace.lock().unwrap().contains(&unused));
        });
    }

    #[test]
    fn archived_header_demand_does_not_hold_the_written_body_arenas() {
        struct Header;
        impl SignaturePhase for Header {
            fn signature(
                &self,
                types: &TypeInterner,
                _: &hirsel_types::TypeMemo,
                syntax: Arc<ItemSyntax>,
                _: &Declarations,
                _: &mut PhaseProducts,
                _: &mut dyn SignatureProvider,
            ) -> Result<QueryOutput<Signature>, QueryError> {
                Ok(crate::queries::output(Signature {
                    metadata: Default::default(),
                    definition: DefId::intern(&syntax.item.definition),
                    kind: SignatureKind::Const {
                        ty: types.intern(Type::unit()),
                        generics: Default::default(),
                    },
                    fingerprint: syntax.text_hash,
                }))
            }
        }
        crate::with_compiler_stack(|| {
            let mut adapters = crate::phases::engine_only_adapters();
            adapters.signature = Some(Arc::new(Header));
            let store = Arc::new(crate::store::MemStore::default());
            let mut engine =
                QueryEngine::new(default_environment(), adapters).with_result_store(store.clone());
            let context = default_context();
            let file = engine
                .sources
                .add(
                    "archived.hir",
                    "fn used() { () }\nfn unused() { () }".into(),
                )
                .unwrap();
            let parsed = engine.parse_file(file, &context).unwrap();
            let path = parsed.value.items[0].item.definition.clone();
            let written = Arc::downgrade(&parsed.value.items[0]);
            let snapshot = engine.signature_inputs();
            snapshot
                .header_products()
                .unwrap()
                .archive_sources(store)
                .unwrap();
            drop(parsed);
            drop(engine);
            assert!(
                written.upgrade().is_none(),
                "a demand source retains only ordinary archive handles"
            );
            assert!(snapshot.signature(DefId::intern(&path)).unwrap().is_ok());
        });
    }

    /// Receipt observation uses a settled immutable row without touching its
    /// phase memo, but a changed header must still be queried before reuse.
    #[test]
    fn live_observation_reuses_a_settled_snapshot_and_requeries_an_edited_header() {
        struct Header;
        impl SignaturePhase for Header {
            fn signature(
                &self,
                t: &TypeInterner,
                _: &hirsel_types::TypeMemo,
                syntax: Arc<ItemSyntax>,
                _: &Declarations,
                _: &mut PhaseProducts,
                _: &mut dyn SignatureProvider,
            ) -> Result<QueryOutput<Signature>, QueryError> {
                let ItemKind::Function { abi: Some(abi), .. } = &syntax.item.kind else {
                    panic!("header fixture")
                };
                Ok(QueryOutput {
                    value: Signature {
                        metadata: Default::default(),
                        definition: DefId::intern(&syntax.item.definition),
                        kind: SignatureKind::Const {
                            ty: t.intern(Type::unit()),
                            generics: Default::default(),
                        },
                        fingerprint: ContentHash::of_bytes(abi.literal.as_bytes()),
                    },
                    dependencies: vec![],
                    diagnostics: vec![],
                })
            }
        }
        let mut adapters = crate::phases::engine_only_adapters();
        adapters.signature = Some(Arc::new(Header));
        let mut engine = QueryEngine::new(default_environment(), adapters);
        let context = default_context();
        let file = engine
            .sources
            .add("snapshot.hir", "extern \"first\" fn probe()".into())
            .unwrap();
        let parsed = engine.parse_file(file, &context).unwrap();
        let owner = parsed.value.items[0].item.definition.clone();
        let key = ReadKey::Signature(DefId::intern(&owner));
        let snapshot = engine.signature_snapshot();
        let expected = crate::queries::memos::signature_value(Some(
            snapshot
                .signature(DefId::intern(&owner))
                .unwrap()
                .unwrap()
                .fingerprint,
        ));
        let before = engine.memo_stats().touched;
        assert_eq!(engine.observe(&owner, &key), Some(expected));
        let executed = ReadKey::ExecutedSignature(DefId::intern(&owner));
        assert_eq!(engine.observe(&owner, &executed), Some(expected));
        assert_eq!(
            engine.memo_stats().touched,
            before,
            "settled observation reads the snapshot, not phase memos"
        );
        let budget = crate::runtime::ProcessBudget::new(64 << 20);
        let bytes = engine.source_index_bytes().unwrap();
        let state = engine
            .into_source_index(budget.reserve(bytes).unwrap())
            .unwrap();
        let mut adapters = crate::phases::engine_only_adapters();
        adapters.signature = Some(Arc::new(Header));
        let mut engine = QueryEngine::new(default_environment(), adapters);
        assert!(state.restore(&mut engine).is_ok());
        assert_eq!(
            engine.memo_stats().touched,
            0,
            "a fresh engine counts only new work"
        );
        let before = engine.memo_stats().touched;
        let rebuilt = engine.signature_snapshot();
        assert_eq!(
            rebuilt
                .signature(DefId::intern(&owner))
                .unwrap()
                .unwrap()
                .fingerprint,
            snapshot
                .signature(DefId::intern(&owner))
                .unwrap()
                .unwrap()
                .fingerprint,
        );
        assert_eq!(
            engine.memo_stats().touched,
            before,
            "source-only carry rebuilds the universe without touching unchanged headers"
        );

        let file = engine
            .sources
            .add("snapshot.hir", "extern \"second\" fn probe()".into())
            .unwrap();
        engine.parse_file(file, &context).unwrap();
        let changed = engine.observe(&owner, &key).unwrap();
        assert_ne!(
            changed, expected,
            "an old snapshot cannot authorize a changed producer"
        );
        let current = engine.signature_snapshot();
        assert_eq!(
            changed,
            crate::queries::memos::signature_value(Some(
                current
                    .signature(DefId::intern(&owner))
                    .unwrap()
                    .unwrap()
                    .fingerprint
            ))
        );
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

    fn cached_views(
        types: Arc<TypeInterner>,
        public: Vec<SignatureRef>,
        private: Vec<SignatureRef>,
        cache: Arc<hirsel_validate::VerifierCache>,
    ) -> SignatureSnapshot {
        let snapshot = SignatureSnapshot::build(
            Revision::FIRST,
            types,
            public
                .into_iter()
                .map(|signature| (signature.definition, Ok(signature))),
            private,
            None,
        )
        .with_verifier(
            Some(cache),
            ContentHash::of_bytes(b"signature-view-test-policy"),
        );
        snapshot.validation_state();
        snapshot
    }
    #[test]
    fn reflection_and_validation_share_only_the_exact_immutable_view_product() {
        let types = TypeInterner::new(None);
        let signature = constant(&types, "shared", hirsel_types::Primitive::U64);
        let d = signature.definition;
        let build = |signature: SignatureRef, previous: Option<&SignatureSnapshot>| {
            SignatureSnapshot::build(
                Revision::FIRST,
                types.clone(),
                [(d, Ok(signature))],
                [],
                previous,
            )
        };
        let first = build(signature.clone(), None);
        let reflected = first.representation(d).unwrap().unwrap().1;
        let product = first.public_views[&d].get().unwrap().identified().clone();
        let state = first.validation_state();
        assert!(Arc::ptr_eq(
            &product,
            state.views.get(d).unwrap().identified()
        ));
        assert_eq!(reflected, state.views.get(d).unwrap().digest());
        let next = build(signature.clone(), Some(&first));
        let next_state = next.validation_state();
        assert!(Arc::ptr_eq(
            &product,
            next_state.views.get(d).unwrap().identified()
        ));

        let mut changed = (*signature).clone();
        changed.kind = SignatureKind::Const {
            ty: types.intern(Type::with_kind(TypeKind::Error)),
            generics: Default::default(),
        };
        let changed = build(Arc::new(changed), Some(&next));
        let changed_state = changed.validation_state();
        assert!(!Arc::ptr_eq(
            &product,
            changed_state.views.get(d).unwrap().identified()
        ));
        assert_ne!(reflected, changed_state.views.get(d).unwrap().digest());
        assert!(!changed_state.views.universe_errors().1.is_empty());
    }

    #[test]
    fn durable_verdicts_recompute_independent_digests_and_reject_changed_views() {
        use hirsel_intern::work::{self, Counter};
        let cache = Arc::new(hirsel_validate::VerifierCache::default());
        let types = TypeInterner::new(None);
        let signature = constant(&types, "durable", hirsel_types::Primitive::U64);
        let snapshot = cached_views(
            types.clone(),
            vec![signature.clone()],
            vec![],
            cache.clone(),
        );
        assert!(!snapshot.validated().is_empty());
        // Ordinary view bytes cross interners; only the shared local verifier
        // capability retains proof. No producer seed carries success claims.
        let views = vec![signature.clone()];
        let (_, roots) =
            hirsel_types::with_table_collection(|| serde_json::to_vec(&views).unwrap());
        let (table, _) = hirsel_types::encode_table(&types, &roots);
        let bytes = hirsel_types::with_table_encoding(&types, &roots, || {
            serde_json::to_vec(&views).unwrap()
        });
        let other = TypeInterner::new(None);
        other.intern(Type::with_kind(TypeKind::Primitive(
            hirsel_types::Primitive::U8,
        )));
        let ids = hirsel_types::decode_table(&other, &table);
        let mut decoded: Vec<SignatureRef> =
            hirsel_types::with_table_decoding(&ids, || serde_json::from_slice(&bytes).unwrap());
        let before = work::read(Counter::ValidateSignatureRows);
        let imported = cached_views(other.clone(), decoded.clone(), vec![], cache.clone());
        assert_eq!(work::read(Counter::ValidateSignatureRows), before);
        assert!(
            imported
                .validation(&signature.definition.path().0.package)
                .universe_errors()
                .1
                .is_empty()
        );
        let changed = Arc::make_mut(&mut decoded[0]);
        let fingerprint = changed.fingerprint;
        let SignatureKind::Const { ty, .. } = &mut changed.kind else {
            unreachable!()
        };
        *ty = other.intern(Type::with_kind(TypeKind::Error));
        assert_eq!(changed.fingerprint, fingerprint);
        let changed = cached_views(other, decoded, vec![], cache);
        assert_eq!(work::read(Counter::ValidateSignatureRows), before + 1);
        assert!(
            !changed
                .validation(&signature.definition.path().0.package)
                .universe_errors()
                .1
                .is_empty()
        );
        assert_eq!(changed.validated().successful().count(), 0);
    }

    #[test]
    fn durable_private_views_reject_shape_changes_without_public_fingerprint_changes() {
        use hirsel_intern::work::{self, Counter};
        use hirsel_types::{Allocation, AuthoritySummary, Bound, Layout, TypeDefinition};
        let types = TypeInterner::new(None);
        let mut public = (*constant(&types, "Opaque", hirsel_types::Primitive::U64)).clone();
        public.metadata.visibility = hirsel_syntax::Visibility::PublicOpaque;
        public.kind = SignatureKind::Type(Box::new(TypeDefinition {
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
        }));
        let mut private = public.clone();
        let SignatureKind::Type(declaration) = &mut private.kind else {
            unreachable!()
        };
        declaration.representation = Some(types.intern(Type::with_kind(TypeKind::Primitive(
            hirsel_types::Primitive::U64,
        ))));
        let public = Arc::new(public);
        let cache = Arc::new(hirsel_validate::VerifierCache::default());
        let mut private = Arc::new(private);
        let snapshot = cached_views(
            types.clone(),
            vec![public.clone()],
            vec![private.clone()],
            cache.clone(),
        );
        assert_eq!(snapshot.private.len(), 1);
        let before = work::read(Counter::ValidateSignatureRows);
        let imported = cached_views(
            types.clone(),
            vec![public.clone()],
            vec![private.clone()],
            cache.clone(),
        );
        assert_eq!(work::read(Counter::ValidateSignatureRows), before);
        let package = &public.definition.path().0.package;
        assert!(imported.validation(package).universe_errors().1.is_empty());
        let changed_private = Arc::make_mut(&mut private);
        let SignatureKind::Type(declaration) = &mut changed_private.kind else {
            unreachable!()
        };
        // String fails the declared plain bound; the unchanged public view
        // still has a reusable pass, while this private view must be checked.
        declaration.representation = Some(types.intern(Type::with_kind(TypeKind::Primitive(
            hirsel_types::Primitive::String,
        ))));
        assert_eq!(changed_private.fingerprint, public.fingerprint);
        let changed = cached_views(types, vec![public.clone()], vec![private], cache);
        assert_eq!(work::read(Counter::ValidateSignatureRows), before + 1);
        assert!(!changed.validation(package).universe_errors().1.is_empty());
        assert_eq!(changed.validated().successful().count(), 1);
    }
    #[test]
    fn local_verdicts_reuse_without_granting_declaration_authority() {
        use hirsel_intern::work::{self, Counter};
        let store = Arc::new(crate::store::MemStore::default());
        let mut original =
            QueryEngine::new(default_environment(), crate::phases::engine_only_adapters())
                .with_result_store(store.clone());
        original.signature_snapshot();
        let injected = constant(
            original.types(),
            "not_declared",
            hirsel_types::Primitive::U64,
        );
        let mut imported =
            QueryEngine::new(default_environment(), crate::phases::engine_only_adapters())
                .with_result_store(store);
        let before = work::read(Counter::ValidateSignatureRows);
        let snapshot = imported.signature_snapshot();
        assert_eq!(work::read(Counter::ValidateSignatureRows), before);
        assert!(snapshot.signature(injected.definition).is_none());
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
    #[test]
    fn frozen_citations_select_occurrences_and_observe_current_body_text() {
        let mut engine =
            QueryEngine::new(default_environment(), crate::phases::engine_only_adapters());
        let context = default_context();
        for (file, source) in [
            ("left.hir", "fn target() {}\nfn left() {}\n"),
            ("right.hir", "fn target() {}\nfn right() {}\n"),
        ] {
            let file = engine.sources.add(file, source.into()).unwrap();
            engine.parse_file(file, &context).unwrap();
        }
        let path = |name: &str| {
            let mut p = crate::entry_path(&context);
            p.0.name = name.into();
            DefId::intern(&p)
        };
        let target = path("target");
        let snapshot = engine.signature_snapshot();
        let source = engine.citation_source();
        let mut receipts = vec![];
        let mut spans = vec![];
        for owner in [path("left"), path("right")] {
            let mut reads = ReadsBuilder::new();
            let mut provider =
                FrozenProvider::for_definition(&snapshot, owner, &source, &mut reads);
            let span = provider.cite_name(target.path()).unwrap();
            assert_eq!(Some(span), source.cited_name(owner, target));
            spans.push(span);
            let reads = reads.finish().unwrap();
            receipts.push(
                reads
                    .iter()
                    .find_map(|(key, value)| (*key == ReadKey::CitedText(target)).then_some(*value))
                    .unwrap(),
            );
        }
        assert_ne!(spans[0], spans[1]);
        assert_ne!(
            engine.sources.resolve(spans[0]).file,
            engine.sources.resolve(spans[1]).file
        );
        assert_eq!(receipts[0], receipts[1]);
        let before = receipts[0];
        drop(source);
        let file = engine
            .sources
            .add("right.hir", "fn target() { 1 }\nfn right() {}\n".into())
            .unwrap();
        engine.parse_file(file, &context).unwrap();
        let source = engine.citation_source();
        let mut reads = ReadsBuilder::new();
        // The retained snapshot is intentionally the earlier signature view.
        let mut provider =
            FrozenProvider::for_definition(&snapshot, path("right"), &source, &mut reads);
        assert!(provider.cite_name(target.path()).is_some());
        let reads = reads.finish().unwrap();
        let now = reads
            .iter()
            .find_map(|(key, value)| (*key == ReadKey::CitedText(target)).then_some(*value))
            .unwrap();
        assert_ne!(before, now);
        assert_eq!(now, source.cited_text(target));
    }
}

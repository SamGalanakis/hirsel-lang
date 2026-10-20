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
pub use provider::FrozenProvider;
pub(crate) use declarations::header_hash;
pub use declarations::{DeclarationIndex, Declarations, ModuleInputs};

pub use views::{PackageViews, View, ViewTable};

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
        let _timing = hirsel_intern::timings::phase(hirsel_intern::timings::Phase::Verify);
        let t = &*types;
        let signatures: DefMap<_> = signatures.into_iter().collect();
        let private: DefMap<SignatureRef> = private
            .into_iter()
            .map(|view| (view.definition, view))
            .collect();
        let old = previous.map(|p| &p.validated);
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
        let paths: Vec<_> = self.items.keys().cloned().collect();
        for path in paths {
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
            candidates: self
                .items
                .keys()
                .filter_map(|path| {
                    Some((DefId::intern(path), self.declarations.candidates(path)?))
                })
                .collect(),
        };
        let snapshot = Arc::new(SignatureSnapshot::new(
            self.snapshot_epoch,
            self.types.clone(),
            signatures,
            private,
            previous.as_deref(),
        ).with_lookup(lookup));
        if complete {
            self.snapshot = Some((stamp, snapshot.clone()));
        }
        snapshot
    }
    /// The inputs a snapshot built now would be built from.
    pub(crate) fn snapshot_stamp(&self) -> SnapshotStamp {
        SnapshotStamp {
            revision: self.graph.revision().get(),
            environment: self.environment.clone(),
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
        let frozen = self.declarations.frozen();
        if let Some(state) = &self.adapters.compiler_state {
            let mut state = state.borrow_mut();
            if !state
                .declarations
                .as_ref()
                .is_some_and(|old| Arc::ptr_eq(old, &frozen))
            {
                state.declarations = Some(frozen.clone());
            }
        }
        frozen
    }
    /// Signatures changed outside a source revision (a staged constant).
    pub(crate) fn retire_snapshot(&mut self) {
        if let Some((stamp, _)) = &mut self.snapshot {
            stamp.revision = u64::MAX;
        }
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
    revision: u64,
    environment: QueryEnvironment,
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
            Revision::from_u64(1),
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
            Revision::from_u64(1),
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
}

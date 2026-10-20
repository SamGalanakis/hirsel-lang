//! The frozen signature snapshot of one revision (design §1(b), "Snapshot").
//!
//! A revision computes its snapshot once: every signature by `DefId`, the
//! private views each opaque type's own package sees, and V's verdict on every
//! view with each package's universe errors. It is immutable, `Send + Sync`
//! and stamped with its epoch, so definitions read it without locks.
use crate::memo::Revision;
use crate::*;
use hirsel_syntax::{DefId, DefMap, PackageId};
use hirsel_types::{SignatureRef, TypeInterner};
use hirsel_validate::{SignatureLookup, ValidatedSignatures};
use std::collections::BTreeMap;
use std::sync::Arc;

mod declarations;
mod views;
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
        }
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
        let manifest = Some(self.declarations.inputs().manifest_revision);
        let stamp = SnapshotStamp {
            revision: self.source_revision,
            environment: self.environment.clone(),
            manifest,
            unavailable: self.unavailable,
        };
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
        self.with_validation_walk(|engine| {
            for path in paths {
                let d = DefId::intern(&path);
                let signature = engine.signature(d);
                if let Ok(public) = &signature {
                    match engine.body_signature(&path.0.package, public.clone()) {
                        Ok((view, _)) if !Arc::ptr_eq(&view, public) => private.push(view),
                        Ok(_) => {}
                        Err(_) => complete = false,
                    }
                } else {
                    complete = false;
                }
                signatures.push((d, signature));
            }
        });
        for (path, constant) in &self.staged_constants {
            signatures.push((DefId::intern(path), Ok(constant.signature.clone())));
        }
        self.snapshot_epoch = self.snapshot_epoch.next();
        let previous = self.snapshot.as_ref().map(|(_, s)| s.clone());
        let snapshot = Arc::new(SignatureSnapshot::new(
            self.snapshot_epoch,
            self.types.clone(),
            signatures,
            private,
            previous.as_deref(),
        ));
        if complete {
            self.snapshot = Some((stamp, snapshot.clone()));
        }
        snapshot
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

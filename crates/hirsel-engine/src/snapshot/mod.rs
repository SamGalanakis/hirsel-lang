//! The frozen signature snapshot of one revision (design §1(b), "Snapshot").
//!
//! A revision computes its snapshot once: every signature by `DefId`, the
//! private views each opaque type's own package sees, and V's verdict on every
//! view with each package's universe errors. It is immutable, `Send + Sync`
//! and stamped with its epoch, so definitions read it without locks.
use crate::*;
use hirsel_syntax::{DefId, DefMap, PackageId};
use hirsel_types::SignatureRef;
use hirsel_validate::{SignatureLookup, ValidatedSignatures};
use std::collections::BTreeMap;
use std::sync::Arc;

mod views;
pub use views::{PackageViews, View, ViewTable};

/// One revision's signatures, private views and V verdicts.
pub struct SignatureSnapshot {
    epoch: u64,
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
        epoch: u64,
        signatures: impl IntoIterator<Item = (DefPath, Result<SignatureRef, QueryError>)>,
        private: impl IntoIterator<Item = SignatureRef>,
        previous: Option<&SignatureSnapshot>,
    ) -> Self {
        let _timing = hirsel_intern::timings::phase(hirsel_intern::timings::Phase::Verify);
        let signatures: DefMap<_> = signatures
            .into_iter()
            .map(|(path, signature)| (DefId::intern(&path), signature))
            .collect();
        let private: DefMap<SignatureRef> = private
            .into_iter()
            .map(|view| (DefId::intern(&view.definition), view))
            .collect();
        let old = previous.map(|p| &p.validated);
        let mut validated = ValidatedSignatures::default();
        let views = Arc::new(ViewTable::new(
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
                .push(View::new(view.clone(), earlier));
        }
        let packages = owned
            .into_iter()
            .map(|(package, replaced)| {
                let views = PackageViews::new(views.clone(), replaced, &mut validated, old);
                (package, views)
            })
            .collect();
        Self {
            epoch,
            signatures,
            private,
            views,
            packages,
            validated,
        }
    }
    pub fn epoch(&self) -> u64 {
        self.epoch
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
        let mut store = ValidatedSignatures::default();
        match self.packages.get(package) {
            Some(views) => views.replace(signature, &mut store, Some(&self.validated)),
            None => PackageViews::new(
                self.views.clone(),
                [View::new(signature, None)],
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
        let manifest = self
            .adapters
            .compiler_state
            .as_ref()
            .map(|s| s.borrow().manifest_revision);
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
        let mut signatures: Vec<(DefPath, Result<SignatureRef, QueryError>)> = self
            .registry
            .signatures()
            .map(|s| (s.definition.clone(), Ok(s.clone())))
            .collect();
        let mut private = vec![];
        let mut complete = true;
        let paths: Vec<_> = self.items.keys().cloned().collect();
        self.with_validation_walk(|engine| {
            for path in paths {
                let signature = engine.signature(&path);
                if let Ok(public) = &signature {
                    match engine.body_signature(&path.0.package, public.clone()) {
                        Ok((view, _)) if !Arc::ptr_eq(&view, public) => private.push(view),
                        Ok(_) => {}
                        Err(_) => complete = false,
                    }
                } else {
                    complete = false;
                }
                signatures.push((path, signature));
            }
        });
        for (path, constant) in &self.staged_constants {
            signatures.push((path.clone(), Ok(constant.signature.clone())));
        }
        self.snapshot_epoch += 1;
        let previous = self.snapshot.as_ref().map(|(_, s)| s.clone());
        let snapshot = Arc::new(SignatureSnapshot::new(
            self.snapshot_epoch,
            signatures,
            private,
            previous.as_deref(),
        ));
        if complete {
            self.snapshot = Some((stamp, snapshot.clone()));
        }
        snapshot
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
        validate: impl FnOnce(&dyn SignatureLookup) -> hirsel_validate::Validation,
    ) -> hirsel_validate::Validation {
        match self.verify {
            Verify::On => validate(self.signature_snapshot().validation(package)),
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

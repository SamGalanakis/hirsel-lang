//! The S0 signature inventory needed by checked-product encoding. Fingerprint
//! coverage is captured explicitly; an inherited body read log is not coverage.
use super::*;
use crate::runtime::MemoryReservation;
use hirsel_proof_export::SignatureSection;

struct Input {
    public: hirsel_types::SignatureRef,
    owner: Option<hirsel_types::SignatureRef>,
    coverage: Vec<SignatureDependency>,
}
/// Immutable header inputs bound to the same ordinary front producer as Check.
/// Shared signature payloads remain owned by that producer's snapshot grant.
/// This contains neither checked bodies nor permission to demand one.
pub struct FrontCheckedSignatures {
    inputs: BTreeMap<DefPath, Input>,
    front: Arc<FrontBodyQueries>,
    environment: hirsel_proof_export::Environment,
    registrations: Vec<Vec<u8>>,
    _allowance: MemoryReservation,
}
impl FrontCheckedSignatures {
    /// Canonical records of the accepted S0 namespace bound to this producer.
    /// Later registration transactions require a new producer and inventory.
    pub fn registrations(&self) -> &[Vec<u8>] {
        &self.registrations
    }
    pub fn environment(&self) -> &hirsel_proof_export::Environment {
        &self.environment
    }
    pub fn signatures(&self) -> impl Iterator<Item = (&DefPath, &hirsel_types::SignatureRef)> {
        self.inputs
            .iter()
            .map(|(path, input)| (path, &input.public))
    }
    pub fn owner_view(&self, path: &DefPath) -> Option<&hirsel_types::SignatureRef> {
        self.inputs.get(path)?.owner.as_ref()
    }
    pub fn coverage(&self, path: &DefPath) -> Option<&[SignatureDependency]> {
        self.inputs.get(path).map(|input| input.coverage.as_slice())
    }
    /// Match the exact source producer, rather than only its signature epoch:
    /// a body-only edit can legitimately reuse the old signature snapshot.
    pub fn matches(&self, front: &Arc<FrontBodyQueries>) -> bool {
        Arc::ptr_eq(&self.front, front)
    }
    /// Format one already captured section in the caller's admitted scratch.
    /// No signature query, source read or compiler phase runs here.
    pub fn section(&self, path: &DefPath) -> Option<SignatureSection> {
        let input = self.inputs.get(path)?;
        Some(SignatureSection {
            path: path.clone(),
            dependencies: input.coverage.clone(),
            public: (*input.public).clone(),
            owner_view: input.owner.as_ref().map(|owner| (**owner).clone()),
        })
    }
}
impl QueryEngine {
    /// Capture the exact header and private-view inputs while S0 still owns the
    /// loader. Failed signatures stay absent, as in ordinary checked_products;
    /// the completed Check scope separately retains their failure reads.
    pub fn front_checked_signatures(
        &mut self,
        front: Arc<FrontBodyQueries>,
        allowance: MemoryReservation,
    ) -> Result<Arc<FrontCheckedSignatures>, QueryError> {
        let invalid = || {
            QueryError::Operational(hirsel_types::OperationalFailure {
                operation: "checked signature inputs".into(),
                message: "signature inventory exceeds admission or differs from the selected front"
                    .into(),
            })
        };
        let snapshot = front.signatures();
        if !Arc::ptr_eq(&snapshot, &self.signature_inputs()) {
            return Err(invalid());
        }
        // The accepted generated/staged namespace is a separate input. Until
        // its ledger is supplied, it must never be silently exported as empty.
        if !self.generated.is_empty()
            || !self.staged_constants.is_empty()
            || !self.staged_values.is_empty()
        {
            return Err(QueryError::NotImplemented);
        }
        let paths: BTreeSet<_> = self
            .source_paths()
            .into_iter()
            .chain(self.builtins.keys().cloned())
            .chain(
                self.registry
                    .signatures()
                    .map(|s| s.definition.path().clone()),
            )
            .collect();
        let mut weight = crate::NativeMetadataWeight::default();
        weight.add(size_of::<FrontCheckedSignatures>() + 2 * size_of::<usize>());
        weight.tree::<DefPath, Input>(paths.len());
        let environment = self.proof_environment();
        let registrations = self.checked_registration_records();
        weight.vec(&registrations, |w, record| w.add(record.capacity()));
        weight.add(environment.compiler_version.capacity());
        if weight.bytes() > allowance.bytes() {
            return Err(invalid());
        }
        let mut inputs = BTreeMap::new();
        // Custom source-less builtins can be absent from the frozen public
        // table. Their retained Arcs need this owner's grant independently.
        let mut owned_signatures = crate::memo::retained::Weight::default();
        for path in paths {
            let id = DefId::intern(&path);
            let public = match snapshot.signature(id) {
                Some(Ok(signature)) => signature.clone(),
                _ => match self.builtins.get(&path) {
                    Some(signature) => {
                        owned_signatures.signature(signature);
                        signature.clone()
                    }
                    None => continue,
                },
            };
            let owner = snapshot
                .body_view(&path.0.package, id)
                .and_then(Result::ok)
                .filter(|owner| ***owner != *public)
                .cloned();
            let coverage = self.signature_coverage(&path)?;
            weight.path(&path);
            weight.flat_vec(&coverage);
            if weight
                .bytes()
                .checked_add(owned_signatures.bytes)
                .is_none_or(|bytes| bytes > allowance.bytes())
            {
                return Err(invalid());
            }
            inputs.insert(
                path,
                Input {
                    public,
                    owner,
                    coverage,
                },
            );
        }
        if !Arc::ptr_eq(&snapshot, &self.signature_inputs()) {
            return Err(invalid());
        }
        Ok(Arc::new(FrontCheckedSignatures {
            inputs,
            front,
            environment,
            registrations,
            _allowance: allowance,
        }))
    }
}

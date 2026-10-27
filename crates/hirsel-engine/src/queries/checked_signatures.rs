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
    pub(crate) fn signature(&self, path: &DefPath) -> Option<&hirsel_types::SignatureRef> {
        self.inputs.get(path).map(|input| &input.public)
    }
    /// Physical payload bound for these already captured public/private
    /// signature values, independently of input table and source owners.
    /// Shared backing is conservatively counted per occurrence; no query,
    /// type loading or allocation occurs while inspecting the payloads.
    pub fn signature_payload_bytes(&self) -> Option<u64> {
        self.inputs.values().try_fold(0u64, |bytes, input| {
            let bytes = bytes.checked_add(
                crate::memo::retained::retained_signature_payload_bytes(&input.public),
            )?;
            match &input.owner {
                Some(owner) => bytes.checked_add(
                    crate::memo::retained::retained_signature_payload_bytes(owner),
                ),
                None => Some(bytes),
            }
        })
    }
    pub(crate) fn sections_clone_bytes(&self) -> Option<u64> {
        let mut weight = crate::NativeMetadataWeight::default();
        weight.tree::<DefPath, SignatureSection>(self.inputs.len());
        let mut payload_bytes = 0u64;
        for (path, input) in &self.inputs {
            weight.path(path);
            weight.path(path);
            weight.flat_vec(&input.coverage);
            let mut payloads = crate::memo::retained::Weight::default();
            payloads.signature(&input.public);
            if let Some(owner) = &input.owner {
                payloads.signature(owner);
            }
            payload_bytes = payload_bytes.checked_add(payloads.bytes)?;
        }
        weight.bytes().checked_add(payload_bytes)
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
    fn validate_checked_registrations(
        &mut self,
        front: &FrontBodyQueries,
    ) -> Result<(), QueryError> {
        if !self.staged_constants.is_empty() || !self.staged_values.is_empty() {
            return Err(QueryError::NotImplemented);
        }
        let paths: Vec<_> = self.generated.keys().cloned().collect();
        for path in paths {
            let accepted = front.registration(&path).ok_or_else(|| {
                QueryError::Operational(hirsel_types::OperationalFailure {
                    operation: "checked signature inputs".into(),
                    message: format!(
                        "generated declaration lacks its accepted front registration: {path}"
                    ),
                })
            })?;
            accepted.validate(self)?;
            if self
                .generated
                .get(&path)
                .is_none_or(|source| source.text != accepted.text())
            {
                return Err(QueryError::NotImplemented);
            }
        }
        Ok(())
    }
    /// Header-capture hold including construction metadata, without body
    /// checking, Lower, body decode or constructing the retained input table.
    pub fn front_checked_signatures_bytes(
        &mut self,
        front: &Arc<FrontBodyQueries>,
    ) -> Result<u64, QueryError> {
        let snapshot = front.signatures();
        if !Arc::ptr_eq(&snapshot, &self.signature_inputs()) {
            return Err(QueryError::NotImplemented);
        }
        self.validate_checked_registrations(front)?;
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
        weight.add(self.environment.compiler_version.capacity());
        let registrations = self.checked_registration_records();
        weight.vec(&registrations, |w, row| w.add(row.capacity()));
        let mut owned = crate::memo::retained::Weight::default();
        for path in paths {
            let id = DefId::intern(&path);
            if !snapshot.signature(id).is_some_and(|s| s.is_ok()) {
                let Some(signature) = self.builtins.get(&path) else {
                    continue;
                };
                owned.signature(signature);
            }
            weight.path(&path);
            weight.flat_vec(&self.signature_coverage(&path)?);
        }
        if !Arc::ptr_eq(&snapshot, &self.signature_inputs()) {
            return Err(QueryError::NotImplemented);
        }
        weight
            .bytes()
            .checked_add(owned.bytes)
            .ok_or(QueryError::NotImplemented)
    }
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
        if self.front_checked_signatures_bytes(&front)? > allowance.bytes() {
            return Err(invalid());
        }
        let snapshot = front.signatures();
        if !Arc::ptr_eq(&snapshot, &self.signature_inputs()) {
            return Err(invalid());
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

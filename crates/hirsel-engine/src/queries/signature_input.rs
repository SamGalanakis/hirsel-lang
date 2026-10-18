//! Publication consumes one immutable, independently checked signature input.
use super::*;
use hirsel_validate::{Signatures, ValidatedSignatures, Validation};
use std::collections::VecDeque;
use std::sync::{Mutex, OnceLock};

/// One independently checked signature input per exact content, shared by
/// every engine in the process, with the definitions V has passed against it.
pub struct SignatureInput {
    validated: ValidatedSignatures,
    passed: Mutex<BTreeMap<DefPath, VecDeque<hirsel_tir::DefinitionRef>>>,
}
impl std::ops::Deref for SignatureInput {
    type Target = ValidatedSignatures;
    fn deref(&self) -> &ValidatedSignatures {
        &self.validated
    }
}
impl SignatureInput {
    /// V's report on `definition`. A definition equal to one that already
    /// passed V against this exact input is not validated again; any other
    /// input runs V in full.
    pub fn validate_once(&self, definition: &hirsel_tir::DefinitionRef) -> Validation {
        let seen = |passed: &BTreeMap<DefPath, VecDeque<hirsel_tir::DefinitionRef>>| {
            passed.get(&definition.path).is_some_and(|rows| {
                rows.iter()
                    .any(|d| Arc::ptr_eq(d, definition) || **d == **definition)
            })
        };
        if seen(&self.passed.lock().unwrap()) {
            return Validation::default();
        }
        let report = self.validated.validate(definition);
        if report.violations.is_empty() {
            let mut passed = self.passed.lock().unwrap();
            if !seen(&passed) {
                let rows = passed.entry(definition.path.clone()).or_default();
                if rows.len() == PASSED_PER_DEFINITION {
                    rows.pop_front();
                }
                rows.push_back(definition.clone());
            }
        }
        report
    }
}
const PASSED_PER_DEFINITION: usize = 4;
const SHARED_INPUTS: usize = 8;
/// The process's input for exactly `signatures`, created (and its signatures
/// checked) only when no retained input has that content.
fn shared_input(fingerprint: ContentHash, signatures: Signatures) -> Arc<SignatureInput> {
    static INPUTS: OnceLock<Mutex<VecDeque<(ContentHash, Arc<SignatureInput>)>>> = OnceLock::new();
    let inputs = INPUTS.get_or_init(Default::default);
    let found = inputs
        .lock()
        .unwrap()
        .iter()
        .find(|(f, input)| *f == fingerprint && input.validated.matches(&signatures))
        .map(|(_, input)| input.clone());
    if let Some(input) = found {
        return input;
    }
    let input = Arc::new(SignatureInput {
        validated: ValidatedSignatures::new(signatures),
        passed: Mutex::default(),
    });
    let mut inputs = inputs.lock().unwrap();
    if inputs.len() == SHARED_INPUTS {
        inputs.pop_front();
    }
    inputs.push_back((fingerprint, input.clone()));
    input
}

pub(super) struct ProgramInput {
    revision: u64,
    environment: QueryEnvironment,
    manifest: Option<ContentHash>,
    unavailable: Option<QueryKind>,
    fingerprint: ContentHash,
    validated: Arc<SignatureInput>,
}
impl QueryEngine {
    pub(crate) fn invalidate_signature_input(&mut self) { self.signature_input.clear(); }
    /// The package's independently checked signature input; owner bodies of
    /// `package` see their private representations through it.
    pub fn validated_signature_input(
        &mut self,
        package: &hirsel_syntax::PackageId,
    ) -> Arc<SignatureInput> {
        let manifest = self
            .adapters
            .compiler_state
            .as_ref()
            .map(|s| s.borrow().manifest_revision);
        if let Some(input) = self.signature_input.get(package) {
            if input.revision == self.source_revision
                && input.environment == self.environment
                && input.manifest == manifest
                && input.unavailable == self.unavailable
            {
                return input.validated.clone();
            }
        }
        // Preserve the complete original universe, including unrelated malformed
        // signatures. Rejected/unavailable headers are omitted as before, but
        // their omission is never retained as evidence for a later publication.
        // Only this package's bodies see its retained defining shapes. Every
        // view keeps the public fingerprint; exact map equality additionally
        // retires the validation proof when private fields change.
        let mut signatures: Signatures = self
            .registry
            .signatures()
            .map(|s| (s.definition.clone(), (**s).clone()))
            .collect();
        let paths: Vec<_> = self.items.keys().cloned().collect();
        let mut complete = true;
        self.with_validation_walk(|engine| {
            for path in paths {
                match engine.signature(&path) {
                    Ok(signature) => match engine.body_signature(package, signature) {
                        Ok((signature, _)) => {
                            signatures.insert(path, (*signature).clone());
                        }
                        Err(_) => complete = false,
                    },
                    Err(_) => complete = false,
                }
            }
        });
        for (path, constant) in &self.staged_constants {
            signatures.insert(path.clone(), (*constant.signature).clone());
        }
        let parts: Vec<_> = signatures
            .iter()
            .flat_map(|(path, signature)| {
                [
                    path.fingerprint().0.to_vec(),
                    signature.fingerprint.0.to_vec(),
                ]
            })
            .collect();
        let fingerprint = ContentHash::of_parts(
            "hirsel.validated-signature-input.v2",
            parts.iter().map(Vec::as_slice),
        );
        let validated = self
            .signature_input
            .get(package)
            .filter(|old| old.fingerprint == fingerprint && old.validated.matches(&signatures))
            .map(|old| old.validated.clone())
            .unwrap_or_else(|| shared_input(fingerprint, signatures));
        if complete {
            self.signature_input.insert(
                package.clone(),
                ProgramInput {
                    revision: self.source_revision,
                    environment: self.environment.clone(),
                    manifest,
                    unavailable: self.unavailable,
                    fingerprint,
                    validated: validated.clone(),
                },
            );
        }
        validated
    }
}

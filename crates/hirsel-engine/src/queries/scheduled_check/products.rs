//! The checked-product receipt of exactly one completed source obligation set.
//! Structure follows candidate_products::QueryEngine::checked_products; the
//! only body/value inputs are immutable Lower results already selected by S5.
use super::*;
use hirsel_proof_export::{ExportInput, Header, Location, Locator};
use std::rc::Rc;

/// Complete Check receipt input with its original-decode/clone working grant.
/// A consumer may borrow the input after the loader and runtime are released.
/// Construction tables and output text require their separate grants.
pub struct FrontCheckedInput {
    input: ExportInput,
    _scope: Arc<FrontCheckScope>,
    _headers: Arc<FrontCheckedSignatures>,
    _allowance: MemoryReservation,
}
impl FrontCheckedInput {
    pub fn input(&self) -> &ExportInput {
        &self.input
    }
    pub fn type_capture_bytes(&self) -> Result<u64, QueryError> {
        FrontPreparedProof::capture_bytes(self)
    }
    pub fn capture_types_with_allowance(
        self: Rc<Self>,
        allowance: MemoryReservation,
    ) -> Result<FrontPreparedProof, QueryError> {
        FrontPreparedProof::capture(self, allowance)
    }
}
impl AdmittedProofInput for FrontCheckedInput {
    fn export_input(&self) -> &ExportInput {
        &self.input
    }
    fn export_module_bytes(&self) -> usize {
        let sources = self._scope.owner().plan.lower.sources();
        sources
            .anchors()
            .filter_map(|(_, file, _)| sources.file(file))
            .map(|file| {
                file.logical_path()
                    .as_os_str()
                    .as_encoded_bytes()
                    .len()
                    .saturating_mul(3)
            })
            .max()
            .unwrap_or(0)
    }
    fn export_clone_bytes(&self) -> u64 {
        self._allowance.bytes()
    }
    fn into_owned(self: Rc<Self>) -> Result<OwnedProofInput, QueryError> {
        let this = Rc::try_unwrap(self).map_err(|_| {
            QueryError::Operational(hirsel_types::OperationalFailure {
                operation: "checked input ownership".into(),
                message: "input must be uniquely owned before moving its payloads".into(),
            })
        })?;
        Ok(OwnedProofInput {
            input: this.input,
            source: OwnedProofSource {
                _proof: None,
                checked: Some((this._scope, this._headers)),
            },
            allowance: this._allowance,
        })
    }
    fn checked_receipt(&self) -> Option<(&FrontCheckScope, &FrontCheckedSignatures)> {
        Some((&self._scope, &self._headers))
    }
}
impl FrontCheckScope {
    /// Size the exact completed scope's prospective input before fetching an
    /// original body. All constant values are a conservative upper bound for
    /// the subset selected by those originals; unrelated bodies stay unopened.
    pub fn checked_product_input_bytes(
        &self,
        headers: &FrontCheckedSignatures,
        entry: &DefPath,
    ) -> Result<u64, QueryError> {
        self.checked_product_input_layout(headers, entry)?
            .total()
            .ok_or(QueryError::NotImplemented)
    }
    fn checked_product_input_layout(
        &self,
        headers: &FrontCheckedSignatures,
        entry: &DefPath,
    ) -> Result<DefinitionDecodeLayout, QueryError> {
        if !headers.matches(&self.owner().plan.front) {
            return Err(QueryError::NotImplemented);
        }
        let lower = &self.owner().plan.lower;
        let mut weight = crate::NativeMetadataWeight::default();
        weight.add(size_of::<FrontCheckedInput>() + 4 * size_of::<usize>());
        weight.path(entry);
        weight.add(hirsel_proof_export::FORMAT.len());
        weight.add(headers.environment().compiler_version.len());
        weight.source_snapshot(lower.sources());
        weight.add(size_of::<SourceMap>() + 4 * size_of::<usize>());
        weight.tree::<DefPath, hirsel_tir::DefinitionRef>(self.lowered().len());
        let mut required = headers
            .sections_clone_bytes()
            .ok_or(QueryError::NotImplemented)?;
        required = required
            .checked_add(lower.settled_constants_input_bytes()?)
            .ok_or(QueryError::NotImplemented)?;
        let mut transient = 0u64;
        for path in self.lowered() {
            if self.failed_lowered().contains(path) {
                continue;
            }
            weight.path(path);
            let layout = lower.settled_definition_decode_layout(path)?;
            transient = transient.max(layout.transient);
            required = required
                .checked_add(layout.retained)
                .and_then(|n| n.checked_add(lower.settled_constant_selection_bytes(path).ok()?))
                .ok_or(QueryError::NotImplemented)?;
        }
        Ok(DefinitionDecodeLayout {
            retained: required
                .checked_add(weight.bytes())
                .ok_or(QueryError::NotImplemented)?,
            transient,
        })
    }
    /// N-1 refuses before original body I/O. The returned wrapper owns the
    /// complete scope, captured headers and every decoded input allocation.
    pub fn checked_product_input_with_allowance(
        self: &Arc<Self>,
        headers: Arc<FrontCheckedSignatures>,
        entry: &DefPath,
        mut allowance: MemoryReservation,
    ) -> Result<FrontCheckedInput, QueryError> {
        let layout = self.checked_product_input_layout(&headers, entry)?;
        if layout.total().ok_or(QueryError::NotImplemented)? > allowance.bytes() {
            return Err(QueryError::Operational(hirsel_types::OperationalFailure {
                operation: "checked product input admission".into(),
                message: "original decode/clone buffers exceed caller admission".into(),
            }));
        }
        let input = self.checked_product_input(&headers, entry)?;
        Ok(FrontCheckedInput {
            input,
            _scope: self.clone(),
            _headers: headers,
            _allowance: allowance
                .split(layout.retained)
                .ok_or(QueryError::NotImplemented)?,
        })
    }
    /// Format already settled ordinary obligations in the caller's admitted
    /// scratch/output storage. This never requests a query, accepts a new
    /// namespace, or obtains validation authority from an encoded artifact.
    pub fn checked_products(
        &self,
        headers: &FrontCheckedSignatures,
        entry: &DefPath,
    ) -> Result<CheckedProducts, QueryError> {
        let input = self.checked_product_input(headers, entry)?;
        Ok(CheckedProductEncoder::new(&input)?
            .finish(self.reads().clone(), headers.registrations().to_vec()))
    }

    /// CheckedProducts' canonical input has all headers, only demanded bodies
    /// and runtime constants, no annotations, and no launch revision. The PS9
    /// artifact export is a separate, larger semantic-closure obligation.
    pub fn checked_product_input(
        &self,
        headers: &FrontCheckedSignatures,
        entry: &DefPath,
    ) -> Result<ExportInput, QueryError> {
        if !headers.matches(&self.owner().plan.front) {
            return Err(QueryError::NotImplemented);
        }
        let lower = &self.owner().plan.lower;
        // A GEN-6 specialization is a header only where a receipt body names
        // it, as checking that body registers it. Others were registered for
        // bodies this receipt does not cover.
        let named: BTreeSet<DefId> = self
            .reads()
            .values()
            .flat_map(|reads| reads.iter())
            .filter_map(|(read, _)| match read {
                ReadKey::Signature(id) => Some(*id),
                _ => None,
            })
            .collect();
        let mut signatures = BTreeMap::new();
        for (path, _) in headers.signatures() {
            if hirsel_types::specialization_parts(path).is_some()
                && !named.contains(&DefId::intern(path))
            {
                continue;
            }
            signatures.insert(
                path.clone(),
                headers.section(path).ok_or(QueryError::NotImplemented)?,
            );
        }
        let mut definitions = BTreeMap::new();
        for path in self.lowered() {
            // The guard is checked before any decoder touches a failed root.
            if !self.failed_lowered().contains(path) {
                definitions.insert(path.clone(), self.definition(path)?);
            }
        }
        let mut required = BTreeMap::new();
        for (path, definition) in &definitions {
            if signatures
                .get(path)
                .is_some_and(|s| matches!(s.public.kind, hirsel_types::SignatureKind::Const { .. }))
            {
                required.insert(path.clone(), (path.clone(), Vec::new()));
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
                        let selected = if arguments.is_empty() {
                            definition.clone()
                        } else {
                            hirsel_tir::constant_instance_path(definition, &arguments)
                        };
                        required.insert(selected, (definition.clone(), arguments));
                    }
                }
            }
        }
        let mut constants = BTreeMap::new();
        for (selected, (path, arguments)) in required {
            constants.insert(
                selected,
                (*lower.settled_constant(&path, &arguments)?).clone(),
            );
        }
        let sources = Rc::new(lower.sources().clone());
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
        Ok(ExportInput {
            types: lower.signatures().types().clone(),
            header: Header {
                format: hirsel_proof_export::FORMAT.into(),
                tir_version: hirsel_tir::TIR_VERSION,
                environment: headers.environment().clone(),
                entry: entry.clone(),
                program_revision: None,
            },
            definitions,
            signatures,
            constants,
            skipped: vec![],
            annotations: vec![],
            locator,
        })
    }
}

impl FrontCheckScope {
    /// Receipt read-key maps and digest-part buffers, after canonical text is
    /// separately admitted. No key encoder or allocation runs during inspection.
    pub(crate) fn receipt_finish_bytes(
        &self,
        headers: &FrontCheckedSignatures,
        _canonical: usize,
    ) -> Result<u64, QueryError> {
        let missing = || QueryError::NotImplemented;
        let mut weight = crate::NativeMetadataWeight::default();
        weight.add(receipt_owner_bytes().ok_or_else(missing)?);
        weight.tree::<DefPath, Arc<Reads>>(self.reads().len());
        weight.tree::<DefPath, BTreeMap<Vec<u8>, ContentHash>>(self.reads().len());
        let mut parts = 1usize
            .checked_add(headers.registrations().len())
            .ok_or_else(missing)?;
        for registration in headers.registrations() {
            weight.add(registration.len());
        }
        weight.add(
            headers
                .registrations()
                .len()
                .checked_mul(size_of::<Vec<u8>>())
                .ok_or_else(missing)?,
        );
        for (path, reads) in self.reads() {
            weight.path(path);
            weight.tree::<Vec<u8>, ContentHash>(reads.len());
            let path_bytes = crate::memo::encoded_def_path_bytes(path).ok_or_else(missing)?;
            weight.add(path_bytes.checked_mul(3).ok_or_else(missing)?);
            parts = parts
                .checked_add(1)
                .and_then(|n| n.checked_add(reads.len().checked_mul(2)?))
                .ok_or_else(missing)?;
            for (key, _) in reads.iter() {
                // Key encode growth plus retained key and digest clone overlap.
                weight.add(
                    key.encoded_bytes()
                        .ok_or_else(missing)?
                        .checked_mul(6)
                        .ok_or_else(missing)?,
                );
                weight.add(32); // digest's independent value bytes
            }
        }
        weight.add(
            parts
                .checked_mul(3)
                .map(|n| n.max(4))
                .and_then(|n| n.checked_mul(size_of::<Vec<u8>>()))
                .ok_or_else(missing)?,
        );
        Ok(weight.bytes())
    }
}
fn receipt_owner_bytes() -> Option<usize> {
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
    arc::<CheckedProducts>()?
        .checked_add(arc::<()>()?)?
        .checked_add(arc::<MemoryReservation>()?)?
        .checked_add(arc::<(Arc<dyn Send + Sync>, Arc<dyn Send + Sync>)>()?.checked_mul(2)?)
}
impl CheckedProducts {
    /// Retained maps only; canonical bytes have their separate detached owner.
    pub(crate) fn receipt_retained_bytes(&self) -> Option<u64> {
        let mut weight = crate::NativeMetadataWeight::default();
        weight.add(receipt_owner_bytes()?);
        weight.tree::<DefPath, BTreeMap<Vec<u8>, ContentHash>>(self.reads.len());
        for (path, reads) in &self.reads {
            weight.path(path);
            weight.tree::<Vec<u8>, ContentHash>(reads.len());
            for key in reads.keys() {
                weight.add(key.capacity());
            }
        }
        Some(weight.bytes())
    }
}

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
        required
            .checked_add(transient)
            .and_then(|n| n.checked_add(weight.bytes()))
            .ok_or(QueryError::NotImplemented)
    }
    /// N-1 refuses before original body I/O. The returned wrapper owns the
    /// complete scope, captured headers and every decoded input allocation.
    pub fn checked_product_input_with_allowance(
        self: &Arc<Self>,
        headers: Arc<FrontCheckedSignatures>,
        entry: &DefPath,
        allowance: MemoryReservation,
    ) -> Result<FrontCheckedInput, QueryError> {
        if self.checked_product_input_bytes(&headers, entry)? > allowance.bytes() {
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
            _allowance: allowance,
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
        let mut signatures = BTreeMap::new();
        for (path, _) in headers.signatures() {
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

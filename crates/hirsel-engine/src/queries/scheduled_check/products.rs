//! The checked-product receipt of exactly one completed source obligation set.
//! Structure follows candidate_products::QueryEngine::checked_products; the
//! only body/value inputs are immutable Lower results already selected by S5.
use super::*;
use hirsel_proof_export::{ExportInput, Header, Location, Locator};
use std::rc::Rc;

impl FrontCheckScope {
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

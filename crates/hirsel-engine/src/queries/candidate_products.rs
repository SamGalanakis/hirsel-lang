//! Snapshot-only receipts for the products an ordinary check actually required.
//! Reuse PS-9's canonical signature/TIR encoding, not transient interner ids.
use super::*;
use hirsel_proof_export::{ExportInput, Header, Location, Locator, SignatureSection};
use std::rc::Rc;

#[derive(Clone, Debug)]
pub struct CheckedProducts {
    pub digest: ContentHash,
    /// Optional diagnostic export; the digest always commits to the full bytes.
    pub canonical: Option<Arc<str>>,
    /// Key bytes use ReadKey::encode: DefPaths and spellings, never DefIds.
    pub reads: BTreeMap<DefPath, BTreeMap<Vec<u8>, ContentHash>>,
    pub type_rows: u64,
    pub type_bytes: u64,
}
impl QueryEngine {
    /// Does not lower additional runtime definitions. Compacted artifacts
    /// already required by this check may be reconstructed through the normal
    /// eligible store path; this cost belongs to receipt production.
    pub fn checked_products(&mut self, entry: &DefPath) -> Result<CheckedProducts, QueryError> {
        let mut definitions = BTreeMap::new();
        let paths: BTreeSet<_> = self
            .lowered
            .keys()
            .map(|id| id.path().clone())
            .chain(
                self.lower_receipts
                    .iter()
                    .filter(|(_, r)| r.retained_artifact())
                    .map(|(id, _)| id.path().clone()),
            )
            .collect();
        for path in paths {
            // Failed lowerings are represented by diagnostics, never a body.
            if self
                .lowered
                .get(&DefId::intern(&path))
                .is_some_and(|e| e.result().is_err())
            {
                continue;
            }
            if self
                .lower_receipts
                .get(&DefId::intern(&path))
                .is_some_and(|e| e.result().is_err())
            {
                continue;
            }
            definitions.insert(path.clone(), self.lower_to_tir(&path)?.value);
        }
        let snapshot = self.signature_snapshot();
        let mut signatures = BTreeMap::new();
        let paths: BTreeSet<_> = self
            .source_paths()
            .into_iter()
            .chain(self.builtins.keys().cloned())
            .chain(
                self.registry
                    .signatures()
                    .map(|s| s.definition.path().clone()),
            )
            .chain(self.staged_constants.keys().cloned())
            .collect();
        for path in paths {
            let id = DefId::intern(&path);
            let public = match snapshot.signature(id) {
                Some(Ok(signature)) => signature.clone(),
                _ => match self.builtins.get(&path) {
                    Some(signature) => signature.clone(),
                    None => continue,
                },
            };
            let owner_view = snapshot
                .body_view(&path.0.package, id)
                .and_then(Result::ok)
                .filter(|view| ***view != *public)
                .map(|view| (**view).clone());
            signatures.insert(
                path.clone(),
                SignatureSection {
                    path: path.clone(),
                    dependencies: self.signature_coverage(&path)?,
                    public: (*public).clone(),
                    owner_view,
                },
            );
        }
        let sources = Rc::new(self.sources.clone());
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
        let input = ExportInput {
            types: self.types.clone(),
            header: Header {
                format: hirsel_proof_export::FORMAT.into(),
                tir_version: hirsel_tir::TIR_VERSION,
                environment: self.proof_environment(),
                entry: entry.clone(),
                program_revision: None,
            },
            definitions,
            signatures,
            constants: self
                .constant_values
                .iter()
                .map(|(path, value)| (path.clone(), value.value.clone()))
                .chain(self.staged_values.iter().map(|((path, args), value)| {
                    (
                        hirsel_tir::constant_instance_path(path, args),
                        value.value.clone(),
                    )
                }))
                .collect(),
            skipped: vec![],
            annotations: vec![],
            locator,
        };
        let export = hirsel_proof_export::build(&input);
        if !export.skipped.is_empty() {
            return Err(QueryError::Operational(hirsel_types::OperationalFailure {
                operation: "candidate products".into(),
                message: format!("incomplete canonical products: {:?}", export.skipped),
            }));
        }
        let canonical: Arc<str> = hirsel_proof_export::encode(&export, &input.locator).into();
        // The check's complete dependency edges, per definition. Intermediate
        // Resolve/Check/Modes tables have different retention lifetimes and a
        // stored Modes product can subsume them. Preserve every observed edge
        // and value, rather than make table retention part of semantic identity.
        let mut reads: BTreeMap<DefPath, BTreeMap<Vec<u8>, ContentHash>> = BTreeMap::new();
        for path in self.source_paths() {
            for phase in [
                QueryKind::Signature,
                QueryKind::Resolve,
                QueryKind::Check,
                QueryKind::Modes,
                QueryKind::Lower,
                QueryKind::Summary,
                QueryKind::CheckedSummary,
            ] {
                if let Some(observed) = self.memo_reads(&path, phase) {
                    let edges = reads.entry(path.clone()).or_default();
                    for (key, value) in observed.iter() {
                        let mut bytes = vec![];
                        key.encode(&mut bytes);
                        if edges.insert(bytes, *value).is_some_and(|old| old != *value) {
                            return Err(QueryError::Operational(
                                hirsel_types::OperationalFailure {
                                    operation: "candidate reads".into(),
                                    message: format!("conflicting complete read values for {path}"),
                                },
                            ));
                        }
                    }
                }
            }
        }
        let mut parts = vec![canonical.as_bytes().to_vec()];
        for (path, generated) in &self.generated {
            let mut key = vec![];
            crate::memo::encode_def_path(&mut key, path);
            parts.push(key);
            parts.push(generated.text.as_bytes().to_vec());
        }
        for declaration in &self.instance_bodies.declarations {
            let mut key = vec![];
            crate::memo::encode_def_path(&mut key, declaration.path());
            parts.push(key);
        }
        for (declaration, instances) in &self.instance_bodies.instances {
            let mut records: Vec<_> = instances
                .iter()
                .map(|(actual, body)| {
                    let mut record = vec![];
                    crate::memo::encode_def_path(&mut record, declaration.path());
                    for spelling in actual.spellings() {
                        record.extend_from_slice(&(spelling.len() as u64).to_le_bytes());
                        record.extend_from_slice(spelling.as_bytes());
                    }
                    crate::memo::encode_def_path(&mut record, body.definition.path());
                    record
                })
                .collect();
            records.sort();
            parts.extend(records);
        }
        for (path, observed) in &reads {
            let mut key = vec![];
            crate::memo::encode_def_path(&mut key, path);
            parts.push(key);
            for (key, value) in observed {
                parts.push(key.clone());
                parts.push(value.0.to_vec());
            }
        }
        let stats = self.types.stats();
        Ok(CheckedProducts {
            digest: ContentHash::of_parts(
                "hirsel.checked-products.v1",
                parts.iter().map(Vec::as_slice),
            ),
            canonical: Some(canonical),
            reads,
            type_rows: stats.rows,
            type_bytes: stats.row_bytes,
        })
    }
}

//! Snapshot-only receipts for the products an ordinary check actually required.
//! Reuse PS-9's canonical signature/TIR encoding, not transient interner ids.
use super::*;
use hirsel_proof_export::{ExportInput, Header, Location, Locator, SignatureSection};
use std::rc::Rc;

/// Current ordinary-check obligations, independent of retained cache contents.
#[derive(Default)]
pub(crate) struct CheckedScope {
    pub(crate) front: BTreeMap<DefPath, Arc<crate::memo::Reads>>,
    pub(crate) lowered: BTreeSet<DefPath>,
}

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
impl CheckedProducts {
    /// Exact owner/read keys ordered by persisted bytes, independent of intern
    /// assignment and scheduling. Includes private views and negative reads.
    pub fn canonical_reads(&self) -> BTreeMap<(Vec<u8>, Vec<u8>), ContentHash> {
        self.reads
            .iter()
            .flat_map(|(owner, reads)| {
                let mut bytes = Vec::new();
                crate::memo::encode_def_path(&mut bytes, owner);
                reads
                    .iter()
                    .map(move |(key, value)| ((bytes.clone(), key.clone()), *value))
            })
            .collect()
    }
}
impl QueryEngine {
    /// Does not lower additional runtime definitions. Compacted artifacts
    /// already required by this check may be reconstructed through the normal
    /// eligible store path; this cost belongs to receipt production.
    pub fn checked_products(&mut self, entry: &DefPath) -> Result<CheckedProducts, QueryError> {
        let mut definitions = BTreeMap::new();
        let paths = self.checked_scope.lowered.clone();
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
        // Only runtime constant references (and materialized constant roots)
        // belong to this check. S1 may retain unrelated std evaluations.
        let mut required_constants = BTreeSet::new();
        for (path, definition) in &definitions {
            if self.constant_values.contains_key(path) {
                required_constants.insert(path.clone());
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
                        required_constants.insert(if arguments.is_empty() {
                            definition.clone()
                        } else {
                            hirsel_tir::constant_instance_path(definition, &arguments)
                        });
                    }
                }
            }
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
                .filter(|(path, _)| required_constants.contains(path))
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
        let mut required_reads = self.checked_scope.front.clone();
        for path in &self.checked_scope.lowered {
            if let Some(lower) = self.shared_memo_reads(path, QueryKind::Lower) {
                let combined = required_reads.entry(path.clone()).or_default();
                let mut edges = combined
                    .iter()
                    .chain(lower.iter())
                    .map(|(key, value)| (*key, *value))
                    .collect::<Vec<_>>();
                edges.sort();
                edges.dedup();
                *combined = Arc::new(crate::memo::Reads::from_vec(edges).map_err(|_| {
                    QueryError::Operational(hirsel_types::OperationalFailure {
                        operation: "candidate reads".into(),
                        message: format!("conflicting complete read values for {path}"),
                    })
                })?);
            }
        }
        for (path, observed) in required_reads {
            let edges = reads.entry(path).or_default();
            for (key, value) in observed.iter() {
                let mut bytes = vec![];
                key.encode(&mut bytes);
                edges.insert(bytes, *value);
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

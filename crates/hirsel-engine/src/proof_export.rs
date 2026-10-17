//! The proof-seam TIR export: the driver selects the
//! entry's semantic closure and supplies TIR, signatures, constants and source
//! sites; `hirsel-proof-export` builds, encodes, digests and imports.
use crate::*;
use hirsel_proof_export::{
    Environment, Export, ExportInput, Header, ImportError, Location, Locator, SignatureSection,
    Skipped,
};
use hirsel_syntax::ContentHash;
use hirsel_syntax::{ItemKind, TypeBody};
use hirsel_types::SignatureKind;
use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;

/// Source locations use a module identity, deliberately excluding the item name.
fn module_identity(path: &DefPath) -> String {
    let item = &path.0;
    let mut text = format!("{}@{}", item.package.name, item.package.version);
    for module in &item.modules {
        text.push('/');
        text.push_str(module);
    }
    text
}

fn reason(error: &QueryError) -> String {
    match error {
        QueryError::Diagnostics(diagnostics) => diagnostics
            .iter()
            .map(|d| format!("{}: {}", d.rule, d.message))
            .collect::<Vec<_>>()
            .join("; "),
        other => format!("{other:?}"),
    }
}

impl QueryEngine {
    /// The producing compiler, as the export header records and import requires.
    pub fn proof_environment(&self) -> Environment {
        Environment {
            interface_version: self.environment.interface_version,
            compiler_version: self.environment.compiler_version.clone(),
            spec_hash: self.environment.spec_hash,
        }
    }

    /// PS-9: every definition of the entry package and its semantic closure, with TIR for each
    /// executable one. A definition whose TIR cannot be produced is skipped
    /// with its reason, never silently dropped.
    pub fn proof_export_input(&mut self, entry: &DefPath) -> Result<ExportInput, QueryError> {
        let roots: Vec<DefPath> = self
            .item_paths()
            .into_iter()
            .filter(|path| path.0.package == entry.0.package)
            .collect();
        let program = self.semantic_program(roots)?;
        // A library-only program has no launch entry and so no revision.
        let program_revision = match self.item_signature(entry) {
            Ok(_) => Some(ContentHash(self.semantic_program_revision(entry)?.digest())),
            Err(QueryError::NotImplemented) => None,
            Err(error) => return Err(error),
        };
        let mut definitions = BTreeMap::new();
        let mut signatures = BTreeMap::new();
        let mut constants = BTreeMap::new();
        let mut skipped = vec![];
        let mut modules = BTreeMap::new();
        let mut executable = BTreeSet::new();
        for path in &program.definitions {
            let signature = self.item_signature(path)?;
            let (owner_view, _) = self.body_signature(&path.0.package, signature.value.clone())?;
            signatures.insert(
                path.clone(),
                SignatureSection {
                    path: path.clone(),
                    dependencies: self.signature_coverage(path)?,
                    public: (*signature.value).clone(),
                    owner_view: (owner_view != signature.value).then(|| (*owner_view).clone()),
                },
            );
            if matches!(signature.value.kind, SignatureKind::Const(_)) {
                match self.materialize_value(path) {
                    Ok(value) => {
                        constants.insert(path.clone(), value);
                    }
                    Err(error) => skipped.push(Skipped {
                        name: path.clone(),
                        reason: format!("constant value: {}", reason(&error)),
                    }),
                }
            }
            let Ok(items) = self.definition_items(path) else {
                continue;
            };
            for (file, item) in items {
                modules.insert(file.file, module_identity(path));
                match &file.tree.items[item.0 as usize].kind {
                    ItemKind::Function { body: Some(_), .. }
                    | ItemKind::Const { .. }
                    | ItemKind::Type { drop: Some(_), .. } => {
                        executable.insert(path.clone());
                    }
                    ItemKind::Type {
                        body: TypeBody::Distinct { fact: Some(_), .. },
                        ..
                    } => {
                        executable.insert(hirsel_check_protocol::fact::fact_path(path));
                    }
                    _ => {}
                }
            }
        }
        for path in executable {
            if !signatures.contains_key(&path) {
                let signature = self.item_signature(&path)?;
                signatures.insert(
                    path.clone(),
                    SignatureSection {
                        path: path.clone(),
                        dependencies: self.signature_coverage(&path)?,
                        public: (*signature.value).clone(),
                        owner_view: None,
                    },
                );
            }
            match self.lower_to_tir(&path) {
                Ok(lowered) if !crate::queries::has_errors(&lowered.diagnostics) => {
                    definitions.insert(path, lowered.value);
                }
                Ok(lowered) => skipped.push(Skipped {
                    name: path,
                    reason: reason(&QueryError::Diagnostics(lowered.diagnostics)),
                }),
                Err(error) => skipped.push(Skipped {
                    name: path,
                    reason: reason(&error),
                }),
            }
        }
        let sources = Rc::new(self.sources.clone());
        let locator: Locator = Rc::new(move |span| {
            let file = sources.file(span.file);
            let start = file.and_then(|f| f.location(span.start));
            let end = file.and_then(|f| f.location(span.end));
            match (modules.get(&span.file), start, end) {
                (Some(module), Some(start), Some(end)) => Location {
                    module: module.clone(),
                    line: start.line,
                    column: start.column,
                    end_line: end.line,
                    end_column: end.column,
                },
                _ => Location::unknown(),
            }
        });
        Ok(ExportInput {
            header: Header {
                format: hirsel_proof_export::FORMAT.into(),
                tir_version: hirsel_tir::TIR_VERSION,
                environment: self.proof_environment(),
                entry: entry.clone(),
                program_revision,
            },
            definitions,
            signatures,
            constants,
            skipped,
            locator,
        })
    }

    /// `hirsel.tir.export.v1` text of the entry's closure.
    pub fn export_tir(&mut self, entry: &DefPath) -> Result<String, QueryError> {
        Ok(hirsel_proof_export::export(
            &self.proof_export_input(entry)?,
        ))
    }

    /// Import an export made by this compiler: refuses unless every digest,
    /// table and V validation recomputes (PS-10).
    pub fn import_tir(&self, text: &str) -> Result<Export, ImportError> {
        hirsel_proof_export::import(text, &self.proof_environment())
    }
}

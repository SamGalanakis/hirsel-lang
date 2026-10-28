//! Name-independent definition hashes (LANG-6), a query over checked TIR.
use super::*;
use hirsel_proof_export::{CallSite, DefinitionContent, DefinitionGroup};
use hirsel_syntax::ContentHash;
use hirsel_types::SignatureKind;

fn operational(message: impl Into<String>) -> QueryError {
    QueryError::Operational(hirsel_types::OperationalFailure {
        operation: "definition hash".into(),
        message: message.into(),
    })
}

/// The definition hashes of the closure of some roots.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct DefinitionHashes {
    /// Every definition the roots reach, with its hash.
    pub hashes: BTreeMap<DefPath, ContentHash>,
    /// Each definition with TIR: its calls and function references, each with
    /// its callee's hash (the identity a runner call carries).
    pub call_sites: BTreeMap<DefPath, Vec<CallSite>>,
}

/// How a definition's content is taken.
enum Form {
    /// Signature and TIR.
    Tir,
    /// A type without TIR: its signature.
    Type,
    /// Path, signature, declaration source fields and reads.
    Named(Vec<Vec<u8>>, BTreeSet<DefPath>),
}

impl QueryEngine {
    /// The hash of every definition `roots` reach, and the call sites of each
    /// one with TIR. Each definition is encoded once per query.
    pub fn definition_hashes(
        &mut self,
        roots: impl IntoIterator<Item = DefPath>,
    ) -> Result<DefinitionHashes, QueryError> {
        let contents = self.closure_contents(roots)?;
        let hashes =
            hirsel_proof_export::definition_hashes(self.types(), &contents).map_err(operational)?;
        let mut call_sites = BTreeMap::new();
        for (path, content) in &contents {
            if let DefinitionContent::Body { tir: Some(tir), .. } = content {
                let sites = hirsel_proof_export::call_sites(tir, &hashes).map_err(|callee| {
                    operational(format!("{path} calls {callee} outside its closure"))
                })?;
                call_sites.insert(path.clone(), sites);
            }
        }
        Ok(DefinitionHashes { hashes, call_sites })
    }

    /// The recursive groups of the closure of `roots`, dependencies first,
    /// with each member's hash preimage: what a code store holds.
    pub fn definition_groups(
        &mut self,
        roots: impl IntoIterator<Item = DefPath>,
    ) -> Result<Vec<DefinitionGroup>, QueryError> {
        let contents = self.closure_contents(roots)?;
        hirsel_proof_export::definition_groups(self.types(), &contents).map_err(operational)
    }

    /// The content of every definition `roots` reach.
    fn closure_contents(
        &mut self,
        roots: impl IntoIterator<Item = DefPath>,
    ) -> Result<BTreeMap<DefPath, DefinitionContent>, QueryError> {
        let mut contents = BTreeMap::new();
        let mut pending: Vec<DefPath> = roots.into_iter().collect();
        while let Some(path) = pending.pop() {
            if contents.contains_key(&path) {
                continue;
            }
            let content = self.definition_content(&path)?;
            pending.extend(
                hirsel_proof_export::references(self.types(), &content)
                    .into_iter()
                    .filter(|p| !contents.contains_key(p)),
            );
            contents.insert(path, content);
        }
        Ok(contents)
    }

    /// The hashes of `package`'s source definitions and their closure.
    pub fn package_definition_hashes(
        &mut self,
        package: &hirsel_syntax::PackageId,
    ) -> Result<DefinitionHashes, QueryError> {
        let roots: Vec<_> = self
            .item_paths()
            .into_iter()
            .filter(|path| &path.0.package == package)
            .collect();
        self.definition_hashes(roots)
    }

    /// One definition's hash.
    pub fn definition_hash(&mut self, path: &DefPath) -> Result<ContentHash, QueryError> {
        self.definition_hashes([path.clone()])?
            .hashes
            .get(path)
            .copied()
            .ok_or_else(|| operational(format!("{path} has no hash")))
    }

    fn definition_content(&mut self, path: &DefPath) -> Result<DefinitionContent, QueryError> {
        let signature = self.item_signature(path)?;
        if has_errors(&signature.diagnostics) {
            return Err(QueryError::Diagnostics(signature.diagnostics));
        }
        let is_type = matches!(signature.value.kind, SignatureKind::Type(_));
        self.materialize(path)?;
        let form = if let Some(source) = self.items.get(path).and_then(|items| items.first()) {
            match () {
                _ if generation::executable(source) => Form::Tir,
                _ if is_type => Form::Type,
                _ => {
                    let (content, reads) = self.semantic_node(path)?;
                    Form::Named(content, reads)
                }
            }
        } else if self.registry.get(DefId::intern(path)).is_some()
            || self.builtins.contains_key(path)
            || self.staged_constants.contains_key(path)
        {
            Form::Named(vec![], BTreeSet::new())
        } else {
            Form::Tir
        };
        let lowered = if matches!(&form, Form::Tir) {
            let lowered = self.lower_to_tir(path)?;
            if has_errors(&lowered.diagnostics) {
                return Err(QueryError::Diagnostics(lowered.diagnostics));
            }
            Some(lowered.value)
        } else {
            None
        };
        let signature = self.item_signature(path)?;
        if has_errors(&signature.diagnostics) {
            return Err(QueryError::Diagnostics(signature.diagnostics));
        }
        // A type's content is its owner view: its representation and finalizer.
        let signature = if is_type {
            self.body_signature(&path.0.package, signature.value)?.0
        } else {
            signature.value
        };
        Ok(match form {
            Form::Tir => DefinitionContent::Body {
                signature,
                tir: lowered,
            },
            Form::Type => DefinitionContent::Body {
                signature,
                tir: None,
            },
            Form::Named(source, reads) => DefinitionContent::Named {
                signature,
                source,
                reads: reads.into_iter().collect(),
            },
        })
    }
}

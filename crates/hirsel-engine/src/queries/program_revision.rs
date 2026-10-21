//! Rooted semantic dependencies, independent of native artifacts and instances.
use super::*;
use hirsel_run_schema::{ProgramNode, ProgramRevision};
use hirsel_syntax::TokenKind;
use hirsel_types::{DependencyRead, SignatureKind};

include!(concat!(env!("OUT_DIR"), "/semantic_runtime.rs"));

fn operational(message: impl Into<String>) -> QueryError {
    QueryError::Operational(hirsel_types::OperationalFailure {
        operation: "semantic program revision".into(),
        message: message.into(),
    })
}

#[derive(Clone)]
pub(crate) struct SemanticProgram {
    nodes: Vec<ProgramNode>,
    /// Every definition the closure visited.
    pub(crate) definitions: BTreeSet<DefPath>,
    /// Each visited definition's direct dependencies.
    pub(crate) uses: BTreeMap<DefPath, BTreeSet<DefPath>>,
}

/// The foreign functions a program reaches (PROG-4).
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ForeignUse {
    /// Every reachable `extern`.
    pub externs: BTreeSet<DefPath>,
    /// Each definition that uses an extern without being a safe wrapper with a
    /// deterministic model, beside one extern it uses. A run reaching any of
    /// them cannot be recorded or replayed.
    pub unmodeled: BTreeMap<DefPath, DefPath>,
}

impl SemanticProgram {
    pub(crate) fn revision(&self, entry: &DefPath) -> Result<ProgramRevision, QueryError> {
        ProgramRevision::encode(&entry.to_string(), self.nodes.clone())
            .map_err(|e| operational(e.to_string()))
    }
}

impl QueryEngine {
    /// Select the same semantic program for interpreter, checked heap and native
    /// launches. No native export, instance graph, compiler options or artifact
    /// identity participates. All checked reads include private implementations;
    /// compile-time reads, implicit type/finalizer dependencies, type facts and serializable
    /// checks also close.
    pub fn semantic_program_revision(
        &mut self,
        entry: &DefPath,
    ) -> Result<ProgramRevision, QueryError> {
        self.semantic_program([entry.clone()])?.revision(entry)
    }

    /// Read-only, batched graph product used by diagnostic shadow tools.
    /// This is the existing semantic revision graph, not a certificate that all
    /// execution dependency classes or host inputs have been established.
    pub fn semantic_program_nodes(
        &mut self,
        roots: impl IntoIterator<Item = DefPath>,
    ) -> Result<Vec<ProgramNode>, QueryError> {
        Ok(self.semantic_program(roots)?.nodes)
    }

    pub(crate) fn semantic_program(
        &mut self,
        roots: impl IntoIterator<Item = DefPath>,
    ) -> Result<SemanticProgram, QueryError> {
        let mut nodes = vec![ProgramNode {
            identity: "$hirsel/runtime@1".into(),
            content: vec![RUNTIME_REVISION.to_vec()],
            dependencies: vec![],
        }];
        let mut visited = BTreeSet::new();
        let mut uses = BTreeMap::new();
        let mut pending: Vec<_> = roots.into_iter().collect();
        // CT-5: the closure also holds every instance of a generic generated
        // declaration that it selects, and what those instances reach.
        loop {
            self.semantic_closure(&mut pending, &mut visited, &mut uses, &mut nodes)?;
            let selected =
                self.select_generated_instances(visited.iter().cloned().collect::<Vec<_>>())?;
            pending.extend(selected.into_iter().filter(|path| !visited.contains(path)));
            if pending.is_empty() {
                break;
            }
        }
        Ok(SemanticProgram {
            nodes,
            definitions: visited,
            uses,
        })
    }

    fn semantic_closure(
        &mut self,
        pending: &mut Vec<DefPath>,
        visited: &mut BTreeSet<DefPath>,
        uses: &mut BTreeMap<DefPath, BTreeSet<DefPath>>,
        nodes: &mut Vec<ProgramNode>,
    ) -> Result<(), QueryError> {
        while let Some(path) = pending.pop() {
            if !visited.insert(path.clone()) {
                continue;
            }
            let (content, dependencies) = match self.image_node(&path) {
                Some(record) => {
                    // Closed materializations are already in the image. Only
                    // size-generic reads need declarations for client selection.
                    if record.has_staged_reads() {
                        let body = self.mode_query(&path)?;
                        self.register_staged(&body.value)?;
                    }
                    (
                        record.node.content.clone(),
                        record.node.dependencies.clone(),
                    )
                }
                None => self.semantic_node(&path)?,
            };
            pending.extend(dependencies.iter().cloned());
            uses.insert(path.clone(), dependencies.clone());
            let mut dependencies: Vec<_> = dependencies.iter().map(ToString::to_string).collect();
            dependencies.push("$hirsel/runtime@1".into());
            nodes.push(ProgramNode {
                identity: path.to_string(),
                content,
                dependencies,
            });
        }
        Ok(())
    }

    /// One definition's revision content and direct semantic dependencies,
    /// computed from its checked queries.
    pub(crate) fn semantic_node(
        &mut self,
        path: &DefPath,
    ) -> Result<(Vec<Vec<u8>>, BTreeSet<DefPath>), QueryError> {
        let signature = self.item_signature(path)?;
        if has_errors(&signature.diagnostics) {
            return Err(QueryError::Diagnostics(signature.diagnostics));
        }
        let mut dependencies: BTreeSet<_> = signature
            .dependencies
            .iter()
            .map(|d| d.definition.path().clone())
            .collect();
        let mut content = vec![
            hirsel_types::encoding::canonical_encoding(
                &self.types,
                &signature.value,
                &signature.dependencies,
            )
            .map_err(|e| operational(format!("signature encoding: {e:?}")))?,
        ];
        self.materialize(path)?;
        if self.items.contains_key(path) {
            if self.semantic_lower_ready(path)? {
                let syntax = self.items[path]
                    .first()
                    .expect("indexed source item")
                    .clone();
                content.extend(self.semantic_tokens(&syntax)?);
            } else {
                let analyzed = self.mode_query(path)?;
                if has_errors(&analyzed.diagnostics) {
                    return Err(QueryError::Diagnostics(analyzed.diagnostics.clone()));
                }
                let syntax = &analyzed.value.typed.syntax;
                content.extend(self.semantic_tokens(syntax)?);
                if generation::executable(syntax) {
                    let lowered = self.lower_to_tir(path)?;
                    if has_errors(&lowered.diagnostics) {
                        return Err(QueryError::Diagnostics(lowered.diagnostics));
                    }
                }
            }
            for kind in [
                QueryKind::Signature,
                QueryKind::Resolve,
                QueryKind::Check,
                QueryKind::Modes,
                QueryKind::Lower,
            ] {
                if let Some(reads) = self.dependency_reads(path, kind) {
                    dependencies
                        .extend(reads.signatures.iter().map(|d| d.definition.path().clone()));
                    for read in reads.resolver.iter().chain(&reads.manifest_lookups) {
                        match read {
                            DependencyRead::Signature { definition, .. } => {
                                dependencies.insert(definition.path().clone());
                            }
                            DependencyRead::Lookup {
                                result: Ok(definition),
                                ..
                            } => {
                                dependencies.insert(definition.clone());
                            }
                            DependencyRead::ProviderLookup { output, .. } => {
                                if let Ok(definition) = &output.result {
                                    dependencies.insert(definition.clone());
                                }
                            }
                            _ => {}
                        }
                    }
                }
            }
            // A generated body (CT-5) depends on what its generator executed,
            // and every body on the private representations it read.
            let id = hirsel_syntax::DefId::intern(path);
            let reads = [
                self.checked.get(&id).map(|e| e.memo.reads.clone()),
                self.modes.get(&id).map(|e| e.memo.reads.clone()),
                self.shared_memo_reads(path, QueryKind::Lower),
            ];
            for reads in reads.into_iter().flatten() {
                dependencies.extend(executed_definitions(&reads));
            }
        }
        for ((constant, _), value) in &self.staged_values {
            if self
                .staged_constants
                .get(constant)
                .is_some_and(|c| c.owner == *path)
            {
                dependencies.extend(executed_definitions(&value.executed));
            }
        }
        if let SignatureKind::Type(declaration) = &signature.value.kind {
            dependencies.extend(declaration.drop.iter().cloned());
            // FACT-2: construction runs the fact's helper, which closes over
            // what its predicate calls.
            if declaration.fact.is_some() {
                dependencies.insert(hirsel_check_protocol::fact::fact_path(path));
            }
            // SER-1: decoding a checked opaque type calls its check.
            if let Some(check) = &declaration.serializable_check {
                dependencies.insert(check.function.clone());
            }
        }
        Ok((content, dependencies))
    }

    /// The externs `entry` reaches and the definitions using them outside a
    /// deterministic safe wrapper.
    pub fn foreign_use(&mut self, entry: &DefPath) -> Result<ForeignUse, QueryError> {
        let program = self.semantic_program([entry.clone()])?;
        let mut foreign = ForeignUse::default();
        for path in &program.definitions {
            if matches!(&self.item_signature(path)?.value.kind, SignatureKind::Function(f) if f.foreign.is_some())
            {
                foreign.externs.insert(path.clone());
            }
        }
        // An extern's own node lists itself; only its callers use it.
        for (user, uses) in program
            .uses
            .iter()
            .filter(|(u, _)| !foreign.externs.contains(*u))
        {
            let Some(called) = uses.iter().find(|d| foreign.externs.contains(*d)) else {
                continue;
            };
            let modeled = self
                .item_signature(user)?
                .value
                .metadata
                .wrapper
                .is_some_and(|w| w.deterministic);
            if !modeled {
                foreign.unmodeled.insert(user.clone(), called.clone());
            }
        }
        Ok(foreign)
    }

    pub(crate) fn semantic_tokens(&self, syntax: &ItemSyntax) -> Result<Vec<Vec<u8>>, QueryError> {
        let item = &syntax.item;
        let mut content = vec![b"hirsel.semantic-source.v1".to_vec()];
        let mut newline = false;
        let start = item.origin.tokens.start as usize;
        let tokens = syntax
            .tree
            .tokens
            .get(start..item.origin.tokens.end as usize)
            .ok_or_else(|| operational("invalid source token range"))?;
        // PS-19: an analysis annotation is not program content. By token
        // range: an adapted item (a FACT-2 predicate) shares its owner's tokens.
        let annotations: Vec<_> = syntax
            .tree
            .analyses
            .iter()
            .map(|a| a.origin.tokens.start as usize..a.origin.tokens.end as usize)
            .collect();
        for (index, token) in tokens.iter().enumerate() {
            if annotations.iter().any(|r| r.contains(&(start + index))) {
                continue;
            }
            let spelling = self
                .sources
                .slice(token.span)
                .ok_or_else(|| operational("missing token source"))?;
            match token.kind {
                TokenKind::Trivia(_) => {
                    newline |= spelling.contains('\n');
                }
                TokenKind::Eof => {}
                TokenKind::Invalid => return Err(operational("invalid source token")),
                _ => {
                    // Newline termination matters (LANG-1), including boundaries
                    // inside comments. Collapse runs; positions and comment/space
                    // spelling never become semantic content.
                    if newline && content.len() > 1 {
                        content.push(vec![0]);
                    }
                    newline = false;
                    let mut field = vec![1];
                    field.extend_from_slice(spelling.as_bytes());
                    content.push(field);
                }
            }
        }
        Ok(content)
    }
}

/// The definitions a memo executed or whose private representation it read.
fn executed_definitions(reads: &crate::memo::Reads) -> Vec<DefPath> {
    reads
        .iter()
        .filter(|(read, _)| {
            matches!(
                read,
                ReadKey::ExecutedBody(_)
                    | ReadKey::ExecutedSignature(_)
                    | ReadKey::Representation(_)
            )
        })
        .map(|(read, _)| read.definition().unwrap().path().clone())
        .collect()
}

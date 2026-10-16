//! Frozen native handoff: all owners are retained; no provider reaches a backend.
use crate::*;
use hirsel_syntax::{ExprId, ItemKind};
use hirsel_tir::DefinitionRef;
use hirsel_tir::Materialized;
use hirsel_types::{SignatureKind, SignatureRef};
use std::{collections::BTreeMap, sync::Arc};
#[derive(Clone, Debug)]
pub struct NativeSource {
    pub syntax: Arc<SyntaxFile>,
    pub item: ItemId,
    pub implementation_revision: ContentHash,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum NativeStatus {
    Complete,
    Unchecked,
}
/// Raw snapshots can be constructed by callers, but must be imported before use.
#[derive(Clone, Debug)]
pub struct NativeInput {
    pub environment: QueryEnvironment,
    pub operations: BTreeMap<OperationKey, OperationRecord>,
    pub registry_revision: ContentHash,
    pub definitions: BTreeMap<DefPath, DefinitionRef>,
    pub signatures: BTreeMap<DefPath, SignatureRef>,
    pub signature_dependencies: BTreeMap<DefPath, Vec<SignatureDependency>>,
    pub representations: BTreeMap<DefPath, Option<RepresentationRecord>>,
    pub source_owners: BTreeMap<DefPath, NativeSource>,
    pub sources: SourceMap,
    pub constants: BTreeMap<DefPath, Materialized>,
    pub constant_definitions: Vec<DefPath>,
    pub materialized: BTreeMap<DefPath, Vec<(ExprId, Materialized)>>,
    pub completed_source_stages: NativeStatus,
    /// One completed source audit per selected executable definition (R9).
    pub audits: BTreeMap<DefPath, SourceAudit>,
    /// GEN-5 instance requirements; import recollects them and rejects growth.
    pub instances: InstanceGraph,
}
/// Construction is private to successful revalidation, including current source stages.
pub struct ValidatedNativeInput {
    input: NativeInput,
    semantic_program: crate::queries::SemanticProgram,
}
impl ValidatedNativeInput {
    /// The same semantic closure as source execution, retained independently of
    /// the importing engine and of native topology/artifact provenance.
    pub fn semantic_program_revision(
        &self,
        entry: &DefPath,
    ) -> Result<hirsel_run_config::ProgramRevision, QueryError> {
        self.semantic_program.revision(entry)
    }
    pub fn snapshot(&self) -> &NativeInput {
        &self.input
    }
}
fn invalid(message: impl Into<String>) -> QueryError {
    QueryError::Operational(hirsel_types::OperationalFailure {
        operation: "native snapshot import".into(),
        message: message.into(),
    })
}
impl QueryEngine {
    /// Select every definition the program reaches (`native_paths`): all of a
    /// standalone program, and of a package program its own package plus what
    /// that reaches in its dependencies. Missing implementations and failed
    /// stages are operational/unavailable outcomes, never source acceptance.
    pub fn export_native_input(&mut self) -> Result<NativeInput, QueryError> {
        let paths = self.native_paths()?;
        let mut signatures = self.native_builtin_signatures();
        let mut signature_dependencies = BTreeMap::new();
        let mut representations: BTreeMap<_, _> = signatures
            .iter()
            .filter_map(|(path, signature)| {
                matches!(signature.kind, SignatureKind::Type(_)).then_some((path.clone(), None))
            })
            .collect();
        let mut definitions = BTreeMap::new();
        let mut source_owners = BTreeMap::new();
        let mut constants = BTreeMap::new();
        let mut materialized = BTreeMap::new();
        let mut audits = BTreeMap::new();
        for path in paths.iter().cloned() {
            let signature = self.item_signature(&path)?;
            if crate::queries::has_errors(&signature.diagnostics) {
                return Err(QueryError::Diagnostics(signature.diagnostics));
            }
            signature_dependencies.insert(path.clone(), signature.dependencies);
            signatures.insert(path.clone(), signature.value.clone());
            if matches!(signature.value.kind, SignatureKind::Type(_)) {
                let record = match self.owner_representation(&path, &path) {
                    Ok(record) => Some(record),
                    Err(QueryError::NotImplemented) => None,
                    Err(error) => return Err(error),
                };
                representations.insert(path.clone(), record);
            }
            let mut analyzed = self.mode_definition(&path)?;
            if crate::queries::has_errors(&analyzed.diagnostics) {
                return Err(QueryError::Diagnostics(analyzed.diagnostics));
            }
            let file = analyzed.value.typed.file.clone();
            let item = analyzed.value.typed.item;
            let source = &file.tree.items[item.0 as usize];
            let executable = matches!(
                &source.kind,
                ItemKind::Function { body: Some(_), .. }
                    | ItemKind::Const { .. }
                    | ItemKind::Type { drop: Some(_), .. }
            );
            source_owners.insert(
                path.clone(),
                NativeSource {
                    syntax: file.clone(),
                    item,
                    implementation_revision: source.text_hash,
                },
            );
            if executable {
                let lowered = self.lower_to_tir(&path)?;
                if crate::queries::has_errors(&lowered.diagnostics) {
                    return Err(QueryError::Diagnostics(lowered.diagnostics));
                }
                definitions.insert(path.clone(), lowered.value);
                audits.insert(
                    path.clone(),
                    SourceAudit::completed(
                        &signature.value,
                        source.text_hash,
                        &analyzed.value,
                        representations.get(&path).and_then(Option::as_ref),
                    ),
                );
                self.prepare_comptime(&mut analyzed.value, &signature.value)?;
                materialized.insert(path.clone(), analyzed.value.materialized);
            }
            if matches!(signature.value.kind, SignatureKind::Const(_)) {
                constants.insert(path.clone(), self.materialize_value(&path)?);
            }
            if matches!(
                &source.kind,
                ItemKind::Type {
                    body: hirsel_syntax::TypeBody::Distinct { fact: Some(_), .. },
                    ..
                }
            ) {
                let helper = hirsel_check_protocol::fact::fact_path(&path);
                let signature = self.item_signature(&helper)?;
                signature_dependencies.insert(helper.clone(), signature.dependencies);
                definitions.insert(helper.clone(), self.lower_to_tir(&helper)?.value);
                let (file, item) = self.parse_definition(&helper)?.value;
                let revision = file.tree.items[item.0 as usize].text_hash;
                let analyzed = self.mode_definition(&helper)?;
                if crate::queries::has_errors(&analyzed.diagnostics) {
                    return Err(QueryError::Diagnostics(analyzed.diagnostics));
                }
                audits.insert(
                    helper.clone(),
                    SourceAudit::completed(&signature.value, revision, &analyzed.value, None),
                );
                signatures.insert(helper.clone(), signature.value);
                source_owners.insert(
                    helper,
                    NativeSource {
                        syntax: file,
                        item,
                        implementation_revision: revision,
                    },
                );
            }
        }
        let diagnostics = crate::generic_graph::check(self)?;
        if crate::queries::has_errors(&diagnostics) {
            return Err(QueryError::Diagnostics(diagnostics));
        }
        // Registry constants, and the selected source constants in their
        // initialization order.
        let items: std::collections::BTreeSet<_> = self.item_paths().into_iter().collect();
        let mut constant_definitions = self.constant_definitions()?;
        constant_definitions.retain(|path| paths.contains(path) || !items.contains(path));
        for path in &constant_definitions {
            if !constants.contains_key(path) {
                constants.insert(path.clone(), self.materialize_value(path)?);
            }
        }
        let mut input = NativeInput {
            environment: self.environment.clone(),
            operations: registry::BuiltinRegistry::new().operation_inventory(),
            registry_revision: registry::BuiltinRegistry::new().revision(),
            definitions,
            signatures,
            signature_dependencies,
            representations,
            source_owners,
            sources: self.sources.clone(),
            constants,
            constant_definitions,
            materialized,
            completed_source_stages: NativeStatus::Complete,
            audits,
            instances: InstanceGraph::default(),
        };
        input.instances = InstanceGraph::native(&input)?;
        Ok(input)
    }
    /// Recheck publication invariants and compare selection against current fully
    /// checked source. A claimed completion bit or an old receipt is insufficient.
    /// This is a session artifact; serialized graph transport is M1b-owned.
    pub fn import_native_input(
        &mut self,
        input: NativeInput,
    ) -> Result<ValidatedNativeInput, QueryError> {
        if input.environment != self.environment
            || input.registry_revision != registry::BuiltinRegistry::new().revision()
            || input.completed_source_stages != NativeStatus::Complete
        {
            return Err(invalid(
                "stale environment/registry or incomplete source stage",
            ));
        }
        let current = self.export_native_input()?;
        if input.constant_definitions != current.constant_definitions {
            return Err(invalid("forged constant initialization order"));
        }
        if input.operations != current.operations {
            return Err(invalid("forged operation inventory"));
        }
        if input.definitions.keys().ne(current.definitions.keys())
            || input.signatures.keys().ne(current.signatures.keys())
            || input.source_owners.keys().ne(current.source_owners.keys())
            || input.constants.keys().ne(current.constants.keys())
            || input.materialized.keys().ne(current.materialized.keys())
            || input
                .representations
                .keys()
                .ne(current.representations.keys())
        {
            return Err(invalid("incomplete or mixed program selection"));
        }
        for (path, owner) in &input.source_owners {
            let expected = &current.source_owners[path];
            let Some(item) = owner.syntax.tree.items.get(owner.item.0 as usize) else {
                return Err(invalid("foreign source item"));
            };
            if &item.definition != path
                || owner.implementation_revision != expected.implementation_revision
                || item.text_hash != owner.implementation_revision
                || owner.syntax.as_ref() != expected.syntax.as_ref()
            {
                return Err(invalid("stale or forged source owner"));
            }
            let expected_file = current
                .sources
                .file(item.origin.span.file)
                .ok_or_else(|| invalid("missing selected source"))?;
            if input
                .sources
                .file(item.origin.span.file)
                .is_none_or(|f| f.text() != expected_file.text())
            {
                return Err(invalid("source snapshot mismatch"));
            }
        }
        for (path, signature) in &input.signatures {
            if signature.as_ref() != current.signatures[path].as_ref()
                || signature.definition != *path
                || input.signature_dependencies.get(path)
                    != current.signature_dependencies.get(path)
            {
                return Err(invalid("forged declaration snapshot"));
            }
        }
        for (path, definition) in &input.definitions {
            if definition.path != *path
                || definition.signature_hash != input.signatures[path].fingerprint
            {
                return Err(invalid("definition/declaration identity mismatch"));
            }
            let signatures = self.validated_signature_input(&path.0.package);
            let report = signatures.validate_once(definition);
            if let Some(error) = hirsel_validate::tooling_limit_error(&report) {
                return Err(error);
            }
            if !report.violations.is_empty() {
                return Err(invalid(format!(
                    "invalid imported TIR: {:?}",
                    report.violations
                )));
            }
            if definition.as_ref() != current.definitions[path].as_ref() {
                return Err(invalid("forged implementation"));
            }
        }
        if input.audits.keys().ne(input.definitions.keys())
            || input
                .audits
                .iter()
                .any(|(path, audit)| audit != &current.audits[path])
        {
            return Err(invalid("forged or missing source audit record"));
        }
        for (path, record) in &input.representations {
            if record != &current.representations[path] {
                return Err(invalid("stale or forged owner representation"));
            }
        }
        let equal_value = |a: &Materialized, b: &Materialized| {
            use hirsel_tir::ValueKind;
            let mut todo = vec![(&a.root, &b.root)];
            while let Some((left, right)) = todo.pop() {
                if hirsel_types::semantic_equal(&a.types, left.ty, &b.types, right.ty) != Ok(true) {
                    return false;
                }
                match (&left.kind, &right.kind) {
                    (ValueKind::Constant(x), ValueKind::Constant(y)) if x == y => (),
                    (ValueKind::FunctionReference(x), ValueKind::FunctionReference(y))
                        if x == y =>
                    {
                        ()
                    }
                    (
                        ValueKind::Construct {
                            constructor: x,
                            fields: xs,
                        },
                        ValueKind::Construct {
                            constructor: y,
                            fields: ys,
                        },
                    ) if x == y && xs.len() == ys.len() => todo.extend(xs.iter().zip(ys)),
                    _ => return false,
                }
            }
            true
        };
        for (path, constant) in &input.constants {
            if !equal_value(constant, &current.constants[path]) {
                return Err(invalid("forged materialized constant"));
            }
        }
        for (path, values) in &input.materialized {
            let expected = &current.materialized[path];
            if values.len() != expected.len()
                || !values
                    .iter()
                    .zip(expected)
                    .all(|((e, a), (f, b))| e == f && equal_value(a, b))
            {
                return Err(invalid("forged materialization substitutions"));
            }
        }
        if !InstanceGraph::native(&input)?.same(&input.instances) {
            return Err(invalid("forged or incomplete instance graph"));
        }
        if let Some(site) = input.instances.growing_sites().first() {
            return Err(invalid(format!(
                "instance expansion grows through an implicit instance at {site:?} (spec #87)"
            )));
        }
        let semantic_program = self.semantic_program(input.source_owners.keys().cloned())?;
        Ok(ValidatedNativeInput {
            input,
            semantic_program,
        })
    }
}

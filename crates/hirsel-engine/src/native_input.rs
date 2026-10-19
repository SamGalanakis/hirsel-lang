//! Frozen native handoff: all owners are retained; no provider reaches a backend.
use crate::*;
use hirsel_syntax::ExprId;
use hirsel_tir::DefinitionRef;
use hirsel_tir::Materialized;
use hirsel_types::{SignatureKind, SignatureRef};
use std::{collections::BTreeMap, sync::Arc};
#[derive(Clone, Debug)]
pub struct NativeSource {
    pub syntax: Arc<ItemSyntax>,
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
    /// CT-5 instance bodies of the selected generic generated declarations.
    pub instance_bodies: hirsel_tir::InstanceBodies,
    /// What the externs resolve against at link time (PROG-4).
    pub foreign_links: Vec<crate::LinkInput>,
    /// Whether executables refuse to record or replay (PROG-4).
    pub foreign_unreplayable: bool,
}
/// Construction is private to successful revalidation, including current source stages.
pub struct ValidatedNativeInput {
    input: NativeInput,
    semantic_program: crate::queries::SemanticProgram,
    escapes: BTreeMap<DefPath, crate::Escape>,
}
impl ValidatedNativeInput {
    /// The same semantic closure as source execution, retained independently of
    /// the importing engine and of native topology/artifact provenance.
    pub fn semantic_program_revision(
        &self,
        entry: &DefPath,
    ) -> Result<hirsel_run_schema::ProgramRevision, QueryError> {
        self.semantic_program.revision(entry)
    }
    /// The escape from capability checking `entry` reaches, if any (CAP-6).
    pub fn unsafe_reach(&self, entry: &DefPath) -> Option<crate::UnsafeReach> {
        self.semantic_program.reach(entry, &self.escapes)
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
        let _interval = hirsel_intern::timings::interval(hirsel_intern::timings::Interval::Plan);
        let _timing = hirsel_intern::timings::phase(hirsel_intern::timings::Phase::Plan);
        let paths = self.native_paths()?;
        self.export_native_paths(paths)
    }
    /// A build-only library surface uses explicit roots, with exactly the same
    /// source audit and independent import validation as a whole program.
    pub fn native_input_for(
        &mut self,
        roots: &[DefPath],
    ) -> Result<ValidatedNativeInput, QueryError> {
        let paths = self.native_paths_from(roots.iter().cloned())?;
        let input = self.export_native_paths(paths.clone())?;
        let current = self.export_native_paths(paths)?;
        self.import_native_against(input, current)
    }
    fn export_native_paths(
        &mut self,
        paths: std::collections::BTreeSet<DefPath>,
    ) -> Result<NativeInput, QueryError> {
        self.prepare_staged_instances()?;
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
            let syntax = self.parse_definition(&path)?.value;
            let executable = crate::queries::generation::executable(&syntax);
            source_owners.insert(
                path.clone(),
                NativeSource {
                    syntax: syntax.clone(),
                    implementation_revision: syntax.text_hash,
                },
            );
            if executable {
                let lowered = self.lower_to_tir(&path)?;
                if crate::queries::has_errors(&lowered.diagnostics) {
                    return Err(QueryError::Diagnostics(lowered.diagnostics));
                }
                definitions.insert(path.clone(), lowered.value);
                let products = self.source_audit_products(&path)?;
                audits.insert(path.clone(), products.audit);
                materialized.insert(path.clone(), products.materialized);
            } else if !self.prechecked(&path.0.package) {
                let analyzed = self.mode_definition(&path)?;
                if crate::queries::has_errors(&analyzed.diagnostics) {
                    return Err(QueryError::Diagnostics(analyzed.diagnostics));
                }
            }
            if matches!(signature.value.kind, SignatureKind::Const { ty: _, .. }) {
                constants.insert(path.clone(), self.materialize_value(&path)?);
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
        for (path, constant) in &self.staged_constants {
            if paths.contains(&constant.owner) {
                signatures.insert(path.clone(), constant.signature.clone());
            }
        }
        for ((path, arguments), value) in &self.staged_values {
            if self
                .staged_constants
                .get(path)
                .is_some_and(|c| paths.contains(&c.owner))
            {
                constants.insert(
                    hirsel_tir::constant_instance_path(path, arguments),
                    value.value.clone(),
                );
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
            instance_bodies: self.instance_bodies(&paths),
            foreign_links: self.foreign_links.clone(),
            foreign_unreplayable: self.foreign_unreplayable,
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
        let _interval = hirsel_intern::timings::interval(hirsel_intern::timings::Interval::Plan);
        let _timing = hirsel_intern::timings::phase(hirsel_intern::timings::Phase::Plan);
        let current = self.export_native_input()?;
        self.import_native_against(input, current)
    }
    fn import_native_against(
        &mut self,
        input: NativeInput,
        current: NativeInput,
    ) -> Result<ValidatedNativeInput, QueryError> {
        if input.environment != self.environment
            || input.registry_revision != registry::BuiltinRegistry::new().revision()
            || input.completed_source_stages != NativeStatus::Complete
        {
            return Err(invalid(
                "stale environment/registry or incomplete source stage",
            ));
        }
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
            let item = &owner.syntax.item;
            if &item.definition != path
                || owner.implementation_revision != expected.implementation_revision
                || owner.syntax.text_hash != owner.implementation_revision
                || owner.syntax.as_ref() != expected.syntax.as_ref()
            {
                return Err(invalid("stale or forged source owner"));
            }
            let expected_file = current
                .sources
                .file(current.sources.resolve(item.origin.span).file)
                .ok_or_else(|| invalid("missing selected source"))?;
            if input
                .sources
                .file(input.sources.resolve(item.origin.span).file)
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
            let report = self.run_v(&path.0.package, |signatures| {
                signatures.validate_once(definition)
            });
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
        if input.instance_bodies != current.instance_bodies {
            return Err(invalid("forged or incomplete instance bodies"));
        }
        let timing = hirsel_intern::timings::phase(hirsel_intern::timings::Phase::Verify);
        let violations = hirsel_validate::validate_instance_bodies(
            &input.instance_bodies,
            &input.signatures,
            &input.definitions,
        );
        drop(timing);
        if !violations.is_empty() {
            return Err(invalid(format!("invalid instance bodies: {violations:?}")));
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
        let escapes = self.escapes(&semantic_program)?;
        Ok(ValidatedNativeInput {
            input,
            semantic_program,
            escapes,
        })
    }
}

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
    /// Existing generated text and its declaring span, retained for origin
    /// diagnostics; source selectors refer to the exact TIR anchors below it.
    pub generated: Option<crate::queries::GeneratedSource>,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum NativeStatus {
    Complete,
    Unchecked,
}
/// Raw snapshots can be constructed by callers, but must be imported before use.
#[derive(Clone, Debug)]
pub struct NativeInput {
    /// The session interner every definition, signature and value TypeId
    /// names; an input is imported only by the session that holds it.
    pub types: std::sync::Arc<hirsel_types::TypeInterner>,
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
    /// Static callable placement claims. Import reconstructs required behavior
    /// from executable operands; an empty table cannot disable a demanded proof.
    pub storage_profiles: Vec<hirsel_types::storage::CallableStorageProfile>,
    /// What the externs resolve against at link time (PROG-4).
    pub foreign_links: Vec<crate::LinkInput>,
    /// Whether executables refuse to record or replay (PROG-4).
    pub foreign_unreplayable: bool,
    /// Each selected `unsafe ffi` package's `threads`; a package absent here is `home`
    /// (S-13).
    pub foreign_threads: BTreeMap<hirsel_syntax::PackageId, hirsel_types::ForeignThreads>,
}
/// Construction is private to successful revalidation, including current source stages.
pub struct ValidatedNativeInput {
    originals: Option<crate::NativeOriginalReader>,
    input: NativeInput,
    verify: Verify,
    semantic: NativeSemanticProgram,
    storage_reads: hirsel_validate::storage::StorageReads,
}
impl ValidatedNativeInput {
    pub fn original_reader(&self) -> Option<&crate::NativeOriginalReader> {
        self.originals.as_ref()
    }

    pub fn storage_proof_reads(&self) -> &hirsel_validate::storage::StorageReads {
        &self.storage_reads
    }
    /// Policy under which this current snapshot was independently checked.
    pub fn verification(&self) -> Verify {
        self.verify
    }
    /// The same semantic closure as source execution, retained independently of
    /// the importing engine and of native topology/artifact provenance.
    pub fn semantic_program_revision(
        &self,
        entry: &DefPath,
    ) -> Result<hirsel_run_schema::ProgramRevision, QueryError> {
        self.semantic.semantic_program_revision(entry)
    }
    /// The escape from capability checking `entry` reaches, if any (CAP-6).
    pub fn unsafe_reach(&self, entry: &DefPath) -> Option<crate::UnsafeReach> {
        self.semantic.unsafe_reach(entry)
    }
    /// Whether this frozen run identity covers the source definition.
    pub fn semantic_contains(&self, definition: &DefPath) -> bool {
        self.semantic.semantic_contains(definition)
    }
    /// Bodyless semantic launch binding from the same independently checked input.
    pub fn semantic_program(&self) -> &NativeSemanticProgram {
        &self.semantic
    }
    pub fn snapshot(&self) -> &NativeInput {
        &self.input
    }
    /// The first extern of a `home` package `root` reaches, on a shortest dependency path
    /// (S-13): a callback that can reach one runs only on the home thread.
    pub fn home_reach(&self, root: &DefPath) -> Option<crate::UnsafeReach> {
        self.semantic.home_reach(root)
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
        self.import_native_against(input, paths)
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
                    generated: None,
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
            } else {
                let diagnostics = self.definition_diagnostics(&path)?;
                if crate::queries::has_errors(&diagnostics) {
                    return Err(QueryError::Diagnostics(diagnostics));
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
        for (path, source) in &mut source_owners {
            source.generated = self.generated_source(path).cloned();
        }
        let registry = registry::BuiltinRegistry::shared();
        let mut input = NativeInput {
            types: self.types.clone(),
            environment: self.environment.clone(),
            operations: registry.operation_inventory(),
            registry_revision: registry.revision(),
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
            storage_profiles: crate::storage_graph::profiles(self)?,
            foreign_links: self.foreign_links.clone(),
            foreign_unreplayable: self.foreign_unreplayable,
            foreign_threads: self.foreign_threads.clone(),
        };
        input.instances = InstanceGraph::native(&input)?;
        Ok(input)
    }
    /// Recheck selection and current query products without exporting a second snapshot.
    /// Exact immutable TIR and current V reads are the only reusable validation evidence.
    pub fn import_native_input(
        &mut self,
        input: NativeInput,
    ) -> Result<ValidatedNativeInput, QueryError> {
        let _interval = hirsel_intern::timings::interval(hirsel_intern::timings::Interval::Plan);
        let _timing = hirsel_intern::timings::phase(hirsel_intern::timings::Phase::Plan);
        let paths = self.native_paths()?;
        self.import_native_against(input, paths)
    }
    fn import_native_against(
        &mut self,
        input: NativeInput,
        paths: std::collections::BTreeSet<DefPath>,
    ) -> Result<ValidatedNativeInput, QueryError> {
        if !Arc::ptr_eq(&input.types, &self.types) {
            return Err(invalid("native input from another session's interner"));
        }
        let registry = registry::BuiltinRegistry::shared();
        if input.environment != self.environment
            || input.registry_revision != registry.revision()
            || input.completed_source_stages != NativeStatus::Complete
        {
            return Err(invalid(
                "stale environment/registry or incomplete source stage",
            ));
        }
        if input.operations != registry.operation_inventory() {
            return Err(invalid("forged operation inventory"));
        }
        if input.foreign_links != self.foreign_links
            || input.foreign_unreplayable != self.foreign_unreplayable
        {
            return Err(invalid("forged foreign link selection"));
        }
        if input.source_owners.keys().ne(paths.iter()) {
            return Err(invalid("incomplete or mixed program selection"));
        }
        self.prepare_staged_instances()?;
        let mut signatures = self.native_builtin_signatures();
        let mut signature_dependencies = BTreeMap::new();
        let mut representations: BTreeMap<_, _> = signatures
            .iter()
            .filter_map(|(p, s)| {
                matches!(s.kind, SignatureKind::Type(_)).then_some((p.clone(), None))
            })
            .collect();
        let mut checked_sources = std::collections::BTreeSet::new();
        let mut executable_paths = std::collections::BTreeSet::new();
        let mut constant_paths = std::collections::BTreeSet::new();
        for path in &paths {
            let current = self.item_signature(path)?;
            if crate::queries::has_errors(&current.diagnostics) {
                return Err(QueryError::Diagnostics(current.diagnostics));
            }
            signature_dependencies.insert(path.clone(), current.dependencies);
            signatures.insert(path.clone(), current.value.clone());
            if matches!(current.value.kind, SignatureKind::Type(_)) {
                let record = match self.owner_representation(path, path) {
                    Ok(record) => Some(record),
                    Err(QueryError::NotImplemented) => None,
                    Err(error) => return Err(error),
                };
                representations.insert(path.clone(), record);
            }
            let syntax = self.parse_definition(path)?.value;
            let owner = &input.source_owners[path];
            if owner.syntax.item.definition != *path
                || owner.implementation_revision != syntax.text_hash
                || owner.syntax.text_hash != owner.implementation_revision
                || !Arc::ptr_eq(&owner.syntax, &syntax)
            {
                return Err(invalid("stale or forged source owner"));
            }
            let span = syntax.item.origin.span;
            let expected = self.sources.resolve(span);
            if input.sources.resolve(span) != expected {
                return Err(invalid("source snapshot mismatch"));
            }
            if checked_sources.insert(expected.file) {
                let expected_file = self
                    .sources
                    .file(expected.file)
                    .ok_or_else(|| invalid("missing selected source"))?;
                if input.sources.file(expected.file).is_none_or(|file| {
                    !std::ptr::eq(file, expected_file) && file.text() != expected_file.text()
                }) {
                    return Err(invalid("source snapshot mismatch"));
                }
            }
            if crate::queries::generation::executable(&syntax) {
                executable_paths.insert(path.clone());
            } else {
                let diagnostics = self.definition_diagnostics(path)?;
                if crate::queries::has_errors(&diagnostics) {
                    return Err(QueryError::Diagnostics(diagnostics));
                }
            }
            if matches!(current.value.kind, SignatureKind::Const { .. }) {
                constant_paths.insert(path.clone());
            }
        }
        if input.definitions.keys().ne(executable_paths.iter())
            || input.materialized.keys().ne(executable_paths.iter())
        {
            return Err(invalid("incomplete or mixed program selection"));
        }
        if input.audits.keys().ne(executable_paths.iter()) {
            return Err(invalid("forged or missing source audit record"));
        }
        for path in &executable_paths {
            let definition = &input.definitions[path];
            if definition.path.path() != path
                || definition.signature_hash != signatures[path].fingerprint
            {
                return Err(invalid("definition/declaration identity mismatch"));
            }
            let current = self.lower_to_tir(path)?;
            if crate::queries::has_errors(&current.diagnostics) {
                return Err(QueryError::Diagnostics(current.diagnostics));
            }
            if !Arc::ptr_eq(definition, &current.value)
                && definition.as_ref() != current.value.as_ref()
            {
                return Err(invalid("forged implementation"));
            }
            if input.source_owners[path].generated.as_ref() != self.generated_source(path) {
                return Err(invalid("forged generated source origin"));
            }
            // Revalidate every source the exact TIR cites, including generated
            // text and nested bodies. A valid declaration anchor alone cannot
            // authorize remapping a generated operation to another file/base.
            for anchor in hirsel_tir::source_anchors(definition) {
                let expected = self.sources.anchor(anchor);
                if input.sources.anchor(anchor) != expected {
                    return Err(invalid("source anchor mismatch"));
                }
                let (file, _) = expected.ok_or_else(|| invalid("missing TIR source anchor"))?;
                let expected = self
                    .sources
                    .file(file)
                    .ok_or_else(|| invalid("missing TIR source"))?;
                let actual = input
                    .sources
                    .file(file)
                    .ok_or_else(|| invalid("missing TIR source"))?;
                if actual.text() != expected.text()
                    || actual.logical_path() != expected.logical_path()
                    || actual.path != expected.path
                {
                    return Err(invalid("TIR source snapshot mismatch"));
                }
            }
            if self.verify == Verify::On {
                self.ensure_lowered_verdict(definition)?;
            }
            let products = self.source_audit_products(path)?;
            if input.audits[path] != products.audit {
                return Err(invalid("forged or missing source audit record"));
            }
            let values = &input.materialized[path];
            if values.len() != products.materialized.len()
                || !values
                    .iter()
                    .zip(&products.materialized)
                    .all(|((a, x), (b, y))| a == b && equal_native_value(&input.types, x, y))
            {
                return Err(invalid("forged materialization substitutions"));
            }
        }
        let diagnostics = crate::generic_graph::check(self)?;
        if crate::queries::has_errors(&diagnostics) {
            return Err(QueryError::Diagnostics(diagnostics));
        }
        let items: std::collections::BTreeSet<_> = self.item_paths().into_iter().collect();
        let mut constant_definitions = self.constant_definitions()?;
        constant_definitions.retain(|path| paths.contains(path) || !items.contains(path));
        if input.constant_definitions != constant_definitions {
            return Err(invalid("forged constant initialization order"));
        }
        constant_paths.extend(constant_definitions);
        for path in &constant_paths {
            let current = self.materialize_value(path)?;
            if input
                .constants
                .get(path)
                .is_none_or(|value| !equal_native_value(&input.types, value, &current))
            {
                return Err(invalid("forged materialized constant"));
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
                let path = hirsel_tir::constant_instance_path(path, arguments);
                if input
                    .constants
                    .get(&path)
                    .is_none_or(|v| !equal_native_value(&input.types, v, &value.value))
                {
                    return Err(invalid("forged materialized constant"));
                }
                constant_paths.insert(path);
            }
        }
        if input.constants.keys().ne(constant_paths.iter())
            || input.signatures.keys().ne(signatures.keys())
            || input.representations != representations
        {
            return Err(invalid(
                "incomplete or mixed program selection or owner representation",
            ));
        }
        for (path, signature) in &input.signatures {
            if (!Arc::ptr_eq(signature, &signatures[path])
                && signature.as_ref() != signatures[path].as_ref())
                || signature.definition.path() != path
                || input.signature_dependencies.get(path) != signature_dependencies.get(path)
            {
                return Err(invalid("forged declaration snapshot"));
            }
        }
        if input
            .signature_dependencies
            .keys()
            .ne(signature_dependencies.keys())
        {
            return Err(invalid("forged declaration dependencies"));
        }
        if input.instance_bodies != self.instance_bodies(&paths) {
            return Err(invalid("forged or incomplete instance bodies"));
        }
        let timing = hirsel_intern::timings::phase(hirsel_intern::timings::Phase::Verify);
        let violations = hirsel_validate::validate_instance_bodies(
            &input.types,
            &input.instance_bodies,
            &input.signatures,
            &input.definitions,
        );
        drop(timing);
        if !violations.is_empty() {
            return Err(invalid(format!("invalid instance bodies: {violations:?}")));
        }
        let storage = validate_storage_input(&input);
        if !storage.violations.is_empty() {
            return Err(invalid(format!(
                "invalid callable storage proof: {:?}",
                storage.violations
            )));
        }
        if !InstanceGraph::native(&input)?.same(&input.instances, &input.types) {
            return Err(invalid("forged or incomplete instance graph"));
        }
        if let Some(site) = input.instances.growing_sites(&input.types).first() {
            return Err(invalid(format!(
                "instance expansion grows through an implicit instance at {site:?} (spec #87)"
            )));
        }
        let semantic_program = self.semantic_program(input.source_owners.keys().cloned())?;
        let escapes = self.escapes(&semantic_program)?;
        let semantic_sources = input
            .source_owners
            .iter()
            .map(|(path, source)| {
                (
                    path.clone(),
                    (
                        source.implementation_revision,
                        input.signatures[path].fingerprint,
                    ),
                )
            })
            .collect();
        let semantic = NativeSemanticProgram::new(
            semantic_program,
            escapes,
            input.foreign_threads.clone(),
            self.signature_snapshot(),
            semantic_sources,
            input.foreign_links.clone(),
            input.foreign_unreplayable,
        );
        let has_seal = input.definitions.values().any(|definition| definition.bodies.iter().any(|body|
            body.blocks.iter().any(|block| matches!(&block.terminator,
                hirsel_tir::Terminator::Call { callee: hirsel_tir::Callee::Definition { path, .. }, .. }
                    if path.is_core("arena.seal") || path.is_core("region.with")))));
        let originals = if has_seal {
            Some(self.native_snapshot_reader(&input)?)
        } else {
            None
        };
        Ok(ValidatedNativeInput {
            originals,
            input,
            verify: self.verify,
            semantic,
            storage_reads: storage.reads,
        })
    }
}

fn validate_storage_input(input: &NativeInput) -> hirsel_validate::storage::StorageValidation {
    let has_read = input.definitions.values().any(|definition| definition.bodies.iter().any(|body| body.blocks.iter().any(|block| matches!(&block.terminator, hirsel_tir::Terminator::Call { callee: hirsel_tir::Callee::Definition { path, .. }, .. } if path.is_core("region.with")))));
    if !has_read && input.storage_profiles.is_empty() {
        return Default::default();
    }
    let mut signatures = hirsel_validate::SignatureMap::new(input.types.clone());
    signatures.extend_signatures(
        input
            .signatures
            .values()
            .map(|signature| (**signature).clone()),
    );
    let definitions: Vec<_> = input.definitions.values().cloned().collect();
    hirsel_validate::storage::validate_program(
        &input.types,
        &definitions,
        &signatures,
        &input.constants,
        &input.instance_bodies,
        &input.storage_profiles,
    )
}

fn equal_native_value(
    types: &hirsel_types::TypeInterner,
    a: &Materialized,
    b: &Materialized,
) -> bool {
    use hirsel_tir::ValueKind;
    let mut todo = vec![(&a.root, &b.root)];
    while let Some((left, right)) = todo.pop() {
        if !types.semantic_eq(left.ty, right.ty) {
            return false;
        }
        match (&left.kind, &right.kind) {
            (ValueKind::Constant(x), ValueKind::Constant(y)) if x == y => (),
            (
                ValueKind::FunctionReference {
                    definition: x,
                    arguments: xs,
                },
                ValueKind::FunctionReference {
                    definition: y,
                    arguments: ys,
                },
            ) if x == y
                && xs.len() == ys.len()
                && xs.iter().zip(ys).all(|(a, b)| {
                    matches!((hirsel_types::encoding::canonical_argument_encoding(types, a),
                        hirsel_types::encoding::canonical_argument_encoding(types, b)),
                        (Ok(a), Ok(b)) if a == b)
                }) =>
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
}

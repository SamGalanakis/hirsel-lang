//! Native-validator-owned portable envelopes. Ordinary content references and
//! producer associations confer no Value/Ready acceptance. Only actual native
//! validation can publish the local association used by this module.
use super::*;
use crate::FnAbiId;
use crate::ir::Callee;
use crate::operation::{OperationArgument, OperationDeclarations};
use crate::products::{Binding, LayoutClosure, UnitBody};
use crate::sealed_payload_source::SealSourceWitness;
use hirsel_artifact_store::{Action, Address, Contents, Domain, Reference, Store};
use hirsel_codec::snapshot_struct;
use hirsel_core_contract::{OperationId, OperationRow};
use hirsel_syntax::ContentHash;
use hirsel_syntax::DefPath;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

const RECEIPT: Domain = Domain {
    kind: 0x4300_0001,
    schema: 8,
};
const VALUE_BODY: Domain = Domain {
    kind: 0x4300_0002,
    schema: 1,
};
const READY_BODY: Domain = Domain {
    kind: 0x4300_0003,
    schema: 1,
};
const LAYOUTS: Domain = Domain {
    kind: 0x4300_0004,
    schema: 2,
};

fn body_domain(stage: StageKind) -> Option<Domain> {
    match stage {
        StageKind::Value => Some(VALUE_BODY),
        StageKind::Ready => Some(READY_BODY),
        _ => None,
    }
}
fn address(subject: ContentHash, policy: ContentHash, stage: StageKind) -> Address {
    let kind = match stage {
        StageKind::Value => 0u32,
        StageKind::Ready => 1,
        _ => 2,
    };
    Address(ContentHash::of_parts(
        "hirsel.native.validation-output.v1",
        [
            subject.0.as_slice(),
            policy.0.as_slice(),
            kind.to_le_bytes().as_slice(),
        ],
    ))
}

#[derive(Clone, Debug)]
struct Record {
    subject: ContentHash,
    policy: ContentHash,
    stage: StageKind,
    lir: u32,
    validator: u32,
    target: ContentHash,
    body: ReferenceRow,
    layouts: ReferenceRow,
    definition: DefPath,
    instance: ContentHash,
    implementation_revision: ContentHash,
    source_witness: Option<SealSourceWitness>,
    declarations: super::declaration_reads::Reads,
    linkage: super::linkage::LinkageRecord,
    operations: Vec<OperationRead>,
    manifest: Vec<(OperationId, Option<OperationRow>)>,
    discharge: ContentHash,
    metadata: Vec<u8>,
}
snapshot_struct!(Record {
    subject,
    policy,
    stage,
    lir,
    validator,
    target,
    body,
    layouts,
    definition,
    instance,
    implementation_revision,
    source_witness,
    declarations,
    linkage,
    operations,
    manifest,
    discharge,
    metadata
});

#[derive(Clone, Debug)]
struct OperationRead {
    id: OperationId,
    arguments: Vec<OperationArgument>,
    output: ContentHash,
}
snapshot_struct!(OperationRead {
    id,
    arguments,
    output
});

#[derive(Clone, Debug)]
struct ReferenceRow {
    kind: u32,
    schema: u32,
    output: ContentHash,
    len: u64,
}
snapshot_struct!(ReferenceRow {
    kind,
    schema,
    output,
    len
});
impl From<Reference> for ReferenceRow {
    fn from(r: Reference) -> Self {
        Self {
            kind: r.domain.kind,
            schema: r.domain.schema,
            output: r.output.0,
            len: r.len,
        }
    }
}
impl ReferenceRow {
    fn reference(&self) -> Reference {
        Reference {
            domain: Domain {
                kind: self.kind,
                schema: self.schema,
            },
            output: hirsel_artifact_store::Output(self.output),
            len: self.len,
        }
    }
}

/// Independently supplied current native environment. Reuse reads the exact
/// declaration/manifest/discharge observations extracted by validation itself;
/// a producer's complete action cannot replace these independent checks.
pub struct NativeEnvironment<'a> {
    pub target: &'a TargetSpec,
    pub manifest: &'a Manifest,
    pub operations: &'a (dyn OperationDeclarations + Sync),
    pub discharge: Option<&'a crate::discharge::Discharge>,
}
fn discharge_identity(plan: Option<&crate::discharge::Discharge>, owner: &DefPath) -> ContentHash {
    use crate::discharge::Form;
    let mut parts = Vec::new();
    for site in plan.and_then(|p| p.sites(owner)).into_iter().flatten() {
        parts.push(site.body.to_le_bytes().to_vec());
        parts.push(site.block.to_le_bytes().to_vec());
        parts.push(site.statement.to_le_bytes().to_vec());
        match &site.form {
            Form::Check(rule) => {
                parts.push(vec![0]);
                parts.push(rule.as_bytes().to_vec());
            }
            Form::Integer => parts.push(vec![1]),
            Form::Negation => parts.push(vec![2]),
            Form::Projection { place, rule } => {
                parts.push(vec![3]);
                parts.push(place.to_le_bytes().to_vec());
                parts.push(rule.as_bytes().to_vec());
            }
        }
    }
    ContentHash::of_parts(
        "hirsel.native.discharge-input.v1",
        parts.iter().map(Vec::as_slice),
    )
}
fn operation_identity(
    env: &NativeEnvironment<'_>,
    id: &OperationId,
    args: &[OperationArgument],
) -> Option<ContentHash> {
    let (abi, table) = env.operations.derive(id, args, env.target).ok()?;
    crate::layout_validate::validate_table(&table, env.target).ok()?;
    Some(crate::LayoutKeys::new(&table).fn_abi(&abi))
}

/// Opaque local native acceptance. No Deserialize or producer constructor.
/// Metadata is ordinary query data and never another verifier capability.
#[derive(Clone, Debug)]
pub struct NativeOutput {
    record: Arc<Record>,
}
impl NativeOutput {
    pub fn body(&self) -> Reference {
        self.record.body.reference()
    }
    pub fn layouts(&self) -> Reference {
        self.record.layouts.reference()
    }
    pub fn metadata(&self) -> &[u8] {
        &self.record.metadata
    }
    pub fn stage(&self) -> StageKind {
        self.record.stage
    }
    pub fn linkage(&self) -> super::CheckedLinkage {
        self.record.linkage.checked()
    }
}

/// Uses the selected ordinary content provider. The only extra table is local
/// native proof authority; it is never a semantic invalidation graph/provider.
pub struct NativeCache {
    contents: Arc<dyn Contents>,
    local: Option<Store>,
    accepted: Mutex<BTreeMap<ContentHash, Arc<Record>>>,
}
impl NativeCache {
    /// The host supplies its explicit own writable Store, never a read-only
    /// packaged/remote root. Without that capability receipts are process-local.
    pub fn new(contents: Arc<dyn Contents>, local: Option<Store>) -> Self {
        Self {
            contents,
            local,
            accepted: Mutex::default(),
        }
    }

    fn get(&self, reference: Reference) -> Option<hirsel_artifact_store::Bytes> {
        let bytes = self.contents.get_content(reference)?;
        (reference.domain.reference(&bytes)? == reference).then_some(bytes)
    }
    fn put(&self, domain: Domain, bytes: &[u8]) -> Option<Reference> {
        let expected = domain.reference(bytes)?;
        if self.contents.put_content(domain, bytes)? != expected {
            return None;
        }
        Some(expected)
    }

    /// Reconstruct acceptance from this validator's own association only.
    /// The caller first validates the query's complete current dependency
    /// outputs, including source/private proof, layouts, graph, policy and all
    /// explicit callee/optimizer inputs, to obtain `subject`.
    pub fn lookup(
        &self,
        subject: ContentHash,
        policy: ContentHash,
        stage: StageKind,
        environment: &NativeEnvironment<'_>,
    ) -> Option<NativeOutput> {
        let domain = body_domain(stage)?;
        let key = address(subject, policy, stage);
        let record = self
            .accepted
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&key.0)
            .cloned()
            .or_else(|| {
                let local = self.local.as_ref()?;
                let association = local.lookup(key)?;
                if association.action != Action(key.0) || association.reference.domain != RECEIPT {
                    return None;
                }
                let bytes = local.get(association.reference)?;
                let record: Record =
                    hirsel_codec::from_text(std::str::from_utf8(&bytes).ok()?).ok()?;
                Some(Arc::new(record))
            })?;
        if record.subject != subject
            || record.policy != policy
            || record.stage != stage
            || record.lir != crate::LIR_VERSION
            || record.validator != crate::VALIDATOR_VERSION
            || record.target != environment.target.digest()
            || record.body.reference().domain != domain
            || record.layouts.reference().domain != LAYOUTS
        {
            return None;
        }
        if record
            .manifest
            .iter()
            .any(|(id, row)| environment.manifest.row(id) != row.as_ref())
            || record.operations.iter().any(|read| {
                operation_identity(environment, &read.id, &read.arguments) != Some(read.output)
            })
            || !record
                .declarations
                .matches(environment.operations, environment.target)
            || record.discharge != discharge_identity(environment.discharge, &record.definition)
            || record.source_witness
                != environment
                    .operations
                    .seal_source_witness(&record.definition, record.instance)
            || record.source_witness.as_ref().is_some_and(|witness| {
                witness.definition != record.definition
                    || witness.instance != record.instance
                    || witness.implementation_revision != record.implementation_revision
            })
        {
            return None;
        }
        // Value and Ready bodies remain unopened. Structural declaration reads
        // may bind only the portable layout metadata needed for reconstruction.
        self.get(record.body.reference())?;
        let layouts = self.get(record.layouts.reference())?;
        if record.declarations.has_structural() {
            let closure: LayoutClosure =
                hirsel_codec::from_text(std::str::from_utf8(&layouts).ok()?).ok()?;
            let binding = Binding::new(environment.target, &[&closure]).ok()?;
            if !record.declarations.matches_structural(
                environment.operations,
                &binding.table,
                environment.target,
            ) {
                return None;
            }
        }
        Some(NativeOutput { record })
    }

    /// Validate actual operations and complete independently supplied inputs,
    /// then choose and publish the exact body/layout envelopes ourselves.
    /// A producer cannot pair its own arbitrary reference with a receipt.
    #[allow(clippy::too_many_arguments)]
    pub fn validate_and_publish<S: Stage>(
        &self,
        subject: ContentHash,
        policy: ContentHash,
        unit: LirUnit<S>,
        inputs: &Inputs<'_>,
        additional_abis: &[FnAbiId],
        metadata: &[u8],
    ) -> Result<(ValidatedUnit<S>, Option<NativeOutput>), Box<Rejected<S>>> {
        let (checked, output) =
            self.validate_output(subject, policy, unit, inputs, additional_abis)?;
        let output = output.and_then(|output| self.publish_output(&output, metadata));
        Ok((checked, output))
    }

    /// Run actual C once and capture its chosen body/layouts and independent
    /// observations. The opaque output is not installed as a successful lookup
    /// association until publish_output, so a query can first derive ordinary
    /// local equations and discovery metadata from the checked unit.
    pub fn validate_output<S: Stage>(
        &self,
        subject: ContentHash,
        policy: ContentHash,
        unit: LirUnit<S>,
        inputs: &Inputs<'_>,
        additional_abis: &[FnAbiId],
    ) -> Result<(ValidatedUnit<S>, Option<NativeOutput>), Box<Rejected<S>>> {
        let observed = super::declaration_reads::Observed::new(inputs.operations);
        let checked_inputs = Inputs {
            target: inputs.target,
            layouts: inputs.layouts,
            manifest: inputs.manifest,
            operations: &observed,
            discharge: inputs.discharge,
            memo: Default::default(),
        };
        let checked = validate(unit, &checked_inputs)?;
        let capture = || -> Option<NativeOutput> {
            let domain = body_domain(S::KIND)?;
            // Observe the independent original-source provider, never candidate
            // metadata. Unavailable is retained as None, not expected-empty.
            let header = &checked.unit().header;
            let source_witness = inputs
                .operations
                .seal_source_witness(&header.definition, header.instance);
            if source_witness.as_ref().is_some_and(|witness| {
                witness.definition != header.definition
                    || witness.instance != header.instance
                    || witness.implementation_revision != header.implementation_revision
            }) {
                return None;
            }
            let (closure, body) = LayoutClosure::capture(&checked, inputs, additional_abis).ok()?;
            let binding = Binding::new(inputs.target, &[&closure]).ok()?;
            let declarations = observed.finish(&binding.table, inputs.layouts, inputs.target)?;
            let body = self.put(domain, hirsel_codec::to_text(&body).as_bytes())?;
            let layouts = self.put(LAYOUTS, hirsel_codec::to_text(&closure).as_bytes())?;
            let environment = NativeEnvironment {
                target: inputs.target,
                manifest: inputs.manifest,
                operations: inputs.operations,
                discharge: inputs.discharge,
            };
            let mut operation_reads = BTreeMap::new();
            for call in &checked.unit().tables.calls {
                if let Callee::Operation(op) = call.callee {
                    let id = &checked.unit().tables.operations[op.index()];
                    let hash = operation_identity(&environment, id, &call.operation_arguments)?;
                    let encoding =
                        hirsel_codec::to_text(&(id.clone(), call.operation_arguments.clone()));
                    operation_reads.insert(
                        encoding,
                        OperationRead {
                            id: id.clone(),
                            arguments: call.operation_arguments.clone(),
                            output: hash,
                        },
                    );
                }
            }
            let record = Arc::new(Record {
                subject,
                policy,
                stage: S::KIND,
                lir: crate::LIR_VERSION,
                validator: crate::VALIDATOR_VERSION,
                target: inputs.target.digest(),
                body: body.into(),
                layouts: layouts.into(),
                definition: header.definition.clone(),
                instance: header.instance,
                implementation_revision: header.implementation_revision,
                source_witness,
                declarations,
                linkage: super::linkage::LinkageRecord::capture(&checked, inputs.layouts)?,
                operations: operation_reads.into_values().collect(),
                manifest: checked
                    .unit()
                    .tables
                    .operations
                    .iter()
                    .map(|id| (id.clone(), inputs.manifest.row(id).cloned()))
                    .collect(),
                discharge: discharge_identity(inputs.discharge, &checked.unit().header.definition),
                metadata: Vec::new(),
            });
            Some(NativeOutput { record })
        };
        let output = capture();
        Ok((checked, output))
    }

    /// Attach ordinary query metadata to an actual C-issued output, retaining
    /// every independent observation verbatim. No body, target, policy, stage,
    /// operation, source witness or discharge evidence can be replaced here.
    /// Lookup still rechecks the current independent environment.
    pub fn publish_output(&self, output: &NativeOutput, metadata: &[u8]) -> Option<NativeOutput> {
        if output.record.lir != crate::LIR_VERSION
            || output.record.validator != crate::VALIDATOR_VERSION
            || body_domain(output.stage()) != Some(output.body().domain)
            || output.layouts().domain != LAYOUTS
        {
            return None;
        }
        self.get(output.body())?;
        self.get(output.layouts())?;
        let mut record = (*output.record).clone();
        record.metadata = metadata.to_vec();
        let record = Arc::new(record);
        let key = address(record.subject, record.policy, record.stage);
        if let Some(local) = &self.local {
            let bytes = hirsel_codec::to_text(record.as_ref());
            if let Ok(reference) = local.put(RECEIPT, bytes.as_bytes()) {
                let _ = local.associate(key, Action(key.0), reference);
            }
        }
        self.accepted
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(key.0, record.clone());
        Some(NativeOutput { record })
    }

    /// Obtain portable materialization inputs from an already accepted output.
    /// Native stage validation still seals the actual temporary bound unit.
    pub fn read(
        &self,
        output: &NativeOutput,
        target: &TargetSpec,
    ) -> Result<(LayoutClosure, UnitBody), String> {
        if output.record.target != target.digest()
            || output.record.lir != crate::LIR_VERSION
            || output.record.validator != crate::VALIDATOR_VERSION
        {
            return Err("native output environment changed".into());
        }
        let closure = self
            .get(output.layouts())
            .ok_or("missing native layout product")?;
        let body = self
            .get(output.body())
            .ok_or("missing native body product")?;
        let closure: LayoutClosure =
            hirsel_codec::from_text(std::str::from_utf8(&closure).map_err(|e| e.to_string())?)
                .map_err(|e| format!("native layout product: {e:?}"))?;
        let body: UnitBody =
            hirsel_codec::from_text(std::str::from_utf8(&body).map_err(|e| e.to_string())?)
                .map_err(|e| format!("native body product: {e:?}"))?;
        closure.check(target)?;
        Ok((closure, body))
    }

    /// A self-contained local materialization for a demanded transformation.
    pub fn materialize<S: Stage>(
        &self,
        output: &NativeOutput,
        inputs: &Inputs<'_>,
    ) -> Result<ValidatedUnit<S>, String> {
        if output.stage() != S::KIND {
            return Err("native output stage differs".into());
        }
        let (closure, body) = self.read(output, inputs.target)?;
        let binding = Binding::new(inputs.target, &[&closure])?;
        if &binding.table != inputs.layouts {
            return Err("native materialization needs its bound local table".into());
        }
        let unit = binding.materialize::<S>(&body)?;
        validate(unit, inputs).map_err(|e| format!("native output refused: {:?}", e.violations))
    }
}

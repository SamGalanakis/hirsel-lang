//! The local Ready miss and descriptor cutoff. Placement/RC/Reuse use the sole
//! existing pass implementation; actual final C owns every accepted envelope.
use super::*;
use crate::native::stages::{LocalRcFacts, lower_local_raw};
use hirsel_artifact_store::{Action, Contents, Domain, Reference};
use hirsel_engine::store::ResultStore;
use hirsel_lir::stage::{Ready, StageKind, ValidatedUnit, Value};
use hirsel_lir::validate::{Inputs, NativeCache, NativeEnvironment, ValidationContext};
use hirsel_rc::instance::{InstanceId, Instances};

pub const EMISSION_CONTEXT_DOMAIN: Domain = Domain {
    kind: 0x4e00_0014,
    schema: 1,
};
pub const UNIT_SUMMARY_DOMAIN: Domain = Domain {
    kind: 0x4e00_0015,
    schema: 1,
};

struct SelectedContents(Arc<dyn ResultStore>);
impl Contents for SelectedContents {
    fn get_content(&self, reference: Reference) -> Option<hirsel_artifact_store::Bytes> {
        self.0.get_content(reference)
    }
    fn put_content(&self, domain: Domain, bytes: &[u8]) -> Option<Reference> {
        self.0.put_content(domain, bytes)
    }
}
fn subject(key: &InstanceAddress, action: Action) -> ContentHash {
    ContentHash::of_parts(
        "hirsel.native.ready-validation-subject.v1",
        [
            key.address(READY_KIND).0.0.as_slice(),
            action.0.0.as_slice(),
        ],
    )
}
fn record(product: &ReadyProduct) -> ReadyRecord {
    (
        product.debug,
        product.ready,
        product.layout_closure,
        product.emission_context,
        product.unit_summary,
        product
            .optimizer_reads
            .iter()
            .map(|p| (p.address.0, p.record))
            .collect(),
    )
}

/// Compact callable facts captured with an instance's source discovery.
/// These are ordinary descriptor data, bound to the accepted instance's
/// discovery output and current source observations by the query owner.
/// Ready does not need the source body or an Instances registry to use them.
#[derive(Clone, Debug)]
pub struct ReadySourceFacts {
    origin: hirsel_layout::instance::InstanceKey,
    parameters: u32,
    noalloc: bool,
    unique: Vec<u32>,
}
impl ReadySourceFacts {
    pub fn for_instance(instances: &Instances, id: InstanceId) -> Self {
        let proofs = hirsel_rc::insert::Proofs::for_instance(instances, id);
        Self {
            origin: instances.view(id).key.clone(),
            parameters: instances.view(id).parameters.len() as u32,
            noalloc: proofs.noalloc,
            unique: proofs.unique,
        }
    }

    /// Portable metadata only; no TypeId, LayoutId or retained source body.
    pub fn encode(&self) -> Result<Vec<u8>, String> {
        serde_json::to_vec(&(
            1u32,
            self.origin.definition(),
            self.origin.body(),
            self.origin.encoding(),
            self.parameters,
            self.noalloc,
            &self.unique,
        ))
        .map_err(|e| e.to_string())
    }
    pub fn decode(bytes: &[u8]) -> Result<Self, String> {
        let (schema, definition, body, encoding, parameters, noalloc, unique): (
            u32,
            hirsel_syntax::DefPath,
            u32,
            Vec<u8>,
            u32,
            bool,
            Vec<u32>,
        ) = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
        if schema != 1
            || unique.windows(2).any(|w| w[0] >= w[1])
            || unique.iter().any(|&p| p >= parameters)
        {
            return Err("noncanonical Ready source facts".into());
        }
        let origin =
            hirsel_layout::instance::InstanceKey::from_encoding(&definition, body, encoding)
                .map_err(|e| format!("Ready source origin: {e:?}"))?;
        Ok(Self {
            origin,
            parameters,
            noalloc,
            unique,
        })
    }
    fn local(&self, regions: bool) -> LocalRcFacts {
        LocalRcFacts {
            placement: hirsel_rc::placement::Options {
                regions,
                noalloc: self.noalloc,
            },
            proofs: hirsel_rc::insert::Proofs {
                noalloc: self.noalloc,
                unique: self.unique.clone(),
            },
        }
    }
}

/// Inputs borrowed only for a demanded Ready transformation. The caller has
/// accepted the Value product and settled its arena/scoped/optimizer reads;
/// the complete action binds those outputs and the exact local source facts.
pub struct ReadyMiss<'a> {
    pub value: hirsel_lir::LirUnit<Value>,
    pub options: &'a crate::native::Options,
    pub source: &'a ReadySourceFacts,
    pub finalizers: &'a hirsel_rc::cleanup::Finalizers,
    pub inputs: &'a Inputs<'a>,
    pub additional_abis: &'a [hirsel_lir::FnAbiId],
    pub statics: &'a [hirsel_lir::statics::StaticData],
    pub emission_context: Reference,
    pub debug: bool,
    pub optimizer_reads: Vec<ProductRef>,
}
#[derive(Default, Debug)]
pub struct ReadyQueryCounts {
    pub hits: u64,
    pub misses: u64,
    pub transformations: u64,
    pub validations: u64,
}
pub struct ReadyQueries {
    store: Arc<dyn ResultStore>,
    native: Arc<NativeCache>,
    pub counts: ReadyQueryCounts,
}
impl ReadyQueries {
    pub fn new(store: Arc<dyn ResultStore>) -> Self {
        let native = Self::validator(&store);
        Self::with_validator(store, native)
    }
    pub(super) fn validator(store: &Arc<dyn ResultStore>) -> Arc<NativeCache> {
        Arc::new(NativeCache::new(
            Arc::new(SelectedContents(store.clone())),
            store.local_product_store(),
        ))
    }
    pub(super) fn with_validator(store: Arc<dyn ResultStore>, native: Arc<NativeCache>) -> Self {
        Self {
            native,
            store,
            counts: ReadyQueryCounts::default(),
        }
    }
    pub fn lookup(
        &mut self,
        key: &InstanceAddress,
        action: Action,
        environment: &NativeEnvironment<'_>,
    ) -> Option<Arc<ReadyProduct>> {
        let result = self.lookup_inner(key, action, environment);
        if result.is_some() {
            self.counts.hits += 1;
        } else {
            self.counts.misses += 1;
        }
        result
    }
    fn lookup_inner(
        &self,
        key: &InstanceAddress,
        action: Action,
        environment: &NativeEnvironment<'_>,
    ) -> Option<Arc<ReadyProduct>> {
        if key.target_abi != environment.target.digest() {
            return None;
        }
        let address = key.address(READY_KIND);
        let previous = self.store.lookup_product(address)?;
        if previous.action != action || previous.reference.domain != READY_RECORD_DOMAIN {
            return None;
        }
        let accepted = self.native.lookup(
            subject(key, action),
            key.policy,
            StageKind::Ready,
            environment,
        )?;
        let product = ready_product(
            self.store.as_ref(),
            ProductRef {
                address,
                action,
                record: previous.reference,
            },
        )
        .ok()?;
        let expected: ReadyRecord = serde_json::from_slice(accepted.metadata()).ok()?;
        if expected != record(&product)
            || product.ready != accepted.body()
            || product.layout_closure != accepted.layouts()
            || product.emission_context.domain != EMISSION_CONTEXT_DOMAIN
            || product.unit_summary.domain != UNIT_SUMMARY_DOMAIN
            || (product.debug && !product.optimizer_reads.is_empty())
            || self.store.get_content(product.emission_context).is_none()
            || self.store.get_content(product.unit_summary).is_none()
        {
            return None;
        }
        Some(Arc::new(product))
    }

    /// Obtain only C's own linkage observations after the same current
    /// descriptor/environment checks used by Ready admission. No body decode.
    pub fn linkage(
        &self,
        key: &InstanceAddress,
        action: Action,
        environment: &NativeEnvironment<'_>,
    ) -> Option<hirsel_lir::validate::CheckedLinkage> {
        self.lookup_inner(key, action, environment)?;
        self.native
            .lookup(
                subject(key, action),
                key.policy,
                StageKind::Ready,
                environment,
            )
            .map(|accepted| accepted.linkage())
    }

    /// Read only the C-bound layout closure for program assembly. The Ready
    /// body remains unopened; the summary reference is separately C-bound in
    /// the same accepted metadata checked by lookup_inner.
    pub(super) fn assembly_layouts(
        &self,
        key: &InstanceAddress,
        product: &ReadyProduct,
        environment: &NativeEnvironment<'_>,
    ) -> Result<hirsel_lir::products::LayoutClosure, String> {
        let current = self
            .lookup_inner(key, product.product.action, environment)
            .ok_or("Ready assembly lost current C admission")?;
        if current.product != product.product {
            return Err("Ready assembly descriptor differs".into());
        }
        let bytes = self
            .store
            .get_content(current.layout_closure)
            .ok_or("missing Ready assembly layouts")?;
        let closure: hirsel_lir::products::LayoutClosure =
            hirsel_codec::from_text(std::str::from_utf8(&bytes).map_err(|e| e.to_string())?)
                .map_err(|e| format!("Ready assembly layouts: {e:?}"))?;
        closure.check(environment.target)?;
        Ok(closure)
    }

    /// Body access reserved for a selected emission miss. The supplied
    /// descriptor must still match this query's current C-owned association.
    pub(super) fn read(
        &self,
        key: &InstanceAddress,
        product: &ReadyProduct,
        environment: &NativeEnvironment<'_>,
    ) -> Result<
        (
            hirsel_lir::products::LayoutClosure,
            hirsel_lir::products::UnitBody,
        ),
        String,
    > {
        let current = self
            .lookup_inner(key, product.product.action, environment)
            .ok_or("Ready materialization lost current C admission")?;
        if current.product != product.product {
            return Err("Ready materialization descriptor differs".into());
        }
        let accepted = self
            .native
            .lookup(
                subject(key, product.product.action),
                key.policy,
                StageKind::Ready,
                environment,
            )
            .ok_or("Ready materialization lost native output")?;
        self.native.read(&accepted, environment.target)
    }

    /// One actual local miss and final C. No proof is inferred from the input
    /// references, the pass outputs, or a producer's supplied summary.
    pub fn publish_from_value(
        &mut self,
        key: &InstanceAddress,
        action: Action,
        miss: ReadyMiss<'_>,
    ) -> Result<(ValidatedUnit<Ready>, Arc<ReadyProduct>), String> {
        if !key.instance.is_same_or_derived_from(&miss.source.origin)
            || miss.value.header.definition != *key.instance.definition()
            || miss.value.header.instance != key.instance.digest()
            || miss.value.header.target != key.target_abi
            || miss.inputs.target.digest() != key.target_abi
            || miss.options.target.digest() != key.target_abi
            || miss.emission_context.domain != EMISSION_CONTEXT_DOMAIN
            || self.store.get_content(miss.emission_context).is_none()
            || (miss.debug && !miss.optimizer_reads.is_empty())
        {
            return Err("Ready miss inputs differ from the selected source instance".into());
        }
        let mut seen = std::collections::BTreeSet::new();
        for dependency in &miss.optimizer_reads {
            if !seen.insert(dependency.address.0)
                || ready_product(self.store.as_ref(), *dependency).is_err()
            {
                return Err("unsettled or duplicate Ready optimizer dependency".into());
            }
        }
        let inputs = miss.inputs;
        let environment = NativeEnvironment {
            target: inputs.target,
            manifest: inputs.manifest,
            operations: inputs.operations,
            discharge: inputs.discharge,
        };
        let previous = self
            .lookup_inner(key, action, &environment)
            .map(|p| p.product.record);
        let facts = miss.source.local(miss.options.regions);
        let context = ValidationContext::new(inputs);
        context
            .preflight()
            .map_err(|errors| format!("Ready input table: {errors:?}"))?;
        self.counts.transformations += 1;
        let ready = lower_local_raw(
            miss.value,
            miss.options,
            &facts,
            miss.finalizers,
            inputs,
            &context,
        )
        .map_err(|e| e.to_string())?;
        self.counts.validations += 1;
        let (checked, output) = self
            .native
            .validate_output(
                subject(key, action),
                key.policy,
                ready,
                inputs,
                miss.additional_abis,
            )
            .map_err(|rejected| format!("native Ready refused: {:?}", rejected.violations))?;
        crate::native::emit::check_unit_runtime_calls(
            checked.unit(),
            inputs.layouts,
            inputs.manifest,
        )
        .map_err(|error| error.to_string())?;
        let output = output.ok_or("native Ready transport failed")?;
        let keys = hirsel_lir::LayoutKeys::new(inputs.layouts);
        let mut summary = hirsel_lir::fragment::UnitSummary::of(
            checked.unit(),
            inputs.layouts,
            &keys,
            inputs.manifest,
        )?;
        summary.add_static_references(miss.statics);
        let summary = self
            .store
            .put_content(UNIT_SUMMARY_DOMAIN, &summary.encode(inputs.layouts, &keys)?)
            .ok_or("Ready summary transport failed")?;
        let optimizer = miss
            .optimizer_reads
            .iter()
            .map(|p| (p.address.0, p.record))
            .collect();
        let payload: ReadyRecord = (
            miss.debug,
            output.body(),
            output.layouts(),
            miss.emission_context,
            summary,
            optimizer,
        );
        let metadata = serde_json::to_vec(&payload).map_err(|e| e.to_string())?;
        let expected_record = READY_RECORD_DOMAIN
            .reference(&metadata)
            .ok_or("Ready descriptor exceeds the record limit")?;
        if previous.is_some_and(|previous| previous != expected_record) {
            return Err(
                "Ready recomputation changed output under unchanged complete action".into(),
            );
        }
        self.native
            .publish_output(&output, &metadata)
            .ok_or("native Ready publication failed")?;
        let product = publish_ready_record(
            self.store.as_ref(),
            key.address(READY_KIND),
            action,
            &payload,
        )?;
        if product.record != expected_record {
            return Err("Ready descriptor content identity differs".into());
        }
        Ok((
            checked,
            Arc::new(ReadyProduct {
                product,
                debug: miss.debug,
                ready: payload.1,
                layout_closure: payload.2,
                emission_context: payload.3,
                unit_summary: payload.4,
                optimizer_reads: miss.optimizer_reads,
            }),
        ))
    }
}

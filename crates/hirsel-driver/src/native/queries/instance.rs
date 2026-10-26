//! Immutable instance descriptors backed by native C's own output authority.
//! The caller validates current source V/dependency outputs before providing
//! the complete action. Ordinary producer associations alone never accept a hit.
use super::arena::EQUATIONS_DOMAIN;
use super::*;
use hirsel_engine::store::ResultStore;
use hirsel_engine::store::{Action, Contents, Domain, Reference};
use hirsel_lir::stage::{StageKind, ValidatedUnit, Value};
use hirsel_lir::validate::{Inputs, NativeCache, NativeEnvironment};

pub const INSTANCE_RECORD_DOMAIN: Domain = Domain {
    kind: INSTANCE_KIND,
    schema: 1,
};
pub const INSTANCE_DISCOVERY_DOMAIN: Domain = Domain {
    kind: 0x4e00_0011,
    schema: 9,
};
pub const STATICS_FINALIZERS_DOMAIN: Domain = Domain {
    kind: 0x4e00_0012,
    schema: 1,
};

/// The rewritten Value is a distinct semantic query stage while retaining the
/// source callable's canonical function identity. C subjects and associations
/// are separated; ordinary records never carry acceptance across the stages.
pub const CALLABLE_VALUE_KIND: u32 = 0x4e00_0029;

#[derive(Clone, Copy)]
enum Phase {
    Imported,
    Callable,
}
impl Phase {
    fn kind(self) -> u32 {
        match self {
            Self::Imported => INSTANCE_KIND,
            Self::Callable => CALLABLE_VALUE_KIND,
        }
    }
}

type Record = (Reference, Reference, Reference, Reference, Reference);
type Metadata = (Reference, Reference, Reference);

struct SelectedContents(Arc<dyn ResultStore>);
impl Contents for SelectedContents {
    fn get_content(&self, reference: Reference) -> Option<hirsel_engine::store::Bytes> {
        self.0.get_content(reference)
    }
    fn put_content(&self, domain: Domain, bytes: &[u8]) -> Option<Reference> {
        self.0.put_content(domain, bytes)
    }
}

fn subject(phase: Phase, key: &InstanceAddress, action: Action) -> ContentHash {
    ContentHash::of_parts(
        "hirsel.native.instance-validation-subject.v1",
        [
            key.address(phase.kind()).0.0.as_slice(),
            action.0.0.as_slice(),
        ],
    )
}

#[derive(Default, Debug)]
pub struct InstanceQueryCounts {
    pub hits: u64,
    pub misses: u64,
    pub validations: u64,
}

/// Uses the single selected request transport. Its only private table is native
/// C authority, never another semantic dependency graph or content provider.
pub struct InstanceQueries {
    store: Arc<dyn ResultStore>,
    native: Arc<NativeCache>,
    phase: Phase,
    pub counts: InstanceQueryCounts,
}
impl InstanceQueries {
    pub fn new(store: Arc<dyn ResultStore>) -> Self {
        let native = Self::validator(&store);
        Self::with_validator(store, native)
    }
    pub fn new_callable(store: Arc<dyn ResultStore>) -> Self {
        let native = Self::validator(&store);
        Self::with_phase(store, native, Phase::Callable)
    }
    pub fn address(&self, key: &InstanceAddress) -> hirsel_engine::store::Address {
        key.address(self.phase.kind())
    }
    pub(super) fn validator(store: &Arc<dyn ResultStore>) -> Arc<NativeCache> {
        Arc::new(NativeCache::new(
            Arc::new(SelectedContents(store.clone())),
            store.local_product_store(),
        ))
    }
    pub(super) fn with_validator(store: Arc<dyn ResultStore>, native: Arc<NativeCache>) -> Self {
        Self::with_phase(store, native, Phase::Imported)
    }
    fn with_phase(store: Arc<dyn ResultStore>, native: Arc<NativeCache>, phase: Phase) -> Self {
        Self {
            native,
            store,
            phase,
            counts: InstanceQueryCounts::default(),
        }
    }

    fn metadata_present(&self, metadata: Metadata) -> bool {
        [
            (metadata.0, EQUATIONS_DOMAIN),
            (metadata.1, INSTANCE_DISCOVERY_DOMAIN),
            (metadata.2, STATICS_FINALIZERS_DOMAIN),
        ]
        .into_iter()
        .all(|(reference, domain)| {
            reference.domain == domain && self.store.get_content(reference).is_some()
        })
    }

    /// Ordinary prior metadata for dependency revalidation. This performs no
    /// native acceptance, Value decode or binding; callers must still use
    /// lookup with a complete current action and independent environment.
    pub fn candidate(&self, key: &InstanceAddress) -> Option<InstanceProduct> {
        let address = self.address(key);
        let previous = self.store.lookup_product(address)?;
        if previous.reference.domain != INSTANCE_RECORD_DOMAIN {
            return None;
        }
        let bytes = self.store.get_content(previous.reference)?;
        let record: Record = serde_json::from_slice(&bytes).ok()?;
        if !self.metadata_present((record.2, record.3, record.4)) {
            return None;
        }
        Some(InstanceProduct {
            product: ProductRef {
                address,
                action: previous.action,
                record: previous.reference,
            },
            value: record.0,
            layout_closure: record.1,
            local_equations: record.2,
            discovery: record.3,
            statics_finalizers: record.4,
        })
    }

    /// Adopt only after both the ordinary complete action and C's independently
    /// rechecked receipt agree. This reads descriptors/content integrity; it
    /// never decodes a Value or constructs an instance body. C may bind layout
    /// metadata to reconstruct an independently consumed structural declaration.
    pub fn lookup(
        &mut self,
        key: &InstanceAddress,
        action: Action,
        environment: &NativeEnvironment<'_>,
    ) -> Option<Arc<InstanceProduct>> {
        let output = self.lookup_inner(key, action, environment);
        if output.is_some() {
            self.counts.hits += 1;
        } else {
            self.counts.misses += 1;
        }
        output
    }

    /// C-owned header observations after ordinary descriptor and current
    /// environment admission. No Value body is decoded or bound.
    pub fn linkage(
        &self,
        key: &InstanceAddress,
        action: Action,
        environment: &NativeEnvironment<'_>,
    ) -> Option<hirsel_lir::validate::CheckedLinkage> {
        self.lookup_inner(key, action, environment)?;
        self.native
            .lookup(
                subject(self.phase, key, action),
                key.policy,
                StageKind::Value,
                environment,
            )
            .map(|output| output.linkage())
    }
    fn lookup_inner(
        &self,
        key: &InstanceAddress,
        action: Action,
        environment: &NativeEnvironment<'_>,
    ) -> Option<Arc<InstanceProduct>> {
        if key.target_abi != environment.target.digest() {
            return None;
        }
        let candidate = self.candidate(key)?;
        if candidate.product.action != action {
            return None;
        }
        let accepted = self.native.lookup(
            subject(self.phase, key, action),
            key.policy,
            StageKind::Value,
            environment,
        )?;
        let metadata: Metadata = serde_json::from_slice(accepted.metadata()).ok()?;
        if !self.metadata_present(metadata) {
            return None;
        }
        if (
            candidate.value,
            candidate.layout_closure,
            candidate.local_equations,
            candidate.discovery,
            candidate.statics_finalizers,
        ) != (
            accepted.body(),
            accepted.layouts(),
            metadata.0,
            metadata.1,
            metadata.2,
        ) {
            return None;
        }
        Some(Arc::new(candidate))
    }

    /// Open a selected accepted Value only for a transformation miss. The
    /// descriptor must still match this stage's own current C admission.
    pub fn read(
        &self,
        key: &InstanceAddress,
        product: &InstanceProduct,
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
            .ok_or("Value materialization lost current C admission")?;
        if current.product != product.product {
            return Err("Value materialization descriptor differs".into());
        }
        let accepted = self
            .native
            .lookup(
                subject(self.phase, key, product.product.action),
                key.policy,
                StageKind::Value,
                environment,
            )
            .ok_or("Value materialization lost native output")?;
        self.native.read(&accepted, environment.target)
    }

    /// Actual one-instance miss: C chooses the Value/layout envelopes; equations
    /// come from that checked Value; only then do all auxiliary outputs and the
    /// descriptor become available. The source/implicit discovery and static
    /// records are ordinary compiler outputs, with explicit independent domains.
    #[allow(clippy::too_many_arguments)]
    pub fn publish(
        &mut self,
        key: &InstanceAddress,
        action: Action,
        unit: hirsel_lir::LirUnit<Value>,
        inputs: &Inputs<'_>,
        additional_abis: &[hirsel_lir::FnAbiId],
        discovery: Reference,
        statics_finalizers: Reference,
    ) -> Result<(ValidatedUnit<Value>, Arc<InstanceProduct>), String> {
        if unit.header.definition != *key.instance.definition()
            || unit.header.instance != key.instance.digest()
            || unit.header.target != key.target_abi
            || inputs.target.digest() != key.target_abi
            || discovery.domain != INSTANCE_DISCOVERY_DOMAIN
            || statics_finalizers.domain != STATICS_FINALIZERS_DOMAIN
            || self.store.get_content(discovery).is_none()
            || self.store.get_content(statics_finalizers).is_none()
        {
            return Err("instance publication inputs differ from the demanded instance".into());
        }
        let environment = NativeEnvironment {
            target: inputs.target,
            manifest: inputs.manifest,
            operations: inputs.operations,
            discharge: inputs.discharge,
        };
        let previous = self
            .lookup_inner(key, action, &environment)
            .map(|p| p.product.record);
        self.counts.validations += 1;
        let (checked, output) = self
            .native
            .validate_output(
                subject(self.phase, key, action),
                key.policy,
                unit,
                inputs,
                additional_abis,
            )
            .map_err(|rejected| {
                format!(
                    "native Value refused for {} body {} instance {:?}: {:?}",
                    key.instance.definition(),
                    key.instance.body(),
                    key.instance.digest(),
                    rejected.violations
                )
            })?;
        let output = output.ok_or_else(|| {
            format!(
                "native Value transport failed: {} body {} instance {:?}",
                key.instance.definition(),
                key.instance.body(),
                key.instance.digest()
            )
        })?;
        let equations = extract_local_arena(key, &checked, inputs.layouts)?;
        let bytes = serde_json::to_vec(&equations).map_err(|e| e.to_string())?;
        let equations = self
            .store
            .put_content(EQUATIONS_DOMAIN, &bytes)
            .ok_or("local equation transport failed")?;
        let metadata = serde_json::to_vec(&(equations, discovery, statics_finalizers))
            .map_err(|e| e.to_string())?;
        let record: Record = (
            output.body(),
            output.layouts(),
            equations,
            discovery,
            statics_finalizers,
        );
        let bytes = serde_json::to_vec(&record).map_err(|e| e.to_string())?;
        let expected = INSTANCE_RECORD_DOMAIN
            .reference(&bytes)
            .ok_or("instance descriptor exceeds record limit")?;
        if previous.is_some_and(|previous| previous != expected) {
            return Err(
                "instance recomputation changed output under unchanged complete action".into(),
            );
        }
        self.native
            .publish_output(&output, &metadata)
            .ok_or("native Value publication failed")?;
        let reference = self
            .store
            .put_content(INSTANCE_RECORD_DOMAIN, &bytes)
            .ok_or("instance descriptor transport failed")?;
        if reference != expected {
            return Err("instance descriptor content identity differs".into());
        }
        let address = self.address(key);
        self.store.associate_product(address, action, reference);
        Ok((
            checked,
            Arc::new(InstanceProduct {
                product: ProductRef {
                    address,
                    action,
                    record: reference,
                },
                value: record.0,
                layout_closure: record.1,
                local_equations: record.2,
                discovery: record.3,
                statics_finalizers: record.4,
            }),
        ))
    }
}

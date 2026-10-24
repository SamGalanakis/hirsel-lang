//! Concrete S3 access to completed dependency products. Construction takes no
//! source bodies and runs no import, arena solver or dependency task.
use super::{
    demand::DemandInstances,
    graph::SettledArenaGraph,
    local::{LocalReadyQueries, UnitEmissionContext, read_statics},
    ready::EMISSION_CONTEXT_DOMAIN,
    *,
};
use hirsel_engine::store::ResultStore;
use hirsel_lir::{
    emission::{EmitInputs, OptLevel},
    products::Binding,
    stage::{InputIdentity, Ready, ValidatedUnit},
    validate::{Inputs, NativeEnvironment},
};
use std::collections::{BTreeMap, BTreeSet};

/// Immutable codegen request inputs, explicitly bound by the emission context.
/// Source paths and line-table bindings remain link inputs.
pub struct EmissionOptions {
    pub input: InputIdentity,
    pub opt: OptLevel,
    pub debug: bool,
    pub callbacks: bool,
    pub tracking: bool,
    pub foreign: BTreeMap<hirsel_syntax::DefPath, u64>,
    pub inline_instances: BTreeSet<ContentHash>,
    pub verify_backend: bool,
    pub measurement: bool,
    pub fuel: bool,
}
impl EmissionOptions {
    fn encode(&self) -> Result<Vec<u8>, String> {
        serde_json::to_vec(&(
            1u32,
            self.input.interface_version,
            self.input.digest,
            match self.opt {
                OptLevel::None => 0u32,
                OptLevel::Default => 1,
                OptLevel::Speed => 2,
            },
            self.debug,
            self.callbacks,
            self.tracking,
            self.foreign.iter().collect::<Vec<_>>(),
            &self.inline_instances,
            self.verify_backend,
            self.measurement,
            self.fuel,
        ))
        .map_err(|e| e.to_string())
    }
}

/// Owned immutable dependency inputs shared by task-local product views. The
/// only shared mutable state is C's own acceptance map, never a query controller.
pub struct NativeSnapshot {
    pub(super) store: Arc<dyn ResultStore>,
    pub(super) instances: DemandInstances,
    pub(super) arena: SettledArenaGraph,
    pub(super) options: crate::native::Options,
    emission: EmissionOptions,
    context: Reference,
    manifest: hirsel_core_contract::Manifest,
    operations: Arc<dyn hirsel_lir::operation::OperationDeclarations + Send + Sync>,
    pub(super) owners: BTreeMap<ContentHash, InstanceAddress>,
    pub(super) summaries: BTreeMap<ContentHash, Arc<ArenaSummary>>,
    native: Arc<hirsel_lir::validate::NativeCache>,
}
impl NativeSnapshot {
    pub fn new(
        store: Arc<dyn ResultStore>,
        instances: DemandInstances,
        arena: SettledArenaGraph,
        options: crate::native::Options,
        emission: EmissionOptions,
        manifest: hirsel_core_contract::Manifest,
        operations: Arc<dyn hirsel_lir::operation::OperationDeclarations + Send + Sync>,
    ) -> Result<Arc<Self>, String> {
        let bytes = emission.encode()?;
        let context = store
            .put_content(EMISSION_CONTEXT_DOMAIN, &bytes)
            .ok_or("emission request publication failed")?;
        if EMISSION_CONTEXT_DOMAIN.reference(&bytes) != Some(context) {
            return Err("emission request content identity differs".into());
        }
        let mut owners = BTreeMap::new();
        for summary in &arena.chosen {
            for (owner, _) in &summary.candidates {
                if owner.target_abi != options.target.digest() {
                    return Err("native product request target differs".into());
                }
                if owners
                    .insert(owner.address(READY_KIND).0, owner.clone())
                    .is_some()
                {
                    return Err("duplicate Ready owner in completed native graph".into());
                }
            }
        }
        let summaries = arena
            .candidates
            .iter()
            .chain(&arena.chosen)
            .map(|summary| (summary.product.address.0, summary.clone()))
            .collect();
        Ok(Arc::new(Self {
            native: super::ready::ReadyQueries::validator(&store),
            store,
            instances,
            arena,
            options,
            emission,
            context,
            manifest,
            operations,
            owners,
            summaries,
        }))
    }

    pub(super) fn environment(&self) -> NativeEnvironment<'_> {
        NativeEnvironment {
            target: &self.options.target,
            manifest: &self.manifest,
            operations: self.operations.as_ref(),
            discharge: Some(&self.options.discharge),
        }
    }
}

/// Task-local mutable counters and temporary transformations over one owned
/// immutable snapshot. Construction runs no query or dependency work.
pub struct DemandProducts {
    snapshot: Arc<NativeSnapshot>,
    pub local: LocalReadyQueries,
    pub ready_materializations: u64,
}
impl DemandProducts {
    pub fn new(snapshot: Arc<NativeSnapshot>) -> Self {
        Self {
            local: LocalReadyQueries::with_validator(
                snapshot.store.clone(),
                snapshot.native.clone(),
            ),
            snapshot,
            ready_materializations: 0,
        }
    }
}
impl NativeProducts for DemandProducts {
    fn instance(&mut self, key: &InstanceAddress) -> Result<Arc<InstanceProduct>, String> {
        self.snapshot
            .instances
            .admitted(key, &self.snapshot.environment())
            .map(|(instance, _)| instance.product.clone())
    }

    fn arena_summary(&mut self, key: &ComponentKey) -> Result<Arc<ArenaSummary>, String> {
        let address = key.address()?;
        let summary = self
            .snapshot
            .summaries
            .get(&address.0)
            .ok_or("arena dependency has not completed")?;
        let current = self
            .snapshot
            .store
            .lookup_product(address)
            .ok_or("missing arena dependency association")?;
        if current.action != summary.product.action
            || current.reference != summary.product.record
            || self.snapshot.store.get_content(current.reference).is_none()
        {
            return Err("arena dependency is no longer settled".into());
        }
        Ok(summary.clone())
    }

    fn ready(&mut self, key: &InstanceAddress) -> Result<Arc<ReadyProduct>, String> {
        self.local.ready(
            &self.snapshot.instances,
            key,
            &self.snapshot.arena,
            &self.snapshot.options,
            self.snapshot.context,
            self.snapshot.emission.debug,
            &self.snapshot.environment(),
        )
    }

    fn with_ready<R>(
        &mut self,
        products: &[Arc<ReadyProduct>],
        use_units: impl FnOnce(&[&ValidatedUnit<Ready>], &EmitInputs<'_>) -> Result<R, String>,
    ) -> Result<R, String> {
        let snapshot = self.snapshot.clone();
        let environment = snapshot.environment();
        if products.is_empty() {
            return Err("empty Ready materialization".into());
        }
        let mut selected = BTreeSet::new();
        let mut captured = Vec::new();
        let mut contexts = Vec::new();
        for product in products {
            let owner = snapshot
                .owners
                .get(&product.product.address.0)
                .ok_or("Ready materialization owner has not completed")?
                .clone();
            if !selected.insert(product.product.address.0) {
                return Err("duplicate Ready materialization owner".into());
            }
            let current = self.ready(&owner)?;
            if current.product != product.product {
                return Err("Ready materialization output changed".into());
            }
            if !product.optimizer_reads.is_empty() {
                return Err("Ready optimizer dependencies have not been admitted".into());
            }
            let bytes = snapshot
                .store
                .get_content(product.emission_context)
                .ok_or("missing unit emission context")?;
            let context: UnitEmissionContext =
                serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
            let (instance, _) = snapshot.instances.admitted(&owner, &environment)?;
            if context.request != snapshot.context
                || context.statics_finalizers != instance.product.statics_finalizers
            {
                return Err("unit emission context differs from the request".into());
            }
            captured.push(self.local.queries.read(&owner, product, &environment)?);
            contexts.push(context);
        }
        let closures = captured
            .iter()
            .map(|(closure, _)| closure)
            .collect::<Vec<_>>();
        let binding = Binding::new(environment.target, &closures)?;
        let inputs = Inputs {
            memo: Default::default(),
            target: environment.target,
            layouts: &binding.table,
            operations: environment.operations,
            manifest: environment.manifest,
            discharge: environment.discharge,
        };
        let mut units = Vec::new();
        for (_, body) in &captured {
            let raw = binding.materialize::<Ready>(body)?;
            self.ready_materializations += 1;
            let unit = hirsel_lir::validate::validate(raw, &inputs)
                .map_err(|e| format!("emission Ready refused: {:?}", e.violations))?;
            units.push(unit);
        }
        let mut statics = BTreeMap::new();
        for context in contexts {
            for data in read_statics(
                snapshot.store.as_ref(),
                context.statics_finalizers,
                &binding,
            )? {
                let name = hirsel_codec::to_text(&data.symbol);
                if let Some(previous) = statics.insert(name, data.clone())
                    && previous != data
                {
                    return Err("conflicting selected static data".into());
                }
            }
        }
        let statics = statics.into_values().collect::<Vec<_>>();
        let keys = hirsel_lir::LayoutKeys::new(&binding.table);
        let index = hirsel_lir::LayoutIndex::new(&binding.table);
        let emit = EmitInputs {
            index: Some(&index),
            input: &snapshot.emission.input,
            target: environment.target,
            layouts: &binding.table,
            keys: &keys,
            manifest: environment.manifest,
            statics: &statics,
            inline_instances: &snapshot.emission.inline_instances,
            available_externally: &BTreeSet::new(),
            callbacks: snapshot.emission.callbacks,
            tracking: snapshot.emission.tracking,
            foreign: &snapshot.emission.foreign,
            opt: snapshot.emission.opt,
            verify_backend: snapshot.emission.verify_backend,
            measurement: snapshot.emission.measurement,
            fuel: snapshot.emission.fuel,
        };
        let units = units.iter().collect::<Vec<_>>();
        use_units(&units, &emit)
    }
}

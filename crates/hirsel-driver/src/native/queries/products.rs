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

/// A view of an admitted immutable request's completed instance and summary
/// dependencies. A local Ready miss executes only its own transformation.
/// Dependency continuations and admission belong to S5, never this view.
pub struct DemandProducts<'a> {
    store: Arc<dyn ResultStore>,
    instances: &'a DemandInstances,
    arena: &'a SettledArenaGraph,
    options: &'a crate::native::Options,
    emission: &'a EmissionOptions,
    context: Reference,
    environment: NativeEnvironment<'a>,
    owners: BTreeMap<ContentHash, InstanceAddress>,
    summaries: BTreeMap<ContentHash, Arc<ArenaSummary>>,
    pub local: LocalReadyQueries,
    /// Actual selected Ready body materializations, including emission misses.
    pub ready_materializations: u64,
}
impl<'a> DemandProducts<'a> {
    pub fn new(
        store: Arc<dyn ResultStore>,
        instances: &'a DemandInstances,
        arena: &'a SettledArenaGraph,
        options: &'a crate::native::Options,
        emission: &'a EmissionOptions,
        environment: NativeEnvironment<'a>,
    ) -> Result<Self, String> {
        if options.target.digest() != environment.target.digest() {
            return Err("native product request target differs".into());
        }
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
        Ok(Self {
            local: LocalReadyQueries::new(store.clone()),
            store,
            instances,
            arena,
            options,
            emission,
            context,
            environment,
            owners,
            summaries,
            ready_materializations: 0,
        })
    }
}
impl NativeProducts for DemandProducts<'_> {
    fn instance(&mut self, key: &InstanceAddress) -> Result<Arc<InstanceProduct>, String> {
        self.instances
            .admitted(key, &self.environment)
            .map(|(instance, _)| instance.product.clone())
    }

    fn arena_summary(&mut self, key: &ComponentKey) -> Result<Arc<ArenaSummary>, String> {
        let address = key.address()?;
        let summary = self
            .summaries
            .get(&address.0)
            .ok_or("arena dependency has not completed")?;
        let current = self
            .store
            .lookup_product(address)
            .ok_or("missing arena dependency association")?;
        if current.action != summary.product.action
            || current.reference != summary.product.record
            || self.store.get_content(current.reference).is_none()
        {
            return Err("arena dependency is no longer settled".into());
        }
        Ok(summary.clone())
    }

    fn ready(&mut self, key: &InstanceAddress) -> Result<Arc<ReadyProduct>, String> {
        self.local.ready(
            self.instances,
            key,
            self.arena,
            self.options,
            self.context,
            self.emission.debug,
            &self.environment,
        )
    }

    fn with_ready<R>(
        &mut self,
        products: &[Arc<ReadyProduct>],
        use_units: impl FnOnce(&[&ValidatedUnit<Ready>], &EmitInputs<'_>) -> Result<R, String>,
    ) -> Result<R, String> {
        if products.is_empty() {
            return Err("empty Ready materialization".into());
        }
        let mut selected = BTreeSet::new();
        let mut captured = Vec::new();
        let mut contexts = Vec::new();
        for product in products {
            let owner = self
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
            let bytes = self
                .store
                .get_content(product.emission_context)
                .ok_or("missing unit emission context")?;
            let context: UnitEmissionContext =
                serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
            let (instance, _) = self.instances.admitted(&owner, &self.environment)?;
            if context.request != self.context
                || context.statics_finalizers != instance.product.statics_finalizers
            {
                return Err("unit emission context differs from the request".into());
            }
            captured.push(
                self.local
                    .queries
                    .read(&owner, product, &self.environment)?,
            );
            contexts.push(context);
        }
        let closures = captured
            .iter()
            .map(|(closure, _)| closure)
            .collect::<Vec<_>>();
        let binding = Binding::new(self.environment.target, &closures)?;
        let inputs = Inputs {
            memo: Default::default(),
            target: self.environment.target,
            layouts: &binding.table,
            operations: self.environment.operations,
            manifest: self.environment.manifest,
            discharge: self.environment.discharge,
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
            for data in read_statics(self.store.as_ref(), context.statics_finalizers, &binding)? {
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
            input: &self.emission.input,
            target: self.environment.target,
            layouts: &binding.table,
            keys: &keys,
            manifest: self.environment.manifest,
            statics: &statics,
            inline_instances: &self.emission.inline_instances,
            available_externally: &BTreeSet::new(),
            callbacks: self.emission.callbacks,
            tracking: self.emission.tracking,
            foreign: &self.emission.foreign,
            opt: self.emission.opt,
            verify_backend: self.emission.verify_backend,
            measurement: self.emission.measurement,
            fuel: self.emission.fuel,
        };
        let units = units.iter().collect::<Vec<_>>();
        use_units(&units, &emit)
    }
}

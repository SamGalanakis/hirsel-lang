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
use std::sync::atomic::{AtomicU64, Ordering};

/// Immutable codegen request inputs, explicitly bound by the emission context.
/// Source paths and line-table bindings remain link inputs.
#[derive(Clone)]
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
    /// Host request fields only. The snapshot derives callback, recovery and
    /// inline selection from its admitted instance graph before publication.
    pub fn request(
        input: InputIdentity,
        opt: OptLevel,
        debug: bool,
        foreign: BTreeMap<hirsel_syntax::DefPath, u64>,
        verify_backend: bool,
        measurement: bool,
        fuel: bool,
    ) -> Self {
        Self {
            input,
            opt,
            debug,
            callbacks: false,
            tracking: false,
            foreign,
            inline_instances: BTreeSet::new(),
            verify_backend,
            measurement,
            fuel,
        }
    }
    pub(super) fn encode(&self) -> Result<Vec<u8>, String> {
        serde_json::to_vec(&(
            2u32,
            self.input.interface_version,
            // EmitInputs checks the interface and C-bound per-unit dependencies.
            // Program identity belongs to the root/link manifest; the ordinary
            // library donor and its consumer share identical machine fragments.
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
    pub(super) execution: std::sync::OnceLock<Arc<super::execution::ExecutionProduct>>,
    pub(super) store: Arc<dyn ResultStore>,
    pub(super) instances: DemandInstances,
    pub(super) arena: SettledArenaGraph,
    pub(super) options: crate::native::Options,
    pub(super) verify_store: hirsel_engine::store::VerifyStore,
    pub(super) emission: EmissionOptions,
    pub(super) context: Reference,
    pub(super) manifest: hirsel_core_contract::Manifest,
    operations: Arc<dyn hirsel_lir::operation::OperationDeclarations + Send + Sync>,
    pub(super) owners: BTreeMap<ContentHash, InstanceAddress>,
    pub(super) summaries: BTreeMap<ContentHash, Arc<ArenaSummary>>,
    templates:
        BTreeMap<super::optimizer::Function, (InstanceAddress, super::optimizer::TemplateFacts)>,
    native: Arc<hirsel_lir::validate::NativeCache>,
    materializations: AtomicU64,
    pub(super) needs_guard: bool,
    pub(super) guard_admitted_bytes: u64,
    pub(super) guard: std::sync::OnceLock<Arc<super::seal_demand::GuardTasks>>,
    // Compilation dispatch and copied instance/arena metadata remain charged
    // through the last snapshot, product scope or linker view. Direct snapshot
    // clients still own admission of their supplied inputs themselves.
    pub(super) retained_allowances: Vec<Arc<hirsel_engine::runtime::MemoryReservation>>,
}
impl NativeSnapshot {
    pub(super) fn retained_dispatch_bytes(&mut self) -> u64 {
        use super::ownership_compilation::metadata as m;
        let mut w = hirsel_engine::NativeMetadataWeight::default();
        w.add(size_of::<Self>() + 2 * size_of::<usize>());
        m::options(&mut w, &self.options);
        m::emission(&mut w, &self.emission);
        w.add(super::ready::ReadyQueries::fresh_validator_bytes() as usize);
        m::manifest(&mut w, &self.manifest);
        w.packed_map(&mut self.owners, |w, _, key| m::address(w, key));
        w.packed_map(&mut self.summaries, |_, _, _| ());
        w.packed_map(&mut self.templates, |w, (path, _), (key, facts)| {
            w.path(path);
            m::address(w, key);
            w.vec(&facts.references, |w, (path, _)| w.path(path));
        });
        w.flat_vec(&self.retained_allowances);
        w.bytes()
            .saturating_add(self.instances.retained_index_bytes())
            .saturating_add(self.arena.retained_index_bytes())
    }
    pub(super) fn guard_selection(
        &self,
    ) -> Result<Option<Arc<super::seal_demand::Selection>>, String> {
        if !self.needs_guard {
            return Ok(None);
        }
        self.guard
            .get()
            .and_then(|guard| guard.selection())
            .map(Some)
            .ok_or_else(|| "Ready seal-demand dependency has not completed".into())
    }

    pub fn new(
        store: Arc<dyn ResultStore>,
        instances: DemandInstances,
        arena: SettledArenaGraph,
        options: crate::native::Options,
        mut emission: EmissionOptions,
        manifest: hirsel_core_contract::Manifest,
        operations: Arc<dyn hirsel_lir::operation::OperationDeclarations + Send + Sync>,
    ) -> Result<Arc<Self>, String> {
        let operations = instances.operation_declarations(operations);
        let environment = NativeEnvironment {
            target: &options.target,
            manifest: &manifest,
            operations: operations.as_ref(),
            discharge: Some(&options.discharge),
        };
        let facts = super::emission::select(&instances, &environment)?;
        emission.callbacks = !facts.callbacks.is_empty();
        emission.tracking = facts.tracking;
        emission.inline_instances = facts.inline.into_iter().collect();
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
        let templates = instances
            .completed_inputs()?
            .into_iter()
            .map(|(owner, output)| {
                (
                    (owner.instance.definition().clone(), owner.instance.digest()),
                    (owner, output.templates.clone()),
                )
            })
            .collect();
        let needs_guard = instances
            .completed_inputs()?
            .iter()
            .any(|(_, output)| output.seal_demand.has_seal());
        Ok(Arc::new(Self {
            execution: Default::default(),
            native: super::ready::ReadyQueries::validator(&store),
            store,
            instances,
            arena,
            options,
            verify_store: hirsel_engine::store::VerifyStore::Off,
            emission,
            context,
            manifest,
            operations,
            owners,
            summaries,
            templates,
            materializations: AtomicU64::new(0),
            needs_guard,
            guard_admitted_bytes: 0,
            guard: Default::default(),
            retained_allowances: Vec::new(),
        }))
    }

    /// Capture cache verification before factories share this immutable view.
    /// A shared snapshot cannot change the policy of an admitted request.
    pub fn with_store_verification(
        mut self: Arc<Self>,
        policy: hirsel_engine::store::VerifyStore,
    ) -> Result<Arc<Self>, String> {
        Arc::get_mut(&mut self)
            .ok_or("native store verification must be selected before sharing")?
            .verify_store = policy;
        Ok(self)
    }

    /// Backend copying observes only settled instance facts. Every selected
    /// body's actual Ready query is a later explicit Codegen dependency.
    pub(super) fn codegen_owners(
        &self,
        key: &InstanceAddress,
        backend: &str,
    ) -> Result<Vec<InstanceAddress>, String> {
        if self.owners.get(&key.address(READY_KIND).0) != Some(key) {
            return Err("unregistered Codegen owner".into());
        }
        let mut owners = vec![key.clone()];
        if backend != "llvm" || self.emission.opt == OptLevel::None || self.emission.debug {
            return Ok(owners);
        }
        let function = (key.instance.definition().clone(), key.instance.digest());
        let mut copies = super::optimizer::copy_closure(
            &function,
            |member| {
                self.templates
                    .get(member)
                    .map_or_else(Vec::new, |(_, facts)| facts.references.clone())
            },
            |member| {
                self.templates.get(member).is_some_and(|(_, facts)| {
                    facts.library
                        || facts.small
                        || self.emission.inline_instances.contains(&member.1)
                })
            },
        )
        .into_iter()
        .map(|member| self.templates[&member].0.clone())
        .collect::<Vec<_>>();
        copies.sort_by_cached_key(|member| {
            hirsel_lir::emission::symbol_name(
                member.instance.definition(),
                Some(&member.instance.digest()),
            )
        });
        owners.extend(copies);
        Ok(owners)
    }

    pub(super) fn environment(&self) -> NativeEnvironment<'_> {
        NativeEnvironment {
            target: &self.options.target,
            manifest: &self.manifest,
            operations: self.operations.as_ref(),
            discharge: Some(&self.options.discharge),
        }
    }
    pub fn emission(&self) -> &EmissionOptions {
        &self.emission
    }
    pub fn ready_materializations(&self) -> u64 {
        self.materializations.load(Ordering::Relaxed)
    }
}

/// Task-local mutable counters and temporary transformations over one owned
/// immutable snapshot. Construction runs no query or dependency work.
pub struct DemandProducts {
    pub(super) snapshot: Arc<NativeSnapshot>,
    pub local: LocalReadyQueries,
    pub ready_materializations: u64,
    pub(super) selected: BTreeMap<ContentHash, ProductRef>,
}
impl DemandProducts {
    pub fn new(snapshot: Arc<NativeSnapshot>) -> Self {
        Self {
            local: LocalReadyQueries::with_validator(
                snapshot.store.clone(),
                snapshot.native.clone(),
            )
            .with_verification(snapshot.verify_store),
            snapshot,
            ready_materializations: 0,
            selected: BTreeMap::new(),
        }
    }

    /// Explicit diagnostic consumer. A cached ordinary Ready body cannot supply
    /// the initial Value observations or its checked pass transport chain.
    pub fn observe(
        &mut self,
        key: &InstanceAddress,
    ) -> Result<super::ready::ObservedReady, String> {
        self.local.guard = self.snapshot.guard_selection()?;
        self.local.observe(
            &self.snapshot.instances,
            key,
            &self.snapshot.arena,
            &self.snapshot.options,
            self.snapshot.context,
            self.snapshot.emission.debug,
            &self.snapshot.environment(),
        )
    }
}

/// Read-only object-task access restricted to the Ready products actually
/// returned by its admitted dependency reads. This view never computes Ready.
pub(super) struct SettledProducts {
    inner: DemandProducts,
    ready: BTreeMap<ContentHash, Arc<ReadyProduct>>,
}
impl SettledProducts {
    pub(super) fn new(snapshot: Arc<NativeSnapshot>, reads: &[ProductRef]) -> Result<Self, String> {
        let mut inner = DemandProducts::new(snapshot.clone());
        let mut ready = BTreeMap::new();
        for read in reads {
            let owner = snapshot
                .owners
                .get(&read.address.0)
                .ok_or("unregistered settled Ready dependency")?;
            let product = inner
                .local
                .queries
                .lookup(
                    owner,
                    read.action,
                    &NativeEnvironment {
                        operations: &snapshot
                            .instances
                            .operations(owner, &snapshot.environment())?,
                        ..snapshot.environment()
                    },
                )
                .ok_or("settled Ready dependency lost current C admission")?;
            if product.product != *read || ready.insert(read.address.0, product).is_some() {
                return Err("duplicate or changed settled Ready dependency".into());
            }
            inner.selected.insert(read.address.0, *read);
        }
        Ok(Self { inner, ready })
    }
}
impl NativeProducts for SettledProducts {
    fn execution_read(&self) -> Option<ProductRef> {
        self.inner.execution_read()
    }
    fn instance(&mut self, _: &InstanceAddress) -> Result<Arc<InstanceProduct>, String> {
        Err("instance is not an object task's direct dependency".into())
    }
    fn arena_summary(&mut self, _: &ComponentKey) -> Result<Arc<ArenaSummary>, String> {
        Err("arena summary is not an object task's direct dependency".into())
    }
    fn ready(&mut self, key: &InstanceAddress) -> Result<Arc<ReadyProduct>, String> {
        let product = self
            .ready
            .get(&key.address(READY_KIND).0)
            .ok_or("unread Ready dependency")?;
        let current = self
            .inner
            .local
            .queries
            .lookup(
                key,
                product.product.action,
                &NativeEnvironment {
                    operations: &self
                        .inner
                        .snapshot
                        .instances
                        .operations(key, &self.inner.snapshot.environment())?,
                    ..self.inner.snapshot.environment()
                },
            )
            .ok_or("object task lost Ready C admission")?;
        if current.product != product.product {
            return Err("object task Ready dependency changed".into());
        }
        Ok(current)
    }
    fn codegen_units(
        &mut self,
        key: &InstanceAddress,
        backend: &str,
    ) -> Result<Vec<Arc<ReadyProduct>>, String> {
        self.inner
            .snapshot
            .codegen_owners(key, backend)?
            .iter()
            .map(|owner| self.ready(owner))
            .collect()
    }
    fn with_ready<R>(
        &mut self,
        products: &[Arc<ReadyProduct>],
        use_units: impl FnOnce(&[&ValidatedUnit<Ready>], &EmitInputs<'_>) -> Result<R, String>,
    ) -> Result<R, String> {
        self.inner.with_ready(products, use_units)
    }
}
impl NativeProducts for DemandProducts {
    fn execution_read(&self) -> Option<ProductRef> {
        self.snapshot.execution.get().map(|p| p.product)
    }
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
        self.local.guard = self.snapshot.guard_selection()?;
        let product = self.local.ready(
            &self.snapshot.instances,
            key,
            &self.snapshot.arena,
            &self.snapshot.options,
            self.snapshot.context,
            self.snapshot.emission.debug,
            &self.snapshot.environment(),
        )?;
        self.selected
            .insert(product.product.address.0, product.product);
        Ok(product)
    }

    fn codegen_units(
        &mut self,
        key: &InstanceAddress,
        backend: &str,
    ) -> Result<Vec<Arc<ReadyProduct>>, String> {
        self.snapshot
            .codegen_owners(key, backend)?
            .iter()
            .map(|owner| self.ready(owner))
            .collect()
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
        let mut adapters = Vec::new();
        for product in products {
            let owner = snapshot
                .owners
                .get(&product.product.address.0)
                .ok_or("Ready materialization owner has not completed")?
                .clone();
            if !selected.insert(product.product.address.0) {
                return Err("duplicate Ready materialization owner".into());
            }
            if self.selected.get(&product.product.address.0) != Some(&product.product) {
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
            let operations = snapshot.instances.operations(&owner, &environment)?;
            // Numeric bindings contain only these selected products. Their
            // independently planned inventories also define the emission scope;
            // unrelated adapters in the snapshot need no ABI in this table.
            for adapter in
                hirsel_lir::operation::OperationDeclarations::pretransfer_adapters(&operations)
            {
                if !adapters.contains(adapter) {
                    adapters.push(adapter.clone());
                }
            }
            let scoped = NativeEnvironment {
                operations: &operations,
                ..environment
            };
            captured.push(self.local.queries.read(&owner, product, &scoped)?);
            contexts.push(context);
        }
        let closures = captured
            .iter()
            .map(|(closure, _)| closure)
            .collect::<Vec<_>>();
        let binding = if let Some(execution) = snapshot.execution.get() {
            execution.binding.clone()
        } else {
            Binding::new(environment.target, &closures)?
        };
        let selected_operations = super::instance_operations::InstanceOperations {
            inventory: None,
            receipts: None,
            declarations: environment.operations,
            source: None,
            all_sources: true,
            pretransfer: Some(adapters),
        };
        let operations = super::instance_operations::InstanceOperations {
            inventory: None,
            receipts: None,
            declarations: environment.operations,
            source: None,
            all_sources: true,
            pretransfer: Some(super::instance_operations::bind_pretransfer(
                &selected_operations,
                &binding.table,
                environment.target,
            )?),
        };
        let inputs = Inputs {
            memo: Default::default(),
            target: environment.target,
            layouts: &binding.table,
            operations: &operations,
            manifest: environment.manifest,
            discharge: environment.discharge,
        };
        let mut units = Vec::new();
        for (_, body) in &captured {
            let raw = binding.materialize::<Ready>(body)?;
            self.ready_materializations += 1;
            snapshot.materializations.fetch_add(1, Ordering::Relaxed);
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
            ahead: snapshot.execution.get().map(|e| &e.ahead),
            continuation: snapshot.execution.get().map(|e| &e.continuation),
            alternate: false,
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

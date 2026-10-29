//! Opt-in diagnostic roots over the same settled instance/arena dependencies
//! as ordinary Ready. No ordinary request installs or polls these producers.
use super::{products::DemandProducts, ready::ObservedReady, tasks::NativeTasks, *};
use hirsel_engine::{
    runtime::*,
    store::{Action, Address, Domain},
};
use hirsel_lir::{
    ir::LirUnit,
    ownership_trace::{Plan, Recipe},
    stage::Ready,
};
use std::{
    collections::BTreeMap,
    sync::{
        OnceLock,
        atomic::{AtomicU64, Ordering},
    },
    task::Poll,
};

pub const OWNERSHIP_READY_KIND: u32 = 0x4e00_002b;
pub const OWNERSHIP_ROOT_KIND: u32 = 0x4e00_002c;
const UNIT: Domain = Domain {
    kind: OWNERSHIP_READY_KIND,
    schema: 1,
};
const ROOT: Domain = Domain {
    kind: OWNERSHIP_ROOT_KIND,
    schema: 1,
};
struct Payload {
    body: LirUnit<Ready>,
    layouts: hirsel_lir::LayoutTable,
    initial: Plan,
    plan: Plan,
    recipes: Vec<Recipe>,
}
hirsel_codec::snapshot_struct!(Payload {
    body,
    layouts,
    initial,
    plan,
    recipes
});
struct Request {
    owner: ContentHash,
    query: QueryId,
    completed: OnceLock<ProductRef>,
}
impl Request {
    fn owner<'a>(&self, native: &'a NativeTasks) -> &'a InstanceAddress {
        &native.snapshot.owners[&self.owner]
    }
}
/// Compact request/completion records only. Diagnostic bodies and side tables
/// live in the selected result store; C authority is never serialized.
pub struct OwnershipTasks {
    native: Arc<NativeTasks>,
    requests: BTreeMap<ContentHash, Request>,
    root: QueryId,
    weight: MemoryWeight,
    completed: OnceLock<ProductRef>,
    _allowance: MemoryReservation,
    pub transformations: AtomicU64,
}
impl OwnershipTasks {
    pub fn new(
        native: Arc<NativeTasks>,
        weight: MemoryWeight,
        allowance: MemoryReservation,
    ) -> Result<Arc<Self>, String> {
        let mut requests = BTreeMap::new();
        for (key, owner) in &native.snapshot.owners {
            let query = QueryId {
                kind: OWNERSHIP_READY_KIND,
                address: owner.address(OWNERSHIP_READY_KIND),
            };
            let reads = native.ownership_reads(owner)?;
            if (std::mem::size_of::<Task>()
                + reads.len() * std::mem::size_of::<(QueryId, Reference)>()) as u64
                > weight.retained
            {
                return Err("ownership diagnostic continuation exceeds original admission".into());
            }
            requests.insert(
                query.address.0,
                Request {
                    owner: *key,
                    query,
                    completed: OnceLock::new(),
                },
            );
        }
        if requests.is_empty() {
            return Err("empty ownership diagnostic closure".into());
        }
        let mut copied = hirsel_engine::NativeMetadataWeight::default();
        copied.add(std::mem::size_of::<Self>() + 3 * std::mem::size_of::<usize>());
        // Owner encodings and dependency rows remain in the already admitted
        // native snapshot/dispatch. This graph retains their sole Arc owner,
        // and stores only checked lookup keys and its own completion cells.
        copied.packed_map(&mut requests, |_, _, _| {});
        if copied.bytes() > allowance.bytes()
            || (std::mem::size_of::<Task>() + requests.len() * std::mem::size_of::<QueryId>())
                as u64
                > weight.retained
        {
            return Err(format!(
                "ownership diagnostic metadata exceeds original admission: {} bytes, {} available",
                copied.bytes(),
                allowance.bytes()
            ));
        }
        let selection = requests.keys().copied().collect::<Vec<_>>();
        let root = QueryId {
            kind: OWNERSHIP_ROOT_KIND,
            address: Address(hirsel_codec::digest(
                "hirsel.native.ownership-root.v1",
                "Units",
                &selection,
            )),
        };
        Ok(Arc::new(Self {
            native,
            requests,
            root,
            weight,
            completed: OnceLock::new(),
            _allowance: allowance,
            transformations: AtomicU64::new(0),
        }))
    }
    pub fn query_id(&self) -> QueryId {
        self.root
    }
    pub fn factory(self: &Arc<Self>) -> Arc<dyn QueryFactory> {
        Arc::new(Factory(self.clone()))
    }
    pub fn unit_queries(&self) -> Vec<QueryId> {
        self.requests
            .values()
            .map(|request| request.query)
            .collect()
    }
    fn publish(
        &self,
        query: QueryId,
        domain: Domain,
        bytes: &[u8],
        selected: &OnceLock<ProductRef>,
    ) -> Result<ProductRef, String> {
        let store = &self.native.snapshot.store;
        let record = store
            .put_content(domain, bytes)
            .ok_or("ownership diagnostic transport exhausted")?;
        if domain.reference(bytes) != Some(record) {
            return Err("ownership diagnostic content differs".into());
        }
        let product = ProductRef {
            address: query.address,
            action: Action(ContentHash::of_parts(
                "hirsel.native.ownership-action.v1",
                [query.address.0.0.as_slice(), record.output.0.0.as_slice()],
            )),
            record,
        };
        if let Some(previous) = selected.get() {
            if *previous != product {
                return Err("ownership diagnostic completion changed".into());
            }
        } else {
            selected
                .set(product)
                .map_err(|_| "ownership diagnostic published twice")?;
        }
        store.associate_product(product.address, product.action, product.record);
        Ok(product)
    }
    /// Only the exact output of this producer's selected root is admitted.
    /// Content references alone never authorize a missing diagnostic result.
    pub fn outputs(&self, root: ProductRef) -> Result<Vec<ProductRef>, String> {
        if self.completed.get() != Some(&root) {
            return Err("ownership root has not completed this selection".into());
        }
        let outputs = self
            .requests
            .values()
            .map(|request| {
                request
                    .completed
                    .get()
                    .copied()
                    .ok_or("ownership unit has not completed".to_owned())
            })
            .collect::<Result<Vec<_>, _>>()?;
        let expected = serde_json::to_vec(
            &outputs
                .iter()
                .map(|p| (p.address.0, p.action.0, p.record))
                .collect::<Vec<_>>(),
        )
        .map_err(|e| e.to_string())?;
        let bytes = self
            .native
            .snapshot
            .store
            .get_content(root.record)
            .ok_or("missing ownership root")?;
        if bytes.as_ref() != expected || ROOT.reference(&bytes) != Some(root.record) {
            return Err("ownership root descriptor differs".into());
        }
        Ok(outputs)
    }
    /// Bind the complete selected diagnostic closure for emission. Every local
    /// product and the shared table re-enter C, and canonical body/ABI/layout
    /// equality transports the original plan through numeric rebinding.
    pub fn with_ready<R>(
        &self,
        root: ProductRef,
        use_units: impl FnOnce(
            &[&hirsel_lir::stage::ValidatedUnit<Ready>],
            &hirsel_lir::emission::EmitInputs<'_>,
            &BTreeMap<ContentHash, Plan>,
        ) -> Result<R, String>,
    ) -> Result<R, String> {
        self.with_bound_ready(root, |units, emit, plans| {
            hirsel_lir::ownership_trace::native::Request::Resources(plans)
                .prepare(units, emit.layouts)?;
            use_units(units, emit, plans)
        })
    }

    fn with_bound_ready<R>(
        &self,
        root: ProductRef,
        use_units: impl FnOnce(
            &[&hirsel_lir::stage::ValidatedUnit<Ready>],
            &hirsel_lir::emission::EmitInputs<'_>,
            &BTreeMap<ContentHash, Plan>,
        ) -> Result<R, String>,
    ) -> Result<R, String> {
        use hirsel_lir::{
            products::{Binding, LayoutClosure},
            validate::Inputs,
        };
        let outputs = self.outputs(root)?;
        let local = outputs
            .iter()
            .map(|product| self.materialize(*product))
            .collect::<Result<Vec<_>, _>>()?;
        let snapshot = &self.native.snapshot;
        let environment = snapshot.environment();
        let mut captures = Vec::new();
        for (product, observed) in outputs.iter().zip(&local) {
            let request = &self.requests[&product.address.0];
            let declarations = snapshot
                .instances
                .operations(request.owner(&self.native), &environment)?;
            let (owner, _) = snapshot
                .instances
                .admitted(request.owner(&self.native), &environment)?;
            let operations = super::instance_operations::InstanceOperations {
                inventory: None,
                receipts: None,
                declarations: &declarations,
                all_sources: false,
                source: (!owner.facts.body.templates.library).then_some((
                    request.owner(&self.native).instance.definition(),
                    request.owner(&self.native).instance.digest(),
                )),
                pretransfer: Some(super::instance_operations::bind_pretransfer(
                    &declarations,
                    &observed.layouts,
                    environment.target,
                )?),
            };
            let inputs = Inputs {
                memo: Default::default(),
                target: environment.target,
                layouts: &observed.layouts,
                operations: &operations,
                manifest: environment.manifest,
                discharge: environment.discharge,
            };
            captures.push(LayoutClosure::capture(&observed.unit, &inputs, &[])?);
        }
        let closures = captures
            .iter()
            .map(|(closure, _)| closure)
            .collect::<Vec<_>>();
        let binding = Binding::new(environment.target, &closures)?;
        let operations = super::instance_operations::InstanceOperations {
            inventory: None,
            receipts: None,
            declarations: environment.operations,
            all_sources: true,
            source: None,
            pretransfer: Some(super::instance_operations::bind_pretransfer(
                environment.operations,
                &binding.table,
                environment.target,
            )?),
        };
        let bound_inputs = Inputs {
            memo: Default::default(),
            target: environment.target,
            layouts: &binding.table,
            operations: &operations,
            manifest: environment.manifest,
            discharge: environment.discharge,
        };
        let mut units = Vec::new();
        let mut plans = BTreeMap::new();
        let mut statics = BTreeMap::new();
        for ((product, observed), (_, body)) in outputs.iter().zip(&local).zip(&captures) {
            let unit =
                hirsel_lir::validate::validate_ready(binding.materialize(body)?, &bound_inputs)
                    .map_err(|e| format!("bound ownership Ready C: {:?}", e.violations))?;
            let request = &self.requests[&product.address.0];
            let declarations = snapshot
                .instances
                .operations(request.owner(&self.native), &environment)?;
            let (owner, _) = snapshot
                .instances
                .admitted(request.owner(&self.native), &environment)?;
            let operations = super::instance_operations::InstanceOperations {
                inventory: None,
                receipts: None,
                declarations: &declarations,
                all_sources: false,
                source: (!owner.facts.body.templates.library).then_some((
                    request.owner(&self.native).instance.definition(),
                    request.owner(&self.native).instance.digest(),
                )),
                pretransfer: Some(super::instance_operations::bind_pretransfer(
                    &declarations,
                    &observed.layouts,
                    environment.target,
                )?),
            };
            let inputs = Inputs {
                memo: Default::default(),
                target: environment.target,
                layouts: &observed.layouts,
                operations: &operations,
                manifest: environment.manifest,
                discharge: environment.discharge,
            };
            let plan = hirsel_lir::ownership_trace::native::rebind_plan(
                &observed.unit,
                &inputs,
                &unit,
                &bound_inputs,
                &observed.plan,
            )?;
            if plans.insert(unit.unit().digest(), plan).is_some() {
                return Err("duplicate ownership emission unit".into());
            }
            for data in super::local::read_statics(
                snapshot.store.as_ref(),
                owner.product.statics_finalizers,
                &binding,
            )? {
                let name = hirsel_codec::to_text(&data.symbol);
                if statics
                    .insert(name, data.clone())
                    .is_some_and(|previous| previous != data)
                {
                    return Err("conflicting ownership emission static data".into());
                }
            }
            units.push(unit);
        }
        let statics = statics.into_values().collect::<Vec<_>>();
        let keys = hirsel_lir::LayoutKeys::new(&binding.table);
        let index = hirsel_lir::LayoutIndex::new(&binding.table);
        let emit = hirsel_lir::emission::EmitInputs {
            ahead: None,
            continuation: None,
            alternate: false,
            index: Some(&index),
            input: &snapshot.emission.input,
            target: environment.target,
            layouts: &binding.table,
            keys: &keys,
            manifest: environment.manifest,
            statics: &statics,
            inline_instances: &snapshot.emission.inline_instances,
            available_externally: &Default::default(),
            callbacks: snapshot.emission.callbacks,
            tracking: snapshot.emission.tracking,
            foreign: &snapshot.emission.foreign,
            opt: snapshot.emission.opt,
            verify_backend: snapshot.emission.verify_backend,
            measurement: snapshot.emission.measurement,
            fuel: snapshot.emission.fuel,
        };
        let units = units.iter().collect::<Vec<_>>();
        use_units(&units, &emit, &plans)
    }

    /// Use the existing source-bound linker projection over the complete
    /// diagnostic C scope. No ordinary Ready result or source compilation is
    /// consulted, and original observation plans remain beside the objects.
    pub fn with_link_context<R>(
        &self,
        root: ProductRef,
        source: &crate::native::root_source::NativeLinkSource,
        use_context: impl FnOnce(
            &crate::native::ReadyProgram,
            &[&hirsel_lir::stage::ValidatedUnit<Ready>],
            &hirsel_lir::emission::EmitInputs<'_>,
            &BTreeMap<ContentHash, Plan>,
        ) -> Result<R, String>,
    ) -> Result<R, String> {
        self.with_bound_link_context(root, source, |program, units, emit, plans| {
            hirsel_lir::ownership_trace::native::Request::Resources(plans)
                .prepare(units, emit.layouts)?;
            use_context(program, units, emit, plans)
        })
    }

    fn with_bound_link_context<R>(
        &self,
        root: ProductRef,
        source: &crate::native::root_source::NativeLinkSource,
        use_context: impl FnOnce(
            &crate::native::ReadyProgram,
            &[&hirsel_lir::stage::ValidatedUnit<Ready>],
            &hirsel_lir::emission::EmitInputs<'_>,
            &BTreeMap<ContentHash, Plan>,
        ) -> Result<R, String>,
    ) -> Result<R, String> {
        use hirsel_lir::{fragment::UnitSummary, ir::UnitOrigin};
        let snapshot = &self.native.snapshot;
        if source.input() != &snapshot.emission.input
            || source.foreign_dispatch() != snapshot.emission.foreign
        {
            return Err("ownership linker source differs from selected emission input".into());
        }
        self.with_bound_ready(root, |units, emit, plans| {
            let raw = units
                .iter()
                .map(|unit| unit.unit().clone())
                .collect::<Vec<_>>();
            hirsel_lir::validate::check_result_arena_calls(&raw, emit.layouts)?;
            hirsel_lir::validate::check_scoped_links(&raw, emit.layouts)?;
            let mut summaries = Vec::new();
            let mut semantic_owners = BTreeMap::new();
            let mut functions = BTreeMap::new();
            for unit in units {
                let mut summary =
                    UnitSummary::of(unit.unit(), emit.layouts, emit.keys, emit.manifest)?;
                summary.add_static_references(emit.statics);
                let owner = match summary.origin {
                    UnitOrigin::Source => Some(summary.definition.clone()),
                    UnitOrigin::Library => None,
                };
                if semantic_owners
                    .insert(summary.definition.clone(), owner.clone())
                    .is_some_and(|previous| previous != owner)
                {
                    return Err("mixed ownership semantic owner origins".into());
                }
                for request in self.requests.values().filter(|request| {
                    request.owner(&self.native).instance.digest() == summary.instance
                        && request.owner(&self.native).instance.definition() == &summary.definition
                }) {
                    if source
                        .functions()
                        .get(request.owner(&self.native).instance.definition())
                        == Some(&request.owner(&self.native).instance.body())
                        && functions
                            .insert(summary.definition.clone(), summary.instance)
                            .is_some()
                    {
                        return Err("duplicate ownership linker entry".into());
                    }
                }
                summaries.push(summary);
            }
            let environment = snapshot.environment();
            let mut finalizers = BTreeMap::new();
            for request in self.requests.values() {
                let (owner, _) = snapshot
                    .instances
                    .admitted(request.owner(&self.native), &environment)?;
                let reference = owner.product.statics_finalizers;
                if reference.domain != super::instance::STATICS_FINALIZERS_DOMAIN {
                    return Err("wrong ownership finalizer domain".into());
                }
                let bytes = snapshot
                    .store
                    .get_content(reference)
                    .ok_or("missing ownership finalizers")?;
                let (_, rows): (super::local::StaticRows, super::local::FinalizerRows) =
                    serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
                if rows.windows(2).any(|pair| pair[0] >= pair[1]) {
                    return Err("noncanonical ownership finalizers".into());
                }
                for (layout, definition, instance, abi, library) in rows {
                    let summary = summaries
                        .iter()
                        .find(|summary| {
                            summary.definition == definition && summary.instance == instance
                        })
                        .ok_or("ownership finalizer has no C-admitted body")?;
                    if emit.keys.fn_abi_of(emit.layouts, summary.abi) != Some(abi)
                        || (summary.origin == UnitOrigin::Library) != library
                    {
                        return Err("ownership finalizer ABI or origin differs".into());
                    }
                    let layout = emit
                        .keys
                        .all()
                        .iter()
                        .position(|key| key.0 == layout)
                        .map(|index| hirsel_lir::LayoutId(index as u32))
                        .ok_or("missing ownership finalizer layout")?;
                    if finalizers
                        .insert(layout, instance)
                        .is_some_and(|previous| previous != instance)
                    {
                        return Err("conflicting ownership finalizer instances".into());
                    }
                }
            }
            let table = emit.layouts.clone();
            let keys = hirsel_lir::LayoutKeys::new(&table);
            super::roots::with_link_assembly(
                snapshot,
                source,
                super::roots::LinkAssembly {
                    operations: source.operations().clone(),
                    semantic_owners,
                    summaries,
                    functions,
                    finalizers,
                    table,
                    keys,
                    statics: emit.statics.to_vec(),
                },
                |program| use_context(program, units, emit, plans),
            )
        })
    }

    /// Project executable bodies from the complete C-admitted source scope.
    /// Certification-only roots remain checked but cannot introduce unbound
    /// callable parameters into the executable call equations.
    pub fn with_execution_context<R>(
        &self,
        root: ProductRef,
        source: &crate::native::root_source::NativeLinkSource,
        entry: &hirsel_syntax::DefPath,
        use_context: impl FnOnce(
            &crate::native::ReadyProgram,
            &[&hirsel_lir::stage::ValidatedUnit<Ready>],
            &hirsel_lir::emission::EmitInputs<'_>,
            &BTreeMap<ContentHash, Plan>,
        ) -> Result<R, String>,
    ) -> Result<R, String> {
        self.with_bound_link_context(root, source, |program, units, emit, plans| {
            let selected = super::ownership_closure::select(program, units, entry)?;
            let mut executable = program.clone();
            executable
                .summaries
                .retain(|summary| selected.contains(&summary.instance));
            executable.callbacks = executable
                .summaries
                .iter()
                .flat_map(|summary| summary.glue.callbacks.iter().map(|(path, _)| path.clone()))
                .collect();
            executable.entries = crate::native::stages::summary_entries(&executable.summaries);
            let units = units
                .iter()
                .copied()
                .filter(|unit| selected.contains(&unit.unit().header.instance))
                .collect::<Vec<_>>();
            let plans = units
                .iter()
                .map(|unit| {
                    let digest = unit.unit().digest();
                    plans
                        .get(&digest)
                        .cloned()
                        .map(|plan| (digest, plan))
                        .ok_or_else(|| "missing executable ownership plan".to_owned())
                })
                .collect::<Result<BTreeMap<_, _>, _>>()?;
            hirsel_lir::ownership_trace::native::Request::Resources(&plans)
                .prepare(&units, emit.layouts)?;
            use_context(&executable, &units, emit, &plans)
        })
    }

    /// Reopen one selected diagnostic output and independently revalidate C.
    /// This performs no source, import or lowering work and grants recipes no
    /// semantic authority. A caller cannot substitute an ordinary Ready hit.
    pub fn materialize(&self, product: ProductRef) -> Result<ObservedReady, String> {
        let request = self
            .requests
            .get(&product.address.0)
            .ok_or("unselected ownership unit")?;
        if request.completed.get() != Some(&product) {
            return Err("ownership unit completion differs".into());
        }
        let snapshot = &self.native.snapshot;
        let environment = snapshot.environment();
        snapshot
            .instances
            .admitted(request.owner(&self.native), &environment)?;
        let bytes = snapshot
            .store
            .get_content(product.record)
            .ok_or("missing ownership diagnostic unit")?;
        if UNIT.reference(&bytes) != Some(product.record) {
            return Err("ownership diagnostic unit content differs".into());
        }
        let (
            Payload {
                body,
                layouts,
                initial,
                plan,
                recipes,
            },
            source_action,
        ): (Payload, ContentHash) =
            hirsel_codec::from_text(std::str::from_utf8(&bytes).map_err(|e| e.to_string())?)
                .map_err(|e| format!("ownership diagnostic decode: {e:?}"))?;
        let source_action = Action(source_action);
        let declarations = snapshot
            .instances
            .operations(request.owner(&self.native), &environment)?;
        let (owner, _) = snapshot
            .instances
            .admitted(request.owner(&self.native), &environment)?;
        let operations = super::instance_operations::InstanceOperations {
            inventory: None,
            receipts: None,
            declarations: &declarations,
            all_sources: false,
            source: (!owner.facts.body.templates.library).then_some((
                request.owner(&self.native).instance.definition(),
                request.owner(&self.native).instance.digest(),
            )),
            pretransfer: Some(super::instance_operations::bind_pretransfer(
                &declarations,
                &layouts,
                environment.target,
            )?),
        };
        let inputs = hirsel_lir::validate::Inputs {
            memo: Default::default(),
            target: environment.target,
            layouts: &layouts,
            operations: &operations,
            manifest: environment.manifest,
            discharge: environment.discharge,
        };
        let unit = hirsel_lir::validate::validate_ready(body, &inputs)
            .map_err(|e| format!("ownership diagnostic C: {:?}", e.violations))?;
        hirsel_lir::ownership_trace::native::LeafPlan::from_relocated(&unit, &layouts, &plan)?;
        Ok(ObservedReady {
            unit,
            layouts,
            initial,
            plan,
            recipes,
            source_action,
        })
    }
}
pub(super) struct Factory(Arc<OwnershipTasks>);
impl QueryFactory for Factory {
    fn create(&self, query: QueryId) -> Result<Box<dyn QueryTask>, Reference> {
        if query != self.0.root
            && !self
                .0
                .requests
                .get(&query.address.0)
                .is_some_and(|request| request.query == query)
        {
            return Err(self
                .0
                .native
                .failure("unregistered ownership diagnostic query"));
        }
        Ok(Box::new(Task {
            owner: self.0.clone(),
            query,
            terminal: None,
        }))
    }
}
struct Task {
    owner: Arc<OwnershipTasks>,
    query: QueryId,
    terminal: Option<Result<ProductRef, Reference>>,
}
impl QueryTask for Task {
    fn id(&self) -> QueryId {
        self.query
    }
    fn weight(&self) -> MemoryWeight {
        self.owner.weight
    }
    fn poll(&mut self, cx: &mut dyn QueryContext) -> QueryPoll {
        if self.terminal.is_none() {
            match self.advance(cx) {
                Ok(Err(waiting)) => return QueryPoll::Waiting(waiting),
                Ok(Ok(product)) => self.terminal = Some(Ok(product)),
                Err(error) => self.terminal = Some(Err(error)),
            }
        }
        match self.terminal.unwrap() {
            Ok(product) => QueryPoll::Complete(product),
            Err(error) => QueryPoll::Failed(error),
        }
    }
}
impl Task {
    fn advance(
        &self,
        cx: &mut dyn QueryContext,
    ) -> Result<Result<ProductRef, Vec<QueryId>>, Reference> {
        let owner = &self.owner;
        let fail = |error: String| owner.native.failure(&error);
        let root = self.query == owner.root;
        let request = owner.requests.get(&self.query.address.0);
        let reads = if root {
            owner
                .unit_queries()
                .into_iter()
                .map(|query| (query, None))
                .collect::<Vec<_>>()
        } else {
            owner
                .native
                .ownership_reads(request.unwrap().owner(&owner.native))
                .map_err(fail)?
                .iter()
                .map(|(query, record)| (*query, Some(*record)))
                .collect()
        };
        let mut waiting = Vec::new();
        let mut outputs = Vec::new();
        for (query, expected) in reads {
            match cx.read(query) {
                Poll::Pending => waiting.push(query),
                Poll::Ready(Err(error)) => return Err(error),
                Poll::Ready(Ok(product))
                    if product.address == query.address
                        && expected.is_none_or(|record| record == product.record) =>
                {
                    outputs.push(product)
                }
                _ => {
                    return Err(fail(
                        "ownership dependency changed within immutable snapshot".into(),
                    ));
                }
            }
        }
        if !waiting.is_empty() {
            return Ok(Err(waiting));
        }
        if root {
            let bytes = serde_json::to_vec(
                &outputs
                    .iter()
                    .map(|p| (p.address.0, p.action.0, p.record))
                    .collect::<Vec<_>>(),
            )
            .map_err(|e| fail(e.to_string()))?;
            owner
                .publish(self.query, ROOT, &bytes, &owner.completed)
                .map(Ok)
                .map_err(fail)
        } else {
            if let Some(guard) = owner.native.snapshot.guard.get() {
                match cx.read(guard.query_id()) {
                    Poll::Pending => return Ok(Err(vec![guard.query_id()])),
                    Poll::Ready(Err(error)) => return Err(error),
                    Poll::Ready(Ok(product)) if guard.accepts(product) => (),
                    _ => return Err(fail("ownership seal-demand dependency changed".into())),
                }
            }
            let request = request.unwrap();
            let mut products = DemandProducts::new(owner.native.snapshot.clone());
            let observed = products
                .observe(request.owner(&owner.native))
                .map_err(fail)?;
            owner.transformations.fetch_add(
                products.local.queries.counts.transformations,
                Ordering::Relaxed,
            );
            let payload = Payload {
                body: observed.unit.unit().clone(),
                layouts: observed.layouts,
                initial: observed.initial,
                plan: observed.plan,
                recipes: observed.recipes,
            };
            let bytes = hirsel_codec::to_text(&(payload, observed.source_action.0));
            owner
                .publish(self.query, UNIT, bytes.as_bytes(), &request.completed)
                .map(Ok)
                .map_err(fail)
        }
    }
}

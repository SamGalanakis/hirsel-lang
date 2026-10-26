//! Demand import over source and library recipes. Descriptor admission precedes
//! typed recipe/body materialization; native C owns every reusable Value.
use super::{
    declarations::NativeDeclarations, instance::*, ready_source::ReadySourceFacts, recipes::*,
    source_inputs::NativeSourceProvider, *,
};
use hirsel_engine::store::{Action, Domain, Reference};
use hirsel_engine::{NativeRecordSource, NativeRecords, Verify, store::ResultStore};
use hirsel_layout::{instance::InstanceKey, layout::LayoutBuilder};
use hirsel_lir::{
    LayoutKeys,
    products::LayoutClosure,
    validate::{Inputs, NativeEnvironment},
};
use hirsel_rc::instance::{InstanceId, Instances, SelectionRead};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

const SOURCE_RECIPE: Domain = Domain {
    kind: 0x4e00_0016,
    schema: 1,
};
const LIBRARY_RECIPE: Domain = Domain {
    kind: 0x4e00_0017,
    schema: 1,
};
const REQUEST_LAYOUTS: Domain = Domain {
    kind: 0x4e00_0018,
    schema: 2,
};
const INSTANCE_PLAN: Domain = Domain {
    kind: 0x4e00_002a,
    schema: 2,
};

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
struct NamedKey {
    definition: hirsel_syntax::DefPath,
    body: u32,
    encoding: Vec<u8>,
}
impl NamedKey {
    fn of(key: &InstanceKey) -> Self {
        Self {
            definition: key.definition().clone(),
            body: key.body(),
            encoding: key.encoding().to_vec(),
        }
    }
    fn bind(&self) -> Result<InstanceKey, String> {
        InstanceKey::from_encoding(&self.definition, self.body, self.encoding.clone())
            .map_err(|e| format!("native discovery key: {e:?}"))
    }
}

/// Compact ordinary discovery data. A recipe's typed actuals are a separate
/// content product and are decoded only when that particular instance misses.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub(super) struct Request {
    key: NamedKey,
    recipe: Reference,
    layouts: Option<Reference>,
    plan: Reference,
}
impl Request {
    pub(super) fn source_definition(&self) -> Option<&hirsel_syntax::DefPath> {
        (self.recipe.domain == SOURCE_RECIPE).then_some(&self.key.definition)
    }
}
#[derive(Serialize, Deserialize)]
struct Discovery {
    schema: u32,
    owner: NamedKey,
    ready_source: Vec<u8>,
    seal_demand: hirsel_rc::seal_demand::equations::LocalDemandEquations,
    callees: Vec<hirsel_rc::arena_credit::equations::Function>,
    requests: Vec<Request>,
    selection_reads: Vec<SelectionRead>,
    constant_reads: Vec<hirsel_syntax::DefPath>,
    templates: super::optimizer::TemplateFacts,
    operation_freedom: hirsel_rc::closure_calls::OperationFreedom,
    emission: super::emission::EmissionFacts,
    discharge: super::discharge::NativeDischargeFacts,
}

/// The descriptor returned after one demand has been admitted by actual C or
/// C's independently rechecked local receipt. It retains neither IR nor types.
pub struct DiscoveredInstance {
    pub product: Arc<InstanceProduct>,
    pub ready_source: ReadySourceFacts,
    /// Ordinary physical-capacity equations, never source or C authority.
    /// These retain no Value body and are bound to this discovery output.
    pub seal_demand: hirsel_rc::seal_demand::equations::LocalDemandEquations,
    pub callees: Vec<hirsel_rc::arena_credit::equations::Function>,
    pub dependencies: Vec<InstanceAddress>,
    pub selection_reads: Vec<SelectionRead>,
    pub constant_reads: Vec<hirsel_syntax::DefPath>,
    pub templates: super::optimizer::TemplateFacts,
    pub operation_freedom: hirsel_rc::closure_calls::OperationFreedom,
    pub emission: super::emission::EmissionFacts,
    pub discharge: super::discharge::NativeDischargeFacts,
    /// Independent original plan selections, re-derived before C on every hit.
    pub adapters: Vec<hirsel_lir::ir::Symbol>,
}

/// One immutable request snapshot. Source/native proof authority stays in the
/// respective validators; this index merely carries admitted ordinary requests.
pub struct DemandInstances {
    store: Arc<dyn ResultStore>,
    records: NativeRecords,
    queries: InstanceQueries,
    verify_store: hirsel_engine::store::VerifyStore,
    target: hirsel_lir::TargetSpec,
    policy: ContentHash,
    requests: BTreeMap<Vec<u8>, Request>,
    completed: BTreeMap<Vec<u8>, Arc<DiscoveredInstance>>,
    roots: Vec<InstanceAddress>,
    pub imports: u64,
    pub recipe_decodes: u64,
}
impl DemandInstances {
    pub fn new_from_source(
        source: &impl NativeRecordSource,
        store: Arc<dyn ResultStore>,
        target: hirsel_lir::TargetSpec,
        policy: ContentHash,
    ) -> Self {
        let native = InstanceQueries::validator(&store);
        Self::with_validator(source, store, target, policy, native)
    }
    pub(super) fn with_validator(
        source: &impl NativeRecordSource,
        store: Arc<dyn ResultStore>,
        target: hirsel_lir::TargetSpec,
        policy: ContentHash,
        native: Arc<hirsel_lir::validate::NativeCache>,
    ) -> Self {
        Self {
            records: NativeRecords::new(source.record_types().clone(), store.clone()),
            queries: InstanceQueries::with_validator(store.clone(), native),
            verify_store: hirsel_engine::store::VerifyStore::Off,
            store,
            target,
            policy,
            requests: BTreeMap::new(),
            completed: BTreeMap::new(),
            roots: vec![],
            imports: 0,
            recipe_decodes: 0,
        }
    }
    /// Recompute admitted hits without changing their complete semantic action.
    pub fn with_store_verification(mut self, policy: hirsel_engine::store::VerifyStore) -> Self {
        self.verify_store = policy;
        self
    }
    pub(super) fn registered_requests(&self) -> Result<Vec<(InstanceAddress, Request)>, String> {
        self.requests
            .values()
            .map(|request| Ok((self.address(request.key.bind()?), request.clone())))
            .collect()
    }
    /// Ordinary completed metadata only; subsequent consumers still call
    /// admitted() against the same snapshot-private C authority.
    pub(super) fn attach_completed(
        &mut self,
        key: &InstanceAddress,
        output: Arc<DiscoveredInstance>,
    ) -> Result<(), String> {
        if !self.requests.contains_key(key.instance.encoding())
            || output.product.product.address != key.address(INSTANCE_KIND)
        {
            return Err("completed descriptor has no selected request".into());
        }
        self.completed
            .insert(key.instance.encoding().to_vec(), output);
        Ok(())
    }
    /// Selected source observations and prior read witnesses, without a source
    /// body, recipe or Value decode. Cold import may discover additional reads;
    /// its final action is still computed by the sole instance producer.
    pub(super) fn producer_inputs(
        &self,
        source: &impl NativeSourceProvider,
        key: &InstanceAddress,
        options: &super::super::Options,
    ) -> Result<Vec<u8>, String> {
        let request = self
            .requests
            .get(key.instance.encoding())
            .ok_or("missing producer request")?;
        let previous = self
            .queries
            .candidate(key)
            .and_then(|product| self.descriptor(key, &product).ok());
        let reads = previous
            .as_ref()
            .map_or(&[][..], |d| d.selection_reads.as_slice());
        let constants = previous
            .as_ref()
            .map_or(&[][..], |d| d.constant_reads.as_slice());
        let action = self.action(source, request, options, reads, constants)?;
        serde_json::to_vec(&(request, action.0)).map_err(|e| e.to_string())
    }
    pub fn counts(&self) -> &InstanceQueryCounts {
        &self.queries.counts
    }
    /// Ordinary completed input metadata for immutable dispatch planning. A
    /// task must still independently admit C before publishing a dependency.
    pub(super) fn completed_inputs(
        &self,
    ) -> Result<Vec<(InstanceAddress, Arc<DiscoveredInstance>)>, String> {
        self.completed
            .iter()
            .map(|(encoding, output)| {
                let request = self
                    .requests
                    .get(encoding)
                    .ok_or("completed native request is missing")?;
                Ok((self.address(request.key.bind()?), output.clone()))
            })
            .collect()
    }

    pub(super) fn original_roots(&self) -> &[InstanceAddress] {
        &self.roots
    }

    pub(super) fn bind_original_roots(&mut self, roots: &[InstanceAddress]) -> Result<(), String> {
        if roots.iter().any(|root| {
            root.target_abi != self.target.digest()
                || root.policy != self.policy
                || !self.requests.contains_key(root.instance.encoding())
        }) {
            return Err("seal-demand root has no original native request".into());
        }
        self.roots = roots.to_vec();
        Ok(())
    }
    /// Read a completed instance from this request snapshot, retaining C's
    /// independent current environment admission. No recipe or Value decode.
    pub(super) fn admitted(
        &self,
        key: &InstanceAddress,
        environment: &NativeEnvironment<'_>,
    ) -> Result<
        (
            Arc<DiscoveredInstance>,
            hirsel_lir::validate::CheckedLinkage,
        ),
        String,
    > {
        if key.target_abi != self.target.digest() || key.policy != self.policy {
            return Err("native dependency differs from the request snapshot".into());
        }
        let output = self
            .completed
            .get(key.instance.encoding())
            .ok_or("native instance dependency has not completed")?;
        let operations = self.operations(key, environment)?;
        let scoped = NativeEnvironment {
            operations: &operations,
            ..*environment
        };
        let linkage = self
            .queries
            .linkage(key, output.product.product.action, &scoped)
            .ok_or_else(|| format!(
                "native instance dependency lost current C admission: {} body {} instance {:?}, library={}, action {:?}, record {:?}",
                key.instance.definition(), key.instance.body(), key.instance.digest(),
                output.templates.library, output.product.product.action, output.product.product.record,
            ))?;
        Ok((output.clone(), linkage))
    }
    pub(super) fn operations<'a>(
        &'a self,
        key: &'a InstanceAddress,
        environment: &NativeEnvironment<'a>,
    ) -> Result<super::instance_operations::InstanceOperations<'a>, String> {
        let output = self
            .completed
            .get(key.instance.encoding())
            .ok_or("native operation scope has no completed original plan")?;
        let request = self
            .requests
            .get(key.instance.encoding())
            .ok_or("native operation scope has no original request")?;
        Ok(super::instance_operations::InstanceOperations {
            declarations: environment.operations,
            all_sources: false,
            source: request
                .source_definition()
                .map(|_| (key.instance.definition(), key.instance.digest())),
            pretransfer: Some(
                output
                    .adapters
                    .iter()
                    .map(|symbol| {
                        let mut selected = environment
                            .operations
                            .pretransfer_adapters()
                            .iter()
                            .filter(|reference| &reference.adapter == symbol);
                        let reference = selected
                            .next()
                            .ok_or("native snapshot lost an independently planned adapter")?;
                        if selected.next().is_some() {
                            return Err("native snapshot repeats an independently planned adapter");
                        }
                        Ok(reference.clone())
                    })
                    .collect::<Result<Vec<_>, _>>()?,
            ),
        })
    }
    fn address(&self, key: InstanceKey) -> InstanceAddress {
        InstanceAddress {
            instance: key,
            target_abi: self.target.digest(),
            policy: self.policy,
        }
    }
    fn put(&self, domain: Domain, bytes: &[u8]) -> Result<Reference, String> {
        let expected = domain
            .reference(bytes)
            .ok_or("native record exceeds limit")?;
        let result = self
            .store
            .put_content(domain, bytes)
            .ok_or("native record transport failed")?;
        if expected != result {
            return Err("native record identity differs".into());
        }
        Ok(result)
    }
    fn request(
        &self,
        engine: &impl NativeRecordSource,
        instances: &Instances,
        id: InstanceId,
        table: &hirsel_lir::LayoutTable,
    ) -> Result<Request, String> {
        let key = &instances.view(id).key;
        let (recipe, layouts) = if instances.roles[id.index()].is_library() {
            let recipe =
                LibraryRecipe::capture(instances, id, table)?.encode(&self.records, engine)?;
            let layouts = LayoutClosure::select(
                table,
                &self.target,
                &instances.environments[id.index()]
                    .into_iter()
                    .collect::<Vec<_>>(),
                &[instances.abis[id.index()]],
            )?;
            (
                self.put(LIBRARY_RECIPE, &recipe)?,
                Some(self.put(REQUEST_LAYOUTS, hirsel_codec::to_text(&layouts).as_bytes())?),
            )
        } else {
            (
                self.put(
                    SOURCE_RECIPE,
                    &SourceRecipe::capture(instances, id)?.encode(&self.records, engine)?,
                )?,
                None,
            )
        };
        Ok(Request {
            key: NamedKey::of(key),
            recipe,
            layouts,
            plan: self.put(
                INSTANCE_PLAN,
                &self
                    .records
                    .encode(engine, &InstancePlan::capture(instances, id)?)
                    .ok_or("native original plan encoding failed")?,
            )?,
        })
    }
    pub(super) fn admit(&mut self, request: Request) -> Result<InstanceAddress, String> {
        let key = request.key.bind()?;
        if ![SOURCE_RECIPE, LIBRARY_RECIPE].contains(&request.recipe.domain)
            || request.plan.domain != INSTANCE_PLAN
            || self.store.get_content(request.plan).is_none()
            || (request.recipe.domain == LIBRARY_RECIPE) != request.layouts.is_some()
            || request.layouts.is_some_and(|r| r.domain != REQUEST_LAYOUTS)
            || self.store.get_content(request.recipe).is_none()
            || request
                .layouts
                .is_some_and(|r| self.store.get_content(r).is_none())
        {
            return Err("native instance request has missing or foreign products".into());
        }
        if let Some(previous) = self.requests.get(key.encoding()) {
            if previous != &request {
                return Err("one native address has inconsistent recipes".into());
            }
        } else {
            if self.requests.len() >= hirsel_rc::instance::MAX_INSTANCES {
                return Err("native instance limit".into());
            }
            self.requests.insert(key.encoding().to_vec(), request);
        }
        Ok(self.address(key))
    }

    /// Reconstruct only independent declarations and ABI inputs. Ordinary
    /// instance plans carry typed actuals; current GEN-5 selects materialized
    /// source references. No source, Value or Ready body is read here.
    pub(super) fn bind_planned_adapters<'a>(
        &self,
        source: &impl NativeSourceProvider,
        requests: impl IntoIterator<Item = &'a Request>,
        registry: &mut hirsel_layout::operation::OperationRegistry,
    ) -> Result<(), String> {
        let declarations = source.declarations();
        let owners = hirsel_rc::library::owners(
            &declarations.program.types,
            &declarations.program.signatures,
        )
        .over(&declarations.program.owners);
        let mut layouts = LayoutBuilder::new(&self.target, &owners);
        layouts.prepare_native_controls();
        let mut instances = Instances::new(
            declarations.program.types.clone(),
            declarations.program.memo.clone(),
        );
        for request in requests {
            let bytes = self
                .store
                .get_content(request.plan)
                .ok_or("missing native original instance plan")?;
            let plan: InstancePlan = self
                .records
                .decode(source, &bytes)
                .ok_or("native original instance plan decoding failed")?;
            plan.register(
                &declarations.program,
                &declarations.context,
                &request.key.bind()?,
                request.source_definition().is_none(),
                &mut layouts,
                &mut instances,
            )?;
        }
        hirsel_rc::import::bind_pretransfer_inventory(registry, &declarations.program, &instances)
    }

    /// Existing source/GEN-5 and constant root policy, with compact declaration
    /// registration only. No Definition or Value is reconstructed here.
    pub fn roots_from(
        &mut self,
        source: &impl NativeSourceProvider,
    ) -> Result<Vec<InstanceAddress>, String> {
        self.root_requests(source, source.declarations())
    }
    fn root_requests(
        &mut self,
        engine: &impl NativeRecordSource,
        declarations: &NativeDeclarations,
    ) -> Result<Vec<InstanceAddress>, String> {
        let owners = hirsel_rc::library::owners(
            &declarations.program.types,
            &declarations.program.signatures,
        )
        .over(&declarations.program.owners);
        let mut layouts = LayoutBuilder::new(&self.target, &owners);
        layouts.prepare_native_controls();
        let (instances, roots) = hirsel_rc::instance::seed_instances(
            &declarations.program,
            &declarations.context,
            &mut layouts,
        )
        .map_err(|e| format!("native root registration: {e:?}"))?;
        let table = layouts.finish();
        let mut requests = roots
            .into_iter()
            .map(|id| self.request(engine, &instances, id, &table))
            .collect::<Result<Vec<_>, _>>()?;
        requests.sort_by(|a, b| a.key.encoding.cmp(&b.key.encoding));
        let roots = requests
            .into_iter()
            .map(|r| self.admit(r))
            .collect::<Result<Vec<_>, _>>()?;
        self.bind_original_roots(&roots)?;
        Ok(roots)
    }

    fn action(
        &self,
        inputs: &impl NativeSourceProvider,
        request: &Request,
        options: &super::super::Options,
        selection_reads: &[SelectionRead],
        constant_reads: &[hirsel_syntax::DefPath],
    ) -> Result<Action, String> {
        let declarations = inputs.declarations();
        let source = if request.recipe.domain == SOURCE_RECIPE {
            Some(inputs.source_observation(&request.key.definition)?)
        } else {
            None
        };
        let constants = constant_reads
            .iter()
            .map(|path| {
                let current = declarations
                    .program
                    .constants
                    .get(path)
                    .map(|value| -> Result<ContentHash, String> {
                        let bytes = self
                            .records
                            .encode(inputs, value)
                            .ok_or("current static input cannot be encoded")?;
                        Ok(ContentHash::of_parts(
                            "hirsel.native.materialized-static-read.v1",
                            [bytes.as_slice()],
                        ))
                    })
                    .transpose()?;
                Ok((path, current))
            })
            .collect::<Result<Vec<_>, String>>()?;
        let bytes = serde_json::to_vec(&(
            1u32,
            crate::COMPILER_IDENTITY.as_slice(),
            hirsel_lir::LIR_VERSION,
            hirsel_lir::LAYOUT_VERSION,
            hirsel_lir::ABI_VERSION,
            declarations.program.input.digest,
            declarations.roots.registry_revision,
            inputs.query_policy().source,
            inputs.verification() == Verify::On,
            request,
            source,
            options.flat_walk,
            format!("{:?}", options.discharge.sites(&request.key.definition)),
            selection_reads
                .iter()
                .map(|read| read.current(&declarations.program.instance_bodies))
                .collect::<Vec<_>>(),
            constants,
        ))
        .map_err(|e| e.to_string())?;
        Ok(Action(ContentHash::of_parts(
            "hirsel.native.demand-import-action.v12",
            [bytes.as_slice()],
        )))
    }

    fn descriptor(
        &self,
        key: &InstanceAddress,
        product: &InstanceProduct,
    ) -> Result<Discovery, String> {
        let bytes = self
            .store
            .get_content(product.discovery)
            .ok_or("missing native discovery")?;
        let discovery: Discovery = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
        discovery.seal_demand.validate()?;
        if discovery.schema != 9
            || discovery.seal_demand.owner()
                != &(key.instance.definition().clone(), key.instance.digest())
            || !discovery.discharge.canonical()
            || !discovery.emission.canonical()
            || !discovery.operation_freedom.canonical()
            || !discovery.templates.canonical()
            || discovery.owner.bind()? != key.instance
            || discovery
                .requests
                .windows(2)
                .any(|w| w[0].key.encoding >= w[1].key.encoding)
            || discovery.callees.windows(2).any(|w| w[0] >= w[1])
            || discovery.constant_reads.windows(2).any(|w| w[0] >= w[1])
            || discovery.selection_reads.iter().any(|read| !read.valid())
            || discovery.selection_reads.windows(2).any(|w| {
                (&w[0].declaration, &w[0].arguments) >= (&w[1].declaration, &w[1].arguments)
            })
        {
            return Err("noncanonical native discovery".into());
        }
        Ok(discovery)
    }

    fn discovered(
        &mut self,
        key: &InstanceAddress,
        product: Arc<InstanceProduct>,
        adapters: Vec<hirsel_lir::ir::Symbol>,
    ) -> Result<Arc<DiscoveredInstance>, String> {
        let discovery = self.descriptor(key, &product)?;
        let ready_source = ReadySourceFacts::decode(&discovery.ready_source)?;
        let dependencies = discovery
            .requests
            .into_iter()
            .map(|r| self.admit(r))
            .collect::<Result<Vec<_>, _>>()?;
        let output = Arc::new(DiscoveredInstance {
            product,
            ready_source,
            seal_demand: discovery.seal_demand,
            callees: discovery.callees,
            dependencies,
            selection_reads: discovery.selection_reads,
            constant_reads: discovery.constant_reads,
            templates: discovery.templates,
            operation_freedom: discovery.operation_freedom,
            emission: discovery.emission,
            discharge: discovery.discharge,
            adapters,
        });
        self.completed
            .insert(key.instance.encoding().to_vec(), output.clone());
        Ok(output)
    }

    /// Complete one base Value query. Its dependencies are returned as compact
    /// requests; no recursive native import is used to decide this query's hit.
    /// The sole native producer over source inputs. Lookup validates prior
    /// dependency observations before any recipe/original body/Value decoding.
    pub fn instance_from(
        &mut self,
        inputs: &mut impl NativeSourceProvider,
        key: &InstanceAddress,
        options: &super::super::Options,
    ) -> Result<Arc<DiscoveredInstance>, String> {
        if key.target_abi != self.target.digest()
            || key.policy != self.policy
            || options.target != self.target
        {
            return Err("native demand differs from the request target/policy".into());
        }
        let request = self
            .requests
            .get(key.instance.encoding())
            .cloned()
            .ok_or("native instance was not discovered")?;
        // Read ordinary prior dependency metadata before deciding a hit.
        // Only the later C lookup can admit the candidate as a native output.
        let previous = self
            .queries
            .candidate(key)
            .and_then(|product| self.descriptor(key, &product).ok());
        let selection_reads = previous
            .as_ref()
            .map_or(&[][..], |record| record.selection_reads.as_slice());
        let constant_reads = previous
            .as_ref()
            .map_or(&[][..], |record| record.constant_reads.as_slice());
        let action = self.action(inputs, &request, options, selection_reads, constant_reads)?;
        if previous.is_some()
            && let Some(output) = self.completed.get(key.instance.encoding())
            && output.product.product.action == action
        {
            return Ok(output.clone());
        }
        let mut original_source = None;
        let mut operations = if request.recipe.domain == SOURCE_RECIPE {
            let selected = hirsel_lir::ir::Symbol {
                kind: hirsel_lir::ir::SymbolKind::Function,
                definition: key.instance.definition().clone(),
                instance: Some(key.instance.digest()),
            };
            let (evidence, original) = inputs.source_invocations(
                &request.key.definition,
                &selected,
                hirsel_tir::BodyId(key.instance.body()),
            )?;
            original_source = original;
            let mut operations = inputs.declarations().program.operation_registry();
            operations
                .invocations
                .insert(evidence.body.definition.clone(), evidence.invocations);
            operations.invocation_bodies.push(evidence.body);
            operations
        } else {
            inputs.declarations().program.operation_registry()
        };
        self.bind_planned_adapters(inputs, [&request], &mut operations)?;
        let adapters = operations
            .pretransfer_adapters
            .iter()
            .map(|reference| reference.adapter.clone())
            .collect::<Vec<_>>();
        let manifest = inputs.declarations().manifest()?;
        let environment = NativeEnvironment {
            target: &self.target,
            operations: &operations,
            manifest: &manifest,
            discharge: Some(&options.discharge),
        };
        let recompute = self.verify_store == hirsel_engine::store::VerifyStore::Recompute
            || options.validation == crate::native::Validation::EveryStage;
        if previous.is_some()
            && !recompute
            && inputs.query_policy().reuse
            && let Some(product) = self.queries.lookup(key, action, &environment)
            && let Ok(output) = self.discovered(key, product, adapters.clone())
        {
            return Ok(output);
        }

        if previous.is_none() || recompute || !inputs.query_policy().reuse {
            self.queries.counts.misses += 1;
        }
        let recipe_bytes = self
            .store
            .get_content(request.recipe)
            .ok_or("missing demanded recipe")?;
        self.recipe_decodes += 1;
        let bound: Option<LayoutClosure> = request
            .layouts
            .map(|r| {
                let bytes = self
                    .store
                    .get_content(r)
                    .ok_or("missing requested environment")?;
                hirsel_codec::from_text(std::str::from_utf8(&bytes).map_err(|e| e.to_string())?)
                    .map_err(|e| format!("native environment: {e:?}"))
            })
            .transpose()?;
        let source = if request.recipe.domain == SOURCE_RECIPE {
            let recipe = SourceRecipe::decode(&self.records, inputs, &recipe_bytes)?;
            if recipe.key()? != key.instance {
                return Err("source recipe differs from demanded key".into());
            }
            let (original, source_input) = match original_source {
                Some(original) => original,
                None => inputs.original_source(&request.key.definition)?,
            };
            if let Some(input) = &source_input {
                input
                    .demand_body(&key.instance)
                    .map_err(|e| format!("native original source: {e:?}"))?;
            }
            Some((recipe, original, source_input))
        } else {
            None
        };
        let owners = hirsel_rc::library::owners(
            &inputs.declarations().program.types,
            &inputs.declarations().program.signatures,
        )
        .over(&inputs.declarations().program.owners);
        let mut layouts =
            LayoutBuilder::from_products(&self.target, &owners, &bound.iter().collect::<Vec<_>>())
                .map_err(|e| format!("native request layout binding: {e:?}"))?;
        layouts.prepare_native_controls();
        let (mut instances, id) = if let Some((recipe, original, source_input)) = source {
            let restored = recipe.restore(
                &inputs.declarations().program,
                &inputs.declarations().context,
                original,
                &mut layouts,
            )?;
            // Keep the original snapshot through restoration. A future receipt
            // factory consumes it here, with complete independent expansions;
            // the current unavailable contract remains unavailable.
            drop(source_input);
            restored
        } else {
            let recipe = LibraryRecipe::decode(&self.records, inputs, &recipe_bytes)?;
            if recipe.key()? != key.instance {
                return Err("library recipe differs from demanded key".into());
            }
            let (instances, id) = recipe.restore(&inputs.declarations().program, &mut layouts)?;
            let instances = hirsel_rc::instance::discover_library_instance(
                &inputs.declarations().program,
                &inputs.declarations().context,
                &mut layouts,
                instances,
                id,
            )
            .map_err(|error| format!("native library cleanup discovery: {error:?}"))?;
            (instances, id)
        };
        self.imports += 1;
        let imported = hirsel_rc::import::import_instance(
            &inputs.declarations().program,
            &mut instances,
            id,
            options.discharge.sites(&request.key.definition),
            &mut layouts,
        )
        .map_err(|e| e.to_string())?;
        let removed = if let Some(discharged) = &imported.discharged {
            hirsel_lir::validate::validate_discharge(
                &discharged.reference,
                &discharged.discharged,
                options
                    .discharge
                    .sites(&discharged.definition)
                    .ok_or("missing discharge plan")?,
            )
            .map_err(|e| format!("native discharge: {e:?}"))?
        } else {
            BTreeSet::new()
        };
        for request in imported.required {
            hirsel_rc::library::instantiate(&mut layouts, &mut instances, request)
                .map_err(|e| format!("native library registration: {e:?}"))?;
        }
        // Root registration also selects callable relocations of registry
        // constants. A worker-local recipe restores only this instance, so
        // register the callables of the statics it actually reads before the
        // common encoder resolves those relocations. These are ordinary recipe
        // dependencies, never source/native validation authority.
        let static_reads = imported.unit.instructions.iter().filter_map(|instruction| {
            let hirsel_lir::ir::Inst::ReadStatic { symbol, .. } = instruction else {
                return None;
            };
            let symbol = &imported.unit.tables.symbols[symbol.index()];
            (symbol.kind == hirsel_lir::ir::SymbolKind::Constant)
                .then(|| {
                    inputs
                        .declarations()
                        .program
                        .constants
                        .get(&symbol.definition)
                })
                .flatten()
        });
        hirsel_rc::instance::register_static_callables(
            &inputs.declarations().program,
            &inputs.declarations().context,
            &mut layouts,
            &mut instances,
            static_reads,
        )
        .map_err(|e| format!("native static callable registration: {e:?}"))?;
        // Batch import discovers these prerequisites while building the adapter.
        // A one-instance import must plan them before checking the caller too.
        // Use original instance actuals, never the candidate's direct-call rows.
        let primitives = instances
            .ids()
            .map(|other| {
                hirsel_rc::library::keyed::primitive_request(
                    &inputs.declarations().program,
                    instances.view(other).key.definition(),
                    instances.actuals[other.index()].arguments.clone(),
                )
            })
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("native adapter primitive plan: {e:?}"))?;
        for request in primitives.into_iter().flatten() {
            hirsel_rc::library::instantiate(&mut layouts, &mut instances, request)
                .map_err(|e| format!("native adapter primitive registration: {e:?}"))?;
        }
        // LayoutBuilder can retain equal ABI content at distinct selectors.
        // In the import table use the original instance planner's exact ID;
        // first matching canonical content need not be that registered slot.
        let mut original = inputs.declarations().program.operation_registry();
        hirsel_rc::import::bind_pretransfer_inventory(
            &mut original,
            &inputs.declarations().program,
            &instances,
        )?;
        let keys = LayoutKeys::new(layouts.table());
        let rebound = operations
            .pretransfer_adapters
            .iter()
            .map(|expected| {
                let reference = original
                    .pretransfer_adapters
                    .iter()
                    .find(|actual| {
                        actual.adapter == expected.adapter && actual.primitive == expected.primitive
                    })
                    .ok_or("native import lost the independently selected adapter instance")?;
                let (abi, table) = hirsel_lir::operation::OperationDeclarations::pretransfer_abi(
                    &operations,
                    expected,
                    &self.target,
                )
                .ok_or("native adapter has no independent canonical ABI")?;
                if keys.fn_abi(&layouts.table().fn_abis[reference.abi.0 as usize])
                    != LayoutKeys::new(&table).fn_abi(&abi)
                {
                    return Err("native original adapter ABI differs from its declaration");
                }
                Ok(reference.clone())
            })
            .collect::<Result<Vec<_>, _>>()?;
        for (actual, reference) in operations.pretransfer_actuals.iter_mut().zip(&rebound) {
            actual.1 = reference.abi;
        }
        operations.pretransfer_adapters = rebound;
        let statics = hirsel_rc::import::instance_statics(
            &inputs.declarations().program,
            &instances,
            &imported.unit,
            &mut layouts,
        )
        .map_err(|e| e.to_string())?;
        let ready_source = ReadySourceFacts::for_instance(&instances, id).encode()?;
        let table = layouts.finish();
        let mut unit = hirsel_rc::flat_walk::rewrite(
            imported.unit,
            &table,
            &hirsel_rc::flat_walk::Options {
                enabled: options.flat_walk,
            },
        );
        hirsel_rc::import::prune_pretransfer(
            std::slice::from_mut(&mut unit),
            &table,
            &operations.pretransfer_adapters,
        );
        let mut requests = instances
            .ids()
            .filter(|&other| other != id)
            .map(|other| self.request(inputs, &instances, other, &table))
            .collect::<Result<Vec<_>, _>>()?;
        requests.sort_by(|a, b| a.key.encoding.cmp(&b.key.encoding));
        let mut callees = unit
            .tables
            .calls
            .iter()
            .filter_map(|call| match call.callee {
                hirsel_lir::ir::Callee::Direct(symbol) => {
                    let symbol = &unit.tables.symbols[symbol.index()];
                    symbol
                        .instance
                        .map(|instance| (symbol.definition.clone(), instance))
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        callees.sort();
        callees.dedup();
        let selection_reads = instances.selection_reads();
        let mut constant_reads = unit
            .instructions
            .iter()
            .filter_map(|instruction| {
                let hirsel_lir::ir::Inst::ReadStatic { symbol, .. } = instruction else {
                    return None;
                };
                let symbol = &unit.tables.symbols[symbol.index()];
                (symbol.kind == hirsel_lir::ir::SymbolKind::Constant
                    && symbol.definition != hirsel_rc::import::empty_sequence()
                    && symbol.definition != hirsel_rc::import::nullary_variant())
                .then(|| symbol.definition.clone())
            })
            .collect::<Vec<_>>();
        constant_reads.sort();
        constant_reads.dedup();
        let action = self.action(inputs, &request, options, &selection_reads, &constant_reads)?;
        let finalizers = hirsel_rc::cleanup::Finalizers::planned(&instances, &table);
        let reached_finalizers = finalizers.reached(&unit, &table);
        let templates = super::optimizer::TemplateFacts::of(
            &unit,
            reached_finalizers
                .iter()
                .map(|(_, finalizer)| (finalizer.definition.clone(), finalizer.instance)),
        );
        let discovery = self.put(
            INSTANCE_DISCOVERY_DOMAIN,
            &serde_json::to_vec(&Discovery {
                schema: 9,
                owner: NamedKey::of(&key.instance),
                ready_source,
                seal_demand: hirsel_rc::seal_demand::equations::LocalDemandEquations::extract(
                    &unit,
                )?,
                callees,
                requests,
                selection_reads,
                constant_reads,
                templates,
                operation_freedom: hirsel_rc::closure_calls::OperationFreedom::extract(
                    &unit, &table,
                ),
                emission: super::emission::EmissionFacts::extract(&unit, []),
                discharge: super::discharge::NativeDischargeFacts::checked(&removed),
            })
            .map_err(|e| e.to_string())?,
        )?;
        let keys = LayoutKeys::new(&table);
        let portable_statics = statics
            .into_iter()
            .map(|mut data| {
                let layouts = data
                    .blocks
                    .iter()
                    .map(|block| keys.key(block.layout).0)
                    .collect::<Vec<_>>();
                for (i, block) in data.blocks.iter_mut().enumerate() {
                    block.layout = hirsel_lir::LayoutId(i as u32);
                }
                (layouts, hirsel_codec::to_text(&data))
            })
            .collect::<Vec<_>>();

        let mut additional_abis = reached_finalizers
            .iter()
            .map(|(_, f)| f.abi)
            .collect::<Vec<_>>();
        additional_abis.extend(operations.pretransfer_adapters.iter().map(|r| r.abi));
        let mut portable_finalizers = reached_finalizers
            .into_iter()
            .map(|(layout, finalizer)| {
                (
                    keys.key(layout).0,
                    finalizer.definition.clone(),
                    finalizer.instance,
                    keys.fn_abi(&table.fn_abis[finalizer.abi.0 as usize]),
                    finalizer.origin == hirsel_lir::ir::UnitOrigin::Library,
                )
            })
            .collect::<Vec<_>>();
        portable_finalizers.sort();
        let statics_finalizers = self.put(
            STATICS_FINALIZERS_DOMAIN,
            &serde_json::to_vec(&(portable_statics, portable_finalizers))
                .map_err(|e| e.to_string())?,
        )?;
        let inputs = Inputs {
            memo: Default::default(),
            target: &self.target,
            layouts: &table,
            operations: &operations,
            manifest: &manifest,
            discharge: Some(&options.discharge),
        };
        let (_, product) = self.queries.publish(
            key,
            action,
            unit,
            &inputs,
            &additional_abis,
            discovery,
            statics_finalizers,
        )?;
        self.discovered(key, product, adapters)
    }
}

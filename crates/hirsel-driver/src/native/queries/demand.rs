//! Demand import over source and library recipes. Descriptor admission precedes
//! typed recipe/body materialization; native C owns every reusable Value.
use super::{
    declarations::NativeDeclarations,
    instance::*,
    ready::ReadySourceFacts,
    recipes::*,
    source_inputs::{LiveNativeSource, NativeSourceProvider},
    *,
};
use hirsel_artifact_store::{Action, Domain, Reference};
use hirsel_engine::{NativeRecordSource, NativeRecords, QueryEngine, Verify, store::ResultStore};
use hirsel_layout::{instance::InstanceKey, layout::LayoutBuilder};
use hirsel_lir::{
    LayoutKeys,
    products::LayoutClosure,
    validate::{Inputs, NativeEnvironment},
};
use hirsel_rc::instance::{InstanceId, Instances, SelectionRead};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

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
}
#[derive(Serialize, Deserialize)]
struct Discovery {
    schema: u32,
    owner: NamedKey,
    ready_source: Vec<u8>,
    callees: Vec<hirsel_rc::arena_credit::equations::Function>,
    requests: Vec<Request>,
    selection_reads: Vec<SelectionRead>,
    constant_reads: Vec<hirsel_syntax::DefPath>,
    templates: super::optimizer::TemplateFacts,
    operation_freedom: hirsel_rc::closure_calls::OperationFreedom,
    emission: super::emission::EmissionFacts,
}

/// The descriptor returned after one demand has been admitted by actual C or
/// C's independently rechecked local receipt. It retains neither IR nor types.
pub struct DiscoveredInstance {
    pub product: Arc<InstanceProduct>,
    pub ready_source: ReadySourceFacts,
    pub callees: Vec<hirsel_rc::arena_credit::equations::Function>,
    pub dependencies: Vec<InstanceAddress>,
    pub selection_reads: Vec<SelectionRead>,
    pub constant_reads: Vec<hirsel_syntax::DefPath>,
    pub templates: super::optimizer::TemplateFacts,
    pub operation_freedom: hirsel_rc::closure_calls::OperationFreedom,
    pub emission: super::emission::EmissionFacts,
}

/// One immutable request snapshot. Source/native proof authority stays in the
/// respective validators; this index merely carries admitted ordinary requests.
pub struct DemandInstances {
    store: Arc<dyn ResultStore>,
    records: NativeRecords,
    queries: InstanceQueries,
    target: hirsel_lir::TargetSpec,
    policy: ContentHash,
    requests: BTreeMap<Vec<u8>, Request>,
    completed: BTreeMap<Vec<u8>, Arc<DiscoveredInstance>>,
    pub imports: u64,
    pub recipe_decodes: u64,
}
impl DemandInstances {
    pub fn new(
        engine: &QueryEngine,
        store: Arc<dyn ResultStore>,
        target: hirsel_lir::TargetSpec,
        policy: ContentHash,
    ) -> Self {
        Self::new_from_source(engine, store, target, policy)
    }
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
            store,
            target,
            policy,
            requests: BTreeMap::new(),
            completed: BTreeMap::new(),
            imports: 0,
            recipe_decodes: 0,
        }
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
        let linkage = self
            .queries
            .linkage(key, output.product.product.action, environment)
            .ok_or("native instance dependency lost current C admission")?;
        Ok((output.clone(), linkage))
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
        })
    }
    pub(super) fn admit(&mut self, request: Request) -> Result<InstanceAddress, String> {
        let key = request.key.bind()?;
        if ![SOURCE_RECIPE, LIBRARY_RECIPE].contains(&request.recipe.domain)
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
            if previous.recipe != request.recipe || previous.layouts != request.layouts {
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

    /// Existing source/GEN-5 and constant root policy, with compact declaration
    /// registration only. No Definition or Value is reconstructed here.
    pub fn roots(
        &mut self,
        engine: &QueryEngine,
        declarations: &NativeDeclarations,
    ) -> Result<Vec<InstanceAddress>, String> {
        self.root_requests(engine, declarations)
    }
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
        requests.into_iter().map(|r| self.admit(r)).collect()
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
            "hirsel.native.demand-import-action.v8",
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
        if discovery.schema != 6
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
            callees: discovery.callees,
            dependencies,
            selection_reads: discovery.selection_reads,
            constant_reads: discovery.constant_reads,
            templates: discovery.templates,
            operation_freedom: discovery.operation_freedom,
            emission: discovery.emission,
        });
        self.completed
            .insert(key.instance.encoding().to_vec(), output.clone());
        Ok(output)
    }

    /// Complete one base Value query. Its dependencies are returned as compact
    /// requests; no recursive native import is used to decide this query's hit.
    pub fn instance(
        &mut self,
        engine: &mut QueryEngine,
        declarations: &mut NativeDeclarations,
        key: &InstanceAddress,
        options: &super::super::Options,
    ) -> Result<Arc<DiscoveredInstance>, String> {
        self.instance_from(
            &mut LiveNativeSource {
                engine,
                declarations,
                store: self.store.clone(),
                ordinary: BTreeMap::new(),
            },
            key,
            options,
        )
    }

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
        let operations = inputs.declarations().program.operation_registry();
        let manifest = inputs.declarations().manifest()?;
        let environment = NativeEnvironment {
            target: &self.target,
            operations: &operations,
            manifest: &manifest,
            discharge: Some(&options.discharge),
        };
        if previous.is_some()
            && inputs.query_policy().reuse
            && let Some(product) = self.queries.lookup(key, action, &environment)
            && let Ok(output) = self.discovered(key, product)
        {
            return Ok(output);
        }

        if previous.is_none() || !inputs.query_policy().reuse {
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
            let (original, source_input) = inputs.original_source(&request.key.definition)?;
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
            recipe.restore(&inputs.declarations().program, &mut layouts)?
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
        if let Some(discharged) = &imported.discharged {
            let removed = hirsel_lir::validate::validate_discharge(
                &discharged.reference,
                &discharged.discharged,
                options
                    .discharge
                    .sites(&discharged.definition)
                    .ok_or("missing discharge plan")?,
            )
            .map_err(|e| format!("native discharge: {e:?}"))?;
            if options
                .discharge
                .sites(&discharged.definition)
                .is_some_and(|sites| !sites.is_subset(&removed))
            {
                return Err("accepted discharge has no native check".into());
            }
        }
        for request in imported.required {
            hirsel_rc::library::instantiate(&mut layouts, &mut instances, request)
                .map_err(|e| format!("native library registration: {e:?}"))?;
        }
        let statics = hirsel_rc::import::instance_statics(
            &inputs.declarations().program,
            &instances,
            &imported.unit,
            &mut layouts,
        )
        .map_err(|e| e.to_string())?;
        let ready_source = ReadySourceFacts::for_instance(&instances, id).encode()?;
        let table = layouts.finish();
        let unit = hirsel_rc::flat_walk::rewrite(
            imported.unit,
            &table,
            &hirsel_rc::flat_walk::Options {
                enabled: options.flat_walk,
            },
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
                schema: 6,
                owner: NamedKey::of(&key.instance),
                ready_source,
                callees,
                requests,
                selection_reads,
                constant_reads,
                templates,
                operation_freedom: hirsel_rc::closure_calls::OperationFreedom::extract(
                    &unit, &table,
                ),
                emission: super::emission::EmissionFacts::extract(&unit, []),
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

        let additional_abis = reached_finalizers
            .iter()
            .map(|(_, f)| f.abi)
            .collect::<Vec<_>>();
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
        self.discovered(key, product)
    }
}

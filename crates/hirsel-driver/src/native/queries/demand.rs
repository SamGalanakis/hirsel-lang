//! Demand import over source and library recipes. Descriptor admission precedes
//! typed recipe/body materialization; native C owns every reusable Value.
use super::{
    declarations::NativeDeclarations, instance::*, ready::ReadySourceFacts, recipes::*, *,
};
use hirsel_artifact_store::{Action, Domain, Reference};
use hirsel_engine::{NativeRecords, QueryEngine, Verify, store::ResultStore};
use hirsel_layout::{instance::InstanceKey, layout::LayoutBuilder};
use hirsel_lir::{
    LayoutKeys,
    products::LayoutClosure,
    validate::{Inputs, NativeEnvironment},
};
use hirsel_rc::instance::{InstanceId, Instances};
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
    schema: 1,
};

#[derive(Clone, Debug, Serialize, Deserialize)]
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
#[derive(Clone, Debug, Serialize, Deserialize)]
struct Request {
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
}

/// The descriptor returned after one demand has been admitted by actual C or
/// C's independently rechecked local receipt. It retains neither IR nor types.
pub struct DiscoveredInstance {
    pub product: Arc<InstanceProduct>,
    pub ready_source: ReadySourceFacts,
    pub callees: Vec<hirsel_rc::arena_credit::equations::Function>,
    pub dependencies: Vec<InstanceAddress>,
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
        Self {
            records: NativeRecords::new(engine.types().clone(), store.clone()),
            queries: InstanceQueries::new(store.clone()),
            store,
            target,
            policy,
            requests: BTreeMap::new(),
            completed: BTreeMap::new(),
            imports: 0,
            recipe_decodes: 0,
        }
    }
    pub fn counts(&self) -> &InstanceQueryCounts {
        &self.queries.counts
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
        engine: &QueryEngine,
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
    fn admit(&mut self, request: Request) -> Result<InstanceAddress, String> {
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
        engine: &QueryEngine,
        declarations: &NativeDeclarations,
        request: &Request,
        options: &super::super::Options,
    ) -> Result<Action, String> {
        let source = if request.recipe.domain == SOURCE_RECIPE {
            let source = declarations
                .sources
                .get(&request.key.definition)
                .ok_or("missing native source output")?;
            Some((
                source.source.implementation_revision,
                source.signature.fingerprint,
                source
                    .lower
                    .as_ref()
                    .map(|v| (v.reference(), v.observation_identity())),
            ))
        } else {
            None
        };
        let bytes = serde_json::to_vec(&(
            1u32,
            crate::COMPILER_IDENTITY.as_slice(),
            hirsel_lir::LIR_VERSION,
            hirsel_lir::LAYOUT_VERSION,
            hirsel_lir::ABI_VERSION,
            declarations.program.input.digest,
            declarations.roots.registry_revision,
            engine.native_query_policy().source,
            engine.verify == Verify::On,
            request,
            source,
            options.flat_walk,
            format!("{:?}", options.discharge.sites(&request.key.definition)),
        ))
        .map_err(|e| e.to_string())?;
        Ok(Action(ContentHash::of_parts(
            "hirsel.native.demand-import-action.v1",
            [bytes.as_slice()],
        )))
    }

    fn discovered(
        &mut self,
        key: &InstanceAddress,
        product: Arc<InstanceProduct>,
    ) -> Result<Arc<DiscoveredInstance>, String> {
        let bytes = self
            .store
            .get_content(product.discovery)
            .ok_or("missing native discovery")?;
        let discovery: Discovery = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
        if discovery.schema != 1
            || discovery.owner.bind()? != key.instance
            || discovery
                .requests
                .windows(2)
                .any(|w| w[0].key.encoding >= w[1].key.encoding)
            || discovery.callees.windows(2).any(|w| w[0] >= w[1])
        {
            return Err("noncanonical native discovery".into());
        }
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
        let action = self.action(engine, declarations, &request, options)?;
        if let Some(output) = self.completed.get(key.instance.encoding())
            && output.product.product.action == action
        {
            return Ok(output.clone());
        }
        let operations = declarations.program.operation_registry();
        let manifest = hirsel_core_contract::Manifest::skeleton(
            declarations.program.operations.values().cloned(),
        );
        let environment = NativeEnvironment {
            target: &self.target,
            operations: &operations,
            manifest: &manifest,
            discharge: Some(&options.discharge),
        };
        if engine.native_query_policy().reuse
            && let Some(product) = self.queries.lookup(key, action, &environment)
            && let Ok(output) = self.discovered(key, product)
        {
            return Ok(output);
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
            let recipe = SourceRecipe::decode(&self.records, engine, &recipe_bytes)?;
            if recipe.key()? != key.instance {
                return Err("source recipe differs from demanded key".into());
            }
            let original = declarations
                .body(engine, &request.key.definition)
                .map_err(|e| format!("native source body: {e:?}"))?;
            Some((recipe, original))
        } else {
            None
        };
        let owners = hirsel_rc::library::owners(
            &declarations.program.types,
            &declarations.program.signatures,
        )
        .over(&declarations.program.owners);
        let mut layouts =
            LayoutBuilder::from_products(&self.target, &owners, &bound.iter().collect::<Vec<_>>())
                .map_err(|e| format!("native request layout binding: {e:?}"))?;
        layouts.prepare_native_controls();
        let (mut instances, id) = if let Some((recipe, original)) = source {
            recipe.restore(
                &declarations.program,
                &declarations.context,
                original,
                &mut layouts,
            )?
        } else {
            let recipe = LibraryRecipe::decode(&self.records, engine, &recipe_bytes)?;
            if recipe.key()? != key.instance {
                return Err("library recipe differs from demanded key".into());
            }
            recipe.restore(&declarations.program, &mut layouts)?
        };
        self.imports += 1;
        let imported = hirsel_rc::import::import_instance(
            &declarations.program,
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
            &declarations.program,
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
            .map(|other| self.request(engine, &instances, other, &table))
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
        let discovery = self.put(
            INSTANCE_DISCOVERY_DOMAIN,
            &serde_json::to_vec(&Discovery {
                schema: 1,
                owner: NamedKey::of(&key.instance),
                ready_source,
                callees,
                requests,
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
        let finalizers = hirsel_rc::cleanup::Finalizers::planned(&instances, &table);
        let mut portable_finalizers = finalizers
            .0
            .iter()
            .map(|(layout, finalizer)| {
                (
                    keys.key(*layout).0,
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
        let action = self.action(engine, declarations, &request, options)?;
        let (_, product) = self.queries.publish(
            key,
            action,
            unit,
            &inputs,
            &instances.abis,
            discovery,
            statics_finalizers,
        )?;
        self.discovered(key, product)
    }
}

//! The existing callable fixed point over explicitly read, C-admitted Values.
use super::*;
use hirsel_lir::{ir::Callee, products::Binding, stage::Value};

pub(in crate::native::queries) struct Scope {
    pub outputs: Vec<(InstanceAddress, Arc<DiscoveredInstance>)>,
    pub operations: hirsel_layout::operation::OperationRegistry,
    requests: BTreeMap<Vec<u8>, Request>,
}
impl Scope {
    pub(in crate::native::queries) fn retained_bytes<'a>(
        &mut self,
        admitted: impl IntoIterator<Item = &'a Request>,
    ) -> u64 {
        let admitted = admitted
            .into_iter()
            .map(|request| Arc::as_ptr(&request.0))
            .collect::<std::collections::HashSet<_>>();
        let mut weight = hirsel_engine::NativeMetadataWeight::default();
        weight.vec(&self.outputs, |weight, (key, output)| {
            weight.path(key.instance.definition());
            weight.add(key.instance.retained_encoding_bytes());
            weight.add(output.retained_metadata_bytes() as usize);
        });
        weight.map(&self.requests, |weight, key, request| {
            weight.add(key.capacity());
            if !admitted.contains(&Arc::as_ptr(&request.0)) {
                weight.add(request.retained_metadata_bytes() as usize);
            }
        });
        weight.bytes()
            + super::super::ownership_compilation::metadata::operations(&mut self.operations)
    }
}
impl DemandInstances {
    fn cached_callable_scope(
        &self,
        originals: &[(InstanceAddress, Arc<DiscoveredInstance>)],
        action: Action,
        options: &crate::native::Options,
        manifest: &hirsel_core_contract::Manifest,
        operations: &mut hirsel_layout::operation::OperationRegistry,
        source: &mut impl FnMut(
            &InstanceAddress,
            &mut hirsel_layout::operation::OperationRegistry,
        ) -> Result<(), String>,
    ) -> Option<Scope> {
        let mut publisher = self.queries.callable();
        let mut requests = self.requests.clone();
        let mut pending = originals
            .iter()
            .map(|(key, _)| key.clone())
            .collect::<Vec<_>>();
        let mut candidates = BTreeMap::new();
        let mut at = 0;
        while at < pending.len() {
            let key = pending[at].clone();
            at += 1;
            if candidates.contains_key(key.instance.encoding()) {
                continue;
            }
            if candidates.len() >= originals.len() + hirsel_rc::closure_calls::MAX_SPECIALIZATIONS {
                return None;
            }
            let product = publisher.candidate(&key)?;
            if product.product.action != action {
                return None;
            }
            let discovery = self.descriptor(&key, &product).ok()?;
            for request in &discovery.requests {
                let dependency = self.address(request.key.bind().ok()?);
                if let Some(previous) =
                    requests.insert(request.key.encoding.clone(), request.clone())
                    && previous != *request
                {
                    return None;
                }
                pending.push(dependency);
            }
            candidates.insert(key.instance.encoding().to_vec(), (key, product, discovery));
        }
        for (key, _, _) in candidates.values() {
            source(key, operations).ok()?;
        }
        let mut outputs = Vec::new();
        for (key, candidate, discovery) in candidates.into_values() {
            let request = requests.get(key.instance.encoding())?;
            let (_, original) = originals.iter().find(|(original, _)| {
                let input = &self.requests[original.instance.encoding()];
                input.recipe == request.recipe
                    && input.lexical == request.lexical
                    && input.receipt_roots == request.receipt_roots
                    && input.layouts == request.layouts
                    && input.plan == request.plan
                    && original.instance.definition() == key.instance.definition()
                    && original.instance.body() == key.instance.body()
            })?;
            let selected_operations = super::super::instance_operations::InstanceOperations {
                inventory: Some(&self.seal_inventory),
                declarations: operations,
                receipts: Some(&self.receipt_sources),
                source: request
                    .source_definition()
                    .map(|_| (key.instance.definition(), key.instance.digest())),
                all_sources: false,
                pretransfer: Some(
                    original
                        .adapters
                        .iter()
                        .map(|symbol| {
                            let mut selected = operations
                                .pretransfer_adapters
                                .iter()
                                .filter(|r| &r.adapter == symbol);
                            let reference = selected.next()?;
                            selected.next().is_none().then(|| reference.clone())
                        })
                        .collect::<Option<Vec<_>>>()?,
                ),
            };
            let product = publisher.lookup(
                &key,
                action,
                &NativeEnvironment {
                    target: &self.target,
                    manifest,
                    operations: &selected_operations,
                    discharge: Some(&options.discharge),
                },
            )?;
            if product.product != candidate.product {
                return None;
            }
            let dependencies = discovery
                .requests
                .iter()
                .map(|r| Some(self.address(r.key.bind().ok()?)))
                .collect::<Option<Vec<_>>>()?;
            outputs.push((
                key,
                Arc::new(DiscoveredInstance {
                    product,
                    ready_source: ReadySourceFacts::decode(&discovery.ready_source).ok()?,
                    seal_demand: discovery.seal_demand,
                    callees: discovery.callees,
                    dependencies,
                    selection_reads: discovery.selection_reads,
                    constant_reads: discovery.constant_reads,
                    templates: discovery.templates,
                    operation_freedom: discovery.operation_freedom,
                    emission: discovery.emission,
                    discharge: discovery.discharge,
                    adapters: original.adapters.clone(),
                }),
            ));
        }
        outputs.sort_by(|a, b| a.0.canonical_cmp(&b.0));
        Some(Scope {
            outputs,
            operations: operations.clone(),
            requests,
        })
    }
    pub(in crate::native::queries) fn use_callable_scope(&mut self, scope: &Scope) {
        self.queries = self.queries.callable();
        self.requests = scope.requests.clone();
        self.completed = scope
            .outputs
            .iter()
            .map(|(key, output)| (key.instance.encoding().to_vec(), output.clone()))
            .collect();
    }
    pub(in crate::native::queries) fn specialize_callables(
        &mut self,
        options: &crate::native::Options,
        manifest: &hirsel_core_contract::Manifest,
        mut operations: hirsel_layout::operation::OperationRegistry,
        reuse: bool,
        mut source: impl FnMut(
            &InstanceAddress,
            &mut hirsel_layout::operation::OperationRegistry,
        ) -> Result<(), String>,
    ) -> Result<Scope, String> {
        let mut originals = self.completed_inputs()?;
        originals.sort_by(|a, b| a.0.canonical_cmp(&b.0));
        let action_bytes = serde_json::to_vec(&(
            crate::COMPILER_IDENTITY.as_slice(),
            originals
                .iter()
                .map(|(_, o)| o.product.product.record)
                .collect::<Vec<_>>(),
            options.scoped_specialization,
        ))
        .map_err(|e| e.to_string())?;
        let action = Action(ContentHash::of_parts(
            "hirsel.native.callable-values-action.v1",
            [action_bytes.as_slice()],
        ));
        if reuse
            && self.verify_store != hirsel_engine::store::VerifyStore::Recompute
            && options.validation != crate::native::Validation::EveryStage
            && let Some(scope) = self.cached_callable_scope(
                &originals,
                action,
                options,
                manifest,
                &mut operations,
                &mut source,
            )
        {
            return Ok(scope);
        }
        let environment = NativeEnvironment {
            target: &self.target,
            manifest,
            operations: &operations,
            discharge: Some(&options.discharge),
        };
        let captured = originals
            .iter()
            .map(|(key, output)| {
                let scoped = self.operations(key, &environment)?;
                self.queries.read(
                    key,
                    &output.product,
                    &NativeEnvironment {
                        operations: &scoped,
                        ..environment
                    },
                )
            })
            .collect::<Result<Vec<_>, _>>()?;
        let binding = Binding::new(
            &self.target,
            &captured
                .iter()
                .map(|(closure, _)| closure)
                .collect::<Vec<_>>(),
        )?;
        let bound_pretransfer = super::super::instance_operations::bind_pretransfer(
            &operations,
            &binding.table,
            &self.target,
        )?;
        let mut units = captured
            .iter()
            .map(|(_, body)| binding.materialize::<Value>(body))
            .collect::<Result<Vec<_>, _>>()?;
        let original_keys = originals
            .iter()
            .map(|(key, _)| key.instance.clone())
            .collect::<Vec<_>>();
        let specialized =
            hirsel_rc::closure_calls::specialize_units(&mut units, &binding.table, &original_keys);
        let keys = specialized
            .keys
            .iter()
            .map(|instance| InstanceAddress {
                instance: instance.clone(),
                target_abi: self.target.digest(),
                policy: self.policy,
            })
            .collect::<Vec<_>>();
        for key in &keys {
            source(key, &mut operations)?;
        }
        let mut requests = BTreeMap::new();
        for (key, origin) in keys.iter().zip(&specialized.origins) {
            let original = &originals[origin.index()].0;
            let mut request = self.requests[original.instance.encoding()].clone();
            if request.key.definition != *key.instance.definition()
                || request.key.body != key.instance.body()
                || request.key.encoding != key.instance.encoding()
            {
                Arc::make_mut(&mut request.0).key = NamedKey::of(&key.instance);
            }
            requests.insert(key.instance.encoding().to_vec(), request);
        }
        let index = keys
            .iter()
            .map(|key| {
                (
                    (key.instance.definition().clone(), key.instance.digest()),
                    key,
                )
            })
            .collect::<BTreeMap<_, _>>();
        let queries = self.queries.callable();
        let mut publisher = queries;
        let mut outputs = Vec::new();
        for ((unit, key), origin) in units.into_iter().zip(&keys).zip(&specialized.origins) {
            let original = &originals[origin.index()].1;
            let callees = unit
                .tables
                .calls
                .iter()
                .filter_map(|call| match call.callee {
                    Callee::Direct(s) | Callee::Bound { code: s, .. } => {
                        let symbol = &unit.tables.symbols[s.index()];
                        symbol.instance.map(|id| (symbol.definition.clone(), id))
                    }
                    _ => None,
                })
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect::<Vec<_>>();
            let finalizers = original
                .dependencies
                .iter()
                .map(|k| (k.instance.definition().clone(), k.instance.digest()));
            let templates = super::super::optimizer::TemplateFacts::of(&unit, finalizers);
            let mut dependencies = templates
                .references
                .iter()
                .filter_map(|name| index.get(name).map(|key| (*key).clone()))
                .collect::<Vec<_>>();
            dependencies.sort_by(InstanceAddress::canonical_cmp);
            dependencies.dedup();
            let dependency_requests = dependencies
                .iter()
                .map(|k| requests[k.instance.encoding()].clone())
                .collect::<Vec<_>>();
            let seal_demand =
                hirsel_rc::seal_demand::equations::LocalDemandEquations::extract(&unit)?;
            let freedom =
                hirsel_rc::closure_calls::OperationFreedom::extract(&unit, &binding.table);
            let emission = super::super::emission::EmissionFacts::extract(
                &unit,
                specialized.inline.iter().copied(),
            );
            let discovery = self.put(
                INSTANCE_DISCOVERY_DOMAIN,
                &serde_json::to_vec(&Discovery {
                    schema: INSTANCE_DISCOVERY_DOMAIN.schema,
                    owner: NamedKey::of(&key.instance),
                    ready_source: original.ready_source.encode()?,
                    seal_demand: seal_demand.clone(),
                    callees: callees.clone(),
                    requests: dependency_requests,
                    selection_reads: original.selection_reads.clone(),
                    constant_reads: original.constant_reads.clone(),
                    templates: templates.clone(),
                    operation_freedom: freedom.clone(),
                    emission: emission.clone(),
                    discharge: original.discharge.clone(),
                })
                .map_err(|e| e.to_string())?,
            )?;
            let (_, finalizers): (
                super::super::local::StaticRows,
                super::super::local::FinalizerRows,
            ) = serde_json::from_slice(
                &self
                    .store
                    .get_content(original.product.statics_finalizers)
                    .ok_or("missing callable static/finalizer input")?,
            )
            .map_err(|e| e.to_string())?;
            let mut additional_abis = finalizers
                .iter()
                .map(|row| binding.abi(row.3).ok_or("missing callable finalizer ABI"))
                .collect::<Result<Vec<_>, _>>()?;
            additional_abis.extend(bound_pretransfer.iter().map(|a| a.abi));
            let selected_operations = super::super::instance_operations::InstanceOperations {
                inventory: Some(&self.seal_inventory),
                declarations: &operations,
                receipts: Some(&self.receipt_sources),
                source: requests[key.instance.encoding()]
                    .source_definition()
                    .map(|_| (key.instance.definition(), key.instance.digest())),
                all_sources: false,
                pretransfer: Some(
                    original
                        .adapters
                        .iter()
                        .map(|symbol| {
                            let mut selected = bound_pretransfer
                                .iter()
                                .filter(|reference| &reference.adapter == symbol);
                            let reference = selected.next().ok_or(
                                "callable publication lost an independently planned adapter",
                            )?;
                            if selected.next().is_some() {
                                return Err(
                                    "callable publication repeats an independently planned adapter",
                                );
                            }
                            Ok(reference.clone())
                        })
                        .collect::<Result<Vec<_>, _>>()?,
                ),
            };
            let inputs = Inputs {
                memo: Default::default(),
                target: &self.target,
                layouts: &binding.table,
                operations: &selected_operations,
                manifest,
                discharge: Some(&options.discharge),
            };
            let (_, product) = publisher.publish(
                key,
                action,
                unit,
                &inputs,
                &additional_abis,
                discovery,
                original.product.statics_finalizers,
            )?;
            outputs.push((
                key.clone(),
                Arc::new(DiscoveredInstance {
                    product,
                    ready_source: original.ready_source.clone(),
                    seal_demand,
                    callees,
                    dependencies,
                    selection_reads: original.selection_reads.clone(),
                    constant_reads: original.constant_reads.clone(),
                    templates,
                    operation_freedom: freedom,
                    emission,
                    discharge: original.discharge.clone(),
                    adapters: original.adapters.clone(),
                }),
            ));
        }
        outputs.sort_by(|a, b| a.0.canonical_cmp(&b.0));
        Ok(Scope {
            outputs,
            operations,
            requests,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn callable_requests_share_admitted_backing_and_specialization_keeps_lexical_identity() {
        let types = hirsel_types::TypeInterner::new(None);
        let path = hirsel_syntax::DefPath::core("request");
        let key = InstanceKey::new(&path, 0, &[], &types, None, None).unwrap();
        let reference = SOURCE_RECIPE.reference(b"request").unwrap();
        let original = Request(Arc::new(RequestData {
            key: NamedKey::of(&key),
            lexical: NamedKey::of(&key),
            recipe: reference,
            receipt_roots: None,
            layouts: None,
            plan: reference,
        }));
        let portable = serde_json::to_vec(&*original.0).unwrap();
        assert_eq!(serde_json::to_vec(&original).unwrap(), portable);
        let restored: Request = serde_json::from_slice(&portable).unwrap();
        assert_eq!(restored, original);
        assert!(!Arc::ptr_eq(&restored.0, &original.0));
        let mut scope = Scope {
            outputs: Vec::new(),
            operations: hirsel_layout::operation::OperationRegistry {
                seal_sources: BTreeMap::new(),
                receipt_sources: BTreeMap::new(),
                pretransfer_adapters: Vec::new(),
                pretransfer_actuals: Vec::new(),
                invocation_bodies: Vec::new(),
                invocations: BTreeMap::new(),
                types,
                signatures: BTreeMap::new(),
                type_declarations: BTreeMap::new(),
                owners: Default::default(),
            },
            requests: BTreeMap::from([(key.encoding().to_vec(), original.clone())]),
        };
        assert!(Arc::ptr_eq(&scope.requests[key.encoding()].0, &original.0));
        let separate = scope.retained_bytes(std::iter::empty());
        let shared = scope.retained_bytes([&original]);
        assert_eq!(separate - shared, original.retained_metadata_bytes());
        let request = scope.requests.get_mut(key.encoding()).unwrap();
        Arc::make_mut(&mut request.0).key.body = 1;
        assert_eq!(original.key.body, 0);
        assert_eq!(request.lexical, original.lexical);
        assert!(!Arc::ptr_eq(&request.0, &original.0));
        assert_eq!(scope.retained_bytes([&original]), separate);
        drop(original);
        assert_eq!(scope.requests[key.encoding()].lexical.body, 0);
    }
}

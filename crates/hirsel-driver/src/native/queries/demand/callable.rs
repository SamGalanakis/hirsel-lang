//! The existing callable fixed point over explicitly read, C-admitted Values.
use super::*;
use hirsel_lir::{ir::Callee, products::Binding, stage::Value};

pub(in crate::native::queries) struct Scope {
    pub outputs: Vec<(InstanceAddress, Arc<DiscoveredInstance>)>,
    pub operations: hirsel_layout::operation::OperationRegistry,
    requests: BTreeMap<Vec<u8>, Request>,
}
impl Scope {
    pub(in crate::native::queries) fn retained_bytes(&mut self) -> u64 {
        super::super::ownership_compilation::metadata::operations(&mut self.operations)
            + self
                .outputs
                .iter()
                .map(|(key, output)| {
                    key.instance.retained_encoding_bytes() as u64
                        + output.retained_metadata_bytes()
                        + 512
                })
                .sum::<u64>()
            + self
                .requests
                .iter()
                .map(|(key, r)| key.capacity() as u64 + r.retained_metadata_bytes() + 512)
                .sum::<u64>()
    }
}
impl DemandInstances {
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
        mut source: impl FnMut(
            &InstanceAddress,
            &mut hirsel_layout::operation::OperationRegistry,
        ) -> Result<(), String>,
    ) -> Result<Scope, String> {
        let mut originals = self.completed_inputs()?;
        originals.sort_by(|a, b| a.0.canonical_cmp(&b.0));
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
        operations.pretransfer_adapters = super::super::instance_operations::bind_pretransfer(
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
            request.key = NamedKey::of(&key.instance);
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
            additional_abis.extend(operations.pretransfer_adapters.iter().map(|a| a.abi));
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
                            let mut selected = operations
                                .pretransfer_adapters
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

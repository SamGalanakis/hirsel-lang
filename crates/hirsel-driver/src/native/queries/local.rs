//! Descriptor-first local Ready production over admitted demand instances.
//! Graph discovery and SCC settlement complete separately; this producer never
//! imports dependencies, restores another Value or retains a program table.
use super::{
    demand::DemandInstances,
    graph::SettledArenaGraph,
    instance::STATICS_FINALIZERS_DOMAIN,
    ready::{EMISSION_CONTEXT_DOMAIN, ReadyMiss, ReadyQueries},
    *,
};
use hirsel_engine::store::{Action, ResultStore};
use hirsel_lir::{
    products::{Binding, LayoutClosure, UnitBody},
    stage::Value,
    validate::{Inputs, NativeEnvironment},
};
use hirsel_rc::arena_credit::equations::Function;
use std::collections::BTreeMap;

pub(super) type StaticRows = Vec<(Vec<ContentHash>, String)>;
pub(super) type FinalizerRows = Vec<(
    ContentHash,
    hirsel_syntax::DefPath,
    ContentHash,
    ContentHash,
    bool,
)>;

/// Ordinary emission inputs for one instance. The request's immutable profile
/// and the selected unit's static bytes are distinct explicit dependencies.
#[derive(serde::Serialize, serde::Deserialize)]
pub struct UnitEmissionContext {
    pub request: Reference,
    pub statics_finalizers: Reference,
}

fn function(key: &InstanceAddress) -> Function {
    (key.instance.definition().clone(), key.instance.digest())
}

fn bind_statics(
    statics: StaticRows,
    binding: &Binding,
) -> Result<Vec<hirsel_lir::statics::StaticData>, String> {
    statics
        .into_iter()
        .map(|(layouts, text)| {
            let mut data: hirsel_lir::statics::StaticData =
                hirsel_codec::from_text(&text).map_err(|e| format!("local static: {e:?}"))?;
            if data.blocks.len() != layouts.len() {
                return Err("local static layout count differs".into());
            }
            for (i, (block, layout)) in data.blocks.iter_mut().zip(layouts).enumerate() {
                if block.layout.0 as usize != i {
                    return Err("noncanonical local static layout index".into());
                }
                block.layout = binding
                    .layout(layout)
                    .ok_or("missing local static layout")?;
            }
            Ok(data)
        })
        .collect()
}

pub(super) fn read_statics(
    store: &dyn ResultStore,
    reference: Reference,
    binding: &Binding,
) -> Result<Vec<hirsel_lir::statics::StaticData>, String> {
    if reference.domain != STATICS_FINALIZERS_DOMAIN {
        return Err("wrong static input domain".into());
    }
    let bytes = store
        .get_content(reference)
        .ok_or("missing local statics")?;
    let (statics, _): (StaticRows, FinalizerRows) =
        serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
    bind_statics(statics, binding)
}

/// One selected request's local Ready provider. Completed base instances and
/// summaries are supplied by the dependency scheduler, never executed here.
pub struct LocalReadyQueries {
    store: Arc<dyn ResultStore>,
    pub queries: ReadyQueries,
}
impl LocalReadyQueries {
    pub fn new(store: Arc<dyn ResultStore>) -> Self {
        Self {
            queries: ReadyQueries::new(store.clone()),
            store,
        }
    }
    pub(super) fn with_validator(
        store: Arc<dyn ResultStore>,
        native: Arc<hirsel_lir::validate::NativeCache>,
    ) -> Self {
        Self {
            queries: ReadyQueries::with_validator(store.clone(), native),
            store,
        }
    }

    fn decode<T: hirsel_codec::Snapshot>(&self, reference: Reference) -> Result<T, String> {
        let bytes = self
            .store
            .get_content(reference)
            .ok_or("missing native dependency content")?;
        hirsel_codec::from_text(std::str::from_utf8(&bytes).map_err(|e| e.to_string())?)
            .map_err(|e| format!("native dependency content: {e:?}"))
    }

    /// All callees are already admitted in `instances`, and `arena` contains
    /// settled ordinary outputs of both strata. Only positive settled arena
    /// claims rewrite call ABIs. Actual local C and final cross-unit linkage
    /// remain independent of these producer summaries.
    #[allow(clippy::too_many_arguments)]
    pub fn ready(
        &mut self,
        instances: &DemandInstances,
        key: &InstanceAddress,
        arena: &SettledArenaGraph,
        options: &crate::native::Options,
        emission_context: Reference,
        debug: bool,
        environment: &NativeEnvironment<'_>,
    ) -> Result<Arc<ReadyProduct>, String> {
        if options.target.digest() != key.target_abi
            || environment.target.digest() != key.target_abi
            || emission_context.domain != EMISSION_CONTEXT_DOMAIN
            || self.store.get_content(emission_context).is_none()
        {
            return Err("local Ready request environment differs".into());
        }
        let (owner, _) = instances.admitted(key, environment)?;
        let context = UnitEmissionContext {
            request: emission_context,
            statics_finalizers: owner.product.statics_finalizers,
        };
        let context_bytes = serde_json::to_vec(&context).map_err(|e| e.to_string())?;
        let emission_context = self
            .store
            .put_content(EMISSION_CONTEXT_DOMAIN, &context_bytes)
            .ok_or("local emission context publication failed")?;
        if EMISSION_CONTEXT_DOMAIN.reference(&context_bytes) != Some(emission_context) {
            return Err("local emission context identity differs".into());
        }
        let mut chosen = BTreeMap::new();
        let selected_summaries = arena.chosen_reads(key, &owner.callees)?;
        for summary in &selected_summaries {
            let product = summary.product;
            let association = self
                .store
                .lookup_product(product.address)
                .ok_or("missing settled arena association")?;
            let bytes = self
                .store
                .get_content(product.record)
                .ok_or("missing settled arena result")?;
            let rows: Vec<(Vec<u8>, Vec<u32>)> =
                serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
            let expected = summary
                .candidates
                .iter()
                .map(|(a, xs)| (a.instance.encoding().to_vec(), xs.clone()))
                .collect::<Vec<_>>();
            if product.record.domain.kind != ARENA_CHOSEN_KIND
                || product.record.domain.schema != 1
                || association.action != product.action
                || association.reference != product.record
                || rows != expected
            {
                return Err("arena result differs from its settled descriptor".into());
            }
            for (address, candidates) in &summary.candidates {
                if address.target_abi != key.target_abi
                    || address.policy != key.policy
                    || candidates.len() > 1
                    || chosen
                        .insert(
                            function(address),
                            (address, candidates.first().copied(), product.record),
                        )
                        .is_some()
                {
                    return Err("duplicate or invalid chosen arena member".into());
                }
            }
        }
        let own = chosen
            .get(&function(key))
            .ok_or("missing owner arena summary")?;
        if own.0 != key {
            return Err("arena owner address differs".into());
        }
        let mut callees = Vec::new();
        for callee in &owner.callees {
            let (address, claim, summary) =
                chosen.get(callee).ok_or("missing callee arena summary")?;
            let (instance, linkage) = instances.admitted(address, environment)?;
            callees.push((callee.clone(), instance, linkage.abi(), *claim, *summary));
        }
        // Producer actions are intentionally excluded: equal semantic outputs
        // stop invalidation even after a dependency genuinely recomputes.
        let callee_reads = callees
            .iter()
            .map(|(f, _, abi, claim, summary)| (f, abi, claim, summary))
            .collect::<Vec<_>>();
        let bytes = serde_json::to_vec(&(
            key.address(READY_KIND).0,
            crate::COMPILER_IDENTITY.as_slice(),
            (
                hirsel_lir::LIR_VERSION,
                hirsel_lir::LAYOUT_VERSION,
                hirsel_lir::ABI_VERSION,
            ),
            owner.product.product.record,
            own.1,
            own.2,
            callee_reads,
            owner.ready_source.encode()?,
            (options.regions, options.elide, options.reuse),
            format!("{:?}", options.validation),
            format!("{:?}", options.discharge.sites(key.instance.definition())),
            emission_context,
            debug,
        ))
        .map_err(|e| e.to_string())?;
        let action = Action(ContentHash::of_parts(
            "hirsel.native.local-ready-action.v1",
            [bytes.as_slice()],
        ));
        if let Some(hit) = self.queries.lookup(key, action, environment) {
            return Ok(hit);
        }

        // Metadata for directly consumed ABIs is bound only on a local miss.
        // The only decoded Value body below belongs to this selected owner.
        let own_closure: LayoutClosure = self.decode(owner.product.layout_closure)?;
        let callee_closures = callees
            .iter()
            .map(|(_, instance, _, _, _)| {
                self.decode::<LayoutClosure>(instance.product.layout_closure)
            })
            .collect::<Result<Vec<_>, _>>()?;
        let closures = std::iter::once(&own_closure)
            .chain(callee_closures.iter())
            .collect::<Vec<_>>();
        let mut binding = Binding::new(environment.target, &closures)?;
        let body: UnitBody = self.decode(owner.product.value)?;
        let mut value = binding.materialize::<Value>(&body)?;
        let mut callee_bindings = Vec::new();
        for (function, _, abi, claim, _) in &callees {
            let base = binding.abi(*abi).ok_or("missing callee ABI content")?;
            let mut abi = binding.table.fn_abis[base.0 as usize].clone();
            if abi.result_arena.is_some() {
                return Err("callee is not a base Value ABI".into());
            }
            abi.result_arena = *claim;
            let id = hirsel_lir::FnAbiId(binding.table.fn_abis.len() as u32);
            binding.table.fn_abis.push(abi);
            callee_bindings.push((function.clone(), id));
        }
        hirsel_rc::arena_credit::bind_local(
            &mut value,
            &mut binding.table,
            own.1,
            &callee_bindings,
        )?;
        if owner.product.statics_finalizers.domain != STATICS_FINALIZERS_DOMAIN {
            return Err("wrong local static/finalizer domain".into());
        }
        let bytes = self
            .store
            .get_content(owner.product.statics_finalizers)
            .ok_or("missing local statics/finalizers")?;
        let (statics, finalizers): (StaticRows, FinalizerRows) =
            serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
        if finalizers.windows(2).any(|w| w[0] >= w[1]) {
            return Err("noncanonical local finalizers".into());
        }
        let finalizers = hirsel_rc::cleanup::Finalizers(
            finalizers
                .into_iter()
                .map(|(layout, definition, instance, abi, library)| {
                    Ok((
                        binding.layout(layout).ok_or("missing finalizer layout")?,
                        hirsel_rc::cleanup::Finalizer {
                            origin: if library {
                                hirsel_lir::ir::UnitOrigin::Library
                            } else {
                                hirsel_lir::ir::UnitOrigin::Source
                            },
                            definition,
                            instance,
                            abi: binding.abi(abi).ok_or("missing finalizer ABI")?,
                        },
                    ))
                })
                .collect::<Result<_, String>>()?,
        );
        let statics = bind_statics(statics, &binding)?;
        let inputs = Inputs {
            memo: Default::default(),
            target: environment.target,
            layouts: &binding.table,
            operations: environment.operations,
            manifest: environment.manifest,
            discharge: environment.discharge,
        };
        let (_, product) = self.queries.publish_from_value(
            key,
            action,
            ReadyMiss {
                value,
                options,
                source: &owner.ready_source,
                finalizers: &finalizers,
                inputs: &inputs,
                additional_abis: &[],
                statics: &statics,
                emission_context,
                debug,
                optimizer_reads: vec![],
            },
        )?;
        Ok(product)
    }
}

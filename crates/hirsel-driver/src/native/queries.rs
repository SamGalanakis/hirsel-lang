//! Native query descriptors. Content references convey integrity, never proof
//! authority; materialization independently checks the native stage and inputs.
use hirsel_artifact_store::Reference;
pub use hirsel_engine::runtime::ProductRef;
use hirsel_syntax::ContentHash;
use std::sync::Arc;

/// Complete type and size actuals, selected implementation, target and policy.
/// Body revision is a dependency output, never part of the logical address.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InstanceAddress {
    pub instance: hirsel_layout::instance::InstanceKey,
    pub target_abi: ContentHash,
    pub policy: ContentHash,
}

#[derive(Clone, Debug)]
pub struct InstanceProduct {
    pub product: ProductRef,
    pub value: Reference,
    pub layout_closure: Reference,
    pub local_equations: Reference,
    pub discovery: Reference,
    pub statics_finalizers: Reference,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ArenaStratum {
    Candidates,
    Chosen,
}

/// Canonically sorted members and internal edges. Topology is an address input;
/// equations and outgoing settled summaries belong to the complete action.
#[derive(Clone, Debug)]
pub struct ComponentKey {
    pub members: Vec<InstanceAddress>,
    pub edges: Vec<(u32, u32)>,
    pub facet: u32,
    pub stratum: ArenaStratum,
}

#[derive(Clone, Debug)]
pub struct ArenaSummary {
    pub product: ProductRef,
    pub candidates: Vec<(InstanceAddress, Vec<u32>)>,
}

#[derive(Clone, Debug)]
pub struct ReadyProduct {
    pub product: ProductRef,
    /// Bound to emission_context; consumed by the backend profile selector.
    pub debug: bool,
    pub ready: Reference,
    pub layout_closure: Reference,
    pub emission_context: Reference,
    pub unit_summary: Reference,
    pub optimizer_reads: Vec<ProductRef>,
}

/// Statically dispatched access; a completed read may not block a worker on a
/// dependent worker. The admitted runtime owns dependency continuations.
pub trait NativeProducts {
    fn instance(&mut self, key: &InstanceAddress) -> Result<Arc<InstanceProduct>, String>;
    fn arena_summary(&mut self, key: &ComponentKey) -> Result<Arc<ArenaSummary>, String>;
    fn ready(&mut self, key: &InstanceAddress) -> Result<Arc<ReadyProduct>, String>;

    /// Selected owner followed by backend-specific copied bodies. The caller
    /// must have settled every read before constructing a restricted view.
    /// Selection observes compact metadata only; bodies open on a Codegen miss.
    fn codegen_units(
        &mut self,
        key: &InstanceAddress,
        _backend: &str,
    ) -> Result<Vec<Arc<ReadyProduct>>, String> {
        Ok(vec![self.ready(key)?])
    }

    /// Units are borrowed only while their reached layout closure is bound.
    /// Supply requested owners first, followed by their complete transitive
    /// optimizer reads in canonical symbol order. Debug has no optimizer reads.
    fn with_ready<R>(
        &mut self,
        products: &[Arc<ReadyProduct>],
        use_units: impl FnOnce(
            &[&hirsel_lir::stage::ValidatedUnit<hirsel_lir::stage::Ready>],
            &hirsel_lir::emission::EmitInputs<'_>,
        ) -> Result<R, String>,
    ) -> Result<R, String>;
}

pub mod declarations;
pub mod source;

pub mod recipes;

pub mod source_inputs;

/// Resolve settled descriptor bytes only. This is not a proof capability: the
/// admitted Ready producer owns independent validation before settling the read.
pub fn ready_product(
    store: &dyn hirsel_engine::store::ResultStore,
    product: ProductRef,
) -> Result<ReadyProduct, String> {
    if product.record.domain != READY_RECORD_DOMAIN {
        return Err("wrong Ready descriptor domain".into());
    }
    let current = store
        .lookup_product(product.address)
        .ok_or("missing Ready association")?;
    if current.action != product.action || current.reference != product.record {
        return Err("Ready descriptor is no longer settled at this action".into());
    }
    let bytes = store
        .get_content(product.record)
        .ok_or("missing Ready descriptor")?;
    let (debug, ready, layout_closure, emission_context, unit_summary, optimizer): ReadyRecord =
        serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
    let mut optimizer_reads = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    for (address, record) in optimizer {
        if !seen.insert(address) || record.domain != READY_RECORD_DOMAIN {
            return Err("duplicate or foreign optimizer descriptor".into());
        }
        let address = hirsel_artifact_store::Address(address);
        let current = store
            .lookup_product(address)
            .ok_or("missing optimizer association")?;
        if current.reference != record {
            return Err("optimizer dependency output changed".into());
        }
        optimizer_reads.push(ProductRef {
            address,
            action: current.action,
            record,
        });
    }
    Ok(ReadyProduct {
        product,
        debug,
        ready,
        layout_closure,
        emission_context,
        unit_summary,
        optimizer_reads,
    })
}

pub const READY_RECORD_DOMAIN: hirsel_artifact_store::Domain = hirsel_artifact_store::Domain {
    kind: READY_KIND,
    schema: 1,
};
type ReadyRecord = (
    bool,
    Reference,
    Reference,
    Reference,
    Reference,
    Vec<(ContentHash, Reference)>,
);

pub mod instance;

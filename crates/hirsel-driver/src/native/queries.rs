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

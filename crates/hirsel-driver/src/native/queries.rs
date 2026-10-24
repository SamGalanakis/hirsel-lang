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

pub const READY_KIND: u32 = 0x4e00_0004;

impl InstanceAddress {
    /// Full-encoding order, independent of session ids or digest ordering.
    pub fn canonical_cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.instance
            .encoding()
            .cmp(other.instance.encoding())
            .then_with(|| self.target_abi.cmp(&other.target_abi))
            .then_with(|| self.policy.cmp(&other.policy))
    }
    pub fn ready_query_id(&self) -> hirsel_engine::runtime::QueryId {
        hirsel_engine::runtime::QueryId {
            kind: READY_KIND,
            address: self.address(READY_KIND),
        }
    }
    pub fn address(&self, kind: u32) -> hirsel_artifact_store::Address {
        hirsel_artifact_store::Address(ContentHash::of_parts(
            "hirsel.native.instance-address.v1",
            [
                kind.to_le_bytes().as_slice(),
                self.instance.encoding(),
                self.target_abi.0.as_slice(),
                self.policy.0.as_slice(),
            ],
        ))
    }
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

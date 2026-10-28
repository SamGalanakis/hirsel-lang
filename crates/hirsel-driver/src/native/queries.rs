//! Native query descriptors. Content references convey integrity, never proof
//! authority; materialization independently checks the native stage and inputs.
pub use hirsel_engine::runtime::ProductRef;
use hirsel_engine::store::Reference;
pub use hirsel_rc::arena_credit::equations::LocalArenaEquations;
use hirsel_syntax::ContentHash;
use std::sync::Arc;
pub mod arena;
pub mod discharge;
pub mod execution;
pub mod graph;
pub mod seal_demand;

pub const INSTANCE_KIND: u32 = 0x4e00_0001;
pub const ARENA_CANDIDATES_KIND: u32 = 0x4e00_0002;
pub const ARENA_CHOSEN_KIND: u32 = 0x4e00_0003;
pub const READY_KIND: u32 = 0x4e00_0004;
pub const ROOT_KIND: u32 = 0x4e00_0005;
pub(super) fn instance_query(
    key: &InstanceAddress,
    product: ProductRef,
) -> hirsel_engine::runtime::QueryId {
    hirsel_engine::runtime::QueryId {
        kind: if product.address == key.address(instance::CALLABLE_VALUE_KIND) {
            instance::CALLABLE_VALUE_KIND
        } else {
            INSTANCE_KIND
        },
        address: product.address,
    }
}

/// Source root selection is independent of the discovered native instance
/// closure. Source GEN-5 still demands its complete required root set.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeRootKey {
    pub roots: Vec<hirsel_syntax::DefPath>,
    pub target_abi: ContentHash,
    pub policy: ContentHash,
}

impl NativeRootKey {
    pub fn query_id(&self) -> Result<hirsel_engine::runtime::QueryId, String> {
        if self.roots.windows(2).any(|w| w[0] >= w[1]) {
            return Err("noncanonical native root selection".into());
        }
        let selection = serde_json::to_vec(&(&self.roots, self.target_abi, self.policy))
            .map_err(|e| e.to_string())?;
        Ok(hirsel_engine::runtime::QueryId {
            kind: ROOT_KIND,
            address: hirsel_engine::store::Address(ContentHash::of_parts(
                "hirsel.native.root-address.v1",
                [selection.as_slice()],
            )),
        })
    }
}

/// Canonical root discovery and launch descriptors, available before deciding
/// a Link hit. These contain no Value or Ready bodies and grant no authority.
#[derive(Clone, Debug)]
pub struct NativeRootProduct {
    pub product: ProductRef,
    pub instances: Vec<InstanceAddress>,
    pub static_manifest: Reference,
    pub launch_manifest: Reference,
    pub statics_finalizers: Reference,
}

/// Native root completion and descriptor-only linker materialization. Link
/// context is demanded only on a Link miss, with ReadyProgram.units empty.
/// Runtime, foreign and linker option snapshots remain explicit S4 inputs.
pub trait NativeRoots: NativeProducts {
    fn root(&mut self, key: &NativeRootKey) -> Result<Arc<NativeRootProduct>, String>;
    fn with_link_context<R>(
        &mut self,
        root: &Arc<NativeRootProduct>,
        products: &[Arc<ReadyProduct>],
        use_context: impl FnOnce(&super::ReadyProgram) -> Result<R, String>,
    ) -> Result<R, String>;
}

/// Complete type and size actuals, selected implementation, target and policy.
/// Body revision is a dependency output, never part of the logical address.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InstanceAddress {
    pub instance: hirsel_layout::instance::InstanceKey,
    pub target_abi: ContentHash,
    pub policy: ContentHash,
}

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
    pub fn address(&self, kind: u32) -> hirsel_engine::store::Address {
        hirsel_engine::store::Address(ContentHash::of_parts(
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

impl ComponentKey {
    pub fn address(&self) -> Result<hirsel_engine::store::Address, String> {
        if self.members.is_empty()
            || self
                .members
                .windows(2)
                .any(|w| !w[0].canonical_cmp(&w[1]).is_lt())
            || self.edges.windows(2).any(|w| w[0] >= w[1])
            || self
                .edges
                .iter()
                .any(|&(a, b)| a as usize >= self.members.len() || b as usize >= self.members.len())
        {
            return Err("noncanonical native arena component address".into());
        }
        let kind = match self.stratum {
            ArenaStratum::Candidates => ARENA_CANDIDATES_KIND,
            ArenaStratum::Chosen => ARENA_CHOSEN_KIND,
        };
        let mut parts = vec![
            kind.to_le_bytes().to_vec(),
            self.facet.to_le_bytes().to_vec(),
            (self.members.len() as u64).to_le_bytes().to_vec(),
        ];
        for m in &self.members {
            parts.push(m.instance.encoding().to_vec());
            parts.push(m.target_abi.0.to_vec());
            parts.push(m.policy.0.to_vec());
        }
        parts.push((self.edges.len() as u64).to_le_bytes().to_vec());
        for &(a, b) in &self.edges {
            parts.push(a.to_le_bytes().to_vec());
            parts.push(b.to_le_bytes().to_vec());
        }
        Ok(hirsel_engine::store::Address(ContentHash::of_parts(
            "hirsel.native.component-address.v1",
            parts.iter().map(Vec::as_slice),
        )))
    }
}

/// The key is metadata, never permission to infer a Value or its equations.
pub fn extract_local_arena(
    key: &InstanceAddress,
    value: &hirsel_lir::stage::ValidatedUnit<hirsel_lir::stage::Value>,
    layouts: &hirsel_lir::layout_types::LayoutTable,
) -> Result<LocalArenaEquations, String> {
    let header = &value.unit().header;
    if header.definition != *key.instance.definition()
        || header.instance != key.instance.digest()
        || header.target != key.target_abi
        || layouts.target != key.target_abi
    {
        return Err("arena extraction key differs from checked Value".into());
    }
    Ok(hirsel_rc::arena_credit::equations::extract_local_arena(
        value, layouts,
    ))
}

/// Complete topology and own-stratum validation around the pure RC solver.
pub fn solve_arena_component(
    key: &ComponentKey,
    members: &[(InstanceAddress, Arc<LocalArenaEquations>)],
    own_candidates: Option<&ArenaSummary>,
    outgoing: &[(InstanceAddress, Vec<u32>)],
) -> Result<Vec<(InstanceAddress, Vec<u32>)>, String> {
    key.address()?;
    let function = |a: &InstanceAddress| (a.instance.definition().clone(), a.instance.digest());
    if members.len() != key.members.len()
        || members
            .iter()
            .zip(&key.members)
            .any(|((address, equations), expected)| {
                address != expected || equations.owner() != &function(address)
            })
        || outgoing
            .windows(2)
            .any(|w| !w[0].0.canonical_cmp(&w[1].0).is_lt())
    {
        return Err("arena members or outgoing order differ from component address".into());
    }
    let mut topology = Vec::new();
    for (i, (_, local)) in members.iter().enumerate() {
        for callee in local.callees() {
            if let Some(j) = members
                .iter()
                .position(|(address, _)| &function(address) == callee)
            {
                topology.push((i as u32, j as u32));
            }
        }
    }
    topology.sort();
    topology.dedup();
    if topology != key.edges {
        return Err("arena component edges differ from local equations".into());
    }
    match (key.stratum, own_candidates) {
        (ArenaStratum::Candidates, None) => {}
        (ArenaStratum::Chosen, Some(own)) => {
            let mut candidates_key = key.clone();
            candidates_key.stratum = ArenaStratum::Candidates;
            if own.product.address != candidates_key.address()?
                || own.candidates.len() != key.members.len()
                || own
                    .candidates
                    .iter()
                    .zip(&key.members)
                    .any(|((a, _), b)| a != b)
            {
                return Err("Chosen lacks this exact settled Candidates component".into());
            }
        }
        _ => return Err("invalid arena stratum dependency".into()),
    }
    let mut locals: Vec<_> = members.iter().map(|(_, e)| e.as_ref()).collect();
    locals.sort_by(|a, b| a.owner().cmp(b.owner()));
    let mut own: Vec<_> = own_candidates.map_or_else(Vec::new, |own| {
        own.candidates
            .iter()
            .map(|(a, xs)| (function(a), xs.clone()))
            .collect()
    });
    own.sort_by(|a, b| a.0.cmp(&b.0));
    let mut external: Vec<_> = outgoing
        .iter()
        .map(|(a, xs)| (function(a), xs.clone()))
        .collect();
    external.sort_by(|a, b| a.0.cmp(&b.0));
    let solved: std::collections::BTreeMap<_, _> =
        hirsel_rc::arena_credit::equations::solve_component(
            &locals,
            own_candidates.map(|_| own.as_slice()),
            &external,
        )?
        .into_iter()
        .collect();
    Ok(key
        .members
        .iter()
        .map(|a| (a.clone(), solved[&function(a)].clone()))
        .collect())
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
    fn execution_read(&self) -> Option<ProductRef> {
        None
    }
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
/// Select once for an admitted native request and share this same provider
/// with native, object and runtime producers. Cache write failures retain only
/// bounded transient transport; verifier capabilities come from the selected
/// backend exactly. The admitted allowance outlives every external byte view.
pub fn request_store(
    engine: &hirsel_engine::QueryEngine,
    allowance: hirsel_engine::runtime::MemoryReservation,
) -> Arc<hirsel_engine::store::RequestStore> {
    let backend = engine
        .product_store()
        .unwrap_or_else(|| Arc::new(hirsel_engine::store::NoneStore));
    Arc::new(hirsel_engine::store::RequestStore::new(backend, allowance))
}

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
        let address = hirsel_engine::store::Address(address);
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

pub const READY_RECORD_DOMAIN: hirsel_engine::store::Domain = hirsel_engine::store::Domain {
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
pub mod arena_tasks;
pub mod compilation_tasks;
pub mod demand;
pub mod emission;
pub mod instance;
pub mod instance_tasks;
pub mod link_tasks;
pub mod local;
pub mod optimizer;
mod ownership_closure;
pub mod ownership_compilation;
pub mod ownership_tasks;
pub mod products;
pub mod ready;
pub mod ready_source;
pub mod root_tasks;
pub mod roots;
pub mod source_inputs;
pub mod tasks;

/// Ordinary descriptor publication, after the native query has obtained its
/// own authority. It does not manufacture any native receipt or stage seal.
pub(crate) fn publish_ready_record(
    store: &dyn hirsel_engine::store::ResultStore,
    address: hirsel_engine::store::Address,
    action: hirsel_engine::store::Action,
    value: &ReadyRecord,
) -> Result<ProductRef, String> {
    let bytes = serde_json::to_vec(value).map_err(|e| e.to_string())?;
    let expected = READY_RECORD_DOMAIN
        .reference(&bytes)
        .ok_or("Ready descriptor exceeds record limit")?;
    let record = store
        .put_content(READY_RECORD_DOMAIN, &bytes)
        .ok_or("Ready descriptor publication failed")?;
    if record != expected {
        return Err("Ready descriptor content identity differs".into());
    }
    store.associate_product(address, action, record);
    Ok(ProductRef {
        address,
        action,
        record,
    })
}

mod instance_operations;

pub(super) mod batch_sources;
mod receipt_requests;
mod seal_inventory;

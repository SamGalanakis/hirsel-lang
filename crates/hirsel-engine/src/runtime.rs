//! Shared admitted-query runtime. Identities describe immutable products; none
//! of these descriptors grants verifier or native-validator authority.
#[cfg(test)]
use hirsel_artifact_store::Action;
use hirsel_artifact_store::{Address, Reference};
use std::task::Poll;
mod budget;
mod future_task;
mod product;
mod scheduler;
pub use budget::{MemoryReservation, MemoryReservationPool, ProcessBudget};
pub use future_task::{LocalFutureTask, QueryReads};
pub use product::{
    ArchiveAdmission, LIVE_ANALYZED, LIVE_LOWER, LIVE_SUMMARY, LiveAnalyzedProduct,
    LiveLowerProduct, LiveSummaryProduct, ProductArchive, SummaryFacet,
};
pub(crate) use product::{
    ProductSource, SharedProductSource, WeakAnalyzedProduct, WeakLowerProduct,
};
pub use scheduler::{
    AdmittedRuntime, RuntimeCompletion, RuntimeErrors, RuntimeEvent, RuntimeObserver, RuntimeStats,
};

/// Logical address, independent of source revision, worker or request order.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct QueryId {
    pub kind: u32,
    pub address: Address,
}

/// Session-local request token, never a durable query identity.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RequestId(pub u64);

mod published;
pub use published::ProductRef;

#[cfg(test)]
pub(crate) fn test_product(
    address: Address,
    action: Action,
    domain: hirsel_artifact_store::Domain,
    bytes: &[u8],
) -> ProductRef {
    use crate::store::ResultStore;
    let store = crate::store::MemStore::default();
    let reference = store.put_content(domain, bytes).unwrap();
    let budget = ProcessBudget::new((bytes.len() as u64).saturating_add(4096));
    let mut allowance = budget.reserve(budget.limit()).unwrap();
    let archive = ProductArchive::open(&store, reference, &mut allowance).unwrap();
    ProductRef::import_archive(address, action, archive)
}

/// Upper bounds charged before polling. Retained includes continuation state,
/// held products and mapped bytes. Suspension releases scratch and the worker;
/// pool stacks remain charged once and are reused by ready work.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct MemoryWeight {
    pub scratch: u64,
    pub retained: u64,
    pub stack: u64,
}

#[derive(Clone, Debug)]
pub enum QueryPoll {
    Waiting(Vec<QueryId>),
    Complete(ProductRef),
    /// Immutable diagnostics, including phase/source-order metadata.
    Failed(Reference),
}

pub trait QueryContext {
    /// The continuation has retained this dependency's complete observations
    /// and released its raw body. Keep the semantic edge and subscriptions;
    /// only its in-flight body pin ends, until the next read.
    fn release_body(&mut self, _query: QueryId) {}
    /// Evict the oldest settled, unpinned analyzed or Lower body outside unfinished
    /// roots and active continuations. Producers call this before retrying a
    /// refused admission.
    fn evict_body(&mut self) -> bool {
        false
    }
    /// Records an actual dependency. Pending never blocks a worker thread.
    fn read(&mut self, key: QueryId) -> Poll<Result<ProductRef, Reference>>;
}

pub trait QueryTask: Send {
    fn id(&self) -> QueryId;
    fn weight(&self) -> MemoryWeight;
    /// Part of `weight().retained` owned solely by this continuation. Retire
    /// only after its destructor runs. Shared products/mappings must retain
    /// their independent reservation or remain in the non-continuation charge.
    fn continuation_bytes(&self) -> u64 {
        0
    }
    /// Retain continuation state on Pending; no provisional publication.
    fn poll(&mut self, cx: &mut dyn QueryContext) -> QueryPoll;
}

/// Installed by the engine for one immutable input snapshot, never by clients.
pub trait QueryFactory: Send + Sync {
    /// Constructs a continuation without executing its dependencies.
    fn create(&self, key: QueryId) -> Result<Box<dyn QueryTask>, Reference>;
}

/// A non-Send continuation, created, resumed and destroyed on its home worker.
/// Its wait releases execution admission; ordinary Send work can still use the
/// home worker. This permits the interpreter's Rc/RefCell state without an
/// unsafe Send assertion or replaying the computation after a dependency miss.
pub trait LocalQueryTask {
    fn id(&self) -> QueryId;
    fn weight(&self) -> MemoryWeight;
    /// Part of `weight().retained` owned solely by this continuation. Retire
    /// only after its destructor runs. Shared products/mappings must retain
    /// their independent reservation or remain in the non-continuation charge.
    fn continuation_bytes(&self) -> u64 {
        0
    }
    fn poll(&mut self, cx: &mut dyn QueryContext) -> QueryPoll;
}

/// Only immutable construction inputs cross threads. The task itself never does.
pub trait LocalQueryFactory: Send + Sync {
    /// Bounded metadata read; reserves the full continuation before creation.
    fn weight(&self, key: QueryId) -> Result<MemoryWeight, Reference>;
    /// Called on the home worker after admission; never eagerly runs dependencies.
    fn create(&self, key: QueryId) -> Result<Box<dyn LocalQueryTask>, Reference>;
}

pub trait QueryRuntime {
    fn request(&mut self, roots: &[QueryId]) -> RequestId;
    /// Completes every required root and legal transaction before returning,
    /// retaining root order. Producer completion alone cannot publish names.
    fn poll(&mut self, request: RequestId) -> Poll<Result<Vec<ProductRef>, Reference>>;
    fn cancel(&mut self, request: RequestId);
    /// Changes retention only, never validity or verifier authority.
    fn evict_to(&mut self, retained_bytes: u64);
}

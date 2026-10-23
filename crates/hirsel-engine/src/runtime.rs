//! Shared admitted-query runtime. Identities describe immutable products; none
//! of these descriptors grants verifier or native-validator authority.
use hirsel_artifact_store::{Action, Address, Reference};
use std::task::Poll;

/// Logical address, independent of source revision, worker or request order.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct QueryId {
    pub kind: u32,
    pub address: Address,
}

/// Session-local request token, never a durable query identity.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RequestId(pub u64);

/// Complete action and canonical output are separate from the logical address.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ProductRef {
    pub address: Address,
    pub action: Action,
    pub record: Reference,
}

/// Upper bounds charged before polling. Retained includes continuation state,
/// held products and mapped bytes; stack/scratch are released on suspension.
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
    /// Records an actual dependency. Pending never blocks a worker thread.
    fn read(&mut self, key: QueryId) -> Poll<Result<ProductRef, Reference>>;
}

pub trait QueryTask: Send {
    fn id(&self) -> QueryId;
    fn weight(&self) -> MemoryWeight;
    /// Retain continuation state on Pending; no provisional publication.
    fn poll(&mut self, cx: &mut dyn QueryContext) -> QueryPoll;
}

/// Installed by the engine for one immutable input snapshot, never by clients.
pub trait QueryFactory: Send + Sync {
    /// Constructs a continuation without executing its dependencies.
    fn create(&self, key: QueryId) -> Result<Box<dyn QueryTask>, Reference>;
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

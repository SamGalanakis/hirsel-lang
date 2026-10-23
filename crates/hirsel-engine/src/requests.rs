//! Request selection shared by fresh and resident hosts. The engine owns input
//! observation, immutable snapshot installation and root selection; a host owns
//! only the lifetime of a request. Descriptors confer no verification authority.
use crate::runtime::{ProductRef, QueryId, QueryRuntime};
use std::sync::{Arc, Mutex};

/// Observe a complete input discovery before installing its query factories.
/// Discovery must use only the supplied adapter, and must not publish roots or
/// semantic transactions. An error is captured too: missing inputs and invalid
/// manifests remain part of the snapshot that selected the error.
///
/// Uses the existing read-once loader snapshot and its content/namespace checks
/// (loader_inputs.rs, TS-Go snapshotfs.go:56-104). Rechecks detect edits racing
/// discovery, without treating mutable user files as high durability. This is
/// an observed filesystem snapshot, not an atomic filesystem transaction.
pub fn observe_inputs<T, E>(
    limits: crate::loader_inputs::InputLimits,
    attempts: usize,
    mut discover: impl FnMut(&crate::loader_inputs::CaptureInputs) -> Result<T, E>,
) -> Result<(Result<T, E>, crate::loader_inputs::FrozenInputs), InputsChanged> {
    for _ in 0..attempts {
        let capture = crate::loader_inputs::CaptureInputs::new(limits);
        let result = discover(&capture);
        let frozen = capture.freeze();
        if frozen.changed_on_disk().is_empty() {
            return Ok((result, frozen));
        }
    }
    Err(InputsChanged)
}

/// Inputs kept changing during bounded discovery; no request was admitted.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InputsChanged;

/// Finish the selected operation after its query transactions have completed.
/// A native implementation resolves the linked product and materializes/checks
/// the executable through the object provider before reporting readiness.
pub trait OutputReadiness {
    type Output;
    type Error;
    fn ready(&mut self, products: &[ProductRef]) -> Result<Self::Output, Self::Error>;
}

/// One engine-selected immutable graph. Runtime sharing is engine-owned: equal
/// requests may coalesce, while an older live request keeps its own snapshot.
/// There is no host-side semantic cache or input-revision invalidation table.
pub struct SelectedRequest<R, O> {
    pub runtime: Arc<Mutex<R>>,
    pub roots: Vec<QueryId>,
    pub output: O,
}

pub trait RequestEngine {
    type Request;
    type Error;
    type Runtime: QueryRuntime;
    type Readiness: OutputReadiness;

    /// Observe and seal the complete actual input union BEFORE selecting roots
    /// or exposing factories to queued/live consumers. Use the same immutable
    /// loader inputs as fresh execution, including missing/directory/alias
    /// observations and compiler/toolchain/policy/store identities. Mutable
    /// source bytes must be inspected; watcher metadata is not identity.
    ///
    /// Install factories inside the engine, never from the request client.
    /// Reuse a runtime only for the same immutable input/environment snapshot;
    /// changed inputs select a new runtime over the same durable store. Query
    /// addresses remain logical; complete actions validate dependency outputs.
    fn select(
        &mut self,
        request: &Self::Request,
    ) -> Result<SelectedRequest<Self::Runtime, Self::Readiness>, Self::Error>;
}

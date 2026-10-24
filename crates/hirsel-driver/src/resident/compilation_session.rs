//! A transport cursor over immutable compiler inputs. The selector owns graph
//! reuse and retained products; changing this cursor never invalidates a graph
//! or changes the inputs of a queued/live request.
use super::{Host, RequestLease};
use crate::requests::{CompileOperation, CompileRequest};
use hirsel_engine::{
    loader_inputs::FrozenInputs, requests::RequestEngine, runtime::AdmittedRuntime,
};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::Arc,
};

/// Fresh and resident callers use the same explicit request descriptor. This
/// cursor owns no loaded program, check result, memo or semantic invalidation
/// state. Old request leases retain their original immutable inputs.
pub struct CompilationSession<E>
where
    E: RequestEngine<Request = CompileRequest, Runtime = AdmittedRuntime>,
{
    host: Host<E>,
    path: PathBuf,
    inputs: FrozenInputs,
    revision: u64,
}

impl<E> CompilationSession<E>
where
    E: RequestEngine<Request = CompileRequest, Runtime = AdmittedRuntime>,
{
    pub fn new(engine: E, path: PathBuf, inputs: FrozenInputs) -> Self {
        Self {
            host: Host::new(engine),
            path,
            inputs,
            revision: 0,
        }
    }

    pub fn entry(&self) -> &Path {
        &self.path
    }

    pub fn inputs(&self) -> &FrozenInputs {
        &self.inputs
    }

    pub fn watched(&self) -> impl Iterator<Item = &Path> {
        self.inputs
            .operations()
            .map(hirsel_engine::loader_inputs::Operation::path)
    }

    /// Delivery revision, independent of query completion or worker ordering.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// Install a complete newly observed input descriptor, including namespace
    /// changes, manifests, deletion and stable loader failures. Observation
    /// belongs to the engine and must finish before this descriptor is exposed.
    pub fn replace_inputs(&mut self, inputs: FrozenInputs) -> u64 {
        if self.inputs != inputs {
            self.inputs = inputs;
            self.revision = self
                .revision
                .checked_add(1)
                .expect("input revision exhausted");
        }
        self.revision
    }

    /// Apply explicitly delivered bytes only to already observed source aliases.
    /// This reads no filesystem state. Namespace/manifest changes require a
    /// complete descriptor through replace_inputs, never a disk fallback.
    /// Failed or oversized delivery leaves the current descriptor unchanged.
    pub fn apply_sources(
        &mut self,
        replacements: &BTreeMap<PathBuf, Arc<[u8]>>,
        input_bytes: usize,
    ) -> std::io::Result<u64> {
        let inputs = self.inputs.replace_sources(replacements, input_bytes)?;
        Ok(self.replace_inputs(inputs))
    }

    pub fn request(
        &mut self,
        operation: CompileOperation,
    ) -> Result<RequestLease<AdmittedRuntime, E::Readiness>, E::Error> {
        self.host.request(&CompileRequest {
            path: self.path.clone(),
            inputs: self.inputs.clone(),
            operation,
        })
    }
}

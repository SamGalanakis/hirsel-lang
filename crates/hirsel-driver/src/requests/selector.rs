//! Explicit host policy and one process allowance for compiler request graphs.
use super::*;
use hirsel_engine::{CompileConfig, Compiler, runtime::ProcessBudget};
use std::sync::Arc;

/// Partition bounds for one admitted graph. These do not raise the process
/// allowance: every partition is reserved from the supplied shared budget.
/// A host chooses them under its existing cap, including its own fixed overhead.
#[derive(Clone, Copy, Debug)]
pub struct RequestLimits {
    pub runtime_bytes: u64,
    pub snapshot_bytes: u64,
    pub transport_bytes: u64,
}
/// Concrete selector policy shared by fresh and resident hosts. It retains
/// no LoadedProgram or checked-output cache. Selected immutable graphs own
/// their products and reservations independently of this constructor's lifetime.
pub struct CompilerRequests {
    compiler: Compiler,
    config: CompileConfig,
    budget: Arc<ProcessBudget>,
    limits: RequestLimits,
    toolchain: Option<Arc<crate::native::Toolchain>>,
}
impl CompilerRequests {
    pub fn new(
        compiler: Compiler,
        config: CompileConfig,
        budget: Arc<ProcessBudget>,
        limits: RequestLimits,
    ) -> Self {
        Self {
            compiler: compiler.with_config(config.clone()).with_owned_engines(),
            config,
            budget,
            limits,
            toolchain: None,
        }
    }
    pub fn with_toolchain(mut self, toolchain: Arc<crate::native::Toolchain>) -> Self {
        self.toolchain = Some(toolchain);
        self
    }
    pub fn compiler(&self) -> &Compiler {
        &self.compiler
    }
    pub fn config(&self) -> &CompileConfig {
        &self.config
    }
    pub fn budget(&self) -> &Arc<ProcessBudget> {
        &self.budget
    }
    pub fn limits(&self) -> RequestLimits {
        self.limits
    }
    pub fn toolchain(&self) -> Option<&Arc<crate::native::Toolchain>> {
        self.toolchain.as_ref()
    }
}

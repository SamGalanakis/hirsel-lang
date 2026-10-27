//! Explicit host policy and one process allowance for compiler request graphs.
use super::*;
use hirsel_engine::loader_inputs::InputLimits;
use hirsel_engine::{CompileConfig, Compiler, runtime::ProcessBudget};
use std::sync::{Arc, OnceLock};

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
    ledger: Arc<super::inspection::InspectionLedger>,
    pub(super) graphs: super::host_selector::GraphSelections,
    config: CompileConfig,
    budget: Arc<ProcessBudget>,
    limits: RequestLimits,
    /// Accepted front facts and Lower outcomes carried from one source
    /// revision's graph to the next.
    carry: Option<super::source_graph::Carry>,
    source_index: std::sync::Mutex<Option<hirsel_engine::SourceIndexState>>,
    scratch_parent: std::path::PathBuf,
    toolchain: Option<Arc<crate::native::Toolchain>>,
    host_toolchain: bool,
}
// Host constructors share one process allowance, including different Sessions.
// Capturing the guard policy once prevents each source revision from creating
// a new full-cap budget. Explicit constructors retain their caller's budget.
static HOST: OnceLock<(Arc<ProcessBudget>, u64)> = OnceLock::new();
impl CompilerRequests {
    /// Capture the existing host guard cap. Construction is lazy/infallible;
    /// actual graph reservations and refusal belong to request selection.
    pub fn for_host(compiler: Compiler, config: CompileConfig) -> Self {
        let (budget, cap) = HOST.get_or_init(|| {
            let cap = std::env::var("HIRSEL_MEMORY_LIMIT")
                .ok()
                .and_then(|text| hirsel_guard::parse_limit(text.as_bytes()))
                .unwrap_or(hirsel_guard::DEFAULT_LIMIT);
            // Keep a quarter for the host, source loader and native libraries.
            (ProcessBudget::new(cap.saturating_sub(cap / 4)), cap)
        });
        let limits = host_limits(*cap, config.jobs.get());
        let mut selected = Self::new(compiler, config, budget.clone(), limits);
        selected.host_toolchain = true;
        selected
    }
    pub fn input_limits(&self) -> InputLimits {
        InputLimits {
            bytes: usize::try_from(self.limits.snapshot_bytes.min(16 << 20)).unwrap_or(usize::MAX),
            operations: 4096,
        }
    }
    pub fn capture_attempts(&self) -> usize {
        3
    }

    pub fn new(
        compiler: Compiler,
        mut config: CompileConfig,
        budget: Arc<ProcessBudget>,
        limits: RequestLimits,
    ) -> Self {
        // Every provider can evict ordinary products. Compact front and Lower
        // facts therefore have the same bounded carry regardless of provider.
        // An off provider also gets a transient byte store within this bound.
        let quarter = limits.transport_bytes / 4;
        if !config.store.enabled() {
            config.store = Arc::new(super::resident_store::ResidentStore::new(
                budget.clone(),
                limits.transport_bytes - 2 * quarter,
            ));
        }
        let front = budget
            .reserve(quarter)
            .ok()
            .and_then(|allowance| hirsel_engine::FrontCarry::new(allowance).ok());
        let lower = budget
            .reserve(quarter)
            .ok()
            .and_then(|allowance| hirsel_engine::LowerCarry::new(allowance).ok());
        let carry = front
            .zip(lower)
            .map(|(front, lower)| super::source_graph::Carry { front, lower });
        let graphs = super::host_selector::GraphSelections::new(&budget);
        let ledger =
            super::inspection::InspectionLedger::new(budget.clone(), limits.snapshot_bytes / 8);
        Self {
            graphs,
            ledger,
            compiler: compiler.with_config(config.clone()).with_owned_engines(),
            config,
            budget,
            limits,
            carry,
            source_index: std::sync::Mutex::new(None),
            scratch_parent: std::env::temp_dir(),
            toolchain: None,
            host_toolchain: false,
        }
    }
    /// Physical request transport policy, captured before any source read.
    pub fn with_scratch_parent(mut self, parent: std::path::PathBuf) -> Self {
        self.scratch_parent = parent;
        self.graphs = super::host_selector::GraphSelections::new(&self.budget);
        self
    }
    pub fn with_toolchain(mut self, toolchain: Arc<crate::native::Toolchain>) -> Self {
        self.toolchain = Some(toolchain);
        self.graphs = super::host_selector::GraphSelections::new(&self.budget);
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

fn host_limits(cap: u64, jobs: usize) -> RequestLimits {
    RequestLimits {
        runtime_bytes: (jobs as u64)
            .saturating_mul(hirsel_intern::jobs::COMPILER_STACK_BYTES as u64)
            .saturating_add(cap / 16),
        snapshot_bytes: cap / 8,
        transport_bytes: cap / 32,
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn host_graphs_share_budget_and_charge_stacks_without_raising_the_cap() {
        let first = CompilerRequests::for_host(crate::COMPILER, CompileConfig::default());
        let second = CompilerRequests::for_host(crate::COMPILER, CompileConfig::default());
        assert!(Arc::ptr_eq(first.budget(), second.budget()));
        let cap = hirsel_guard::DEFAULT_LIMIT;
        for jobs in [1, 2, 4, 8, 16] {
            let limits = host_limits(cap, jobs);
            assert!(
                limits.runtime_bytes + limits.snapshot_bytes + limits.transport_bytes
                    <= cap - cap / 4
            );
        }
        let zero = host_limits(0, 2);
        assert_eq!(zero.snapshot_bytes + zero.transport_bytes, 0);
        assert!(ProcessBudget::new(0).reserve(zero.runtime_bytes).is_err());
        assert_eq!(first.capture_attempts(), 3);
        assert!(first.input_limits().bytes <= 16 << 20);
    }
}

impl hirsel_engine::requests::RequestEngine for CompilerRequests {
    type Request = CompileRequest;
    type Error = hirsel_engine::ToolError;
    type Runtime = hirsel_engine::runtime::AdmittedRuntime;
    type Readiness = CompilerReadiness;

    fn select(
        &mut self,
        request: &CompileRequest,
    ) -> Result<hirsel_engine::requests::SelectedRequest<Self::Runtime, Self::Readiness>, Self::Error>
    {
        self.ledger.admit()?;
        let ledger = self.ledger.clone();
        let compiler = &self.compiler;
        let config = &self.config;
        let limits = self.limits;
        let carry = self.carry.as_ref();
        let source_index = &self.source_index;
        let scratch_parent = &self.scratch_parent;
        let toolchain = self.toolchain.as_ref();
        let host_toolchain = self.host_toolchain;
        self.graphs.select(request, |budget, may_wait| {
            super::source_graph::install(
                compiler,
                config,
                limits,
                budget,
                may_wait,
                request,
                ledger,
                carry,
                source_index,
                scratch_parent,
                toolchain,
                host_toolchain,
            )
        })
    }
}

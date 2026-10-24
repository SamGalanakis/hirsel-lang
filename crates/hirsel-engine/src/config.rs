//! Host-selected compiler policy. These options affect work and retention, not
//! language meaning. Native object storage remains the driver's own provider.
use crate::{LoweringPolicy, QueryEngine, Verify, memo::Retention, store};
use hirsel_intern::jobs::Jobs;
use std::sync::Arc;

/// Policy for fresh compiler sessions. Explicit stores, including none@1, take
/// precedence over the CLI's environment-selected result provider.
#[derive(Clone)]
pub struct CompileConfig {
    pub jobs: Jobs,
    pub verify: Verify,
    pub retention: Retention,
    pub lowering: LoweringPolicy,
    pub store: Arc<dyn store::ResultStore>,
    pub verify_store: store::VerifyStore,
    /// Total retained-body allowance, shared across active workers. Zero is a
    /// real off setting; active evaluator/phase artifacts are in-flight memory.
    pub hold_bytes: u64,
}
impl Default for CompileConfig {
    fn default() -> Self {
        Self::for_memory_cap(configured_memory_cap())
    }
}
impl CompileConfig {
    /// Defaults under the host's memory cap. None uses the 256 MiB ceiling.
    pub fn for_memory_cap(memory_cap: Option<u64>) -> Self {
        Self {
            jobs: Jobs::new(hirsel_guard::jobs()),
            verify: Verify::On,
            retention: Retention::Batch,
            lowering: LoweringPolicy::Comptime,
            store: Arc::new(store::NoneStore),
            verify_store: store::VerifyStore::Off,
            hold_bytes: crate::memo::default_hold_bytes(memory_cap),
        }
    }
}

fn configured_memory_cap() -> Option<u64> {
    Some(
        std::env::var("HIRSEL_MEMORY_LIMIT")
            .ok()
            .and_then(|text| hirsel_guard::parse_limit(text.as_bytes()))
            .unwrap_or(hirsel_guard::DEFAULT_LIMIT),
    )
}
#[cfg(test)]
thread_local! { static FORBID_HOST_DEFAULTS: std::cell::Cell<bool> = const { std::cell::Cell::new(false) }; }
#[cfg(test)]
pub(crate) struct HostDefaultsGuard;
#[cfg(test)]
impl Drop for HostDefaultsGuard {
    fn drop(&mut self) {
        FORBID_HOST_DEFAULTS.with(|v| v.set(false));
    }
}
#[cfg(test)]
pub(crate) fn forbid_host_defaults() -> HostDefaultsGuard {
    FORBID_HOST_DEFAULTS.with(|v| assert!(!v.replace(true), "nested host-default trap"));
    HostDefaultsGuard
}
pub(crate) fn configured_hold_bytes() -> u64 {
    #[cfg(test)]
    FORBID_HOST_DEFAULTS.with(|v| assert!(!v.get(), "post-capture environment default lookup"));
    crate::memo::default_hold_bytes(configured_memory_cap())
}

impl QueryEngine {
    /// Configure this session before requesting program products. Explicit
    /// body-returning queries continue to return their demanded bodies.
    pub fn with_config(mut self, config: CompileConfig) -> Self {
        self.verify = config.verify;
        self.set_retention(config.retention);
        self.configured_jobs = Some(config.jobs);
        self.configured_lowering = config.lowering;
        self.set_hold_bytes(config.hold_bytes);
        self.verify_store = config.verify_store;
        self.with_result_store(config.store)
    }
    /// The total retained-body allowance selected by the host.
    pub fn hold_bytes(&self) -> u64 {
        self.hold_bytes
    }
    /// The worker policy captured by an explicit CompileConfig. None means
    /// this engine is unconfigured; reading it never consults the environment
    /// or chooses host defaults. Host adapters capture those at request entry.
    pub fn configured_jobs(&self) -> Option<Jobs> {
        self.configured_jobs
    }
    pub(crate) fn jobs(&self) -> Jobs {
        self.configured_jobs
            .unwrap_or_else(|| Jobs::new(hirsel_guard::jobs()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn memory_cap_defaults_and_zero_allowance_are_explicit() {
        assert_eq!(CompileConfig::for_memory_cap(None).hold_bytes, 256 << 20);
        assert_eq!(CompileConfig::for_memory_cap(Some(1000)).hold_bytes, 100);
        assert_eq!(CompileConfig::for_memory_cap(Some(0)).hold_bytes, 0);
        let config = CompileConfig {
            jobs: Jobs::new(8),
            hold_bytes: 0,
            retention: Retention::Resident,
            ..CompileConfig::for_memory_cap(None)
        };
        let engine =
            QueryEngine::new(crate::default_environment(), Default::default()).with_config(config);
        let unconfigured = QueryEngine::new(crate::default_environment(), Default::default());
        let _guard = forbid_host_defaults();
        assert_eq!(engine.configured_jobs().map(Jobs::get), Some(8));
        assert_eq!(unconfigured.configured_jobs(), None);
        assert_eq!(engine.jobs().get(), 8);
        assert_eq!(engine.hold_bytes(), 0);
        assert_eq!(engine.retention, Retention::Resident);
        assert!(
            engine.result_store.is_none(),
            "off providers disable capture"
        );
    }
}

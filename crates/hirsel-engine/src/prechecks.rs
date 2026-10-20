//! The prechecks a program load adopts: the toolchain's std precheck, or an
//! explicit set. Paths locate inputs; digests validate them.
use std::sync::{Arc, OnceLock};

/// Which prechecks loading adopts. A missing or unmatched precheck only means
/// checking the package from source.
#[derive(Clone, Default)]
pub struct Prechecks(Option<Arc<ToolchainPrechecks>>);
/// The toolchain's prechecks: `$HIRSEL_PRECHECK`, or `hirsel-std.precheck` beside the
/// executable. A missing or unreadable precheck only means checking from source.
pub(crate) struct ToolchainPrechecks {
    pub prechecks: Vec<Arc<crate::Precheck>>,
    /// Why there is no precheck, for `--verbose`.
    pub missing: Option<String>,
}
/// Write each later load's precheck notes (`LoadedProgram::precheck_notes`) to stderr.
static REPORT_PRECHECKS: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
/// With a heap meter (`--mem-usage`), drop the process's retained signature
/// inputs and shared results, recording the bytes each held. Call it once
/// compiling is over.
pub fn release_shared_caches() {
    if !hirsel_intern::memory::metered() {
        return;
    }
    let shared = crate::store::MemStore::shared();
    hirsel_intern::memory::released("result store: mem@1", shared.take());
}
pub fn report_prechecks(on: bool) {
    REPORT_PRECHECKS.store(on, std::sync::atomic::Ordering::Relaxed);
}
pub(crate) fn reporting_prechecks() -> bool {
    REPORT_PRECHECKS.load(std::sync::atomic::Ordering::Relaxed)
}
fn toolchain_prechecks() -> Arc<ToolchainPrechecks> {
    static PRECHECKS: OnceLock<Arc<ToolchainPrechecks>> = OnceLock::new();
    PRECHECKS
        .get_or_init(|| Arc::new(read_toolchain_prechecks()))
        .clone()
}
fn read_toolchain_prechecks() -> ToolchainPrechecks {
    let _timing = hirsel_intern::timings::interval(hirsel_intern::timings::Interval::ImageOpen);
    let path = match std::env::var_os("HIRSEL_PRECHECK") {
        Some(path) => std::path::PathBuf::from(path),
        None => match std::env::current_exe() {
            Ok(exe) => exe.with_file_name("hirsel-std.precheck"),
            Err(e) => {
                return ToolchainPrechecks {
                    prechecks: vec![],
                    missing: Some(format!(
                        "no precheck: the executable's path is unknown ({e})"
                    )),
                };
            }
        },
    };
    match hirsel_intern::memory::retained("precheck", || crate::Precheck::read(&path)) {
        Ok(precheck) => ToolchainPrechecks {
            prechecks: vec![Arc::new(precheck)],
            missing: None,
        },
        Err(e) => ToolchainPrechecks {
            prechecks: vec![],
            missing: Some(format!("no precheck: {e}")),
        },
    }
}
impl Prechecks {
    /// No prechecks: every package checks from source.
    pub fn none() -> Self {
        Self(None)
    }
    /// The toolchain's prechecks (`$HIRSEL_PRECHECK` or beside the executable).
    pub fn toolchain() -> Self {
        Self(Some(toolchain_prechecks()))
    }
    /// Exactly `prechecks`.
    pub fn of(prechecks: Vec<Arc<crate::Precheck>>) -> Self {
        Self(Some(Arc::new(ToolchainPrechecks {
            prechecks,
            missing: None,
        })))
    }
    pub(crate) fn get(&self) -> Option<&ToolchainPrechecks> {
        self.0.as_deref()
    }
}

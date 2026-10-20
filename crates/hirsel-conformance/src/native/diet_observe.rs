//! A feature observation owns a process, including all lowering choices. The
//! parent retains only strings and indices, never compiler arenas or LLVM state.
use std::collections::BTreeSet;
use std::path::Path;
use std::process::{Command, ExitCode, Stdio};

const INPUT: &str = "HIRSEL_NATIVE_OBSERVATION_INPUT";
pub(super) type Observed = (BTreeSet<String>, Vec<String>);

pub(super) fn observe(path: &Path) -> Observed {
    hirsel_engine::with_compiler_stack(|| {
        let observe = || {
            let mut program = hirsel_driver::load_program(path).map_err(|e| e.to_string())?;
            program.check_for_execution().map_err(|e| e.to_string())?;
            let raw = program
                .engine
                .export_native_input()
                .map_err(|e| format!("{e:?}"))?;
            let input = program
                .engine
                .import_native_input(raw)
                .map_err(|e| format!("{e:?}"))?;
            Ok::<_, String>(hirsel_driver::native::backend_features(&input))
        };
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(observe))
            .unwrap_or_else(|panic| {
                Err(format!(
                    "a native stage panicked: {}",
                    panic
                        .downcast_ref::<String>()
                        .map(String::as_str)
                        .or_else(|| panic.downcast_ref::<&str>().copied())
                        .unwrap_or("(no message)")
                ))
            });
        result.unwrap_or_else(|e| (BTreeSet::new(), vec![e]))
    })
}

/// Called before CLI parsing by every executable that can derive a selection.
/// Check/feature errors remain evidence; a crash or memory guard exit fails the
/// collection rather than silently treating a missing observation as coverage.
pub fn worker() -> Option<ExitCode> {
    let path = std::env::var_os(INPUT)?;
    let observed = observe(Path::new(&path));
    let status = std::fs::read_to_string("/proc/self/status").unwrap_or_default();
    let peak = status
        .lines()
        .find_map(|line| {
            line.strip_prefix("VmHWM:")
                .and_then(|s| s.split_whitespace().next())
        })
        .unwrap_or("unknown");
    eprintln!(
        "native observation {} {peak} {}",
        std::process::id(),
        Path::new(&path).display()
    );
    println!("{}", serde_json::to_string(&observed).unwrap());
    Some(ExitCode::SUCCESS)
}

pub(super) fn isolated(path: &Path) -> Result<Observed, String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let output = Command::new(exe)
        .env(INPUT, path)
        .stderr(Stdio::inherit())
        .output()
        .map_err(|e| format!("{}: feature worker: {e}", path.display()))?;
    if !output.status.success() {
        return Err(format!(
            "{}: feature worker {}",
            path.display(),
            output.status
        ));
    }
    serde_json::from_slice(&output.stdout)
        .map_err(|e| format!("{}: invalid feature worker output: {e}", path.display()))
}

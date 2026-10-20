//! Smoke and parity run complete cases in fresh processes. Dropping an engine
//! does not release process-global intern tables or the allocator's arenas.
//! Re-exec also frees LLVM state and the interpreter between cases, including
//! between the smoke's compilation, replay and parity phases.
use hirsel_conformance::native::Case;
use std::process::Command;
use std::time::Duration;

const WORKER_CASE: &str = "HIRSEL_NATIVE_WORKER_CASE";

pub(super) fn selected() -> Option<String> {
    std::env::var(WORKER_CASE).ok()
}

/// None in a worker or an empty selection (the ordinary judge rejects it).
pub(super) fn cases(cases: &[Case], jobs: usize) -> Result<Option<bool>, String> {
    if selected().is_some() || cases.is_empty() {
        return Ok(None);
    }
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    let commands = cases
        .iter()
        .map(|case| {
            let mut command = Command::new(&exe);
            command
                .args(&args)
                .args(["--jobs", "1"])
                .env(WORKER_CASE, &case.id);
            command
        })
        .collect();
    run(commands, jobs).map(Some)
}

/// Bounded fan-out; reap every worker and keep running after a case fails.
/// Children inherit the action's memory guard, toolchains and output streams.
fn run(commands: Vec<Command>, jobs: usize) -> Result<bool, String> {
    let commands = std::sync::Mutex::new(commands.into_iter());
    std::thread::scope(|scope| {
        let handles: Vec<_> = (0..jobs.max(1))
            .map(|_| {
                let commands = &commands;
                scope.spawn(move || {
                    let mut ok = true;
                    loop {
                        let Some(mut command) = commands.lock().unwrap().next() else {
                            break;
                        };
                        let status = command
                            .status()
                            .map_err(|e| format!("native worker: {e}"))?;
                        ok &= status.success();
                        if !status.success() {
                            eprintln!("native worker: {status}");
                        }
                    }
                    Ok(ok)
                })
            })
            .collect();
        let mut ok = true;
        let mut error = None;
        for handle in handles {
            match handle.join().expect("native worker supervisor panicked") {
                Ok(passed) => ok &= passed,
                Err(message) => error = Some(message),
            }
        }
        error.map_or(Ok(ok), Err)
    })
}

/// Machine-readable measurements feed the same per-case TSV cost tables as
/// libtest. RSS is the worker's high water mark, in KiB, not a timing verdict.
pub(super) fn report(wall: Duration, passed: bool) {
    if let Some(id) = selected() {
        let status = std::fs::read_to_string("/proc/self/status").unwrap_or_default();
        let peak = status
            .lines()
            .find_map(|line| {
                line.strip_prefix("VmHWM:")
                    .and_then(|s| s.split_whitespace().next())
            })
            .unwrap_or("unknown");
        let kind = if passed {
            "native case"
        } else {
            "native failed case"
        };
        eprintln!("{kind}\t{id}\t{:.3}\t{peak}", wall.as_secs_f64());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workers_are_reaped_and_a_failure_does_not_skip_later_cases() {
        let dir = std::env::temp_dir().join(format!("native-workers-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let commands = (0..4)
            .map(|i| {
                let mut command = Command::new("sh");
                command
                    .args(["-c", "echo $$ > \"$1\"; exit \"$2\"", "worker"])
                    .arg(dir.join(i.to_string()))
                    .arg(if i == 1 { "1" } else { "0" });
                command
            })
            .collect();
        assert!(!run(commands, 2).unwrap());
        let pids: std::collections::BTreeSet<_> = (0..4)
            .map(|i| std::fs::read_to_string(dir.join(i.to_string())).unwrap())
            .collect();
        assert_eq!(pids.len(), 4, "each case needs a fresh process");
        assert!(
            pids.iter()
                .all(|pid| !std::path::Path::new("/proc").join(pid.trim()).exists())
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn worker_launch_errors_fail_the_campaign() {
        assert!(run(vec![Command::new("/no/such/native-worker")], 1).is_err());
    }
}

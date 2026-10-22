//! Host-only canonical trace selection. Parsing and qualification never open a
//! destination or admit a program; publication uses the existing pinned writer.
use std::ffi::OsString;
use std::path::{Path, PathBuf};

pub(crate) const UNQUALIFIED: &str =
    "canonical trace events require production-qualified Ordered execution";

pub(crate) fn check() -> Result<(), String> {
    if crate::tasks::ordered::admission::qualified() {
        Ok(())
    } else {
        Err(UNQUALIFIED.into())
    }
}

/// The observer prefix belongs before run/replay arguments. It never consumes
/// an application's trailing argument with the same spelling.
pub(crate) fn select(arguments: &mut Vec<OsString>) -> Result<Option<PathBuf>, String> {
    if arguments
        .first()
        .is_none_or(|arg| arg != "--hirsel-trace-events")
    {
        return Ok(None);
    }
    let path = arguments
        .get(1)
        .filter(|p| !p.is_empty())
        .ok_or("expected a path after --hirsel-trace-events")?;
    let path = PathBuf::from(path);
    check()?;
    if arguments
        .get(2)
        .is_some_and(|flag| flag == "--hirsel-replay")
        && let Some(input) = arguments.get(3)
    {
        let output =
            crate::host_paths::RecorderDestination::pin(&path).map_err(|e| e.to_string())?;
        let input = crate::host_paths::RecorderDestination::pin(Path::new(input))
            .map_err(|e| e.to_string())?;
        output.check_distinct(&input).map_err(|e| e.to_string())?;
    }
    arguments.drain(..2);
    Ok(Some(path))
}

pub(crate) fn pin(
    path: &Path,
    ordered: bool,
) -> Result<crate::host_paths::RecorderDestination, String> {
    check()?;
    if !ordered {
        return Err("canonical trace events require the FIFO cooperative task provider".into());
    }
    crate::host_paths::RecorderDestination::pin(path).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn host_trace_request_refuses_before_entry_and_context_admission() {
        unsafe extern "C" fn entry(_: *mut crate::TaskCtx, _: *mut u8, out: *mut u8) -> u8 {
            unsafe { out.write_bytes(0, 4) };
            crate::STATUS_OK
        }
        let config = crate::launch::Config {
            hash_seed: 0,
            random_seed: 0,
            test_seed: 0,
            run_identity: [0; 32],
        };
        let program = crate::program::Program::harness(
            crate::program::Code::Zero(entry),
            4,
            std::ptr::null(),
            0,
            config,
        );
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("events.jsonl");
        let (sink, output) = crate::host::Sink::capture();
        let sinks = crate::host::Sinks {
            stdout: sink.clone(),
            stderr: sink.clone(),
            report: sink,
        };
        let host = crate::host::Host::process();
        let outcome = host
            .start(
                &program,
                config,
                crate::host::Selection::Harness,
                vec![],
                sinks,
            )
            .with_trace_events(&path)
            .wait();
        assert_eq!(
            outcome,
            hirsel_run_schema::RunOutcome::Refused(UNQUALIFIED.into())
        );
        assert!(!path.exists());
        assert!(
            String::from_utf8(output.bytes())
                .unwrap()
                .contains(UNQUALIFIED)
        );
    }

    #[test]
    fn trace_destination_distinguishes_recordings_before_any_creation_or_truncation() {
        use crate::host_paths::RecorderDestination as D;
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("missing");
        let first = D::pin(&missing).unwrap();
        let alias = dir.path().join("alias-dir");
        std::os::unix::fs::symlink(dir.path(), &alias).unwrap();
        assert!(
            first
                .check_distinct(&D::pin(&alias.join("missing")).unwrap())
                .is_err()
        );
        assert!(!missing.exists());
        let existing = dir.path().join("existing");
        let hardlink = dir.path().join("hardlink");
        std::fs::write(&existing, b"sentinel").unwrap();
        std::fs::hard_link(&existing, &hardlink).unwrap();
        assert!(
            D::pin(&existing)
                .unwrap()
                .check_distinct(&D::pin(&hardlink).unwrap())
                .is_err()
        );
        assert!(first.check_distinct(&D::pin(&existing).unwrap()).is_ok());
        assert_eq!(std::fs::read(existing).unwrap(), b"sentinel");
    }

    #[test]
    fn unqualified_trace_selection_refuses_without_opening_or_consuming_any_input() {
        let dir = tempfile::tempdir().unwrap();
        let existing = dir.path().join("existing.jsonl");
        let missing = dir.path().join("missing.jsonl");
        std::fs::write(&existing, b"sentinel").unwrap();
        for path in [&existing, &missing] {
            let mut args = vec![
                "--hirsel-trace-events".into(),
                path.as_os_str().into(),
                "--hirsel-run".into(),
                "missing-run.json".into(),
            ];
            let before = args.clone();
            assert_eq!(select(&mut args), Err(UNQUALIFIED.into()));
            assert_eq!(args, before);
            assert!(pin(path, true).is_err());
            assert_eq!(std::fs::read(&existing).unwrap(), b"sentinel");
            assert!(!missing.exists());
        }
        for mut args in [
            vec!["--hirsel-trace-events".into()],
            vec!["--hirsel-trace-events".into(), "".into()],
        ] {
            assert_eq!(
                select(&mut args),
                Err("expected a path after --hirsel-trace-events".into())
            );
        }
        let mut args = vec![
            "--hirsel-grants".into(),
            "{}".into(),
            "--".into(),
            "--hirsel-trace-events".into(),
        ];
        let before = args.clone();
        assert_eq!(select(&mut args), Ok(None));
        assert_eq!(args, before);
    }
}

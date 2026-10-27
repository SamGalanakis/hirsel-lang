//! Host-only canonical trace selection. Parsing and qualification never open a
//! destination or admit a program; publication uses the existing pinned writer.
use std::ffi::OsString;
use std::path::{Path, PathBuf};

pub(crate) mod free;

pub(crate) const REQUIRES_ORDERED: &str =
    "canonical trace events require a qualified Free or FIFO cooperative task provider";

/// Select an observer anywhere in the runtime prefix, preserving the first
/// application word and explicit delimiter as permanent parsing boundaries.
pub(crate) fn select(arguments: &mut Vec<OsString>) -> Result<Option<PathBuf>, String> {
    let options = crate::standalone::live_options(arguments).map_err(|error| match &error {
        hirsel_run_schema::RunRefusal::Invalid { field, detail }
            if field == "argv" && detail == "expected a path after --hirsel-trace-events" =>
        {
            detail.clone()
        }
        _ => error.to_string(),
    })?;
    let Some((offset, path)) = options.trace else {
        return Ok(None);
    };
    if options
        .input
        .first()
        .is_some_and(|flag| flag == "--hirsel-replay")
        && let Some(input) = options.input.get(1)
    {
        let output =
            crate::host_paths::RecorderDestination::pin(&path).map_err(|e| e.to_string())?;
        let input = crate::host_paths::RecorderDestination::pin(Path::new(input))
            .map_err(|e| e.to_string())?;
        output.check_distinct(&input).map_err(|e| e.to_string())?;
    }
    arguments.drain(offset..offset + 2);
    Ok(Some(path))
}

pub(crate) fn pin(
    path: &Path,
    ordered: bool,
) -> Result<crate::host_paths::RecorderDestination, String> {
    if !ordered {
        return Err(REQUIRES_ORDERED.into());
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
            hirsel_run_schema::RunOutcome::Refused(REQUIRES_ORDERED.into())
        );
        assert!(!path.exists());
        assert!(
            String::from_utf8(output.bytes())
                .unwrap()
                .contains(REQUIRES_ORDERED)
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
    fn trace_selection_consumes_only_its_prefix_without_opening_any_destination() {
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
            assert_eq!(select(&mut args), Ok(Some(path.clone())));
            assert_eq!(args, before[2..]);
            assert!(pin(path, false).is_err());
            assert!(pin(path, true).is_ok());
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

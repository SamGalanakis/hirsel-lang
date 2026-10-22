//! Preserve a timed-out launch before a campaign reuses its job directory.
use std::path::{Path, PathBuf};
use std::process::Command;

pub(super) fn retain(dir: &Path, command: &Command, reason: &str) -> std::io::Result<PathBuf> {
    let parent = std::env::var_os("HIRSEL_NATIVE_TIMEOUT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| dir.join("timeouts"));
    retain_in(dir, &parent, command, reason)
}

fn retain_in(
    dir: &Path,
    parent: &Path,
    command: &Command,
    reason: &str,
) -> std::io::Result<PathBuf> {
    std::fs::create_dir_all(parent)?;
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let retained = parent.join(format!("{}-{stamp}", std::process::id()));
    std::fs::create_dir(&retained)?;
    // The executable, run record, observations and link inputs are regular
    // top-level files. Do not follow fixture symlinks or recursively copy an
    // earlier retained timeout. The campaign retains the fixture source too.
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        if entry.file_type()?.is_file() {
            std::fs::copy(entry.path(), retained.join(entry.file_name()))?;
        }
    }
    std::fs::write(
        retained.join("launch.txt"),
        format!("{command:?}\n{reason}\n"),
    )?;
    Ok(retained)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retained_launch_survives_job_reuse_without_following_fixture_links() {
        let dir_scratch = tempfile::Builder::new()
            .prefix("hirsel-timeout-")
            .tempdir()
            .unwrap();
        let dir = dir_scratch.path().to_path_buf();
        std::fs::write(dir.join("program"), b"first executable").unwrap();
        std::fs::write(dir.join("run.json"), b"first record").unwrap();
        std::os::unix::fs::symlink("/dev/zero", dir.join("fixture-link")).unwrap();
        let retained = retain_in(
            &dir,
            &dir.join("timeouts"),
            Command::new("loader").arg("program"),
            "launch timeout",
        )
        .unwrap();
        std::fs::write(dir.join("program"), b"second executable").unwrap();
        assert_eq!(
            std::fs::read(retained.join("program")).unwrap(),
            b"first executable"
        );
        assert_eq!(
            std::fs::read(retained.join("run.json")).unwrap(),
            b"first record"
        );
        assert!(!retained.join("fixture-link").exists());
        assert!(!retained.join("timeouts").exists());
        assert!(
            std::fs::read_to_string(retained.join("launch.txt"))
                .unwrap()
                .contains("launch timeout")
        );
        std::fs::remove_dir_all(dir).unwrap();
    }
}

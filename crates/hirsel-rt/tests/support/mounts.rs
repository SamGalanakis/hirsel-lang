//! Scratch host directories of the mount tests and of their interpreter-replay twins in
//! hirsel-pipeline-tests (`rt_mounts`).
use hirsel_rt::{host_paths::PinnedDirectory, io_real::RealIo};
use hirsel_run_config::Access;
use std::{fs, path::PathBuf};
pub struct Scratch(pub PathBuf);

impl Scratch {
    pub fn new() -> Self {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let root = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!(
            "mounts-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        fs::create_dir(&root).unwrap();
        for dir in ["one", "one/sub", "two", "tapes"] {
            fs::create_dir(root.join(dir)).unwrap();
        }
        fs::write(root.join("one/sub/a"), b"A").unwrap();
        fs::write(root.join("two/b"), b"B").unwrap();
        Self(root)
    }
    pub fn io(&self, mounts: &[(&str, Access, &str)]) -> RealIo {
        RealIo::from_pinned_mounts(
            mounts.iter().map(|(name, access, host)| {
                (
                    name.to_string(),
                    *access,
                    PinnedDirectory::open(&self.0.join(host)).unwrap(),
                )
            }),
            std::io::sink(),
        )
        .unwrap()
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

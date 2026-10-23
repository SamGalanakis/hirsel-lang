//! Main-based integration fixture. The production EpochLease is supplied by
//! pack.rs when the protocol owner integrates the same index module.
hirsel_guard::install!();
#[path = "../src/group.rs"]
mod group;
#[path = "../src/index.rs"]
mod index;
mod pack {
    pub(crate) struct EpochLease(pub(crate) std::path::PathBuf);
    pub(crate) fn index_benchmark_epoch(root: &std::path::Path) -> std::io::Result<EpochLease> {
        Ok(EpochLease(root.to_owned()))
    }
    impl EpochLease {
        pub(crate) fn root(&self) -> &std::path::Path {
            &self.0
        }
    }
}
mod modules;

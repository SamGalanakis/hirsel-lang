//! Optional physical work counters. They describe one in-process owner, never
//! durable identities, source freshness or verifier authority.
use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};

macro_rules! counters {
    ($($name:ident),+ $(,)?) => {
        #[derive(Clone, Copy)]
        pub(crate) enum Counter { $($name,)+ Count }
        const NAMES: &[&str] = &[$(stringify!($name),)+];
        const ALL: &[Counter] = &[$(Counter::$name,)+];
    };
}
counters!(
    Epochs,
    Commits,
    FinishCommits,
    DropCommits,
    RecordLimitCommits,
    ByteLimitCommits,
    CollectionCommits,
    PrivateSpills,
    ByteLimitSpills,
    RecordLimitSpills,
    AdmissionCommits,
    Aborts,
    PrivatePeakBytes,
    EpochMetadataPeakBytes,
    GcProtectedEpochs,
    SealedSegments,
    IndexLoadAttempts,
    IndexPins,
    InvalidIndexRoots,
    BadIndexBlocks,
    BadIndexBuckets,
    IndexBackingBytesLoaded,
    IndexLayerBuilds,
    IndexPublicationAttempts,
    IndexPublicationSkips,
    IndexMaintenanceAttempts,
    IndexMaintenancePublications,
    IndexChecks,
    CheckedIndexBytes,
    IndexStamps,
    IndexOpens,
    SegmentOpens,
    StandaloneOpens,
    RegionLoads,
    RegionCacheHits,
    ContentChecks,
    ContentReuses,
    RetainedRegionPeakBytes,
    RetainedRegionPeakCount,
    RegionBytesLoaded,
    LiveBackingBytes,
    PeakBackingBytes,
    PendingPeakBytes,
    PendingPeakRecords,
    PublicationAttempts,
    PublicationAdmissionBytes,
    Publications,
    IndexPublications,
    FileSyncs,
    DirectorySyncs,
    GcScans,
    GcEntries,
    GcUnlinks,
    SkippedPersistence,
    IdenticalUpdates,
);

pub struct Statistics {
    owner: u64,
    values: [AtomicU64; Counter::Count as usize],
}
#[derive(Debug, serde::Serialize)]
pub struct StatisticsSnapshot {
    pub owner: u64,
    pub counters: BTreeMap<&'static str, u64>,
}
impl Statistics {
    pub(crate) fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        Self {
            owner: NEXT.fetch_add(1, Relaxed),
            values: std::array::from_fn(|_| AtomicU64::new(0)),
        }
    }
    pub fn snapshot(&self) -> StatisticsSnapshot {
        StatisticsSnapshot {
            owner: self.owner,
            counters: NAMES
                .iter()
                .zip(ALL)
                .map(|(&name, &counter)| (name, self.values[counter as usize].load(Relaxed)))
                .collect(),
        }
    }
    pub(crate) fn add(&self, counter: Counter, value: u64) {
        self.values[counter as usize].fetch_add(value, Relaxed);
    }
    pub(crate) fn peak(&self, counter: Counter, value: u64) {
        self.values[counter as usize].fetch_max(value, Relaxed);
    }
}
pub(crate) struct Backing {
    statistics: Arc<Statistics>,
    bytes: u64,
}
impl Backing {
    pub(crate) fn new(statistics: Arc<Statistics>, bytes: u64) -> Self {
        let live = statistics.values[Counter::LiveBackingBytes as usize]
            .fetch_add(bytes, Relaxed)
            .saturating_add(bytes);
        statistics.peak(Counter::PeakBackingBytes, live);
        Self { statistics, bytes }
    }
}
impl Drop for Backing {
    fn drop(&mut self) {
        self.statistics.values[Counter::LiveBackingBytes as usize].fetch_sub(self.bytes, Relaxed);
    }
}

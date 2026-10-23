//! Immutable demand-checked routing hints; never content or verifier authority.
//! Format: hirsel-briefs/store-moonshot-36/DESIGN.md, revision 2026-10-23.
//! Git idx/MIDX informed the separated planes and chain; no Git code is copied.

use hirsel_syntax::ContentHash;
use std::{io, path::Path, sync::Arc};

#[path = "index/format.rs"]
mod format;
#[path = "index/storage.rs"]
mod storage;
use format::{Descriptor, Layer, Manifest};
use storage::{Lock, Stage};

pub(crate) const MAX_ROWS: usize = 2_000_000;
pub(crate) const MAX_BYTES: usize = 128 << 20;
pub(crate) const MAX_SCRATCH: usize = 256 << 20;
const MAX_LAYERS: usize = 8;

#[cfg(test)]
thread_local! {
    static COMPACT_OBSERVER: std::cell::RefCell<Option<Box<dyn FnOnce()>>> = const { std::cell::RefCell::new(None) };
    static PIN_OBSERVER: std::cell::RefCell<Option<Box<dyn FnOnce()>>> = const { std::cell::RefCell::new(None) };
}
/// Deterministic test barrier after selection, with the publication lock free.
#[cfg(test)]
pub(crate) fn observe_compaction(callback: impl FnOnce() + 'static) {
    COMPACT_OBSERVER.with(|slot| *slot.borrow_mut() = Some(Box::new(callback)));
}
#[cfg(test)]
pub(crate) fn observe_pin(callback: impl FnOnce() + 'static) {
    PIN_OBSERVER.with(|slot| *slot.borrow_mut() = Some(Box::new(callback)));
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
#[repr(u8)]
pub(crate) enum KeyKind {
    Content = 0,
    Association = 1,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub(crate) struct Key {
    pub(crate) kind: KeyKind,
    pub(crate) digest: ContentHash,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Location {
    pub(crate) segment: ContentHash,
    pub(crate) offset: u32,
    pub(crate) len: u32,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Entry {
    pub(crate) key: Key,
    pub(crate) location: Location,
}

#[derive(Default)]
struct PinnedState {
    layers: Vec<Layer>,
    invalid_root: bool,
}
#[derive(Clone, Default)]
pub(crate) struct PinnedGeneration(Arc<PinnedState>);
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct IndexDiagnostics {
    pub(crate) invalid_root: bool,
    pub(crate) bad_blocks: u64,
    pub(crate) bad_buckets: u64,
}
impl PinnedGeneration {
    /// Full retained mappings plus directory/block/bucket allocations, including
    /// unlinked inodes. Shared clones must be charged once by their owner.
    pub(crate) fn retained_bytes(&self) -> u64 {
        // Include spare Vec capacity and the Arc allocation even for absence.
        let shared = size_of::<PinnedState>()
            + 2 * size_of::<usize>()
            + self.0.layers.capacity() * size_of::<Layer>();
        shared as u64 + self.0.layers.iter().map(Layer::retained_bytes).sum::<u64>()
    }
    /// Boundary observation, O(layers), without scanning status arrays or files.
    pub(crate) fn diagnostics(&self) -> IndexDiagnostics {
        let mut result = IndexDiagnostics {
            invalid_root: self.0.invalid_root,
            ..IndexDiagnostics::default()
        };
        for layer in &self.0.layers {
            let (blocks, buckets) = layer.diagnostics();
            result.bad_blocks += blocks;
            result.bad_buckets += buckets;
        }
        result
    }
    fn invalid_root() -> Self {
        Self(Arc::new(PinnedState {
            invalid_root: true,
            ..PinnedState::default()
        }))
    }
}
pub(crate) struct PreparedDelta {
    root: std::path::PathBuf,
    stage: Option<Stage>,
    descriptor: Option<Descriptor>,
}
impl PreparedDelta {
    /// New staged bytes plus the largest bounded root replacement. This is
    /// conservative disk admission, without probing any existing file.
    pub(crate) fn admission_bytes(&self) -> u64 {
        self.descriptor.as_ref().map_or(0, |d| d.len + 420)
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SkipReason {
    LayerLimit,
    ByteLimit,
    RowLimit,
    Contended,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PublishOutcome {
    Empty,
    Published {
        generation: ContentHash,
        layers: u8,
        maintenance_due: bool,
    },
    Skipped(SkipReason),
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct MaintenanceBudget {
    pub(crate) max_input_bytes: u64,
    pub(crate) max_rows: u32,
    pub(crate) max_scratch_bytes: u64,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CompactOutcome {
    NotNeeded,
    Deferred,
    Conflict,
    Published { generation: ContentHash, layers: u8 },
}

fn invalid() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, "invalid index")
}
fn input() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, "invalid index input or budget")
}

pub(crate) fn build(root: &Path, entries: &[Entry]) -> io::Result<PreparedDelta> {
    std::fs::create_dir_all(root)?;
    let root = root.canonicalize()?;
    if entries.is_empty() {
        return Ok(PreparedDelta {
            root,
            stage: None,
            descriptor: None,
        });
    }
    let bytes = format::encode_layer(entries)?;
    // Hash only completed NEW bytes, once, outside the publication lock.
    let descriptor = Descriptor {
        id: ContentHash::of_parts("hirsel.artifact.index-layer.v36", [&bytes[..]]),
        rows: entries.len() as u64,
        len: bytes.len() as u64,
    };
    let stage = Stage::new(&root)?;
    stage.write("layer", &bytes)?;
    Ok(PreparedDelta {
        root,
        stage: Some(stage),
        descriptor: Some(descriptor),
    })
}

fn pin(root: &Path, manifest: &Manifest) -> io::Result<PinnedGeneration> {
    let layers = manifest
        .layers
        .iter()
        .map(|d| Layer::open(root, *d))
        .collect::<io::Result<_>>()?;
    Ok(PinnedGeneration(Arc::new(PinnedState {
        layers,
        invalid_root: false,
    })))
}

pub(crate) fn load_pinned(root: &Path) -> io::Result<PinnedGeneration> {
    if !root.exists() {
        return Ok(PinnedGeneration::default());
    }
    let Some(_lock) = Lock::publication(root, false)? else {
        return Ok(PinnedGeneration::default());
    };
    let (manifest, invalid_root) = Manifest::read_with_status(root)?;
    if invalid_root {
        return Ok(PinnedGeneration::invalid_root());
    }
    #[cfg(test)]
    PIN_OBSERVER.with(|slot| {
        let callback = slot.borrow_mut().take();
        if let Some(callback) = callback {
            callback();
        }
    });
    match pin(root, &manifest) {
        Ok(pin) => Ok(pin),
        Err(e)
            if matches!(
                e.kind(),
                io::ErrorKind::InvalidData | io::ErrorKind::NotFound
            ) =>
        {
            Ok(PinnedGeneration::invalid_root())
        }
        Err(e) => Err(e),
    }
}

pub(crate) fn find(generation: &PinnedGeneration, key: Key) -> Option<Location> {
    generation.0.layers.iter().find_map(|layer| layer.find(key))
}

/// GC already owns .pack-lock. This bounded metadata helper does not lock,
/// open any layer or recover orphan names.
pub(crate) fn current_layer_ids_locked(root: &Path) -> io::Result<Vec<ContentHash>> {
    Ok(Manifest::read(root)?
        .layers
        .into_iter()
        .map(|d| d.id)
        .collect())
}

fn admission(layers: &[Descriptor]) -> Option<SkipReason> {
    if layers.len() > MAX_LAYERS {
        return Some(SkipReason::LayerLimit);
    }
    if layers
        .iter()
        .try_fold(0u64, |n, d| n.checked_add(d.rows))
        .is_none_or(|n| n > MAX_ROWS as u64)
    {
        return Some(SkipReason::RowLimit);
    }
    if layers
        .iter()
        .try_fold(0u64, |n, d| n.checked_add(d.len))
        .is_none_or(|n| n > MAX_BYTES as u64)
    {
        return Some(SkipReason::ByteLimit);
    }
    None
}

/// Number of newest-first descriptors selected by the chronological suffix rule.
fn selection(layers: &[Descriptor]) -> usize {
    let Some(first) = layers.first() else {
        return 0;
    };
    let mut rows = first.rows;
    let mut count = 1;
    for older in &layers[1..] {
        if older.rows > rows.saturating_mul(4) {
            break;
        }
        rows += older.rows; // manifest admission bounds sum to MAX_ROWS
        count += 1;
    }
    if count == 1 && layers.len() == MAX_LAYERS {
        2
    } else {
        count
    }
}

pub(crate) fn publish_delta(
    root: &Path,
    delta: PreparedDelta,
    epoch: &crate::pack::EpochLease,
) -> io::Result<PublishOutcome> {
    if root.canonicalize()? != delta.root || epoch.root().canonicalize()? != delta.root {
        return Err(input());
    }
    let (Some(descriptor), Some(stage)) = (delta.descriptor, delta.stage.as_ref()) else {
        return Ok(PublishOutcome::Empty);
    };
    let Some(_lock) = Lock::publication(root, true)? else {
        return Ok(PublishOutcome::Skipped(SkipReason::Contended));
    };
    let mut manifest = Manifest::read(root)?;
    if manifest.layers.iter().any(|d| d.id == descriptor.id) {
        return Ok(PublishOutcome::Empty);
    }
    manifest.layers.insert(0, descriptor);
    if let Some(reason) = admission(&manifest.layers) {
        return Ok(PublishOutcome::Skipped(reason));
    }
    // Existing same-name inodes are never overwritten or body-compared here.
    if !stage.install(root, descriptor)? {
        return Ok(PublishOutcome::Empty);
    }
    let generation = manifest.publish(root, stage)?;
    Ok(PublishOutcome::Published {
        generation,
        layers: manifest.layers.len() as u8,
        maintenance_due: selection(&manifest.layers) >= 2,
    })
}

pub(crate) fn compact(root: &Path, budget: MaintenanceBudget) -> io::Result<CompactOutcome> {
    if !root.exists() {
        return Ok(CompactOutcome::NotNeeded);
    }
    let Some(_owner) = Lock::maintenance(root)? else {
        return Ok(CompactOutcome::Deferred);
    };
    let (original, selected, pinned) = {
        let Some(_lock) = Lock::publication(root, false)? else {
            return Ok(CompactOutcome::Deferred);
        };
        let original = Manifest::read(root)?;
        let count = selection(&original.layers);
        if count < 2 {
            return Ok(CompactOutcome::NotNeeded);
        }
        let selected = Manifest {
            number: original.number,
            layers: original.layers[..count].to_vec(),
        };
        let rows: u64 = selected.layers.iter().map(|d| d.rows).sum();
        let bytes: u64 = selected.layers.iter().map(|d| d.len).sum();
        // Conservative peak: merged Entry storage, dictionary, all logical and
        // physical output planes, and mapping status arrays. No payload copy.
        let scratch = rows
            .checked_mul(240)
            .and_then(|n| n.checked_add(4 << 20))
            .ok_or_else(input)?;
        if rows > u64::from(budget.max_rows)
            || bytes > budget.max_input_bytes
            || scratch > budget.max_scratch_bytes.min(MAX_SCRATCH as u64)
        {
            return Ok(CompactOutcome::Deferred);
        }
        let pinned = pin(root, &selected)?;
        (original, selected, pinned)
    };
    #[cfg(test)]
    COMPACT_OBSERVER.with(|slot| {
        let callback = slot.borrow_mut().take();
        if let Some(callback) = callback {
            callback();
        }
    });
    for layer in &pinned.0.layers {
        layer.validate_all()?;
    }
    // At most eight sorted streams. A linear head picker is bounded and avoids
    // heap churn; newest-first tie selection preserves the newest locator.
    let capacity = selected.layers.iter().map(|d| d.rows as usize).sum();
    let mut merged = Vec::new();
    merged.try_reserve_exact(capacity).map_err(|_| input())?;
    let mut positions = [0usize; MAX_LAYERS];
    loop {
        let mut best: Option<Entry> = None;
        for (i, layer) in pinned.0.layers.iter().enumerate() {
            if positions[i] < layer.rows() {
                let entry = layer.entry(positions[i]).ok_or_else(invalid)?;
                if best.is_none_or(|b| entry.key < b.key) {
                    best = Some(entry);
                }
            }
        }
        let Some(best) = best else {
            break;
        };
        merged.push(best);
        for (i, layer) in pinned.0.layers.iter().enumerate() {
            if positions[i] < layer.rows()
                && layer.key(positions[i]).ok_or_else(invalid)? == best.key
            {
                positions[i] += 1;
            }
        }
    }
    let delta = build(root, &merged)?;
    let descriptor = delta.descriptor.ok_or_else(invalid)?;
    let stage = delta.stage.as_ref().ok_or_else(invalid)?;
    let Some(_lock) = Lock::publication(root, true)? else {
        return Ok(CompactOutcome::Deferred);
    };
    let mut latest = Manifest::read(root)?;
    // Only concurrent prepends may change the input root. This verifies exact
    // descriptors and retained precedence rather than trusting a generation.
    if !latest.layers.ends_with(&original.layers) {
        return Ok(CompactOutcome::Conflict);
    }
    let start = latest.layers.len() - original.layers.len();
    // A concurrent writer may already have appended exactly our output. Do
    // not emit duplicate descriptors (which a subsequent pin must refuse).
    if latest
        .layers
        .iter()
        .enumerate()
        .any(|(i, d)| (i < start || i >= start + selected.layers.len()) && d.id == descriptor.id)
    {
        return Ok(CompactOutcome::Conflict);
    }
    let reusable = selected.layers.contains(&descriptor);
    latest
        .layers
        .splice(start..start + selected.layers.len(), [descriptor]);
    if admission(&latest.layers).is_some() {
        return Ok(CompactOutcome::Deferred);
    }
    // If newer inputs completely shadow older ones, the deterministic output
    // can be an already-listed, fully validated selected layer. Reuse its
    // immutable inode instead of deferring maintenance forever.
    if !stage.install(root, descriptor)? && !reusable {
        return Ok(CompactOutcome::Deferred);
    }
    let generation = latest.publish(root, stage)?;
    Ok(CompactOutcome::Published {
        generation,
        layers: latest.layers.len() as u8,
    })
}

#[cfg(test)]
#[path = "index/policy_tests.rs"]
mod policy_tests;

#[cfg(test)]
#[path = "index/microbench.rs"]
mod microbench;

#[cfg(test)]
#[path = "index/tests/mod.rs"]
mod tests;

//! A source carry owns exact header records and effective dirty marks.
//! Reverse indexes are derived from those receipts in the fresh source engine.
use super::*;

pub(crate) struct SourceHeaderGraph {
    revision: Revision,
    last_changed: [Revision; 3],
    records: Vec<(MemoKey, Record)>,
    dirty: Vec<(MemoKey, Invalidation)>,
    stats: MemoStats,
    touched_memos: Option<HashSet<MemoKey>>,
}
impl SourceHeaderGraph {
    pub(crate) fn restore(self) -> MemoGraph {
        let mut graph = MemoGraph {
            revision: self.revision,
            last_changed: self.last_changed,
            stats: self.stats,
            touched_memos: self.touched_memos,
            ..Default::default()
        };
        for (key, record) in self.records {
            for (read, _) in record.reads.iter() {
                Arc::make_mut(graph.subscribers.entry(*read).or_default()).insert(key);
                if matches!(
                    read,
                    ReadKey::ValidationUniverse(_) | ReadKey::UniverseErrors | ReadKey::FirstHeap
                ) {
                    graph.validation_contexts.entry(*read).or_default();
                }
            }
            graph.by_def.entry(key.def).or_default().insert(key);
            graph.records.insert(key, record);
        }
        graph.dirty.extend(self.dirty);
        graph
    }
}
fn set_bytes(capacity: usize) -> Option<usize> {
    if capacity == 0 {
        return Some(0);
    }
    capacity
        .checked_mul(8)?
        .div_ceil(7)
        .checked_next_power_of_two()?
        .max(4)
        .checked_mul(size_of::<(MemoKey, ())>().checked_add(1)?)?
        .checked_add(16)?
        .checked_add(align_of::<(MemoKey, ())>())
}
impl MemoGraph {
    /// Inspect the complete compact state without allocation or query work.
    /// Header entries already own the same exact read Arcs and payloads.
    pub(crate) fn source_headers_bytes<'a>(
        &self,
        headers: impl Fn(&MemoKey) -> Option<&'a Arc<Reads>>,
    ) -> Option<u64> {
        let count = self
            .records
            .keys()
            .filter(|key| headers(key).is_some())
            .count();
        let dirty = self
            .records
            .keys()
            .filter(|key| headers(key).is_some() && self.invalidation(key).is_some())
            .count();
        let mut bytes = size_of::<SourceHeaderGraph>()
            .checked_add(count.checked_mul(size_of::<(MemoKey, Record)>())?)?
            .checked_add(dirty.checked_mul(size_of::<(MemoKey, Invalidation)>())?)?;
        if let Some(touched) = &self.touched_memos {
            bytes = bytes.checked_add(set_bytes(touched.capacity())?)?;
        }
        let mut stats = crate::NativeMetadataWeight::default();
        stats.tree::<QueryKind, u64>(self.stats.recomputed.len());
        (bytes as u64).checked_add(stats.bytes())
    }
    /// Discard indexes that can be rebuilt from the exact retained receipts.
    /// Flatten pending context marks before dropping their reverse sets.
    pub(crate) fn into_source_headers<'a>(
        mut self,
        headers: impl Fn(&MemoKey) -> Option<&'a Arc<Reads>>,
    ) -> SourceHeaderGraph {
        let count = self
            .records
            .keys()
            .filter(|key| headers(key).is_some())
            .count();
        let dirty_count = self
            .records
            .keys()
            .filter(|key| headers(key).is_some() && self.invalidation(key).is_some())
            .count();
        let mut dirty = Vec::with_capacity(dirty_count);
        for key in self.records.keys().filter(|key| headers(key).is_some()) {
            if let Some(why) = self.invalidation(key) {
                dirty.push((*key, why));
            }
        }
        let mut records = Vec::with_capacity(count);
        for (key, mut record) in std::mem::take(&mut self.records) {
            if let Some(reads) = headers(&key) {
                debug_assert_eq!(&record.reads, reads);
                record.reads = reads.clone();
                records.push((key, record));
            }
        }
        SourceHeaderGraph {
            revision: self.revision,
            last_changed: self.last_changed,
            records,
            dirty,
            stats: self.stats,
            touched_memos: self.touched_memos,
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::memo::{ChangeSet, PackageKey, ReadsBuilder};
    use hirsel_syntax::DefPath;

    #[test]
    fn source_projection_keeps_header_invalidation_without_body_indexes_or_work() {
        let header = MemoKey::new(QueryKind::Signature, DefId::intern(&DefPath::core("kept")));
        let universe = ReadKey::ValidationUniverse(PackageKey::of(&header.def.path().0.package));
        let mut builder = ReadsBuilder::new();
        builder.record(ReadKey::Environment, ContentHash::default());
        builder.record(universe, ContentHash::default());
        let reads = Arc::new(builder.finish().unwrap());
        for width in [1, 4, 16, 64] {
            let bodies: Vec<_> = (0..width)
                .map(|i| {
                    MemoKey::new(
                        QueryKind::Check,
                        DefId::intern(&DefPath::core(&format!("body{i}"))),
                    )
                })
                .collect();
            let ((graph, bound), allocated) = crate::test_alloc::measured(|| {
                let mut graph = MemoGraph::default();
                graph.begin_touch_tracking();
                graph.adopt_value(
                    header,
                    ContentHash::default(),
                    Ok(Arc::new(0u8)),
                    reads.clone(),
                    Durability::Low,
                );
                for key in &bodies {
                    graph.adopt_value(
                        *key,
                        ContentHash::default(),
                        Ok(Arc::new(0u8)),
                        reads.clone(),
                        Durability::Low,
                    );
                }
                let mut changes = ChangeSet::default();
                changes.read(universe, Durability::Low);
                graph.apply(&changes);
                let stats = graph.stats().clone();
                let touched = graph.touched_memos.clone();
                let choose = |key: &MemoKey| (*key == header).then_some(&reads);
                let (bound, inspection) = crate::test_alloc::measured_independent(|| {
                    graph.source_headers_bytes(choose).unwrap()
                });
                assert_eq!(inspection.allocations, 0);
                assert_eq!(graph.stats(), &stats);
                let graph = graph.into_source_headers(choose);
                assert_eq!(graph.stats, stats);
                assert_eq!(graph.touched_memos, touched);
                (graph, bound)
            });
            assert!(
                allocated.live as u64 <= bound,
                "{} > {bound}",
                allocated.live
            );
            assert_eq!(graph.records.len(), 1);
            let graph = graph.restore();
            assert!(graph.is_dirty(&header));
            assert_eq!(
                graph.subscribers(&universe).copied().collect::<Vec<_>>(),
                [header]
            );
            assert!(bodies.iter().all(|key| !graph.records.contains_key(key)));
            assert!(Arc::ptr_eq(&graph.records[&header].reads, &reads));
        }
    }
}

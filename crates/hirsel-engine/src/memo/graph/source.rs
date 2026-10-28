//! Source carry retains the reverse reads of its actual header table. Body
//! records have no retained query value and cannot validate a later body.
use super::*;

// The same Rust std/hashbrown capacity and layout bound as retained_bytes.
fn hash<K, V>(len: usize) -> Option<usize> {
    if len == 0 {
        return Some(0);
    }
    len.checked_mul(8)?
        .div_ceil(7)
        .checked_next_power_of_two()?
        .max(4)
        .checked_mul(size_of::<(K, V)>().checked_add(1)?)?
        .checked_add(16)?
        .checked_add(align_of::<(K, V)>())
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
                let choose = |key: &MemoKey| (*key == header).then_some(&reads);
                let (bound, inspection) = crate::test_alloc::measured_independent(|| {
                    graph.source_headers_bytes(choose).unwrap()
                });
                assert_eq!(inspection.allocations, 0);
                assert_eq!(graph.stats(), &stats);
                let graph = graph.into_source_headers(choose);
                assert_eq!(graph.stats(), &stats);
                (graph, bound)
            });
            assert!(
                allocated.live as u64 <= bound,
                "{} > {bound}",
                allocated.live
            );
            assert!(graph.is_dirty(&header));
            assert_eq!(graph.records.len(), 1);
            assert_eq!(graph.subscribers(&universe).copied().collect::<Vec<_>>(), [header]);
            assert!(bodies.iter().all(|key| !graph.records.contains_key(key)));
            assert!(Arc::ptr_eq(&graph.records[&header].reads, &reads));
        }
    }
}
fn set(len: usize) -> Option<usize> {
    hash::<MemoKey, ()>(len)
}
fn shared_set(len: usize) -> Option<usize> {
    size_of::<HashSet<MemoKey>>()
        .checked_add(2 * size_of::<usize>())?
        .checked_add(set(len)?)
}
fn selected_set(keys: &HashSet<MemoKey>, keep: impl Fn(&MemoKey) -> bool) -> HashSet<MemoKey> {
    let mut selected = HashSet::with_capacity(keys.iter().filter(|key| keep(key)).count());
    selected.extend(keys.iter().filter(|key| keep(key)).copied());
    selected
}

impl MemoGraph {
    /// Complete projected index backing. The matching header entries already
    /// own the read Arcs and payloads; projection reuses those exact owners.
    /// This inspection allocates nothing and changes no memo or work counter.
    pub(crate) fn source_headers_bytes<'a>(
        &self,
        headers: impl Fn(&MemoKey) -> Option<&'a Arc<Reads>>,
    ) -> Option<u64> {
        let keep = |key: &MemoKey| headers(key).is_some() && self.records.contains_key(key);
        let records = self.records.keys().filter(|key| keep(key)).count();
        let subscribers = self
            .subscribers
            .values()
            .filter(|keys| keys.iter().any(&keep))
            .count();
        let contexts = self
            .validation_contexts
            .keys()
            .filter(|read| {
                self.subscribers
                    .get(read)
                    .is_some_and(|keys| keys.iter().any(&keep))
            })
            .count();
        let definitions = self
            .by_def
            .values()
            .filter(|keys| keys.iter().any(&keep))
            .count();
        let dirty = self.dirty.keys().filter(|key| keep(key)).count();
        let mut bytes = size_of::<Self>()
            .checked_add(hash::<MemoKey, Record>(records)?)?
            .checked_add(hash::<ReadKey, Arc<HashSet<MemoKey>>>(subscribers)?)?
            .checked_add(hash::<ReadKey, ContextInvalidation>(contexts)?)?
            .checked_add(hash::<DefId, HashSet<MemoKey>>(definitions)?)?
            .checked_add(hash::<MemoKey, Invalidation>(dirty)?)?;
        for keys in self.subscribers.values() {
            let count = keys.iter().filter(|key| keep(key)).count();
            if count != 0 {
                bytes = bytes.checked_add(shared_set(count)?)?;
            }
        }
        for (read, context) in &self.validation_contexts {
            if self
                .subscribers
                .get(read)
                .is_none_or(|keys| !keys.iter().any(&keep))
            {
                continue;
            }
            bytes =
                bytes.checked_add(set(context.confirmed.iter().filter(|k| keep(k)).count())?)?;
            if let Some(pending) = &context.pending {
                bytes =
                    bytes.checked_add(shared_set(pending.iter().filter(|k| keep(k)).count())?)?;
            }
        }
        for keys in self.by_def.values() {
            let count = keys.iter().filter(|key| keep(key)).count();
            if count != 0 {
                bytes = bytes.checked_add(set(count)?)?;
            }
        }
        if let Some(touched) = &self.touched_memos {
            bytes = bytes.checked_add(set(touched.capacity())?)?;
        }
        let mut stats = crate::NativeMetadataWeight::default();
        stats.tree::<QueryKind, u64>(self.stats.recomputed.len());
        (bytes as u64).checked_add(stats.bytes())
    }

    /// Project under the source carry's admitted reservation. Existing public
    /// header/read owners remain unchanged. No query is verified or executed.
    pub(crate) fn into_source_headers<'a>(
        self,
        headers: impl Fn(&MemoKey) -> Option<&'a Arc<Reads>>,
    ) -> Self {
        let Self {
            revision,
            last_changed,
            records,
            subscribers,
            validation_contexts,
            by_def,
            dirty,
            stats,
            touched_memos,
        } = self;
        let mut retained =
            HashMap::with_capacity(records.keys().filter(|key| headers(key).is_some()).count());
        for (key, mut record) in records {
            if let Some(reads) = headers(&key) {
                debug_assert_eq!(&record.reads, reads);
                record.reads = reads.clone();
                retained.insert(key, record);
            }
        }
        let keep = |key: &MemoKey| retained.contains_key(key);
        let mut selected_subscribers = HashMap::with_capacity(
            subscribers
                .values()
                .filter(|keys| keys.iter().any(&keep))
                .count(),
        );
        for (read, keys) in &subscribers {
            if keys.iter().any(&keep) {
                selected_subscribers.insert(*read, Arc::new(selected_set(keys, &keep)));
            }
        }
        let mut contexts = HashMap::with_capacity(
            validation_contexts
                .keys()
                .filter(|read| selected_subscribers.contains_key(read))
                .count(),
        );
        for (read, context) in validation_contexts {
            let Some(current) = selected_subscribers.get(&read) else {
                continue;
            };
            let pending = context.pending.map(|pending| {
                if subscribers
                    .get(&read)
                    .is_some_and(|old| Arc::ptr_eq(old, &pending))
                {
                    current.clone()
                } else {
                    Arc::new(selected_set(&pending, &keep))
                }
            });
            contexts.insert(
                read,
                ContextInvalidation {
                    pending,
                    confirmed: selected_set(&context.confirmed, &keep),
                },
            );
        }
        let mut definitions = HashMap::with_capacity_and_hasher(
            by_def
                .values()
                .filter(|keys| keys.iter().any(&keep))
                .count(),
            IdHasher::new(),
        );
        for (definition, keys) in by_def {
            if keys.iter().any(&keep) {
                definitions.insert(definition, selected_set(&keys, &keep));
            }
        }
        let mut selected_dirty =
            HashMap::with_capacity(dirty.keys().filter(|key| keep(key)).count());
        selected_dirty.extend(dirty.into_iter().filter(|(key, _)| keep(key)));
        // Work already observed remains observable after carrying the source heads.
        let touched_memos = touched_memos;
        Self {
            revision,
            last_changed,
            records: retained,
            subscribers: selected_subscribers,
            validation_contexts: contexts,
            by_def: definitions,
            dirty: selected_dirty,
            stats,
            touched_memos,
        }
    }
}

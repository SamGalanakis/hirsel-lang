use crate::{TypeRecords, indexed_types};
use bincode::Options;
use hirsel_span::{Anchor, AnchorCodec};
use hirsel_syntax::DefPath;
use hirsel_types::TypeId;
use serde::Serialize;
use std::collections::{HashMap, HashSet};

const FIXED_WORKING: u64 = 1024;
const ROOT_WORKING: u64 = 128;
const ANCHOR_WORKING: u64 = 1024;
const TYPE_NODE_WORKING: u64 = 4096;
const TYPE_EDGE_WORKING: u64 = 512;
const TYPE_LABEL_COPIES: u64 = 32;
const SOURCE_COPIES: u64 = 32;
const PAYLOAD_COPIES: u64 = 4;

pub struct RecordEncoding<'a> {
    records: &'a TypeRecords,
    limit: u64,
    required: u64,
    payload: u64,
    roots: usize,
    owners: Vec<(Anchor, DefPath, u32)>,
    nodes: HashMap<TypeId, u64>,
}

fn serialized_size<T: Serialize + ?Sized>(value: &T, limit: u64) -> Option<u64> {
    bincode::DefaultOptions::new()
        .with_fixint_encoding()
        .with_limit(limit)
        .serialized_size(value)
        .ok()
}

fn collect<T: Serialize>(
    value: &T,
    roots: usize,
    anchors: usize,
    limit: u64,
) -> Option<(u64, Vec<TypeId>, Vec<Anchor>)> {
    let (result, roots) = hirsel_types::with_bounded_table_collection(roots, || {
        hirsel_span::with_bounded_anchor_collection(anchors, || {
            indexed_types::inline_names(|| serialized_size(value, limit))
        })
    })?;
    let (size, anchors) = result?;
    Some((size?, roots, anchors))
}

fn fixed_working() -> Option<u64> {
    FIXED_WORKING
        .checked_add(hirsel_types::collection_scope_working_bytes(4)?)?
        .checked_add(hirsel_syntax::def_path_scope_working_bytes(4)?)?
        .checked_add(hirsel_intern::persist::symbol_scope_working_bytes(4)?)
}

impl TypeRecords {
    pub fn prepare_record_encoding<'a, 'source, T: Serialize>(
        &'a self,
        value: &T,
        mut owner: impl FnMut(Anchor) -> Option<(&'source DefPath, u32)>,
        limit: u64,
        serialization_working: u64,
    ) -> Option<RecordEncoding<'a>> {
        if limit < FIXED_WORKING || serialization_working > limit.checked_sub(FIXED_WORKING)? {
            return None;
        }
        let fixed = fixed_working()?.checked_add(serialization_working)?;
        let slots = usize::try_from(limit.checked_sub(fixed)? / ROOT_WORKING).ok()?;
        let (payload, roots, anchors) = collect(
            value,
            slots,
            slots,
            hirsel_artifact_store::MAX_RECORD as u64,
        )?;
        let required = fixed
            .checked_add(payload.checked_mul(PAYLOAD_COPIES)?)?
            .checked_add((roots.len() as u64).checked_mul(ROOT_WORKING)?)?
            .checked_add((anchors.len() as u64).checked_mul(ANCHOR_WORKING)?)?;
        if required > limit {
            return None;
        }
        let mut encoding = RecordEncoding {
            records: self,
            limit,
            required,
            payload,
            roots: roots.len(),
            owners: Vec::with_capacity(anchors.len()),
            nodes: HashMap::new(),
        };
        for anchor in anchors {
            let (path, occurrence) = owner(anchor)?;
            let bytes = indexed_types::inline_names(|| {
                serialized_size(
                    &(path, occurrence),
                    hirsel_artifact_store::MAX_RECORD as u64,
                )
            })?;
            encoding.require(bytes.checked_mul(SOURCE_COPIES)?)?;
            encoding.owners.push((anchor, path.clone(), occurrence));
        }
        let mut pending = roots;
        while let Some(id) = pending.pop() {
            if encoding.nodes.contains_key(&id) {
                continue;
            }
            let row = self.types.get_if_loaded(id)?;
            let label = indexed_types::inline_names(|| {
                hirsel_types::with_session_ids(|| {
                    serialized_size(row, hirsel_artifact_store::MAX_RECORD as u64)
                })
            })?;
            let mut edges = Some(0u64);
            row.kind.visit_graph_edges(|_| {
                edges = edges.and_then(|count| count.checked_add(1));
            });
            encoding.require(
                TYPE_NODE_WORKING
                    .checked_add(edges?.checked_mul(TYPE_EDGE_WORKING)?)?
                    .checked_add(label.checked_mul(TYPE_LABEL_COPIES)?)?,
            )?;
            encoding.nodes.insert(id, label);
            row.kind.visit_graph_edges(|edge| pending.push(edge.ty));
        }
        Some(encoding)
    }
}

impl RecordEncoding<'_> {
    fn require(&mut self, bytes: u64) -> Option<()> {
        let required = self.required.checked_add(bytes)?;
        if required > self.limit {
            return None;
        }
        self.required = required;
        Some(())
    }

    fn type_table_bytes(
        &self,
        root: TypeId,
        seen: &mut HashSet<TypeId>,
        pending: &mut Vec<TypeId>,
    ) -> Option<u64> {
        seen.clear();
        pending.clear();
        pending.push(root);
        let mut bytes = 8u64;
        while let Some(id) = pending.pop() {
            if !seen.insert(id) {
                continue;
            }
            bytes = bytes.checked_add(*self.nodes.get(&id)?)?;
            if bytes > self.limit {
                return None;
            }
            self.records
                .types
                .get_if_loaded(id)?
                .kind
                .visit_graph_edges(|edge| pending.push(edge.ty));
        }
        Some(bytes)
    }

    pub fn capture<T>(
        &mut self,
        physical_bytes: u64,
        type_tables: impl IntoIterator<Item = TypeId>,
        capture: impl FnOnce() -> T,
    ) -> Option<T> {
        self.require(physical_bytes)?;
        let mut seen = HashSet::new();
        let mut pending = Vec::new();
        for root in type_tables {
            let bytes = self.type_table_bytes(root, &mut seen, &mut pending)?;
            self.require(bytes.checked_mul(4)?.checked_add(ROOT_WORKING)?)?;
        }
        drop((seen, pending));
        Some(indexed_types::inline_names(capture))
    }

    pub fn encode<T: Serialize>(self, value: &T) -> Option<Vec<u8>> {
        let (size, roots, anchors) = collect(value, self.roots, self.owners.len(), self.payload)?;
        if !roots.iter().all(|id| self.nodes.contains_key(id))
            || !anchors
                .iter()
                .all(|anchor| self.owners.iter().any(|(id, ..)| id == anchor))
        {
            return None;
        }
        let records = TypeRecords::new(self.records.types.clone(), self.records.store.clone());
        records
            .indexed_record(
                &roots,
                AnchorCodec::Collect(anchors),
                |anchor| {
                    self.owners
                        .iter()
                        .find(|(id, ..)| *id == anchor)
                        .map(|(_, owner, occurrence)| (owner.clone(), *occurrence))
                },
                || {
                    let mut payload = Vec::with_capacity(usize::try_from(size).ok()?);
                    bincode::serialize_into(&mut payload, value).ok()?;
                    Some(payload)
                },
            )
            .map(|(bytes, _)| bytes)
    }
}

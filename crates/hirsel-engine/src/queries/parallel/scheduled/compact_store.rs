//! Ordinary body-free P projections use the same canonical facts as hot carry.
//! Source text and all reads must still match; Lower retains independent V.
use super::*;
use bincode::Options;

const OUTPUT: Domain = Domain {
    kind: 0x4600_0037,
    schema: 1,
};
#[derive(serde::Serialize, serde::Deserialize)]
struct Record {
    key: Key,
    body: Reference,
    reads: Reads,
    decode_bytes: u64,
    facts: Vec<u8>,
}
#[derive(serde::Serialize)]
struct RecordRef<'a> {
    key: Key,
    body: Reference,
    reads: &'a Reads,
    decode_bytes: u64,
    facts: &'a [u8],
}
fn address(key: Key) -> hirsel_artifact_store::Address {
    hirsel_artifact_store::Address(ContentHash::of_parts(
        "hirsel.front.compact.address.v1",
        [key.0.as_slice()],
    ))
}
impl BodyTask {
    pub(super) fn store_compact(
        &self,
        body: Reference,
        reads: &Reads,
        facts: &[u8],
        decode_bytes: u64,
    ) {
        let queries = &self.queries;
        if !queries.store.query_cache_enabled() {
            return;
        }
        let record = RecordRef {
            key: self.input,
            body,
            reads,
            decode_bytes,
            facts,
        };
        let Ok(encoded) = bincode::serialized_size(&record) else {
            return;
        };
        if encoded
            .checked_mul(8)
            .and_then(|outer| outer.checked_add(decode_bytes))
            .is_none_or(|peak| peak > self.weight.scratch)
        {
            return;
        }
        let Ok(bytes) = bincode::serialize(&record) else {
            return;
        };
        let input = &queries.inputs[&self.input];
        let action = crate::store::query_action(&input.key, input.key.1, reads);
        queries
            .store
            .cache_product(address(self.input), action, OUTPUT, &bytes);
    }

    pub(super) fn load_compact(&self) -> Option<Result<ProductRef, Reference>> {
        let queries = &self.queries;
        if !queries.store.query_cache_enabled() {
            return None;
        }
        let selected = queries.store.lookup_product(address(self.input))?;
        if selected.reference.domain != OUTPUT
            || selected.reference.len.checked_mul(8)? > self.weight.scratch
        {
            return None;
        }
        let bytes = queries.store.get_content(selected.reference)?;
        let record: Record = bincode::DefaultOptions::new()
            .with_fixint_encoding()
            .with_limit(selected.reference.len)
            .reject_trailing_bytes()
            .deserialize(&bytes)
            .ok()?;
        if record.key != self.input
            || record.body.domain != crate::store::product_domain(QueryKind::Modes)
            || record.body.len > self.weight.scratch
            || selected
                .reference
                .len
                .checked_mul(8)?
                .checked_add(record.decode_bytes)?
                > self.weight.scratch
        {
            return None;
        }
        let input = &queries.inputs[&self.input];
        if selected.action != crate::store::query_action(&input.key, input.key.1, &record.reads)
            || record
                .reads
                .first_changed(|read| queries.observe(input.definition.path(), read))
                .is_some()
        {
            return None;
        }
        let reads = Arc::new(record.reads);
        let facts = self.decode_facts(&record.facts, &reads)?;
        self.install_cached_facts(record.body, reads, facts)
    }
}

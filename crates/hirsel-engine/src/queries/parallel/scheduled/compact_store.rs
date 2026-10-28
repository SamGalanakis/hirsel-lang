//! Ordinary body-free P projections use the same canonical facts as hot carry.
//! Source text and all reads must still match; Lower retains independent V.
use super::*;
use bincode::Options;

const OUTPUT: Domain = Domain {
    kind: 0x4600_0037,
    schema: 2,
};
#[derive(serde::Serialize, serde::Deserialize)]
struct Record {
    key: Key,
    body: Reference,
    reads: Reads,
    decode_bytes: u64,
    #[serde(
        serialize_with = "hirsel_product_codec::serialize_payload",
        deserialize_with = "hirsel_product_codec::deserialize_payload"
    )]
    facts: Vec<u8>,
}
#[derive(serde::Serialize)]
struct RecordRef<'a> {
    key: Key,
    body: Reference,
    reads: &'a Reads,
    decode_bytes: u64,
    #[serde(serialize_with = "hirsel_product_codec::serialize_payload")]
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
        let completed = self.install_cached_facts(record.body, reads.clone(), facts)?;
        if completed.is_ok() {
            self.remember_cached_carry(record.body, reads, record.facts, record.decode_bytes);
        }
        Some(completed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compact_fact_bytes_preserve_the_stored_sequence_and_its_bounds() {
        #[derive(serde::Serialize, serde::Deserialize)]
        struct Legacy {
            key: Key,
            body: Reference,
            reads: Reads,
            decode_bytes: u64,
            facts: Vec<u8>,
        }
        for len in [0, 1, 127, 8192] {
            let key = ContentHash::of_bytes(b"compact-owner");
            let legacy = Legacy {
                key,
                body: crate::store::product_domain(QueryKind::Modes)
                    .reference(b"body")
                    .unwrap(),
                reads: Reads::from_vec(vec![(ReadKey::Environment, key)]).unwrap(),
                decode_bytes: 12345,
                facts: (0..len).map(|n| n as u8).collect(),
            };
            let bytes = bincode::serialize(&legacy).unwrap();
            let options = || {
                bincode::DefaultOptions::new()
                    .with_fixint_encoding()
                    .with_limit(bytes.len() as u64)
                    .reject_trailing_bytes()
            };
            let record: Record = options().deserialize(&bytes).unwrap();
            assert_eq!(record.facts, legacy.facts);
            assert_eq!(record.reads, legacy.reads);
            assert_eq!(bincode::serialize(&record).unwrap(), bytes);
            let borrowed = RecordRef {
                key: record.key,
                body: record.body,
                reads: &record.reads,
                decode_bytes: record.decode_bytes,
                facts: &record.facts,
            };
            assert_eq!(bincode::serialize(&borrowed).unwrap(), bytes);
            for end in 0..bytes.len() {
                assert!(options().deserialize::<Record>(&bytes[..end]).is_err());
            }
            let mut trailing = bytes.clone();
            trailing.push(0);
            assert!(options().deserialize::<Record>(&trailing).is_err());
        }
    }
}

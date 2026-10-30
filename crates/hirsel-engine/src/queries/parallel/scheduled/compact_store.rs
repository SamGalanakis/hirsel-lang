//! Ordinary body-free P projections use the same canonical facts as hot carry.
//! Source text and all reads must still match; Lower retains independent V.
use super::*;
use bincode::Options;

const OUTPUT: Domain = Domain {
    kind: 0x4600_0037,
    schema: 5,
};
#[derive(serde::Serialize, serde::Deserialize)]
struct Record {
    key: Key,
    body: Option<Reference>,
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
    body: Option<Reference>,
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
/// Scalar Summary demand consumes the same checked compact facts written by
/// scheduled P. It does not reconstruct or execute the separate body product.
pub(in crate::queries) fn load_publication(
    engine: &mut QueryEngine,
    path: &DefPath,
) -> Option<PublishedResult> {
    if engine.verify_store == crate::store::VerifyStore::Recompute {
        return None;
    }
    let store = engine
        .result_store
        .clone()
        .filter(|s| s.query_cache_enabled())?;
    let key = engine.store_key(path, QueryKind::Modes)?;
    let selected = store.lookup_product(address(key.0))?;
    if selected.reference.domain != OUTPUT
        || selected.reference.len > crate::store::MAX_RECORD as u64
    {
        return None;
    }
    let bytes = store.get_content(selected.reference)?;
    let record: Record = bincode::DefaultOptions::new()
        .with_fixint_encoding()
        .with_limit(selected.reference.len)
        .reject_trailing_bytes()
        .deserialize(&bytes)
        .ok()?;
    if record.key != key.0
        || record.body.is_some_and(|body| {
            body.domain != crate::store::product_domain(QueryKind::Modes)
                || body.len > crate::store::MAX_RECORD as u64
        })
        || selected
            .reference
            .len
            .checked_mul(8)?
            .checked_add(record.decode_bytes)?
            > (crate::store::MAX_RECORD as u64).checked_mul(8)?
        || selected.action != crate::store::query_action(&key, key.1, &record.reads)
        || record
            .reads
            .first_changed(|read| engine.observe(path, read))
            .is_some()
    {
        return None;
    }
    let facts: carry::Facts = engine.decode_record(&record.facts)?;
    let summary = facts.summary?;
    if summary.storage.is_some() || has_errors(&facts.diagnostics) {
        return None;
    }
    let checked = facts.checked?;
    engine.adopt_publication(
        path,
        persistent::PublicationRecord {
            summary,
            diagnostics: facts.diagnostics,
            phases: facts.diagnostic_phases,
            detail: DependencyReads {
                signatures: checked.2.values()?,
                ..Default::default()
            },
            dead_arms: facts.dead_arms,
            lowering: facts.lowering,
        },
        Arc::new(record.reads),
        (checked.0, checked.1),
    )
}

impl BodyTask {
    pub(super) fn store_compact(
        &self,
        body: Option<Reference>,
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

    pub(super) fn load_compact(
        &self,
        cx: &mut dyn QueryContext,
    ) -> Option<Result<ProductRef, Reference>> {
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
            || record.body.is_some_and(|body| {
                body.domain != crate::store::product_domain(QueryKind::Modes)
                    || body.len > self.weight.scratch
            })
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
        let completed = self.install_cached_facts(cx, record.body, reads.clone(), facts)?;
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
    fn scalar_summary_adopts_checked_facts_without_a_body_and_refuses_substitutions() {
        for verify in [
            crate::store::VerifyStore::Off,
            crate::store::VerifyStore::Recompute,
        ] {
            for fault in 0..10 {
                let store = Arc::new(crate::store::MemStore::default());
                let mut engine =
                    QueryEngine::new(crate::default_environment(), PhaseAdapters::default())
                        .with_result_store(store.clone());
                let file = engine
                    .sources
                    .add("summary-cache.hir", "fn main(){}".into())
                    .unwrap();
                let tokens = hirsel_lexer::lex(file, &engine.sources).tokens;
                let context = crate::default_context();
                let mut parsed = hirsel_parser::parse(file, tokens, &context, &mut engine.sources);
                assert!(parsed.diagnostics.is_empty());
                engine.index(&mut parsed.syntax, &context).unwrap();
                let syntax = &parsed.syntax.items[0];
                let path = &syntax.item.definition;
                let key = engine.store_key(path, QueryKind::Modes).unwrap();
                let checked_summary = Arc::new(CheckedSummary {
                    summary: DefinitionSummary {
                        owner: syntax.owner().def,
                        frames: vec![crate::summary::Frame {
                            site: hirsel_types::NameSite::Item,
                            parent: None,
                            kind: crate::summary::FrameKind::Item,
                            generic: false,
                            size_bindings: Box::new([]),
                        }]
                        .into(),
                        calls: [].into(),
                        staged: [].into(),
                    },
                    growth: Default::default(),
                });
                let reads = Arc::new(Reads::empty());
                let mut summary = PublishedSummary {
                    definition: checked_summary.summary.owner,
                    summary: checked_summary.summary.clone(),
                    growth: checked_summary.growth.clone(),
                    storage: None,
                    checked: Some(checked_summary.clone()),
                    reads: reads.clone(),
                    text: engine.key_text(path, QueryKind::Summary).unwrap(),
                };
                let mut checked = Some((
                    checked_summary,
                    reads.clone(),
                    Arc::new(OrderedSignatureReads::new(&[])),
                ));
                match fault {
                    1 => summary.text = ContentHash::default(),
                    2 => summary.checked = None,
                    3 => summary.summary.frames[0].parent = Some(0),
                    4 => checked = None,
                    5 => {
                        Arc::make_mut(&mut checked.as_mut().unwrap().0)
                            .summary
                            .owner = DefId::intern(&DefPath::core("substituted"))
                    }
                    _ => (),
                }
                let facts = carry::FactsRef {
                    summary: Some(Arc::new(summary)),
                    checked: &checked,
                    dead_arms: &None,
                    diagnostics: &vec![],
                    diagnostic_phases: &vec![],
                    lowering: &None,
                    table_keys: None,
                };
                let mut record = Record {
                    key: key.0,
                    // Body-free publication must not fetch this absent archive.
                    body: Some(
                        crate::store::product_domain(QueryKind::Modes)
                            .reference(b"absent body")
                            .unwrap(),
                    ),
                    reads: Reads::empty(),
                    decode_bytes: 0,
                    facts: engine.encode_record(&facts).unwrap(),
                };
                if fault == 6 {
                    record.reads =
                        Reads::from_vec(vec![(ReadKey::Environment, ContentHash::default())])
                            .unwrap();
                }
                if fault == 8 {
                    record.decode_bytes = u64::MAX;
                }
                if fault == 9 {
                    record.body.as_mut().unwrap().domain = OUTPUT;
                }
                let action = if fault == 7 {
                    hirsel_artifact_store::Action(ContentHash::default())
                } else {
                    crate::store::query_action(&key, key.1, &record.reads)
                };
                store.cache_product(
                    address(key.0),
                    action,
                    OUTPUT,
                    &bincode::serialize(&record).unwrap(),
                );
                assert!(store.get_content(record.body.unwrap()).is_none());
                engine.verify_store = verify;
                let hit = engine.load_publication(path);
                let accepted = fault == 0 && verify == crate::store::VerifyStore::Off;
                assert_eq!(hit.is_some(), accepted, "fault {fault}, {verify:?}");
                assert_eq!(engine.store_adoptions(), u64::from(accepted));
                assert_eq!(engine.summaries.is_empty(), !accepted);
                assert_eq!(engine.checked_summaries.is_empty(), !accepted);
                for kind in [
                    QueryKind::Resolve,
                    QueryKind::Check,
                    QueryKind::Modes,
                    QueryKind::Summary,
                ] {
                    assert_eq!(engine.recompute_count(path, kind), 0);
                }
            }
        }
    }

    #[test]
    fn compact_fact_bytes_preserve_the_stored_sequence_and_its_bounds() {
        #[derive(serde::Serialize, serde::Deserialize)]
        struct Legacy {
            key: Key,
            body: Option<Reference>,
            reads: Reads,
            decode_bytes: u64,
            facts: Vec<u8>,
        }
        for len in [0, 1, 127, 8192] {
            let key = ContentHash::of_bytes(b"compact-owner");
            let legacy = Legacy {
                key,
                body: Some(
                    crate::store::product_domain(QueryKind::Modes)
                        .reference(b"body")
                        .unwrap(),
                ),
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

//! Retained P allocations; interned identities are owned by the session.
//! Wire tables can repeat path spellings and shared reads, so their length is
//! neither this representation's storage size nor an allocation allowance.
use super::*;
use crate::memo::retained::Weight;
pub(super) fn definition(weight: &mut Weight, summary: &DefinitionSummary) {
    weight.add(size_of_val(&*summary.frames));
    for frame in &summary.frames {
        weight.add(size_of_val(&*frame.size_bindings));
    }
    weight.add(size_of_val(&*summary.calls));
    for call in &summary.calls {
        weight.add(size_of_val(&*call.arguments));
    }
    weight.add(size_of_val(&*summary.staged));
    for (_, parameters) in &summary.staged {
        weight.add(size_of_val(&**parameters));
    }
}
pub(super) fn checked(weight: &mut Weight, summary: &CheckedSummary) {
    definition(weight, &summary.summary);
    weight.add(size_of_val(&*summary.growth.edges));
}
#[cfg(test)]
pub(super) fn metadata(
    outcome: &ModeResult,
    checked_product: &CheckedProduct,
    table_keys: Option<&Arc<crate::table_keys::TableKeyUse>>,
    memos: &[(crate::memo::MemoKey, Arc<Reads>)],
) -> Option<u64> {
    metadata_inner(outcome, checked_product, table_keys, memos, false)
}
pub(super) fn compact_metadata(
    outcome: &ModeResult,
    checked_product: &CheckedProduct,
    table_keys: Option<&Arc<crate::table_keys::TableKeyUse>>,
    memos: &[(crate::memo::MemoKey, Arc<Reads>)],
) -> Option<u64> {
    metadata_inner(outcome, checked_product, table_keys, memos, true)
}
fn metadata_inner(
    outcome: &ModeResult,
    checked_product: &CheckedProduct,
    table_keys: Option<&Arc<crate::table_keys::TableKeyUse>>,
    memos: &[(crate::memo::MemoKey, Arc<Reads>)],
    compact: bool,
) -> Option<u64> {
    // Storage equations have their own admitted producer. Ordinary P never
    // carries a storage graph; reject unfamiliar retained payloads explicitly.
    if outcome
        .summary
        .as_ref()
        .is_some_and(|s| s.storage.is_some())
    {
        return None;
    }
    let mut weight = Weight::default();
    // Compact front rows retain the session's DefId, not this temporary path.
    if !compact {
        weight.def_path(&outcome.definition);
    }
    if let Err(error) = &outcome.result {
        weight.error(error);
    }
    if let Ok(output) = &outcome.result {
        weight.vec(&output.diagnostics, Weight::diagnostic);
        weight.add(lowering::LoweringUse::capture_bytes(&output.value.typed));
    }
    weight.reads(&outcome.reads);
    weight.vec(&outcome.diagnostic_phases, |w, (diagnostic, phase)| {
        w.diagnostic(diagnostic);
        w.add(phase.capacity());
    });
    if let Some(summary) = &outcome.summary
        && !(compact && FrontSummary::shareable(summary, checked_product, &outcome.reads))
    {
        weight.arc(summary, |w, summary| {
            definition(w, &summary.summary);
            w.add(size_of_val(&*summary.growth.edges));
            if let Some(value) = &summary.checked {
                w.arc(value, checked);
            }
            w.reads(&summary.reads);
        });
    }
    if let Some((summary, reads)) = checked_product {
        weight.arc(summary, checked);
        weight.reads(reads);
    }
    if let Some(keys) = table_keys {
        weight.arc(keys, |w, keys| keys.retained(w));
    }
    for (_, reads) in memos {
        weight.reads(reads);
    }
    Some(weight.bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn outcome(reads: Arc<Reads>, checked: Arc<CheckedSummary>) -> ModeResult {
        ModeResult {
            definition: DefPath::core("metadata"),
            result: Err(QueryError::NotImplemented),
            phase: Some(QueryKind::Modes),
            summary: Some(Arc::new(PublishedSummary {
                definition: checked.summary.owner,
                summary: checked.summary.clone(),
                growth: checked.growth.clone(),
                checked: Some(checked),
                storage: None,
                reads: reads.clone(),
                text: ContentHash::default(),
            })),
            reads,
            diagnostic_phases: vec![],
        }
    }
    #[test]
    fn compact_front_metadata_owns_interned_definition_identity() {
        let checked = Arc::new(CheckedSummary {
            summary: DefinitionSummary {
                owner: DefId::intern(&DefPath::core("metadata")),
                frames: Box::new([]),
                calls: Box::new([]),
                staged: Box::new([]),
            },
            growth: Default::default(),
        });
        let reads = Arc::new(Reads::empty());
        let mut short = outcome(reads.clone(), checked.clone());
        short.summary = None;
        let selected = Some((checked, reads));
        let mut long = short.clone();
        long.definition = DefPath::core(&"long".repeat(4096));
        assert_eq!(
            compact_metadata(&short, &selected, None, &[]),
            compact_metadata(&long, &selected, None, &[])
        );
        assert_eq!(
            FrontBodyQueries::input_bytes(&[short.definition.clone()]),
            FrontBodyQueries::input_bytes(&[long.definition])
        );
        let row = FrontOutcome::capture(
            crate::store::Domain { kind: 1, schema: 1 }
                .reference(b"row")
                .unwrap(),
            short.clone(),
            selected,
            None,
        );
        assert_eq!(row.definition.path(), &short.definition);
    }
    #[test]
    fn compact_summary_reuses_only_identical_checked_facts_and_restores_every_field() {
        let definition = DefId::intern(&DefPath::core("metadata"));
        let checked = Arc::new(CheckedSummary {
            summary: DefinitionSummary {
                owner: definition,
                frames: Box::new([]),
                calls: Box::new([]),
                staged: Box::new([]),
            },
            growth: Default::default(),
        });
        let reads = Arc::new(Reads::empty());
        let output = outcome(reads.clone(), checked.clone());
        let selected = Some((checked.clone(), reads.clone()));
        let original = output.summary.as_ref().unwrap().clone();
        let compact = FrontSummary::capture(original.clone(), &selected, &reads);
        assert!(matches!(compact, FrontSummary::Checked { .. }));
        assert!(
            compact_metadata(&output, &selected, None, &[]).unwrap()
                < metadata(&output, &selected, None, &[]).unwrap()
        );
        let restored = compact.materialize(&selected, &reads);
        assert_eq!(restored.definition, original.definition);
        assert_eq!(restored.text, original.text);
        assert_eq!(restored.summary, original.summary);
        assert_eq!(restored.growth, original.growth);
        assert_eq!(restored.reads, original.reads);
        assert!(Arc::ptr_eq(restored.checked.as_ref().unwrap(), &checked));
        let mut different = (*original).clone();
        different.summary.calls = vec![].into_boxed_slice();
        different.summary.owner = DefId::intern(&DefPath::core("different"));
        let different = Arc::new(different);
        let captured = FrontSummary::capture(different.clone(), &selected, &reads);
        assert!(matches!(captured, FrontSummary::Owned(_)));
        assert!(Arc::ptr_eq(
            &captured.materialize(&selected, &reads),
            &different
        ));
    }
    #[test]
    fn p_metadata_counts_shared_summary_and_reads_once_using_compact_read_layout() {
        let definition = DefId::intern(&DefPath::core("metadata"));
        let checked = Arc::new(CheckedSummary {
            summary: DefinitionSummary {
                owner: definition,
                frames: Box::new([]),
                calls: Box::new([]),
                staged: Box::new([]),
            },
            growth: Default::default(),
        });
        let key = crate::memo::MemoKey::new(QueryKind::Modes, definition);
        let empty = Arc::new(Reads::empty());
        let first = outcome(empty.clone(), checked.clone());
        let base = metadata(&first, &None, None, &[]).unwrap();
        assert_eq!(
            metadata(
                &first,
                &Some((checked.clone(), empty.clone())),
                None,
                &[(key, empty)]
            ),
            Some(base)
        );
        let reads = Arc::new(
            Reads::from_vec(vec![(
                ReadKey::Signature(DefId::intern(&DefPath::core(&"long".repeat(4096)))),
                ContentHash::default(),
            )])
            .unwrap(),
        );
        let second = outcome(reads.clone(), checked.clone());
        let bytes = metadata(
            &second,
            &Some((checked.clone(), reads.clone())),
            None,
            &[(key, reads)],
        )
        .unwrap();
        assert_eq!(
            bytes - base,
            size_of::<(ReadKey, ContentHash)>()
                .next_multiple_of(align_of::<usize>().max(align_of::<(ReadKey, ContentHash)>()))
                as u64,
            "read identities retain interned rows, not repeated wire spellings"
        );
        let independent = Arc::new((*checked).clone());
        let distinct = metadata(
            &second,
            &Some((independent, second.reads.clone())),
            None,
            &[],
        )
        .unwrap();
        assert!(
            distinct > bytes,
            "an independent equal allocation still needs its own debit"
        );
    }
}

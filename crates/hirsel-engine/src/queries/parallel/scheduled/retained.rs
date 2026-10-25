//! Retained P allocations; interned identities are owned by the session.
//! Wire tables can repeat path spellings and shared reads, so their length is
//! neither this representation's storage size nor an allocation allowance.
use super::*;
use crate::memo::retained::Weight;
fn definition(weight: &mut Weight, summary: &DefinitionSummary) {
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
fn checked(weight: &mut Weight, summary: &CheckedSummary) {
    definition(weight, &summary.summary);
    weight.add(size_of_val(&*summary.growth.edges));
}
pub(super) fn metadata(
    outcome: &ModeResult,
    checked_product: &CheckedProduct,
    table_keys: Option<&Arc<crate::table_keys::TableKeyUse>>,
    memos: &[(crate::memo::MemoKey, Arc<Reads>)],
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
    weight.def_path(&outcome.definition);
    if let Err(error) = &outcome.result {
        weight.error(error);
    }
    weight.reads(&outcome.reads);
    weight.vec(&outcome.diagnostic_phases, |w, (diagnostic, phase)| {
        w.diagnostic(diagnostic);
        w.add(phase.capacity());
    });
    if let Some(summary) = &outcome.summary {
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
            size_of::<(ReadKey, ContentHash)>() as u64,
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

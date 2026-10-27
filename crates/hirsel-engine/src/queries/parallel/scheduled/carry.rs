//! Bounded accepted front facts across fresh source graphs. The ordinary
//! type/anchor codec relocates data; no body, source map or controller survives.
use super::*;
use crate::memo::retained::Weight;

struct Row {
    generation: u64,
    owner: DefId,
    text: ContentHash,
    reads: Arc<Reads>,
    reference: Reference,
    bytes: Arc<[u8]>,
    decode_bytes: u64,
    charge: u64,
}
#[derive(Default)]
struct Rows {
    entries: BTreeMap<Key, Row>,
    used: u64,
    generation: u64,
    subscriptions: BTreeSet<(ReadKey, DefId, ContentHash, Key)>,
}
/// Validated subscription generations for one immutable source view. A later
/// graph may replace a shared row, but cannot make this view trust that row.
pub(super) struct CarryChanges {
    clean: BTreeMap<Key, u64>,
}
/// Optional host-owned carry, independently bounded from ordinary body storage.
/// Its rows are data only: current text and every read must validate before
/// adoption; Lower/Run still demand bodies and their ordinary authority.
pub struct FrontCarry {
    rows: Mutex<Rows>,
    allowance: MemoryReservation,
}
impl FrontCarry {
    pub fn new(allowance: MemoryReservation) -> Result<Arc<Self>, MemoryReservation> {
        let header = size_of::<Self>() as u64 + 2 * size_of::<usize>() as u64;
        if allowance.bytes() < header.saturating_add(index_bytes(0, 0)) {
            return Err(allowance);
        }
        Ok(Arc::new(Self {
            rows: Mutex::new(Rows {
                used: header,
                ..Rows::default()
            }),
            allowance,
        }))
    }
    pub fn retained_bytes(&self) -> u64 {
        let rows = self.rows.lock().unwrap_or_else(|e| e.into_inner());
        rows.used
            .saturating_add(index_bytes(rows.entries.len(), rows.subscriptions.len()))
    }
    pub fn len(&self) -> usize {
        self.rows
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .entries
            .len()
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
    fn remember(
        &self,
        key: Key,
        owner: DefId,
        text: ContentHash,
        reads: Arc<Reads>,
        reference: Reference,
        bytes: Vec<u8>,
        decode_bytes: u64,
    ) {
        let mut charge = crate::NativeMetadataWeight::default();
        charge.add(2 * size_of::<usize>() + bytes.len());
        let mut read_weight = Weight::default();
        read_weight.reads(&reads);
        let charge = charge.bytes().saturating_add(read_weight.bytes);
        let mut rows = self.rows.lock().unwrap_or_else(|e| e.into_inner());
        let old = rows.entries.get(&key).map_or(0, |r| r.charge);
        let Some(used) = rows
            .used
            .checked_sub(old)
            .and_then(|n| n.checked_add(charge))
        else {
            return;
        };
        let old_reads = rows.entries.get(&key).map_or(0, |row| row.reads.len());
        let Some(subscriptions) = rows
            .subscriptions
            .len()
            .checked_sub(old_reads)
            .and_then(|len| len.checked_add(reads.len()))
        else {
            return;
        };
        let entries = rows.entries.len() + usize::from(!rows.entries.contains_key(&key));
        if used.saturating_add(index_bytes(entries, subscriptions)) > self.allowance.bytes() {
            return;
        }
        let Some(generation) = rows.generation.checked_add(1) else {
            return;
        };
        if let Some(old) = rows.entries.remove(&key) {
            for (read, value) in old.reads.iter() {
                rows.subscriptions.remove(&(*read, old.owner, *value, key));
            }
        }
        for (read, value) in reads.iter() {
            rows.subscriptions.insert((*read, owner, *value, key));
        }
        rows.generation = generation;
        rows.entries.insert(
            key,
            Row {
                generation,
                owner,
                text,
                reads,
                reference,
                bytes: bytes.into(),
                decode_bytes,
                charge,
            },
        );
        rows.used = used;
    }
}

// Count the complete trees once using Rust's minimum node occupancy bound.
// Charging a whole node for every read makes bounded carry discard most facts.
fn index_bytes(entries: usize, subscriptions: usize) -> u64 {
    let mut weight = crate::NativeMetadataWeight::default();
    weight.tree::<Key, Row>(entries);
    weight.tree::<(ReadKey, DefId, ContentHash, Key), ()>(subscriptions);
    weight.bytes()
}

#[derive(serde::Serialize)]
struct FactsRef<'a> {
    summary: Option<Arc<PublishedSummary>>,
    checked: &'a CheckedProduct,
    dead_arms: &'a Option<crate::tools::DeadArmUse>,
    diagnostics: &'a Vec<Diagnostic>,
    diagnostic_phases: &'a Vec<(Diagnostic, String)>,
    lowering: &'a Option<lowering::LoweringUse>,
    table_keys: Option<&'a Arc<crate::table_keys::TableKeyUse>>,
}
#[derive(serde::Deserialize, serde::Serialize)]
struct Facts {
    summary: Option<Arc<PublishedSummary>>,
    checked: CheckedProduct,
    dead_arms: Option<crate::tools::DeadArmUse>,
    diagnostics: Vec<Diagnostic>,
    diagnostic_phases: Vec<(Diagnostic, String)>,
    lowering: Option<lowering::LoweringUse>,
    table_keys: Option<Arc<crate::table_keys::TableKeyUse>>,
}
impl FactsRef<'_> {
    fn bytes(&self, decoded: bool) -> u64 {
        let mut w = if decoded {
            Weight::decoded()
        } else {
            Weight::default()
        };
        if let Some(summary) = &self.summary {
            w.arc(summary, |w, summary| {
                retained::definition(w, &summary.summary);
                w.add(size_of_val(&*summary.growth.edges));
                if let Some(checked) = &summary.checked {
                    w.arc(checked, retained::checked);
                }
                w.reads(&summary.reads);
            });
        }
        if let Some((summary, reads)) = self.checked {
            w.arc(summary, retained::checked);
            w.reads(reads);
        }
        w.vec(self.diagnostics, Weight::diagnostic);
        w.vec(self.diagnostic_phases, |w, (diagnostic, phase)| {
            w.diagnostic(diagnostic);
            w.add(phase.capacity());
        });
        if let Some(lowering) = self.lowering {
            lowering.retained(&mut w);
        }
        if let Some(keys) = self.table_keys {
            w.arc(keys, |w, keys| keys.retained(w));
        }
        let mut dead = crate::NativeMetadataWeight::default();
        if let Some(uses) = self.dead_arms {
            if !uses.errors.is_empty() {
                dead.tree::<DefPath, ()>(uses.errors.len());
                for path in &uses.errors {
                    dead.path(path);
                }
            }
            if !uses.constructed.is_empty() {
                dead.tree::<(DefPath, u32), ()>(uses.constructed.len());
                for (path, _) in &uses.constructed {
                    dead.path(path);
                }
            }
        }
        w.bytes
            .saturating_add(dead.bytes())
            .saturating_add(size_of::<Facts>() as u64)
    }
}
impl FrontBodyQueries {
    /// Install before submitting tasks. The caller keeps the same carry for
    /// later fresh graphs; it holds neither this producer nor its sources.
    pub fn install_carry(&self, carry: Arc<FrontCarry>) -> Result<(), Arc<FrontCarry>> {
        let mut slot = self.carry.lock().unwrap_or_else(|e| e.into_inner());
        if slot.is_some()
            || !self
                .outcomes
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .is_empty()
        {
            return Err(carry);
        }
        let rows = carry.rows.lock().unwrap_or_else(|e| e.into_inner());
        let mut weight = crate::NativeMetadataWeight::default();
        weight.add(size_of::<CarryChanges>() + 2 * size_of::<usize>());
        weight.tree::<Key, u64>(rows.entries.len());
        let mut retention = self.retention.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(remaining) = retention.checked_sub(weight.bytes()) {
            *retention = remaining;
            drop(retention);
            let mut clean: BTreeMap<_, _> = rows
                .entries
                .iter()
                .map(|(key, row)| (*key, row.generation))
                .collect();
            // Observe the reverse subscription index without visiting body
            // memos. Only subscribers to changed or unknown inputs are dirty.
            let mut previous = None;
            for (read, owner, value, key) in &rows.subscriptions {
                let current = match previous {
                    Some((last_read, last_owner, observed))
                        if last_read == *read && last_owner == *owner =>
                    {
                        observed
                    }
                    _ => {
                        let observed = self.observe(owner.path(), read);
                        previous = Some((*read, *owner, observed));
                        observed
                    }
                };
                if current != Some(*value) {
                    clean.remove(key);
                }
            }
            *self.carry_changes.lock().unwrap_or_else(|e| e.into_inner()) =
                Some(Arc::new(CarryChanges { clean }));
        }
        drop(rows);
        *slot = Some(carry);
        Ok(())
    }
}
impl BodyTask {
    pub(super) fn remember_carry(&self, reference: Reference) {
        let queries = &self.queries;
        let Some(carry) = queries
            .carry
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
        else {
            return;
        };
        let outcomes = queries.outcomes.lock().unwrap_or_else(|e| e.into_inner());
        let Some(outcome) = outcomes.get(&self.input) else {
            return;
        };
        if outcome.status.is_err() {
            return;
        }
        let keys = queries.table_keys.lock().unwrap_or_else(|e| e.into_inner());
        let facts = FactsRef {
            summary: outcome
                .summary
                .as_ref()
                .map(|s| s.materialize(&outcome.checked, &outcome.reads)),
            checked: &outcome.checked,
            dead_arms: &outcome.dead_arms,
            diagnostics: &outcome.diagnostics,
            diagnostic_phases: &outcome.diagnostic_phases,
            lowering: &outcome.lowering,
            table_keys: keys.get(&self.input),
        };
        if facts.summary.as_ref().is_some_and(|s| s.storage.is_some()) {
            return;
        }
        let Some((bytes, outer)) = queries
            .records
            .encode_record_with_decode_bytes(&facts, |anchor| {
                queries.citations.record_owner(anchor)
            })
        else {
            return;
        };
        let Some(decode_bytes) = outer.checked_add(facts.bytes(true)) else {
            return;
        };
        if decode_bytes > self.weight.scratch {
            return;
        }
        carry.remember(
            self.input,
            queries.inputs[&self.input].definition,
            queries.inputs[&self.input].key.1,
            outcome.reads.clone(),
            reference,
            bytes,
            decode_bytes,
        );
    }
    pub(super) fn load_carry(&self) -> Option<Result<ProductRef, Reference>> {
        let queries = &self.queries;
        let carry = queries
            .carry
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()?;
        let changes = queries
            .carry_changes
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        let rows = carry.rows.lock().unwrap_or_else(|e| e.into_inner());
        let row = rows.entries.get(&self.input)?;
        let clean = changes
            .as_ref()
            .is_some_and(|changes| changes.clean.get(&self.input) == Some(&row.generation));
        let input = &queries.inputs[&self.input];
        if row.text != input.key.1
            || row.decode_bytes > self.weight.scratch
            || (!clean
                && row
                    .reads
                    .first_changed(|read| queries.observe(input.definition.path(), read))
                    .is_some())
        {
            return None;
        }
        let facts: Facts = queries
            .records
            .decode_record(&row.bytes, |path, occurrence| {
                queries
                    .citations
                    .owners
                    .record_source_anchor(path, occurrence)
            })?;
        let (encoded, roots) = hirsel_types::with_table_collection(|| {
            bincode::serialize_into(std::io::sink(), &facts)
        });
        encoded.ok()?;
        if !roots.iter().all(|id| queries.types.get(*id).is_some())
            || facts.summary.as_ref().is_none_or(|s| {
                s.definition != input.definition
                    || s.storage.is_some()
                    || s.text != input.key.1
                    || s.reads.as_ref() != row.reads.as_ref()
            })
        {
            return None;
        }
        let mut memo_rows = vec![(
            crate::memo::MemoKey::new(QueryKind::Modes, input.definition),
            row.reads.clone(),
        )];
        if let Some((_, reads)) = &facts.checked {
            memo_rows.push((
                crate::memo::MemoKey::new(QueryKind::Check, input.definition),
                reads.clone(),
            ));
        }
        let view = FactsRef {
            summary: facts.summary.clone(),
            checked: &facts.checked,
            dead_arms: &facts.dead_arms,
            diagnostics: &facts.diagnostics,
            diagnostic_phases: &facts.diagnostic_phases,
            lowering: &facts.lowering,
            table_keys: facts.table_keys.as_ref(),
        };
        let mut weight = Weight::default();
        weight.reads(&row.reads);
        let mut clean_rows = crate::NativeMetadataWeight::default();
        if clean {
            for _ in &memo_rows {
                clean_rows.tree::<crate::memo::MemoKey, ()>(0);
            }
        }
        let charge = view
            .bytes(false)
            .checked_add(weight.bytes)?
            .checked_add(clean_rows.bytes())?;
        let mut retention = queries.retention.lock().unwrap_or_else(|e| e.into_inner());
        let remaining = retention.checked_sub(charge)?;
        *retention = remaining;
        drop(retention);
        // Publication takes the outcomes/table-key locks. Never hold the
        // shared carry lock across them: a completing producer captures in
        // the opposite order, and must be able to publish concurrently.
        let reference = row.reference;
        let reads = row.reads.clone();
        let action = crate::store::query_action(&input.key, input.key.1, &reads);
        drop(rows);
        let summary = facts
            .summary
            .map(|s| FrontSummary::capture(s, &facts.checked, &reads));
        if let Some(keys) = facts.table_keys {
            queries
                .table_keys
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .insert(self.input, keys);
        }
        if clean {
            queries
                .clean_memos
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .extend(memo_rows.iter().map(|(key, _)| *key));
        }
        queries
            .memos
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .extend(memo_rows);
        let outcome = FrontOutcome {
            reference,
            definition: input.definition,
            status: Ok(()),
            phase: None,
            summary,
            reads,
            diagnostic_phases: facts.diagnostic_phases,
            checked: facts.checked,
            dead_arms: facts.dead_arms,
            body: Weak::new(),
            diagnostics: facts.diagnostics,
            lowering: facts.lowering,
        };
        queries
            .outcomes
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(self.input, outcome);
        queries.store_adoptions.fetch_add(1, Relaxed);
        // Carried facts complete this in-process query independently of body
        // storage. Publishing a body association would fetch its archive just
        // to check existence, even though Check needs only these compact facts.
        // A later actual body demand owns materialization and its association.
        timings::count(Count::StoreHits);
        Some(Ok(ProductRef {
            address: self.query.address,
            action,
            record: reference,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hirsel_span::Anchor;

    #[test]
    fn carry_reservation_covers_rows_refuses_n_minus_one_and_lives_through_last_clone() {
        let owner = DefId::intern(&DefPath::core("carry"));
        for width in [1, 4, 16] {
            let keys: Vec<_> = (0..width).map(|i| ContentHash::of_bytes(&[i])).collect();
            let reference = Domain { kind: 1, schema: 1 }.reference(b"body").unwrap();
            let budget = ProcessBudget::new(1 << 20);
            let (carry, allocations) = crate::test_alloc::measured(|| {
                let carry = FrontCarry::new(budget.reserve(1 << 20).unwrap())
                    .ok()
                    .unwrap();
                for key in &keys {
                    carry.remember(
                        *key,
                        owner,
                        *key,
                        Arc::new(Reads::from_vec(vec![(ReadKey::Environment, *key)]).unwrap()),
                        reference,
                        vec![9; 64],
                        1024,
                    );
                }
                carry
            });
            assert_eq!(carry.len(), width as usize);
            let required = carry.retained_bytes();
            assert!(
                allocations.live as u64 <= required,
                "live={} required={required}",
                allocations.live
            );
            let detached = carry.clone();
            drop(carry);
            assert_eq!(budget.reserved(), 1 << 20);
            drop(detached);
            assert_eq!(budget.reserved(), 0);

            let short = FrontCarry::new(budget.reserve(required - 1).unwrap())
                .ok()
                .unwrap();
            for key in &keys {
                short.remember(
                    *key,
                    owner,
                    *key,
                    Arc::new(Reads::from_vec(vec![(ReadKey::Environment, *key)]).unwrap()),
                    reference,
                    vec![9; 64],
                    1024,
                );
            }
            assert!(short.len() < width as usize);
            assert!(short.retained_bytes() <= required - 1);
            drop(short);
            assert_eq!(budget.reserved(), 0);
        }
        let header = (size_of::<FrontCarry>() + 2 * size_of::<usize>()) as u64;
        let budget = ProcessBudget::new(header);
        let (refused, allocations) = crate::test_alloc::measured(|| {
            FrontCarry::new(budget.reserve(header - 1).unwrap())
                .err()
                .unwrap()
        });
        assert_eq!(allocations.allocations, 0);
        assert_eq!(budget.reserved(), header - 1);
        drop(refused);
        assert_eq!(budget.reserved(), 0);
    }

    #[test]
    fn carry_codec_layout_covers_independent_reads_diagnostics_and_summary_decode_peaks() {
        crate::with_compiler_stack(|| {
            for width in [1, 4, 16, 64] {
                let anchor = Anchor::fresh();
                let relocated = Anchor::fresh();
                let owner = DefPath::core("portable");
                let definition = DefId::intern(&owner);
                let reads = Arc::new(
                    Reads::from_vec(
                        (0..width)
                            .map(|i| {
                                (
                                    ReadKey::Signature(DefId::intern(&DefPath::core(&format!(
                                        "carry_read_{i}"
                                    )))),
                                    ContentHash::of_bytes(&[i as u8]),
                                )
                            })
                            .collect(),
                    )
                    .unwrap(),
                );
                let checked = Arc::new(CheckedSummary {
                    summary: DefinitionSummary {
                        owner: definition,
                        frames: (0..width)
                            .map(|_| Frame {
                                site: hirsel_types::NameSite::Item,
                                parent: None,
                                kind: FrameKind::Item,
                                generic: false,
                                size_bindings: Box::new([]),
                            })
                            .collect(),
                        calls: Box::new([]),
                        staged: Box::new([]),
                    },
                    growth: Default::default(),
                });
                let facts = Facts {
                    summary: Some(Arc::new(PublishedSummary {
                        definition,
                        summary: checked.summary.clone(),
                        growth: checked.growth.clone(),
                        checked: Some(checked.clone()),
                        storage: None,
                        reads: reads.clone(),
                        text: Default::default(),
                    })),
                    checked: Some((checked, reads)),
                    dead_arms: None,
                    diagnostics: vec![Diagnostic::error(
                        const { hirsel_diag::RuleId::of("CARRY-1") },
                        Span {
                            anchor,
                            start: 0,
                            end: 1,
                        },
                        "retained warning",
                    )],
                    diagnostic_phases: vec![],
                    lowering: None,
                    table_keys: None,
                };
                let view = FactsRef {
                    summary: facts.summary.clone(),
                    checked: &facts.checked,
                    dead_arms: &facts.dead_arms,
                    diagnostics: &facts.diagnostics,
                    diagnostic_phases: &facts.diagnostic_phases,
                    lowering: &facts.lowering,
                    table_keys: facts.table_keys.as_ref(),
                };
                let store = Arc::new(persistent::Contents(Arc::new(
                    crate::store::MemStore::default(),
                )));
                let encoder = persistent::TypeRecords::new(
                    hirsel_types::TypeInterner::new(None),
                    store.clone(),
                );
                let decoder =
                    persistent::TypeRecords::new(hirsel_types::TypeInterner::new(None), store);
                let (bytes, outer) = encoder
                    .encode_record_with_decode_bytes(&view, |_| Some((owner.clone(), 0)))
                    .unwrap();
                let bound = outer + view.bytes(true);
                let (decoded, allocations) = crate::test_alloc::measured(|| {
                    decoder
                        .decode_record::<Facts>(&bytes, |_, _| Some(relocated))
                        .unwrap()
                });
                assert!(
                    allocations.peak as u64 <= bound,
                    "width={width}, peak={}, bound={bound}",
                    allocations.peak
                );
                assert_eq!(decoded.diagnostics[0].primary.anchor, relocated);
                assert_eq!(decoded.summary.as_ref().unwrap().reads.len(), width);
            }
        });
    }
}

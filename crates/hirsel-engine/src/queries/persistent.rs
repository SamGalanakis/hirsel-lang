//! Per-definition products in the existing ResultStore. Like precheck.rs, a
//! record owns canonical type rows and relocates spans while decoding. Reads
//! are verified before payload decoding; a bad or obsolete record is a miss.
use super::*;
use crate::store::{Entry as StoredEntry, EntryKey, Variant};
use hirsel_span::{Anchor, AnchorCodec};
use hirsel_types::{OwnershipFacts, TypeId, TypeTable};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
struct Body {
    checked: CheckedBody,
    plans: hirsel_check_protocol::body::Plans,
    unit: TypeId,
    boolean: TypeId,
    ownership: OwnershipFacts,
    // HashSets are encoded as content-sorted sequences, never bucket order.
    plain: Vec<TypeId>,
    silent: Vec<TypeId>,
    results: Vec<(hirsel_types::NameSite, TypeId)>,
    calls: Vec<(hirsel_syntax::ExprId, hirsel_types::Allocation)>,
}
impl Body {
    fn capture(types: &Arc<hirsel_types::TypeInterner>, body: &AnalyzedBody) -> Self {
        fn ordered(
            types: &hirsel_types::TypeInterner,
            set: &std::collections::HashSet<TypeId>,
        ) -> Vec<TypeId> {
            let mut entries: Vec<_> = set
                .iter()
                .map(|id| {
                    let (table, _) = hirsel_types::encode_table(types, &[*id]);
                    (bincode::serialize(&table).expect("type table"), *id)
                })
                .collect();
            entries.sort_by(|a, b| a.0.cmp(&b.0));
            entries.into_iter().map(|(_, id)| id).collect()
        }
        let mut ownership = body.ownership.clone();
        let plain = ordered(types, &ownership.plain_types);
        let silent = ordered(types, &ownership.silent_drop_types);
        ownership.plain_types.clear();
        ownership.silent_drop_types.clear();
        Self {
            checked: body.typed.checked.clone(),
            plans: body.typed.plans.clone(),
            unit: body.typed.unit,
            boolean: body.typed.boolean,
            ownership,
            plain,
            silent,
            results: body.results.clone(),
            calls: body.call_allocations.clone(),
        }
    }
    fn restore(mut self, syntax: Arc<ItemSyntax>) -> AnalyzedBody {
        self.ownership.plain_types.extend(self.plain);
        self.ownership.silent_drop_types.extend(self.silent);
        AnalyzedBody {
            typed: TypedBody {
                syntax,
                checked: self.checked,
                plans: self.plans,
                unit: self.unit,
                boolean: self.boolean,
            }
            .into(),
            ownership: self.ownership,
            results: self.results,
            call_allocations: self.calls,
        }
    }
}
#[derive(Serialize, Deserialize)]
struct CheckedRecord {
    checked: CheckedBody,
    plans: hirsel_check_protocol::body::Plans,
    unit: TypeId,
    boolean: TypeId,
    diagnostics: Vec<Diagnostic>,
    dependencies: Vec<SignatureDependency>,
    detail: DependencyReads,
}
#[derive(Serialize, Deserialize)]
struct ModeRecord {
    checked: Option<CheckedRecord>,
    phases: Vec<(Diagnostic, String)>,
    output: Result<QueryOutput<Body>, QueryError>,
    detail: DependencyReads,
}
#[derive(Serialize, Deserialize)]
struct LowerRecord {
    output: QueryOutput<hirsel_tir::DefinitionRef>,
    detail: DependencyReads,
    /// Present only when V actually accepted the exact stored TIR.
    verdict: Option<Vec<hirsel_validate::VRead>>,
}
#[derive(Serialize, Deserialize)]
struct Envelope {
    anchors: Vec<(DefPath, u32)>,
    table: TypeTable,
    payload: Vec<u8>,
}

fn encode_record<T: Serialize>(
    types: Arc<hirsel_types::TypeInterner>,
    items: &BTreeMap<DefPath, Vec<Arc<ItemSyntax>>>,
    record: &T,
) -> Option<Vec<u8>> {
    let mut anchors = AnchorCodec::Collect(vec![]);
    let mut table = hirsel_types::TableBuilder::new(types);
    let payload = table
        .with_encoding(|| {
            hirsel_span::with_anchor_codec(&mut anchors, || bincode::serialize(record))
        })
        .ok()?;
    let AnchorCodec::Collect(anchors) = anchors else {
        unreachable!()
    };
    let anchors = anchors
        .into_iter()
        .map(|anchor| {
            items.iter().find_map(|(path, items)| {
                items
                    .iter()
                    .find(|item| item.anchor == anchor)
                    .map(|item| (path.clone(), item.occurrence))
            })
        })
        .collect::<Option<Vec<_>>>()?;
    bincode::serialize(&Envelope {
        anchors,
        table: table.finish(),
        payload,
    })
    .ok()
}

/// Capture through the exact scalar mode codec using immutable inputs only.
/// Callers must check the selected provider before invoking this: an off store
/// never clones body data or enters canonical type/anchor encoding.
pub(super) fn capture_mode(
    types: Arc<hirsel_types::TypeInterner>,
    items: &BTreeMap<DefPath, Vec<Arc<ItemSyntax>>>,
    result: &Result<Arc<QueryOutput<AnalyzedBody>>, QueryError>,
    checked: Option<(&QueryOutput<TypedBody>, &DependencyReads)>,
    phases: Vec<(Diagnostic, String)>,
    detail: DependencyReads,
) -> Option<Vec<u8>> {
    let output = match result {
        Ok(output) => Ok(QueryOutput {
            value: Body::capture(&types, &output.value),
            diagnostics: output.diagnostics.clone(),
            dependencies: output.dependencies.clone(),
        }),
        Err(error) if error.cacheable() => Err(error.clone()),
        Err(_) => return None,
    };
    let checked = checked.map(|(output, detail)| CheckedRecord {
        checked: output.value.checked.clone(),
        plans: output.value.plans.clone(),
        unit: output.value.unit,
        boolean: output.value.boolean,
        diagnostics: output.diagnostics.clone(),
        dependencies: output.dependencies.clone(),
        detail: detail.clone(),
    });
    encode_record(
        types,
        items,
        &ModeRecord {
            checked,
            phases,
            output,
            detail,
        },
    )
}

impl QueryEngine {
    /// Select a persistent result store. Library engines default to no store;
    /// CLI engines use dir@1. Injected stores keep integration tests isolated.
    pub fn with_result_store(mut self, store: Arc<dyn crate::store::ResultStore>) -> Self {
        self.result_store = store.enabled().then_some(store);
        self
    }
    /// Recompute store hits and compare their canonical bytes (cache oracle).
    pub fn with_verify_store(mut self, verify: crate::store::VerifyStore) -> Self {
        self.verify_store = verify;
        self
    }
    fn store_key(&mut self, path: &DefPath, kind: QueryKind) -> Option<EntryKey> {
        // A generated body's registration must still be replayed by S1; its
        // transient syntax isn't a source-owned record.
        let item = self.items.get(path)?.first()?;
        if generation::generator(item).is_some() || self.generated.contains_key(path) {
            return None;
        }
        if self
            .sources
            .file(self.item_file(item))?
            .path
            .to_string_lossy()
            .starts_with('<')
        {
            return None;
        }
        let env = ContentHash::of_parts(
            "hirsel.persisted-products.v1",
            [
                self.environment_value().0.as_slice(),
                self.store_identity.0.as_slice(),
                self.package_environment(&path.0.package)
                    .package_manifest_hash
                    .0
                    .as_slice(),
            ],
        );
        Some(EntryKey::new(
            env,
            path,
            self.key_text(path, kind).ok()?,
            kind,
            LoweringPolicy::Reachable,
        ))
    }
    fn encode_record<T: Serialize>(&self, record: &T) -> Option<Vec<u8>> {
        encode_record(self.types.clone(), &self.items, record)
    }
    fn decode_record<T: serde::de::DeserializeOwned>(&self, bytes: &[u8]) -> Option<T> {
        let envelope: Envelope = bincode::deserialize(bytes).ok()?;
        let anchors = envelope
            .anchors
            .iter()
            .map(|(path, occurrence)| {
                self.items
                    .get(path)?
                    .iter()
                    .find(|item| item.occurrence == *occurrence)
                    .map(|item| item.anchor)
            })
            .collect::<Option<Vec<Anchor>>>()?;
        let ids = hirsel_types::decode_table(&self.types, &envelope.table);
        hirsel_types::with_table_decoding(&ids, || {
            hirsel_span::with_anchor_codec(&mut AnchorCodec::Decode(anchors), || {
                bincode::deserialize(&envelope.payload)
            })
        })
        .ok()
    }
    fn stored_variant(&mut self, path: &DefPath, kind: QueryKind) -> Option<Variant> {
        let _time = timings::phase(Phase::StoreLookup);
        let store = self.result_store.clone()?;
        let key = self.store_key(path, kind)?;
        let hit = StoredEntry::load(&*store, &key)
            .hit(|read| self.observe(path, read))
            .cloned();
        if hit.is_none() {
            timings::count(Count::StoreMisses);
        }
        hit
    }
    fn write_record<T: Serialize>(
        &mut self,
        path: &DefPath,
        kind: QueryKind,
        reads: &Reads,
        record: &T,
    ) -> Result<(), QueryError> {
        self.write_encoded(path, kind, reads, |engine| engine.encode_record(record))
    }
    fn write_encoded(
        &mut self,
        path: &DefPath,
        kind: QueryKind,
        reads: &Reads,
        encode: impl FnOnce(&Self) -> Option<Vec<u8>>,
    ) -> Result<(), QueryError> {
        let Some(store) = self.result_store.clone() else {
            return Ok(());
        };
        let Some(key) = self.store_key(path, kind) else {
            return Ok(());
        };
        let Some(payload) = encode(self) else {
            return Ok(());
        };
        if let Some(expected) = self
            .pending_store_checks
            .remove(&(DefId::intern(path), kind))
        {
            if expected.payload.as_ref() != payload || expected.reads != *reads {
                return Err(QueryError::Operational(hirsel_types::OperationalFailure {
                    operation: "verify-cache".into(),
                    message: format!("{path}: {kind:?} differs from its cached result"),
                }));
            }
        }
        let mut entry = StoredEntry::load(&*store, &key);
        entry.insert(Variant {
            reads: reads.clone(),
            payload: payload.into(),
        });
        store.put(&key, &entry.encode());
        Ok(())
    }
    fn adopted<T>(
        &mut self,
        path: &DefPath,
        kind: QueryKind,
        variant: Variant,
        value: Result<QueryOutput<T>, QueryError>,
        detail: DependencyReads,
    ) -> Arc<Entry<T>> {
        let key = MemoKey::new(kind, DefId::intern(path));
        let reads = Arc::new(variant.reads);
        let durability = reads.durability(|read| self.durability_of(read));
        let text = self.key_text(path, kind).expect("indexed");
        let memo = self
            .graph
            .adopt_value(key, text, value.map(Arc::new), reads, durability);
        Arc::new(Entry {
            memo,
            detail,
            covered: vec![],
            depth: None,
            consistent: true,
            products: Default::default(),
        })
    }
    pub(super) fn load_mode(&mut self, path: &DefPath) -> Option<Arc<Entry<AnalyzedBody>>> {
        if !self.s1.is_empty() {
            return None;
        }
        let variant = self.stored_variant(path, QueryKind::Modes)?;
        let record: ModeRecord = self.decode_record(&variant.payload)?;
        if self.verify_store == crate::store::VerifyStore::Recompute {
            self.pending_store_checks
                .insert((DefId::intern(path), QueryKind::Modes), variant);
            return None;
        }
        let syntax = self.snapshot(path).ok()?;
        let output = record.output.map(|o| QueryOutput {
            value: o.value.restore(syntax),
            diagnostics: o.diagnostics,
            dependencies: o.dependencies,
        });
        if let Some(c) = record.checked {
            let checked = QueryOutput {
                value: TypedBody {
                    syntax: self.snapshot(path).ok()?,
                    checked: c.checked,
                    plans: c.plans,
                    unit: c.unit,
                    boolean: c.boolean,
                },
                diagnostics: c.diagnostics,
                dependencies: c.dependencies,
            };
            let entry = self.adopted(
                path,
                QueryKind::Check,
                variant.clone(),
                Ok(checked),
                c.detail,
            );
            self.checked.insert(DefId::intern(path), entry);
        }
        timings::count(Count::StoreHits);
        let mut entry = self.adopted(path, QueryKind::Modes, variant, output, record.detail);
        Arc::get_mut(&mut entry)
            .expect("restored mode entry is not yet shared")
            .products = Arc::new(PhaseProducts {
            mode_diagnostics: record.phases,
            ..Default::default()
        });
        Some(entry)
    }
    pub(super) fn store_mode(
        &mut self,
        path: &DefPath,
        entry: &Entry<AnalyzedBody>,
    ) -> Result<(), QueryError> {
        if self.result_store.is_none()
            || !entry.consistent
            || !self.s1.is_empty()
            || !self.s1.failure_is_own()
        {
            return Ok(());
        }
        let output = entry.result();
        let diagnostics = match &output {
            Ok(o) => &o.diagnostics,
            Err(QueryError::Diagnostics(ds)) => ds,
            _ => return Ok(()),
        };
        let phases = diagnostics
            .iter()
            .filter_map(|d| {
                entry
                    .products
                    .mode_diagnostics
                    .iter()
                    .find(|(diagnostic, _)| {
                        diagnostic.rule == d.rule && diagnostic.primary == d.primary
                    })
                    .map(|(_, phase)| phase.clone())
                    .or_else(|| self.diagnostic_phase(d))
                    .map(|phase| (d.clone(), phase))
            })
            .collect();
        self.write_encoded(path, QueryKind::Modes, entry.reads(), |engine| {
            let checked = engine
                .checked
                .get(&DefId::intern(path))
                .and_then(|entry| entry.result().ok().map(|output| (output, &entry.detail)));
            capture_mode(
                engine.types.clone(),
                &engine.items,
                &output,
                checked
                    .as_ref()
                    .map(|(output, detail)| (&**output, *detail)),
                phases,
                entry.detail.clone(),
            )
        })
    }
    /// Commit an already canonical mode record after immutable workers have
    /// released its body products. Provider, key and verify-cache behavior are
    /// shared with the ordinary writer; no decoder or alternate format exists.
    pub(super) fn store_mode_payload(
        &mut self,
        path: &DefPath,
        reads: &Reads,
        payload: Vec<u8>,
    ) -> Result<(), QueryError> {
        if !self.s1.is_empty() || !self.s1.failure_is_own() {
            return Ok(());
        }
        self.write_encoded(path, QueryKind::Modes, reads, |_| Some(payload))
    }
    pub(super) fn load_lower(
        &mut self,
        path: &DefPath,
    ) -> Option<Arc<Entry<hirsel_tir::DefinitionRef>>> {
        if !self.s1.is_empty() {
            return None;
        }
        let variant = self.stored_variant(path, QueryKind::Lower)?;
        // Such results also own depth requests/registrations. Until those
        // products have a portable replay record, use their ordinary S1 path.
        if variant.reads.iter().any(|(read, _)| {
            matches!(
                read,
                ReadKey::ExecutedBody(_) | ReadKey::ValidationUniverse(_)
            )
        }) {
            return None;
        }
        let mut record: LowerRecord = self.decode_record(&variant.payload)?;
        if crate::comptime::has_staged_reads(&record.output.value) {
            return None;
        }
        if self.verify_store == crate::store::VerifyStore::Recompute {
            self.pending_store_checks
                .insert((DefId::intern(path), QueryKind::Lower), variant);
            return None;
        }
        if let Some(reads) = record.verdict.clone() {
            self.remember_lowered_verdict(record.output.value.clone(), reads);
        }
        if self.verify == Verify::On {
            self.ensure_lowered_verdict(&record.output.value).ok()?;
            let reads = self
                .lower_verdicts
                .get(&record.output.value.path)?
                .1
                .clone();
            if record.verdict.as_ref() != Some(&reads) {
                record.verdict = Some(reads);
                self.write_record(path, QueryKind::Lower, &variant.reads, &record)
                    .ok()?;
            }
        }
        timings::count(Count::StoreHits);
        Some(self.adopted(
            path,
            QueryKind::Lower,
            variant,
            Ok(record.output),
            record.detail,
        ))
    }
    pub(crate) fn remember_lowered_verdict(
        &mut self,
        definition: hirsel_tir::DefinitionRef,
        reads: Vec<hirsel_validate::VRead>,
    ) {
        self.lower_verdicts
            .insert(definition.path, (definition, reads));
    }
    /// Evidence is for these exact immutable bytes and this validator's inputs.
    pub(crate) fn lowered_verdict_holds(&mut self, definition: &hirsel_tir::DefinitionRef) -> bool {
        let Some((accepted, reads)) = self.lower_verdicts.get(&definition.path).cloned() else {
            return false;
        };
        if !Arc::ptr_eq(&accepted, definition) {
            return false;
        }
        let snapshot = self.signature_snapshot();
        reads.iter().all(|r| {
            hirsel_validate::read_holds(r, snapshot.validation(&definition.path.path().0.package))
        })
    }
    pub(crate) fn ensure_lowered_verdict(
        &mut self,
        definition: &hirsel_tir::DefinitionRef,
    ) -> Result<(), QueryError> {
        if self.verify == Verify::Off || self.lowered_verdict_holds(definition) {
            return Ok(());
        }
        let mut reads = vec![];
        let report = self.run_v(&definition.path.path().0.package, |types, views| {
            let _time = timings::phase(Phase::Verify);
            timings::count(Count::VRuns);
            hirsel_validate::validate(definition, types, views, &mut reads)
        });
        if let Some(error) = hirsel_validate::tooling_limit_error(&report) {
            return Err(error);
        }
        if !report.violations.is_empty() {
            return Err(QueryError::Operational(hirsel_types::OperationalFailure {
                operation: "TIR validation".into(),
                message: format!("{:?}", report.violations),
            }));
        }
        self.remember_lowered_verdict(definition.clone(), reads);
        Ok(())
    }
    pub(super) fn store_lower(
        &mut self,
        path: &DefPath,
        entry: &Entry<hirsel_tir::DefinitionRef>,
        verdict: Option<Vec<hirsel_validate::VRead>>,
    ) -> Result<(), QueryError> {
        if self.result_store.is_none()
            || !entry.consistent
            || !self.s1.is_empty()
            || !self.s1.failure_is_own()
        {
            return Ok(());
        }
        if entry.reads().iter().any(|(read, _)| {
            matches!(
                read,
                ReadKey::ExecutedBody(_) | ReadKey::ValidationUniverse(_)
            )
        }) {
            return Ok(());
        }
        let Ok(output) = entry.output() else {
            return Ok(());
        };
        if crate::comptime::has_staged_reads(&output.value) {
            return Ok(());
        }
        self.write_record(
            path,
            QueryKind::Lower,
            entry.reads(),
            &LowerRecord {
                output,
                detail: entry.detail.clone(),
                verdict,
            },
        )
    }
}

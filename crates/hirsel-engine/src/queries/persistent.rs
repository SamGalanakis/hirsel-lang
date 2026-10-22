//! Per-definition products in the existing ResultStore. Like precheck.rs, a
//! record owns canonical type rows and relocates spans while decoding. Reads
//! are verified before payload decoding; a bad or obsolete record is a miss.
use super::*;
use crate::store::{Entry as StoredEntry, EntryKey, Variant};
use hirsel_span::{Anchor, AnchorCodec};
use hirsel_types::{OwnershipFacts, TypeId, TypeTable};
use serde::{Deserialize, Serialize};
mod expected;
pub(crate) use expected::{Expected, Spool};

#[cfg(test)]
mod expected_tests {
    use super::*;

    #[test]
    fn verify_preflight_releases_old_payloads_before_workers_start() {
        let mut engine = QueryEngine::new(crate::default_environment(), PhaseAdapters::default());
        let mut observed = Vec::new();
        for i in 0..64 {
            let path = DefPath(hirsel_syntax::ItemPath {
                package: crate::default_context().package,
                modules: vec![],
                name: format!("p{i}"),
            });
            let payload: Arc<[u8]> = vec![i as u8; 128 * 1024].into();
            observed.push(Arc::downgrade(&payload));
            let expected = Expected::capture(
                Variant {
                    reads: Reads::empty(),
                    payload,
                },
                &mut engine.store_check_spool,
            );
            engine
                .pending_store_checks
                .insert((DefId::intern(&path), QueryKind::Modes), expected);
        }
        assert_eq!(engine.pending_store_checks.len(), 64);
        assert!(
            observed.iter().all(|weak| weak.upgrade().is_none()),
            "preflight retained all old canonical payloads until the frontier drained"
        );
    }
}

// Match complete_reads' observation semantics: manifest graph revisions are
// unused provenance for every lookup, including failed probes of local names.
// Negative lookups observe accessible repair candidates through their canonical
// Reads, not through this stamp. Keeping it here makes equal products differ
// when the same package moves between root and dependency positions.
fn canonical_reads(reads: &mut [hirsel_types::DependencyRead]) {
    use hirsel_types::DependencyRead;
    for read in reads {
        let manifest = match read {
            DependencyRead::Signature { .. } => continue,
            DependencyRead::Lookup { manifest, .. }
            | DependencyRead::ModuleHead { manifest, .. } => manifest,
            DependencyRead::ProviderLookup { output, .. } => &mut output.manifest,
        };
        *manifest = ContentHash::default();
    }
}
fn canonical_checked(checked: &CheckedBody) -> CheckedBody {
    let mut checked = checked.clone();
    canonical_reads(&mut checked.resolution.reads);
    checked
}
fn canonical_detail(mut detail: DependencyReads) -> DependencyReads {
    canonical_reads(&mut detail.manifest_lookups);
    canonical_reads(&mut detail.resolver);
    detail
}

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
            checked: canonical_checked(&body.typed.checked),
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
impl ModeRecord {
    fn valid(&self, syntax: &ItemSyntax) -> bool {
        let valid = |checked: &CheckedBody, plans: &hirsel_check_protocol::body::Plans| {
            valid_body_indices(syntax, checked, plans)
        };
        if self
            .checked
            .as_ref()
            .is_some_and(|c| !valid(&c.checked, &c.plans))
        {
            return false;
        }
        match &self.output {
            Ok(output) => {
                let body = &output.value;
                self.checked.is_some()
                    && valid(&body.checked, &body.plans)
                    && body
                        .results
                        .iter()
                        .all(|(site, _)| valid_site(syntax, *site))
                    && body
                        .calls
                        .iter()
                        .all(|(id, _)| valid_expression(syntax, *id))
                    && valid_ownership(syntax, &body.ownership)
            }
            Err(error) => error.cacheable(),
        }
    }
}
fn valid_expression(syntax: &ItemSyntax, id: hirsel_syntax::ExprId) -> bool {
    (id.0 as usize) < syntax.tree.expressions.len()
}
fn valid_site(syntax: &ItemSyntax, site: hirsel_types::NameSite) -> bool {
    use hirsel_types::NameSite;
    match site {
        NameSite::Expression(id) => valid_expression(syntax, id),
        NameSite::Statement(id) => (id.0 as usize) < syntax.tree.statements.len(),
        NameSite::Pattern(id) => (id.0 as usize) < syntax.tree.patterns.len(),
        NameSite::Type(id) => (id.0 as usize) < syntax.tree.types.len(),
        NameSite::Item => true,
    }
}
/// These are source arena positions, never process IDs. Check the lexical
/// owner first: even an in-range row from another item has no meaning here.
fn valid_body_indices(
    syntax: &ItemSyntax,
    checked: &CheckedBody,
    plans: &hirsel_check_protocol::body::Plans,
) -> bool {
    use hirsel_types::{CallTarget, NameSite, SourceProjectionKind, SourceRoot};
    let expression = |id| valid_expression(syntax, id);
    let site = |id| valid_site(syntax, id);
    let pattern = |id| site(NameSite::Pattern(id));
    let target = |target: &CallTarget| match target {
        CallTarget::Value(id) => expression(*id),
        _ => true,
    };
    let resolution = &checked.resolution;
    checked.lexical == syntax.owner()
        && checked
            .expressions
            .iter()
            .all(|row| expression(row.expression))
        && checked.calls.iter().all(|row| {
            expression(row.expression)
                && target(&row.target)
                && row.arguments.iter().all(|arg| expression(arg.expression))
        })
        && checked.tails.iter().all(|row| {
            expression(row.call)
                && row
                    .owned_loop_arguments
                    .iter()
                    .all(|(_, id)| expression(*id))
        })
        && checked
            .instantiations
            .iter()
            .all(|row| expression(row.expression))
        && checked
            .closures
            .iter()
            .all(|row| expression(row.expression))
        && checked
            .bindings
            .iter()
            .all(|row| site(row.owner) && site(row.site))
        && checked
            .patterns
            .iter()
            .all(|row| pattern(row.pattern) && row.subpatterns().all(pattern))
        && checked.places.iter().all(|row| {
            expression(row.expression)
                && match row.root {
                    SourceRoot::Value(id) => expression(id),
                    SourceRoot::Binding { owner, .. } => site(owner),
                    _ => true,
                }
                && row.projections.iter().all(|projection| {
                    expression(projection.expression)
                        && match projection.kind {
                            SourceProjectionKind::Index(id) => expression(id),
                            _ => true,
                        }
                })
        })
        && resolution.contextual.iter().all(|(id, _)| site(*id))
        && resolution.paths.iter().all(|(id, _)| site(*id))
        && resolution
            .local_functions
            .iter()
            .all(|(id, _)| site(NameSite::Statement(*id)))
        && resolution.deferred.iter().all(|row| site(row.site))
        && resolution.generics.iter().all(|row| site(row.owner))
        && resolution
            .reference_scopes
            .iter()
            .all(|(a, b)| site(*a) && site(*b))
        && resolution.expressions.iter().all(|(id, _)| expression(*id))
        && resolution
            .types
            .iter()
            .all(|(id, _)| site(NameSite::Type(*id)))
        && resolution.bindings.iter().all(|(id, _, _)| pattern(*id))
        && plans.resource_patterns.iter().all(|(id, _)| pattern(*id))
        && plans.pattern_literals.iter().all(|(id, _)| pattern(*id))
        && plans
            .tries
            .iter()
            .all(|row| expression(row.expression) && row.mapper.as_ref().is_none_or(target))
        && plans.loops.iter().all(|row| {
            site(NameSite::Statement(row.statement()))
                && row.cursor().is_none_or(|cursor| expression(cursor.source))
        })
        && plans.functions.iter().all(|row| site(row.owner))
        && plans
            .shapes
            .iter()
            .all(|(a, b, _)| expression(*a) && site(NameSite::Type(*b)))
        && plans.contracts.iter().all(|row| {
            site(row.owner)
                && expression(row.expression)
                && row.snapshots.iter().copied().all(expression)
                && row.parameters.iter().all(|p| match p {
                    hirsel_check_protocol::body::ContractParameter::Snapshot(id) => expression(*id),
                    _ => true,
                })
        })
        && plans.facts.iter().all(|row| expression(row.expression))
}
fn valid_ownership(syntax: &ItemSyntax, facts: &OwnershipFacts) -> bool {
    let expression = |id| valid_expression(syntax, id);
    let site = |id| valid_site(syntax, id);
    facts.field_loans.iter().all(|(id, _)| expression(*id))
        && facts.composed_arguments.iter().copied().all(expression)
        && facts
            .captures
            .iter()
            .all(|row| expression(row.expression) && site(row.owner))
        && facts
            .call_arguments
            .iter()
            .all(|(a, b, _)| expression(*a) && expression(*b))
        && facts
            .identity_joins
            .iter()
            .all(|(a, b, _)| site(*a) && site(*b))
        && facts
            .identity_fields
            .iter()
            .all(|(a, b, _, _)| site(*a) && site(*b))
        && facts
            .physical_projections
            .iter()
            .all(|(id, _)| expression(*id))
        && facts.unique.iter().all(|row| expression(row.expression))
        && facts
            .guarded_patterns
            .iter()
            .all(|row| site(hirsel_types::NameSite::Pattern(row.pattern)))
        && facts.cursor_rebindings.iter().copied().all(expression)
        && facts
            .borrow_cursors
            .iter()
            .all(|cursor| valid_site(syntax, cursor.binding))
        && facts.allocating_mutations.iter().copied().all(expression)
        && facts.replacements.iter().all(|(id, _, _)| expression(*id))
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
    #[serde(serialize_with = "serialize_payload")]
    payload: Vec<u8>,
}

fn serialize_payload<S: serde::Serializer>(
    payload: &[u8],
    serializer: S,
) -> Result<S::Ok, S::Error> {
    // bincode 1.3.3 ser/mod.rs:132-135 writes the same length and bytes as
    // Vec<u8>, in one bulk write. Keep the bounded Vec decoder unchanged.
    serializer.serialize_bytes(payload)
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
        checked: canonical_checked(&output.value.checked),
        plans: output.value.plans.clone(),
        unit: output.value.unit,
        boolean: output.value.boolean,
        diagnostics: output.diagnostics.clone(),
        dependencies: output.dependencies.clone(),
        detail: canonical_detail(detail.clone()),
    });
    encode_record(
        types,
        items,
        &ModeRecord {
            checked,
            phases,
            output,
            detail: canonical_detail(detail),
        },
    )
}

/// Immutable writer shared by serial and worker publication. Keys and expected
/// verification bytes are selected by the engine before freezing; provider I/O
/// owns no query state and retains no encoded payload after write returns.
pub(super) struct EncodedWriter {
    store: Arc<dyn crate::store::ResultStore>,
    key: EntryKey,
    path: DefPath,
    kind: QueryKind,
    expected: Option<Expected>,
}
impl EncodedWriter {
    pub(super) fn write(&self, reads: &Reads, payload: Vec<u8>) -> Result<(), QueryError> {
        if let Some(expected) = &self.expected {
            let matches = expected.matches(reads, &payload);
            if !matches.as_ref().is_ok_and(|same| *same) {
                return Err(QueryError::Operational(hirsel_types::OperationalFailure {
                    operation: "verify-cache".into(),
                    message: match matches {
                        Ok(_) => format!(
                            "{}: {:?} differs from its cached result",
                            self.path, self.kind
                        ),
                        Err(error) => {
                            format!("{}: {:?} expected bytes: {error}", self.path, self.kind)
                        }
                    },
                }));
            }
        }
        let mut entry = StoredEntry::load(&*self.store, &self.key);
        let mut variant = Variant {
            reads: reads.clone(),
            payload: payload.into(),
        };
        variant.bind_payload();
        entry.insert(variant);
        self.store.put(&self.key, &entry.encode_for(&self.key));
        Ok(())
    }
}

impl QueryEngine {
    pub fn store_adoptions(&self) -> u64 {
        self.store_adoptions
    }
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
    fn decode_record<T: serde::de::DeserializeOwned + Serialize>(&self, bytes: &[u8]) -> Option<T> {
        use bincode::Options;
        let envelope: Envelope = bincode::DefaultOptions::new()
            .with_fixint_encoding()
            .with_limit(bytes.len() as u64)
            .reject_trailing_bytes()
            .deserialize(bytes)
            .ok()?;
        if !envelope.table.valid_indices() {
            return None;
        }
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
        let record: T = hirsel_types::with_table_decoding(&ids, || {
            hirsel_span::with_anchor_codec(&mut AnchorCodec::Decode(anchors), || {
                bincode::DefaultOptions::new()
                    .with_fixint_encoding()
                    .with_limit(envelope.payload.len() as u64)
                    .reject_trailing_bytes()
                    .deserialize(&envelope.payload)
            })
        })
        .ok()?;
        // The compatibility decoder represents an invalid type position as
        // DANGLING for V. Front records must reject it before any consumer can
        // index types; collecting through serde covers all nested payload IDs.
        let (encoded, roots) = hirsel_types::with_table_collection(|| {
            bincode::serialize_into(std::io::sink(), &record)
        });
        encoded.ok()?;
        roots
            .iter()
            .all(|id| self.types.get(*id).is_some())
            .then_some(record)
    }
    fn stored_variant(&mut self, path: &DefPath, kind: QueryKind) -> Option<Variant> {
        let _time = timings::phase(Phase::StoreLookup);
        let store = self.result_store.clone()?;
        let key = self.store_key(path, kind)?;
        let mut entry = StoredEntry::load(&*store, &key);
        entry
            .variants
            .retain_mut(|variant| variant.unbind_payload().is_some());
        let hit = entry.hit(|read| self.observe(path, read)).cloned();
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
        let expected = self
            .pending_store_checks
            .remove(&(DefId::intern(path), kind));
        EncodedWriter {
            store,
            key,
            path: path.clone(),
            kind,
            expected,
        }
        .write(reads, payload)
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
        let syntax = self.snapshot(path).ok()?;
        if !record.valid(&syntax) {
            return None;
        }
        if self.verify_store == crate::store::VerifyStore::Recompute {
            self.pending_store_checks.insert(
                (DefId::intern(path), QueryKind::Modes),
                Expected::capture(variant, &mut self.store_check_spool),
            );
            return None;
        }
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
        self.store_adoptions += 1;
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
    /// Snapshot this task's store policy without consuming its pending
    /// verification obligation; canonical merge retires it only after encoding.
    pub(super) fn mode_writer(&mut self, path: &DefPath) -> Option<EncodedWriter> {
        if !self.s1.is_empty() || !self.s1.failure_is_own() {
            return None;
        }
        Some(EncodedWriter {
            store: self.result_store.clone()?,
            key: self.store_key(path, QueryKind::Modes)?,
            path: path.clone(),
            kind: QueryKind::Modes,
            expected: self
                .pending_store_checks
                .get(&(DefId::intern(path), QueryKind::Modes))
                .cloned(),
        })
    }
    pub(super) fn finish_mode_write(&mut self, path: &DefPath) {
        self.pending_store_checks
            .remove(&(DefId::intern(path), QueryKind::Modes));
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
        if record.output.value.path != DefId::intern(path) {
            return None;
        }
        if crate::comptime::has_staged_reads(&record.output.value) {
            return None;
        }
        if self.verify_store == crate::store::VerifyStore::Recompute {
            self.pending_store_checks.insert(
                (DefId::intern(path), QueryKind::Lower),
                Expected::capture(variant, &mut self.store_check_spool),
            );
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
        self.store_adoptions += 1;
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
        *self
            .scalar_validation_runs
            .entry(definition.path)
            .or_default() += 1;
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
                detail: canonical_detail(entry.detail.clone()),
                verdict,
            },
        )
    }
}

#[cfg(test)]
mod malformed_tests {
    use super::*;

    // The pre-bulk codec, including its original Vec<u8> serialization.
    #[derive(Serialize, Deserialize)]
    struct LegacyEnvelope {
        anchors: Vec<(DefPath, u32)>,
        table: TypeTable,
        payload: Vec<u8>,
    }

    #[test]
    fn bulk_payload_preserves_legacy_envelope_bytes_and_bounded_decoding() {
        use bincode::Options;
        let (engine, syntax) = fixture();
        let unit = engine.types.intern(hirsel_types::Type::unit());
        let bytes = engine
            .encode_record(&(unit, syntax.item.origin.span))
            .unwrap();
        let mut envelope: Envelope = bincode::deserialize(&bytes).unwrap();
        assert!(!envelope.anchors.is_empty());
        assert!(!envelope.table.is_empty());
        // Include bincode 1.3.3 tests/test.rs:565-572's byte equality vector,
        // every byte value, and payloads spanning multiple copy chunks.
        for payload in [
            vec![],
            vec![0],
            b"abc\0123".to_vec(),
            (0..=255).collect(),
            vec![0xa5; 65_537],
            vec![0xff; 1 << 20],
        ] {
            envelope.payload = payload;
            let legacy = LegacyEnvelope {
                anchors: envelope.anchors.clone(),
                table: envelope.table.clone(),
                payload: envelope.payload.clone(),
            };
            let old = bincode::serialize(&legacy).unwrap();
            let new = bincode::serialize(&envelope).unwrap();
            assert_eq!(old, new);
            let options = bincode::DefaultOptions::new()
                .with_fixint_encoding()
                .with_limit(old.len() as u64)
                .reject_trailing_bytes();
            let decoded: Envelope = options.deserialize(&old).unwrap();
            assert_eq!(decoded.payload, legacy.payload);
            let decoded: LegacyEnvelope = options.deserialize(&new).unwrap();
            assert_eq!(decoded.payload, envelope.payload);
            assert!(
                options
                    .deserialize::<Envelope>(&old[..old.len() - 1])
                    .is_err()
            );
            let mut trailing = old;
            trailing.push(0);
            assert!(options.deserialize::<Envelope>(&trailing).is_err());
        }
    }

    #[test]
    fn legacy_envelope_records_relocate_types_and_anchors() {
        let (engine, syntax) = fixture();
        let unit = engine.types.intern(hirsel_types::Type::unit());
        let record = (unit, syntax.item.origin.span);
        let bytes = engine.encode_record(&record).unwrap();
        // Decode the current writer using the legacy layout, then write with
        // the old serializer and consume through the production decoder.
        let legacy: LegacyEnvelope = bincode::deserialize(&bytes).unwrap();
        let old = bincode::serialize(&legacy).unwrap();
        assert_eq!(old, bytes);
        assert_eq!(engine.decode_record::<(TypeId, Span)>(&old), Some(record));
    }

    fn fixture() -> (QueryEngine, Arc<ItemSyntax>) {
        let mut engine = QueryEngine::new(crate::default_environment(), PhaseAdapters::default())
            .with_result_store(Arc::new(crate::store::MemStore::default()));
        let file = engine
            .sources
            .add("cache-record.hir", "fn main() { true }".into())
            .unwrap();
        let tokens = hirsel_lexer::lex(file, &engine.sources).tokens;
        let context = crate::default_context();
        let mut parsed = hirsel_parser::parse(file, tokens, &context, &mut engine.sources);
        assert!(parsed.diagnostics.is_empty());
        engine.index(&mut parsed.syntax, &context).unwrap();
        (engine, parsed.syntax.items[0].clone())
    }
    fn record(engine: &QueryEngine, syntax: &ItemSyntax) -> ModeRecord {
        let unit = engine.types.intern(hirsel_types::Type::unit());
        let boolean = engine.types.intern(hirsel_types::Type::boolean());
        let checked = CheckedBody {
            lexical: syntax.owner(),
            places: vec![],
            calls: vec![],
            tails: vec![],
            bindings: vec![],
            patterns: vec![],
            resolution: Default::default(),
            expressions: vec![hirsel_types::Elaboration {
                construction_destination: None,
                expression: hirsel_syntax::ExprId(0),
                ty: boolean,
                usage: hirsel_types::UseKind::Copy,
                instantiated_arguments: vec![],
            }],
            instantiations: vec![],
            closures: vec![],
        };
        ModeRecord {
            checked: Some(CheckedRecord {
                checked: checked.clone(),
                plans: Default::default(),
                unit,
                boolean,
                diagnostics: vec![],
                dependencies: vec![],
                detail: Default::default(),
            }),
            phases: vec![],
            detail: Default::default(),
            output: Ok(QueryOutput {
                value: Body {
                    checked,
                    plans: Default::default(),
                    unit,
                    boolean,
                    ownership: Default::default(),
                    plain: vec![],
                    silent: vec![],
                    results: vec![],
                    calls: vec![],
                },
                diagnostics: vec![],
                dependencies: vec![],
            }),
        }
    }
    #[test]
    fn structurally_invalid_records_miss_before_adoption_or_verification() {
        for verify in [
            crate::store::VerifyStore::Off,
            crate::store::VerifyStore::Recompute,
        ] {
            for fault in 0..7 {
                let (mut engine, syntax) = fixture();
                let mut record = record(&engine, &syntax);
                match fault {
                    0 => (),
                    1 => {
                        record.output.as_mut().unwrap().value.checked.expressions[0]
                            .expression
                            .0 = u32::MAX
                    }
                    2 => record.checked.as_mut().unwrap().checked.lexical.occurrence += 1,
                    3 => record.checked = None,
                    4 => record.output.as_mut().unwrap().value.unit = TypeId::DANGLING,
                    5 => record.output.as_mut().unwrap().value.results.push((
                        hirsel_types::NameSite::Pattern(hirsel_syntax::PatternId(u32::MAX)),
                        record.checked.as_ref().unwrap().unit,
                    )),
                    6 => record
                        .output
                        .as_mut()
                        .unwrap()
                        .value
                        .checked
                        .resolution
                        .expressions
                        .push((
                            hirsel_syntax::ExprId(u32::MAX),
                            hirsel_types::ResolvedName::Primitive(hirsel_types::Primitive::Bool),
                        )),
                    _ => unreachable!(),
                }
                let path = &syntax.item.definition;
                // Encode through the genuine writer, with valid key, checksum
                // and reads. These exercise semantic guards, not just framing.
                engine
                    .write_record(path, QueryKind::Modes, &Reads::empty(), &record)
                    .unwrap();
                engine.verify_store = verify;
                let hit = engine.load_mode(path);
                if fault == 0 {
                    assert_eq!(hit.is_some(), verify == crate::store::VerifyStore::Off);
                    assert_eq!(
                        engine.pending_store_checks.len(),
                        usize::from(verify == crate::store::VerifyStore::Recompute)
                    );
                } else {
                    assert!(hit.is_none(), "fault {fault}");
                    assert_eq!(engine.store_adoptions(), 0);
                    assert!(engine.checked.is_empty());
                    assert!(engine.pending_store_checks.is_empty());
                }
            }
        }
    }
    #[test]
    fn canonical_type_and_anchor_domains_and_exact_payload_consumption_are_checked() {
        let (engine, syntax) = fixture();
        let unit = engine.types.intern(hirsel_types::Type::unit());
        let bytes = engine.encode_record(&unit).unwrap();
        assert_eq!(engine.decode_record::<TypeId>(&bytes), Some(unit));
        let mut envelope: Envelope = bincode::deserialize(&bytes).unwrap();
        envelope.payload = u32::MAX.to_le_bytes().to_vec();
        assert!(
            engine
                .decode_record::<TypeId>(&bincode::serialize(&envelope).unwrap())
                .is_none()
        );
        let bad = engine
            .types
            .intern(hirsel_types::Type::with_kind(hirsel_types::TypeKind::Box(
                TypeId::DANGLING,
            )));
        let bytes = engine.encode_record(&bad).unwrap();
        assert!(engine.decode_record::<TypeId>(&bytes).is_none());
        let bytes = engine.encode_record(&syntax.item.origin.span).unwrap();
        let mut envelope: Envelope = bincode::deserialize(&bytes).unwrap();
        envelope.payload[..4].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(
            engine
                .decode_record::<Span>(&bincode::serialize(&envelope).unwrap())
                .is_none()
        );
        let mut envelope: Envelope =
            bincode::deserialize(&engine.encode_record(&unit).unwrap()).unwrap();
        envelope.payload.push(0);
        assert!(
            engine
                .decode_record::<TypeId>(&bincode::serialize(&envelope).unwrap())
                .is_none()
        );
        let mut bytes = engine.encode_record(&unit).unwrap();
        bytes.push(0);
        assert!(engine.decode_record::<TypeId>(&bytes).is_none());
    }
}

//! Exact native action/output associations, before Value construction.
//!
//! Go cache.go:28–34,185–235,479–489 separates complete actions from outputs;
//! Cranelift incremental_cache.rs:39–95,120–146 separates a stencil from fresh
//! binding and checks reuse by recompilation. This local experiment uses the
//! existing trusted compiler/cache-directory model, not remote authentication.
//! Production remains disabled until the bounded correctness/benefit gates pass.
use super::{NativeError, Options, ReadyProgram, StageName, Validation};
use hirsel_codec::snapshot_struct;
use hirsel_engine::{NativeInput, ValidatedNativeInput, Verify};
use hirsel_lir::fragment::UnitFragment;
use hirsel_lir::ir::{Callee, LirUnit, Symbol};
use hirsel_lir::objects::{ObjectStore, UnitKey};
use hirsel_lir::{LayoutKeys, abi_types::FnAbiId, layout_types::LayoutTable};
use hirsel_syntax::{ContentHash, DefPath};
use std::collections::{BTreeMap, BTreeSet};

// Deliberately not an option that a cache receipt can turn on.
const PRODUCTION_REUSE: bool = false;
const SCHEMA: u32 = 3;
const MAGIC: &[u8] = b"HIRNPL03";
#[cfg(test)]
#[path = "seed_parity_tests.rs"]
mod seed_parity_tests;
thread_local! { static RECORDING: std::cell::Cell<bool> = const { std::cell::Cell::new(false) }; }
pub(super) fn recording() -> bool {
    RECORDING.get()
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct Record {
    pub key: UnitKey,
    pub digest: ContentHash,
    pub calls: Vec<Calls>,
}
snapshot_struct!(Record { key, digest, calls });
#[derive(Clone, Debug, Eq, PartialEq)]
struct Call {
    direct: Option<Symbol>,
    abi: ContentHash,
    target: Option<Symbol>,
}
snapshot_struct!(Call {
    direct,
    abi,
    target
});
#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct Calls {
    definition: DefPath,
    instance: ContentHash,
    calls: Vec<Call>,
}
snapshot_struct!(Calls {
    definition,
    instance,
    calls
});
#[derive(Clone, Debug)]
pub(super) struct Production {
    pub records: Vec<Record>,
}

pub(super) fn calls<S>(
    units: &[&LirUnit<S>],
    table: &LayoutTable,
    keys: &LayoutKeys,
) -> Vec<Calls> {
    units
        .iter()
        .map(|u| Calls {
            definition: u.header.definition.clone(),
            instance: u.header.instance,
            calls: u
                .tables
                .calls
                .iter()
                .map(|c| Call {
                    // Bound/indirect calls must never claim a result arena.
                    direct: match c.callee {
                        Callee::Direct(s) => Some(u.tables.symbols[s.index()].clone()),
                        _ => None,
                    },
                    target: match c.callee {
                        Callee::Direct(s) | Callee::Bound { code: s, .. } => {
                            Some(u.tables.symbols[s.index()].clone())
                        }
                        _ => None,
                    },
                    abi: keys.fn_abi(&table.fn_abis[c.abi.0 as usize]),
                })
                .collect(),
        })
        .collect()
}

// Additive evidence domain leaves legacy fragment bytes/codecs intact. A
// missing sidecar makes population recompute that owner through the old passes.
#[derive(Clone)]
struct ReadyCalls {
    producer: ContentHash,
    fragment: ContentHash,
    calls: Vec<Calls>,
}
snapshot_struct!(ReadyCalls {
    producer,
    fragment,
    calls
});
fn calls_key(key: &UnitKey, fragment: ContentHash) -> UnitKey {
    UnitKey(ContentHash::of_parts(
        "hirsel.native-plan.ready-calls.v1",
        [key.0.0.as_slice(), fragment.0.as_slice()],
    ))
}
pub(super) fn load_calls(
    store: &dyn ObjectStore,
    key: &UnitKey,
    bytes: &[u8],
) -> Option<Vec<Calls>> {
    let digest = fragment_digest(bytes);
    let bytes = store.get(&calls_key(key, digest))?;
    let evidence: ReadyCalls = hirsel_codec::from_text(std::str::from_utf8(&bytes).ok()?).ok()?;
    (evidence.producer == ContentHash(crate::COMPILER_IDENTITY) && evidence.fragment == digest)
        .then_some(evidence.calls)
}
pub(super) fn save_calls(
    store: &dyn ObjectStore,
    key: &UnitKey,
    bytes: &[u8],
    calls: &[Calls],
) -> Result<(), NativeError> {
    let fragment = fragment_digest(bytes);
    let key = calls_key(key, fragment);
    let bytes = hirsel_codec::to_text(&ReadyCalls {
        producer: ContentHash(crate::COMPILER_IDENTITY),
        fragment,
        calls: calls.to_vec(),
    });
    if let Some(old) = store.get(&key) {
        if old.as_ref() != bytes.as_bytes() {
            return Err(failed(
                "recomputation differs from cached Ready call evidence",
            ));
        }
    }
    store.put(&key, bytes.as_bytes());
    Ok(())
}

/// Portable keyed-adapter association. Numeric ABI IDs belong only to the
/// current table and are rebound after the complete ABI content key is checked.
#[derive(Clone, Debug, Eq, PartialEq)]
struct AdapterBinding {
    adapter: Symbol,
    primitive: Symbol,
    abi: ContentHash,
}
snapshot_struct!(AdapterBinding {
    adapter,
    primitive,
    abi
});

#[derive(Clone, Debug, Eq, PartialEq)]
struct NativePlan {
    ahead_units: Vec<LirUnit<hirsel_lir::stage::Ready>>,
    ahead_environments: Vec<(ContentHash, Option<hirsel_lir::LayoutId>)>,
    schema: u32,
    producer: ContentHash,
    action: ContentHash,
    table: LayoutTable,
    statics: Vec<hirsel_lir::statics::StaticData>,
    owners: Vec<(DefPath, Option<DefPath>)>,
    invocation_bodies: Vec<hirsel_lir::invocations::ReferenceBody>,
    pretransfer_adapters: Vec<AdapterBinding>,
    functions: Vec<(DefPath, ContentHash)>,
    finalizers: Vec<(hirsel_lir::layout_types::LayoutId, ContentHash)>,
    inline: Vec<ContentHash>,
    callbacks: Vec<DefPath>,
    tracking: bool,
    generic_units: u64,
    records: Vec<Record>,
}
snapshot_struct!(NativePlan {
    ahead_units,
    ahead_environments,
    schema,
    producer,
    action,
    table,
    statics,
    owners,
    invocation_bodies,
    pretransfer_adapters,
    functions,
    finalizers,
    inline,
    callbacks,
    tracking,
    generic_units,
    records
});

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Mode {
    Disabled,
    Reuse,
    VerifyReuse,
}
#[derive(Default, Debug)]
pub(super) struct Stats {
    pub hit: bool,
    pub manifest_bytes: usize,
    pub metadata_bytes: usize,
    pub fragment_bytes: usize,
    pub fallback: Option<&'static str>,
}
pub(super) struct Prepared {
    pub ready: ReadyProgram,
    pub stats: Stats,
}

pub(super) fn fragment_digest(bytes: &[u8]) -> ContentHash {
    ContentHash::of_parts("hirsel.native-plan.fragment.v1", [bytes])
}
fn action_key(hash: ContentHash) -> UnitKey {
    UnitKey(ContentHash::of_parts(
        "hirsel.native-plan.action.v1",
        [hash.0.as_slice()],
    ))
}
fn output_key(bytes: &[u8]) -> UnitKey {
    UnitKey(ContentHash::of_parts(
        "hirsel.native-plan.output.v1",
        [bytes],
    ))
}

pub(super) fn prepare(
    validated: &ValidatedNativeInput,
    options: &Options,
) -> Result<Prepared, NativeError> {
    // Experimental opt-in exists solely to measure this lane; default callers do
    // no new key/metadata work. EveryStage still selects the full reference.
    let experiment = std::env::var_os("HIRSEL_NATIVE_PLAN_EXPERIMENT");
    let mode = if !PRODUCTION_REUSE && experiment.is_none() {
        Mode::Disabled
    } else if options.validation == Validation::EveryStage
        || experiment.as_deref() == Some(std::ffi::OsStr::new("verify"))
    {
        Mode::VerifyReuse
    } else {
        Mode::Reuse
    };
    let result = prepare_in(validated, options, mode)?;
    if experiment.is_some() {
        eprintln!(
            "native-plan: hit={} manifest_bytes={} metadata_bytes={} fragment_bytes={} fallback={}",
            result.stats.hit,
            result.stats.manifest_bytes,
            result.stats.metadata_bytes,
            result.stats.fragment_bytes,
            result.stats.fallback.unwrap_or("none")
        );
    }
    Ok(result)
}

fn prepare_in(
    validated: &ValidatedNativeInput,
    options: &Options,
    mode: Mode,
) -> Result<Prepared, NativeError> {
    let mut stats = Stats::default();
    let fallback = |mut stats: Stats, reason| {
        stats.fallback = Some(reason);
        super::stages::build_full(validated, options).map(|ready| Prepared { ready, stats })
    };
    if mode == Mode::Disabled {
        return fallback(stats, "disabled");
    }
    if validated.verification() != Verify::On {
        return fallback(stats, "verification-off");
    }
    if !options.discharge.0.is_empty() {
        return fallback(stats, "discharge");
    }
    let Some(objects) = &options.objects else {
        return fallback(stats, "bodies-required");
    };
    if !objects.store.enabled() || !objects.store.production_enabled() {
        return fallback(stats, "store-disabled");
    }
    let Some(bytes) = input_manifest(validated.snapshot(), options) else {
        return fallback(stats, "incomplete-input-evidence");
    };
    stats.manifest_bytes = bytes.len();
    let action = ContentHash::of_parts("hirsel.native-plan.input.v1", [bytes.as_slice()]);
    let store = objects.store.as_ref();
    let cached = retrieve(store, action, &mut stats);
    if mode == Mode::Reuse {
        if let Some(plan) = &cached {
            match assemble(plan, validated, options, store, &mut stats) {
                Ok(ready) => {
                    stats.hit = true;
                    return Ok(Prepared { ready, stats });
                }
                Err(_) => stats.fallback = Some("inconsistent-or-missing-fragment"),
            }
        }
    }
    // Neither import nor object hits can supply the reference computation.
    // EveryStage's existing object code compares complete fragments itself.
    let mut reference = options.clone();
    if mode == Mode::VerifyReuse {
        reference.validation = Validation::EveryStage;
    }
    struct Reset(bool);
    impl Drop for Reset {
        fn drop(&mut self) {
            RECORDING.set(self.0);
        }
    }
    let reset = Reset(RECORDING.replace(true));
    let ready = super::stages::build_full(validated, &reference)?;
    drop(reset);
    let plan = NativePlan::of(action, &ready)?;
    if let Err(error) = check(&plan, &ready.summaries) {
        if mode == Mode::VerifyReuse && cached.is_some() {
            return Err(error);
        }
        stats.fallback = Some("unsupported-production-evidence");
        return Ok(Prepared { ready, stats });
    }
    let encoded = encode(&plan);
    stats.metadata_bytes = encoded.len();
    if mode == Mode::VerifyReuse {
        if let Some(old) = cached {
            if old != plan {
                return Err(failed("VerifyReuse differs from cached native plan"));
            }
            // Also re-read every blob and check its bound metadata and full bytes.
            assemble(&old, validated, options, store, &mut stats)
                .map_err(|e| failed(format!("VerifyReuse: {e}")))?;
        }
    }
    // Content first, association last. Failed writes/eviction remain misses.
    publish(store, action, &encoded);
    Ok(Prepared { ready, stats })
}
fn failed(message: impl Into<String>) -> NativeError {
    NativeError::new(StageName::Import, message)
}

impl NativePlan {
    fn of(action: ContentHash, ready: &ReadyProgram) -> Result<Self, NativeError> {
        let objects = ready
            .unit_objects
            .as_ref()
            .ok_or_else(|| failed("no object production"))?;
        let production = ready
            .production
            .as_ref()
            .ok_or_else(|| failed("no plan production evidence"))?;
        Ok(Self {
            ahead_units: ready.ahead_units.iter().map(|u| u.unit().clone()).collect(),
            ahead_environments: ready
                .ahead_environments
                .iter()
                .map(|(k, v)| (*k, *v))
                .collect(),
            schema: SCHEMA,
            producer: ContentHash(crate::COMPILER_IDENTITY),
            action,
            table: ready.table.clone(),
            statics: ready.statics.clone(),
            owners: ready
                .semantic_owners
                .iter()
                .map(|(a, b)| (a.clone(), b.clone()))
                .collect(),
            invocation_bodies: ready.operations.invocation_bodies.clone(),
            pretransfer_adapters: ready
                .operations
                .pretransfer_adapters
                .iter()
                .map(|row| {
                    let abi = ready
                        .table
                        .fn_abis
                        .get(row.abi.0 as usize)
                        .ok_or_else(|| failed("adapter ABI outside current table"))?;
                    Ok(AdapterBinding {
                        adapter: row.adapter.clone(),
                        primitive: row.primitive.clone(),
                        abi: ready.keys.fn_abi(abi),
                    })
                })
                .collect::<Result<_, NativeError>>()?,
            functions: ready
                .functions
                .iter()
                .map(|(a, b)| (a.clone(), *b))
                .collect(),
            finalizers: ready.finalizers.iter().map(|(a, b)| (*a, *b)).collect(),
            inline: ready.inline_instances.iter().copied().collect(),
            callbacks: ready.callbacks.iter().cloned().collect(),
            tracking: ready.tracking(),
            generic_units: objects.generic_units as u64,
            records: production.records.clone(),
        })
    }
}
fn encode(plan: &NativePlan) -> Vec<u8> {
    let mut bytes = MAGIC.to_vec();
    bytes.extend_from_slice(hirsel_codec::to_text(plan).as_bytes());
    bytes
}
fn retrieve(store: &dyn ObjectStore, action: ContentHash, stats: &mut Stats) -> Option<NativePlan> {
    stats.fallback = Some("association-missing");
    let association = store.get_produced(&action_key(action))?;
    let key: UnitKey =
        hirsel_codec::from_text(std::str::from_utf8(association.bytes()).ok()?).ok()?;
    stats.fallback = Some("metadata-missing-or-corrupt");
    let bytes = store.get(&key)?;
    if output_key(&bytes) != key {
        return None;
    }
    let text = std::str::from_utf8(bytes.strip_prefix(MAGIC)?).ok()?;
    let plan: NativePlan = hirsel_codec::from_text(text).ok()?;
    if plan.schema != SCHEMA
        || plan.producer != ContentHash(crate::COMPILER_IDENTITY)
        || plan.action != action
    {
        return None;
    }
    stats.metadata_bytes = bytes.len();
    stats.fallback = None;
    Some(plan)
}
fn publish(store: &dyn ObjectStore, action: ContentHash, bytes: &[u8]) {
    let output = output_key(bytes);
    store.put(&output, bytes);
    // Do not associate a failed content publication.
    if store.get(&output).as_deref() == Some(bytes) {
        store.put(
            &action_key(action),
            hirsel_codec::to_text(&output).as_bytes(),
        );
    }
}

fn assemble(
    plan: &NativePlan,
    validated: &ValidatedNativeInput,
    options: &Options,
    store: &dyn ObjectStore,
    stats: &mut Stats,
) -> Result<ReadyProgram, NativeError> {
    let objects = options
        .objects
        .as_ref()
        .ok_or_else(|| failed("object-backed assembly required"))?;
    let keys = LayoutKeys::new(&plan.table);
    let layouts: BTreeMap<_, _> = keys
        .all()
        .iter()
        .copied()
        .enumerate()
        .rev()
        .map(|(i, k)| (k, hirsel_lir::layout_types::LayoutId(i as u32)))
        .collect();
    let abis: BTreeMap<_, _> = plan
        .table
        .fn_abis
        .iter()
        .enumerate()
        .rev()
        .map(|(i, a)| (keys.fn_abi(a), FnAbiId(i as u32)))
        .collect();
    let mut fragments = Vec::with_capacity(plan.records.len());
    for record in &plan.records {
        let bytes = store
            .get(&record.key)
            .ok_or_else(|| failed("missing plan fragment"))?;
        if fragment_digest(&bytes) != record.digest {
            return Err(failed("changed plan fragment"));
        }
        stats.fragment_bytes += bytes.len();
        let fragment = UnitFragment::decode(&bytes, &|k| layouts.get(k).copied(), &|k| {
            abis.get(k).copied()
        })
        .map_err(failed)?;
        if fragment.object.target != options.target.digest() {
            return Err(failed("object target differs"));
        }
        fragments.push(fragment);
    }
    let summaries: Vec<_> = fragments.iter().map(|f| f.summary.clone()).collect();
    check(plan, &summaries)?;
    let selected: BTreeMap<_, _> = summaries
        .iter()
        .map(|s| ((s.definition.clone(), s.instance), s))
        .collect();
    for (fragment, record) in fragments.iter().zip(&plan.records) {
        if record.calls.len() != fragment.templates.len() + 1
            || record.calls[1..]
                .iter()
                .zip(&fragment.templates)
                .any(|(c, s)| c.definition != s.definition || c.instance != s.instance)
        {
            return Err(failed("copied call evidence differs"));
        }
        for template in &fragment.templates {
            if selected
                .get(&(template.definition.clone(), template.instance))
                .copied()
                != Some(template)
            {
                return Err(failed("copied owner outside exact plan"));
            }
        }
    }
    let program = super::adapt::program(validated);
    let manifest = super::adapt::manifest(validated.snapshot()).map_err(failed)?;
    let mut operations = program.operation_registry();
    operations.invocation_bodies = plan.invocation_bodies.clone();
    operations.pretransfer_adapters = plan
        .pretransfer_adapters
        .iter()
        .map(|row| {
            Ok(hirsel_lir::pretransfer::AdapterReference {
                adapter: row.adapter.clone(),
                primitive: row.primitive.clone(),
                abi: *abis
                    .get(&row.abi)
                    .ok_or_else(|| failed("adapter ABI outside current table"))?,
            })
        })
        .collect::<Result<_, NativeError>>()?;
    // Re-read the current original source. Cached unit anchors never select its
    // body or its invocation rows, including for a specialized source alias.
    let owners: BTreeMap<_, _> = plan.owners.iter().cloned().collect();
    for row in &operations.invocation_bodies {
        let owner = owners
            .get(&row.definition)
            .and_then(Option::as_ref)
            .ok_or_else(|| failed("source invocation owner missing"))?;
        let original = program
            .definitions
            .get(owner)
            .ok_or_else(|| failed("source invocation owner absent from current input"))?;
        let rows = operations
            .invocations
            .get(owner)
            .cloned()
            .ok_or_else(|| failed("current source invocation inventory missing"))?;
        let evidence = hirsel_rc::invocations::bind_source(
            original,
            rows,
            &Symbol {
                kind: hirsel_lir::ir::SymbolKind::Function,
                definition: row.definition.clone(),
                instance: Some(row.instance),
            },
            hirsel_tir::BodyId(row.body),
        )
        .map_err(failed)?;
        operations
            .invocations
            .insert(row.definition.clone(), evidence.invocations);
    }
    let inputs = hirsel_lir::validate::Inputs {
        memo: Default::default(),
        target: &options.target,
        layouts: &plan.table,
        manifest: &manifest,
        operations: &operations,
        discharge: None,
    };
    hirsel_lir::validate::ValidationContext::new(&inputs)
        .preflight()
        .map_err(|e| failed(format!("plan layout preflight: {e:?}")))?;
    let ahead_units = plan
        .ahead_units
        .iter()
        .cloned()
        .map(|unit| {
            hirsel_lir::validate::validate_ready(unit, &inputs)
                .map(std::sync::Arc::new)
                .map_err(|e| failed(format!("cached ahead Ready validation: {e:?}")))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let mut ready = ReadyProgram {
        ahead_units,
        ahead_environments: plan.ahead_environments.iter().copied().collect(),
        entries: super::stages::summary_entries(&summaries),
        callee_facts: None,
        foreign_dispatch: BTreeMap::new(),
        semantic_owners: plan.owners.iter().cloned().collect(),
        semantic_units: None,
        production: Some(Production {
            records: plan.records.clone(),
        }),
        validation: options.validation,
        operations,
        launch: options.launch.clone(),
        input: program.input.clone(),
        target: options.target.clone(),
        table: plan.table.clone(),
        keys,
        manifest,
        units: Vec::new(),
        summaries,
        unit_objects: Some(super::stages::UnitObjectArtifacts {
            backend: objects.backend,
            opt: objects.opt,
            debug: objects.debug,
            objects: fragments.into_iter().map(|f| f.object).collect(),
            hits: plan.records.len(),
            generic_units: plan.generic_units as usize,
        }),
        inline_instances: plan.inline.iter().copied().collect(),
        callbacks: plan.callbacks.iter().cloned().collect(),
        finalizers: plan.finalizers.iter().copied().collect(),
        functions: plan.functions.iter().cloned().collect(),
        launches: program
            .signatures
            .iter()
            .filter_map(|(p, s)| {
                super::link::launch_kind(&program.types, s).map(|k| (p.clone(), k))
            })
            .collect(),
        revisions: BTreeMap::new(),
        exit_reports: BTreeMap::new(),
        statics: plan.statics.clone(),
        discharged: Default::default(),
        foreign_links: Vec::new(),
        data_abi: None,
        foreign_unreplayable: false,
        escaping: BTreeSet::new(),
        fuel: objects.fuel,
        failure_table: None,
    };
    ready.bind_revisions(validated)?;
    ready.check_semantic_units(&ready.unit_objects.as_ref().unwrap().objects)?;
    Ok(ready)
}

// Independent relation check, ported from hirsel_lir::validate::arena::check_calls
// (335–376 at a9a96640). No arena inference is shared with arena_credit.
fn check(
    plan: &NativePlan,
    summaries: &[hirsel_lir::fragment::UnitSummary],
) -> Result<(), NativeError> {
    let targets: BTreeMap<_, _> = summaries
        .iter()
        .map(|s| ((&s.definition, s.instance), s))
        .collect();
    if targets.len() != summaries.len() || summaries.len() != plan.records.len() {
        return Err(failed("incomplete or duplicate plan selection"));
    }
    let owners: BTreeMap<_, _> = plan.owners.iter().cloned().collect();
    if owners.len() != plan.owners.len()
        || summaries
            .iter()
            .any(|s| !owners.contains_key(&s.definition))
    {
        return Err(failed("incomplete or duplicate semantic owners"));
    }
    let keys = LayoutKeys::new(&plan.table);
    let abis: BTreeMap<_, _> = plan
        .table
        .fn_abis
        .iter()
        .map(|abi| (keys.fn_abi(abi), abi))
        .collect();
    // Every source selection and materialized keyed adapter keeps independent
    // evidence even when no candidate Value/Ready body is rebuilt on a hit.
    let source: BTreeSet<_> = summaries
        .iter()
        .filter(|s| s.origin == hirsel_lir::ir::UnitOrigin::Source)
        .map(|s| (&s.definition, s.instance))
        .collect();
    let bodies: BTreeSet<_> = plan
        .invocation_bodies
        .iter()
        .map(|r| (&r.definition, r.instance))
        .collect();
    if bodies.len() != plan.invocation_bodies.len() || bodies != source {
        return Err(failed(
            "missing, extra or duplicate source invocation body inventory",
        ));
    }
    let expected: BTreeSet<_> = summaries
        .iter()
        .filter(|s| hirsel_lir::pretransfer::primitive(&s.definition).is_some())
        .map(|s| (&s.definition, s.instance))
        .collect();
    let mut adapters = BTreeSet::new();
    for row in &plan.pretransfer_adapters {
        let key = (
            &row.adapter.definition,
            row.adapter
                .instance
                .ok_or_else(|| failed("adapter instance missing"))?,
        );
        if !adapters.insert(key)
            || !expected.contains(&key)
            || row.adapter.kind != hirsel_lir::ir::SymbolKind::Function
            || row.primitive.kind != hirsel_lir::ir::SymbolKind::Function
            || hirsel_lir::pretransfer::primitive(&row.adapter.definition).as_ref()
                != Some(&row.primitive.definition)
        {
            return Err(failed("invalid or duplicate keyed adapter inventory"));
        }
        let adapter = targets[&key];
        let primitive = row
            .primitive
            .instance
            .and_then(|instance| targets.get(&(&row.primitive.definition, instance)))
            .ok_or_else(|| failed("keyed adapter primitive absent from selection"))?;
        let abi = abis
            .get(&row.abi)
            .ok_or_else(|| failed("adapter ABI outside catalog"))?;
        let actual = plan
            .table
            .fn_abis
            .get(adapter.abi.0 as usize)
            .ok_or_else(|| failed("adapter entry ABI outside catalog"))?;
        let primitive_abi = plan
            .table
            .fn_abis
            .get(primitive.abi.0 as usize)
            .ok_or_else(|| failed("primitive entry ABI outside catalog"))?;
        if *abi != actual || actual != primitive_abi {
            return Err(failed("keyed adapter ABI differs from selected bodies"));
        }
    }
    if adapters != expected {
        return Err(failed("missing keyed adapter inventory"));
    }
    for (record, summary) in plan.records.iter().zip(summaries) {
        if record
            .calls
            .first()
            .is_none_or(|c| c.definition != summary.definition || c.instance != summary.instance)
        {
            return Err(failed("missing own Ready call evidence"));
        }
        for row in &record.calls {
            let summary = targets
                .get(&(&row.definition, row.instance))
                .ok_or_else(|| failed("call evidence outside selection"))?;
            let named: Vec<_> = row.calls.iter().filter_map(|c| c.target.clone()).collect();
            if summary.calls != named {
                return Err(failed("call inventory differs from produced fragment"));
            }
            for call in &row.calls {
                let abi = *abis
                    .get(&call.abi)
                    .ok_or_else(|| failed("call ABI outside catalog"))?;
                if let Some(symbol) = &call.direct {
                    let callee = symbol
                        .instance
                        .and_then(|i| targets.get(&(&symbol.definition, i)));
                    if let Some(callee) = callee {
                        let actual = &plan.table.fn_abis[callee.abi.0 as usize];
                        if abi.nonretaining != actual.nonretaining
                            || abi.nonretaining_captures != actual.nonretaining_captures
                            || abi.scoped_contract != actual.scoped_contract
                            || abi.scoped_entries != actual.scoped_entries
                        {
                            return Err(failed("call forges its scoped body contract"));
                        }
                        if abi.result_arena.is_some() && actual != abi {
                            return Err(failed("call forges its complete result-arena ABI"));
                        }
                    } else if abi.result_arena.is_some() {
                        return Err(failed("result-arena callee absent"));
                    }
                } else if abi.result_arena.is_some() {
                    return Err(failed("an indirect call claims a result arena"));
                }
            }
        }
    }
    if plan.tracking != summaries.iter().any(|s| s.recovers) {
        return Err(failed("inconsistent global tracking policy"));
    }
    let callbacks: BTreeSet<_> = summaries
        .iter()
        .flat_map(|s| s.glue.callbacks.iter().map(|(p, _)| p.clone()))
        .collect();
    if callbacks.iter().ne(plan.callbacks.iter()) {
        return Err(failed("inconsistent global callback policy"));
    }
    for s in summaries {
        if plan.table.fn_abis.get(s.abi.0 as usize).is_none() {
            return Err(failed("entry ABI outside catalog"));
        }
    }
    for (layout, instance) in &plan.finalizers {
        if plan.table.layout(*layout).is_none()
            || !summaries.iter().any(|s| s.instance == *instance)
        {
            return Err(failed("missing finalizer layout/owner"));
        }
    }
    for (path, instance) in &plan.functions {
        if !targets.contains_key(&(path, *instance)) {
            return Err(failed("missing function owner"));
        }
    }
    Ok(())
}

/// Broad lane-1 manifest. Exhaustive destructuring intentionally breaks the
/// audit when a NativeInput/Options field is added. No process address is a key.
/// Existing signature fingerprints and source revisions are checked products;
/// missing lossless identities are encoded with the existing type/anchor codecs.
fn input_manifest(input: &NativeInput, options: &Options) -> Option<Vec<u8>> {
    let NativeInput {
        types,
        environment,
        operations,
        registry_revision,
        definitions,
        signatures,
        signature_dependencies,
        representations,
        source_owners,
        sources,
        constants,
        constant_definitions,
        materialized,
        completed_source_stages,
        audits,
        instances,
        instance_bodies,
        storage_profiles,
        foreign_links,
        foreign_unreplayable,
        foreign_threads,
    } = input;
    if *completed_source_stages != hirsel_engine::NativeStatus::Complete {
        return None;
    }
    let Options {
        validation: _,
        launch: _,
        target,
        regions,
        elide,
        reuse,
        flat_walk,
        scoped_specialization,
        discharge,
        objects,
    } = options;
    // Launch inputs are rebound on every request, never persisted. Validation
    // mode selects recomputation, not different emitted bytes. Empty discharge
    // and V=On were checked before this function. Worker count/salt is topology.
    if !discharge.0.is_empty() {
        return None;
    }
    let objects = objects.as_ref()?;
    let mut parts = vec![
        b"hirsel.native-plan.manifest.v1".to_vec(),
        crate::COMPILER_IDENTITY.to_vec(),
        hirsel_codec::to_text(target).into_bytes(),
        serde_json::to_vec(&(
            environment.interface_version,
            &environment.compiler_version,
            environment.spec_hash,
            environment.package_manifest_hash,
            environment.builtin_signatures_hash,
            registry_revision,
        ))
        .ok()?,
        serde_json::to_vec(&(
            hirsel_tir::TIR_VERSION,
            hirsel_lir::LIR_VERSION,
            hirsel_lir::INTERFACE_VERSION,
            hirsel_lir::VALIDATOR_VERSION,
            hirsel_lir::LAYOUT_VERSION,
            hirsel_lir::ABI_VERSION,
            hirsel_lir::RUNTIME_ABI_VERSION,
            hirsel_rt::ABI_VERSION,
        ))
        .ok()?,
        serde_json::to_vec(&(
            objects.backend.name(),
            format!("{:?}", objects.opt),
            objects.debug,
            objects.fuel,
            regions,
            elide,
            reuse,
            flat_walk,
            scoped_specialization,
            false, /* measurement */
            hirsel_rc::closure_calls::MAX_PER_FUNCTION,
            hirsel_rc::closure_calls::MAX_SPECIALIZATIONS,
        ))
        .ok()?,
        hirsel_cranelift::CRANELIFT_VERSION.as_bytes().to_vec(),
        super::llvm_version().into_bytes(),
        hirsel_codec::to_text(&super::adapt::manifest(input).ok()?).into_bytes(),
    ];
    // Ordinary assembly's exact external artifacts. Cargo-fallback/custom
    // toolchains cannot yet supply a frozen complete identity at this seam.
    // Full content identities, shared with the link command. Metadata reuse
    // requires a non-racy ctime/device/inode match as well as path/size/mtime.
    parts.push(link_identity(target, objects.debug)?);
    if !storage_profiles.is_empty() {
        parts.push(b"hirsel.native-plan.storage-profiles.v1".to_vec());
        parts.push(serde_json::to_vec(storage_profiles).ok()?);
    }
    // Named foreign libraries lack a resolved byte identity at this seam.
    for link in foreign_links {
        let hirsel_engine::LinkInput::File(path) = link else {
            return None;
        };
        parts.push(path.as_os_str().as_encoded_bytes().to_vec());
        parts.push(std::fs::read(path).ok()?);
    }
    parts.push(vec![u8::from(*foreign_unreplayable)]);
    parts.push(
        serde_json::to_vec(
            &foreign_threads
                .iter()
                .map(|(p, t)| (p, t.name()))
                .collect::<Vec<_>>(),
        )
        .ok()?,
    );
    // These fingerprints are engine-produced complete signature identities;
    // current native import already checks each whole declaration. Reuse them
    // instead of canonically serializing the builtin universe on every hit.
    for (path, signature) in signatures {
        parts.push(
            serde_json::to_vec(&(
                path,
                signature.fingerprint,
                signature_dependencies.get(path),
            ))
            .ok()?,
        );
    }
    // Registry equality has already been independently checked by import.
    for (key, record) in operations {
        parts.push(
            serde_json::to_vec(&(
                &key.definition,
                key.declaration_digest,
                record.declaration.fingerprint,
            ))
            .ok()?,
        );
    }
    // Per-definition source ordinals come from the existing anchor codec. It
    // collects spans in product traversal order, then erases process-local IDs.
    let owners: Vec<_> = source_owners
        .iter()
        .map(|(p, o)| {
            (
                p,
                o.implementation_revision,
                o.syntax.owner().site(hirsel_types::NameSite::Item),
                o.generated.as_ref().map(|g| (&g.text, g.site)),
            )
        })
        .collect();
    let edges: Vec<_> = instances
        .edges
        .iter()
        .map(|e| {
            (
                &e.owner,
                e.owner_binder,
                e.site,
                match e.kind {
                    hirsel_engine::InstanceKind::Call => 0u8,
                    hirsel_engine::InstanceKind::Reference => 1,
                    hirsel_engine::InstanceKind::Drop => 2,
                    hirsel_engine::InstanceKind::Representation => 3,
                    hirsel_engine::InstanceKind::Materialized => 4,
                },
                &e.callee,
                e.binder,
                &e.arguments,
            )
        })
        .collect();
    // V's context is all declarations, representations, generated selections,
    // exact TIR and current type closure below; it is compiler/verifier-bound.
    let encode_products = || {
        serde_json::to_vec(&(
            definitions.iter().collect::<Vec<_>>(),
            signature_dependencies.iter().collect::<Vec<_>>(),
            owners.clone(),
            constants.iter().collect::<Vec<_>>(),
            constant_definitions,
            materialized.iter().collect::<Vec<_>>(),
            audits.iter().collect::<Vec<_>>(),
            &edges,
            instance_bodies,
        ))
    };
    let mut anchors = hirsel_span::AnchorCodec::Collect(Vec::new());
    hirsel_span::with_anchor_codec(&mut anchors, || {
        hirsel_types::with_table_collection(&encode_products).0
    })
    .ok()?;
    let hirsel_span::AnchorCodec::Collect(anchors) = anchors else {
        unreachable!()
    };
    let mut codec = hirsel_span::AnchorCodec::Encode(
        anchors
            .iter()
            .enumerate()
            .map(|(i, a)| (*a, i as u32 + 1))
            .collect(),
    );
    let products =
        hirsel_span::with_anchor_codec(&mut codec, || canonical(types, &encode_products))?;
    parts.push(products);
    // All representation fields matter (private shapes, promises, finalizers,
    // generic binders and FFI). No assumed completeness of revision alone.
    for (path, record) in representations {
        parts.push(serde_json::to_vec(path).ok()?);
        parts.push(match record {
            None => b"absent".to_vec(),
            Some(r) => canonical(types, || {
                serde_json::to_vec(&(
                    &r.definition,
                    r.representation_revision,
                    &r.generics,
                    r.private_shape,
                    (
                        r.layout_promises.c,
                        r.layout_promises.packed,
                        r.layout_promises.align,
                        r.layout_promises.soa,
                        r.layout_promises.flat,
                    ),
                    r.resource,
                    &r.finalizer,
                    r.unsafe_ffi_package,
                ))
            })?,
        });
    }
    // Paths/lines are fresh output metadata, not reusable code. Exact relative
    // span topology and selected source revision are already encoded above;
    // unresolved nonzero anchors make the record ineligible.
    for anchor in anchors {
        if anchor != hirsel_span::Anchor(0) && sources.anchor(anchor).is_none() {
            return None;
        }
    }
    // Explicit membership framing binds absent lookups as well as present rows.
    let mut out = Vec::new();
    for part in parts {
        out.extend_from_slice(&(part.len() as u64).to_le_bytes());
        out.extend(part);
    }
    Some(out)
}
fn link_identity(target: &hirsel_lir::TargetSpec, debug: bool) -> Option<Vec<u8>> {
    use std::path::PathBuf;
    let runtime = PathBuf::from(std::env::var_os("HIRSEL_RUNTIME_ARCHIVES")?);
    let kit = PathBuf::from(std::env::var_os("HIRSEL_LINK_KIT")?);
    let llvm = PathBuf::from(std::env::var_os("LLVM_SYS_231_PREFIX")?);
    let mut files = vec![
        runtime.join(&target.triple).join("libhirsel_builtins.a"),
        runtime.join(&target.triple).join("default").join(if debug {
            "debug/libhirsel_rt.a"
        } else {
            "libhirsel_rt.a"
        }),
        kit.join("manifest.json"),
        kit.join("digest"),
        llvm.join("bin/ld.lld"),
        std::env::current_exe().ok()?,
    ];
    let maps = std::fs::read_to_string("/proc/self/maps").ok()?;
    let loaded_llvm = maps
        .lines()
        .filter_map(|line| line.split_whitespace().nth(5))
        .find(|path| path.contains("/libLLVM") && path.starts_with('/'))?;
    files.push(PathBuf::from(loaded_llvm));
    // The approved kit manifest content-names all members. Existing link kit
    // checking and normal linker rebinding remain mandatory after assembly.
    files.sort();
    let mut out = Vec::new();
    for path in files {
        out.extend_from_slice(&super::file_digest::file(&path).ok()?.raw);
    }
    Some(out)
}
fn canonical(
    types: &hirsel_types::TypeInterner,
    encode: impl Fn() -> Result<Vec<u8>, serde_json::Error>,
) -> Option<Vec<u8>> {
    let (probe, roots) = hirsel_types::with_table_collection(&encode);
    probe.ok()?;
    let table = hirsel_types::encode_table(types, &roots).0;
    let bytes = hirsel_types::with_table_encoding(types, &roots, encode).ok()?;
    let table = serde_json::to_vec(&table).ok()?;
    let mut framed = Vec::with_capacity(table.len() + bytes.len() + 8);
    framed.extend_from_slice(&(table.len() as u64).to_le_bytes());
    framed.extend(table);
    framed.extend(bytes);
    Some(framed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};
    #[derive(Default)]
    struct Store {
        rows: Mutex<BTreeMap<UnitKey, Arc<[u8]>>>,
        produced: hirsel_lir::objects::SimObjects,
        drop_writes: std::sync::atomic::AtomicBool,
    }
    impl ObjectStore for Store {
        fn production_enabled(&self) -> bool {
            true
        }
        fn get_produced(&self, key: &UnitKey) -> Option<hirsel_lir::objects::ProducedBytes> {
            self.get(key)?;
            self.produced.get_produced(key)
        }
        fn get(&self, key: &UnitKey) -> Option<Arc<[u8]>> {
            self.rows.lock().unwrap().get(key).cloned()
        }
        fn put(&self, key: &UnitKey, bytes: &[u8]) {
            if !self.drop_writes.load(std::sync::atomic::Ordering::Relaxed) {
                self.rows.lock().unwrap().insert(*key, bytes.into());
                self.produced.put(key, bytes);
            }
        }
    }
    const SOURCE: &str = "fn helper(x:u64)->u64{x+1u64}\nfn main()->u64{helper(7u64)}";
    fn input(source: &str, verify: Verify) -> ValidatedNativeInput {
        let mut engine = hirsel_engine::QueryEngine::new(
            hirsel_engine::default_environment(),
            crate::compiler_adapters(),
        );
        engine.verify = verify;
        let file = engine.sources.add("plan.hir", source.into()).unwrap();
        let checked =
            hirsel_engine::check_progress(&mut engine, file, &hirsel_engine::default_context())
                .unwrap();
        assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
        let input = engine.export_native_input().unwrap();
        engine.import_native_input(input).unwrap()
    }
    pub(super) fn options(store: Arc<dyn ObjectStore>, jobs: usize) -> Options {
        let mut options = Options::new(hirsel_lir::TargetSpec::x86_64_unknown_linux_gnu());
        options.validation = Validation::Ready;
        options.objects = Some(super::super::units::UnitObjects {
            store,
            backend: super::super::Backend::Cranelift,
            opt: hirsel_lir::emission::OptLevel::None,
            debug: false,
            fuel: false,
            jobs: hirsel_intern::jobs::Jobs::new(jobs),
            byte_budget: None,
        });
        options
    }
    pub(super) fn count(c: hirsel_intern::timings::Count) -> u64 {
        hirsel_intern::timings::report()
            .counts
            .iter()
            .find(|(k, _)| *k == c)
            .unwrap()
            .1
    }
    #[test]
    fn bodyless_plan_rechecks_home_reaching_callbacks() {
        hirsel_engine::with_compiler_stack(|| {
            use serde_json::json;
            let scratch = super::super::executable::Scratch::new().unwrap();
            let root = &scratch.0;
            let mut dependencies = Vec::new();
            for (name, source, wrapper) in [
                (
                    "home",
                    "/// Reads nothing and retains nothing.\nextern \"c\" fn tid(u:Unsafe)->u64\npub fn read(u:Unsafe)->u64{tid(u)}\n",
                    "read",
                ),
                (
                    "away",
                    "pub type Probe { u:Unsafe }\n/// Calls f during this call and retains nothing.\nextern \"c\" fn invoke(u:Unsafe,mut p:unique Probe,f:FnPtr[fn(mut state:unique Probe,id:u64)->u64])->u64\npub fn call(u:Unsafe,mut p:unique Probe,f:FnPtr[fn(mut state:unique Probe,id:u64)->u64])->u64{invoke(u,mut p,f)}\n",
                    "call",
                ),
            ] {
                let dir = root.join(name);
                std::fs::create_dir_all(&dir).unwrap();
                std::fs::write(dir.join("lib.hir"), source).unwrap();
                // Only object emission is exercised; no foreign code is linked.
                std::fs::write(dir.join("link.o"), b"callback-link-identity").unwrap();
                let manifest = json!({"schema":1,"package":{"name":name,"version":"1"},
                    "sources":[{"path":"lib.hir","module":[]}],"dependencies":[],
                    "unsafe_ffi":{"link":["link.o"],"headers_checked":false,
                        "wrappers":[{"module":[],"name":wrapper,"kind":"effect","model":"none"}]}});
                std::fs::write(dir.join("hirsel.json"), manifest.to_string()).unwrap();
                dependencies.push(json!({"manifest":format!("{name}/hirsel.json"),
                    "package":{"name":name,"version":"1"},"expose":[{"head":name,"module":[]}]}));
            }
            std::fs::write(root.join("main.hir"), "fn sneak(mut p:away.Probe,id:u64)->u64{home.read(p.u)}\npub fn main(u:Unsafe)->Result[(),()]{let mut p=away.Probe(u:u);let _=away.call(u,mut p,sneak);Ok(())}\n").unwrap();
            let manifest = json!({"schema":1,"package":{"name":"app","version":"1"},
                "entry":{"module":[],"name":"main"},"sources":[{"path":"main.hir","module":[]}],
                "dependencies":dependencies});
            let path = root.join("hirsel.json");
            std::fs::write(&path, manifest.to_string()).unwrap();
            let mut loaded = crate::load_program(&path).unwrap();
            loaded.check_for_execution().unwrap();
            let snapshot = loaded.engine.export_native_input().unwrap();
            let accepted = loaded.engine.import_native_input(snapshot.clone()).unwrap();
            for backend in [
                super::super::Backend::Llvm,
                super::super::Backend::Cranelift,
            ] {
                let store = Arc::new(Store::default());
                let mut options = options(store.clone(), 1);
                options.objects.as_mut().unwrap().backend = backend;
                let first = prepare_in(&accepted, &options, Mode::Reuse).unwrap();
                assert!(!first.stats.hit);
                let hit = prepare_in(&accepted, &options, Mode::Reuse).unwrap();
                assert!(hit.stats.hit, "{:?}", hit.stats);
                assert!(hit.ready.units.is_empty());
                assert!(hit.ready.callbacks.iter().any(|p| p.0.name == "sneak"));
                let action =
                    ContentHash::of_parts("callback-policy-control", [b"same-code".as_slice()]);
                let plan = NativePlan::of(action, &first.ready).unwrap();
                for threads in [
                    hirsel_types::ForeignThreads::Any,
                    hirsel_types::ForeignThreads::Serial,
                ] {
                    let mut changed = snapshot.clone();
                    let package = changed
                        .foreign_threads
                        .keys()
                        .find(|p| p.name == "away")
                        .unwrap()
                        .clone();
                    changed.foreign_threads.insert(package, threads);
                    let rejected = loaded.engine.import_native_input(changed).unwrap();
                    let mut cold_options = options.clone();
                    cold_options.objects = None;
                    let cold = super::super::stages::build_full(&rejected, &cold_options)
                        .err()
                        .expect("cold callback refusal");
                    let warm = assemble(
                        &plan,
                        &rejected,
                        &options,
                        store.as_ref(),
                        &mut Stats::default(),
                    )
                    .err()
                    .expect("bodyless callback refusal");
                    assert_eq!(cold, warm, "{backend:?} {threads:?}");
                    assert!(
                        warm.message.contains("PROG-4")
                            && warm.message.contains("sneak")
                            && warm.message.contains("home@1/tid"),
                        "{warm}"
                    );
                }
            }
        });
    }
    #[test]
    fn whole_plan_cross_open_zero_value_work_and_recompute_mismatch() {
        let run = || {
            hirsel_engine::with_compiler_stack(|| {
                use hirsel_intern::timings::{self, Count};
                timings::enable();
                let mut observed = Vec::new();
                for jobs in [1, 2, 8] {
                    let store = Arc::new(Store::default());
                    let options = options(store.clone(), jobs);
                    let original = input(SOURCE, Verify::On);
                    let manifest = input_manifest(original.snapshot(), &options)
                        .expect("complete simple manifest");
                    let first = prepare_in(&original, &options, Mode::Reuse).unwrap();
                    assert!(!first.stats.hit);
                    assert!(!first.ready.summaries.is_empty());
                    // A separate importer/session reopens the same action, like Go's
                    // cache_test.go:22–60 cross-open vector.
                    let reopened = input(SOURCE, Verify::On);
                    assert_eq!(
                        input_manifest(reopened.snapshot(), &options),
                        Some(manifest)
                    );
                    let work = [
                        Count::ValueDecodes,
                        Count::ValueDecodeBytes,
                        Count::ValueRebinds,
                        Count::ImportEffectReplays,
                        Count::NativeAppendExpand,
                        Count::ClosureBodyScans,
                        Count::ArenaBodyScans,
                        Count::LateKeys,
                        Count::Units,
                    ];
                    let before = work.map(count);
                    let hit = prepare_in(&reopened, &options, Mode::Reuse).unwrap();
                    assert!(hit.stats.hit, "{:?}", hit.stats);
                    assert_eq!(
                        before,
                        work.map(count),
                        "complete hit did Value or late-key work"
                    );
                    assert_eq!(first.ready.summaries, hit.ready.summaries);
                    assert_eq!(
                        first.ready.unit_objects.as_ref().unwrap().objects,
                        hit.ready.unit_objects.as_ref().unwrap().objects
                    );
                    let verified = prepare_in(&reopened, &options, Mode::VerifyReuse).unwrap();
                    assert_eq!(first.ready.summaries, verified.ready.summaries);
                    assert!(count(Count::NativeAppendExpand) > before[4]);
                    let records = &first.ready.production.as_ref().unwrap().records;
                    observed.push(
                        records
                            .iter()
                            .map(|r| (r.key, r.digest))
                            .collect::<Vec<_>>(),
                    );
                    // Go cache_test.go:99–128 mismatch vector: metadata still decodes,
                    // but identical action cannot silently acquire different bytes.
                    let key = records[0].key;
                    let mut bytes = store.get(&key).unwrap().to_vec();
                    *bytes.last_mut().unwrap() ^= 1;
                    store.put(&key, &bytes);
                    let err = prepare_in(&reopened, &options, Mode::VerifyReuse)
                        .err()
                        .expect("changed output must fail");
                    assert!(err.message.contains("recomputation differs"), "{err}");
                }
                assert!(observed.windows(2).all(|w| w[0] == w[1]));
                format!("{observed:?}")
            })
        };
        if let Some(outputs) = hirsel_intern::under_salts(
            "native::plan::tests::whole_plan_cross_open_zero_value_work_and_recompute_mismatch",
            &[1, 17, 65537],
            run,
        ) {
            assert!(outputs.windows(2).all(|w| w[0] == w[1]));
        }
    }
    #[test]
    fn manifest_binds_edits_options_membership_and_foreign_bytes() {
        hirsel_engine::with_compiler_stack(|| {
            let store = Arc::new(Store::default());
            let options = options(store, 1);
            let input = input(SOURCE, Verify::On);
            let base = input_manifest(input.snapshot(), &options).unwrap();
            for source in [
                SOURCE.replace("1u64", "2u64"),
                format!("{SOURCE}\nfn other()->u64{{1u64}}"),
            ] {
                let changed = self::input(&source, Verify::On);
                assert_ne!(
                    Some(&base),
                    input_manifest(changed.snapshot(), &options).as_ref()
                );
            }
            let mut changed = options.clone();
            changed.flat_walk = !changed.flat_walk;
            assert_ne!(
                Some(&base),
                input_manifest(input.snapshot(), &changed).as_ref()
            );
            let mut changed = options.clone();
            changed.scoped_specialization = !changed.scoped_specialization;
            assert_ne!(
                Some(&base),
                input_manifest(input.snapshot(), &changed).as_ref()
            );
            let mut changed = options.clone();
            changed.objects.as_mut().unwrap().fuel = true;
            assert_ne!(
                Some(&base),
                input_manifest(input.snapshot(), &changed).as_ref()
            );
            let scratch = super::super::executable::Scratch::new().unwrap();
            let path = scratch.0.join("foreign.o");
            std::fs::write(&path, b"one").unwrap();
            let mut changed = input.snapshot().clone();
            changed
                .foreign_links
                .push(hirsel_engine::LinkInput::File(path.clone()));
            let before = input_manifest(&changed, &options).unwrap();
            std::fs::write(&path, b"two").unwrap();
            assert_ne!(Some(before), input_manifest(&changed, &options));
            changed.foreign_links = vec![hirsel_engine::LinkInput::Library("unknown".into())];
            assert!(input_manifest(&changed, &options).is_none());
        });
    }
    #[test]
    fn missing_corrupt_metadata_and_failed_publication_fall_back() {
        hirsel_engine::with_compiler_stack(|| {
            let store = Arc::new(Store::default());
            let options = options(store.clone(), 1);
            let input = input(SOURCE, Verify::On);
            let first = prepare_in(&input, &options, Mode::Reuse).unwrap();
            let action = NativePlan::of(
                ContentHash::of_parts(
                    "hirsel.native-plan.input.v1",
                    [input_manifest(input.snapshot(), &options)
                        .unwrap()
                        .as_slice()],
                ),
                &first.ready,
            )
            .unwrap()
            .action;
            let record = first.ready.production.as_ref().unwrap().records[0].key;
            store.rows.lock().unwrap().remove(&record);
            let missing = prepare_in(&input, &options, Mode::Reuse).unwrap();
            assert!(!missing.stats.hit);
            store.put(&action_key(action), b"truncated");
            let corrupt = prepare_in(&input, &options, Mode::Reuse).unwrap();
            assert!(!corrupt.stats.hit);
            store.rows.lock().unwrap().clear();
            store
                .drop_writes
                .store(true, std::sync::atomic::Ordering::Relaxed);
            prepare_in(&input, &options, Mode::Reuse).unwrap();
            assert!(store.get(&action_key(action)).is_none());
            let unverified = self::input(SOURCE, Verify::Off);
            assert_eq!(
                prepare_in(&unverified, &options, Mode::Reuse)
                    .unwrap()
                    .stats
                    .fallback,
                Some("verification-off")
            );
            let bodies = Options::new(options.target.clone());
            assert_eq!(
                prepare_in(&input, &bodies, Mode::Reuse)
                    .unwrap()
                    .stats
                    .fallback,
                Some("bodies-required")
            );
        });
    }
    #[test]
    fn independent_checker_rejects_forged_calls_owners_policy_and_untrusted_associations() {
        hirsel_engine::with_compiler_stack(|| {
            let store = Arc::new(Store::default());
            let options = options(store.clone(), 1);
            let input = input(SOURCE, Verify::On);
            let prepared = prepare_in(&input, &options, Mode::Reuse).unwrap();
            let bytes = input_manifest(input.snapshot(), &options).unwrap();
            let action = ContentHash::of_parts("hirsel.native-plan.input.v1", [bytes.as_slice()]);
            let plan = NativePlan::of(action, &prepared.ready).unwrap();
            let summaries = &prepared.ready.summaries;
            let mut forged = plan.clone();
            forged.owners.clear();
            assert!(check(&forged, summaries).is_err());
            let mut forged = plan.clone();
            forged.tracking = !forged.tracking;
            assert!(check(&forged, summaries).is_err());
            let mut forged = plan.clone();
            forged.callbacks.push(DefPath::core("unselected"));
            assert!(check(&forged, summaries).is_err());
            let mut forged = plan.clone();
            let mut abi = forged.table.fn_abis[0].clone();
            abi.result_arena = Some(0);
            let key = LayoutKeys::new(&forged.table).fn_abi(&abi);
            forged.table.fn_abis.push(abi);
            let mut contradictory = forged.clone();
            let call = contradictory
                .records
                .iter_mut()
                .flat_map(|r| r.calls.iter_mut())
                .flat_map(|r| r.calls.iter_mut())
                .find(|c| c.direct.is_some())
                .expect("written helper call");
            call.abi = key;
            assert!(
                check(&contradictory, summaries)
                    .unwrap_err()
                    .message
                    .contains("result-arena ABI")
            );
            call_direct_none(&mut contradictory);
            assert!(
                check(&contradictory, summaries)
                    .unwrap_err()
                    .message
                    .contains("indirect call")
            );
            let mut wrong_schema = plan.clone();
            wrong_schema.schema += 1;
            publish(store.as_ref(), action, &encode(&wrong_schema));
            assert!(!prepare_in(&input, &options, Mode::Reuse).unwrap().stats.hit);
            struct Unknown(Arc<Store>);
            impl ObjectStore for Unknown {
                fn get(&self, key: &UnitKey) -> Option<Arc<[u8]>> {
                    self.0.get(key)
                }
                fn put(&self, key: &UnitKey, bytes: &[u8]) {
                    self.0.put(key, bytes);
                }
            }
            let mut unknown = options.clone();
            unknown.objects.as_mut().unwrap().store = Arc::new(Unknown(store));
            assert!(
                !prepare_in(&input, &unknown, Mode::Reuse).unwrap().stats.hit,
                "coherent bytes supplied their own provenance"
            );
        });
    }
    #[test]
    fn bodyless_keyed_plan_retains_inventory_and_refuses_missing_or_stale_bindings() {
        hirsel_engine::with_compiler_stack(|| {
            let source = "fn main()->u64 {let m:Map[u64,u64]=map.empty();let get:fn(Map[u64,u64],u64)->Option[u64]=map.get;match get(m,1u64) {Some(x)=>x,None=>0u64}}";
            let input = input(source, Verify::On);
            let store = Arc::new(Store::default());
            let options = options(store.clone(), 1);
            let first = prepare_in(&input, &options, Mode::Reuse).unwrap();
            let warm = prepare_in(&input, &options, Mode::Reuse).unwrap();
            assert!(warm.stats.hit, "{:?}", warm.stats);
            assert!(warm.ready.units.is_empty());
            assert_eq!(
                warm.ready.operations.pretransfer_adapters,
                first.ready.operations.pretransfer_adapters
            );
            assert_eq!(warm.ready.operations.pretransfer_adapters.len(), 1);
            let action = ContentHash::of_parts(
                "hirsel.native-plan.input.v1",
                [input_manifest(input.snapshot(), &options)
                    .unwrap()
                    .as_slice()],
            );
            let plan = NativePlan::of(action, &first.ready).unwrap();
            check(&plan, &first.ready.summaries).unwrap();
            let rejected = |plan: &NativePlan| {
                assert!(check(plan, &first.ready.summaries).is_err());
                assert!(
                    assemble(
                        plan,
                        &input,
                        &options,
                        store.as_ref(),
                        &mut Stats::default()
                    )
                    .is_err()
                );
            };
            let mut missing = plan.clone();
            missing.pretransfer_adapters.clear();
            rejected(&missing);
            let mut forged = plan.clone();
            forged.pretransfer_adapters[0].abi = ContentHash::of_bytes(b"not an ABI");
            rejected(&forged);
            let mut duplicate = plan.clone();
            duplicate
                .pretransfer_adapters
                .push(plan.pretransfer_adapters[0].clone());
            rejected(&duplicate);
            let mut missing_body = plan.clone();
            missing_body.invocation_bodies.clear();
            rejected(&missing_body);
            let mut legacy = encode(&plan);
            legacy[..MAGIC.len()].copy_from_slice(b"HIRNPL02");
            publish(store.as_ref(), action, &legacy);
            assert!(retrieve(store.as_ref(), action, &mut Stats::default()).is_none());
            publish(store.as_ref(), action, &encode(&missing));
            let repaired = prepare_in(&input, &options, Mode::Reuse).unwrap();
            assert!(!repaired.stats.hit, "missing inventory was accepted");
            assert_eq!(
                repaired.ready.operations.pretransfer_adapters,
                first.ready.operations.pretransfer_adapters
            );
        });
    }

    fn call_direct_none(plan: &mut NativePlan) {
        for call in plan
            .records
            .iter_mut()
            .flat_map(|r| r.calls.iter_mut())
            .flat_map(|r| r.calls.iter_mut())
        {
            call.direct = None;
        }
    }
}

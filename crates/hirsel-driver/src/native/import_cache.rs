//! The pre-import half of the native cache. Keys describe the import's TIR,
//! instance, planned ABI and declaration/representation closure. Stored Value
//! LIR has content-named bindings; its recorded builder effects restore those
//! bindings before the whole-program Value passes run.
//!
//! Follows Go's complete action-key contract (cmd/go/internal/cache/cache.go:28)
//! and Cranelift's parameter-free cached stencil (incremental_cache.rs:36-95).
#[path = "import_envelope.rs"]
mod envelope;

use hirsel_lir::layout_types::TargetSpec;
use hirsel_lir::objects::{ObjectStore, UnitKey};
use hirsel_rc::import::{CachedImport, ImportCache};
use hirsel_rc::instance::{InstanceId, InstanceRole, Instances, Program};
use hirsel_syntax::{ContentHash, DefPath};
use hirsel_types::{TypeInterner, TypeKind};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

/// Profile policy wraps both cached imports and the no-cache path. Import
/// fragments precede specialization, so their reusable content stays unchanged.
pub(super) struct Scoped<'a> {
    pub inner: &'a mut dyn ImportCache,
    pub enabled: bool,
}
impl ImportCache for Scoped<'_> {
    fn scoped_specialization(&self) -> bool {
        self.enabled && self.inner.scoped_specialization()
    }
    fn enabled(&self) -> bool {
        self.inner.enabled()
    }
    fn get(
        &mut self,
        program: &Program,
        instances: &Instances,
        id: InstanceId,
    ) -> Option<CachedImport> {
        self.inner.get(program, instances, id)
    }
    fn put(&mut self, id: InstanceId, import: Result<CachedImport, String>) {
        self.inner.put(id, import);
    }
}

pub(crate) struct Imports<'a> {
    store: &'a dyn ObjectStore,
    target: &'a TargetSpec,
    types: Arc<TypeInterner>,
    /// Stage-checking harnesses always execute the importer too.
    enabled: bool,
    pending: BTreeMap<InstanceId, UnitKey>,
    pub hits: usize,
    pub misses: usize,
}

impl<'a> Imports<'a> {
    pub fn new(
        store: &'a dyn ObjectStore,
        target: &'a TargetSpec,
        types: Arc<TypeInterner>,
        verify: bool,
    ) -> Self {
        Self {
            store,
            target,
            types,
            enabled: store.enabled() && !verify,
            pending: BTreeMap::new(),
            hits: 0,
            misses: 0,
        }
    }
}

/// Serialize exact semantic data beside its canonical type table, never session ids.
fn canonical(
    types: &TypeInterner,
    encode: impl Fn() -> Result<Vec<u8>, serde_json::Error>,
) -> Option<Vec<u8>> {
    let (probe, roots) = hirsel_types::with_table_collection(&encode);
    probe.ok()?;
    let table = hirsel_types::encode_table(types, &roots).0;
    let value = hirsel_types::with_table_encoding(types, &roots, encode).ok()?;
    serde_json::to_vec(&(table, value)).ok()
}

fn named(types: &TypeInterner, roots: &[hirsel_types::TypeId]) -> Vec<DefPath> {
    hirsel_types::closure(types, roots.iter().copied())
        .iter()
        .filter_map(|id| match &types.get(*id)?.kind {
            TypeKind::Named { definition, .. } => Some(definition.path().clone()),
            _ => None,
        })
        .collect()
}

fn representation(
    record: &hirsel_tir::RepresentationRecord,
    types: &TypeInterner,
) -> Option<Vec<u8>> {
    canonical(types, || {
        serde_json::to_vec(&(
            &record.definition,
            record.representation_revision,
            &record.generics,
            record.private_shape,
            (
                record.layout_promises.c,
                record.layout_promises.packed,
                record.layout_promises.align,
                record.layout_promises.soa,
                record.layout_promises.flat,
            ),
            record.resource,
            &record.finalizer,
            record.unsafe_ffi_package,
        ))
    })
}

/// The TIR's exact item-relative sites matter to failure/debug metadata, but the
/// process-local anchors do not. Number anchors by their first span in the item.
fn definition(program: &Program, def: &hirsel_tir::Definition) -> Option<Vec<u8>> {
    let anchors = hirsel_tir::source_anchors(def)
        .into_iter()
        .enumerate()
        .map(|(i, anchor)| (anchor, i as u32 + 1))
        .collect();
    let mut codec = hirsel_span::AnchorCodec::Encode(anchors);
    hirsel_span::with_anchor_codec(&mut codec, || {
        canonical(&program.types, || serde_json::to_vec(def))
    })
}

fn inputs(
    program: &Program,
    instances: &Instances,
    id: InstanceId,
    target: &TargetSpec,
) -> Option<Vec<Vec<u8>>> {
    let view = instances.view(id);
    let actuals = &instances.actuals[id.index()];
    let roles = match instances.roles[id.index()] {
        InstanceRole::Function => "function".to_owned(),
        InstanceRole::Finalizer => "finalizer".to_owned(),
        InstanceRole::Closure { parent } => format!(
            "closure:{}",
            format!("{:?}", instances.view(parent).key.digest())
        ),
        InstanceRole::Library { body, finalizer } => format!("library:{body}:{finalizer}"),
    };
    let actuals_bytes = canonical(&program.types, || {
        serde_json::to_vec(&(
            actuals.binder,
            &actuals.arguments,
            &actuals.outer,
            instances.callables[id.index()],
        ))
    })?;
    let (_, roots) = hirsel_types::with_table_collection(|| {
        serde_json::to_vec(&(
            &actuals.arguments,
            &actuals.outer,
            instances.callables[id.index()],
        ))
    });
    let mut pending = named(&program.types, &roots);
    let path = view.key.definition();
    pending.push(path.clone());
    let mut facts = vec![
        crate::COMPILER_IDENTITY.to_vec(),
        hirsel_codec::to_text(target).into_bytes(),
        view.key.encoding().to_vec(),
        roles.into_bytes(),
        actuals_bytes,
        definition(program, view.definition().ok()?)?,
    ];
    facts.push(serde_json::to_vec(&program.revisions.get(path)).ok()?);
    // The native library can select registry roles and opaque container shapes
    // independently of a source call's explicit types. Bind those finite inputs.
    if let Some(signature) = program
        .signatures
        .get(path)
        .or_else(|| program.signatures.values().next())
    {
        let roles = &signature.metadata.roles;
        pending.extend(roles.unsafe_capability.iter().cloned());
        pending.extend(roles.result_type.iter().cloned());
        pending.extend(roles.tasks_capability.iter().cloned());
        pending.extend(roles.scope_capability.iter().cloned());
        if let Some(heap) = &roles.heap {
            pending.push(heap.arena.clone());
            pending.push(heap.operation.clone());
        }
    }
    for (container, record) in &program.owners.containers {
        facts.push(format!("{container:?}").into_bytes());
        facts.push(representation(record, &program.types)?);
        pending.extend(named(&program.types, &[record.private_shape]));
    }
    let mut seen = BTreeSet::new();
    let mut declarations = BTreeMap::new();
    while let Some(path) = pending.pop() {
        if !seen.insert(path.clone()) {
            continue;
        }
        let mut rows = vec![serde_json::to_vec(&path).ok()?];
        rows.push(serde_json::to_vec(&program.binders.get(&path)).ok()?);
        rows.push(vec![u8::from(program.definitions.contains_key(&path))]);
        if let Some(signature) = program.signatures.get(&path) {
            rows.push(
                hirsel_types::encoding::canonical_encoding(&program.types, signature, &[]).ok()?,
            );
            pending.extend(hirsel_proof_export::references(
                &program.types,
                &hirsel_proof_export::DefinitionContent::Body {
                    signature: signature.clone(),
                    tir: (path == *view.key.definition())
                        .then(|| view.definition().ok().cloned())
                        .flatten(),
                },
            ));
        }
        if let Some(owner) = program
            .owners
            .nominals
            .get(&hirsel_syntax::DefId::intern(&path))
        {
            use hirsel_layout::layout::OwnerRepresentation as O;
            match owner {
                O::Defined(record) => {
                    rows.push(representation(record, &program.types)?);
                    pending.extend(named(&program.types, &[record.private_shape]));
                }
                other => rows.push(format!("{other:?}").into_bytes()),
            }
        }
        if let Some(value) = program.constants.get(&path) {
            rows.push(canonical(&program.types, || serde_json::to_vec(value))?);
        }
        // Generated call selection is a declaration input, independent of which
        // monomorphic definitions happen to be numbered first in the program.
        let id = hirsel_syntax::DefId::intern(&path);
        if program.instance_bodies.declarations.contains(&id) {
            let mut selected = hirsel_tir::InstanceBodies::default();
            selected.declarations.insert(id);
            if let Some(bodies) = program.instance_bodies.instances.get(&id) {
                selected.instances.insert(id, bodies.clone());
            }
            rows.push(canonical(&program.types, || serde_json::to_vec(&selected))?);
        }
        declarations.insert(path, rows);
    }
    facts.extend(declarations.into_values().flatten());
    Some(facts)
}

fn key(
    program: &Program,
    instances: &Instances,
    id: InstanceId,
    target: &TargetSpec,
) -> Option<UnitKey> {
    let facts = inputs(program, instances, id, target)?;
    Some(UnitKey(ContentHash::of_parts(
        "hirsel.native.import.v1",
        facts.iter().map(Vec::as_slice),
    )))
}

// Stored effects deliberately do not use canonical(): that byte-array JSON
// encoding is an action-key contract. Keep its table collection and TypeId
// remapping, but store the table and raw effect JSON as separate bounded sections.
fn encode(import: &CachedImport, types: &TypeInterner) -> Option<Vec<u8>> {
    let encode = || serde_json::to_vec(&import.effects);
    let (probe, roots) = hirsel_types::with_table_collection(&encode);
    probe.ok()?;
    let table = hirsel_types::encode_table(types, &roots).0;
    let table = serde_json::to_vec(&table).ok()?;
    let effects = hirsel_types::with_table_encoding(types, &roots, encode).ok()?;
    let layouts: Vec<ContentHash> = import.layouts.iter().map(|k| k.0).collect();
    let unit = hirsel_codec::to_text(&import.unit);
    let layouts = serde_json::to_vec(&layouts).ok()?;
    let abis = serde_json::to_vec(&import.abis).ok()?;
    envelope::encode([unit.as_bytes(), &layouts, &abis, &table, &effects])
}

fn decode(bytes: &[u8], types: &TypeInterner) -> Option<CachedImport> {
    let [unit, layouts, abis, table, effects] = envelope::decode(bytes)?;
    let unit = std::str::from_utf8(unit).ok()?;
    let layouts: Vec<ContentHash> = serde_json::from_slice(layouts).ok()?;
    let abis = serde_json::from_slice(abis).ok()?;
    let table: hirsel_types::TypeTable = serde_json::from_slice(table).ok()?;
    let ids = hirsel_types::decode_table(types, &table);
    let effects =
        hirsel_types::with_table_decoding(&ids, || serde_json::from_slice(effects)).ok()?;
    hirsel_intern::timings::count(hirsel_intern::timings::Count::ValueDecodes);
    hirsel_intern::timings::count_by(
        hirsel_intern::timings::Count::ValueDecodeBytes,
        unit.len() as u64,
    );
    Some(CachedImport {
        unit: hirsel_codec::from_text(unit).ok()?,
        layouts: layouts.into_iter().map(hirsel_lir::LayoutKey).collect(),
        abis,
        effects,
    })
}

impl ImportCache for Imports<'_> {
    fn enabled(&self) -> bool {
        self.enabled
    }
    fn get(
        &mut self,
        program: &Program,
        instances: &Instances,
        id: InstanceId,
    ) -> Option<CachedImport> {
        if !self.enabled {
            return None;
        }
        let key = key(program, instances, id, self.target)?;
        self.pending.insert(id, key);
        let hit = self
            .store
            .get(&key)
            .and_then(|bytes| decode(&bytes, &program.types));
        if hit.is_some() {
            self.hits += 1;
        } else {
            self.misses += 1;
        }
        hit
    }
    fn put(&mut self, id: InstanceId, import: Result<CachedImport, String>) {
        let Some(key) = self.pending.remove(&id) else {
            return;
        };
        if let Ok(import) = import
            && let Some(bytes) = encode(&import, &self.types)
        {
            self.store.put(&key, &bytes);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn program(source: &str) -> Program {
        let mut engine = hirsel_engine::QueryEngine::new(
            hirsel_engine::default_environment(),
            crate::compiler_adapters(),
        );
        let file = engine.sources.add("imports.hir", source.into()).unwrap();
        let checked =
            hirsel_engine::check_progress(&mut engine, file, &hirsel_engine::default_context())
                .unwrap();
        assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
        let input = engine.export_native_input().unwrap();
        let validated = engine.import_native_input(input).unwrap();
        super::super::adapt::program(&validated)
    }

    #[test]
    fn native_import_envelope_11_roundtrips_effects_across_shifted_type_ids() {
        hirsel_engine::with_compiler_stack(|| {
            #[derive(Default)]
            struct Capture(Vec<CachedImport>);
            impl ImportCache for Capture {
                fn get(
                    &mut self,
                    _: &Program,
                    _: &Instances,
                    _: InstanceId,
                ) -> Option<CachedImport> {
                    None
                }
                fn put(&mut self, _: InstanceId, value: Result<CachedImport, String>) {
                    self.0.push(value.unwrap());
                }
            }
            let donor = program(
                "type P { n:u64 }\nfn keep(x:List[P])->List[P] in x { x }\nfn main()->u64 { let xs=keep([P(n:3u64)]); xs[0u64].n }",
            );
            let target = TargetSpec::x86_64_unknown_linux_gnu();
            let mut capture = Capture::default();
            hirsel_rc::import::lower_cached(
                &donor,
                &target,
                &Default::default(),
                &Default::default(),
                &mut capture,
            )
            .unwrap();
            let consumer = TypeInterner::new(None);
            for size in 0..128 {
                consumer.intern(hirsel_types::Type::with_kind(TypeKind::Tuple(
                    vec![consumer.intern(hirsel_types::Type::boolean()); size].into(),
                )));
            }
            let mut shifted = false;
            assert!(!capture.0.is_empty());
            for import in capture.0 {
                let bytes = encode(&import, &donor.types).unwrap();
                let decoded = decode(&bytes, &consumer).unwrap();
                assert_eq!(encode(&decoded, &consumer).unwrap(), bytes);
                assert_eq!(decoded.unit, import.unit);
                assert_eq!(decoded.layouts, import.layouts);
                assert_eq!(decoded.abis, import.abis);
                let (_, before) =
                    hirsel_types::with_table_collection(|| serde_json::to_vec(&import.effects));
                let (_, after) =
                    hirsel_types::with_table_collection(|| serde_json::to_vec(&decoded.effects));
                shifted |= before != after && !before.is_empty();
                assert_eq!(
                    canonical(&consumer, || serde_json::to_vec(&decoded.effects)),
                    canonical(&donor.types, || serde_json::to_vec(&import.effects))
                );
                let legacy = serde_json::to_vec(&(
                    hirsel_codec::to_text(&import.unit),
                    &import.layouts.iter().map(|k| k.0).collect::<Vec<_>>(),
                    &import.abis,
                    canonical(&donor.types, || serde_json::to_vec(&import.effects)).unwrap(),
                ))
                .unwrap();
                assert!(decode(&legacy, &consumer).is_none());
            }
            assert!(shifted, "the roundtrip must actually remap effect TypeIds");
        });
    }

    #[test]
    fn native_import_envelope_11_corruption_misses_then_overwrites_and_reopens() {
        hirsel_engine::with_compiler_stack(|| {
            #[derive(Default)]
            struct Store(std::sync::Mutex<BTreeMap<UnitKey, Arc<[u8]>>>);
            impl ObjectStore for Store {
                fn get(&self, key: &UnitKey) -> Option<Arc<[u8]>> {
                    self.0.lock().unwrap().get(key).cloned()
                }
                fn put(&self, key: &UnitKey, bytes: &[u8]) {
                    self.0.lock().unwrap().insert(*key, bytes.into());
                }
            }
            let source = "type P { n:u64 }\nfn helper(x:P)->u64 { x.n + 1u64 }\nfn main()->u64 { helper(P(n:3u64)) }";
            let target = TargetSpec::x86_64_unknown_linux_gnu();
            let store = Store::default();
            let run = |program: &Program| {
                let mut cache = Imports::new(&store, &target, program.types.clone(), false);
                let cached = hirsel_rc::import::lower_cached(
                    program,
                    &target,
                    &Default::default(),
                    &Default::default(),
                    &mut cache,
                )
                .unwrap();
                let fresh = hirsel_rc::import::lower(program, &target).unwrap();
                assert_eq!(cached.units, fresh.units);
                assert_eq!(
                    hirsel_codec::to_text(&cached.table),
                    hirsel_codec::to_text(&fresh.table)
                );
                (cache.hits, cache.misses)
            };
            assert!(run(&program(source)).1 > 0);
            let original = store.0.lock().unwrap().clone();
            assert!(!original.is_empty());
            for kind in 0..6 {
                let mut damaged = original.clone();
                for bytes in damaged.values_mut() {
                    let mut corrupt = bytes.to_vec();
                    match kind {
                        0 => corrupt = br#"["legacy",[],[],[]]"#.to_vec(),
                        1 => corrupt[6] += 1,
                        2 => {
                            corrupt.pop();
                        }
                        3 => corrupt.push(0),
                        4 => corrupt[8] = 3,
                        _ => {
                            let [unit, layouts, abis, table, effects] =
                                envelope::decode(bytes).unwrap();
                            corrupt =
                                envelope::encode([effects, layouts, abis, table, unit]).unwrap();
                        }
                    }
                    assert!(
                        decode(&corrupt, &TypeInterner::new(None)).is_none(),
                        "damage {kind}"
                    );
                    *bytes = corrupt.into();
                }
                *store.0.lock().unwrap() = damaged;
                assert_eq!(run(&program(source)), (0, original.len()));
                // A fresh cache handle sees only the complete replacements, as in
                // Go cache_test.go:22-61's overwrite/reopen vectors.
                assert_eq!(run(&program(source)), (original.len(), 0));
            }
        });
    }

    #[test]
    fn native_effects_frame_15_raw_sections_and_corrupt_json_miss() {
        hirsel_engine::with_compiler_stack(|| {
            #[derive(Default)]
            struct Store(std::sync::Mutex<BTreeMap<UnitKey, Arc<[u8]>>>);
            impl ObjectStore for Store {
                fn get(&self, key: &UnitKey) -> Option<Arc<[u8]>> {
                    self.0.lock().unwrap().get(key).cloned()
                }
                fn put(&self, key: &UnitKey, bytes: &[u8]) {
                    self.0.lock().unwrap().insert(*key, bytes.into());
                }
            }
            let source = "type P { n:u64 }\nfn keep(x:List[P])->List[P] in x { x }\nfn main()->u64 { let xs=keep([P(n:3u64)]); xs[0u64].n }";
            let target = TargetSpec::x86_64_unknown_linux_gnu();
            let store = Store::default();
            let run = || {
                let program = program(source);
                let mut cache = Imports::new(&store, &target, program.types.clone(), false);
                let cached = hirsel_rc::import::lower_cached(
                    &program,
                    &target,
                    &Default::default(),
                    &Default::default(),
                    &mut cache,
                )
                .unwrap();
                let fresh = hirsel_rc::import::lower(&program, &target).unwrap();
                assert_eq!(cached.units, fresh.units);
                assert_eq!(
                    hirsel_codec::to_text(&cached.table),
                    hirsel_codec::to_text(&fresh.table)
                );
                (cache.hits, cache.misses)
            };
            assert!(run().1 > 0);
            let original = store.0.lock().unwrap().clone();
            let donor = program(source);
            for bytes in original.values() {
                let [_, _, _, table, effects] = envelope::decode(bytes).unwrap();
                let decoded = decode(bytes, &donor.types).unwrap();
                let old = canonical(&donor.types, || serde_json::to_vec(&decoded.effects)).unwrap();
                let (old_table, old_payload): (hirsel_types::TypeTable, Vec<u8>) =
                    serde_json::from_slice(&old).unwrap();
                assert_eq!(table, serde_json::to_vec(&old_table).unwrap());
                assert_eq!(effects, old_payload);
            }
            // Invalid JSON, wrong shapes and complete-but-extra JSON in either
            // section are semantic misses even when framing is valid. Reopen
            // after rebuilding to prove the miss replaces the damaged entry.
            for section in [3, 4] {
                for malformed in [&b"{"[..], &b"null"[..], &b"[] []"[..]] {
                    let mut damaged = original.clone();
                    for bytes in damaged.values_mut() {
                        let mut sections = envelope::decode(bytes).unwrap();
                        sections[section] = malformed;
                        let corrupt = envelope::encode(sections).unwrap();
                        assert!(decode(&corrupt, &TypeInterner::new(None)).is_none());
                        *bytes = corrupt.into();
                    }
                    *store.0.lock().unwrap() = damaged;
                    assert_eq!(run(), (0, original.len()));
                    assert_eq!(run(), (original.len(), 0));
                }
            }
        });
    }

    #[test]
    fn http1_seed_imports_replay_complete_layouts() {
        hirsel_engine::with_compiler_stack(|| {
            use hirsel_engine::DefinitionQueries;
            let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../stdlib/std@1/hirsel.json");
            let mut package = crate::COMPILER
                .load_package_declarations(&manifest)
                .unwrap();
            let mut roots = Vec::new();
            for (file, context) in package.files.clone() {
                if context.modules != ["internal", "http1_parse"] {
                    continue;
                }
                let syntax = package.engine.parse_file(file, &context).unwrap().value;
                for item in &syntax.items {
                    if let hirsel_syntax::ItemKind::Function {
                        signature,
                        body: Some(_),
                        ..
                    } = &item.item.kind
                        && signature.generics.is_empty()
                    {
                        roots.push(item.item.definition.clone());
                    }
                }
            }
            roots.sort();
            roots.dedup();
            assert_eq!(roots.len(), 13);
            let donor_root = DefPath(hirsel_syntax::ItemPath {
                package: package.package.clone(),
                modules: vec!["http".into()],
                name: "http_server_reject".into(),
            });
            let donor_input = package.engine.native_input_for(&[donor_root]).unwrap();
            let donor_program = super::super::adapt::program(&donor_input);
            drop(donor_input);
            let input = package.engine.native_input_for(&roots).unwrap();
            let program = super::super::adapt::program(&input);
            drop(input);
            drop(package);
            let target = TargetSpec::x86_64_unknown_linux_gnu();
            let store = hirsel_lir::objects::SimObjects::default();
            let mut cache = Imports::new(&store, &target, program.types.clone(), false);
            let donor = hirsel_rc::import::lower_cached(
                &donor_program,
                &target,
                &Default::default(),
                &Default::default(),
                &mut cache,
            )
            .unwrap();
            drop(donor);
            drop(donor_program);
            let cold = hirsel_rc::import::lower(&program, &target).unwrap();
            let expected = (
                hirsel_codec::to_text(&cold.table),
                cold.units
                    .iter()
                    .map(hirsel_codec::to_text)
                    .collect::<Vec<_>>(),
            );
            drop(cold);
            let mut cache = Imports::new(&store, &target, program.types.clone(), false);
            let replay = hirsel_rc::import::lower_cached(
                &program,
                &target,
                &Default::default(),
                &Default::default(),
                &mut cache,
            )
            .unwrap();
            assert!(cache.hits > 0);
            assert_eq!(hirsel_codec::to_text(&replay.table), expected.0);
            assert_eq!(
                replay
                    .units
                    .iter()
                    .map(hirsel_codec::to_text)
                    .collect::<Vec<_>>(),
                expected.1
            );
        });
    }

    #[test]
    fn imported_closure_symbols_match_fresh_after_layout_numbering_changes() {
        hirsel_engine::with_compiler_stack(|| {
            let target = TargetSpec::x86_64_unknown_linux_gnu();
            let store = hirsel_lir::objects::SimObjects::default();
            let source = "type Captured { n:u64 }\nfn compute(x:Captured)->u64 { let f=fn()->u64 { x.n }; f() }\nfn main()->u64 { compute(Captured(n:3u64)) }\n";
            let donor = program(&format!("fn aaa()->(bool,u32) {{ (true,0u32) }}\n{source}"));
            let mut cache = Imports::new(&store, &target, donor.types.clone(), false);
            hirsel_rc::import::lower_cached(
                &donor,
                &target,
                &Default::default(),
                &Default::default(),
                &mut cache,
            )
            .unwrap();
            let client = program(source);
            let fresh = hirsel_rc::import::lower(&client, &target).unwrap();
            let mut cache = Imports::new(&store, &target, client.types.clone(), false);
            let cached = hirsel_rc::import::lower_cached(
                &client,
                &target,
                &Default::default(),
                &Default::default(),
                &mut cache,
            )
            .unwrap();
            assert!(
                cache.hits > 0,
                "the distinct consumer must exercise stored imports"
            );
            let symbols = |lowered: &hirsel_rc::import::Lowered| {
                lowered
                    .units
                    .iter()
                    .map(|u| (u.header.definition.clone(), u.header.instance))
                    .collect::<std::collections::BTreeSet<_>>()
            };
            assert_eq!(
                symbols(&cached),
                symbols(&fresh),
                "closure code identity must not contain another program's layout numbering"
            );
            assert_eq!(
                hirsel_codec::to_text(&cached.table),
                hirsel_codec::to_text(&fresh.table)
            );
            assert_eq!(cached.units, fresh.units);
        });
    }

    #[test]
    fn imports_replay_across_sessions_unrelated_layouts_and_body_edits() {
        hirsel_engine::with_compiler_stack(|| {
            let target = TargetSpec::x86_64_unknown_linux_gnu();
            let store = hirsel_lir::objects::SimObjects::default();
            let source =
                "fn helper(x: u64) -> u64 { x + 1u64 }\nfn main() -> u64 { helper(3u64) }\n";
            let run = |source: &str| {
                let program = program(source);
                let mut cache = Imports::new(&store, &target, program.types.clone(), false);
                let lowered = hirsel_rc::import::lower_cached(
                    &program,
                    &target,
                    &hirsel_rc::flat_walk::Options::default(),
                    &hirsel_lir::discharge::Discharge::default(),
                    &mut cache,
                )
                .unwrap();
                let cold = hirsel_rc::import::lower(&program, &target).unwrap();
                assert_eq!(
                    hirsel_codec::to_text(&lowered.table),
                    hirsel_codec::to_text(&cold.table)
                );
                assert_eq!(
                    lowered
                        .units
                        .iter()
                        .map(hirsel_codec::to_text)
                        .collect::<Vec<_>>(),
                    cold.units
                        .iter()
                        .map(hirsel_codec::to_text)
                        .collect::<Vec<_>>()
                );
                (cache.hits, cache.misses, lowered.units.len())
            };
            let (hits, misses, count) = run(source);
            assert_eq!(hits, 0);
            assert_eq!(misses, count);
            assert!(store.len() >= count);
            assert_eq!(run(source), (count, 0, count));
            let unrelated = format!(
                "type Other {{ text: String }}\nfn unrelated() -> Other {{ Other(text: \"x\") }}\n{source}"
            );
            let original = program(source);
            let shifted = program(&unrelated);
            let original_lowered = hirsel_rc::import::lower(&original, &target).unwrap();
            let shifted_lowered = hirsel_rc::import::lower(&shifted, &target).unwrap();
            for id in original_lowered.instances.ids() {
                let path = original_lowered.instances.view(id).key.definition();
                let other = shifted_lowered
                    .instances
                    .ids()
                    .find(|i| shifted_lowered.instances.view(*i).key.definition() == path)
                    .unwrap();
                let a = inputs(&original, &original_lowered.instances, id, &target).unwrap();
                let b = inputs(&shifted, &shifted_lowered.instances, other, &target).unwrap();
                assert_eq!(a.len(), b.len(), "input count for {path}");
                for (i, (a, b)) in a.iter().zip(&b).enumerate() {
                    assert_eq!(
                        a,
                        b,
                        "import input {i} for {path}: {} vs {}",
                        String::from_utf8_lossy(a),
                        String::from_utf8_lossy(b)
                    );
                }
            }
            let (hits, misses, total) = run(&unrelated);
            assert!(
                hits >= count,
                "unrelated layout lost shared imports: {hits}/{count}"
            );
            assert_eq!(hits + misses, total);
            let lists = "fn main() -> u64 { let mut xs: List[u64] = []; list.push(mut xs, 1u64); list.len(xs) }";
            let (_, misses, list_count) = run(lists);
            assert!(misses > 0);
            assert_eq!(run(lists), (list_count, 0, list_count));
            let edited = unrelated.replace("x + 1u64", "x + 2u64");
            let (hits, misses, total) = run(&edited);
            assert_eq!(misses, 1, "only the helper's body changed");
            assert_eq!(hits + misses, total);
            let program = program(source);
            let mut harness = Imports::new(&store, &target, program.types.clone(), true);
            let lowered = hirsel_rc::import::lower_cached(
                &program,
                &target,
                &hirsel_rc::flat_walk::Options::default(),
                &hirsel_lir::discharge::Discharge::default(),
                &mut harness,
            )
            .unwrap();
            assert!(!harness.enabled());
            assert_eq!(harness.hits, 0);
            assert_eq!(lowered.units.len(), count);
        });
    }
}

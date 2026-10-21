//! The pre-import half of the native cache. Keys describe the import's TIR,
//! instance, planned ABI and declaration/representation closure. Stored Value
//! LIR has content-named bindings; its recorded builder effects restore those
//! bindings before the whole-program Value passes run.
//!
//! Follows Go's complete action-key contract (cmd/go/internal/cache/cache.go:28)
//! and Cranelift's parameter-free cached stencil (incremental_cache.rs:36-95).
use hirsel_lir::layout_types::TargetSpec;
use hirsel_lir::objects::{ObjectStore, UnitKey};
use hirsel_rc::import::{CachedImport, ImportCache};
use hirsel_rc::instance::{InstanceId, InstanceRole, Instances, Program};
use hirsel_syntax::{ContentHash, DefPath};
use hirsel_types::{TypeInterner, TypeKind};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

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
    let mut anchors = BTreeMap::new();
    for body in &def.bodies {
        let spans = body
            .locals
            .iter()
            .map(|v| v.span)
            .chain(body.scopes.iter().map(|v| v.span))
            .chain(body.blocks.iter().flat_map(|b| {
                std::iter::once(b.span).chain(b.instructions.iter().map(|i| i.span))
            }));
        for span in spans {
            let next = anchors.len() as u32 + 1;
            anchors.entry(span.anchor).or_insert(next);
        }
    }
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
        definition(program, &view.definition)?,
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
                    tir: (path == *view.key.definition()).then(|| view.definition.clone()),
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

fn encode(import: &CachedImport, types: &TypeInterner) -> Option<Vec<u8>> {
    let effects = canonical(types, || serde_json::to_vec(&import.effects))?;
    let layouts: Vec<ContentHash> = import.layouts.iter().map(|k| k.0).collect();
    serde_json::to_vec(&(
        hirsel_codec::to_text(&import.unit),
        layouts,
        &import.abis,
        effects,
    ))
    .ok()
}

fn decode(bytes: &[u8], types: &TypeInterner) -> Option<CachedImport> {
    let (unit, layouts, abis, effects): (String, Vec<ContentHash>, Vec<ContentHash>, Vec<u8>) =
        serde_json::from_slice(bytes).ok()?;
    let (table, value): (hirsel_types::TypeTable, Vec<u8>) =
        serde_json::from_slice(&effects).ok()?;
    let ids = hirsel_types::decode_table(types, &table);
    let effects =
        hirsel_types::with_table_decoding(&ids, || serde_json::from_slice(&value)).ok()?;
    Some(CachedImport {
        unit: hirsel_codec::from_text(&unit).ok()?,
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

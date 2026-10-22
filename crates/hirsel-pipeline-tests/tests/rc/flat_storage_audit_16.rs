//! Finite MEM-12 physical evidence, independent of the flat encoder. The
//! contract supplies field order, UTF-8 content and explicit-copy independence;
//! typed metadata supplies pointer/scanning and field-offset facts. H supplies
//! checked backing allocations/cells, not native addresses or public ABI bytes.
use crate::rc_support::*;
use hirsel_heapcheck::ledger::{AllocId, Owner};
use hirsel_heapcheck::{Config, FlatStorage, NoOperations, Observed};
use hirsel_lir::layout_types::{CopyRecipe, LayoutId, LayoutKind, LayoutTable, ScalarKind};
use hirsel_lir::stage::Rc;
use hirsel_lir::validate::Inputs;
use hirsel_rc::instance::InstanceId;
use std::collections::BTreeSet;

const TYPES: &str = "flat type Item { text: String, n: u64 }
flat type Batch { items: List[Item] @ growable }
fn sum(xs: Batch) -> u64 {
 let mut total = list.len(xs.items)
 for x in xs.items { total += x.n + string.byte_len(x.text) }
 total
}
";

fn program(source: &str, regions: bool) -> hirsel_heapcheck::Program<Rc> {
    let input = compile(source);
    let lowered = hirsel_rc::import::lower(&input, &target()).unwrap();
    assert!(violations(&input, &lowered).is_empty());
    let units = lowered
        .units
        .iter()
        .enumerate()
        .map(|(i, unit)| {
            let id = InstanceId(i as u32);
            let placed = hirsel_rc::placement::place(
                unit.clone(),
                &lowered.table,
                &hirsel_rc::placement::Options::for_instance(regions, &lowered.instances, id),
            )
            .unwrap();
            let rc = hirsel_rc::insert::insert(
                placed,
                &lowered.table,
                &hirsel_rc::insert::Proofs::for_instance(&lowered.instances, id),
            )
            .unwrap();
            let rc = hirsel_rc::elide::elide(rc, &lowered.table).unwrap();
            let problems = hirsel_lir::validate::check(
                &rc,
                &Inputs {
                    memo: Default::default(),
                    discharge: None,
                    operations: &input.operation_registry(),
                    target: &target(),
                    layouts: &lowered.table,
                    manifest: &manifest(&input),
                },
            );
            assert!(problems.is_empty(), "{problems:?}\n{}", dump(&rc));
            rc
        })
        .collect();
    hirsel_heapcheck::Program {
        target: target(),
        layouts: lowered.table,
        units,
        statics: lowered.statics,
    }
}

fn record(table: &LayoutTable, id: LayoutId) -> &hirsel_lir::layout_types::LayoutRecord {
    table.layout(id).unwrap()
}

/// Recursively reject every handle-bearing kind as well as nonempty scanning,
/// drop/resource metadata. The Block wrapper is deliberately *not* traversed:
/// it is a handle to the payload, not a field inside the flat value.
fn pointer_free(table: &LayoutTable, id: LayoutId) {
    let r = record(table, id);
    assert!(r.scan.is_empty() && r.drop.is_empty() && r.finalizer.is_none());
    assert!(r.abi.scalars.iter().all(|(_, k)| *k != ScalarKind::Pointer));
    match r.kind {
        LayoutKind::Flat => {
            assert_eq!(r.align, 1);
            assert_eq!(r.copy, CopyRecipe::FlatBytes);
            for f in &r.fields {
                pointer_free(table, f.layout);
            }
        }
        LayoutKind::Scalar(ScalarKind::U8 | ScalarKind::U64) => {}
        _ => panic!("pointer-bearing or unsupported payload leaf: {:?}", r.kind),
    }
}

fn item_layout(table: &LayoutTable) -> LayoutId {
    let matches: Vec<_> = table
        .layouts
        .iter()
        .enumerate()
        .filter(|(_, r)| {
            r.kind == LayoutKind::Flat
                && r.fields.len() == 2
                && record(table, r.fields[1].layout).kind == LayoutKind::Scalar(ScalarKind::U64)
        })
        .map(|(i, _)| LayoutId(i as u32))
        .collect();
    assert_eq!(matches.len(), 1);
    matches[0]
}

fn batch_layout(table: &LayoutTable, item: LayoutId) -> LayoutId {
    let matches: Vec<_> = table
        .layouts
        .iter()
        .enumerate()
        .filter(|(_, r)| {
            if r.kind != LayoutKind::Flat || r.fields.len() != 1 || r.fields[0].offset != 0 {
                return false;
            }
            let list = record(table, r.fields[0].layout);
            if list.fields.len() != 1 || list.fields[0].offset != 16 {
                return false;
            }
            let element = record(table, list.fields[0].layout);
            element.fields.len() == 1
                && element.fields[0].offset == 8
                && element.fields[0].layout == item
        })
        .map(|(i, _)| LayoutId(i as u32))
        .collect();
    assert_eq!(matches.len(), 1);
    matches[0]
}

fn word(bytes: &[u8], at: usize) -> u64 {
    u64::from_le_bytes(
        bytes
            .get(at..at + 8)
            .expect("length/fixed field outside payload")
            .try_into()
            .unwrap(),
    )
}

fn size(table: &LayoutTable, layout: LayoutId, bytes: &[u8]) {
    assert_eq!(
        hirsel_lir::layout::evaluate_size(&record(table, layout).size, bytes).unwrap(),
        bytes.len() as u64
    );
}

/// Walk this bounded family using declaration order and typed offsets. Every
/// prefix must meet the next field/element, and the final cursor the allocation
/// extent. Expected text/n values come from handwritten test vectors.
fn item(table: &LayoutTable, layout: LayoutId, bytes: &[u8]) -> (String, u64) {
    pointer_free(table, layout);
    size(table, layout, bytes);
    let r = record(table, layout);
    assert_eq!((r.fields[0].ordinal, r.fields[0].offset), (0, 0));
    assert_eq!((r.fields[1].ordinal, r.fields[1].offset), (1, 8));
    let text = record(table, r.fields[0].layout);
    assert_eq!(text.fields.len(), 1);
    assert_eq!(text.fields[0].offset, 8);
    assert_eq!(
        record(table, text.fields[0].layout).kind,
        LayoutKind::Scalar(ScalarKind::U8)
    );
    let length = usize::try_from(word(bytes, 0)).unwrap();
    let end = 8usize.checked_add(length).unwrap();
    size(table, r.fields[0].layout, bytes.get(..end).unwrap());
    assert_eq!(end + 8, bytes.len());
    (
        std::str::from_utf8(&bytes[8..end]).unwrap().into(),
        word(bytes, end),
    )
}

fn batch(
    table: &LayoutTable,
    layout: LayoutId,
    item_id: LayoutId,
    bytes: &[u8],
) -> Vec<(String, u64)> {
    pointer_free(table, layout);
    size(table, layout, bytes);
    let list_id = record(table, layout).fields[0].layout;
    size(table, list_id, bytes);
    assert_eq!(word(bytes, 0) + 8, bytes.len() as u64);
    let count = word(bytes, 8);
    let mut at = 16;
    let mut items = Vec::new();
    let element = record(table, list_id).fields[0].layout;
    for _ in 0..count {
        let length = usize::try_from(word(bytes, at)).unwrap();
        let end = at.checked_add(8).unwrap().checked_add(length).unwrap();
        size(table, element, bytes.get(at..end).unwrap());
        items.push(item(table, item_id, bytes.get(at + 8..end).unwrap()));
        at = end;
    }
    assert_eq!(at, bytes.len());
    items
}

fn payload(s: &FlatStorage) {
    assert!(s.initialized);
    assert!(
        s.cell_offsets.is_empty(),
        "typed pointer cells in flat payload: {s:?}"
    );
    assert_eq!(s.resources, 0);
    assert!(s.capacity >= s.bytes.len() as u64);
}

fn clean(run: &hirsel_heapcheck::Run, expected: u128) {
    let audit = run.result.as_ref().unwrap();
    assert!(audit.observation.failures.is_empty());
    assert_eq!(
        audit.observation.returned,
        Some(Observed::Integer {
            scalar: ScalarKind::U64,
            bits: expected
        })
    );
    assert_eq!(run.counters.live_heap_bytes, 0);
    assert_eq!(run.counters.allocations, run.counters.frees);
    assert!(
        audit
            .exemptions
            .iter()
            .all(|e| matches!(e, hirsel_heapcheck::Exemption::Static { .. }))
    );
}

fn distinct(storage: &[&FlatStorage], blocks: &[AllocId]) {
    let ids: BTreeSet<_> = storage.iter().map(|s| s.alloc).collect();
    assert_eq!(ids.len(), storage.len(), "explicit copies alias backing");
    // Frame slots and runtime headers/handles are excluded; no separately
    // retained text/list block remains at this settled checkpoint.
    assert_eq!(ids, blocks.iter().copied().collect());
}

#[test]
fn batch_payloads_are_contiguous_pointer_free_and_copied_to_distinct_backing() {
    // Independent finite UTF-8 vectors: zero, one, three variable elements.
    for texts in [
        vec![],
        vec![""],
        vec!["abc"],
        vec!["é🐑"],
        vec!["", "abc", "é🐑"],
    ] {
        let entries = texts
            .iter()
            .enumerate()
            .map(|(i, t)| format!("Item(text: \"{t}\", n: {}u64)", i + 3))
            .collect::<Vec<_>>()
            .join(", ");
        let source = format!(
            "{TYPES}fn main() -> u64 {{ let xs = Batch(items: [{entries}])\n let ys = copy xs\n let zs = copy xs\n sum(xs) + sum(ys) + sum(zs) + sum(xs) }}"
        );
        let expected: Vec<_> = texts
            .iter()
            .enumerate()
            .map(|(i, t)| (t.to_string(), i as u64 + 3))
            .collect();
        let total = expected
            .iter()
            .map(|(t, n)| n + t.len() as u64 + 1)
            .sum::<u64>();
        for regions in [false, true] {
            let p = program(&source, regions);
            let i = item_layout(&p.layouts);
            let b = batch_layout(&p.layouts, i);
            let mut settled = false;
            let run = hirsel_heapcheck::run_with_flat_storage_observer(
                &p,
                &path("main"),
                &[],
                &mut NoOperations,
                &Config::default(),
                &mut |_, storage, blocks| {
                    let batches: Vec<_> = storage
                        .iter()
                        .filter(|s| s.layout == b && s.initialized)
                        .collect();
                    for s in &batches {
                        payload(s);
                        assert_eq!(batch(&p.layouts, b, i, &s.bytes), expected);
                    }
                    if batches.len() == 3 && blocks.len() == 3 {
                        distinct(&batches, blocks);
                        assert_eq!(batches[0].bytes, batches[1].bytes);
                        assert_eq!(batches[1].bytes, batches[2].bytes);
                        settled = true;
                    }
                },
            );
            clean(&run, u128::from(total * 4));
            assert!(
                settled,
                "no three-copy physical checkpoint, regions={regions}"
            );
        }
    }
}

#[test]
fn two_item_copies_mutate_only_their_own_fixed_field() {
    for text in ["", "abc", "é🐑"] {
        let source = format!(
            "{TYPES}fn main() -> u64 {{ let x = Item(text: \"{text}\", n: 7)\n let mut y = copy x\n let mut z = copy x\n y.n = 11\n z.n = 19\n x.n + y.n + z.n + string.byte_len(x.text) + string.byte_len(y.text) + string.byte_len(z.text) }}"
        );
        for regions in [false, true] {
            let p = program(&source, regions);
            let i = item_layout(&p.layouts);
            let mut mutated = false;
            let mut placements = BTreeSet::new();
            let run = hirsel_heapcheck::run_with_flat_storage_observer(
                &p,
                &path("main"),
                &[],
                &mut NoOperations,
                &Config::default(),
                &mut |_, storage, blocks| {
                    let items: Vec<_> = storage
                        .iter()
                        .filter(|s| s.layout == i && s.initialized)
                        .collect();
                    let mut values = Vec::new();
                    for s in &items {
                        payload(s);
                        let (seen, n) = item(&p.layouts, i, &s.bytes);
                        assert_eq!(seen, text);
                        values.push(n);
                        placements.insert(matches!(s.owner, Owner::Region(_)));
                    }
                    values.sort_unstable();
                    if values == [7, 11, 19] && blocks.len() == 3 {
                        distinct(&items, blocks);
                        mutated = true;
                    }
                },
            );
            clean(&run, 37 + 3 * text.len() as u128);
            assert!(mutated, "no simultaneous independently mutated copies");
            // The immutable original can use a region; the two mutable
            // copies retain their independently allocated heap backing.
            assert_eq!(placements.contains(&true), regions);
            assert!(placements.contains(&false));
        }
    }
}

#[test]
fn copies_survive_either_lifetime_order_and_every_failure_site() {
    for (body, expected) in [
        (
            "let y = { let x = Item(text: \"é🐑\", n: 7)\n copy x }\n y.n + string.byte_len(y.text)",
            13,
        ),
        (
            "let x = Item(text: \"é🐑\", n: 7)\n let seen = { let mut y = copy x\n y.n = 19\n y.n }\n x.n + string.byte_len(x.text) + seen - 19",
            13,
        ),
        (
            "let ys = { let xs = Batch(items: [Item(text: \"é🐑\", n: 7)])\n copy xs }\n sum(ys)",
            14,
        ),
        (
            "let xs = Batch(items: [Item(text: \"é🐑\", n: 7)])\n let seen = { let ys = copy xs\n sum(ys) }\n sum(xs) + seen",
            28,
        ),
    ] {
        let source = format!("{TYPES}fn main() -> u64 {{ {body} }}");
        for regions in [false, true] {
            let p = program(&source, regions);
            let campaign = hirsel_heapcheck::fault_campaign(
                &p,
                &path("main"),
                &[],
                &mut || Box::new(NoOperations),
                &Config::default(),
            );
            assert!(
                campaign.first_violation().is_none(),
                "{:?}",
                campaign.first_violation()
            );
            clean(&campaign.clean, expected);
            assert!(campaign.clean.sites > 0);
            assert_eq!(campaign.injected.len() as u64, campaign.clean.sites);
            eprintln!(
                "lifetime regions={regions}: clean=1 injected={} sites={} allocations={} frees={}",
                campaign.injected.len(),
                campaign.clean.sites,
                campaign.clean.counters.allocations,
                campaign.clean.counters.frees
            );
            for (_, run) in &campaign.injected {
                assert!(
                    run.result
                        .as_ref()
                        .unwrap()
                        .observation
                        .failures
                        .iter()
                        .any(|f| f.rule == hirsel_heapcheck::INJECTED_RULE)
                );
                assert_eq!(run.counters.live_heap_bytes, 0);
            }
        }
    }
}

#[test]
fn a_reserved_builder_crosses_its_capacity_and_copies_without_retained_parts() {
    let source = format!(
        "{TYPES}fn main() -> u64 {{ let mut items: List[Item] = []\n list.reserve(mut items, 1)\n for n in 0u64..3u64 {{ list.push(mut items, Item(text: \"é🐑\", n: n)) }}\n let xs = Batch(items: take items)\n let ys = copy xs\n sum(xs) + sum(ys) + sum(xs) }}"
    );
    for regions in [false, true] {
        let p = program(&source, regions);
        let i = item_layout(&p.layouts);
        let b = batch_layout(&p.layouts, i);
        let mut settled = false;
        let run = hirsel_heapcheck::run_with_flat_storage_observer(
            &p,
            &path("main"),
            &[],
            &mut NoOperations,
            &Config::default(),
            &mut |_, storage, blocks| {
                let batches: Vec<_> = storage
                    .iter()
                    .filter(|s| s.layout == b && s.initialized)
                    .collect();
                if batches.len() == 2 && blocks.len() == 2 {
                    distinct(&batches, blocks);
                    for s in batches {
                        payload(s);
                        assert_eq!(
                            batch(&p.layouts, b, i, &s.bytes),
                            vec![("é🐑".into(), 0), ("é🐑".into(), 1), ("é🐑".into(), 2)]
                        );
                    }
                    settled = true;
                }
            },
        );
        clean(&run, 72);
        assert!(settled);
        let campaign = hirsel_heapcheck::fault_campaign(
            &p,
            &path("main"),
            &[],
            &mut || Box::new(NoOperations),
            &Config::default(),
        );
        assert!(
            campaign.first_violation().is_none(),
            "{:?}",
            campaign.first_violation()
        );
        assert_eq!(campaign.injected.len() as u64, run.sites);
        eprintln!(
            "growth regions={regions}: clean=1 injected={} sites={} allocations={} frees={}",
            campaign.injected.len(),
            run.sites,
            run.counters.allocations,
            run.counters.frees
        );
        assert!(
            run.sites >= 3,
            "construction, growth and copy sites must execute"
        );
        for (_, failed) in campaign.injected {
            assert!(
                failed
                    .result
                    .unwrap()
                    .observation
                    .failures
                    .iter()
                    .any(|f| f.rule == hirsel_heapcheck::INJECTED_RULE)
            );
            assert_eq!(failed.counters.live_heap_bytes, 0);
        }
    }
}

#[test]
fn physical_oracle_rejects_bad_metadata_extents_cells_aliases_and_retained_blocks() {
    let p = program(
        &format!(
            "{TYPES}fn main() -> u64 {{ let x = Item(text: \"é🐑\", n: 7)\n x.n + string.byte_len(x.text) }}"
        ),
        false,
    );
    let i = item_layout(&p.layouts);
    let mut saved = None;
    let run = hirsel_heapcheck::run_with_flat_storage_observer(
        &p,
        &path("main"),
        &[],
        &mut NoOperations,
        &Config::default(),
        &mut |_, storage, _| {
            if let Some(s) = storage.iter().find(|s| s.layout == i && s.initialized) {
                saved = Some(s.clone());
            }
        },
    );
    clean(&run, 13);
    assert_eq!(
        run,
        hirsel_heapcheck::run(
            &p,
            &path("main"),
            &[],
            &mut NoOperations,
            &Config::default()
        )
    );
    let fail = Config {
        inject: Some(0),
        ..Config::default()
    };
    let observed_failure = hirsel_heapcheck::run_with_flat_storage_observer(
        &p,
        &path("main"),
        &[],
        &mut NoOperations,
        &fail,
        &mut |_, _, _| {},
    );
    assert_eq!(
        observed_failure,
        hirsel_heapcheck::run(&p, &path("main"), &[], &mut NoOperations, &fail)
    );
    assert!(
        observed_failure
            .result
            .unwrap()
            .observation
            .failures
            .iter()
            .any(|f| f.rule == hirsel_heapcheck::INJECTED_RULE)
    );
    let s = saved.expect("an initialized Item backing allocation");
    payload(&s);
    assert_eq!(item(&p.layouts, i, &s.bytes), ("é🐑".into(), 7));
    let rejects =
        |f: &dyn Fn()| assert!(std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)).is_err());
    let mut cells = s.clone();
    cells.cell_offsets.push(8);
    rejects(&|| payload(&cells));
    let mut extent = s.bytes.clone();
    extent.pop();
    rejects(&|| {
        item(&p.layouts, i, &extent);
    });
    let mut prefix = s.bytes.clone();
    prefix[..8].copy_from_slice(&u64::MAX.to_le_bytes());
    rejects(&|| {
        item(&p.layouts, i, &prefix);
    });
    let mut layout = p.layouts.clone();
    layout.layouts[i.0 as usize].fields[1].offset += 1;
    rejects(&|| {
        item(&layout, i, &s.bytes);
    });
    let mut pointer = p.layouts.clone();
    let scalar = record(&pointer, i).fields[1].layout;
    pointer.layouts[scalar.0 as usize].kind = LayoutKind::Scalar(ScalarKind::Pointer);
    rejects(&|| pointer_free(&pointer, i));
    let mut scan = p.layouts.clone();
    scan.layouts[i.0 as usize]
        .scan
        .push(hirsel_lir::layout_types::ScanStep::Counted { offset: 0 });
    rejects(&|| pointer_free(&scan, i));
    rejects(&|| distinct(&[&s, &s], &[s.alloc]));
    rejects(&|| distinct(&[&s], &[s.alloc, AllocId(u32::MAX)]));
}

#[test]
fn local_region_sources_still_make_two_distinct_flat_copies() {
    // The original is eligible for a region. Flat copies must keep their
    // allocation/failure path, rather than becoming borrowed pointer reads.
    let source = format!(
        "{TYPES}fn main() -> u64 {{ let x = Item(text: \"é🐑\", n: 7)\n let y = copy x\n let z = copy x\n x.n + y.n + z.n + string.byte_len(x.text) + string.byte_len(y.text) + string.byte_len(z.text) }}"
    );
    for regions in [false, true] {
        let p = program(&source, regions);
        let i = item_layout(&p.layouts);
        let mut settled = false;
        let mut saw_region = false;
        let run = hirsel_heapcheck::run_with_flat_storage_observer(
            &p,
            &path("main"),
            &[],
            &mut NoOperations,
            &Config::default(),
            &mut |_, storage, blocks| {
                let items: Vec<_> = storage
                    .iter()
                    .filter(|s| s.layout == i && s.initialized)
                    .collect();
                for s in &items {
                    payload(s);
                    assert_eq!(item(&p.layouts, i, &s.bytes), ("é🐑".into(), 7));
                    saw_region |= matches!(s.owner, Owner::Region(_));
                }
                if items.len() == 3 && blocks.len() == 3 {
                    distinct(&items, blocks);
                    settled = true;
                }
            },
        );
        clean(&run, 39);
        assert!(
            settled,
            "explicit flat copies lost distinct backing, regions={regions}"
        );
        assert_eq!(
            saw_region, regions,
            "the original must exercise actual region placement"
        );
        let campaign = hirsel_heapcheck::fault_campaign(
            &p,
            &path("main"),
            &[],
            &mut || Box::new(NoOperations),
            &Config::default(),
        );
        assert!(
            campaign.first_violation().is_none(),
            "{:?}",
            campaign.first_violation()
        );
        assert_eq!(campaign.injected.len() as u64, run.sites);
        for (_, failed) in campaign.injected {
            assert!(
                failed
                    .result
                    .unwrap()
                    .observation
                    .failures
                    .iter()
                    .any(|f| f.rule == hirsel_heapcheck::INJECTED_RULE)
            );
            assert_eq!(failed.counters.live_heap_bytes, 0);
        }
    }
}

#[test]
#[ignore = "native: focused flat region copy parity"]
fn native_flat_region_copy_parity() {
    use hirsel_driver::compiler_adapters;
    use hirsel_driver::native::*;
    use hirsel_engine::*;
    let matrix = vec![
        Configuration {
            options: Options::new(target()),
            engine: Engine::Heap { campaign: true },
        },
        Configuration {
            options: Options::new(target()),
            engine: Engine::Native {
                opt: OptLevel::Speed,
                partition: Partition::Whole,
                runtime: Runtime::Diagnostic,
            },
        },
        Configuration {
            options: Options::new(target()),
            engine: Engine::Cranelift {
                partition: Partition::Whole,
                runtime: Runtime::Diagnostic,
            },
        },
    ];
    let directory =
        std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("flat-storage-audit-16-native");
    std::fs::create_dir_all(&directory).unwrap();
    let toolchain = Toolchain::for_matrix(&directory.join("runtime"), &matrix).unwrap();
    for (name, source, succeeds) in [
        (
            "region-item",
            "flat type Item { text: String, n: u64 }\npub fn main() -> Result[(), u64] { let x = Item(text: \"a\", n: 7)\n let y = copy x\n if x.n == 7 && y.n == 7 && string.byte_len(y.text) == 1 { Ok(()) } else { Error(y.n) } }",
            true,
        ),
        (
            "materialized-list",
            "flat type Item { text: String, n: u64 }\nflat type Batch { items: List[Item] @ growable }\npub fn main() -> Result[(), ()] { let original = Batch(items: [Item(text: \"é🐑\", n: 7)])\n let copied = copy original\n let materialized = copy copied.items\n let index = list.len(materialized)\n let missing = materialized[index]\n let _ = missing.n + list.len(original.items)\n Ok(()) }",
            false,
        ),
    ] {
        let directory = directory.join(name);
        std::fs::create_dir_all(&directory).unwrap();
        with_compiler_stack(|| {
            let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
            let context = default_context();
            let file = engine
                .sources
                .add("flat-region-copy.hir", source.into())
                .unwrap();
            let checked = check_progress(&mut engine, file, &context).unwrap();
            assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
            let entry = entry_path(&context);
            let reference = run_observed_snapshot(
                &mut engine,
                &RunConfiguration {
                    entry: entry.clone(),
                    hash_seed: 0,
                    inputs: vec![],
                },
                SimulationConfig::default(),
            )
            .unwrap();
            let reference = Observation::of_reference(reference.observation.as_ref().unwrap());
            if succeeds {
                assert_eq!(reference.exit_code, Some(0));
            } else {
                assert_eq!(reference.failures[0].rule, "MEM-14");
            }
            let snapshot = engine.export_native_input().unwrap();
            let validated = engine.import_native_input(snapshot).unwrap();
            for (configuration, outcome) in differential(
                &validated, &entry, &reference, &matrix, &toolchain, &directory,
            ) {
                assert!(
                    matches!(outcome, Outcome::Agree),
                    "{configuration}: {outcome:?}"
                );
            }
        });
    }
}

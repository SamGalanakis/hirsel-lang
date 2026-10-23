//! Parameter-only construction receipts are lifetime sources, not ABI destinations.
use crate::rc_support::*;
use hirsel_lir::ir::{DestinationClass, Placement, StorageOwner};
use hirsel_rc::placement::{Options as PlaceOptions, PlaceError, place};

const BLOCK: &str = include_str!("../../../../conformance/LANG/LANG-11-block-local-share-run.hir");
const PAIRED: &str =
    include_str!("../../../../conformance/LANG/LANG-15-paired-cursor-recipes-17-run.hir");
const VIEW: &str = "fn view(x: List[u64]) -> List[u64] in x { x }\n";
const COMPARE: &str = r#"
fn transaction(a: List[u64]) -> List[u64] in a {
    let mut work = copy a
    list.push(mut work, 9)
    take work
}
fn main() -> bool { transaction([1]) == [1, 9] }
"#;

#[test]
fn scalar_counted_and_nested_consumers() {
    for (source, destination) in [
        (COMPARE, false),
        (
            "fn main() -> u64 { list.len({ let v: List[u64] = [1]; view(v) }) }",
            false,
        ),
        (
            "fn main() -> List[u64] { let v: List[u64] = [1]; view(v) }",
            true,
        ),
        (
            "fn main() -> u64 { list.len({ { let v: List[u64] = [1]; view(v) } }) }",
            false,
        ),
        (
            "fn main() -> u64 { list.len({ let v: List[u64] = [1]; let f = fn() { v }; f() }) }",
            false,
        ),
        (
            "fn main() -> u64 { list.len(if true { let v: List[u64] = [1]; view(v) } else { [] }) }",
            false,
        ),
    ] {
        let (_, lowered) = lower_valid(&format!("{VIEW}{source}"));
        let main = unit_named(&lowered, "main");
        assert_eq!(
            lowered.table.fn_abis[main.header.abi.0 as usize].destination,
            destination
        );
        for (unit, &origin) in lowered.units.into_iter().zip(&lowered.unit_origins) {
            assert!(
                unit.tables
                    .constructs
                    .iter()
                    .all(|c| !matches!(c.destination, DestinationClass::Caller(_)))
            );
            place(
                unit,
                &lowered.table,
                &PlaceOptions::for_instance(true, &lowered.instances, origin),
            )
            .unwrap();
        }
    }
}

#[test]
fn forged_caller_destination_is_still_an_obligation() {
    for (source, has_destination) in [
        (
            "fn main() -> u64 { let v: List[u64] = [1]; list.len(v) }",
            false,
        ),
        ("fn main() -> List[u64] { [1] }", true),
    ] {
        let (_, lowered) = lower_valid(source);
        let mut main = unit_named(&lowered, "main").clone();
        main.tables.constructs[0].destination = DestinationClass::Caller(vec![]);
        let result = place(
            main,
            &lowered.table,
            &PlaceOptions {
                regions: true,
                noalloc: false,
            },
        );
        if has_destination {
            assert_eq!(
                result.unwrap().tables.constructs[0].placement,
                Some(Placement::CallerDestination)
            );
        } else {
            assert_eq!(
                result.unwrap_err(),
                PlaceError::Unsupported("a caller destination in a unit whose ABI has none".into())
            );
        }
    }
}

const ARENA: &str = r#"
type Scratch { a: u64, b: u64 }
fn view(x: List[u64]) -> List[u64] in x { x }
fn bounded(a: Arena) -> u64 {
    let v: List[u64] in a = [1, 2]
    list.len(if true { view(v) } else { [] })
}
@noalloc
fn scratch() -> u64 { let v: box Scratch = box Scratch(a: 1, b: 2); let held = copy v; 2 }
pub fn main() -> Result[(), ()] {
    let n = arena.with(fn(a: Arena) -> u64 { bounded(a) })
    if n == 2 && scratch() == 2 { Ok(()) } else { Error(()) }
}
"#;

#[test]
fn bounded_arena_and_noalloc_bounds_remain_strict() {
    let (_, lowered) = lower_valid(ARENA);
    for regions in [false, true] {
        for (unit, &origin) in lowered.units.iter().zip(&lowered.unit_origins) {
            let name = unit.header.definition.0.name.as_str();
            let placed = place(
                unit.clone(),
                &lowered.table,
                &PlaceOptions::for_instance(regions, &lowered.instances, origin),
            )
            .unwrap();
            if name == "bounded" {
                assert!(placed.tables.constructs.iter().any(|c| matches!(
                    c.placement,
                    Some(Placement::Fixed(StorageOwner::Arena(_)))
                )));
            }
            if name == "scratch" {
                assert!(placed.tables.constructs.iter().any(|c| matches!(
                    c.placement,
                    Some(Placement::Fixed(StorageOwner::LocalRegion(_)))
                )));
            }
        }
    }
    let error = try_compile("@noalloc\nfn main() -> List[u64] { [1] }").unwrap_err();
    assert!(error.contains("MEM-5"), "{error}");
    let error = try_compile("fn main(a: Arena) -> List[u64] { let v: List[u64] in a = [1]; v }")
        .unwrap_err();
    assert!(error.contains("PROV-4"), "{error}");
    let (_, lowered) = lower_valid("fn main() -> List[u64] { [1] }");
    assert!(matches!(
        place(
            unit_named(&lowered, "main").clone(),
            &lowered.table,
            &PlaceOptions {
                regions: false,
                noalloc: true
            }
        ),
        Err(PlaceError::HeapInNoalloc(_))
    ));
}

// Independent oracles follow rustc_mir_dataflow's all-target cursor tests:
// compare every configuration with the same reference observation, including
// independently constructed equal and unequal roots in PAIRED. No Rust lifetime
// rule is imported; LANG-11, LANG-15 and PROV-4 define the accepted programs.
#[test]
fn originals_and_bounded_twins_execute_on_all_engines() {
    native_parity(&[("block", BLOCK), ("paired", PAIRED)]);
}

#[test]
fn bounded_twins_execute_on_all_engines() {
    native_parity(&[("bounded", ARENA)]);
}

fn native_parity(cases: &[(&str, &str)]) {
    use hirsel_driver::native::*;
    use hirsel_driver::*;
    use hirsel_engine::*;
    let matrix: Vec<_> = [true, false]
        .into_iter()
        .flat_map(|regions| {
            let mut options = Options::new(target());
            options.regions = regions;
            [
                Engine::Heap { campaign: false },
                Engine::Native {
                    opt: OptLevel::None,
                    partition: Partition::Whole,
                    runtime: Runtime::Diagnostic,
                },
                Engine::Cranelift {
                    partition: Partition::Whole,
                    runtime: Runtime::Diagnostic,
                },
            ]
            .into_iter()
            .map(move |engine| Configuration {
                options: options.clone(),
                engine,
            })
        })
        .collect();
    let directory =
        std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("native-caller-placement-18");
    std::fs::create_dir_all(&directory).unwrap();
    let runner = crate::rc_support::history::Runner::with_matrix(&directory, matrix).unwrap();
    for &(name, source) in cases {
        with_compiler_stack(|| {
            let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
            let context = default_context();
            let file = engine.sources.add("caller.hir", source.into()).unwrap();
            let checked = check_progress(&mut engine, file, &context).unwrap();
            assert!(
                checked.diagnostics.is_empty(),
                "{name}: {:?}",
                checked.diagnostics
            );
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
            assert_eq!(reference.exit_code, Some(0), "{name}: {reference:?}");
            assert!(reference.failures.is_empty(), "{name}: {reference:?}");
            let snapshot = engine.export_native_input().unwrap();
            let validated = engine.import_native_input(snapshot).unwrap();
            // Inspect actual placement and lifetime accounting, beyond exit parity.
            for regions in [true, false] {
                let mut options = Options::new(target());
                options.regions = regions;
                let ready = build(&validated, &options).unwrap();
                let heap = run_heap(&ready, &entry, &Sites::new(validated.snapshot())).unwrap();
                assert_eq!(heap.observation, reference);
                assert_eq!(
                    heap.counters.live_heap_bytes, 0,
                    "{name}: {:?}",
                    heap.counters
                );
                if name == "bounded" {
                    assert_eq!(heap.counters.allocations, 0);
                }
                eprintln!("{name} regions={regions}: {:?}", heap.counters);
            }
            let report = runner.compare(&validated, &entry, &reference, name);
            assert_eq!(report.cells.len(), 7); // reference plus the six existing configurations
            assert!(report.agrees(), "{name}: {report:#?}");
            eprintln!("{name}: six configurations agree");
        });
    }
}

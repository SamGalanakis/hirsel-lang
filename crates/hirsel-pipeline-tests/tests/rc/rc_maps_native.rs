//! Compiled laws inspect the private index directly; source has no bucket API.
use hirsel_driver::compiler_adapters;
use hirsel_driver::native::*;
use hirsel_engine::{
    QueryEngine, check_progress, default_context, default_environment, with_compiler_stack,
};
use hirsel_lir::layout::sequence_elements_offset;
use hirsel_lir::layout_types::{
    FieldLayout, LayoutId, LayoutKind, LayoutTable, ScalarKind, SizeSpec,
};

/// The count offset, first-element offset, stride and element layout of the
/// counted sequence a block field references, from the layout table.
fn sequence(table: &LayoutTable, field: &FieldLayout) -> (u64, u64, u64, LayoutId) {
    let LayoutKind::Block { payload } = table.layouts[field.layout.0 as usize].kind else {
        panic!("a sequence block field")
    };
    let record = &table.layouts[payload.0 as usize];
    let LayoutKind::Sequence { element } = record.kind else {
        panic!("a counted sequence payload")
    };
    let element_record = &table.layouts[element.0 as usize];
    let SizeSpec::Fixed(stride) = element_record.size else {
        panic!("a fixed element stride")
    };
    (
        record.fields[0].offset,
        sequence_elements_offset(element_record).unwrap(),
        stride,
        element,
    )
}

#[test]
#[ignore = "native: run by tools/check-native.sh"]
fn compiled_hash_seed_changes_buckets_and_preserves_insertion_order() {
    let inserts = (0..32)
        .map(|i| format!("map.insert(mut m, {i}, {})\n", i + 100))
        .collect::<String>();
    let source = format!(
        "pub fn build() -> Map[u64,u64] {{ let mut m: Map[u64,u64] = map.empty()\n{inserts}take m }}\npub fn first(take m: Map[u64,u64]) -> u64 {{ for (k,v) in take m {{ return v }}\n 0 }}\npub fn main(rt: Io, args: Args) -> Result[(), ()] {{ Ok(()) }}"
    );
    let targets = TargetSpec::executed();
    let root = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("map-seed");
    let tools =
        Toolchain::in_tree_diagnostic(&root.join("runtime"), &targets.iter().collect::<Vec<_>>())
            .unwrap();
    with_compiler_stack(move || {
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
        let file = engine.sources.add("map-seed.hir", source).unwrap();
        let checked = check_progress(&mut engine, file, &default_context()).unwrap();
        assert!(
            !checked
                .diagnostics
                .iter()
                .any(|d| d.severity == hirsel_diag::Severity::Error),
            "{:?}",
            checked.diagnostics
        );
        let input = engine.export_native_input().unwrap();
        let validated = engine.import_native_input(input).unwrap();
        for target in targets {
            for reuse in [false, true] {
                let mut options = Options::new(target.clone());
                options.regions = false;
                options.reuse = reuse;
                let ready = build(&validated, &options).unwrap();
                let abi_of = |name: &str| {
                    let unit = ready
                        .units
                        .iter()
                        .map(|u| u.unit())
                        .find(|u| u.header.definition.0.name == name)
                        .unwrap();
                    &ready.table.fn_abis[unit.header.abi.0 as usize]
                };
                let abi = abi_of("build");
                assert!(
                    abi.context && abi.destination && !abi.environment && abi.parameters.is_empty()
                );
                let first_abi = abi_of("first");
                assert!(
                    first_abi.context
                        && !first_abi.destination
                        && !first_abi.environment
                        && first_abi.parameters.len() == 1
                );
                // The calling convention comes from the emitter's own rule, so a
                // register-result change rewrites the harness's calls with it.
                let calls = [
                    c_call(&ready.table, abi, "@BUILD@", "call_build"),
                    c_call(&ready.table, first_abi, "@FIRST@", "call_first"),
                ]
                .concat();
                let layout = abi.result.as_ref().unwrap().layout;
                let LayoutKind::Block { payload } = ready.table.layouts[layout.0 as usize].kind
                else {
                    panic!("map block")
                };
                let map = &ready.table.layouts[payload.0 as usize];
                let (entries_count, start, stride, pair) = sequence(&ready.table, &map.fields[0]);
                let pair = &ready.table.layouts[pair.0 as usize];
                let (index_count, index_start, index_stride, slot) =
                    sequence(&ready.table, &map.fields[1]);
                assert!(
                    ready.table.layouts[slot.0 as usize].kind
                        == LayoutKind::Scalar(ScalarKind::U32)
                        && index_stride == 4,
                    "the harness reads index slots as uint32_t"
                );
                for opt in [OptLevel::None, OptLevel::Speed] {
                    let object = emit(&ready, opt, |_| true).unwrap();
                    let symbol = object
                        .exports
                        .iter()
                        .find(|e| e.definition.as_ref().is_some_and(|p| p.0.name == "build"))
                        .unwrap()
                        .name
                        .clone();
                    let first = object
                        .exports
                        .iter()
                        .find(|e| e.definition.as_ref().is_some_and(|p| p.0.name == "first"))
                        .unwrap()
                        .name
                        .clone();
                    let dir = root.join(format!("{}-{reuse}-{opt:?}", target.triple));
                    std::fs::create_dir_all(&dir).unwrap();
                    let c = include_str!("../rc/maps/seed.c")
                        .replace("@CALLS@", &calls)
                        .replace("@BUILD@", &symbol)
                        .replace("@FIRST@", &first)
                        .replace(
                            "@MAPSIZE@",
                            &match map.size {
                                SizeSpec::Fixed(n) => n,
                                _ => panic!("map size"),
                            }
                            .to_string(),
                        )
                        .replace("@ENTRIES@", &map.fields[0].offset.to_string())
                        .replace("@INDEX@", &map.fields[1].offset.to_string())
                        .replace("@COUNT@", &entries_count.to_string())
                        .replace("@START@", &start.to_string())
                        .replace("@STRIDE@", &stride.to_string())
                        .replace("@CAPACITY@", &index_count.to_string())
                        .replace("@SLOTS@", &index_start.to_string())
                        .replace("@KEY@", &pair.fields[0].offset.to_string())
                        .replace("@VALUE@", &pair.fields[1].offset.to_string());
                    let harness = dir.join("seed.c");
                    let obj = dir.join("map.o");
                    let exe = dir.join("program");
                    std::fs::write(&harness, c).unwrap();
                    std::fs::write(&obj, object.bytes).unwrap();
                    let arm = target.triple.starts_with("aarch64");
                    let mut compiler = std::process::Command::new(if arm {
                        &tools.aarch64_cc
                    } else {
                        &tools.clang
                    });
                    let linked = compiler
                        .arg(&harness)
                        .arg("-I")
                        .arg(
                            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                                .join("../hirsel-rt/include"),
                        )
                        .arg(&obj)
                        .arg(tools.archive(&target.triple).unwrap())
                        .args([
                            "-lgcc_s",
                            "-lutil",
                            "-lrt",
                            "-lpthread",
                            "-lm",
                            "-ldl",
                            "-o",
                        ])
                        .arg(&exe)
                        .output()
                        .unwrap();
                    assert!(
                        linked.status.success(),
                        "{}",
                        String::from_utf8_lossy(&linked.stderr)
                    );
                    let mut command = if arm {
                        let mut command = std::process::Command::new(&tools.qemu_aarch64);
                        command.arg("-L").arg(&tools.aarch64_sysroot).arg(&exe);
                        command
                    } else {
                        std::process::Command::new(&exe)
                    };
                    let run = command.output().unwrap();
                    assert!(
                        run.status.success(),
                        "{} {reuse} {opt:?}: {}",
                        target.triple,
                        String::from_utf8_lossy(&run.stderr)
                    );
                    assert_eq!(
                        run.stdout,
                        b"seed changes buckets; insertion order stays 0..31\n"
                    );
                }
            }
        }
    });
}

#[test]
#[ignore = "native: run by tools/check-native.sh"]
fn internal_set_order_agrees_on_heap_and_both_native_targets() {
    use hirsel_lir::ir::{CompareOp, Inst};
    use hirsel_lir::validate::{Inputs, validate};
    // Source ordering remains scalar/bytes. This fixture asks C to seal a
    // hand-written Compare Lt of the source equality function's Set layout.
    let source = "fn less(a: Set[u64], b: Set[u64]) -> bool { a == b }\nfn singleton(n: u64) -> Set[u64] { let mut s: Set[u64] = set.empty()\n set.insert(mut s, n)\n take s }\npub fn main(rt: Io, args: Args) -> Result[(), ()] { let a = singleton(256)\n let b = singleton(2)\n let c = singleton(1)\n let mut d: Set[u64] = set.empty()\n set.insert(mut d, 9)\n set.insert(mut d, 1)\n if !less(c,b) || less(a,b) || !less(d,b) || less(b,c) || !less(c,d) { Error(()) } else { Ok(()) } }";
    let targets = TargetSpec::executed();
    let root = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("map-set-order");
    let tools =
        Toolchain::in_tree_diagnostic(&root.join("runtime"), &targets.iter().collect::<Vec<_>>())
            .unwrap();
    with_compiler_stack(move || {
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
        let file = engine.sources.add("set-order.hir", source.into()).unwrap();
        let checked = check_progress(&mut engine, file, &default_context()).unwrap();
        assert!(
            !checked
                .diagnostics
                .iter()
                .any(|d| d.severity == hirsel_diag::Severity::Error),
            "{:?}",
            checked.diagnostics
        );
        let input = engine.export_native_input().unwrap();
        let input = engine.import_native_input(input).unwrap();
        let sites = Sites::new(input.snapshot());
        let entry = hirsel_engine::entry_path(&default_context());
        for target in targets {
            let mut ready = build(&input, &Options::new(target.clone())).unwrap();
            let inputs = Inputs {
                discharge: None,
                operations: &ready.operations,
                target: &ready.target,
                layouts: &ready.table,
                manifest: &ready.manifest,
            };
            let units = std::mem::take(&mut ready.units);
            let mut replacements = 0;
            for u in units {
                let mut u = u.into_raw();
                if u.header.definition.0.name == "less" {
                    for inst in &mut u.instructions {
                        if let Inst::Compare { op, .. } = inst {
                            assert_eq!(*op, CompareOp::Eq);
                            *op = CompareOp::Lt;
                            replacements += 1;
                        }
                    }
                }
                ready
                    .units
                    .push(validate(u, &inputs).unwrap_or_else(|v| panic!("{:?}", v.violations)));
            }
            assert_eq!(replacements, 1);
            let heap = heap_campaign(&ready, &entry, &sites).unwrap();
            assert_eq!(heap.observation.exit_code, Some(0));
            for opt in [OptLevel::None, OptLevel::Speed] {
                for partition in [Partition::Whole, Partition::Singleton] {
                    let dir = root.join(format!("{}-{opt:?}-{}", target.triple, partition.name()));
                    std::fs::create_dir_all(&dir).unwrap();
                    let exe = link(&ready, &entry, opt, partition, &tools, &dir).unwrap();
                    let run = launch(&exe, &target, &tools, &dir, true).unwrap();
                    assert_eq!(
                        run.exit,
                        Some(0),
                        "{}",
                        String::from_utf8_lossy(&run.stderr)
                    );
                }
            }
        }
    });
}

#[test]
#[ignore = "native: run by tools/check-native.sh"]
fn deep_key_failures_name_the_source_operation_through_nested_library_calls() {
    let targets = TargetSpec::executed();
    let root = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("map-failure-sites");
    let tools =
        Toolchain::in_tree_diagnostic(&root.join("runtime"), &targets.iter().collect::<Vec<_>>())
            .unwrap();
    with_compiler_stack(move || {
        for operation in [
            "let found = map.get(m, key)",
            "map.insert(mut m, take key, 1)",
        ] {
            for indexed in [false, true] {
                let inserts = if indexed {
                    (0..9)
                        .map(|i| format!("map.insert(mut m, chain({i}), {i})\n"))
                        .collect::<String>()
                } else {
                    String::new()
                };
                let source = format!(
                    "type Chain = End | Link(next: box Chain)\nfn chain(depth: u64) -> Chain {{ let mut c: Chain = End\n for i in 0..depth {{ c = Link(next: box take c) }}\n take c }}\nfn invoke(mut m: Map[Chain,u64], take key: Chain) {{ {operation} }}\nfn relay(mut m: Map[Chain,u64], take key: Chain) {{ invoke(mut m,take key) }}\npub fn main(rt: Io, args: Args) -> Result[(),()] {{ let mut m: Map[Chain,u64] = map.empty()\n {inserts}let key = chain(545)\n relay(mut m,take key)\n Ok(()) }}"
                );
                let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
                let file = engine
                    .sources
                    .add("map-failure-sites.hir", source.clone())
                    .unwrap();
                let checked = check_progress(&mut engine, file, &default_context()).unwrap();
                assert!(
                    !checked
                        .diagnostics
                        .iter()
                        .any(|d| d.severity == hirsel_diag::Severity::Error),
                    "{source}\n{:?}",
                    checked.diagnostics
                );
                let input = engine.export_native_input().unwrap();
                let input = engine.import_native_input(input).unwrap();
                let sites = Sites::new(input.snapshot());
                let entry = hirsel_engine::entry_path(&default_context());
                for target in &targets {
                    let ready = build(&input, &Options::new(target.clone())).unwrap();
                    let heap = run_heap(&ready, &entry, &sites).unwrap();
                    assert_eq!(heap.observation.failures.len(), 1);
                    let failure = &heap.observation.failures[0];
                    assert_eq!(failure.rule, "MEM-3");
                    assert_eq!(failure.definition.0.name, "invoke");
                    assert_eq!(failure.line, 5);
                    for opt in [OptLevel::None, OptLevel::Speed] {
                        for partition in [Partition::Whole, Partition::Singleton] {
                            let dir = root.join(format!(
                                "{}-{indexed}-{}-{opt:?}-{}",
                                target.triple,
                                operation.starts_with("let"),
                                partition.name()
                            ));
                            std::fs::create_dir_all(&dir).unwrap();
                            let executable =
                                link(&ready, &entry, opt, partition, &tools, &dir).unwrap();
                            let launched = launch(&executable, target, &tools, &dir, true).unwrap();
                            assert_eq!(
                                observe(&launched, &ready, &sites).unwrap(),
                                heap.observation
                            );
                        }
                    }
                }
            }
        }
    });
}

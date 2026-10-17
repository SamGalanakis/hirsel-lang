//! Iterator closures are checked through the production pipeline and failure audits.
use crate::rc_support as support;
use hirsel_driver::compiler_adapters;
use hirsel_engine::{
    QueryEngine, RunConfiguration, check_progress, default_context, default_environment,
    entry_path, run_observed_snapshot, with_compiler_stack,
};
use support::*;
fn observed_heap(source: &str) -> hirsel_driver::native::HeapRun {
    let source = source.to_owned();
    with_compiler_stack(move || {
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
        let file = engine.sources.add("test.hir", source.clone()).unwrap();
        check_progress(&mut engine, file, &default_context()).unwrap();
        let entry = entry_path(&default_context());
        let config = RunConfiguration {
            entry: entry.clone(),
            hash_seed: 0,
            inputs: vec![],
        };
        let run = run_observed_snapshot(&mut engine, &config, Default::default()).unwrap();
        let reference = hirsel_driver::native::Observation::of_reference(
            run.observation
                .as_ref()
                .unwrap_or_else(|| panic!("{source}\n{run:?}")),
        );
        let input = engine.export_native_input().unwrap();
        let validated = engine.import_native_input(input).unwrap();
        let sites = hirsel_driver::native::Sites::new(validated.snapshot());
        let ready = hirsel_driver::native::build(
            &validated,
            &hirsel_driver::native::Options::new(target()),
        )
        .unwrap_or_else(|e| panic!("{source}\n{e}"));
        let heap = hirsel_driver::native::heap_campaign(&ready, &entry, &sites)
            .unwrap_or_else(|e| panic!("{source}\n{e}"));
        assert_eq!(heap.observation, reference, "{source}");
        heap
    })
}

#[test]
fn front_middle_end_and_equal_inserts_keep_order_and_values() {
    let source = r#"pub fn main(rt: Io, args: Args) -> Result[(), GrantError] {
  let by = u64.order
  let mut m: SortedMap[u64,u64] of by = sorted_map.empty(by)
  sorted_map.insert(by, mut m, 5, 50)
  sorted_map.insert(by, mut m, 1, 10)
  sorted_map.insert(by, mut m, 9, 90)
  sorted_map.insert(by, mut m, 3, 30)
  sorted_map.insert(by, mut m, 7, 70)
  sorted_map.insert(by, mut m, 3, 31)
  let mut keys: u64 = 0
  let mut values: u64 = 0
  for (k,v) in m { keys = keys * 10 + k; values += v }
  let _ = out.print(io.stdout(rt)?, if keys == 13579 && values == 251 { "ok\n" } else { "bad\n" })
  Ok(())
}"#;
    assert_eq!(observed_heap(source).observation.output, b"ok\n");
}
#[test]
fn sorted_map_corpus_controls_agree_and_pass_terminal_failure_audits() {
    for source in [
        include_str!("../../../../conformance/DROP/DROP-2-sorted-map-replaced-value.hir"),
        include_str!("../../../../conformance/DROP/DROP-2-sorted-map-retained-key.hir"),
        include_str!("../../../../conformance/FAIL/FAIL-3-sorted-insert-taken-callback.hir"),
        include_str!("../../../../conformance/GEN/GEN-1-sorted-map-copyable-key.hir"),
        include_str!(
            "../../../../conformance/SEM/SEM-5-sorted-map-projected-and-consuming-run.hir"
        ),
        include_str!("../../../../conformance/SEM/SEM-5-sorted-map-resource-modes-run.hir"),
    ] {
        observed_heap(source);
    }
}

fn integer_orders() -> String {
    let mut source = String::from(
        "pub fn main(rt: Io, args: Args) -> Result[(), GrantError] {\nlet mut good: bool = true\n",
    );
    for (kind, low, high) in [
        ("u8", "0", "255"),
        ("u16", "0", "65535"),
        ("u32", "0", "4294967295"),
        ("u64", "0", "18446744073709551615"),
        ("u128", "0", "340282366920938463463374607431768211455"),
        ("i8", "-128", "127"),
        ("i16", "-32768", "32767"),
        ("i32", "-2147483648", "2147483647"),
        ("i64", "-9223372036854775808", "9223372036854775807"),
        (
            "i128",
            "-170141183460469231731687303715884105728",
            "170141183460469231731687303715884105727",
        ),
    ] {
        source.push_str(&format!("let by_{kind} = {kind}.order\ngood = good && by_{kind}.compare({low}{kind},{high}{kind}) == -1i32 && by_{kind}.compare({high}{kind},{low}{kind}) == 1i32 && by_{kind}.compare({low}{kind},{low}{kind}) == 0i32\n"));
    }
    source.push_str(
        "let _ = out.print(io.stdout(rt)?, if good { \"ok\\n\" } else { \"bad\\n\" })\nOk(())\n}",
    );
    source
}
#[test]
fn all_ten_integer_orders_are_callable_at_their_signed_and_unsigned_extremes() {
    assert_eq!(observed_heap(&integer_orders()).observation.output, b"ok\n");
}
fn comparator_failure(fail: bool, key: u64) -> String {
    format!(
        r#"resource type Trace {{ stdout: Stdout, key: u64 }}
drop(x) {{
  let _ = out.print(x.stdout, if x.key == 4 {{ "incoming\n" }} else if x.key == 8 {{ "eight\n" }} else if x.key == 5 {{ "five\n" }} else if x.key == 3 {{ "three\n" }} else if x.key == 2 {{ "two\n" }} else {{ "one\n" }})
}}
fn compare(a: u64, b: u64) -> i32 uses {{}} {{
  if {fail} && a == 4 && b == {key} {{ let z: i32 = 0; let _ = 1 / z }}
  if a < b {{ -7 }} else if a > b {{ 9 }} else {{ 0 }}
}}
pub fn main(rt: Io, args: Args) -> Result[(), GrantError] {{
  let o = io.stdout(rt)?
  let by: Order[u64] = Order(compare: compare)
  let mut m: SortedMap[u64,Trace] of by = sorted_map.empty(by)
  let keys: Array[u64] = [1,2,3,5,8]
  for key in keys {{ sorted_map.insert(by,mut m,key,Trace(stdout:o,key:key)) }}
  sorted_map.insert(by,mut m,4,Trace(stdout:o,key:4))
  Ok(())
}}"#
    )
}
#[test]
fn a_failure_at_each_binary_probe_keeps_the_map_and_releases_the_incoming_value() {
    // Inserting 4 into [1,2,3,5,8] compares against 3, 8, then 5 (#94).
    for key in [3, 8, 5] {
        let failed = observed_heap(&comparator_failure(true, key));
        assert_eq!(
            failed.observation.output,
            b"incoming\neight\nfive\nthree\ntwo\none\n"
        );
        assert_eq!(failed.observation.failures.len(), 1);
        assert_eq!(failed.observation.failures[0].line, 6);
        let accepted = observed_heap(&comparator_failure(false, key));
        assert_eq!(
            accepted.observation.output,
            b"eight\nfive\nincoming\nthree\ntwo\none\n"
        );
    }
}
#[test]
fn consuming_a_shared_sorted_map_preserves_the_other_holder() {
    let source = r#"pub fn main(rt: Io, args: Args) -> Result[(), GrantError] {
let by = u64.order
let mut m: SortedMap[u64,String] of by = sorted_map.empty(by)
sorted_map.insert(by,mut m,3,"three")
sorted_map.insert(by,mut m,1,"one")
sorted_map.insert(by,mut m,2,"two")
let saved = copy m
for (k,v) in take m { let _ = out.print(io.stdout(rt)?,v); break }
for (k,v) in saved { let _ = out.print(io.stdout(rt)?,v) }
Ok(())
}"#;
    assert_eq!(observed_heap(source).observation.output, b"oneonetwothree");
}

#[test]
fn copied_function_references_have_no_holder_and_captured_functions_dup_the_environment() {
    let empty = r#"fn answer() -> u64 { 5 }
pub fn main(rt: Io, args: Args) -> Result[(), GrantError] {
let f = answer
let g = copy f
let _ = out.print(io.stdout(rt)?,if f() + g() == 10 { "ok\n" } else { "bad\n" })
Ok(())
}"#;
    let captured = r#"fn make(take xs: List[u64]) -> fn() -> u64 uses {} { fn() -> u64 { list.len(xs) } }
pub fn main(rt: Io, args: Args) -> Result[(), GrantError] {
let xs: List[u64] = [1,2,3]
let f = make(take xs)
let g = copy f
let _ = out.print(io.stdout(rt)?,if f() + g() == 6 { "ok\n" } else { "bad\n" })
Ok(())
}"#;
    let empty = observed_heap(empty);
    assert_eq!(empty.observation.output, b"ok\n");
    assert_eq!(empty.counters.dups, 0);
    let captured = observed_heap(captured);
    assert_eq!(captured.observation.output, b"ok\n");
    assert!(captured.counters.dups >= 1);
}

#[test]
fn the_hand_built_callback_observer_records_the_exact_binary_probe_pairs() {
    use hirsel_core_contract::{OperationId, Passing};
    use hirsel_heapcheck::{Config, OpOutcome, OpValue, Operations};
    use hirsel_lir::ir::*;
    use hirsel_lir::layout_types::ScalarKind;
    let source = r#"fn compare(a: u64,b: u64) -> i32 uses {} { 0 }
pub fn main() -> Result[(), ()] {
let by: Order[u64] = Order(compare:compare)
let mut m: SortedMap[u64,u64] of by = sorted_map.empty(by)
sorted_map.insert(by,mut m,1,1)
sorted_map.insert(by,mut m,2,2)
sorted_map.insert(by,mut m,3,3)
sorted_map.insert(by,mut m,5,5)
sorted_map.insert(by,mut m,8,8)
sorted_map.insert(by,mut m,4,4)
Ok(())
}"#;
    let program = compile(source);
    let ready = hirsel_driver::native::build_program(
        &program,
        manifest(&program),
        &hirsel_driver::native::Options::new(target()),
    )
    .unwrap();
    let mut units = ready.raw_units();
    let unit = units
        .iter_mut()
        .find(|u| u.header.definition == path("compare"))
        .unwrap();
    // Replace only the user callback by an H operation observer. The map body
    // and its indirect calls are the production Ready LIR, with no source Io.
    let layout = ready.table.fn_abis[unit.header.abi.0 as usize]
        .result
        .as_ref()
        .unwrap()
        .layout;
    let ty = unit.types.iter().position(|t| t.layout == layout).unwrap();
    let result = ValueId(unit.values.len() as u32);
    unit.values.push(ValueInfo {
        ty: LirTypeId(ty as u32),
        holding: Holding::Plain,
    });
    unit.instructions.clear();
    unit.pools.values.clear();
    unit.pools.call_arguments.clear();
    let mut reads = Vec::new();
    for (i, &reaching) in unit.parameters.clone().iter().enumerate() {
        let slot = unit
            .slots
            .iter()
            .position(|s| s.kind == SlotKind::Parameter(i as u32))
            .unwrap();
        let address = AddressId(unit.addresses.len() as u32);
        unit.addresses.push(AddressInfo {
            base: AddressBase::Slot(SlotId(slot as u32)),
            projection: PoolRange::EMPTY,
            ty: unit.values[reaching.index()].ty,
        });
        let result = ValueId(unit.values.len() as u32);
        unit.values.push(ValueInfo {
            ty: unit.values[reaching.index()].ty,
            holding: Holding::Plain,
        });
        unit.instructions.push(Inst::Address {
            address,
            failure: FailureId(0),
        });
        unit.instructions.push(Inst::Read {
            result,
            address,
            reaching,
            mode: ReadMode::Bits,
        });
        reads.push(result);
    }
    let arguments = PoolRange::push(
        &mut unit.pools.call_arguments,
        reads.iter().enumerate().map(|(i, &value)| CallArgument {
            parameter: i as u32,
            passing: Passing::Borrow,
            value: ArgumentValue::Value(value),
        }),
    );
    unit.tables.operations = vec![OperationId {
        definition: hirsel_rc::library::native_path("observe_sorted_pair"),
        declaration_digest: Default::default(),
    }];
    unit.tables.edges = vec![
        Edge {
            target: BlockId(1),
            arguments: PoolRange::EMPTY,
        },
        Edge {
            target: BlockId(2),
            arguments: PoolRange::EMPTY,
        },
    ];
    unit.tables.failures = vec![FailureEdge {
        cleanup: EdgeId(1),
        rule: None,
        anchor: SourceAnchorId(0),
        dispose: PoolRange::EMPTY,
    }];
    unit.tables.calls = vec![CallSite {
        operation_arguments: Vec::new(),
        callee: Callee::Operation(OperationRef(0)),
        abi: unit.header.abi,
        arguments,
        result: Some(result),
        destination: None,
        declared: hirsel_lir::effect::Effects::READS,
        anchor: SourceAnchorId(0),
    }];
    unit.blocks = (0..3)
        .map(|i| Block {
            kind: if i == 2 {
                BlockKind::FailureCleanup
            } else {
                BlockKind::Body
            },
            parameters: PoolRange::EMPTY,
            instructions: PoolRange::EMPTY,
            anchor: SourceAnchorId(0),
        })
        .collect();
    unit.blocks[0].instructions = PoolRange::new(0, unit.instructions.len() as u32);
    unit.terminators = vec![
        Terminator::Call {
            call: CallId(0),
            normal: Some(EdgeId(0)),
            failure: FailureId(0),
        },
        Terminator::Return(Some(result)),
        Terminator::Propagate,
    ];
    unit.entry = BlockId(0);
    struct Observer(Vec<(u64, u64)>);
    impl Operations for Observer {
        fn call(&mut self, _: &OperationId, args: &[OpValue], _: &mut Vec<u8>) -> OpOutcome {
            let [
                OpValue::Bits {
                    scalar: ScalarKind::U64,
                    bits: a,
                },
                OpValue::Bits {
                    scalar: ScalarKind::U64,
                    bits: b,
                },
            ] = args
            else {
                panic!("{args:?}")
            };
            self.0.push((*a as u64, *b as u64));
            let order = match a.cmp(b) {
                std::cmp::Ordering::Less => u32::MAX,
                std::cmp::Ordering::Equal => 0,
                std::cmp::Ordering::Greater => 1,
            };
            OpOutcome::Value(OpValue::Bits {
                scalar: ScalarKind::I32,
                bits: order as u128,
            })
        }
    }
    let heap = hirsel_heapcheck::Program {
        target: target(),
        layouts: ready.table,
        units,
        statics: ready.statics,
    };
    let mut observer = Observer(Vec::new());
    let run = hirsel_heapcheck::run(&heap, &path("main"), &[], &mut observer, &Config::default());
    assert!(run.result.is_ok(), "{:#?}", run.result);
    assert_eq!(
        observer.0,
        [
            (2, 1),
            (3, 2),
            (5, 2),
            (5, 3),
            (8, 3),
            (8, 5),
            (4, 3),
            (4, 8),
            (4, 5)
        ]
    );
}

fn large_map(ascending: bool) -> String {
    format!(
        r#"pub fn main(rt: Io, args: Args) -> Result[(), GrantError] {{
let by = u64.order
let mut m: SortedMap[u64,u64] of by = sorted_map.empty(by)
let mut i: u64 = 0
while i < 10000 {{
  let key: u64 = if {ascending} {{ i }} else {{ 9999 - i }}
  sorted_map.insert(by,mut m,key,key * 3)
  i += 1
}}
let mut expected: u64 = 0
let mut good: bool = true
for (k,v) in m {{ good = good && k == expected && v == k * 3; expected += 1 }}
let _ = out.print(io.stdout(rt)?,if good && expected == 10000 {{ "ok\n" }} else {{ "bad\n" }})
Ok(())
}}"#
    )
}
fn native_cases(cases: Vec<(&str, String)>, large: bool) {
    use hirsel_driver::native::*;
    let dir = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(if large {
        "sorted-large"
    } else {
        "sorted-native"
    });
    let mut matrix = full_matrix();
    if large {
        matrix.retain(|c| matches!(c.engine, Engine::Native { .. }));
    }
    let toolchain = Toolchain::for_matrix(&dir.join("runtime"), &matrix).unwrap();
    for (name, source) in cases {
        let directory = dir.join(name);
        std::fs::create_dir_all(&directory).unwrap();
        let toolchain = &toolchain;
        let matrix = &matrix;
        with_compiler_stack(move || {
            let mut adapters = compiler_adapters();
            if large {
                adapters.interpreter = Some(Box::new(hirsel_engine::InterpreterAdapter {
                    limits: hirsel_interp::Limits {
                        steps: 100_000_000,
                        call_depth: 256,
                    },
                    ..Default::default()
                }));
            }
            let mut engine = QueryEngine::new(default_environment(), adapters);
            let file = engine.sources.add("test.hir", source.clone()).unwrap();
            check_progress(&mut engine, file, &default_context()).unwrap();
            let entry = entry_path(&default_context());
            let config = RunConfiguration {
                entry: entry.clone(),
                hash_seed: 0,
                inputs: vec![],
            };
            let reference =
                run_observed_snapshot(&mut engine, &config, Default::default()).unwrap();
            let reference = Observation::of_reference(
                reference
                    .observation
                    .as_ref()
                    .unwrap_or_else(|| panic!("{name}: {reference:?}")),
            );
            if large {
                assert_eq!(reference.output, b"ok\n");
            }
            let input = engine.export_native_input().unwrap();
            let input = engine.import_native_input(input).unwrap();
            for (configuration, outcome) in
                differential(&input, &entry, &reference, matrix, toolchain, &directory)
            {
                assert!(
                    matches!(outcome, Outcome::Agree),
                    "{name}: {configuration}: {outcome:?}\n{source}"
                );
            }
        });
    }
}
#[test]
#[ignore = "native: run by tools/check-native.sh"]
fn integer_function_values_and_each_comparator_failure_agree_in_every_configuration() {
    let mut cases = vec![("integer-orders", integer_orders())];
    for (key, name) in [(3, "first-probe"), (8, "second-probe"), (5, "third-probe")] {
        cases.push((name, comparator_failure(true, key)));
    }
    cases.push(("accepted-comparator", comparator_failure(false, 0)));
    cases.push(("cursor-break", resource_cursor(false)));
    cases.push(("cursor-drop-failure", resource_cursor(true)));
    native_cases(cases, false);
}
#[test]
#[ignore = "native scaling: run by tools/check-native.sh under host resource limits"]
fn ten_thousand_ascending_and_descending_inserts_agree_with_the_interpreter() {
    native_cases(
        vec![
            ("ascending", large_map(true)),
            ("descending", large_map(false)),
        ],
        true,
    );
}

#[test]
fn equal_counted_keys_keep_the_stored_key_and_a_break_drains_the_suffix() {
    let source = r#"fn compare(a:String,b:String)->i32 uses {} {
let x=string.byte_len(a)
let y=string.byte_len(b)
if x<y { -1 } else if x>y { 1 } else { 0 }
}
pub fn main(rt:Io,args:Args)->Result[(),GrantError] {
let by:Order[String]=Order(compare:compare)
let mut m:SortedMap[String,String] of by=sorted_map.empty(by)
sorted_map.insert(by,mut m,"old","first")
sorted_map.insert(by,mut m,"new","replaced")
for(k,v) in m { let _=out.print(io.stdout(rt)?,k); let _=out.print(io.stdout(rt)?,v) }
for(k,v) in take m { break }
Ok(())
}"#;
    assert_eq!(observed_heap(source).observation.output, b"oldreplaced");
}
#[test]
fn a_cursor_flag_that_ignores_entries_uniqueness_is_rejected() {
    use hirsel_lir::ir::*;
    let source = r#"pub fn main()->Result[(),()] {
let by=u64.order
let mut m:SortedMap[u64,u64] of by=sorted_map.empty(by)
sorted_map.insert(by,mut m,1,2)
for(k,v) in take m { break }
Ok(())
}"#;
    let (program, mut lowered) = lower_valid(source);
    let open = lowered
        .units
        .iter_mut()
        .find(|u| {
            u.header.definition == hirsel_syntax::DefPath::core("$intrinsic.SortedMapOrder_open")
        })
        .unwrap();
    let and = open
        .instructions
        .iter_mut()
        .find(|i| {
            matches!(
                i,
                Inst::Binary {
                    op: BinaryOp::BitAnd,
                    ..
                }
            )
        })
        .unwrap();
    if let Inst::Binary { left, right, .. } = and {
        *right = *left;
    }
    let errors = violations(&program, &lowered);
    assert!(
        errors.iter().any(|(_, v)| v.iter().any(|v| matches!(
            v.kind,
            hirsel_lir::validate::ViolationKind::Type("library uniqueness ownership")
        ))),
        "{errors:#?}"
    );
}

fn resource_cursor(failing: bool) -> String {
    format!(
        r#"resource type Trace {{ stdout: Stdout, key: u32 }}
drop(x) {{
let _=out.print(x.stdout,if x.key==1 {{ "one\n" }} else if x.key==2 {{ "two\n" }} else {{ "three\n" }})
if {failing} && x.key==3 {{ let z:u32=0; let _=1/z }}
}}
pub fn main(rt:Io,args:Args)->Result[(),GrantError] {{
let o=io.stdout(rt)?
let by=u32.order
let mut m:SortedMap[u32,Trace] of by=sorted_map.empty(by)
sorted_map.insert(by,mut m,3,Trace(stdout:o,key:3))
sorted_map.insert(by,mut m,1,Trace(stdout:o,key:1))
sorted_map.insert(by,mut m,2,Trace(stdout:o,key:2))
for(k,v) in take m {{ break }}
Ok(())
}}"#
    )
}
#[test]
fn a_break_drains_resources_last_first_and_continues_after_a_suffix_finalizer_fails() {
    for failing in [false, true] {
        let run = observed_heap(&resource_cursor(failing));
        assert_eq!(run.observation.output, b"one\nthree\ntwo\n");
        assert_eq!(run.observation.failures.len(), usize::from(failing));
    }
}

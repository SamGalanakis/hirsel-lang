//! Composed invocation promises. Koka Type/Unify.hs:177-214,374-404 and
//! test/type/wrong/{subsume,effect1}.kk (198fc3bb, Apache-2.0) supply
//! subsumption and nested-effect vector shapes. Hirsel CLO-2/CAP-2/LANG-6
//! determine these finite expectations; no Koka handler semantics are used.
use hirsel_driver::*;
use hirsel_engine::*;

fn checked(id: &str, source: &str, expected: Option<&str>) -> QueryEngine {
    let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
    let file = engine
        .sources
        .add("closure-effect-joins.hir", source.into())
        .unwrap();
    let output = check_progress(&mut engine, file, &default_context()).unwrap();
    assert_eq!(output.completion, CheckCompletion::Complete, "{id}");
    let rules: Vec<_> = output.diagnostics.iter().map(|d| d.rule.as_str()).collect();
    if let Some(rule) = expected {
        assert!(
            !rules.is_empty() && rules.iter().all(|r| *r == rule),
            "{id}: {source}\n{rules:?}\n{:?}",
            output.diagnostics
        );
    } else {
        assert!(rules.is_empty(), "{id}: {source}\n{:?}", output.diagnostics);
        let input = engine
            .export_native_input()
            .unwrap_or_else(|e| panic!("{id}: {e:?}"));
        engine
            .import_native_input(input)
            .unwrap_or_else(|e| panic!("{id}: {e:?}"));
    }
    eprintln!(
        "{id}: checker={rules:?}; independent-V={}",
        if expected.is_none() {
            "pass"
        } else {
            "unrun/rejected-source"
        }
    );
    engine
}

struct Case {
    id: &'static str,
    source: &'static str,
    expected: Option<&'static str>,
}
const CASES: &[Case] = &[
    Case {
        id: "CEJ26-01",
        source: "fn leaf()->u64 uses {} {37u64}\n@noalloc fn other()->u64 uses {} {37u64}\nfn choose(flag:bool)->fn()->u64 uses {} {if flag {leaf} else {other}}\npub fn main()->Result[(),u64] {let observed={let f=choose(false);f()};if observed==37u64 {Ok(())} else {Error(1u64)}}",
        expected: None,
    },
    Case {
        id: "CEJ26-02",
        source: "fn allocate(n:u64)->List[u64] in heap {[n]}\nfn leaf()->u64 uses {} {let counted=allocate(37u64);counted[0]}\n@noalloc fn other()->u64 uses {} {37u64}\nfn choose(flag:bool)->fn()->u64 uses {} {if flag {other} else {leaf}}\npub fn main()->Result[(),u64] {let observed={let f=choose(false);f()};if observed==37u64 {Ok(())} else {Error(1u64)}}",
        expected: None,
    },
    Case {
        id: "CEJ26-03",
        source: "fn leaf()->u64 uses {} {37u64}\n@noalloc fn other()->u64 uses {} {37u64}\nfn choose(flag:bool)->@noalloc fn()->u64 uses {} {if flag {leaf} else {other}}\npub fn main(){}",
        expected: Some("MEM-5"),
    },
    Case {
        id: "CEJ26-04",
        source: "fn leaf()->u64 uses {} {37u64}\n@noalloc fn other()->u64 uses {} {37u64}\nfn choose(flag:bool)->@noalloc fn()->u64 uses {} {if flag {other} else {leaf}}\npub fn main(){}",
        expected: Some("MEM-5"),
    },
    Case {
        id: "CEJ26-05",
        source: "@noalloc fn leaf()->u64 uses {} {37u64}\n@noalloc fn other()->u64 uses {} {37u64}\nfn choose(flag:bool)->fn()->u64 uses {} {if flag {leaf} else {other}}\npub fn main(){}",
        expected: None,
    },
    Case {
        id: "CEJ26-06",
        source: "@noalloc fn leaf()->u64 uses {} {37u64}\n@noalloc fn other()->u64 uses {} {37u64}\nfn choose(flag:bool)->fn()->u64 uses {} {if flag {other} else {leaf}}\npub fn main(){}",
        expected: None,
    },
    Case {
        id: "CEJ26-07",
        source: "@noalloc fn leaf()->u64 uses {} {37u64}\n@noalloc fn other()->u64 uses {} {37u64}\nfn choose(flag:bool)->@noalloc fn()->u64 uses {} {if flag {leaf} else {other}}\npub fn main(){}",
        expected: None,
    },
    Case {
        id: "CEJ26-08",
        source: "@noalloc fn leaf()->u64 uses {} {37u64}\n@noalloc fn other()->u64 uses {} {37u64}\nfn choose(flag:bool)->@noalloc fn()->u64 uses {} {if flag {other} else {leaf}}\npub fn main(){}",
        expected: None,
    },
    Case {
        id: "CEJ26-09",
        source: "fn allocate(n:u64)->List[u64] in heap {[n]}\nfn leaf()->u64 uses {} {let counted=allocate(37u64);counted[0]}\ntype Slot { read:fn()->u64 uses {} }\npub fn main()->Result[(),u64] {let observed={let held:Slot=Slot(read:leaf);held.read()};if observed==37u64 {Ok(())} else {Error(1u64)}}",
        expected: None,
    },
    Case {
        id: "CEJ26-10",
        source: "fn leaf()->u64 uses {} {37u64}\ntype Slot { read:@noalloc fn()->u64 uses {} }\npub fn main(){let held:Slot=Slot(read:leaf);let _=held}",
        expected: Some("MEM-5"),
    },
    Case {
        id: "CEJ26-11",
        source: "@noalloc fn leaf()->u64 uses {} {37u64}\ntype Slot { read:fn()->u64 uses {} }\npub fn main(){let held:Slot=Slot(read:leaf);let _=held}",
        expected: None,
    },
    Case {
        id: "CEJ26-12",
        source: "@noalloc fn leaf()->u64 uses {} {37u64}\ntype Slot { read:@noalloc fn()->u64 uses {} }\npub fn main()->Result[(),u64] {let observed={let held:Slot=Slot(read:leaf);held.read()};if observed==37u64 {Ok(())} else {Error(1u64)}}",
        expected: None,
    },
    Case {
        id: "CEJ26-13",
        source: "fn leaf()->u64 uses {} {let counted:List[u64] in heap=[37u64];counted[0]}\npub fn main()->Result[(),u64] {let observed={let held:List[fn()->u64 uses {}] in heap=[leaf];held[0]()};if observed==37u64 {Ok(())} else {Error(1u64)}}",
        expected: None,
    },
    Case {
        id: "CEJ26-14",
        source: "fn leaf()->u64 uses {} {37u64}\npub fn main(){let held:List[@noalloc fn()->u64 uses {}]=[leaf];let _=held}",
        expected: Some("MEM-5"),
    },
    Case {
        id: "CEJ26-15",
        source: "@noalloc fn leaf()->u64 uses {} {37u64}\npub fn main(){let held:List[fn()->u64 uses {}]=[leaf];let _=held}",
        expected: None,
    },
    Case {
        id: "CEJ26-16",
        source: "@noalloc fn leaf()->u64 uses {} {37u64}\npub fn main()->Result[(),u64] {let observed={let held:List[@noalloc fn()->u64 uses {}] in heap=[leaf];held[0]()};if observed==37u64 {Ok(())} else {Error(1u64)}}",
        expected: None,
    },
    Case {
        id: "CEJ26-17",
        source: "fn leaf()->u64 uses {} {37u64}\nfn make0()->fn()->u64 uses {} uses {} {leaf}\npub fn main(){let held:fn()->fn()->u64 uses {} uses {}=make0;let _=held}",
        expected: None,
    },
    Case {
        id: "CEJ26-18",
        source: "fn leaf()->u64 uses {} {37u64}\nfn make0()->fn()->u64 uses {} uses {} {leaf}\npub fn main(){let held:fn()->@noalloc fn()->u64 uses {} uses {}=make0;let _=held}",
        expected: Some("CLO-2"),
    },
    Case {
        id: "CEJ26-19",
        source: "@noalloc fn leaf()->u64 uses {} {37u64}\nfn make0()->@noalloc fn()->u64 uses {} uses {} {leaf}\npub fn main(){let held:fn()->fn()->u64 uses {} uses {}=make0;let _=held}",
        expected: None,
    },
    Case {
        id: "CEJ26-20",
        source: "@noalloc fn leaf()->u64 uses {} {37u64}\nfn make0()->@noalloc fn()->u64 uses {} uses {} {leaf}\npub fn main()->Result[(),u64] {let observed={let outer=make0;let f=outer();f()};if observed==37u64 {Ok(())} else {Error(1u64)}}",
        expected: None,
    },
    Case {
        id: "CEJ26-21",
        source: "fn leaf()->u64 uses {} {37u64}\nfn make0()->fn()->u64 uses {} uses {} {leaf}\nfn make1()->fn()->fn()->u64 uses {} uses {} uses {} {make0}\npub fn main(){let held:fn()->fn()->fn()->u64 uses {} uses {} uses {}=make1;let _=held}",
        expected: None,
    },
    Case {
        id: "CEJ26-22",
        source: "fn leaf()->u64 uses {} {37u64}\nfn make0()->fn()->u64 uses {} uses {} {leaf}\nfn make1()->fn()->fn()->u64 uses {} uses {} uses {} {make0}\npub fn main(){let held:fn()->fn()->@noalloc fn()->u64 uses {} uses {} uses {}=make1;let _=held}",
        expected: Some("CLO-2"),
    },
    Case {
        id: "CEJ26-23",
        source: "@noalloc fn leaf()->u64 uses {} {37u64}\nfn make0()->@noalloc fn()->u64 uses {} uses {} {leaf}\nfn make1()->fn()->@noalloc fn()->u64 uses {} uses {} uses {} {make0}\npub fn main(){let held:fn()->fn()->fn()->u64 uses {} uses {} uses {}=make1;let _=held}",
        expected: None,
    },
    Case {
        id: "CEJ26-24",
        source: "@noalloc fn leaf()->u64 uses {} {37u64}\nfn make0()->@noalloc fn()->u64 uses {} uses {} {leaf}\nfn make1()->fn()->@noalloc fn()->u64 uses {} uses {} uses {} {make0}\npub fn main(){let held:fn()->fn()->@noalloc fn()->u64 uses {} uses {} uses {}=make1;let _=held}",
        expected: None,
    },
    Case {
        id: "CEJ26-25",
        source: "@noalloc fn ignore(x:Bytes)->u64 uses {} {37u64}\nfn caller(flag:bool)->u64 uses {} {ignore({let x:Bytes in heap=b\"arg\";take x})}\npub fn main(){}",
        expected: None,
    },
    Case {
        id: "CEJ26-26",
        source: "@noalloc fn ignore(x:Bytes)->u64 uses {} {37u64}\n@noalloc fn caller(flag:bool)->u64 uses {} {ignore({let x:Bytes in heap=b\"arg\";take x})}\npub fn main(){}",
        expected: Some("MEM-5"),
    },
    Case {
        id: "CEJ26-27",
        source: "@noalloc fn ignore(x:Bytes)->u64 uses {} {37u64}\nfn caller(flag:bool)->u64 uses {} {ignore(if flag {let x:Bytes in heap=b\"left\";take x} else {let y:Bytes in heap=b\"right\";take y})}\npub fn main(){}",
        expected: None,
    },
    Case {
        id: "CEJ26-28",
        source: "@noalloc fn ignore(x:Bytes)->u64 uses {} {37u64}\n@noalloc fn caller(flag:bool)->u64 uses {} {ignore(if flag {let x:Bytes in heap=b\"left\";take x} else {let y:Bytes in heap=b\"right\";take y})}\npub fn main(){}",
        expected: Some("MEM-5"),
    },
    Case {
        id: "CEJ26-29",
        source: "@noalloc fn ignore(x:Bytes)->u64 uses {} {37u64}\nfn caller(flag:bool)->u64 uses {} {let saved=37u64;let f: @noalloc fn()->u64 uses {}=fn()->u64{saved};f()}\npub fn main()->Result[(),u64] {let observed={caller(false)};if observed==37u64 {Ok(())} else {Error(1u64)}}",
        expected: None,
    },
    Case {
        id: "CEJ26-30",
        source: "@noalloc fn ignore(x:Bytes)->u64 uses {} {37u64}\n@noalloc fn caller(flag:bool)->u64 uses {} {let saved=37u64;let f: @noalloc fn()->u64 uses {}=fn()->u64{saved};f()}\npub fn main(){}",
        expected: Some("MEM-5"),
    },
    Case {
        id: "CEJ26-31",
        source: "@noalloc fn ignore(x:Bytes)->u64 uses {} {37u64}\nfn caller(flag:bool)->u64 uses {} {let saved=37u64;let pair=({let f: @noalloc fn()->u64 uses {}=fn()->u64{saved};take f},);let (f,)=pair;f()}\npub fn main(){}",
        expected: None,
    },
    Case {
        id: "CEJ26-32",
        source: "@noalloc fn ignore(x:Bytes)->u64 uses {} {37u64}\n@noalloc fn caller(flag:bool)->u64 uses {} {let saved=37u64;let pair=({let f: @noalloc fn()->u64 uses {}=fn()->u64{saved};take f},);let (f,)=pair;f()}\npub fn main(){}",
        expected: Some("MEM-5"),
    },
    Case {
        id: "CEJ26-33",
        source: "fn leaf(x:Dir)->u64 {37u64}\nfn caller(x:Dir,flag:bool)->u64 {let f=if flag {leaf} else {leaf};f(x)}\npub fn main(){}",
        expected: None,
    },
    Case {
        id: "CEJ26-34",
        source: "fn leaf(x:Dir)->u64 {37u64}\n@incomplete fn caller(x:Dir,flag:bool)->u64 {let f=if flag {leaf} else {leaf};f(x)}\npub fn main(){}",
        expected: None,
    },
    Case {
        id: "CEJ26-35",
        source: "@incomplete fn leaf(x:Dir)->u64 {37u64}\nfn caller(x:Dir,flag:bool)->u64 {let f=if flag {leaf} else {leaf};f(x)}\npub fn main(){}",
        expected: Some("LANG-6"),
    },
    Case {
        id: "CEJ26-36",
        source: "@incomplete fn leaf(x:Dir)->u64 {37u64}\n@incomplete fn caller(x:Dir,flag:bool)->u64 {let f=if flag {leaf} else {leaf};f(x)}\npub fn main(){}",
        expected: None,
    },
    Case {
        id: "CEJ26-37",
        source: "fn leaf(x:u64)->u64 {37u64}\nfn caller(x:u64,flag:bool)->u64 {let pair=(leaf,);let (f,)=pair;f(x)}\npub fn main(){}",
        expected: None,
    },
    Case {
        id: "CEJ26-38",
        source: "fn leaf(x:u64)->u64 {37u64}\n@incomplete fn caller(x:u64,flag:bool)->u64 {let pair=(leaf,);let (f,)=pair;f(x)}\npub fn main(){}",
        expected: None,
    },
    Case {
        id: "CEJ26-39",
        source: "@incomplete fn leaf(x:u64)->u64 {37u64}\nfn caller(x:u64,flag:bool)->u64 {let pair=(leaf,);let (f,)=pair;f(x)}\npub fn main(){}",
        expected: Some("LANG-6"),
    },
    Case {
        id: "CEJ26-40",
        source: "@incomplete fn leaf(x:u64)->u64 {37u64}\n@incomplete fn caller(x:u64,flag:bool)->u64 {let pair=(leaf,);let (f,)=pair;f(x)}\npub fn main(){}",
        expected: None,
    },
];

#[test]
fn closure_effect_joins_26_source_03() {
    with_compiler_stack(|| {
        let c = &CASES[2];
        checked(c.id, c.source, c.expected);
    });
}

#[test]
fn closure_effect_joins_26_source_04() {
    with_compiler_stack(|| {
        let c = &CASES[3];
        checked(c.id, c.source, c.expected);
    });
}

#[test]
fn closure_effect_joins_26_source_05() {
    with_compiler_stack(|| {
        let c = &CASES[4];
        checked(c.id, c.source, c.expected);
    });
}

#[test]
fn closure_effect_joins_26_source_06() {
    with_compiler_stack(|| {
        let c = &CASES[5];
        checked(c.id, c.source, c.expected);
    });
}

#[test]
fn closure_effect_joins_26_source_07() {
    with_compiler_stack(|| {
        let c = &CASES[6];
        checked(c.id, c.source, c.expected);
    });
}

#[test]
fn closure_effect_joins_26_source_08() {
    with_compiler_stack(|| {
        let c = &CASES[7];
        checked(c.id, c.source, c.expected);
    });
}

#[test]
fn closure_effect_joins_26_source_10() {
    with_compiler_stack(|| {
        let c = &CASES[9];
        checked(c.id, c.source, c.expected);
    });
}

#[test]
fn closure_effect_joins_26_source_11() {
    with_compiler_stack(|| {
        let c = &CASES[10];
        checked(c.id, c.source, c.expected);
    });
}

#[test]
fn closure_effect_joins_26_source_14() {
    with_compiler_stack(|| {
        let c = &CASES[13];
        checked(c.id, c.source, c.expected);
    });
}

#[test]
fn closure_effect_joins_26_source_15() {
    with_compiler_stack(|| {
        let c = &CASES[14];
        checked(c.id, c.source, c.expected);
    });
}

#[test]
fn closure_effect_joins_26_source_17() {
    with_compiler_stack(|| {
        let c = &CASES[16];
        checked(c.id, c.source, c.expected);
    });
}

#[test]
fn closure_effect_joins_26_source_18() {
    with_compiler_stack(|| {
        let c = &CASES[17];
        checked(c.id, c.source, c.expected);
    });
}

#[test]
fn closure_effect_joins_26_source_19() {
    with_compiler_stack(|| {
        let c = &CASES[18];
        checked(c.id, c.source, c.expected);
    });
}

#[test]
fn closure_effect_joins_26_source_21() {
    with_compiler_stack(|| {
        let c = &CASES[20];
        checked(c.id, c.source, c.expected);
    });
}

#[test]
fn closure_effect_joins_26_source_22() {
    with_compiler_stack(|| {
        let c = &CASES[21];
        checked(c.id, c.source, c.expected);
    });
}

#[test]
fn closure_effect_joins_26_source_23() {
    with_compiler_stack(|| {
        let c = &CASES[22];
        checked(c.id, c.source, c.expected);
    });
}

#[test]
fn closure_effect_joins_26_source_24() {
    with_compiler_stack(|| {
        let c = &CASES[23];
        checked(c.id, c.source, c.expected);
    });
}

#[test]
fn closure_effect_joins_26_source_25() {
    with_compiler_stack(|| {
        let c = &CASES[24];
        checked(c.id, c.source, c.expected);
    });
}

#[test]
fn closure_effect_joins_26_source_26() {
    with_compiler_stack(|| {
        let c = &CASES[25];
        checked(c.id, c.source, c.expected);
    });
}

#[test]
fn closure_effect_joins_26_source_27() {
    with_compiler_stack(|| {
        let c = &CASES[26];
        checked(c.id, c.source, c.expected);
    });
}

#[test]
fn closure_effect_joins_26_source_28() {
    with_compiler_stack(|| {
        let c = &CASES[27];
        checked(c.id, c.source, c.expected);
    });
}

#[test]
fn closure_effect_joins_26_source_30() {
    with_compiler_stack(|| {
        let c = &CASES[29];
        checked(c.id, c.source, c.expected);
    });
}

#[test]
fn closure_effect_joins_26_source_31() {
    with_compiler_stack(|| {
        let c = &CASES[30];
        checked(c.id, c.source, c.expected);
    });
}

#[test]
fn closure_effect_joins_26_source_32() {
    with_compiler_stack(|| {
        let c = &CASES[31];
        checked(c.id, c.source, c.expected);
    });
}

#[test]
fn closure_effect_joins_26_source_33() {
    with_compiler_stack(|| {
        let c = &CASES[32];
        checked(c.id, c.source, c.expected);
    });
}

#[test]
fn closure_effect_joins_26_source_34() {
    with_compiler_stack(|| {
        let c = &CASES[33];
        checked(c.id, c.source, c.expected);
    });
}

#[test]
fn closure_effect_joins_26_source_35() {
    with_compiler_stack(|| {
        let c = &CASES[34];
        checked(c.id, c.source, c.expected);
    });
}

#[test]
fn closure_effect_joins_26_source_36() {
    with_compiler_stack(|| {
        let c = &CASES[35];
        checked(c.id, c.source, c.expected);
    });
}

#[test]
fn closure_effect_joins_26_source_37() {
    with_compiler_stack(|| {
        let c = &CASES[36];
        checked(c.id, c.source, c.expected);
    });
}

#[test]
fn closure_effect_joins_26_source_38() {
    with_compiler_stack(|| {
        let c = &CASES[37];
        checked(c.id, c.source, c.expected);
    });
}

#[test]
fn closure_effect_joins_26_source_39() {
    with_compiler_stack(|| {
        let c = &CASES[38];
        checked(c.id, c.source, c.expected);
    });
}

#[test]
fn closure_effect_joins_26_source_40() {
    with_compiler_stack(|| {
        let c = &CASES[39];
        checked(c.id, c.source, c.expected);
    });
}

fn runtime(index: usize, allocates: bool) {
    use hirsel_driver::native::*;
    let c = &CASES[index];
    let mut engine = checked(c.id, c.source, None);
    // The finite source model expects scalar 37; main maps only that to exit 0.
    // Every engine receives this same expectation, never another engine's output.
    let expected = Observation {
        exit_code: Some(0),
        blocked: false,
        budget: None,
        signaled: None,
        output: vec![],
        failures: vec![],
        roots: vec![],
    };
    let entry = entry_path(&default_context());
    let run = run_observed_snapshot(
        &mut engine,
        &RunConfiguration {
            entry: entry.clone(),
            hash_seed: 20261026,
            inputs: vec![],
        },
        Default::default(),
    )
    .unwrap();
    assert_eq!(
        Observation::of_reference(run.observation.as_ref().unwrap()),
        expected,
        "{} interpreter",
        c.id
    );
    eprintln!("{} interpreter: pass expected=37", c.id);
    let input = engine.export_native_input().unwrap();
    let validated = engine.import_native_input(input).unwrap();
    let mut options = Options::new(TargetSpec::x86_64_unknown_linux_gnu());
    options.validation = Validation::EveryStage;
    let ready = build(&validated, &options).unwrap();
    let heap = run_heap(&ready, &entry, &Sites::new(validated.snapshot())).unwrap();
    assert_eq!(heap.observation, expected, "{} checked-H", c.id);
    assert_eq!(
        heap.counters.allocations > 0,
        allocates,
        "{}: {:?}",
        c.id,
        heap.counters
    );
    assert_eq!(heap.counters.live_heap_bytes, 0, "{}", c.id);
    eprintln!(
        "{} checked-H: pass allocations={} frees={} live={}",
        c.id, heap.counters.allocations, heap.counters.frees, heap.counters.live_heap_bytes
    );
    let matrix = [
        Configuration {
            options: options.clone(),
            engine: Engine::Native {
                opt: OptLevel::Speed,
                partition: Partition::Whole,
                runtime: Runtime::Diagnostic,
            },
        },
        Configuration {
            options,
            engine: Engine::Cranelift {
                partition: Partition::Whole,
                runtime: Runtime::Diagnostic,
            },
        },
    ];
    let scratch = tempfile::Builder::new()
        .prefix("closure-effect-joins-26-")
        .tempdir()
        .unwrap();
    let toolchain = Toolchain::for_matrix(&scratch.path().join("runtime"), &matrix).unwrap();
    let outcomes = differential(
        &validated,
        &entry,
        &expected,
        &matrix,
        &toolchain,
        scratch.path(),
    );
    assert_eq!(outcomes.len(), 2);
    for (configuration, outcome) in outcomes {
        assert!(
            matches!(outcome, Outcome::Agree),
            "{} {configuration}: {outcome:?}",
            c.id
        );
        eprintln!("{} {configuration}: pass expected=37", c.id);
    }
}

#[test]
fn closure_effect_joins_26_runtime_01() {
    with_compiler_stack(|| runtime(0, false));
}

#[test]
fn closure_effect_joins_26_runtime_02() {
    with_compiler_stack(|| runtime(1, true));
}

#[test]
fn closure_effect_joins_26_runtime_09() {
    with_compiler_stack(|| runtime(8, true));
}

#[test]
fn closure_effect_joins_26_runtime_12() {
    with_compiler_stack(|| runtime(11, false));
}

#[test]
fn closure_effect_joins_26_runtime_13() {
    with_compiler_stack(|| runtime(12, true));
}

#[test]
fn closure_effect_joins_26_runtime_16() {
    with_compiler_stack(|| runtime(15, true));
}

#[test]
fn closure_effect_joins_26_runtime_20() {
    with_compiler_stack(|| runtime(19, false));
}

#[test]
fn closure_effect_joins_26_runtime_29() {
    with_compiler_stack(|| runtime(28, true));
}

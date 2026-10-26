//! Reference twins of H's direct graph fixtures. These execute source only on
//! reference/checked reference; they make no public native/H activation claim.
use hirsel_driver::*;
use hirsel_engine::*;
use hirsel_interp::{Outcome, library::Library};

#[test]
fn sealed_graph_reference_59_small_charge_vectors_agree_with_direct_h() {
    with_compiler_stack(|| {
        let mut vectors = Vec::new();
        for cap in [15, 16, 17] {
            vectors.push((
                cap,
                16,
                "List[u64]",
                "let root:List[u64] in heap=[11u64,12u64];".to_owned(),
                "root",
                "list.len(view)==2u64",
            ));
        }
        for cap in [7, 8, 15, 16] {
            vectors.push((
                cap,
                16,
                "List[List[u64]]",
                "let leaf:List[u64] in heap=[7u64];let root:List[List[u64]] in a=[copy leaf];"
                    .to_owned(),
                "root",
                "view[0][0]==7u64",
            ));
        }
        for live in [false, true] {
            for cap in [15, 16, 21, 22] {
                vectors.push((cap,22,if live{"List[Bytes]"}else{"u64"},
                "let h:Bytes in heap=copy_to(heap,b\"abc\");let retained:List[Bytes] in a=[copy h,copy h];let root=7u64;".to_owned(),
                if live{"retained"}else{"root"},if live{"list.len(view)==2u64"}else{"view==7u64"}));
            }
        }
        for distinct in [false, true] {
            for len in [1, 3] {
                let total = 16 + 2 * len;
                for cap in [total - 1, total] {
                    let text = if len == 1 { "a" } else { "abc" };
                    let setup = if distinct {
                        format!(
                            "let h:Bytes in heap=copy_to(heap,b\"{text}\");let j:Bytes in heap=copy_to(heap,b\"{text}\");let root:List[Bytes] in a=[take h,take j];"
                        )
                    } else {
                        format!(
                            "let h:Bytes in heap=copy_to(heap,b\"{text}\");let root:List[Bytes] in a=[copy h,copy h];"
                        )
                    };
                    vectors.push((
                        cap,
                        total,
                        "List[Bytes]",
                        setup,
                        "root",
                        "list.len(view)==2u64",
                    ));
                }
            }
        }
        for cow in [false, true] {
            let total = if cow { 33 } else { 22 };
            for cap in [total - 1, total] {
                let setup = format!(
                    "let h:Bytes in heap=copy_to(heap,b\"abc\");let mut root:List[Bytes] in a=[];list.push(mut root,copy h);{}list.push(mut root,copy h);{}",
                    if cow { "let old=copy root;" } else { "" },
                    if cow {
                        "failure.check(list.len(old)==1u64,Bounds());"
                    } else {
                        ""
                    }
                );
                vectors.push((
                    cap,
                    total,
                    "List[Bytes]",
                    setup,
                    "root",
                    "list.len(view)==2u64",
                ));
            }
        }
        assert_eq!(vectors.len(), 27);
        for (cap, total, ty, setup, root, read) in vectors {
            let source = format!(
                "pub fn main(){{let outcome=arena.bounded({cap}u64,fn(a:Arena)->Region[{ty}]{{{setup}arena.seal(take a,take {root})}});let accepted=match take outcome{{Ok(r)=>{{let mut owner=take r;region.with(mut owner,fn(view:{ty})->bool{{{read}}})}},Error(_)=>false,}};failure.check(accepted=={},Bounds());}}",
                cap >= total
            );
            let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
            let file = engine
                .sources
                .add("sealed-graph-reference-59.hir", source.clone())
                .unwrap();
            let checked = check_progress(&mut engine, file, &default_context()).unwrap();
            assert!(
                checked.diagnostics.is_empty(),
                "{source}\n{:?}",
                checked.diagnostics
            );
            let exported = engine.export_native_input().unwrap();
            engine.import_native_input(exported).unwrap();
            for checked in [false, true] {
                let runtime = if checked {
                    engine.checked_runtime_types()
                } else {
                    engine.runtime_types()
                }
                .unwrap();
                let run = RunConfiguration {
                    entry: entry_path(&default_context()),
                    hash_seed: 0,
                    inputs: vec![],
                };
                let sources = engine.sources.clone();
                let mut library = Library::new(&run, SimulationConfig::default()).unwrap();
                let execution = hirsel_interp::run_with_runtime(
                    &mut engine,
                    &sources,
                    &run,
                    vec![],
                    Default::default(),
                    &mut library,
                    &runtime,
                )
                .unwrap();
                assert!(
                    matches!(execution.outcome, Outcome::Returned { exit_code: 0, .. }),
                    "cap={cap} checked={checked} {source}\n{:?}",
                    execution.outcome
                );
            }
        }
    });
}

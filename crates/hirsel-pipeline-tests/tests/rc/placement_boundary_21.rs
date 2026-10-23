//! Neighbors of the repaired schema/Option fieldless-alternative control
//! (d5a24c98); that completed oracle stays in native_noalloc_list_19.
//! Rust vec.rs:257-292 and vec/mod.rs:4054-4092 motivate empty/equal/short/long
//! saved-owner snapshots. Reuse ownership_history_21's runner and existing copy
//! ports; Hirsel copies may share, so buffer identity is deliberately unobserved.
use crate::rc_support::{self as support, history};
use hirsel_driver::native::*;
use hirsel_lir::ir::{Inst, Placement, StorageOwner, ValueRepr};

const PRELUDE: &str = r#"
type Empty {}
type Row { rows: List[List[u64]] }
resource type Trace { out: Stdout, text: String }
 drop(x) { let _ = out.print(x.out, x.text) }
fn fail() -> u64 { let zero:u64=0; 1u64/zero }
"#;

// Six representations, including both empty inline products and a nullary
// alternative whose layout can also carry nested counted children.
const SHAPES: [(&str, &str, &str); 6] = [
    ("()", "()", "()"),
    ("(List[List[u64]],u64)", "([[],[1,2]],7)", "([[3]],8)"),
    ("Option[List[List[u64]]]", "None()", "Some([[],[3]])"),
    ("Empty", "Empty()", "Empty()"),
    ("Row", "Row(rows:[[],[1,2]])", "Row(rows:[[3]])"),
    ("List[List[u64]]", "[[],[1,2]]", "[[3]]"),
];

fn snapshot(shape: usize, value: &str, prefix: &str) -> String {
    let print = |text: &str| format!("let _=out.print(stdout,\"{text}\");");
    let rows = |value: &str| {
        format!(
            "{} for {prefix}_row in {value} {{ {}\n\
             for {prefix}_n in {prefix}_row {{\n\
               let _=out.print(stdout,match {prefix}_n {{1=>\"1\",2=>\"2\",3=>\"3\",_=>\"?\"}});\n\
             }} {} }} {}",
            print("["),
            print("["),
            print("]"),
            print("]")
        )
    };
    let item = format!("{prefix}_item");
    let element = match shape {
        0 => print("()"),
        1 => format!(
            "let ({prefix}_rows,{prefix}_tag)={item}; {} {}\n\
             let _=out.print(stdout,if {prefix}_tag==7 {{\"7\"}} else {{\"8\"}});",
            print("T"),
            rows(&format!("{prefix}_rows"))
        ),
        2 => format!(
            "match {item} {{None=>{{ {} }},Some({prefix}_rows)=>{{ {} {} }} }};",
            print("N"),
            print("S"),
            rows(&format!("{prefix}_rows"))
        ),
        3 => print("E"),
        4 => format!("{} {}", print("R"), rows(&format!("{item}.rows"))),
        5 => rows(&item),
        _ => unreachable!(),
    };
    format!(
        "{{ {} for {item} in {value} {{ {element} }} {} }};",
        print("["),
        print("]")
    )
}

// Immutable textual snapshots, independent of source emission and placement.
fn expected(shape: usize, profile: usize) -> String {
    let (a, b) = [
        ("()", "()"),
        ("T[[][12]]7", "T[[3]]8"),
        ("N", "S[[][3]]"),
        ("E", "E"),
        ("R[[][12]]", "R[[3]]"),
        ("[[][12]]", "[[3]]"),
    ][shape];
    match profile {
        0 => format!("[][{a}{b}{a}]ok21"),
        1 => format!("[{a}][{b}][{b}][{b}]ok21"),
        2 => format!("[{a}{b}{a}][{b}]ok21"),
        3 => format!("[{b}]21"),
        _ => unreachable!(),
    }
}

fn source(shape: usize, profile: usize) -> String {
    let (ty, a, b) = SHAPES[shape];
    let long = format!("[{a},{b},{a}]");
    let short = format!("[{b}]");
    // The four Rust length shapes, applied to every representation without
    // duplicating the full nested-copy ownership histories.
    let (before, after) = match profile {
        0 => ("[]".to_owned(), long),
        1 => (short.clone(), short),
        2 => (long, short),
        3 => (short, long),
        _ => unreachable!(),
    };
    let mut text = PRELUDE.to_owned();
    if profile == 2 {
        // Returning a block and returning a closure are escapes. A foreign
        // child alongside a fresh sibling must keep the whole container counted.
        text.push_str(&format!(
            "fn make()->List[{ty}] {{ {before} }}\n\
             fn foreign(take xs:List[{ty}])->List[List[{ty}]] {{ [take xs,{after}] }}\n\
             fn callback(take xs:List[List[{ty}]])->fn()->bool uses {{}} {{\n\
               fn()->bool {{ xs==[{before},{after}] }}\n}}\n"
        ));
    }
    text.push_str(
        "pub fn main(rt:Io)->Result[(),GrantError] {\n\
         let stdout=io.stdout(rt)?\n\
         let first=Trace(out:stdout,text:\"1\")\n\
         let second=Trace(out:stdout,text:\"2\")\n",
    );
    let body = match profile {
        0 => format!(
            "let mut xs:List[{ty}]={before}; let saved=copy xs;\n\
             xs={after}; {} {} saved=={before} && xs=={after}",
            snapshot(shape, "saved", "saved"),
            snapshot(shape, "xs", "current")
        ),
        1 => format!(
            // Both branch directions execute; None and Some meet at a join.
            "let flags:List[bool]=[true,false]; let mut good=true;\n\
             for flag in flags {{\n\
               let xs:List[{ty}]=if flag {{ [{a}] }} else {{ [{b}] }};\n\
               let saved=copy xs;\n\
               let joined:List[{ty}]=if flag {{ {after} }} else {{ {before} }};\n\
             {} {} good=good && list.len(saved)==1 && joined=={after}\n\
             }}; good",
            snapshot(shape, "saved", "saved"),
            snapshot(shape, "joined", "joined")
        ),
        2 => format!(
            "let xs=make(); let nested=foreign(take xs);\n\
              {} {} let f=callback(take nested); f()",
            snapshot(shape, "nested[0]", "returned"),
            snapshot(shape, "nested[1]", "sibling")
        ),
        3 => format!(
            // fail() runs after two nested owners are prepared, before the
            // tuple is committed. Its unreachable observation must be skipped.
            "let xs:List[{ty}]={before}; let saved=copy xs;\n\
             {}\n\
             let pending:(List[List[{ty}]],u64)=([copy xs,{after}],fail());\n\
             let (prepared_rows,prepared_number)=take pending;\n\
             saved=={before} && prepared_number==0 && list.len(prepared_rows)==2",
            snapshot(shape, "saved", "saved")
        ),
        _ => unreachable!(),
    };
    text.push_str(&format!(
        "let good={{ {body} }};\n\
         let _=out.print(stdout,if good {{\"ok\"}} else {{\"bad\"}}); Ok(())\n}}\n"
    ));
    text
}

fn matrix() -> Vec<Configuration> {
    [true, false]
        .into_iter()
        .flat_map(|regions| {
            let mut options = Options::new(support::target());
            options.regions = regions;
            [
                Engine::Heap { campaign: true },
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
        .collect()
}

#[test]
fn generated_siblings_all_engines() {
    use hirsel_driver::compiler_adapters;
    use hirsel_engine::*;
    let directory =
        std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("placement-boundary-21");
    let runner = history::Runner::with_matrix(&directory, matrix())
        .expect("toolchain setup is incomplete, not a language failure");
    let mut failures = Vec::new();
    let mut counts = std::collections::BTreeMap::new();
    for shape in 0..SHAPES.len() {
        for profile in 0..4 {
            let name = format!("shape-{shape}-profile-{profile}");
            let source = source(shape, profile);
            history::persist(
                &directory,
                &name,
                (shape * 4 + profile) as u64,
                &source,
                &format!("shape={shape} profile={profile}"),
            );
            let report = runner.run(&name, &source);
            let expected = expected(shape, profile);
            let differences = report.disagreements(expected.as_bytes(), profile == 3);
            for cell in &report.cells {
                *counts
                    .entry(format!("{}:{:?}", cell.engine, cell.category))
                    .or_insert(0) += 1;
            }
            if !differences.is_empty() || report.cells.len() != 7 {
                history::persist(
                    &directory,
                    &name,
                    (shape * 4 + profile) as u64,
                    &source,
                    &format!("{report:#?}\nexpected={expected:?}"),
                );
                failures.push((name, differences));
                continue;
            }
            // Additional clean-run counters expose real region/holder cleanup;
            // the shared runner separately injects failures at H preparation sites.
            with_compiler_stack(|| {
                let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
                let file = engine.sources.add("main.hir", source.clone()).unwrap();
                check_progress(&mut engine, file, &default_context()).unwrap();
                let input = engine.export_native_input().unwrap();
                let validated = engine.import_native_input(input).unwrap();
                for regions in [true, false] {
                    let mut options = Options::new(support::target());
                    options.regions = regions;
                    let ready = build(&validated, &options).unwrap();
                    let heap = run_heap(
                        &ready,
                        &entry_path(&default_context()),
                        &Sites::new(validated.snapshot()),
                    )
                    .unwrap();
                    assert_eq!(
                        Some(&heap.observation),
                        report.cells[0].observation.as_ref(),
                        "{name}"
                    );
                    assert_eq!(heap.counters.live_heap_bytes, 0, "{name}");
                    eprintln!("{name} regions={regions}: {:?}", heap.counters);
                }
            });
            eprintln!(
                "{name}: source/V/C accepted; seven observations agree; preparation_failure={}",
                profile == 3
            );
        }
    }
    eprintln!("placement-boundary coverage: {counts:?}");
    assert!(failures.is_empty(), "{failures:#?}");
    assert_eq!(counts.values().sum::<usize>(), 24 * 7);
}

#[test]
fn inline_and_foreign_placement_stays_conservative() {
    let mut local_blocks = 0;
    let mut inline_empty = 0;
    for shape in 0..SHAPES.len() {
        for profile in 0..4 {
            let (program, lowered) = support::lower_valid(&source(shape, profile));
            let manifest = support::manifest(&program);
            let target = support::target();
            let operations = program.operation_registry();
            let inputs = hirsel_lir::validate::Inputs {
                memo: Default::default(),
                discharge: None,
                operations: &operations,
                target: &target,
                layouts: &lowered.table,
                manifest: &manifest,
            };
            for regions in [true, false] {
                for (unit, &origin) in lowered.units.iter().zip(&lowered.unit_origins) {
                    let placed = hirsel_rc::placement::place(
                        unit.clone(),
                        &lowered.table,
                        &hirsel_rc::placement::Options::for_instance(
                            regions,
                            &lowered.instances,
                            origin,
                        ),
                    )
                    .unwrap();
                    assert!(
                        hirsel_lir::validate::check(&placed, &inputs).is_empty(),
                        "{}",
                        support::dump(&placed)
                    );
                    for inst in &placed.instructions {
                        let Inst::Construct {
                            result, construct, ..
                        } = *inst
                        else {
                            continue;
                        };
                        let site = &placed.tables.constructs[construct.index()];
                        let regional = matches!(
                            site.placement,
                            Some(Placement::Fixed(StorageOwner::LocalRegion(_)))
                        );
                        let original = &unit.types[unit.values[result.index()].ty.index()];
                        if original.repr == ValueRepr::Aggregate && site.fields.len == 0 {
                            inline_empty += 1;
                            assert!(!regional);
                            assert_eq!(
                                placed.types[placed.values[result.index()].ty.index()].repr,
                                ValueRepr::Aggregate
                            );
                        }
                        if regional {
                            local_blocks += 1;
                        }
                        if !regions
                            || matches!(
                                placed.header.definition.0.name.as_str(),
                                "make" | "foreign"
                            )
                        {
                            assert!(
                                !regional,
                                "escaping/foreign block in {}",
                                support::dump(&placed)
                            );
                        }
                    }
                }
            }
        }
    }
    assert!(inline_empty > 0);
    assert!(local_blocks > 0);
    eprintln!(
        "24 programs x two placement options: fieldless_inline={inline_empty}, local_blocks={local_blocks}"
    );
}

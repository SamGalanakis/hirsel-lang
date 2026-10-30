//! Definition summaries carry what instance selection and GEN-5 read, so the
//! selections they imply equal those today's checked bodies give.
use hirsel_driver::*;
use hirsel_engine::*;
use hirsel_types::{
    ActualBindings, BinderRef, GenericArgument, NameSite, Size, SizeParameterRef, TypeKind,
};
use std::collections::{BTreeMap, BTreeSet};

fn check(source: &str) -> (QueryEngine, CheckOutput) {
    let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
    let file = engine.sources.add("summary.hir", source.into()).unwrap();
    let output = check_progress(&mut engine, file, &default_context()).unwrap();
    (engine, output)
}
fn summaries(engine: &mut QueryEngine) -> BTreeMap<String, (DefinitionSummary, ParameterGrowth)> {
    engine
        .definition_summaries()
        .unwrap()
        .into_iter()
        .map(|pair| (pair.0.owner.path().0.name.clone(), pair))
        .collect()
}
fn path(name: &str) -> DefPath {
    let mut p = entry_path(&default_context());
    p.0.name = name.into();
    p
}
fn typed(engine: &mut QueryEngine, name: &str) -> std::sync::Arc<TypedBody> {
    engine.mode_definition(&path(name)).unwrap().value.typed
}

const WIDTH: &str = r#"
type Point {
  x: u64,
  y: u64,
}

type Box3 {
  a: u64,
  b: u64,
  c: u64,
}

fn width_body[T](s: TypeShape[T]) -> Result[Body[fn(T) -> u64 uses {}], String] uses {} {
  let info = type_shape.info(s)
  let n = match info.body {
    Record(fields) => list.len(fields),
    _ => 0u64,
  }
  if n == 2 {
    return Ok(codegen.body("fn(v: standalone@0/Point) -> u64 uses {} { 2 }"))
  }
  Ok(codegen.body("fn(v: standalone@0/Box3) -> u64 uses {} { 3 }"))
}

fn width[T](v: T) -> u64 uses {} = comptime width_body(shape(T))
"#;

/// CT-5 over summaries alone: walk each root's calls that name another
/// definition, closing their open arguments with the bindings carried into
/// the owner, and collect the closed actuals each generated callee is
/// selected at.
fn generated_selections(
    engine: &mut QueryEngine,
    summaries: &BTreeMap<String, (DefinitionSummary, ParameterGrowth)>,
    roots: &[&str],
    generated: &str,
) -> BTreeSet<Vec<String>> {
    let mut selected = BTreeSet::new();
    let mut pending: Vec<(String, ActualBindings)> = roots
        .iter()
        .map(|root| (root.to_string(), BTreeMap::new()))
        .collect();
    let mut seen = BTreeSet::new();
    while let Some((owner, bindings)) = pending.pop() {
        let key: Vec<_> = bindings
            .iter()
            .map(|(site, actuals)| {
                (
                    *site,
                    hirsel_tir::InstanceBodies::key(engine.types(), &actuals.arguments),
                )
            })
            .collect();
        if !seen.insert((owner.clone(), key)) {
            continue;
        }
        let Some((summary, _)) = summaries.get(&owner) else {
            continue;
        };
        let types = engine.types().clone();
        let memo = engine.type_memo().clone();
        for call in summary
            .calls
            .iter()
            .filter(|call| call.target_frame.is_none())
        {
            let callee = call.callee.path().0.name.clone();
            let actuals =
                hirsel_types::close_arguments(&types, &memo, &call.arguments, &bindings).unwrap();
            let closed = hirsel_tir::InstanceBodies::key(&types, &actuals.arguments);
            if callee == generated {
                selected.extend(
                    closed.map(|key| key.spellings().into_iter().map(String::from).collect()),
                );
            } else if actuals.arguments.is_empty() {
                pending.push((callee, BTreeMap::new()));
            } else if closed.is_some() {
                pending.push((callee, BTreeMap::from([(call.binder, actuals)])));
            }
        }
    }
    selected
}

#[test]
fn a_generic_forwarder_summarizes_one_open_call_selected_at_each_actual() {
    hirsel_engine::with_compiler_stack(
        a_generic_forwarder_summarizes_one_open_call_selected_at_each_actual_on_compiler_thread,
    );
}
fn a_generic_forwarder_summarizes_one_open_call_selected_at_each_actual_on_compiler_thread() {
    let source = format!(
        "{WIDTH}
fn forward[T](v: T) -> u64 uses {{}} {{
  width(v)
}}

pub fn main() -> Result[(), ()] {{
  let _ = forward(Point(x: 1, y: 2))
  let _ = forward(Box3(a: 1, b: 2, c: 3))
  Ok(())
}}
"
    );
    let (mut engine, output) = check(&source);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let summaries = summaries(&mut engine);
    let (forward, growth) = &summaries["forward"];
    assert_eq!(forward.frames.len(), 1);
    assert_eq!(forward.frames[0].kind, FrameKind::Item);
    let [call] = &forward.calls[..] else {
        panic!("{:?}", forward.calls)
    };
    assert_eq!(call.callee.path().0.name, "width");
    assert_eq!(call.class, CallClass::Runtime);
    assert_eq!((call.owning_frame, call.target_frame), (0, None));
    // The argument stays open: forward's own parameter, not an actual.
    let types = engine.types().clone();
    let [GenericArgument::Type(argument)] = &call.arguments[..] else {
        panic!("{:?}", call.arguments)
    };
    let TypeKind::Parameter(BinderRef::Body { scope, .. }) = &types.get(*argument).unwrap().kind
    else {
        panic!("closed argument")
    };
    assert_eq!(scope, &forward.item_binder());
    // One stable edge forward.T -> width.T.
    let [(from, to, grows, _)] = &growth.edges[..] else {
        panic!("{:?}", growth.edges)
    };
    assert_eq!(
        (
            from.0.path().0.name.as_str(),
            to.0.path().0.name.as_str(),
            *grows
        ),
        ("forward", "width", false)
    );
    let selected = generated_selections(&mut engine, &summaries, &["main"], "width");
    let names: BTreeSet<_> = selected.iter().map(|key| key.join(",")).collect();
    assert_eq!(names.len(), 2, "{names:?}");
    assert!(
        names.iter().any(|n| n.ends_with("Point")) && names.iter().any(|n| n.ends_with("Box3"))
    );
    // The same instances the checked bodies selected.
    let generated: Vec<_> = engine
        .generated_sources()
        .filter(|(path, _)| path.0.name.starts_with("width"))
        .map(|(_, source)| source.text.clone())
        .collect();
    assert_eq!(generated.len(), 2, "{generated:?}");
    for name in ["Point", "Box3"] {
        assert!(
            generated.iter().any(|text| text.contains(name)),
            "{generated:?}"
        );
    }
}

#[test]
fn nested_local_size_generic_functions_capture_the_enclosing_size() {
    hirsel_engine::with_compiler_stack(
        nested_local_size_generic_functions_capture_the_enclosing_size_on_compiler_thread,
    );
}
fn nested_local_size_generic_functions_capture_the_enclosing_size_on_compiler_thread() {
    let source = "
fn outer[n: size](xs: Inline[u8, n]) -> u64 {
  fn inner[m: size](ys: Inline[u8, m]) -> u64 {
    let _ = ys
    comptime (n * m)
  }
  inner(xs)
}

pub fn main() -> Result[(), ()] {
  let xs: Inline[u8, 2] = [1, 2]
  let ys: Inline[u8, 3] = [1, 2, 3]
  let _ = outer(xs) + outer(ys)
  Ok(())
}
";
    let (mut engine, output) = check(source);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let summaries = summaries(&mut engine);
    let (outer, _) = &summaries["outer"];
    let kinds: Vec<_> = outer.frames.iter().map(|f| (f.kind, f.parent)).collect();
    assert_eq!(
        kinds,
        [
            (FrameKind::Item, None),
            (FrameKind::LocalFunction, Some(0)),
            (FrameKind::ComptimeRegion, Some(1)),
        ]
    );
    let n = SizeParameterRef::body(outer.item_binder(), hirsel_types::SizeParamId(0));
    let m = SizeParameterRef::body(
        outer.item_binder().owner.site(outer.frames[1].site),
        hirsel_types::SizeParamId(0),
    );
    // inner captures outer's n; the region captures n and inner's m.
    assert_eq!(&*outer.frames[0].size_bindings, &[]);
    assert_eq!(&*outer.frames[1].size_bindings, &[(n, Size::Parameter(n))]);
    let region: BTreeSet<_> = outer.frames[2]
        .size_bindings
        .iter()
        .map(|(p, _)| *p)
        .collect();
    assert_eq!(region, BTreeSet::from([n, m]));
    let [(site, inputs)] = &outer.staged[..] else {
        panic!("{:?}", outer.staged)
    };
    assert_eq!(*site, outer.frames[2].site);
    assert_eq!(
        BTreeSet::from_iter(inputs.iter().copied()),
        BTreeSet::from([n, m])
    );
    let [call] = &outer.calls[..] else {
        panic!("{:?}", outer.calls)
    };
    assert_eq!(call.target_frame, Some(1));
    assert_eq!(
        call.binder,
        outer.item_binder().owner.site(outer.frames[1].site)
    );
    assert_eq!(
        &*call.arguments,
        &[GenericArgument::Size(Size::Parameter(n))]
    );
    assert_eq!(call.class, CallClass::Runtime);

    // Staged selection over summaries: each size outer is called at selects
    // the region's constant at (n, n), through inner's call by name.
    let staged = staged_selections(&summaries, "main");
    let inputs: Vec<_> = inputs.to_vec();
    let expected: BTreeSet<_> = [2u64, 3]
        .into_iter()
        .map(|k| {
            (
                outer.owner,
                *site,
                inputs.iter().map(|_| k).collect::<Vec<_>>(),
            )
        })
        .collect();
    assert_eq!(staged, expected);
}

/// Staged-constant selection over summaries alone: a frame selects the
/// constants of its own comptime regions and enters the non-generic frames
/// it encloses; calls outside comptime enter their callee's frame with the
/// call's size actuals.
fn staged_selections(
    summaries: &BTreeMap<String, (DefinitionSummary, ParameterGrowth)>,
    root: &str,
) -> BTreeSet<(hirsel_syntax::DefId, NameSite, Vec<u64>)> {
    let mut selected = BTreeSet::new();
    let mut pending = vec![(
        root.to_string(),
        0u32,
        BTreeMap::<SizeParameterRef, u64>::new(),
    )];
    let mut seen = BTreeSet::new();
    while let Some((owner, frame, bindings)) = pending.pop() {
        if !seen.insert((owner.clone(), frame, bindings.clone())) {
            continue;
        }
        let Some((summary, _)) = summaries.get(&owner) else {
            continue;
        };
        let function_frame = |mut f: u32| {
            while summary.frames[f as usize].kind == FrameKind::ComptimeRegion {
                f = summary.frames[f as usize].parent.unwrap();
            }
            f
        };
        for (site, inputs) in summary.staged.iter() {
            let region = summary.frames.iter().position(|f| f.site == *site).unwrap() as u32;
            if function_frame(summary.frames[region as usize].parent.unwrap()) == frame {
                let actuals = inputs.iter().map(|p| bindings[p]).collect();
                selected.insert((summary.owner, *site, actuals));
            }
        }
        for call in summary.calls.iter() {
            if call.owning_frame != frame || call.class == CallClass::ComptimeExecuted {
                continue;
            }
            let (callee, target) = match call.target_frame {
                Some(target) => (owner.clone(), target),
                None => (call.callee.path().0.name.clone(), 0),
            };
            let mut actuals = match call.target_frame {
                Some(target) => summary.frames[target as usize]
                    .size_bindings
                    .iter()
                    .map(|(p, _)| (*p, bindings[p]))
                    .collect(),
                None => BTreeMap::new(),
            };
            for (ordinal, argument) in call.arguments.iter().enumerate() {
                if let GenericArgument::Size(size) = argument {
                    let value = match size {
                        Size::Constant(n) => *n,
                        Size::Parameter(p) => bindings[p],
                    };
                    actuals.insert(
                        SizeParameterRef::body(
                            call.binder,
                            hirsel_types::SizeParamId(ordinal as u32),
                        ),
                        value,
                    );
                }
            }
            pending.push((callee, target, actuals));
        }
    }
    selected
}

#[test]
fn a_generated_call_in_an_unexecuted_comptime_branch_is_selected_by_execution() {
    hirsel_engine::with_compiler_stack(
        a_generated_call_in_an_unexecuted_comptime_branch_is_selected_by_execution_on_compiler_thread,
    );
}
fn a_generated_call_in_an_unexecuted_comptime_branch_is_selected_by_execution_on_compiler_thread() {
    let source = format!(
        "{WIDTH}
fn pick() -> u64 uses {{}} {{
  comptime (if false {{ width(Point(x: 1, y: 2)) }} else {{ 0u64 }})
}}

pub fn main() -> Result[(), ()] {{
  let _ = pick()
  Ok(())
}}
"
    );
    let (mut engine, output) = check(&source);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let summaries = summaries(&mut engine);
    let (pick, _) = &summaries["pick"];
    let regions: Vec<_> = pick
        .frames
        .iter()
        .enumerate()
        .filter(|(_, f)| f.kind == FrameKind::ComptimeRegion)
        .map(|(i, _)| i as u32)
        .collect();
    assert_eq!(regions.len(), 1, "{:?}", pick.frames);
    let width: Vec<_> = pick
        .calls
        .iter()
        .filter(|call| call.callee.path().0.name == "width")
        .collect();
    assert!(!width.is_empty());
    for call in width {
        assert_eq!(call.class, CallClass::ComptimeExecuted);
        assert_eq!(call.owning_frame, regions[0]);
    }
    // CT-5 still selects the instance statically, as the checked body did.
    let selected = generated_selections(&mut engine, &summaries, &["main"], "width");
    let generated = engine
        .generated_sources()
        .filter(|(path, _)| path.0.name.starts_with("width"))
        .count();
    assert_eq!(selected.len(), generated);
    assert_eq!(generated, 1);
}

fn growth_edges_give_the_checked_bodies_gen5_verdict_on_compiler_thread() {
    for source in [
        "fn grow[T: copyable](x:T) { grow((copy x,copy x)) }",
        "fn first[T: copyable](x:T) { second((copy x,copy x)) }\nfn second[U: copyable](x:U) { first(x) }",
        "fn grow[T: copyable](x:T) { grow(x) }",
        "fn first[T: copyable](x:T) { second((copy x,copy x)) }\nfn second[U](x:U) {}",
        "fn first[T: copyable](x:T) { second((copy x,copy x)) }\nfn second[U: copyable](x:(U,U)) { let (a,b)=x; first(a) }",
        "fn first[T: copyable](x:T) { second((copy x,copy x)) }\nfn second[U: copyable](x:U) { first(1u32) }",
    ] {
        let (mut engine, _) = check(source);
        let edges: Vec<_> = engine
            .definition_summaries()
            .unwrap()
            .into_iter()
            .flat_map(|(_, growth)| growth.edges.into_iter())
            .collect();
        let graph = engine.gen5_source_graph().unwrap();
        assert_eq!(growing_sites(&edges), graph.growing_sites(), "{source}");
    }
}

/// Published summaries survive body release, serialize using canonical type
/// rows, restore reverse subscriptions, and reject stale source/read inputs.
#[test]
fn published_summary_survives_body_release_and_rejects_stale_seeds() {
    with_compiler_stack(|| {
        use std::sync::Arc;
        let source = "fn helper(x: u64) -> u64 { x }\nfn main() -> u64 { helper(1) }\n";
        let (mut engine, output) = check(source);
        assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
        let main = hirsel_syntax::DefId::intern(&path("main"));
        let early = engine.checked_definition_summary(main).unwrap();
        let analyzed = engine.mode_definition(main.path()).unwrap();
        let mode_summary = summarize(engine.types(), &analyzed.value.typed);
        let summary = engine
            .publish_definition(main, hirsel_engine::memo::Retention::Batch)
            .unwrap();
        assert_eq!(summary.summary, mode_summary.0);
        assert_eq!(summary.growth, mode_summary.1);
        let checked = summary.checked.as_ref().expect("pre-mode input retained");
        assert_eq!((&checked.summary, &checked.growth), (&early.0, &early.1));
        assert!(engine.memo_reads(main.path(), QueryKind::Check).is_none());
        assert!(engine.memo_reads(main.path(), QueryKind::Modes).is_none());
        let checks = engine.recompute_count(main.path(), QueryKind::Check);
        assert!(Arc::ptr_eq(
            &summary,
            &engine.definition_summary(main).unwrap()
        ));
        assert_eq!(
            engine.recompute_count(main.path(), QueryKind::Check),
            checks
        );

        assert_eq!(engine.checked_definition_summary(main).unwrap(), early);
        assert_eq!(
            engine.recompute_count(main.path(), QueryKind::Check),
            checks
        );

        let (_, roots) =
            hirsel_types::with_table_collection(|| serde_json::to_vec(&*summary).unwrap());
        let (table, _) = hirsel_types::encode_table(engine.types(), &roots);
        let bytes = hirsel_types::with_table_encoding(engine.types(), &roots, || {
            serde_json::to_vec(&*summary).unwrap()
        });
        let ids = hirsel_types::decode_table(engine.types(), &table);
        let seeded = hirsel_types::with_table_decoding(&ids, || {
            serde_json::from_slice::<PublishedSummary>(&bytes).unwrap()
        });
        let seeded = Arc::new(seeded);
        assert_eq!(seeded.summary, summary.summary);
        assert_eq!(seeded.growth, summary.growth);
        let restored_early = seeded.checked.as_ref().unwrap();
        assert_eq!(
            (&restored_early.summary, &restored_early.growth),
            (&early.0, &early.1)
        );
        assert_eq!(seeded.reads, summary.reads);
        let computations = engine.memo_stats().recomputed.clone();
        assert!(engine.seed_definition_summary(seeded.clone()));
        assert_eq!(engine.memo_stats().recomputed, computations);
        assert!(Arc::ptr_eq(
            &seeded,
            &engine.definition_summary(main).unwrap()
        ));

        engine.mode_definition(main.path()).unwrap();
        assert_eq!(
            engine.recompute_count(main.path(), QueryKind::Check),
            checks + 1
        );
        let changed = source.replace("helper(1)", "helper(2)");
        let file = engine.sources.add("summary.hir", changed).unwrap();
        engine.parse_file(file, &default_context()).unwrap();
        assert!(!engine.seed_definition_summary(seeded));
        let body = engine.definition_summary(main).unwrap();
        assert!(!Arc::ptr_eq(&body, &summary));

        let file = engine
            .sources
            .add(
                "summary.hir",
                "fn helper(x: u8) -> u64 { 1 }\nfn main() -> u64 { helper(2) }\n".into(),
            )
            .unwrap();
        engine.parse_file(file, &default_context()).unwrap();
        assert!(!engine.seed_definition_summary(body));
        engine.definition_summary(main).unwrap();
    });
}

#[test]
fn published_summaries_match_mode_refined_selection_inputs() {
    hirsel_engine::with_compiler_stack(|| {
        for source in [
            format!("{WIDTH}\nfn forward[T](v: T) -> u64 uses {{}} {{ width(v) }}\npub fn main() -> Result[(), ()] {{ let _ = forward(Point(x: 1, y: 2)); Ok(()) }}"),
            "fn outer[n: size](xs: Inline[u8, n]) -> u64 { fn inner[m: size](ys: Inline[u8, m]) -> u64 { let _ = ys; comptime (n * m) }; inner(xs) }\npub fn main() { let xs: Inline[u8, 2] = [1, 2]; let _ = outer(xs) }".into(),
            "fn identity[T](take x: T) -> T { x }\nfn pair[T: copyable](take x: T) -> (T, T) { identity((copy x, copy x)) }\npub fn main() { let _ = pair(1u64); let f = fn(x: u64) -> u64 { identity(x) }; let _ = f(2) }".into(),
        ] {
            let (mut engine, output) = check(&source);
            assert!(output.diagnostics.is_empty(), "{source}\n{:?}", output.diagnostics);
            assert!(super::growth_sites::source_compare(&mut engine).is_empty());
            let checked = engine.checked_definition_summaries().unwrap();
            if source.contains("fn width[T]") {
                assert!(checked.iter().any(|(definition, checked)| *definition != checked.summary.owner),
                    "generated instances retain source lexical owners but distinct query identities");
            }
            for (definition, checked) in checked {
                let CheckedSummary { summary: checked, growth } = checked;
                let path = definition.path();
                let analyzed = engine.mode_definition(path).unwrap();
                let mode = summarize(engine.types(), &analyzed.value.typed);
                // Exact equality is stronger than CT-3 selection equivalence:
                // every frame, call, generic argument, site and growth edge.
                let published = engine.definition_summary(definition).unwrap();
                assert_eq!((&published.summary, &published.growth), (&mode.0, &mode.1), "{path}\n{source}");
                assert_eq!(published.definition, definition);
                let retained=published.checked.as_ref().expect("retain original H input");
                assert_eq!((&retained.summary,&retained.growth),(&checked,&growth));
                engine.publish_definition(definition,hirsel_engine::memo::Retention::Batch).unwrap();
                let before=engine.recompute_count(path,QueryKind::Check);
                assert_eq!(engine.checked_definition_summary(definition).unwrap(),(checked.clone(),growth.clone()));
                assert_eq!(engine.recompute_count(path,QueryKind::Check),before);
            }
        }
    });
}

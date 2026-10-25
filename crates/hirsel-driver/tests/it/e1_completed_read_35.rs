//! The public Unix-read/compute program, with only the recurrence reduced for
//! the reference engine's step budget. Native acceptance uses the full fixture.
use hirsel_driver::*;
use hirsel_engine::*;

#[test]
fn public_completed_read_compute_agrees_on_four_engines() {
    use hirsel_driver::native::*;
    with_compiler_stack(|| {
        let source = include_str!("../fixtures/e1-completed-read-35/main.hir");
        let mut engine = crate::source_std::engine();
        let file = engine
            .sources
            .add("public-read-compute.hir", source.into())
            .unwrap();
        let checked = check_progress(&mut engine, file, &default_context()).unwrap();
        assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
        let net = serde_json::from_value(serde_json::json!({
            "provider": "net.script@2",
            "rules": [{"address": "/fixture/gate.sock", "service": "0", "operation": "unix_connect"}],
            "peers": [{"address": "/fixture/gate.sock", "port": 0, "resolves_to": [], "tls": "plain",
                "connections": [
                    {"open": [{"expect": [71]}, {"send": [71]}, "close"]},
                    {"open": [{"expect": [71]}, {"send": [71]}, "close"]},
                    {"open": [{"expect": [71]}, {"send": [71]}, "close"]},
                    {"open": [{"expect": [71]}, {"send": [71]}, "close"]}
                ]}]
        })).unwrap();
        let fixture = Fixture {
            args: vec!["1".into(), "/fixture/gate.sock".into()],
            net: Some(net),
            migrate: true,
            ..Default::default()
        };
        let entry = entry_path(&default_context());
        let reference = run_observed_snapshot(
            &mut engine,
            &RunConfiguration {
                entry: entry.clone(),
                hash_seed: 35,
                inputs: vec![],
            },
            (&fixture).into(),
        )
        .unwrap();
        let expected = Observation::of_reference(reference.observation.as_ref().unwrap());
        assert_eq!(expected.exit_code, Some(0), "{expected:?}");
        assert!(expected.failures.is_empty(), "{expected:?}");
        let input = engine.export_native_input().unwrap();
        let validated = engine.import_native_input(input).unwrap();
        let options = Options::new(TargetSpec::x86_64_unknown_linux_gnu());
        let configurations = [
            Configuration {
                options: options.clone(),
                engine: Engine::Heap { campaign: false },
            },
            Configuration {
                options: options.clone(),
                engine: Engine::Native {
                    opt: OptLevel::Speed,
                    partition: Partition::Whole,
                    runtime: Runtime::Default,
                },
            },
            Configuration {
                options,
                engine: Engine::Cranelift {
                    partition: Partition::Whole,
                    runtime: Runtime::Default,
                },
            },
        ];
        let scratch = tempfile::Builder::new()
            .prefix("e1-completed-read-35-")
            .tempdir()
            .unwrap();
        let toolchain =
            Toolchain::for_matrix(&scratch.path().join("runtime"), &configurations).unwrap();
        for (configuration, outcome) in differential_in(
            &validated,
            &entry,
            &expected,
            &configurations,
            &toolchain,
            scratch.path(),
            &fixture,
        ) {
            assert_eq!(outcome, Outcome::Agree, "{configuration}: {outcome:?}");
        }
    });
}

#[test]
fn public_read_continuation_covers_active_callers_and_inert_cleanup() {
    use hirsel_driver::native::*;
    with_compiler_stack(|| {
        let mut engine = crate::source_std::engine();
        // A real materialized keyed adapter can reject with pending String
        // ownership. Keep this extra ordinary prefix out of the unchanged
        // public semantic/performance fixture.
        let source = include_str!("../fixtures/e1-completed-read-35/main.hir").replace(
            "  let mut stream = net.unix_connect(n, socket)?",
            "  let mut keys: Set[String] = set.empty()\n  let insert: fn(mut Set[String], take String) -> () = set.insert\n  insert(mut keys, \"key\")\n  let mut stream = net.unix_connect(n, socket)?",
        );
        let file = engine
            .sources
            .add("public-read-segment.hir", source.into())
            .unwrap();
        let checked = check_progress(&mut engine, file, &default_context()).unwrap();
        assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
        let input = engine.export_native_input().unwrap();
        let validated = engine.import_native_input(input).unwrap();
        let program = build(
            &validated,
            &Options::new(TargetSpec::x86_64_unknown_linux_gnu()),
        )
        .unwrap();
        let plan = ahead_plan(&program);
        let roots: Vec<_> = plan.roots.values().filter(|r| r.host.is_some()).collect();
        assert_eq!(roots.len(), 1, "{:?}", plan.refusals);
        let root = roots[0];
        let host = root.host.as_ref().unwrap();
        assert_eq!(host.reads.len(), 1);
        let (&(reader, _), recipe) = host.reads.first_key_value().unwrap();
        assert_eq!(recipe.maximum, 1);
        assert!(!recipe.slots.is_empty());
        assert!(!recipe.values.is_empty());
        assert!(!recipe.loans.is_empty());
        assert!(root.graph.bodies.contains_key(&root.graph.root));
        assert!(root.graph.bodies.contains_key(&reader));
        assert!(root.graph.bodies.len() > host.spines.len());
        assert!(!host.spines[&reader].deferred_releases.is_empty());
        assert!(host.spines.contains_key(&root.graph.root));
        let mut pretransfer = 0;
        let mut pending_take = 0;
        for (id, spine) in &host.spines {
            let caller = program
                .units
                .iter()
                .find(|u| u.unit().header.instance == *id)
                .unwrap()
                .unit();
            for (call, owners) in &spine.suspended {
                assert!(!owners.slots.is_empty());
                if let Some(rejected) = caller.tables.calls[call.index()].pretransfer_failure {
                    pretransfer += 1;
                    for arg in caller.tables.calls[call.index()]
                        .arguments
                        .get(&caller.pools.call_arguments)
                        .unwrap_or(&[])
                    {
                        if arg.passing == hirsel_core_contract::Passing::Take
                            && let hirsel_lir::ir::ArgumentValue::Value(value) = arg.value
                            && !program.table.layouts[caller.types
                                [caller.values[value.index()].ty.index()]
                            .layout
                            .0 as usize]
                                .drop
                                .is_empty()
                        {
                            pending_take += 1;
                            assert!(
                                owners.values.iter().any(|(v, _, _)| *v == value),
                                "a rejected keyed call leaves its actual Take value with the saved caller"
                            );
                        }
                    }
                    for value in caller.tables.failures[rejected.index()]
                        .dispose
                        .get(&caller.pools.values)
                        .unwrap_or(&[])
                    {
                        assert!(
                            owners.values.iter().any(|(v, _, _)| v == value),
                            "the saved caller must retain pretransfer cleanup custody for {value:?}"
                        );
                    }
                }
            }
        }
        assert!(
            pretransfer > 0,
            "host caller proof must exercise rejected-before-transfer custody"
        );
        assert!(
            pending_take > 0,
            "the pretransfer fixture must retain a real disposable Take actual"
        );
        let reader_unit = program
            .units
            .iter()
            .find(|u| u.unit().header.instance == reader)
            .unwrap()
            .unit();
        let work = reader_unit
            .tables
            .calls
            .iter()
            .enumerate()
            .find_map(|(i, c)| match c.callee {
                hirsel_lir::ir::Callee::Direct(sym)
                    if reader_unit.tables.symbols[sym.index()].definition.0.name == "work" =>
                {
                    Some(hirsel_lir::ir::CallId(i as u32))
                }
                _ => None,
            })
            .unwrap();
        assert!(
            host.spines[&reader].reached_calls.contains(&work),
            "work must be reachable privately before any barrier: {:?}",
            host.spines[&reader]
        );
        for &i in host.spines[&reader].deferred_releases.keys() {
            assert!(
                matches!(reader_unit.instructions[i], hirsel_lir::ir::Inst::Release { layout, .. } if layout == recipe.descriptor)
            );
        }
        for backend in [Backend::Llvm, Backend::Cranelift] {
            for partition in [
                Partition::Whole,
                Partition::Singleton,
                Partition::Random(39),
            ] {
                let objects = emit_backend_objects(
                    &program,
                    backend,
                    OptLevel::Default,
                    partition,
                    false,
                    None,
                )
                .unwrap();
                check_ahead_objects(&program, &objects).unwrap();
                let evidence = objects
                    .iter()
                    .filter_map(|o| o.ahead.as_ref())
                    .find(|e| !e.bounds.is_empty())
                    .unwrap_or_else(|| {
                        panic!(
                            "host frame certificate refused for {backend:?}: {:#?}",
                            objects
                                .iter()
                                .map(|o| (&o.imports, &o.ahead))
                                .collect::<Vec<_>>()
                        )
                    });
                let (root_id, _) = plan.roots.iter().find(|(_, r)| r.host.is_some()).unwrap();
                assert!(
                    evidence
                        .bounds
                        .iter()
                        .any(|(id, n)| id == root_id && *n <= 65536)
                );
                for caller in root.graph.bodies.keys() {
                    assert!(evidence.frames.iter().any(|(id, _)| id == caller));
                }
                // Recipe bytes, final callers, and imports must remain bound in
                // every ordinary partitioning mode, including coherent sidecars.
                let code = objects
                    .iter()
                    .position(|o| {
                        o.ahead.as_ref().is_some_and(|e| {
                            hirsel_syntax::ContentHash::of_bytes(&o.bytes) == e.object
                        })
                    })
                    .unwrap();
                if matches!(backend, Backend::Llvm) {
                    use object::{Object, ObjectSection, ObjectSymbol};
                    let bytes = &objects[code].bytes;
                    let file = object::File::parse(bytes.as_slice()).unwrap();
                    let helper = file
                        .symbols()
                        .find(|symbol| {
                            symbol.kind() == object::SymbolKind::Text
                                && symbol
                                    .name()
                                    .is_ok_and(|n| n.starts_with("hirsel.release."))
                        })
                        .expect("ordinary-prefix cleanup retains its local function identity");
                    assert!(helper.is_local());
                    assert!(
                        !objects[code]
                            .exports
                            .iter()
                            .any(|s| s.name == helper.name().unwrap())
                    );
                    let frames = hirsel_lir::object::llvm_stack_sizes(bytes).unwrap();
                    assert!(frames[helper.name().unwrap()] >= 8);
                    // A local helper's record still requires a real function symbol.
                    // Retaining its identity never admits anonymous extents.
                    let (symbols_at, _) = file
                        .section_by_name(".symtab")
                        .unwrap()
                        .file_range()
                        .unwrap();
                    let info_at = symbols_at as usize + helper.index().0 * 24 + 4;
                    let mut malformed = bytes.clone();
                    malformed[info_at] &= 0xf0;
                    assert!(hirsel_lir::object::llvm_stack_sizes(&malformed).is_err());
                }
                let meta = objects
                    .iter()
                    .position(|o| {
                        o.exports
                            .iter()
                            .any(|s| s.name.starts_with("hirsel_ahead_"))
                    })
                    .unwrap();
                for mutation in 0..5 {
                    let mut bad = objects.clone();
                    match mutation {
                        0 => bad[meta].bytes[0] ^= 1,
                        1 => {
                            bad.remove(code);
                        }
                        2 => {
                            bad[code].ahead.as_mut().unwrap().frames.pop();
                        }
                        3 => {
                            bad[meta].ahead.as_mut().unwrap().plan =
                                hirsel_syntax::ContentHash::of_bytes(b"stale selected-read recipe");
                        }
                        _ => {
                            let old = b"hirsel_rt_v2_host_enter\0";
                            let at = bad[code]
                                .bytes
                                .windows(old.len())
                                .position(|w| w == old)
                                .unwrap();
                            bad[code].bytes[at + 13] = b'x';
                            bad[code]
                                .imports
                                .iter_mut()
                                .find(|s| s.name == "hirsel_rt_v2_host_enter")
                                .unwrap()
                                .name = "hirsel_rt_v2_xost_enter".into();
                            let mut changed = bad[code].ahead.clone().unwrap();
                            changed.object = hirsel_syntax::ContentHash::of_bytes(&bad[code].bytes);
                            bad[code].ahead = Some(changed.clone());
                            bad[meta].ahead = Some(changed);
                        }
                    }
                    assert!(
                        check_ahead_objects(&program, &bad).is_err(),
                        "{backend:?}/{partition:?}/{mutation}"
                    );
                }
            }
        }
    });
}

#[test]
fn ordinary_prefix_thunk_does_not_authorize_a_post_read_private_site() {
    use hirsel_driver::native::*;
    use hirsel_lir::ir::{Callee, Inst};
    with_compiler_stack(|| {
        let mut engine = crate::source_std::engine();
        let source = include_str!("../fixtures/e1-completed-read-35/main.hir")
            .replace(
                "  let mut stream = net.unix_connect(n, socket)?",
                "  let mut keys: Set[String] = set.empty()\n  let insert: fn(mut Set[String], take String) -> () = set.insert\n  insert(mut keys, \"key\")\n  let mut stream = net.unix_connect(n, socket)?",
            )
            .replace(
                "  failure.check(received == 1u64, Arithmetic())",
                "  let tail_insert: fn(mut Set[String], take String) -> () = set.insert\n  tail_insert(mut keys, \"tail\")\n  failure.check(received == 1u64, Arithmetic())",
            );
        // Return the prefix callable through the definition's result.
        // A direct-only use lets LLVM remove its address after devirtualizing
        // the call, leaving no machine import for the forged-thunk control.
        let source = source
            .replace(
                "-> Result[u64, NetError] {",
                "-> Result[(u64, fn(mut Set[String], take String) -> ()), NetError] {",
            )
            .replace("Ok(work(seed, count))", "Ok((work(seed, count), insert))")
            .replace("Ok(value) => Ok(value)", "Ok((value, _)) => Ok(value)");
        let file = engine
            .sources
            .add("prefix-thunk-barrier.hir", source.into())
            .unwrap();
        let checked = check_progress(&mut engine, file, &default_context()).unwrap();
        assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
        let input = engine.export_native_input().unwrap();
        let validated = engine.import_native_input(input).unwrap();
        let program = build(
            &validated,
            &Options::new(TargetSpec::x86_64_unknown_linux_gnu()),
        )
        .unwrap();
        let plan = ahead_plan(&program);
        let root = plan.roots.values().find(|r| r.host.is_some()).unwrap();
        let host = root.host.as_ref().unwrap();
        let (&(reader, _), _) = host.reads.first_key_value().unwrap();
        let unit = program
            .units
            .iter()
            .find(|u| u.unit().header.instance == reader)
            .unwrap()
            .unit();
        let spine = &host.spines[&reader];
        let refs: Vec<_> = unit
            .instructions
            .iter()
            .enumerate()
            .filter_map(|(i, inst)| {
                let Inst::FunctionRef { symbol, .. } = inst else {
                    return None;
                };
                let symbol = &unit.tables.symbols[symbol.index()];
                (symbol.definition.to_string().contains("keyed_set_insert")).then_some((i, symbol))
            })
            .collect();
        assert_eq!(refs.len(), 2, "both actual sites must survive lowering");
        assert_eq!(refs[0].1, refs[1].1, "same ordinary callable identity");
        for (i, _) in &refs {
            assert!(!spine.private_instructions[*i]);
        }
        assert!(
            spine.barriers.contains(&refs[1].0),
            "post-read callable construction must stop private execution"
        );
        let work = unit
            .tables
            .calls
            .iter()
            .enumerate()
            .find_map(|(i, call)| {
                let Callee::Direct(symbol) = call.callee else {
                    return None;
                };
                unit.tables.symbols[symbol.index()]
                    .definition
                    .to_string()
                    .contains("work")
                    .then_some(hirsel_lir::ir::CallId(i as u32))
            })
            .unwrap();
        assert!(
            !spine.reached_calls.contains(&work),
            "prefix thunk permission cannot cross the selected-tail barrier"
        );
        let thunk = hirsel_lir::emission::function_thunk_name(
            &refs[0].1.definition,
            refs[0].1.instance.as_ref(),
        );
        for backend in [Backend::Llvm, Backend::Cranelift] {
            let objects = emit_backend_objects(
                &program,
                backend,
                OptLevel::Default,
                Partition::Whole,
                false,
                None,
            )
            .unwrap();
            check_ahead_objects(&program, &objects).unwrap();
            let code = objects
                .iter()
                .position(|o| {
                    o.ahead
                        .as_ref()
                        .is_some_and(|e| hirsel_syntax::ContentHash::of_bytes(&o.bytes) == e.object)
                })
                .unwrap();
            assert!(!objects[code].ahead.as_ref().unwrap().bounds.is_empty());
            assert!(
                objects[code].imports.iter().any(|s| s.name == thunk),
                "{backend:?}: missing {thunk} among {:?}",
                objects[code].imports
            );
            // A different machine thunk is still unknown even when a valid
            // ordinary-prefix thunk and coherent object sidecars are present.
            let mut bad = objects.clone();
            let mut unknown = thunk.clone().into_bytes();
            unknown[3] = b'Q';
            let at = bad[code]
                .bytes
                .windows(thunk.len())
                .position(|w| w == thunk.as_bytes())
                .unwrap();
            bad[code].bytes[at..at + thunk.len()].copy_from_slice(&unknown);
            bad[code]
                .imports
                .iter_mut()
                .find(|s| s.name == thunk)
                .unwrap()
                .name = String::from_utf8(unknown).unwrap();
            let mut evidence = bad[code].ahead.clone().unwrap();
            evidence.object = hirsel_syntax::ContentHash::of_bytes(&bad[code].bytes);
            for object in &mut bad {
                if object.ahead.is_some() {
                    object.ahead = Some(evidence.clone());
                }
            }
            assert!(
                check_ahead_objects(&program, &bad).is_err(),
                "unknown thunk must not inherit prefix permission"
            );
        }
    });
}

//! Actual compiler controls for portable local native products. These tests
//! exercise independent Value/Ready validation, not a producer-only oracle.
use crate::rc_support;
use hirsel_lir::products::{Binding, LayoutClosure, UnitBody};
use hirsel_lir::stage::{Stage, Value};
use hirsel_lir::validate::{Inputs, UnitInputs};

fn roundtrip<S: Stage>(
    units: Vec<hirsel_lir::stage::ValidatedUnit<S>>,
    inputs: &Inputs<'_>,
) -> Vec<String> {
    let products: Vec<_> = units
        .iter()
        .map(|u| {
            let (closure, body) = LayoutClosure::capture(u, inputs, &[]).unwrap();
            closure.check(inputs.target).unwrap();
            let closure: LayoutClosure =
                hirsel_codec::from_text(&hirsel_codec::to_text(&closure)).unwrap();
            let body: UnitBody = hirsel_codec::from_text(&hirsel_codec::to_text(&body)).unwrap();
            (closure, body)
        })
        .collect();
    let closures: Vec<_> = products.iter().map(|(c, _)| c).collect();
    let binding = Binding::new(inputs.target, &closures).unwrap();
    let current = Inputs {
        memo: Default::default(),
        target: inputs.target,
        layouts: &binding.table,
        manifest: inputs.manifest,
        operations: inputs.operations,
        discharge: inputs.discharge,
    };
    for (_, body) in &products {
        let raw = binding.materialize::<S>(body).unwrap();
        let checked = hirsel_lir::validate::validate(raw, &current).unwrap();
        let (_, body_again) = LayoutClosure::capture(&checked, &current, &[]).unwrap();
        assert_eq!(
            *body, body_again,
            "canonical body must survive table relocation"
        );
        // The complete content-named body is the contract; numeric aliases
        // in the original program table may legitimately merge when binding.
        assert!(UnitInputs::from(&current).digest(checked.unit()).is_ok());
    }
    // The same closure repeated must preserve cyclic component multiplicity
    // and leave bindings/content keys unchanged.
    let duplicate: Vec<_> = closures.iter().chain(&closures).copied().collect();
    assert_eq!(
        Binding::new(inputs.target, &duplicate).unwrap().table,
        binding.table
    );
    products
        .iter()
        .map(|(c, b)| format!("{}\n{}", hirsel_codec::to_text(c), hirsel_codec::to_text(b)))
        .collect()
}

#[test]
fn native_products_relocate_recursive_layouts_closures_and_finalizers() {
    for source in [
        "resource type Token { n:u64 }\ndrop(x) { () }\nfn main() { let t=Token(n:1u64); }",
        "type Node = End | More(next: box Node)\nfn make()->box Node { box More(next:box End()) }\nfn main() { let n=make(); }",
        "type Captured { n:u64 }\nfn compute(x:Captured)->u64 { let f=fn()->u64 { x.n }; f() }\nfn main()->u64 { compute(Captured(n:3u64)) }",
        "type Item { n:u64 }\nfn first(a:Arena,n:u64)->(box Item) in a { if n==0u64 { box Item(n:6u64) } else { second(a,n-1u64) } }\nfn second(a:Arena,n:u64)->(box Item) in a { first(a,n) }\nfn main() { arena.with(fn(a:Arena) { let x=second(a,2u64); }); }",
    ] {
        let program = rc_support::compile(source);
        let target = hirsel_lir::TargetSpec::x86_64_unknown_linux_gnu();
        let lowered = hirsel_rc::import::lower(&program, &target).unwrap();
        assert_eq!(lowered.unit_keys.len(), lowered.units.len());
        for (unit, key) in lowered.units.iter().zip(&lowered.unit_keys) {
            assert_eq!(unit.header.instance, key.digest());
            assert_eq!(&unit.header.definition, key.definition());
        }
        let manifest = rc_support::manifest(&program);
        let operations = program.operation_registry();
        let inputs = Inputs {
            memo: Default::default(),
            target: &target,
            layouts: &lowered.table,
            manifest: &manifest,
            operations: &operations,
            discharge: Some(&lowered.native_discharge),
        };
        let units = lowered
            .units
            .iter()
            .cloned()
            .map(|u| hirsel_lir::validate::validate_value(u, &inputs).unwrap())
            .collect();
        roundtrip::<Value>(units, &inputs);
        let ready = hirsel_driver::native::build_program(
            &program,
            manifest.clone(),
            &hirsel_driver::native::Options::new(target.clone()),
        )
        .unwrap();
        let inputs = Inputs {
            memo: Default::default(),
            target: &target,
            layouts: &ready.table,
            manifest: &manifest,
            operations: &operations,
            discharge: Some(&ready.discharged),
        };
        roundtrip(ready.units.clone(), &inputs);
    }
}

#[test]
fn native_products_reject_changed_input_rows_and_corrupt_graph_keys() {
    let program = rc_support::compile("fn main()->u64 { 7u64 }");
    let target = hirsel_lir::TargetSpec::x86_64_unknown_linux_gnu();
    let lowered = hirsel_rc::import::lower(&program, &target).unwrap();
    let manifest = rc_support::manifest(&program);
    let operations = program.operation_registry();
    let inputs = Inputs {
        memo: Default::default(),
        target: &target,
        layouts: &lowered.table,
        manifest: &manifest,
        operations: &operations,
        discharge: None,
    };
    let checked = hirsel_lir::validate::validate_value(lowered.units[0].clone(), &inputs).unwrap();
    let (mut closure, mut body) = LayoutClosure::capture(&checked, &inputs, &[]).unwrap();
    closure.layouts[0] = hirsel_syntax::ContentHash::default();
    assert!(closure.check(&target).is_err());
    assert!(Binding::new(&target, &[&closure]).is_err());
    let (closure, _) = LayoutClosure::capture(&checked, &inputs, &[]).unwrap();
    let binding = Binding::new(&target, &[&closure]).unwrap();
    body.abis[0] = hirsel_syntax::ContentHash::default();
    assert!(binding.materialize::<Value>(&body).is_err());
    let mut foreign_table = lowered.table.clone();
    foreign_table.fn_abis[checked.unit().header.abi.0 as usize].result_arena = Some(0);
    let changed = Inputs {
        memo: Default::default(),
        layouts: &foreign_table,
        ..inputs
    };
    assert!(LayoutClosure::capture(&checked, &changed, &[]).is_err());
}

#[test]
fn selected_implementation_keys_keep_full_inputs_and_existing_symbol_domains() {
    let types = hirsel_types::TypeInterner::new(None);
    let base = hirsel_layout::instance::InstanceKey::new(
        &rc_support::path("key"),
        0,
        &[],
        &types,
        None,
        None,
    )
    .unwrap();
    let derived = base.derived("selected-test", &[b"a", b"bc"]);
    assert_ne!(derived, base.derived("selected-test", &[b"ab", b"c"]));
    assert_ne!(derived, base.derived("other-test", &[b"a", b"bc"]));
    assert_eq!(
        derived.digest(),
        hirsel_syntax::ContentHash::of_parts(
            "selected-test",
            [base.digest().0.as_slice(), b"a", b"bc"]
        )
    );
    assert_eq!(derived.definition(), base.definition());
    assert_eq!(derived.body(), base.body());
}

#[test]
fn arena_query_graph_settles_local_equations_and_reuses_unchanged_components() {
    use hirsel_driver::native::queries::arena::{
        ArenaQueryCounts, EQUATIONS_DOMAIN, EquationProduct,
    };
    use hirsel_driver::native::queries::graph::{ArenaNode, settle_arena_queries};
    use hirsel_driver::native::queries::{InstanceAddress, extract_local_arena};
    use hirsel_engine::store::{MemStore, ResultStore};
    use hirsel_syntax::ContentHash;
    let source = "type Item { n:u64 }\nfn first(a:Arena,n:u64)->(box Item) in a {if n==0u64 {box Item(n:6u64)} else {second(a,n-1u64)}}\nfn second(a:Arena,n:u64)->(box Item) in a {first(a,n)}\nfn wrapper(a:Arena)->(box Item) in a {second(a,2u64)}\nfn main(){arena.with(fn(a:Arena){let x=wrapper(a);});}";
    let program = rc_support::compile(source);
    let target = hirsel_lir::TargetSpec::x86_64_unknown_linux_gnu();
    let lowered = hirsel_rc::import::lower(&program, &target).unwrap();
    let manifest = rc_support::manifest(&program);
    let operations = program.operation_registry();
    let inputs = Inputs {
        memo: Default::default(),
        target: &target,
        layouts: &lowered.table,
        manifest: &manifest,
        operations: &operations,
        discharge: Some(&lowered.native_discharge),
    };
    let store = MemStore::default();
    let mut nodes = Vec::new();
    for (unit, key) in lowered.units.iter().zip(&lowered.unit_keys) {
        let checked = hirsel_lir::validate::validate_value(unit.clone(), &inputs).unwrap();
        let owner = InstanceAddress {
            instance: key.clone(),
            target_abi: target.digest(),
            policy: ContentHash::of_bytes(b"query-test-policy"),
        };
        let equations = extract_local_arena(&owner, &checked, &lowered.table).unwrap();
        let reference = store
            .put_content(EQUATIONS_DOMAIN, &serde_json::to_vec(&equations).unwrap())
            .unwrap();
        nodes.push(ArenaNode {
            equations: EquationProduct { owner, reference },
            callees: equations.callees().to_vec(),
        });
    }
    nodes.sort_by(|a, b| a.equations.owner.canonical_cmp(&b.equations.owner));
    let mut counts = ArenaQueryCounts::default();
    let first = settle_arena_queries(&store, &nodes, &[], 0, &mut counts).unwrap();
    assert!(counts.misses > 0);
    assert_eq!(counts.hits, 0);
    for name in ["first", "second", "wrapper"] {
        let xs = first
            .chosen
            .iter()
            .flat_map(|s| &s.candidates)
            .find(|(a, _)| a.instance.definition().0.name == name)
            .unwrap();
        assert_eq!(xs.1, [0], "{name}");
    }
    let misses = counts.misses;
    let next = settle_arena_queries(&store, &nodes, &[], 0, &mut counts).unwrap();
    assert_eq!(counts.misses, misses);
    assert_eq!(counts.hits, misses);
    assert_eq!(
        first
            .chosen
            .iter()
            .map(|s| s.product.record)
            .collect::<Vec<_>>(),
        next.chosen
            .iter()
            .map(|s| s.product.record)
            .collect::<Vec<_>>()
    );
    let mut missing = nodes.pop().unwrap();
    missing.callees.push((
        rc_support::path("missing-callee"),
        ContentHash::of_bytes(b"missing"),
    ));
    missing.callees.sort();
    missing.callees.dedup();
    nodes.push(missing);
    assert!(settle_arena_queries(&store, &nodes, &[], 0, &mut counts).is_err());
}

#[test]
fn native_checked_linkage_survives_body_release_and_layout_relocation() {
    use hirsel_artifact_store::Memory;
    use hirsel_lir::validate::{NativeCache, NativeEnvironment, check_arena_scoped_links};
    use hirsel_syntax::ContentHash;
    use std::sync::Arc;
    let program = rc_support::compile(
        "type Item {n:u64}\nfn leaf(a:Arena)->(box Item) in a {box Item(n:1u64)}\nfn wrap(a:Arena)->(box Item) in a {leaf(a)}\nfn apply(f:fn()->u64)->u64{f()}\nfn answer()->u64{9u64}\nfn main()->u64 {arena.with(fn(a:Arena){let x=wrap(a);});apply(answer)}",
    );
    let target = hirsel_lir::TargetSpec::x86_64_unknown_linux_gnu();
    let lowered = hirsel_rc::import::lower(&program, &target).unwrap();
    hirsel_lir::validate::check_result_arena_calls(&lowered.units, &lowered.table).unwrap();
    hirsel_lir::validate::check_scoped_links(&lowered.units, &lowered.table).unwrap();
    let manifest = rc_support::manifest(&program);
    let operations = program.operation_registry();
    let inputs = Inputs {
        memo: Default::default(),
        target: &target,
        layouts: &lowered.table,
        operations: &operations,
        manifest: &manifest,
        discharge: Some(&lowered.native_discharge),
    };
    let contents = Arc::new(Memory::default());
    let cache = NativeCache::new(contents.clone(), None);
    let policy = ContentHash::of_bytes(b"checked linkage policy");
    let mut outputs = Vec::new();
    let mut products = Vec::new();
    for (index, raw) in lowered.units.iter().cloned().enumerate() {
        let subject = ContentHash::of_bytes(&(index as u64).to_le_bytes());
        let (checked, output) = cache
            .validate_and_publish(
                subject,
                policy,
                raw,
                &inputs,
                &[],
                b"ordinary producer metadata",
            )
            .unwrap();
        let output = output.unwrap();
        let (closure, body) = cache.read(&output, &target).unwrap();
        assert_eq!(
            output.linkage().definition(),
            &checked.unit().header.definition
        );
        products.push((closure, body));
        outputs.push((subject, output));
    }
    let expected = outputs
        .iter()
        .map(|(_, output)| output.linkage())
        .collect::<Vec<_>>();
    check_arena_scoped_links(&expected).unwrap();
    let binding = Binding::new(
        &target,
        &products
            .iter()
            .map(|(closure, _)| closure)
            .collect::<Vec<_>>(),
    )
    .unwrap();
    let rebound_inputs = Inputs {
        memo: Default::default(),
        layouts: &binding.table,
        ..inputs
    };
    let mut relocated = Vec::new();
    for (index, (_, body)) in products.iter().enumerate() {
        let raw = binding.materialize::<Value>(body).unwrap();
        let subject = ContentHash::of_parts(
            "relocated checked linkage",
            [(index as u64).to_le_bytes().as_slice()],
        );
        let (_, output) = cache
            .validate_and_publish(subject, policy, raw, &rebound_inputs, &[], b"relocated")
            .unwrap();
        let observed = output.unwrap().linkage();
        assert_eq!(observed.abi(), expected[index].abi());
        relocated.push(observed);
    }
    check_arena_scoped_links(&relocated).unwrap();
    drop(products);
    drop(binding);
    drop(lowered);
    let environment = NativeEnvironment {
        target: &target,
        manifest: &manifest,
        operations: &operations,
        discharge: None,
    };
    let trace = hirsel_lir::construction::ValueTrace::default();
    let _observe = trace.enter();
    let retained = outputs
        .iter()
        .map(|(subject, _)| {
            cache
                .lookup(
                    *subject,
                    policy,
                    hirsel_lir::stage::StageKind::Value,
                    &environment,
                )
                .unwrap()
                .linkage()
        })
        .collect::<Vec<_>>();
    check_arena_scoped_links(&retained).unwrap();
    assert_eq!(
        trace.totals(),
        hirsel_lir::construction::ValueCounts::default()
    );
    let unassociated = NativeCache::new(contents, None);
    assert!(
        unassociated
            .lookup(
                outputs[0].0,
                policy,
                hirsel_lir::stage::StageKind::Value,
                &environment
            )
            .is_none()
    );
}

#[test]
fn local_arena_binding_and_ready_match_global_results_without_other_value_bodies() {
    use hirsel_driver::native::{
        Options,
        queries::{
            InstanceAddress,
            arena::{ArenaQueryCounts, EQUATIONS_DOMAIN, EquationProduct},
            extract_local_arena,
            graph::{ArenaNode, settle_arena_queries},
            ready::{EMISSION_CONTEXT_DOMAIN, ReadyMiss, ReadyQueries, ReadySourceFacts},
        },
    };
    use hirsel_engine::store::{Action, Domain, MemStore, ResultStore};
    use hirsel_lir::validate::{NativeEnvironment, check_arena_scoped_links};
    use hirsel_syntax::ContentHash;
    use std::{collections::BTreeMap, sync::Arc};
    const BEFORE: &str = "type Item { n: u64 }\nfn first(a: Arena, n: u64) -> (box Item) in a {\n  if n == 0u64 { box Item(n:6u64) }\n  else { second(a,n-1u64) }\n}\nfn second(a: Arena, n: u64) -> (box Item) in a { first(a,n) }\nfn wrapper(a: Arena) -> (box Item) in a { second(a,2u64) }\nfn main() { arena.with(fn(a: Arena) { let item = wrapper(a); }); }\n";
    const AFTER: &str = "type Item { n: u64 }\nfn first(a: Arena, n: u64) -> (box Item) in a {\n  if n == 0u64 { let h: (box Item) in heap = box Item(n:6u64); h }\n  else { second(a,n-1u64) }\n}\nfn second(a: Arena, n: u64) -> (box Item) in a { first(a,n) }\nfn wrapper(a: Arena) -> (box Item) in a { second(a,2u64) }\nfn main() { arena.with(fn(a: Arena) { let item = wrapper(a); }); }\n";
    let target = hirsel_lir::TargetSpec::x86_64_unknown_linux_gnu();
    let options = Options::new(target.clone());
    let store = Arc::new(MemStore::default());
    let mut ready_queries = ReadyQueries::new(store.clone());
    let policy = ContentHash::of_bytes(b"local arena Ready equivalence");
    let emission = store.put_content(EMISSION_CONTEXT_DOMAIN, b"{}").unwrap();
    let body_domain = Domain {
        kind: 0x4e00_001c,
        schema: 1,
    };
    let mut earlier: BTreeMap<
        hirsel_rc::arena_credit::equations::Function,
        (
            hirsel_engine::store::Reference,
            hirsel_engine::store::Reference,
        ),
    > = BTreeMap::new();
    for (round, source) in [BEFORE, AFTER].into_iter().enumerate() {
        let mut program = rc_support::compile(source);
        let lowered = hirsel_rc::import::lower(&program, &target).unwrap();
        let expected = hirsel_driver::native::build_program(
            &program,
            rc_support::manifest(&program),
            &options,
        )
        .unwrap();
        let operations = program.operation_registry();
        let manifest = rc_support::manifest(&program);
        let expected_inputs = Inputs {
            memo: Default::default(),
            target: &target,
            layouts: &expected.table,
            operations: &operations,
            manifest: &manifest,
            discharge: None,
        };
        let expected_bodies = expected
            .units
            .iter()
            .map(|unit| {
                let (_, body) = LayoutClosure::capture(unit, &expected_inputs, &[]).unwrap();
                (
                    (
                        unit.unit().header.definition.clone(),
                        unit.unit().header.instance,
                    ),
                    body,
                )
            })
            .collect::<BTreeMap<_, _>>();
        drop(expected);
        let mut base_table = lowered.table.clone();
        for abi in &mut base_table.fn_abis {
            abi.result_arena = None;
        }
        let base_inputs = Inputs {
            memo: Default::default(),
            target: &target,
            layouts: &base_table,
            operations: &operations,
            manifest: &manifest,
            discharge: None,
        };
        let finalizers = hirsel_rc::cleanup::Finalizers::planned(&lowered.instances, &base_table);
        let keys = hirsel_lir::LayoutKeys::new(&base_table);
        let introduced = hirsel_lir::LayoutIndex::new(&base_table)
            .introduced()
            .map(|id| keys.key(id).0)
            .collect::<Vec<_>>();
        let mut products = Vec::new();
        let mut nodes = Vec::new();
        let mut sources = Vec::new();
        let mut owners = Vec::new();
        let mut header_abis = BTreeMap::new();
        let mut reached = Vec::new();
        for (index, raw) in lowered.units.iter().cloned().enumerate() {
            let owner = InstanceAddress {
                instance: lowered.unit_keys[index].clone(),
                target_abi: target.digest(),
                policy,
            };
            let unit = hirsel_lir::validate::validate_value(raw, &base_inputs).unwrap();
            let local = extract_local_arena(&owner, &unit, &base_table).unwrap();
            let equations = store
                .put_content(EQUATIONS_DOMAIN, &serde_json::to_vec(&local).unwrap())
                .unwrap();
            nodes.push(ArenaNode {
                equations: EquationProduct {
                    owner: owner.clone(),
                    reference: equations,
                },
                callees: local.callees().to_vec(),
            });
            let cleanups = finalizers
                .reached(unit.unit(), &base_table)
                .into_iter()
                .map(|(layout, finalizer)| {
                    (
                        keys.key(layout).0,
                        finalizer.clone(),
                        keys.fn_abi(&base_table.fn_abis[finalizer.abi.0 as usize]),
                    )
                })
                .collect::<Vec<_>>();
            let extra = cleanups
                .iter()
                .map(|(_, finalizer, _)| finalizer.abi)
                .collect::<Vec<_>>();
            let (closure, body) = LayoutClosure::capture(&unit, &base_inputs, &extra).unwrap();
            let reference = store
                .put_content(body_domain, hirsel_codec::to_text(&body).as_bytes())
                .unwrap();
            let function = (
                unit.unit().header.definition.clone(),
                unit.unit().header.instance,
            );
            header_abis.insert(
                function,
                (
                    products.len(),
                    keys.fn_abi(&base_table.fn_abis[unit.unit().header.abi.0 as usize]),
                ),
            );
            products.push((closure, body, reference));
            reached.push(cleanups);
            sources.push(ReadySourceFacts::for_instance(
                &lowered.instances,
                lowered.unit_origins[index],
            ));
            owners.push(owner);
        }
        nodes.sort_by(|a, b| a.equations.owner.canonical_cmp(&b.equations.owner));
        let settled = settle_arena_queries(
            store.as_ref(),
            &nodes,
            &[],
            0,
            &mut ArenaQueryCounts::default(),
        )
        .unwrap();
        let claims = settled
            .chosen
            .iter()
            .flat_map(|summary| {
                summary.candidates.iter().map(move |(owner, candidates)| {
                    (
                        (owner.instance.definition().clone(), owner.instance.digest()),
                        (candidates.first().copied(), summary.product.record),
                    )
                })
            })
            .collect::<BTreeMap<_, _>>();
        for name in ["first", "second", "wrapper"] {
            let (_, (claim, _)) = claims
                .iter()
                .find(|((path, _), _)| path.0.name == name)
                .unwrap();
            assert_eq!(*claim, if round == 0 { Some(0) } else { None });
        }
        let definitions = program
            .definitions
            .values()
            .map(Arc::downgrade)
            .collect::<Vec<_>>();
        program.definitions.clear();
        drop(lowered);
        drop(base_table);
        assert!(definitions.iter().all(|body| body.upgrade().is_none()));
        let environment = NativeEnvironment {
            target: &target,
            manifest: &manifest,
            operations: &operations,
            discharge: None,
        };
        let mut links = Vec::new();
        for (index, owner) in owners.iter().enumerate() {
            let function = (owner.instance.definition().clone(), owner.instance.digest());
            let node = nodes
                .iter()
                .find(|node| node.equations.owner == *owner)
                .unwrap();
            let mut reads = vec![claims[&function].1];
            reads.extend(node.callees.iter().map(|callee| claims[callee].1));
            let action_bytes = serde_json::to_vec(&(products[index].2, &reads)).unwrap();
            let action = Action(ContentHash::of_parts(
                "local arena Ready test action",
                [action_bytes.as_slice()],
            ));
            let trace = hirsel_lir::construction::ValueTrace::default();
            let _observe = trace.enter();
            let ready = if let Some(ready) = ready_queries.lookup(owner, action, &environment) {
                ready
            } else {
                let mut closures = vec![&products[index].0];
                for callee in &node.callees {
                    closures.push(&products[header_abis[callee].0].0);
                }
                let mut binding = Binding::new(&target, &closures).unwrap();
                let bound_keys = hirsel_lir::LayoutKeys::new(&binding.table);
                assert_eq!(
                    hirsel_lir::LayoutIndex::new(&binding.table)
                        .introduced()
                        .map(|id| bound_keys.key(id).0)
                        .collect::<Vec<_>>(),
                    introduced,
                    "binding must preserve the actual synthetic layout selections"
                );
                let mut unit = binding.materialize::<Value>(&products[index].1).unwrap();
                let mut callees = Vec::new();
                for callee in &node.callees {
                    let base = binding.abi(header_abis[callee].1).unwrap();
                    let mut abi = binding.table.fn_abis[base.0 as usize].clone();
                    abi.result_arena = claims[callee].0;
                    let id = hirsel_lir::FnAbiId(binding.table.fn_abis.len() as u32);
                    binding.table.fn_abis.push(abi);
                    callees.push((callee.clone(), id));
                }
                hirsel_rc::arena_credit::bind_local(
                    &mut unit,
                    &mut binding.table,
                    claims[&function].0,
                    &callees,
                )
                .unwrap();
                let finalizers = hirsel_rc::cleanup::Finalizers(
                    reached[index]
                        .iter()
                        .map(|(layout, finalizer, abi)| {
                            let mut finalizer = finalizer.clone();
                            finalizer.abi = binding.abi(*abi).unwrap();
                            (binding.layout(*layout).unwrap(), finalizer)
                        })
                        .collect(),
                );
                let inputs = Inputs {
                    memo: Default::default(),
                    target: &target,
                    layouts: &binding.table,
                    operations: &operations,
                    manifest: &manifest,
                    discharge: None,
                };
                let (checked, ready) = ready_queries
                    .publish_from_value(
                        owner,
                        action,
                        ReadyMiss {
                            value: unit,
                            options: &options,
                            source: &sources[index],
                            finalizers: &finalizers,
                            inputs: &inputs,
                            additional_abis: &[],
                            statics: &[],
                            emission_context: emission,
                            debug: true,
                            optimizer_reads: vec![],
                        },
                    )
                    .unwrap();
                let (_, body) = LayoutClosure::capture(&checked, &inputs, &[]).unwrap();
                assert_eq!(
                    body, expected_bodies[&function],
                    "canonical Ready {}",
                    function.0.0.name
                );
                ready
            };
            assert!(
                trace
                    .snapshot()
                    .keys()
                    .all(|observed| observed == &function),
                "another Value body was materialized"
            );
            if round == 1 && ["second", "wrapper"].contains(&function.0.0.name.as_str()) {
                assert_eq!(
                    products[index].2, earlier[&function].0,
                    "unchanged base Value"
                );
                assert_ne!(
                    ready.product.record, earlier[&function].1,
                    "changed arena facts must invalidate Ready"
                );
            }
            earlier.insert(function, (products[index].2, ready.product.record));
            drop(_observe);
            let trace = hirsel_lir::construction::ValueTrace::default();
            let _observe = trace.enter();
            assert_eq!(
                ready_queries
                    .lookup(owner, action, &environment)
                    .unwrap()
                    .product,
                ready.product
            );
            links.push(ready_queries.linkage(owner, action, &environment).unwrap());
            assert_eq!(
                trace.totals(),
                hirsel_lir::construction::ValueCounts::default()
            );
        }
        check_arena_scoped_links(&links).unwrap();
    }
}

use crate::rc_support;
use hirsel_lir::validate::Inputs;

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
    let operations = lowered.operation_registry(&program);
    let inputs = Inputs {
        memo: Default::default(),
        target: &target,
        layouts: &lowered.table,
        manifest: &manifest,
        operations: &operations,
        discharge: Some(&lowered.native_discharge),
    };
    let budget = hirsel_engine::runtime::ProcessBudget::new(64 << 20);
    let store = hirsel_engine::store::RequestStore::new(
        std::sync::Arc::new(MemStore::default()),
        budget.reserve(64 << 20).unwrap(),
    );
    let mut nodes = Vec::new();
    for (index, unit) in lowered.units.iter().enumerate() {
        let key = &lowered.unit_keys[index];
        assert_eq!(unit.header.instance, key.digest());
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
            .map(|s| s
                .product
                .archive(hirsel_engine::runtime::ArchiveAdmission::Retained)
                .unwrap()
                .reference())
            .collect::<Vec<_>>(),
        next.chosen
            .iter()
            .map(|s| s
                .product
                .archive(hirsel_engine::runtime::ArchiveAdmission::Retained)
                .unwrap()
                .reference())
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

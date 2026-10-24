//! Explicit source discovery shares the batch collector; declarations carry no proof.
use crate::rc_support as support;
use hirsel_layout::layout::LayoutBuilder;
use hirsel_rc::instance::{DiscoveryContext, InstanceRole, discover_instance, seed_instances};
use std::collections::BTreeSet;

#[test]
fn explicit_source_discovery_matches_batch_keys_without_a_program_body_collection() {
    let mut program = support::compile(
        "resource type R {n:u64} drop(x) {let _=x.n}\nfn keep[T](take x:T)->T {x}\nfn recur(n:u64)->u64 {if n==0u64 {0u64}else{recur(n-1u64)}}\npub fn main()->u64 {let r=R(n:1u64);let f=fn(x:u64)->u64{keep(x)};f(recur(2u64))}",
    );
    let target = support::target();
    let owners =
        hirsel_rc::library::owners(&program.types, &program.signatures).over(&program.owners);
    let mut batch_layouts = LayoutBuilder::new(&target, &owners);
    let batch = hirsel_rc::instance::expand(&program, &mut batch_layouts).unwrap();
    let expected = batch
        .ids()
        .map(|id| batch.view(id).key.encoding().to_vec())
        .collect::<BTreeSet<_>>();
    let context = DiscoveryContext::of(&program).unwrap();
    let bodies = std::mem::take(&mut program.definitions);
    let owners =
        hirsel_rc::library::owners(&program.types, &program.signatures).over(&program.owners);
    let mut layouts = LayoutBuilder::new(&target, &owners);
    let (mut instances, mut pending) = seed_instances(&program, &context, &mut layouts).unwrap();
    assert!(
        instances
            .ids()
            .filter(|id| !instances.roles[id.index()].is_library())
            .all(|id| instances.view(id).definition().is_err())
    );
    let mut seen = BTreeSet::new();
    let mut discovered = 0;
    while let Some(id) = pending.pop() {
        if !seen.insert(instances.view(id).key.encoding().to_vec())
            || instances.roles[id.index()].is_library()
        {
            continue;
        }
        let original = bodies[instances.view(id).key.definition()].clone();
        let (updated, required) =
            discover_instance(&program, &context, &mut layouts, instances, id, original).unwrap();
        instances = updated;
        pending.extend(required);
        discovered += 1;
    }
    assert!(program.definitions.is_empty());
    assert!(discovered >= 5);
    assert!(
        instances
            .roles
            .iter()
            .any(|role| matches!(role, InstanceRole::Closure { .. }))
    );
    assert!(
        instances
            .roles
            .iter()
            .any(|role| matches!(role, InstanceRole::Finalizer))
    );
    assert_eq!(
        instances
            .ids()
            .map(|id| instances.view(id).key.encoding().to_vec())
            .collect::<BTreeSet<_>>(),
        expected
    );
    let main = context
        .sources
        .iter()
        .find(|(path, _)| path.0.name == "main")
        .unwrap()
        .1;
    let mut stale = (*main.declaration).clone();
    stale.signature = hirsel_syntax::ContentHash::default();
    assert!(
        instances
            .register_function(
                &program,
                std::sync::Arc::new(stale),
                main.entry,
                hirsel_rc::instance::Actuals::none(),
                &mut layouts
            )
            .is_err()
    );
}

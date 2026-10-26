use crate::rc_support as support;

use hirsel_lir::ir::Callee;
use support::*;

fn indirect(source: &str) -> usize {
    let (_, lowered) = lower_valid(source);
    lowered
        .units
        .iter()
        .flat_map(|u| &u.tables.calls)
        .filter(|c| matches!(c.callee, Callee::Indirect(_)))
        .count()
}

#[test]
fn a_literal_through_local_storage_calls_its_body_directly() {
    assert_eq!(
        indirect("fn main() -> u64 { let x = 3u64; let f = fn(v:u64)->u64 {v+x}; f(4u64) }"),
        0
    );
}

#[test]
fn known_arguments_specialize_an_erased_apply() {
    let (_, lowered) = crate::scoped_consumer_9::lower_disabled(
        "fn apply(x:u64, f:fn(u64)->u64 uses {})->u64 uses {f} {f(x)}\nfn main()->u64 {let a=3u64;let b=2u64;let add=fn(x:u64)->u64 uses {} {x+a};let mul=fn(x:u64)->u64 uses {} {x*b};apply(apply(4u64,add),mul)}",
    );
    let main = lowered
        .units
        .iter()
        .find(|u| u.header.definition.0.name == "main")
        .unwrap();
    let targets: Vec<_> = main
        .tables
        .calls
        .iter()
        .filter_map(|c| match c.callee {
            Callee::Direct(s) if main.tables.symbols[s.index()].definition.0.name == "apply" => {
                main.tables.symbols[s.index()].instance
            }
            _ => None,
        })
        .collect();
    assert_eq!(targets.len(), 2);
    assert_ne!(
        targets[0], targets[1],
        "different closure bodies need different apply entries"
    );
    for target in targets {
        let unit = lowered
            .units
            .iter()
            .find(|u| Some(u.header.instance) == Some(target))
            .unwrap();
        assert!(
            unit.tables
                .calls
                .iter()
                .all(|c| !matches!(c.callee, Callee::Indirect(_)))
        );
        assert!(
            unit.tables
                .calls
                .iter()
                .any(|c| matches!(c.callee, Callee::Bound { .. }))
        );
    }
}

#[test]
fn a_dynamic_parameter_retains_indirect_dispatch() {
    assert_eq!(
        indirect(
            "fn apply(x:u64,f:fn(u64)->u64 uses {})->u64 uses {f} {f(x)}\nfn main()->u64 {0u64}"
        ),
        1
    );
}

#[test]
fn a_join_of_different_targets_is_dynamic() {
    assert_eq!(
        indirect(
            "fn choose(c:bool)->u64 {let f=if c {fn(x:u64)->u64 {x+1u64}} else {fn(x:u64)->u64 {x+2u64}};f(4u64)}\nfn main()->u64 {choose(true)}"
        ),
        1
    );
}

#[test]
fn exclusive_callable_storage_does_not_keep_a_stale_target() {
    assert_eq!(
        indirect(
            "fn replace(mut f:fn(u64)->u64 uses {}) {f=fn(x:u64)->u64 uses {} {x+9u64}}\nfn main()->u64 {let mut f:fn(u64)->u64 uses {}=fn(x:u64)->u64 uses {} {x+1u64};replace(mut f);f(1u64)}"
        ),
        1
    );
}

#[test]
fn specialization_is_bounded_and_falls_back_to_the_original_entry() {
    let mut source = String::from(
        "fn apply(x:u64,f:fn(u64)->u64 uses {})->u64 uses {f} {f(x)}\nfn main()->u64 {let mut sum=0u64;\n",
    );
    for i in 0..20 {
        source.push_str(&format!(
            "let f{i}=fn(x:u64)->u64 uses {{}} {{x+{i}u64}};sum+=apply(1u64,f{i});\n"
        ));
    }
    source.push_str("sum}");
    let (_, lowered) = lower_valid(&source);
    let original = lowered
        .instances
        .ids()
        .find(|&i| lowered.instances.view(i).key.definition().0.name == "apply")
        .unwrap();
    let count = lowered
        .unit_origins
        .iter()
        .enumerate()
        .filter(|&(i, &o)| o == original && i != original.index())
        .count();
    assert_eq!(count, hirsel_rc::closure_calls::MAX_PER_FUNCTION);
    let main = lowered
        .units
        .iter()
        .find(|u| u.header.definition.0.name == "main")
        .unwrap();
    let original_digest = lowered.units[original.index()].header.instance;
    assert_eq!(
        main.tables
            .calls
            .iter()
            .filter(|c| match c.callee {
                Callee::Direct(s) =>
                    main.tables.symbols[s.index()].instance == Some(original_digest),
                _ => false,
            })
            .count(),
        4
    );
}

#[test]
fn a_specialized_once_callback_preserves_success_failure_and_drop_order() {
    for divisor in [0, 2] {
        let source = format!(
            r#"resource type Trace {{ stdout:Stdout, text:String }}
drop(x) {{let _=out.print(x.stdout,x.text)}}
fn invoke(take f:once fn()->u64)->u64 uses {{f}} {{f()}}
pub fn main(rt:Io,args:Args)->Result[(),GrantError] {{
  let s=io.stdout(rt)?
  let outer=Trace(stdout:s,text:"outer\n")
  let captured=Trace(stdout:s,text:"capture\n")
  let divisor={divisor}u64
  let f:once fn()->u64=fn()->u64 {{let moved=take captured;10u64/divisor}}
  let _=out.print(s,"before\n")
  let value=invoke(take f)
  let _=out.print(s,"after\n")
  Ok(())
}}"#
        );
        hirsel_engine::with_compiler_stack(move || {
            let mut engine = hirsel_engine::QueryEngine::new(
                hirsel_engine::default_environment(),
                hirsel_driver::compiler_adapters(),
            );
            let file = engine.sources.add("closure.hir", source.clone()).unwrap();
            let checked =
                hirsel_engine::check_progress(&mut engine, file, &hirsel_engine::default_context())
                    .unwrap();
            assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
            let entry = hirsel_engine::entry_path(&hirsel_engine::default_context());
            let config = hirsel_engine::RunConfiguration {
                entry: entry.clone(),
                hash_seed: 0,
                inputs: vec![],
            };
            let run =
                hirsel_engine::run_observed_snapshot(&mut engine, &config, Default::default())
                    .unwrap();
            let reference =
                hirsel_driver::native::Observation::of_reference(run.observation.as_ref().unwrap());
            let input = engine.export_native_input().unwrap();
            let validated = engine.import_native_input(input).unwrap();
            let sites = hirsel_driver::native::Sites::new(validated.snapshot());
            let ready = hirsel_driver::native::build(
                &validated,
                &hirsel_driver::native::Options::new(target()),
            )
            .unwrap();
            assert!(
                ready
                    .units
                    .iter()
                    .filter(|u| u.unit().header.definition.0.name == "invoke")
                    .count()
                    > 1
            );
            let heap = hirsel_driver::native::heap_campaign(&ready, &entry, &sites).unwrap();
            assert_eq!(heap.observation, reference);
            assert_eq!(
                reference.output,
                if divisor == 0 {
                    b"before\ncapture\nouter\n".as_slice()
                } else {
                    b"before\ncapture\nafter\nouter\n".as_slice()
                }
            );
        });
    }
}

fn spawn_flags(source: &str) -> Vec<u128> {
    use hirsel_lir::ir::{ArgumentValue, Callee, ConstantValue, Inst};
    let (_, lowered) = lower_valid(source);
    let mut found = Vec::new();
    for u in &lowered.units {
        for call in &u.tables.calls {
            let Callee::Operation(op) = call.callee else {
                continue;
            };
            if !u.tables.operations[op.index()]
                .definition
                .is_core("$intrinsic.TaskSpawn")
            {
                continue;
            }
            let flags = call
                .arguments
                .get(&u.pools.call_arguments)
                .unwrap()
                .iter()
                .find(|a| a.parameter == 2)
                .unwrap();
            let ArgumentValue::Value(v) = flags.value else {
                panic!("flags are bits")
            };
            let constant = u
                .instructions
                .iter()
                .find_map(|i| match *i {
                    Inst::Const { result, constant } if result == v => Some(constant),
                    _ => None,
                })
                .unwrap();
            let ConstantValue::Integer(bits) = u.tables.constants[constant.index()].value else {
                panic!("flags are u64")
            };
            found.push(bits);
        }
    }
    found
}

#[test]
fn a_scalar_child_has_an_op_free_proof_and_an_adjacent_join() {
    let flags = spawn_flags(
        "fn main(tasks: Tasks)->u64 {task.scope(tasks, fn(s:Scope)->u64 {let child=task.async(s,fn()->u64 {42u64}); task.await(s,take child)})}",
    );
    assert!(flags.contains(&3), "flags: {flags:?}");
}

#[test]
fn an_empty_authority_summary_does_not_prove_an_atomic_child_op_free() {
    let flags = spawn_flags(
        "fn main(tasks: Tasks)->u64 {let a=atomic.create(tasks,0u64); task.scope(tasks, fn(s:Scope)->u64 {let child=task.async(s,fn()->u64 {atomic.load(tasks,a)}); task.await(s,take child)})}",
    );
    assert!(flags.contains(&1), "flags: {flags:?}");
    assert!(!flags.iter().any(|f| f & 2 != 0));
}

#[test]
fn an_intervening_operation_prevents_the_immediate_join_proof() {
    let flags = spawn_flags(
        "fn main(tasks: Tasks)->u64 {task.scope(tasks, fn(s:Scope)->u64 {let child=task.async(s,fn()->u64 {42u64}); task.check_canceled(s); task.await(s,take child)})}",
    );
    assert!(flags.contains(&2), "flags: {flags:?}");
    assert!(!flags.iter().any(|f| f & 1 != 0));
}

#[test]
fn a_child_returning_an_owned_result_retains_its_cleanup_fiber() {
    let flags = spawn_flags(
        "fn main(tasks: Tasks)->String {task.scope(tasks, fn(s:Scope)->String {let child=task.async(s,fn()->String {\"owned\"}); task.await(s,take child)})}",
    );
    assert!(flags.contains(&1), "flags: {flags:?}");
    assert!(!flags.iter().any(|f| f & 2 != 0));
}

#[test]
fn a_known_spawned_callable_reaches_its_owned_task_main_adapter() {
    let (_, lowered) = lower_valid(
        "fn main(tasks: Tasks)->u64 {task.scope(tasks, fn(s:Scope)->u64 {let n=42u64; let child=task.async(s,fn()->u64 {n}); task.await(s,take child)})}",
    );
    let adapters: Vec<_> = lowered
        .units
        .iter()
        .filter(|u| u.header.definition.is_core("$native.task_main"))
        .collect();
    assert!(
        adapters.iter().any(|u| u
            .tables
            .calls
            .iter()
            .any(|c| matches!(c.callee, Callee::Indirect(_)))),
        "the dynamic control remains available"
    );
    let specialized = adapters
        .iter()
        .find(|u| {
            u.tables
                .calls
                .iter()
                .any(|c| matches!(c.callee, Callee::Bound { .. }))
        })
        .expect("known callable code must cross the task_main capture");
    assert!(
        !specialized
            .tables
            .calls
            .iter()
            .any(|c| matches!(c.callee, Callee::Indirect(_)))
    );
    assert!(specialized.tables.calls.iter().any(|c| matches!(c.callee, Callee::Operation(op) if specialized.tables.operations[op.index()].definition.is_core("$intrinsic.TaskFinish"))), "holding-result ownership still belongs to TaskFinish");
    assert!(
        lowered
            .units
            .iter()
            .any(|u| u.tables.closures.iter().any(|c| c
                .code
                .is_some_and(|code| u.tables.symbols[code.symbol.index()].instance
                    == Some(specialized.header.instance)))),
        "spawn must actually reference the specialized adapter"
    );
}

#[test]
fn nonadjacent_result_spawn_preserves_code_identity_without_a_fast_hint() {
    let (_, lowered) = lower_valid(
        "fn work(seed:u64,count:u64)->u64 uses {} {let mut x=seed;for i in 0u64..count {x=int.wrapping_add(int.wrapping_mul(x ^ (x >> 7),0x9e3779b97f4a7c15u64),i)};x}\nfn main(tasks:Tasks)->u64 {task.scope(tasks,fn(s:Scope)->u64 {let mut set:TaskSet[Result[u64,()]] of s=task.set(s);for seed in 1u64..5u64 {task.add(s,mut set,task.async(s,fn()->Result[u64,()]{Ok(work(seed,64u64))}))};let mut sum=0u64;if let Ok(rows)=std@1/task.all(s,take set){for x in rows {sum=sum ^ x}};sum})}",
    );
    let adapters: Vec<_> = lowered
        .units
        .iter()
        .filter(|u| u.header.definition.is_core("$native.task_main"))
        .collect();
    assert!(
        adapters.iter().any(|u| u
            .tables
            .calls
            .iter()
            .any(|c| matches!(c.callee, Callee::Bound { .. }))),
        "the ordinary nonadjacent Result child needs a specialized owned adapter"
    );
    assert!(
        adapters.iter().any(|u| u
            .tables
            .calls
            .iter()
            .any(|c| matches!(c.callee, Callee::Indirect(_)))),
        "the original erased adapter must remain available"
    );
}

#[test]
fn known_scope_callbacks_specialize_without_erasing_the_owned_environment() {
    for (name, parameters, arguments) in [
        ("task.scope", "tasks:Tasks", "tasks"),
        (
            "task.scope_until",
            "tasks:Tasks, until:Deadline",
            "tasks,until",
        ),
    ] {
        let source = format!(
            "fn main({parameters}) {{ let captured:List[u64]=[41u64,42u64]; let rows={name}({arguments},fn(s:Scope)->List[u64] {{let kept=take captured;kept}}); }}"
        );
        let (_, lowered) = lower_valid(&source);
        let main = lowered
            .units
            .iter()
            .find(|u| u.header.definition.0.name == "main")
            .unwrap();
        let called = main
            .tables
            .calls
            .iter()
            .find_map(|call| match call.callee {
                Callee::Direct(symbol)
                    if main.tables.symbols[symbol.index()].definition.is_core(name) =>
                {
                    main.tables.symbols[symbol.index()].instance
                }
                _ => None,
            })
            .expect("selected scope instance");
        let wrapper = lowered
            .units
            .iter()
            .find(|u| u.header.instance == called)
            .unwrap();
        assert_eq!(wrapper.header.origin, hirsel_lir::ir::UnitOrigin::Library);
        assert!(
            wrapper
                .tables
                .calls
                .iter()
                .all(|call| !matches!(call.callee, Callee::Indirect(_)))
        );
        let (code, closure) = wrapper
            .tables
            .calls
            .iter()
            .find_map(|call| match call.callee {
                Callee::Bound { code, closure } => Some((code, closure)),
                _ => None,
            })
            .expect("known callback with its environment");
        let callback = lowered
            .units
            .iter()
            .find(|u| Some(u.header.instance) == wrapper.tables.symbols[code.index()].instance)
            .unwrap();
        assert!(
            !callback.captures.is_empty(),
            "specialization preserves captures"
        );
        assert!(
            wrapper.values[closure.index()].holding != hirsel_lir::ir::Holding::Plain,
            "callback environment stays loaned/owned"
        );
        assert!(
            wrapper
                .tables
                .calls
                .iter()
                .filter_map(|call| match call.callee {
                    Callee::Operation(op) =>
                        Some(&wrapper.tables.operations[op.index()].definition),
                    _ => None,
                })
                .any(|definition| definition.is_core(if name == "task.scope" {
                    "$intrinsic.TaskScopeExit"
                } else {
                    "$intrinsic.TaskScopeExitUntil"
                }))
        );
    }
}

#[test]
fn scope_callbacks_with_different_phi_targets_keep_the_general_instance() {
    let (_, lowered) = lower_valid(
        "fn main(tasks:Tasks,flag:bool)->List[u64] {let f=if flag {fn(s:Scope)->List[u64]{[41u64]}}else{fn(s:Scope)->List[u64]{[42u64]}};task.scope(tasks,f)}",
    );
    let main = lowered
        .units
        .iter()
        .find(|u| u.header.definition.0.name == "main")
        .unwrap();
    let called = main
        .tables
        .calls
        .iter()
        .find_map(|call| match call.callee {
            Callee::Direct(symbol)
                if main.tables.symbols[symbol.index()]
                    .definition
                    .is_core("task.scope") =>
            {
                main.tables.symbols[symbol.index()].instance
            }
            _ => None,
        })
        .unwrap();
    let wrapper = lowered
        .units
        .iter()
        .find(|u| u.header.instance == called)
        .unwrap();
    assert!(
        wrapper
            .tables
            .calls
            .iter()
            .any(|call| matches!(call.callee, Callee::Indirect(_)))
    );
}

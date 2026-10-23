//! Two-stage CLO-2/CLO-4 transport controls. Koka Unify.hs:177-214,374-404
//! and wrong/{subsume,effect1}.kk at 198fc3bb (Apache-2.0) motivate
//! subsumption and nested-callback adversaries; Hirsel's own uses rules are the oracle.
use hirsel_driver::*;
use hirsel_engine::*;
use hirsel_types::*;

fn source_case(id: &str, source: &str, expected: Option<&str>) -> bool {
    with_compiler_stack(|| {
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
        let file = engine
            .sources
            .add("stored-callable.hir", source.into())
            .unwrap();
        let output = check_progress(&mut engine, file, &default_context()).unwrap();
        let rules: Vec<_> = output.diagnostics.iter().map(|d| d.rule.as_str()).collect();
        println!(
            "AUTH26 {}",
            serde_json::json!({"id":id,"engine":"checker","source":source,"expected":expected,"observed":rules})
        );
        assert_eq!(output.completion, CheckCompletion::Complete, "{id}");
        if rules.first().copied() != expected {
            eprintln!("{id}: {source}\n{:?}", output.diagnostics);
            return false;
        }
        if expected.is_none() {
            let input = engine
                .export_native_input()
                .unwrap_or_else(|e| panic!("{id}: {e:?}"));
            let mut signatures = hirsel_validate::SignatureMap::new(input.types.clone());
            signatures.extend_signatures(input.signatures.values().map(|s| s.as_ref().clone()));
            for definition in input.definitions.values() {
                let report =
                    hirsel_validate::validate(definition, &input.types, &signatures, &mut vec![]);
                assert!(report.violations.is_empty(), "{id}: {report:?}");
            }
            println!(
                "AUTH26 {}",
                serde_json::json!({"id":id,"engine":"V","observed":"valid"})
            );
            paired_v_negative(id, &input, &signatures);
        }
        true
    })
}

#[test]
fn stored_callable_authority_26_source_partition_0() {
    let mut failed = 0;
    failed += usize::from(!source_case(
        "S-0-list-relay-1",
        r#"fn effect(d:Dir) uses {Dir} {}
fn relay(take xs:List[fn(Dir)->() uses {Dir}])->List[fn(Dir)->() uses {Dir}] {take xs}
pub fn main(d:Dir,flag:bool) uses {Dir} {let first:List[fn(Dir)->() uses {Dir}]=[effect];let held=relay(take first);held[0](d)}
"#,
        None,
    ));
    failed += usize::from(!source_case(
        "S-0-list-relay-0",
        r#"fn effect(d:Dir) uses {Dir} {}
fn relay(take xs:List[fn(Dir)->() uses {}])->List[fn(Dir)->() uses {}] {take xs}
pub fn main(d:Dir,flag:bool) uses {Dir} {let first:List[fn(Dir)->() uses {}]=[effect];let held=relay(take first);held[0](d)}
"#,
        Some("CLO-4"),
    ));
    failed += usize::from(!source_case(
        "S-0-record-relay-1",
        r#"fn effect(d:Dir) uses {Dir} {}
type Slot { call: fn(Dir)->() uses {Dir} }
fn relay(take slot:Slot)->Slot {take slot}
pub fn main(d:Dir,flag:bool) uses {Dir} {let first=Slot(call:effect);let held=relay(take first);held.call(d)}
"#,
        None,
    ));
    failed += usize::from(!source_case(
        "S-0-record-relay-0",
        r#"fn effect(d:Dir) uses {Dir} {}
type Slot { call: fn(Dir)->() uses {} }
fn relay(take slot:Slot)->Slot {take slot}
pub fn main(d:Dir,flag:bool) uses {Dir} {let first=Slot(call:effect);let held=relay(take first);held.call(d)}
"#,
        Some("CLO-4"),
    ));
    failed += usize::from(!source_case(
        "S-0-returned-list-1",
        r#"fn effect(d:Dir) uses {Dir} {}
fn make()->List[fn(Dir)->() uses {Dir}] {[effect]}
pub fn main(d:Dir,flag:bool) uses {Dir} {let held=make();held[0](d)}
"#,
        None,
    ));
    failed += usize::from(!source_case(
        "S-0-returned-list-0",
        r#"fn effect(d:Dir) uses {Dir} {}
fn make()->List[fn(Dir)->() uses {}] {[effect]}
pub fn main(d:Dir,flag:bool) uses {Dir} {let held=make();held[0](d)}
"#,
        Some("CLO-4"),
    ));
    failed += usize::from(!source_case(
        "S-0-returned-record-1",
        r#"fn effect(d:Dir) uses {Dir} {}
type Slot { call: fn(Dir)->() uses {Dir} }
fn make()->Slot {Slot(call:effect)}
pub fn main(d:Dir,flag:bool) uses {Dir} {let held=make();held.call(d)}
"#,
        None,
    ));
    failed += usize::from(!source_case(
        "S-0-returned-record-0",
        r#"fn effect(d:Dir) uses {Dir} {}
type Slot { call: fn(Dir)->() uses {} }
fn make()->Slot {Slot(call:effect)}
pub fn main(d:Dir,flag:bool) uses {Dir} {let held=make();held.call(d)}
"#,
        Some("CLO-4"),
    ));
    failed += usize::from(!source_case(
        "S-0-list-list-1",
        r#"fn effect(d:Dir) uses {Dir} {}
pub fn main(d:Dir,flag:bool) uses {Dir} {let first:List[List[fn(Dir)->() uses {Dir}]]=[[effect]];let held=first[0];held[0](d)}
"#,
        None,
    ));
    failed += usize::from(!source_case(
        "S-0-list-list-0",
        r#"fn effect(d:Dir) uses {Dir} {}
pub fn main(d:Dir,flag:bool) uses {Dir} {let first:List[List[fn(Dir)->() uses {}]]=[[effect]];let held=first[0];held[0](d)}
"#,
        Some("CLO-4"),
    ));
    failed += usize::from(!source_case(
        "S-0-record-list-1",
        r#"fn effect(d:Dir) uses {Dir} {}
type Batch { calls:List[fn(Dir)->() uses {Dir}] }
pub fn main(d:Dir,flag:bool) uses {Dir} {let first=Batch(calls:[effect]);let held=first.calls;held[0](d)}
"#,
        None,
    ));
    failed += usize::from(!source_case(
        "S-0-record-list-0",
        r#"fn effect(d:Dir) uses {Dir} {}
type Batch { calls:List[fn(Dir)->() uses {}] }
pub fn main(d:Dir,flag:bool) uses {Dir} {let first=Batch(calls:[effect]);let held=first.calls;held[0](d)}
"#,
        Some("CLO-4"),
    ));
    failed += usize::from(!source_case(
        "S-0-branch-list-1",
        r#"fn effect(d:Dir) uses {Dir} {}
pub fn main(d:Dir,flag:bool) uses {Dir} {let first:List[fn(Dir)->() uses {Dir}]=[effect];let held=if flag {copy first} else {copy first};held[0](d)}
"#,
        None,
    ));
    failed += usize::from(!source_case(
        "S-0-branch-list-0",
        r#"fn effect(d:Dir) uses {Dir} {}
pub fn main(d:Dir,flag:bool) uses {Dir} {let first:List[fn(Dir)->() uses {}]=[effect];let held=if flag {copy first} else {copy first};held[0](d)}
"#,
        Some("CLO-4"),
    ));
    failed += usize::from(!source_case(
        "S-0-branch-record-1",
        r#"fn effect(d:Dir) uses {Dir} {}
type Slot { call: fn(Dir)->() uses {Dir} }
pub fn main(d:Dir,flag:bool) uses {Dir} {let first=Slot(call:effect);let held=if flag {copy first} else {copy first};held.call(d)}
"#,
        None,
    ));
    failed += usize::from(!source_case(
        "S-0-branch-record-0",
        r#"fn effect(d:Dir) uses {Dir} {}
type Slot { call: fn(Dir)->() uses {} }
pub fn main(d:Dir,flag:bool) uses {Dir} {let first=Slot(call:effect);let held=if flag {copy first} else {copy first};held.call(d)}
"#,
        Some("CLO-4"),
    ));
    assert_eq!(failed, 0);
}

#[test]
fn stored_callable_authority_26_source_partition_1() {
    let mut failed = 0;
    failed += usize::from(!source_case(
        "S-1-list-relay-1-setup3",
        r#"fn effect(token:Dir) uses {Dir} {}
fn relay(take xs:List[fn(Dir)->() uses {Dir}])->List[fn(Dir)->() uses {Dir}] {take xs}
pub fn main(d:Dir,flag:bool) uses {Dir} {let implementation=effect;let cb:fn(Dir)->() uses {Dir}=fn(arg:Dir) uses {Dir} {implementation(arg)};let first:List[fn(Dir)->() uses {Dir}]=[cb];let held=relay(take first);held[0](d)}
"#,
        None,
    ));
    failed += usize::from(!source_case(
        "S-1-list-relay-0-setup3",
        r#"fn effect(token:Dir) uses {Dir} {}
fn relay(take xs:List[fn(Dir)->() uses {}])->List[fn(Dir)->() uses {}] {take xs}
pub fn main(d:Dir,flag:bool) uses {Dir} {let implementation=effect;let cb:fn(Dir)->() uses {Dir}=fn(arg:Dir) uses {Dir} {implementation(arg)};let first:List[fn(Dir)->() uses {}]=[cb];let held=relay(take first);held[0](d)}
"#,
        Some("CLO-4"),
    ));
    failed += usize::from(!source_case(
        "S-1-record-relay-1-setup3",
        r#"fn effect(token:Dir) uses {Dir} {}
type Slot {call:fn(Dir)->() uses {Dir}}
fn relay(take slot:Slot)->Slot {take slot}
pub fn main(d:Dir,flag:bool) uses {Dir} {let implementation=effect;let cb:fn(Dir)->() uses {Dir}=fn(arg:Dir) uses {Dir} {implementation(arg)};let first=Slot(call:cb);let held=relay(take first);held.call(d)}
"#,
        None,
    ));
    failed += usize::from(!source_case(
        "S-1-record-relay-0-setup3",
        r#"fn effect(token:Dir) uses {Dir} {}
type Slot {call:fn(Dir)->() uses {}}
fn relay(take slot:Slot)->Slot {take slot}
pub fn main(d:Dir,flag:bool) uses {Dir} {let implementation=effect;let cb:fn(Dir)->() uses {Dir}=fn(arg:Dir) uses {Dir} {implementation(arg)};let first=Slot(call:cb);let held=relay(take first);held.call(d)}
"#,
        Some("CLO-4"),
    ));
    failed += usize::from(!source_case(
        "S-1-returned-callable-1-setup3",
        r#"fn effect(token:Dir) uses {Dir} {}
fn make()->fn(Dir)->() uses {Dir} {let implementation=effect;let cb:fn(Dir)->() uses {Dir}=fn(arg:Dir) uses {Dir} {implementation(arg)};cb}
pub fn main(d:Dir,flag:bool) uses {Dir} {let held=make();held(d)}
"#,
        None,
    ));
    failed += usize::from(!source_case(
        "S-1-returned-callable-0-setup3",
        r#"fn effect(token:Dir) uses {Dir} {}
fn make()->fn(Dir)->() uses {} {let implementation=effect;let cb:fn(Dir)->() uses {Dir}=fn(arg:Dir) uses {Dir} {implementation(arg)};cb}
pub fn main(d:Dir,flag:bool) uses {Dir} {let held=make();held(d)}
"#,
        Some("CLO-4"),
    ));
    failed += usize::from(!source_case(
        "S-1-returned-factory-1-setup3",
        r#"fn effect(token:Dir) uses {Dir} {}
fn make()->fn()->fn(Dir)->() uses {Dir} uses {Dir} {let implementation=effect;let cb:fn(Dir)->() uses {Dir}=fn(arg:Dir) uses {Dir} {implementation(arg)};fn()->fn(Dir)->() uses {Dir} uses {Dir} {cb}}
pub fn main(d:Dir,flag:bool) uses {Dir} {let factory=make();let held=factory();held(d)}
"#,
        None,
    ));
    failed += usize::from(!source_case(
        "S-1-returned-factory-0-setup3",
        r#"fn effect(token:Dir) uses {Dir} {}
fn make()->fn()->fn(Dir)->() uses {} uses {Dir} {let implementation=effect;let cb:fn(Dir)->() uses {Dir}=fn(arg:Dir) uses {Dir} {implementation(arg)};fn()->fn(Dir)->() uses {} uses {Dir} {cb}}
pub fn main(d:Dir,flag:bool) uses {Dir} {let factory=make();let held=factory();held(d)}
"#,
        Some("CLO-4"),
    ));
    failed += usize::from(!source_case(
        "S-1-list-list-1-setup3",
        r#"fn effect(token:Dir) uses {Dir} {}
pub fn main(d:Dir,flag:bool) uses {Dir} {let implementation=effect;let cb:fn(Dir)->() uses {Dir}=fn(arg:Dir) uses {Dir} {implementation(arg)};let first:List[List[fn(Dir)->() uses {Dir}]]=[[cb]];let held=first[0];held[0](d)}
"#,
        None,
    ));
    failed += usize::from(!source_case(
        "S-1-list-list-0-setup3",
        r#"fn effect(token:Dir) uses {Dir} {}
pub fn main(d:Dir,flag:bool) uses {Dir} {let implementation=effect;let cb:fn(Dir)->() uses {Dir}=fn(arg:Dir) uses {Dir} {implementation(arg)};let first:List[List[fn(Dir)->() uses {}]]=[[cb]];let held=first[0];held[0](d)}
"#,
        Some("CLO-4"),
    ));
    failed += usize::from(!source_case(
        "S-1-record-list-1-setup3",
        r#"fn effect(token:Dir) uses {Dir} {}
type Batch {calls:List[fn(Dir)->() uses {Dir}]}
pub fn main(d:Dir,flag:bool) uses {Dir} {let implementation=effect;let cb:fn(Dir)->() uses {Dir}=fn(arg:Dir) uses {Dir} {implementation(arg)};let first=Batch(calls:[cb]);let held=first.calls;held[0](d)}
"#,
        None,
    ));
    failed += usize::from(!source_case(
        "S-1-record-list-0-setup3",
        r#"fn effect(token:Dir) uses {Dir} {}
type Batch {calls:List[fn(Dir)->() uses {}]}
pub fn main(d:Dir,flag:bool) uses {Dir} {let implementation=effect;let cb:fn(Dir)->() uses {Dir}=fn(arg:Dir) uses {Dir} {implementation(arg)};let first=Batch(calls:[cb]);let held=first.calls;held[0](d)}
"#,
        Some("CLO-4"),
    ));
    failed += usize::from(!source_case(
        "S-1-branch-list-1-setup3",
        r#"fn effect(token:Dir) uses {Dir} {}
pub fn main(d:Dir,flag:bool) uses {Dir} {let implementation=effect;let cb:fn(Dir)->() uses {Dir}=fn(arg:Dir) uses {Dir} {implementation(arg)};let first:List[fn(Dir)->() uses {Dir}]=[cb];let held=if flag {copy first} else {copy first};held[0](d)}
"#,
        None,
    ));
    failed += usize::from(!source_case(
        "S-1-branch-list-0-setup3",
        r#"fn effect(token:Dir) uses {Dir} {}
pub fn main(d:Dir,flag:bool) uses {Dir} {let implementation=effect;let cb:fn(Dir)->() uses {Dir}=fn(arg:Dir) uses {Dir} {implementation(arg)};let first:List[fn(Dir)->() uses {}]=[cb];let held=if flag {copy first} else {copy first};held[0](d)}
"#,
        Some("CLO-4"),
    ));
    failed += usize::from(!source_case(
        "S-1-branch-record-1-setup3",
        r#"fn effect(token:Dir) uses {Dir} {}
type Slot {call:fn(Dir)->() uses {Dir}}
pub fn main(d:Dir,flag:bool) uses {Dir} {let implementation=effect;let cb:fn(Dir)->() uses {Dir}=fn(arg:Dir) uses {Dir} {implementation(arg)};let first=Slot(call:cb);let held=if flag {copy first} else {copy first};held.call(d)}
"#,
        None,
    ));
    failed += usize::from(!source_case(
        "S-1-branch-record-0-setup3",
        r#"fn effect(token:Dir) uses {Dir} {}
type Slot {call:fn(Dir)->() uses {}}
pub fn main(d:Dir,flag:bool) uses {Dir} {let implementation=effect;let cb:fn(Dir)->() uses {Dir}=fn(arg:Dir) uses {Dir} {implementation(arg)};let first=Slot(call:cb);let held=if flag {copy first} else {copy first};held.call(d)}
"#,
        Some("CLO-4"),
    ));
    assert_eq!(failed, 0);
}

#[test]
fn stored_callable_authority_26_source_partition_2() {
    let mut failed = 0;
    failed += usize::from(!source_case(
        "S-select-f-0-1-setup3",
        r#"fn seven()->u64 uses {} {7}
fn twentynine()->u64 uses {} {29}
fn choose(f:fn()->u64,g:fn()->u64,flag:bool)->u64 uses {f} {let outer=fn()->u64 {let inner=fn()->u64 {f()};inner()};outer()}
pub fn main()->Result[(),()] uses {} {let saved:fn()->u64 uses {}=seven;let callbacks:List[fn()->u64 uses {}]=[saved,twentynine];if choose(callbacks[0],callbacks[1],true)!=7u64 {return Error(())};if choose(callbacks[0],callbacks[1],false)!=7u64 {return Error(())};Ok(())}
"#,
        None,
    ));
    failed += usize::from(!source_case(
        "S-select-f-0-0-setup3",
        r#"fn seven()->u64 uses {} {7}
fn twentynine()->u64 uses {} {29}
fn choose(f:fn()->u64,g:fn()->u64,flag:bool)->u64 uses {f} {let outer=fn()->u64 {let inner=fn()->u64 {g()};inner()};outer()}
pub fn main()->Result[(),()] uses {} {let saved:fn()->u64 uses {}=seven;let callbacks:List[fn()->u64 uses {}]=[saved,twentynine];if choose(callbacks[0],callbacks[1],true)!=7u64 {return Error(())};if choose(callbacks[0],callbacks[1],false)!=7u64 {return Error(())};Ok(())}
"#,
        Some("CLO-2"),
    ));
    failed += usize::from(!source_case(
        "S-select-f-1-1-setup3",
        r#"fn seven()->u64 uses {} {7}
fn twentynine()->u64 uses {} {29}
fn choose(f:fn()->u64,g:fn()->u64,flag:bool)->u64 uses {f} {let first=fn()->u64 {f()};let outer=fn()->u64 {let inner=fn()->u64 {first()};inner()};outer()}
pub fn main()->Result[(),()] uses {} {let saved=seven;let callbacks:List[fn()->u64 uses {}]=[saved,twentynine];if choose(callbacks[0],callbacks[1],true)!=7u64 {return Error(())};if choose(callbacks[0],callbacks[1],false)!=7u64 {return Error(())};Ok(())}
"#,
        None,
    ));
    failed += usize::from(!source_case(
        "S-select-f-1-0-setup3",
        r#"fn seven()->u64 uses {} {7}
fn twentynine()->u64 uses {} {29}
fn choose(f:fn()->u64,g:fn()->u64,flag:bool)->u64 uses {f} {let first=fn()->u64 {g()};let outer=fn()->u64 {let inner=fn()->u64 {first()};inner()};outer()}
pub fn main()->Result[(),()] uses {} {let saved=seven;let callbacks:List[fn()->u64 uses {}]=[saved,twentynine];if choose(callbacks[0],callbacks[1],true)!=7u64 {return Error(())};if choose(callbacks[0],callbacks[1],false)!=7u64 {return Error(())};Ok(())}
"#,
        Some("CLO-2"),
    ));
    failed += usize::from(!source_case(
        "S-select-f-2-1-setup3",
        r#"fn seven()->u64 uses {} {7}
fn twentynine()->u64 uses {} {29}
fn choose(f:fn()->u64,g:fn()->u64,flag:bool)->u64 uses {f} {let outer=fn()->u64 {let inner=fn()->u64 {if flag {f()} else {f()}};inner()};outer()}
pub fn main()->Result[(),()] uses {} {let saved:fn()->u64 uses {}=seven;let callbacks:List[fn()->u64 uses {}]=[saved,twentynine];if choose(callbacks[0],callbacks[1],true)!=7u64 {return Error(())};if choose(callbacks[0],callbacks[1],false)!=7u64 {return Error(())};Ok(())}
"#,
        None,
    ));
    failed += usize::from(!source_case(
        "S-select-f-2-0-setup3",
        r#"fn seven()->u64 uses {} {7}
fn twentynine()->u64 uses {} {29}
fn choose(f:fn()->u64,g:fn()->u64,flag:bool)->u64 uses {f} {let outer=fn()->u64 {let inner=fn()->u64 {if flag {g()} else {f()}};inner()};outer()}
pub fn main()->Result[(),()] uses {} {let saved:fn()->u64 uses {}=seven;let callbacks:List[fn()->u64 uses {}]=[saved,twentynine];if choose(callbacks[0],callbacks[1],true)!=7u64 {return Error(())};if choose(callbacks[0],callbacks[1],false)!=7u64 {return Error(())};Ok(())}
"#,
        Some("CLO-2"),
    ));
    failed += usize::from(!source_case(
        "S-select-f-3-1-setup3",
        r#"fn seven()->u64 uses {} {7}
fn twentynine()->u64 uses {} {29}
fn choose(f:fn()->u64,g:fn()->u64,flag:bool)->u64 uses {f} {let first=f;let outer=fn()->u64 {let next=first;let inner=fn()->u64 {next()};inner()};outer()}
pub fn main()->Result[(),()] uses {} {let saved=seven;let callbacks:List[fn()->u64 uses {}]=[saved,twentynine];if choose(callbacks[0],callbacks[1],true)!=7u64 {return Error(())};if choose(callbacks[0],callbacks[1],false)!=7u64 {return Error(())};Ok(())}
"#,
        None,
    ));
    failed += usize::from(!source_case(
        "S-select-f-3-0-setup3",
        r#"fn seven()->u64 uses {} {7}
fn twentynine()->u64 uses {} {29}
fn choose(f:fn()->u64,g:fn()->u64,flag:bool)->u64 uses {f} {let first=g;let outer=fn()->u64 {let next=first;let inner=fn()->u64 {next()};inner()};outer()}
pub fn main()->Result[(),()] uses {} {let saved=seven;let callbacks:List[fn()->u64 uses {}]=[saved,twentynine];if choose(callbacks[0],callbacks[1],true)!=7u64 {return Error(())};if choose(callbacks[0],callbacks[1],false)!=7u64 {return Error(())};Ok(())}
"#,
        Some("CLO-2"),
    ));
    assert_eq!(failed, 0);
}

#[test]
fn stored_callable_authority_26_source_partition_3() {
    let mut failed = 0;
    failed += usize::from(!source_case(
        "S-select-g-0-1-setup3",
        r#"fn seven()->u64 uses {} {7}
fn twentynine()->u64 uses {} {29}
fn choose(f:fn()->u64,g:fn()->u64,flag:bool)->u64 uses {g} {let outer=fn()->u64 {let inner=fn()->u64 {g()};inner()};outer()}
pub fn main()->Result[(),()] uses {} {let saved:fn()->u64 uses {}=seven;let callbacks:List[fn()->u64 uses {}]=[saved,twentynine];if choose(callbacks[0],callbacks[1],true)!=29u64 {return Error(())};if choose(callbacks[0],callbacks[1],false)!=29u64 {return Error(())};Ok(())}
"#,
        None,
    ));
    failed += usize::from(!source_case(
        "S-select-g-0-0-setup3",
        r#"fn seven()->u64 uses {} {7}
fn twentynine()->u64 uses {} {29}
fn choose(f:fn()->u64,g:fn()->u64,flag:bool)->u64 uses {g} {let outer=fn()->u64 {let inner=fn()->u64 {f()};inner()};outer()}
pub fn main()->Result[(),()] uses {} {let saved:fn()->u64 uses {}=seven;let callbacks:List[fn()->u64 uses {}]=[saved,twentynine];if choose(callbacks[0],callbacks[1],true)!=29u64 {return Error(())};if choose(callbacks[0],callbacks[1],false)!=29u64 {return Error(())};Ok(())}
"#,
        Some("CLO-2"),
    ));
    failed += usize::from(!source_case(
        "S-select-g-1-1-setup3",
        r#"fn seven()->u64 uses {} {7}
fn twentynine()->u64 uses {} {29}
fn choose(f:fn()->u64,g:fn()->u64,flag:bool)->u64 uses {g} {let first=fn()->u64 {g()};let outer=fn()->u64 {let inner=fn()->u64 {first()};inner()};outer()}
pub fn main()->Result[(),()] uses {} {let saved=seven;let callbacks:List[fn()->u64 uses {}]=[saved,twentynine];if choose(callbacks[0],callbacks[1],true)!=29u64 {return Error(())};if choose(callbacks[0],callbacks[1],false)!=29u64 {return Error(())};Ok(())}
"#,
        None,
    ));
    failed += usize::from(!source_case(
        "S-select-g-1-0-setup3",
        r#"fn seven()->u64 uses {} {7}
fn twentynine()->u64 uses {} {29}
fn choose(f:fn()->u64,g:fn()->u64,flag:bool)->u64 uses {g} {let first=fn()->u64 {f()};let outer=fn()->u64 {let inner=fn()->u64 {first()};inner()};outer()}
pub fn main()->Result[(),()] uses {} {let saved=seven;let callbacks:List[fn()->u64 uses {}]=[saved,twentynine];if choose(callbacks[0],callbacks[1],true)!=29u64 {return Error(())};if choose(callbacks[0],callbacks[1],false)!=29u64 {return Error(())};Ok(())}
"#,
        Some("CLO-2"),
    ));
    failed += usize::from(!source_case(
        "S-select-g-2-1-setup3",
        r#"fn seven()->u64 uses {} {7}
fn twentynine()->u64 uses {} {29}
fn choose(f:fn()->u64,g:fn()->u64,flag:bool)->u64 uses {g} {let outer=fn()->u64 {let inner=fn()->u64 {if flag {g()} else {g()}};inner()};outer()}
pub fn main()->Result[(),()] uses {} {let saved:fn()->u64 uses {}=seven;let callbacks:List[fn()->u64 uses {}]=[saved,twentynine];if choose(callbacks[0],callbacks[1],true)!=29u64 {return Error(())};if choose(callbacks[0],callbacks[1],false)!=29u64 {return Error(())};Ok(())}
"#,
        None,
    ));
    failed += usize::from(!source_case(
        "S-select-g-2-0-setup3",
        r#"fn seven()->u64 uses {} {7}
fn twentynine()->u64 uses {} {29}
fn choose(f:fn()->u64,g:fn()->u64,flag:bool)->u64 uses {g} {let outer=fn()->u64 {let inner=fn()->u64 {if flag {f()} else {g()}};inner()};outer()}
pub fn main()->Result[(),()] uses {} {let saved:fn()->u64 uses {}=seven;let callbacks:List[fn()->u64 uses {}]=[saved,twentynine];if choose(callbacks[0],callbacks[1],true)!=29u64 {return Error(())};if choose(callbacks[0],callbacks[1],false)!=29u64 {return Error(())};Ok(())}
"#,
        Some("CLO-2"),
    ));
    failed += usize::from(!source_case(
        "S-select-g-3-1-setup3",
        r#"fn seven()->u64 uses {} {7}
fn twentynine()->u64 uses {} {29}
fn choose(f:fn()->u64,g:fn()->u64,flag:bool)->u64 uses {g} {let first=g;let outer=fn()->u64 {let next=first;let inner=fn()->u64 {next()};inner()};outer()}
pub fn main()->Result[(),()] uses {} {let saved=seven;let callbacks:List[fn()->u64 uses {}]=[saved,twentynine];if choose(callbacks[0],callbacks[1],true)!=29u64 {return Error(())};if choose(callbacks[0],callbacks[1],false)!=29u64 {return Error(())};Ok(())}
"#,
        None,
    ));
    failed += usize::from(!source_case(
        "S-select-g-3-0-setup3",
        r#"fn seven()->u64 uses {} {7}
fn twentynine()->u64 uses {} {29}
fn choose(f:fn()->u64,g:fn()->u64,flag:bool)->u64 uses {g} {let first=f;let outer=fn()->u64 {let next=first;let inner=fn()->u64 {next()};inner()};outer()}
pub fn main()->Result[(),()] uses {} {let saved=seven;let callbacks:List[fn()->u64 uses {}]=[saved,twentynine];if choose(callbacks[0],callbacks[1],true)!=29u64 {return Error(())};if choose(callbacks[0],callbacks[1],false)!=29u64 {return Error(())};Ok(())}
"#,
        Some("CLO-2"),
    ));
    assert_eq!(failed, 0);
}

#[path = "../support/history.rs"]
#[allow(dead_code, reason = "shared runner also supports minimization")]
mod history;
fn runtime_case(id: &str, source: &str) {
    if std::env::var_os("HIRSEL_AUTH26_CHILD").is_none() {
        let dir = tempfile::tempdir().unwrap();
        let capped = dir.path().join("capped");
        std::fs::write(&capped, include_str!("../../../../tools/capped")).unwrap();
        let name = std::thread::current().name().unwrap().to_owned();
        let output = std::process::Command::new("python3")
            .arg(capped)
            .args(["--timeout", "60"])
            .arg(std::env::current_exe().unwrap())
            .args([&name, "--exact", "--nocapture", "--test-threads=1"])
            .env("HIRSEL_AUTH26_CHILD", "1")
            .env("HIRSEL_JOBS", "1")
            .env("HIRSEL_MEMORY_LIMIT", "2G")
            .output()
            .unwrap();
        println!("{}", String::from_utf8_lossy(&output.stdout));
        eprintln!("{}", String::from_utf8_lossy(&output.stderr));
        assert!(output.status.success(), "{id}: {:?}", output.status);
        return;
    }

    let dir = tempfile::tempdir().unwrap();
    let runner = history::Runner::new(dir.path()).unwrap();
    let report = runner.run(id, source);
    assert!(report.validated, "{id}: {report:?}");
    assert_eq!(report.cells.len(), 4, "{id}: {report:?}");
    for cell in &report.cells {
        println!(
            "AUTH26 {}",
            serde_json::json!({"id":id,"engine":cell.engine,"category":format!("{:?}",cell.category),"observation":format!("{:?}",cell.observation)})
        );
        assert_eq!(
            cell.category,
            history::Category::Observation,
            "{id}: {cell:?}"
        );
        let got = cell.observation.as_ref().unwrap();
        assert_eq!(got.exit_code, Some(0), "{id}: {cell:?}");
        assert!(
            got.output.is_empty()
                && got.failures.is_empty()
                && !got.blocked
                && got.budget.is_none()
                && got.signaled.is_none()
                && got.roots.is_empty(),
            "{id}: {cell:?}"
        );
    }
}

#[test]
fn stored_callable_authority_26_runtime_0() {
    runtime_case(
        "S-select-f-0-1-setup3",
        r#"fn seven()->u64 uses {} {7}
fn twentynine()->u64 uses {} {29}
fn choose(f:fn()->u64,g:fn()->u64,flag:bool)->u64 uses {f} {let outer=fn()->u64 {let inner=fn()->u64 {f()};inner()};outer()}
pub fn main()->Result[(),()] uses {} {let saved:fn()->u64 uses {}=seven;let callbacks:List[fn()->u64 uses {}]=[saved,twentynine];if choose(callbacks[0],callbacks[1],true)!=7u64 {return Error(())};if choose(callbacks[0],callbacks[1],false)!=7u64 {return Error(())};Ok(())}
"#,
    );
}

#[test]
fn stored_callable_authority_26_runtime_1() {
    runtime_case(
        "S-select-f-1-1-setup3",
        r#"fn seven()->u64 uses {} {7}
fn twentynine()->u64 uses {} {29}
fn choose(f:fn()->u64,g:fn()->u64,flag:bool)->u64 uses {f} {let first=fn()->u64 {f()};let outer=fn()->u64 {let inner=fn()->u64 {first()};inner()};outer()}
pub fn main()->Result[(),()] uses {} {let saved=seven;let callbacks:List[fn()->u64 uses {}]=[saved,twentynine];if choose(callbacks[0],callbacks[1],true)!=7u64 {return Error(())};if choose(callbacks[0],callbacks[1],false)!=7u64 {return Error(())};Ok(())}
"#,
    );
}

#[test]
fn stored_callable_authority_26_runtime_2() {
    runtime_case(
        "S-select-f-2-1-setup3",
        r#"fn seven()->u64 uses {} {7}
fn twentynine()->u64 uses {} {29}
fn choose(f:fn()->u64,g:fn()->u64,flag:bool)->u64 uses {f} {let outer=fn()->u64 {let inner=fn()->u64 {if flag {f()} else {f()}};inner()};outer()}
pub fn main()->Result[(),()] uses {} {let saved:fn()->u64 uses {}=seven;let callbacks:List[fn()->u64 uses {}]=[saved,twentynine];if choose(callbacks[0],callbacks[1],true)!=7u64 {return Error(())};if choose(callbacks[0],callbacks[1],false)!=7u64 {return Error(())};Ok(())}
"#,
    );
}

#[test]
fn stored_callable_authority_26_runtime_3() {
    runtime_case(
        "S-select-f-3-1-setup3",
        r#"fn seven()->u64 uses {} {7}
fn twentynine()->u64 uses {} {29}
fn choose(f:fn()->u64,g:fn()->u64,flag:bool)->u64 uses {f} {let first=f;let outer=fn()->u64 {let next=first;let inner=fn()->u64 {next()};inner()};outer()}
pub fn main()->Result[(),()] uses {} {let saved=seven;let callbacks:List[fn()->u64 uses {}]=[saved,twentynine];if choose(callbacks[0],callbacks[1],true)!=7u64 {return Error(())};if choose(callbacks[0],callbacks[1],false)!=7u64 {return Error(())};Ok(())}
"#,
    );
}

#[test]
fn stored_callable_authority_26_runtime_4() {
    runtime_case(
        "S-select-g-0-1-setup3",
        r#"fn seven()->u64 uses {} {7}
fn twentynine()->u64 uses {} {29}
fn choose(f:fn()->u64,g:fn()->u64,flag:bool)->u64 uses {g} {let outer=fn()->u64 {let inner=fn()->u64 {g()};inner()};outer()}
pub fn main()->Result[(),()] uses {} {let saved:fn()->u64 uses {}=seven;let callbacks:List[fn()->u64 uses {}]=[saved,twentynine];if choose(callbacks[0],callbacks[1],true)!=29u64 {return Error(())};if choose(callbacks[0],callbacks[1],false)!=29u64 {return Error(())};Ok(())}
"#,
    );
}

#[test]
fn stored_callable_authority_26_runtime_5() {
    runtime_case(
        "S-select-g-1-1-setup3",
        r#"fn seven()->u64 uses {} {7}
fn twentynine()->u64 uses {} {29}
fn choose(f:fn()->u64,g:fn()->u64,flag:bool)->u64 uses {g} {let first=fn()->u64 {g()};let outer=fn()->u64 {let inner=fn()->u64 {first()};inner()};outer()}
pub fn main()->Result[(),()] uses {} {let saved=seven;let callbacks:List[fn()->u64 uses {}]=[saved,twentynine];if choose(callbacks[0],callbacks[1],true)!=29u64 {return Error(())};if choose(callbacks[0],callbacks[1],false)!=29u64 {return Error(())};Ok(())}
"#,
    );
}

#[test]
fn stored_callable_authority_26_runtime_6() {
    runtime_case(
        "S-select-g-2-1-setup3",
        r#"fn seven()->u64 uses {} {7}
fn twentynine()->u64 uses {} {29}
fn choose(f:fn()->u64,g:fn()->u64,flag:bool)->u64 uses {g} {let outer=fn()->u64 {let inner=fn()->u64 {if flag {g()} else {g()}};inner()};outer()}
pub fn main()->Result[(),()] uses {} {let saved:fn()->u64 uses {}=seven;let callbacks:List[fn()->u64 uses {}]=[saved,twentynine];if choose(callbacks[0],callbacks[1],true)!=29u64 {return Error(())};if choose(callbacks[0],callbacks[1],false)!=29u64 {return Error(())};Ok(())}
"#,
    );
}

#[test]
fn stored_callable_authority_26_runtime_7() {
    runtime_case(
        "S-select-g-3-1-setup3",
        r#"fn seven()->u64 uses {} {7}
fn twentynine()->u64 uses {} {29}
fn choose(f:fn()->u64,g:fn()->u64,flag:bool)->u64 uses {g} {let first=g;let outer=fn()->u64 {let next=first;let inner=fn()->u64 {next()};inner()};outer()}
pub fn main()->Result[(),()] uses {} {let saved=seven;let callbacks:List[fn()->u64 uses {}]=[saved,twentynine];if choose(callbacks[0],callbacks[1],true)!=29u64 {return Error(())};if choose(callbacks[0],callbacks[1],false)!=29u64 {return Error(())};Ok(())}
"#,
    );
}

// Rebuild only graph edges that point at the changed immutable callable row.
// Consistent aliases are bookkeeping; each mutant changes one logical field.
fn replace_row(types: &TypeInterner, root: TypeId, old: TypeId, new: TypeId) -> TypeId {
    if root == old {
        return new;
    }
    let mut row = types.get(root).unwrap().clone();
    row.kind = row
        .kind
        .map_edges(|edge| replace_row(types, edge, old, new));
    types.intern(row)
}
fn mutated_definition(
    input: &NativeInput,
    name: &str,
    field: &str,
) -> (hirsel_tir::Definition, hirsel_validate::SignatureMap) {
    use hirsel_tir::*;
    let path = input.definitions.keys().find(|p| p.0.name == name).unwrap();
    let mut def = input.definitions[path].as_ref().clone();
    let mut signatures = hirsel_validate::SignatureMap::new(input.types.clone());
    signatures.extend_signatures(input.signatures.values().map(|s| s.as_ref().clone()));
    {
        let old=def.bodies.iter().flat_map(|b|b.blocks.iter().flat_map(move |block|block.instructions.iter().filter_map(move |inst| {
            let InstructionKind::Assign{destination,value}=&inst.kind else{return None};
            matches!(&b.rvalues[value.0 as usize],Rvalue::Closure{captures,..} if field=="selected" || !captures.is_empty()).then_some(b.places[destination.0 as usize].ty)
        }))).next().unwrap();
        let mut row = input.types.get(old).unwrap().clone();
        let TypeKind::Function(f) = &mut row.kind else {
            unreachable!()
        };
        if field == "uses" {
            let Uses::Only(AuthoritySummary::Known(terms)) = &mut f.uses else {
                panic!("{f:?}")
            };
            let before = terms.len();
            *terms = terms
                .iter()
                .filter(|t| !matches!(t, AuthorityTerm::Capability(_)))
                .cloned()
                .collect();
            assert_eq!(terms.len(), before - 1);
        } else if field == "selected" {
            let Uses::Only(AuthoritySummary::Known(terms)) = &mut f.uses else {
                panic!("{f:?}")
            };
            let mut changed = 0;
            for term in terms.iter_mut() {
                if let AuthorityTerm::Parameter(ParameterRef::Bound {
                    depth: 0,
                    parameter,
                }) = term
                {
                    parameter.0 = 1 - parameter.0;
                    changed += 1;
                }
            }
            assert_eq!(changed, 1);
            *terms = terms
                .iter()
                .cloned()
                .collect::<std::collections::BTreeSet<_>>()
                .into_iter()
                .collect();
        } else if field == "captures" {
            assert_ne!(f.authority, AuthoritySummary::inert());
            f.authority = AuthoritySummary::inert();
        } else {
            assert_eq!(field, "slot");
            let capture = def
                .bodies
                .iter_mut()
                .find(|b| b.locals.iter().any(|l| l.storage == Storage::Capture))
                .unwrap();
            let capture_old = capture
                .locals
                .iter()
                .find(|l| {
                    l.storage == Storage::Capture
                        && matches!(input.types.get(l.ty).unwrap().kind, TypeKind::Function(_))
                })
                .unwrap()
                .ty;
            let mut row = input.types.get(capture_old).unwrap().clone();
            let TypeKind::Function(held) = &mut row.kind else {
                unreachable!()
            };
            assert_ne!(held.authority, AuthoritySummary::inert());
            held.authority = AuthoritySummary::inert();
            let new = input.types.intern(row);
            for local in &mut capture.locals {
                local.ty = replace_row(&input.types, local.ty, capture_old, new);
            }
            for place in &mut capture.places {
                place.ty = replace_row(&input.types, place.ty, capture_old, new);
            }
            def.types = definition_types(&input.types, &def);
            return (def, signatures);
        }
        let new = input.types.intern(row);
        for body in &mut def.bodies {
            body.result = replace_row(&input.types, body.result, old, new);
            for local in &mut body.locals {
                local.ty = replace_row(&input.types, local.ty, old, new);
            }
            for place in &mut body.places {
                place.ty = replace_row(&input.types, place.ty, old, new);
            }
        }
        def.types = definition_types(&input.types, &def);
    }
    (def, signatures)
}
fn mutant_case(id: &str, source: &str, fields: &[&str]) {
    with_compiler_stack(|| {
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
        let file = engine.sources.add("mutant.hir", source.into()).unwrap();
        let checked = check_progress(&mut engine, file, &default_context()).unwrap();
        assert!(
            checked.diagnostics.is_empty(),
            "{id}: {:?}",
            checked.diagnostics
        );
        let input = engine.export_native_input().unwrap();
        let mut signatures = hirsel_validate::SignatureMap::new(input.types.clone());
        signatures.extend_signatures(input.signatures.values().map(|s| s.as_ref().clone()));
        let base = input
            .definitions
            .iter()
            .find(|(p, _)| p.0.name == "choose")
            .unwrap()
            .1;
        let valid = hirsel_validate::validate(base, &input.types, &signatures, &mut vec![]);
        assert!(valid.violations.is_empty(), "{id}: {valid:?}");
        println!(
            "AUTH26 {}",
            serde_json::json!({"id":id,"engine":"V-base","observed":"valid"})
        );
        for field in fields {
            let (bad, signatures) = mutated_definition(&input, "choose", field);
            let got = hirsel_validate::validate(&bad, &input.types, &signatures, &mut vec![]);
            println!(
                "AUTH26 {}",
                serde_json::json!({"id":format!("{id}-{field}"),"engine":"V-mutant","observed":format!("{:?}",got.violations)})
            );
            let expected = if *field == "selected" {
                "body uses"
            } else if *field == "uses" {
                "closure uses"
            } else if *field == "captures" {
                "closure authority"
            } else {
                "closure capture"
            };
            assert!(
                got.violations
                    .iter()
                    .any(|v| v.kind
                        == hirsel_validate::ViolationKind::TypeMismatch { entity: expected }),
                "{id}-{field}: {got:?}"
            );
            assert!(
                got.violations
                    .iter()
                    .all(|v| matches!(v.kind, hirsel_validate::ViolationKind::TypeMismatch { .. })),
                "unrelated malformed proof: {got:?}"
            );
        }
    });
}

#[test]
fn stored_callable_authority_26_mutations_0() {
    mutant_case(
        "V-capture-0-0",
        r#"fn effect(arg:Dir) uses {Dir} {}
fn choose(d:Dir) uses {Dir} {let held=effect;let callback=fn(token:Dir) uses {Dir} {held(token)};callback(d)}
pub fn main(d:Dir) uses {Dir} {choose(d)}
"#,
        &["uses", "captures", "slot"],
    );
}

#[test]
fn stored_callable_authority_26_mutations_1() {
    mutant_case(
        "V-capture-0-1",
        r#"fn effect(arg:Dir) uses {Dir} {}
fn choose(d:Dir) uses {Dir} {let held=effect;let callback:fn(Dir)->() uses {Dir}=fn(token:Dir) uses {Dir} {held(token)};callback(d)}
pub fn main(d:Dir) uses {Dir} {choose(d)}
"#,
        &["uses", "captures", "slot"],
    );
}

#[test]
fn stored_callable_authority_26_mutations_2() {
    mutant_case(
        "V-capture-1-0",
        r#"fn effect(arg:Dir) uses {Dir} {}
fn choose(d:Dir) uses {Dir} {let held:fn(Dir)->() uses {Dir}=effect;let callback=fn(token:Dir) uses {Dir} {held(token)};callback(d)}
pub fn main(d:Dir) uses {Dir} {choose(d)}
"#,
        &["uses", "captures", "slot"],
    );
}

#[test]
fn stored_callable_authority_26_mutations_3() {
    mutant_case(
        "V-capture-1-1",
        r#"fn effect(arg:Dir) uses {Dir} {}
fn choose(d:Dir) uses {Dir} {let held:fn(Dir)->() uses {Dir}=effect;let callback:fn(Dir)->() uses {Dir}=fn(token:Dir) uses {Dir} {held(token)};callback(d)}
pub fn main(d:Dir) uses {Dir} {choose(d)}
"#,
        &["uses", "captures", "slot"],
    );
}

fn paired_v_negative(id: &str, input: &NativeInput, signatures: &hirsel_validate::SignatureMap) {
    use hirsel_tir::*;
    let (path, original) = if id.contains("select") {
        input
            .definitions
            .iter()
            .find(|(p, _)| p.0.name == "choose")
            .unwrap()
    } else {
        input
            .definitions
            .iter()
            .find(|(_, d)| {
                d.bodies.iter().any(|b| {
                    b.rvalues.iter().any(|v| {
                        matches!(v, Rvalue::Closure { .. } | Rvalue::FunctionReference { .. })
                    })
                })
            })
            .unwrap()
    };
    let mut def = original.as_ref().clone();
    let mut sigs = hirsel_validate::SignatureMap::new(input.types.clone());
    for s in input.signatures.values() {
        let mut s = s.as_ref().clone();
        if id.contains("select") && s.definition.path() == path {
            let SignatureKind::Function(f) = &mut s.kind else {
                unreachable!()
            };
            let Uses::Only(AuthoritySummary::Known(terms)) = &mut f.uses else {
                unreachable!()
            };
            assert_eq!(terms.len(), 1);
            let AuthorityTerm::Parameter(ParameterRef::Bound {
                depth: 0,
                parameter,
            }) = &mut terms[0]
            else {
                unreachable!()
            };
            parameter.0 = 1 - parameter.0;
        }
        sigs.insert(s.definition, s);
    }
    if !id.contains("select") {
        let old = def
            .bodies
            .iter()
            .find_map(|b| {
                b.blocks
                    .iter()
                    .flat_map(|block| &block.instructions)
                    .find_map(|inst| {
                        let InstructionKind::Assign { destination, value } = &inst.kind else {
                            return None;
                        };
                        let ty = b.places[destination.0 as usize].ty;
                        (matches!(
                            &b.rvalues[value.0 as usize],
                            Rvalue::Closure { .. } | Rvalue::FunctionReference { .. }
                        ) && matches!(input.types.get(ty).unwrap().kind, TypeKind::Function(_)))
                        .then_some(ty)
                    })
            })
            .unwrap();
        let mut row = input.types.get(old).unwrap().clone();
        let TypeKind::Function(f) = &mut row.kind else {
            unreachable!()
        };
        assert_ne!(f.uses, Uses::Only(AuthoritySummary::inert()));
        f.uses = Uses::Only(AuthoritySummary::inert());
        let new = input.types.intern(row);
        for body in &mut def.bodies {
            body.result = replace_row(&input.types, body.result, old, new);
            for l in &mut body.locals {
                l.ty = replace_row(&input.types, l.ty, old, new);
            }
            for p in &mut body.places {
                p.ty = replace_row(&input.types, p.ty, old, new);
            }
        }
        def.types = definition_types(&input.types, &def);
    }
    let base = hirsel_validate::validate(original, &input.types, signatures, &mut vec![]);
    assert!(base.violations.is_empty(), "{id}: {base:?}");
    let got = hirsel_validate::validate(&def, &input.types, &sigs, &mut vec![]);
    println!(
        "AUTH26 {}",
        serde_json::json!({"id":id,"engine":"V-negative-twin","observed":format!("{:?}",got.violations)})
    );
    assert!(
        !got.violations.is_empty(),
        "{id}: missing independent V refusal"
    );
    assert!(
        got.violations
            .iter()
            .all(|v| matches!(v.kind, hirsel_validate::ViolationKind::TypeMismatch { .. })),
        "{id}: unrelated malformed proof: {got:?}"
    );
    if id.contains("select") {
        assert!(
            got.violations.iter().any(|v| v.kind
                == hirsel_validate::ViolationKind::TypeMismatch {
                    entity: "body uses"
                }),
            "{id}: {got:?}"
        );
    }
}

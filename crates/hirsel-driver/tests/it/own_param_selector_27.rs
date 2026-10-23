//! CLO-2/CLO-4: selectors bind to each callable's own declaration slots.
use hirsel_driver::*;
use hirsel_engine::*;

fn check(source: &str, expected: Option<&str>) {
    with_compiler_stack(|| {
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
        let file = engine
            .sources
            .add("own-selector.hir", source.into())
            .unwrap();
        let output = check_progress(&mut engine, file, &default_context()).unwrap();
        assert_eq!(output.completion, CheckCompletion::Complete);
        let rules: Vec<_> = output.diagnostics.iter().map(|d| d.rule.as_str()).collect();
        assert_eq!(
            rules.first().copied(),
            expected,
            "{source}\n{:?}",
            output.diagnostics
        );
        if expected.is_none() {
            let input = engine.export_native_input().unwrap();
            let mut signatures = hirsel_validate::SignatureMap::new(input.types.clone());
            signatures.extend_signatures(input.signatures.values().map(|s| s.as_ref().clone()));
            for definition in input.definitions.values() {
                let report =
                    hirsel_validate::validate(definition, &input.types, &signatures, &mut vec![]);
                assert!(report.violations.is_empty(), "{source}\n{report:?}");
            }
        }
    });
}

#[test]
fn own_parameter_type() {
    check(
        "fn effect(d:Dir) uses {d} {}\npub fn main(d:Dir) uses {d} {let f:fn(arg:Dir)->() uses {arg}=effect;f(d)}",
        None,
    );
}

#[test]
fn own_parameter_literal() {
    check(
        "fn pure() uses {} {}\nfn choose() uses {} {let callback=fn(f:fn()->(),g:fn()->()) uses {f} {f()};callback(pure,pure)}\npub fn main() uses {} {choose()}",
        None,
    );
}

#[test]
fn named_type_twin() {
    check(
        r#"fn effect(d:Dir) uses {d} {}
pub fn main(d:Dir) uses {d} {effect(d)}"#,
        None,
    );
}

#[test]
fn named_literal_twin() {
    check(
        r#"fn pure() uses {} {}
fn choose(f:fn()->(),g:fn()->()) uses {f} {f()}
pub fn main() uses {} {choose(pure,pure)}"#,
        None,
    );
}

#[test]
fn selector_to_nominal() {
    check(
        r#"fn effect(d:Dir) uses {d} {}
pub fn main(d:Dir) uses {d} {let f:fn(arg:Dir)->() uses {Dir}=effect;f(d)}"#,
        None,
    );
}

#[test]
fn nominal_to_selector() {
    check(
        r#"fn effect(d:Dir) uses {Dir} {}
pub fn main(d:Dir) uses {d} {let f:fn(arg:Dir)->() uses {arg}=effect;f(d)}"#,
        None,
    );
}

#[test]
fn renamed_second_parameter() {
    check(
        r#"fn effect(n:u64,d:Dir) uses {d} {}
pub fn main(d:Dir) uses {d} {let f:fn(n:u64,arg:Dir)->() uses {arg}=effect;f(3,d)}"#,
        None,
    );
}

#[test]
fn type_actual_exceeds_caller() {
    check(
        r#"fn effect(d:Dir) uses {d} {}
pub fn main(d:Dir) uses {} {let f:fn(arg:Dir)->() uses {arg}=effect;f(d)}"#,
        Some("CAP-1"),
    );
}

#[test]
fn literal_actual_exceeds_caller() {
    check(
        r#"pub fn main(d:Dir) uses {} {let callback=fn(arg:Dir) uses {arg} {};callback(d)}"#,
        Some("CLO-2"),
    );
}

#[test]
fn named_actual_exceeds_caller() {
    check(
        r#"fn effect(d:Dir) uses {d} {}
pub fn main(d:Dir) uses {} {effect(d)}"#,
        Some("CAP-1"),
    );
}

#[test]
fn literal_unselected_sibling() {
    check(
        r#"pub fn main() uses {} {let callback=fn(f:fn()->(),g:fn()->()) uses {f} {g()}}"#,
        Some("CAP-3"),
    );
}

#[test]
fn literal_plain_selector_does_not_authorize_callback() {
    check(
        r#"pub fn main() uses {} {let callback=fn(n:u64,f:fn()->()) uses {n} {f()}}"#,
        Some("CAP-3"),
    );
}

#[test]
fn type_plain_selector_does_not_authorize_callback() {
    check(
        r#"fn effect(n:u64) uses {Dir} {}
pub fn main() uses {Dir} {let f:fn(n:u64)->() uses {n}=effect;f(1)}"#,
        Some("CLO-4"),
    );
}

#[test]
fn plain_parameter_inert() {
    check(
        r#"fn pure(n:u64) uses {n} {}
pub fn main() uses {} {let f:fn(arg:u64)->() uses {arg}=pure;f(1);let g=fn(n:u64) uses {n} {};g(2)}"#,
        None,
    );
}

#[test]
fn misspelled_type_selector() {
    check(
        r#"fn pure(n:u64) uses {} {}
pub fn main() {let f:fn(arg:u64)->() uses {missing}=pure}"#,
        Some("LANG-4"),
    );
}

#[test]
fn misspelled_literal_selector() {
    check(
        r#"pub fn main() {let f=fn(arg:u64) uses {missing} {}}"#,
        Some("LANG-4"),
    );
}

#[test]
fn noncallable_invocation() {
    check(
        r#"pub fn main() {let f=fn(arg:u64) uses {arg} {arg()}}"#,
        Some("LANG-7"),
    );
}

#[test]
fn selector_callback_identity() {
    check(
        r#"fn choose(f:fn()->(),g:fn()->()) uses {f} {f()}
pub fn main() {let held:fn(f:fn()->(),g:fn()->())->() uses {f}=choose}"#,
        None,
    );
}

#[test]
fn callback_sibling_subsumption() {
    check(
        r#"fn choose(f:fn()->(),g:fn()->()) uses {g} {g()}
pub fn main() {let held:fn(f:fn()->(),g:fn()->())->() uses {f}=choose}"#,
        Some("CLO-4"),
    );
}

use hirsel_types::*;

// Rebind aliases of one immutable row; only its own selector changes.
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

#[test]
fn verifier_rejects_forged_own_selectors() {
    use hirsel_tir::*;
    with_compiler_stack(|| {
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
        let file = engine.sources.add("forged-selector.hir", "fn pure() uses {} {}\nfn choose() uses {} {let callback=fn(f:fn()->(),g:fn()->()) uses {f} {f()};callback(pure,pure)}\npub fn main() uses {} {choose()}".into()).unwrap();
        let checked = check_progress(&mut engine, file, &default_context()).unwrap();
        assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
        let input = engine.export_native_input().unwrap();
        let mut signatures = hirsel_validate::SignatureMap::new(input.types.clone());
        signatures.extend_signatures(input.signatures.values().map(|s| s.as_ref().clone()));
        let original = input
            .definitions
            .iter()
            .find(|(p, _)| p.0.name == "choose")
            .unwrap()
            .1;
        let valid = hirsel_validate::validate(original, &input.types, &signatures, &mut vec![]);
        assert!(valid.violations.is_empty(), "{valid:?}");
        let old = original
            .bodies
            .iter()
            .find_map(|body| {
                body.blocks
                    .iter()
                    .flat_map(|b| &b.instructions)
                    .find_map(|instruction| {
                        let InstructionKind::Assign { destination, value } = instruction.kind
                        else {
                            return None;
                        };
                        matches!(body.rvalues[value.0 as usize], Rvalue::Closure { .. })
                            .then_some(body.places[destination.0 as usize].ty)
                    })
            })
            .unwrap();
        for selected in [1, 99] {
            let mut row = input.types.get(old).unwrap().clone();
            let TypeKind::Function(function) = &mut row.kind else {
                unreachable!()
            };
            function.uses = Uses::Only(AuthoritySummary::Known(
                vec![AuthorityTerm::Parameter(ParameterRef::bound(ParamId(
                    selected,
                )))]
                .into(),
            ));
            let new = input.types.intern(row);
            let mut forged = original.as_ref().clone();
            for body in &mut forged.bodies {
                body.result = replace_row(&input.types, body.result, old, new);
                for local in &mut body.locals {
                    local.ty = replace_row(&input.types, local.ty, old, new);
                }
                for place in &mut body.places {
                    place.ty = replace_row(&input.types, place.ty, old, new);
                }
            }
            forged.types = definition_types(&input.types, &forged);
            let report = hirsel_validate::validate(&forged, &input.types, &signatures, &mut vec![]);
            println!("OWN27 V selector={selected}: {:?}", report.violations);
            assert!(
                !report.violations.is_empty(),
                "forged selector {selected} accepted"
            );
            if selected == 1 {
                assert!(
                    report.violations.iter().any(|v| v.kind
                        == hirsel_validate::ViolationKind::TypeMismatch {
                            entity: "body uses"
                        }),
                    "{report:?}"
                );
            }
        }
    });
}

#[path = "../support/history.rs"]
#[allow(dead_code)]
mod history;

fn runtime_case(name: &str, source: &str) {
    let directory = tempfile::tempdir().unwrap();
    let runner = history::Runner::new(directory.path()).unwrap();
    let report = runner.run(name, source);
    assert!(report.validated, "{report:?}");
    assert_eq!(report.cells.len(), 4, "{report:?}");
    for cell in report.cells {
        println!(
            "OWN27 {name} {} {:?} {:?}",
            cell.engine, cell.category, cell.observation
        );
        assert_eq!(cell.category, history::Category::Observation, "{cell:?}");
        let observed = cell.observation.unwrap();
        assert_eq!(observed.exit_code, Some(0));
        assert!(
            observed.output.is_empty()
                && observed.failures.is_empty()
                && !observed.blocked
                && observed.budget.is_none()
                && observed.signaled.is_none()
                && observed.roots.is_empty()
        );
    }
}

#[test]
fn runtime_literal_and_named_twin() {
    runtime_case(
        "literal",
        r#"
fn seven()->u64 uses {} {7}
fn other()->u64 uses {} {29}
fn named(f:fn()->u64,g:fn()->u64)->u64 uses {f} {f()}
pub fn main()->Result[(),()] uses {} {
  let callback=fn(f:fn()->u64,g:fn()->u64)->u64 uses {f} {f()}
  if callback(seven,other)!=7u64 {return Error(())}
  if named(seven,other)!=7u64 {return Error(())}
  let second=fn(f:fn()->u64,g:fn()->u64)->u64 uses {g} {g()}
  if second(seven,other)!=29u64 {return Error(())}
  Ok(())
}
"#,
    );
}

#[test]
fn runtime_type_and_named_twin() {
    runtime_case(
        "type",
        r#"
fn effect(d:Dir)->u64 uses {d} {17}
fn named(d:Dir)->u64 uses {d} {effect(d)}
pub fn main(t:Test)->Result[(),()] {
  let d=test.dir(t,[])
  let f:fn(arg:Dir)->u64 uses {arg}=effect
  if f(d)!=17u64 {return Error(())}
  if named(d)!=17u64 {return Error(())}
  Ok(())
}
"#,
    );
}

#[test]
fn runtime_subsumption_both_directions() {
    runtime_case(
        "subsumption",
        r#"
fn selected(d:Dir)->u64 uses {d} {17}
fn nominal(d:Dir)->u64 uses {Dir} {29}
pub fn main(t:Test)->Result[(),()] {
  let d=test.dir(t,[])
  let f:fn(arg:Dir)->u64 uses {Dir}=selected
  let g:fn(arg:Dir)->u64 uses {arg}=nominal
  if f(d)!=17u64 {return Error(())}
  if g(d)!=29u64 {return Error(())}
  Ok(())
}
"#,
    );
}

#[test]
fn runtime_second_slot_and_inert_selectors() {
    runtime_case(
        "remaining-controls",
        r#"
fn effect(n:u64,d:Dir)->u64 uses {d} {n+17}
fn pure(n:u64)->u64 uses {n} {n}
fn seven()->u64 uses {} {7}
fn other()->u64 uses {} {29}
fn choose(f:fn()->u64,g:fn()->u64)->u64 uses {f} {f()}
pub fn main(t:Test)->Result[(),()] {
  let d=test.dir(t,[])
  let second:fn(n:u64,arg:Dir)->u64 uses {arg}=effect
  if second(3,d)!=20u64 {return Error(())}
  let inert:fn(arg:u64)->u64 uses {arg}=pure
  if inert(11)!=11u64 {return Error(())}
  let literal=fn(n:u64)->u64 uses {n} {n}
  if literal(13)!=13u64 {return Error(())}
  let selected:fn(f:fn()->u64,g:fn()->u64)->u64 uses {f}=choose
  if selected(seven,other)!=7u64 {return Error(())}
  Ok(())
}
"#,
    );
}

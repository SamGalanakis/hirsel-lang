//! Creation owns copied captures; Many invocation operands cannot take them.
use hirsel_driver::*;
use hirsel_engine::*;
use hirsel_tir::*;
use hirsel_types::{TypeKind, UseKind};

#[test]
fn v_rejects_taking_a_copied_capture_from_a_many_invocation() {
    with_compiler_stack(|| {
        for source in [
            r#"fn make()->fn()->u64 {
                let text:String in heap="x"; let pair=(text,1u8);
                fn(){let(s,n)=pair;string.byte_len(s)}
            }
            fn main(){}"#,
            r#"fn make()->fn()->fn()->u64 {
                let cb:fn()->u64=fn(){7u64}; fn(){cb}
            }
            fn main(){}"#,
        ] {
            let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
            let file = engine
                .sources
                .add("capture-take.hir", source.into())
                .unwrap();
            let output = check_progress(&mut engine, file, &default_context()).unwrap();
            assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
            let input = engine.export_native_input().unwrap();
            let mut signatures = hirsel_validate::SignatureMap::new(input.types.clone());
            signatures.extend_signatures(input.signatures.values().map(|s| s.as_ref().clone()));
            let mut make = entry_path(&default_context());
            make.0.name = "make".into();
            let original = &input.definitions[&make];
            assert!(
                hirsel_validate::validate(original, &input.types, &signatures, &mut vec![])
                    .violations
                    .is_empty()
            );
            let mut probes = 0;
            for (body_index, body) in original.bodies.iter().enumerate() {
                for (value_index, value) in body.rvalues.iter().enumerate() {
                    let Rvalue::Use(operand) = value else {
                        continue;
                    };
                    let place = &body.places[operand.place.0 as usize];
                    if operand.usage != UseKind::Copy
                        || body.locals[place.root.0 as usize].storage != Storage::Capture
                        || !place.projection.is_empty()
                        || !matches!(
                            input.types.row(place.ty).kind,
                            TypeKind::Tuple(_) | TypeKind::Function(_)
                        )
                    {
                        continue;
                    }
                    let mut forged = (**original).clone();
                    let Rvalue::Use(taken) = &mut forged.bodies[body_index].rvalues[value_index]
                    else {
                        unreachable!()
                    };
                    taken.usage = UseKind::Take;
                    let report =
                        hirsel_validate::validate(&forged, &input.types, &signatures, &mut vec![]);
                    assert!(report.violations.iter().any(|v| matches!(v.kind, hirsel_validate::ViolationKind::BorrowedMove(p) if p == operand.place)), "{report:?}");
                    probes += 1;
                }
            }
            assert!(probes > 0, "no copied capture transfer in {source}");
        }
    });
}

#[test]
fn a_written_many_contract_rejects_taking_its_capture() {
    with_compiler_stack(|| {
        for source in [
            r#"fn make()->fn()->String {let text:String in heap="x";fn(){take text}}
fn main(){}"#,
            r#"fn consume(take text:String){}
fn make()->fn()->() {let text:String in heap="x";fn(){consume(take text)}}
fn main(){}"#,
        ] {
            let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
            let file = engine
                .sources
                .add("capture-take-bad.hir", source.into())
                .unwrap();
            let output = check_progress(&mut engine, file, &default_context()).unwrap();
            assert!(
                output
                    .diagnostics
                    .iter()
                    .any(|d| d.rule == hirsel_diag::RuleId::of("MODE-3")),
                "{:?}",
                output.diagnostics
            );
        }
    });
}

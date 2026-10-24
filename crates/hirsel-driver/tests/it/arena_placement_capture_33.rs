//! Placement-only Arena names are real captures with independent V transport.
use hirsel_driver::*;
use hirsel_engine::*;
use hirsel_tir::{Rvalue, Storage};
use hirsel_types::{OriginRoot, ParameterRef, TypeKind};

fn prepared(source: &str) -> (QueryEngine, NativeInput) {
    let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
    let file = engine
        .sources
        .add("arena-placement-capture.hir", source.into())
        .unwrap();
    let output = check_progress(&mut engine, file, &default_context()).unwrap();
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let input = engine.export_native_input().unwrap();
    engine.import_native_input(input.clone()).unwrap();
    (engine, input)
}

fn placement_mutations(mutations: std::ops::Range<usize>) {
    with_compiler_stack(|| {
        let (engine, input) = prepared(
            "pub fn main(){arena.with(fn(a:Arena){let callback=fn()->u64{let xs:List[u64] in a=[9u64];xs[0]};let n=callback();});}",
        );
        let definition = input.definitions[&entry_path(&default_context())].clone();
        let mut signatures = hirsel_validate::SignatureMap::new(input.types.clone());
        signatures.extend_signatures(input.signatures.values().map(|s| s.as_ref().clone()));
        hirsel_validate::assert_valid(&definition, engine.types(), &signatures);
        let arena = |ty| matches!(&input.types.get(ty).unwrap().kind, TypeKind::Named { definition, .. } if definition.path().is_core("Arena"));
        let (body, token) = definition
            .bodies
            .iter()
            .enumerate()
            .find_map(|(b, body)| {
                body.locals.iter().enumerate().find_map(|(i, local)| {
                    (local.storage == Storage::Capture && arena(local.ty))
                        .then_some((b, hirsel_tir::LocalId(i as u32)))
                })
            })
            .expect("placement-only capture supplies Arena storage");
        assert_eq!(
            definition.bodies[body]
                .locals
                .iter()
                .filter(|l| l.storage == Storage::Capture)
                .count(),
            1
        );
        for mutation in mutations {
            let mut forged = (*definition).clone();
            if mutation < 2 {
                for parent in &mut forged.bodies {
                    for value in &mut parent.rvalues {
                        if let Rvalue::Closure {
                            body: child,
                            captures,
                            ..
                        } = value
                            && child.0 as usize == body
                        {
                            assert_eq!(captures.len(), 1);
                            captures.clear();
                        }
                    }
                }
                let changed = &mut forged.bodies[body];
                changed.locals[token.0 as usize].storage = Storage::Temporary;
                if mutation == 0 {
                    for place in &mut changed.places {
                        if place.root == token {
                            place.binding = None;
                        }
                    }
                }
            } else {
                let wrong_scope = forged.bodies[0].parameter_scope;
                for value in &mut forged.bodies[body].rvalues {
                    if let Rvalue::Construct {
                        destination: hirsel_types::Destination::Arena(origin),
                        ..
                    } = value
                    {
                        let OriginRoot::Parameter(ParameterRef::Body { scope, parameter }) =
                            &mut origin.root
                        else {
                            panic!("Arena origin is scoped");
                        };
                        match mutation {
                            2 => *scope = wrong_scope,
                            3 => scope.owner.occurrence += 1,
                            4 => *parameter = hirsel_types::ParamId(99),
                            _ => unreachable!(),
                        }
                    }
                }
            }
            forged.types = hirsel_tir::definition_types(engine.types(), &forged);
            let report =
                hirsel_validate::validate(&forged, engine.types(), &signatures, &mut vec![]);
            assert!(
                report.violations.iter().any(|v| match (&v.kind, mutation) {
                    (hirsel_validate::ViolationKind::ReadBeforeInit(_), 1) => true,
                    (
                        hirsel_validate::ViolationKind::TypeMismatch {
                            entity: "constructor placement",
                        },
                        2..=4,
                    ) => true,
                    (
                        hirsel_validate::ViolationKind::TypeMismatch {
                            entity: "allocation arena token",
                        },
                        _,
                    ) => true,
                    _ => false,
                }),
                "mutation={mutation}: {report:?}"
            );
        }
    });
}

#[test]
fn arena_placement_capture_33_local_annotation_transports_exact_token_and_v_refuses_forgery() {
    placement_mutations(0..2)
}

#[test]
fn arena_placement_capture_33_unknown_owner_or_scope_is_refused_before_flow() {
    placement_mutations(2..5)
}

#[test]
fn arena_placement_capture_33_written_result_transports_its_outer_arena() {
    with_compiler_stack(|| {
        let (_, input) = prepared(
            "pub fn main(){arena.with(fn(a:Arena){let callback=fn()->List[u64] in a{[9u64]};let xs=callback();});}",
        );
        assert!(
            input.definitions[&entry_path(&default_context())]
                .bodies
                .iter()
                .any(|body| body.locals.iter().any(|l| l.storage == Storage::Capture))
        );
    });
}

#[test]
fn arena_placement_capture_33_scope_grants_have_no_handle_drop_observation() {
    with_compiler_stack(|| {
        let (mut engine, _) =
            prepared("pub fn main(){arena.with(fn(a:Arena){let xs:List[u64] in a=[9u64];});}");
        for checked in [false, true] {
            let runtime = if checked {
                engine.checked_runtime_types()
            } else {
                engine.runtime_types()
            }
            .unwrap();
            let run = RunConfiguration {
                entry: entry_path(&default_context()),
                hash_seed: 0,
                inputs: vec![],
            };
            let sources = engine.sources.clone();
            let mut library =
                hirsel_interp::library::Library::new(&run, SimulationConfig::default()).unwrap();
            let execution = hirsel_interp::run_with_runtime(
                &mut engine,
                &sources,
                &run,
                vec![],
                Default::default(),
                &mut library,
                &runtime,
            )
            .unwrap();
            assert!(
                matches!(
                    execution.outcome,
                    hirsel_interp::Outcome::Returned { exit_code: 0, .. }
                ),
                "checked={checked}: {:?}",
                execution.outcome
            );
            assert!(
                !execution.trace.iter().any(|event| matches!(event,
                    hirsel_interp::TraceEvent::Effect(hirsel_interp::observation::OrderedEffect {
                        effect: hirsel_interp::observation::Effect::HandleDropped { kind, .. }, ..
                    }) if kind == "Arena"
                )),
                "an internal scope grant must not publish a handle event"
            );
        }
    });
}

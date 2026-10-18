use hirsel_driver::*;
use hirsel_engine::*;
use std::sync::Arc;
fn engine(text: &str) -> QueryEngine {
    let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
    let file = engine.sources.add("snapshot.hir", text.into()).unwrap();
    let result = check_progress(&mut engine, file, &default_context()).unwrap();
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    engine
}

#[test]
fn native_selected_source_survives_call_and_aggregate_transfer_before_unique_operations() {
    use hirsel_syntax::UnaryOp;
    use hirsel_tir::Rvalue;
    use hirsel_types::UseKind;
    with_compiler_stack(|| {
        engine("fn view(x:List[Bytes])->List[Bytes] in x{x}\nfn probe(take xs:unique List[Bytes]){let kept=(copy view(xs),);let frozen=shared.freeze(unique xs)}")
            .export_native_input().unwrap();
        for view in [
            "view(xs)",
            "field(xs).value",
            "identity(view(xs))",
            "{let f:fn(x:List[Bytes])->List[Bytes] in x=view;f(xs)}",
        ] {
            let prelude = "fn view(x:List[Bytes])->List[Bytes] in x{x}\nfn identity[T:copyable](x:T)->T in x{x}\ntype Wrap{value:List[Bytes]}\nfn field(x:List[Bytes])->Wrap in x{Wrap(value:copy x)}\nfn change(mut xs:unique List[Bytes]){xs=[]}\nfn send[T:sendable](take x:T){}\n";
            for (action, rule) in [
                ("change(mut xs)", "MODE-1"),
                ("let frozen=shared.freeze(take xs)", "MODE-1"),
                ("send(take xs)", "MODE-1"),
            ] {
                // `kept` is used after the action, so its loan is still live (LANG-3).
                let body = format!(
                    "{prelude}fn probe(take input:unique List[Bytes]){{let mut xs:unique List[Bytes]=take input;let kept=(copy {view},);{action};let _=kept}}"
                );
                let mut rejected = QueryEngine::new(default_environment(), compiler_adapters());
                let file = rejected.sources.add("reject.hir", body.clone()).unwrap();
                let output = check_progress(&mut rejected, file, &default_context()).unwrap();
                assert_eq!(
                    output.diagnostics[0].rule.as_str(),
                    rule,
                    "{body}: {:?}",
                    output.diagnostics
                );
                let site = action.strip_prefix("let frozen=").unwrap_or(action);
                assert_eq!(
                    rejected.sources.slice(output.diagnostics[0].primary),
                    Some(site)
                );
                let expired = format!(
                    "{prelude}fn probe(take input:unique List[Bytes]){{let mut xs:unique List[Bytes]=take input;{{let kept=(copy {view},);}};{action}}}"
                );
                let mut rejected = QueryEngine::new(default_environment(), compiler_adapters());
                let file = rejected
                    .sources
                    .add("expired.hir", expired.clone())
                    .unwrap();
                let output = check_progress(&mut rejected, file, &default_context()).unwrap();
                // MODE-2: the dead `kept` held only a direct call's related
                // share, which ends with it; a function value's may be kept.
                let expected = if action.starts_with("change") {
                    view.starts_with('{').then_some("MODE-2")
                } else if action.starts_with("let frozen") {
                    view.starts_with('{').then_some("MEM-11")
                } else {
                    Some("MODE-4")
                };
                assert_eq!(
                    output.diagnostics.first().map(|d| d.rule.as_str()),
                    expected,
                    "{expired}: {:?}",
                    output.diagnostics
                );
                if expected.is_some() {
                    assert_eq!(
                        rejected.sources.slice(output.diagnostics[0].primary),
                        Some(site)
                    );
                }
                let operation = action.replace("xs", "fresh");
                let allowed = format!(
                    "{prelude}fn probe(take input:unique List[Bytes]){{let mut xs:unique List[Bytes]=take input;let kept=(copy {view},);let mut fresh=unique xs;{operation}}}"
                );
                engine(&allowed).export_native_input().unwrap();
                let allowed_expired = expired.replace(
                    &format!(";{action}"),
                    &format!(";let mut fresh=unique xs;{operation}"),
                );
                engine(&allowed_expired).export_native_input().unwrap();
            }
            let source = format!(
                "{prelude}fn probe(take input:unique List[Bytes]){{let mut xs:unique List[Bytes]=take input;let kept=(copy {view},);let mut fresh=unique xs;change(mut fresh);let frozen=shared.freeze(take fresh);send(take frozen)}}\npub fn main()->Result[(),()]{{let xs:List[Bytes]=[];probe(unique xs);Ok(())}}"
            );
            let mut producer = engine(&source);
            let input = producer.export_native_input().unwrap();
            producer.import_native_input(input.clone()).unwrap();
            let mut probe = entry_path(&default_context());
            probe.0.name = "probe".into();
            let signatures = input
                .signatures
                .iter()
                .map(|(p, s)| (p.clone(), (**s).clone()))
                .collect();
            let definition = &input.definitions[&probe];
            hirsel_validate::assert_valid(definition, &signatures);
            let mut forged = input.clone();
            let output = Arc::make_mut(forged.definitions.get_mut(&probe).unwrap());
            let mut count = 0;
            for body in &mut output.bodies {
                for value in &mut body.rvalues {
                    if let Rvalue::Unary {
                        op: UnaryOp::Unique,
                        value: source,
                    } = value
                    {
                        let mut source = source.clone();
                        source.usage = UseKind::Take;
                        *value = Rvalue::Use(source);
                        count += 1;
                    }
                }
            }
            assert_eq!(count, 1);
            assert!(
                !hirsel_validate::validate(output, &signatures)
                    .violations
                    .is_empty()
            );
            assert!(producer.import_native_input(forged).is_err());
            let observed = run_observed_snapshot(
                &mut producer,
                &RunConfiguration {
                    entry: entry_path(&default_context()),
                    hash_seed: 0,
                    inputs: vec![],
                },
                SimulationConfig::default(),
            )
            .unwrap();
            assert_eq!(observed.outcome.unwrap(), RunOutcome::Exit(0));
            assert!(observed.observation.unwrap().failures.is_empty());
        }
    });
}
#[test]
fn native_nested_partial_resources_finalize_once_in_reverse_order_on_every_exit() {
    use hirsel_interp::observation::{Effect, SemanticRole};
    use hirsel_tir::CleanupAction;
    with_compiler_stack(|| {
        for (call_fails, bad, failures) in [
            (false, 0, 0),
            (true, 0, 1),
            (false, 1, 1),
            (false, 3, 1),
            (true, 1, 2),
            (true, 3, 2),
        ] {
            let source = format!(
                r#"
resource type Token {{stdout:Stdout,n:u8,bad:bool}}
  drop(x) {{let _=out.print(x.stdout,if x.n==1 {{"one\n"}} else {{if x.n==2 {{"two\n"}} else {{"three\n"}}}});if x.bad {{let z:u8=0;let _=1u8/z}}}}
type Pair {{left:Token,right:Token}}
type Bundle {{pair:Pair,tail:Token}}
fn consume(take x:Token,fail:bool) {{if fail {{let z:u8=0;let _=1u8/z}}}}
fn probe(take p:Bundle,fail:bool) {{let Bundle(pair:Pair(left:a,right:b),tail:c)=take p;consume(take a,fail)}}
pub fn main(rt:Io)->Result[(),GrantError] {{
 let p=Bundle(pair:Pair(left:Token(stdout:io.stdout(rt)?,n:1,bad:{one}),right:Token(stdout:io.stdout(rt)?,n:2,bad:false)),tail:Token(stdout:io.stdout(rt)?,n:3,bad:{three}))
 probe(take p,{call_fails});Ok(())
}}
"#,
                one = bad == 1,
                three = bad == 3
            );
            let mut producer = engine(&source);
            let input = producer.export_native_input().unwrap();
            producer.import_native_input(input.clone()).unwrap();
            let observed = run_observed_snapshot(
                &mut producer,
                &RunConfiguration {
                    entry: entry_path(&default_context()),
                    hash_seed: 0,
                    inputs: vec![],
                },
                SimulationConfig::default(),
            )
            .unwrap();
            let outcome = observed.outcome.unwrap();
            let observation = observed.observation.unwrap();
            if failures == 0 {
                assert_eq!(outcome, RunOutcome::Exit(0))
            } else {
                assert!(matches!(outcome, RunOutcome::Failure { .. }))
            }
            assert_eq!(observation.output, b"one\nthree\ntwo\n");
            assert_eq!(observation.failures.len(), failures);
            let finalized: Vec<_> = observation
                .effects
                .iter()
                .filter(|effect| matches!(effect.effect, Effect::Finalizer { .. }))
                .collect();
            assert_eq!(finalized.len(), 3);
            assert!(
                finalized
                    .iter()
                    .all(|effect| effect.anchor.role == SemanticRole::FinalizerEntry)
            );
            for (index, failure) in observation.failures.iter().enumerate() {
                assert!(failure.spawn_path.is_empty());
                assert_eq!(failure.rule.as_str(), "SEM-1");
                assert_eq!(failure.kind, "DivisionByZero");
                let from_call = call_fails && index == 0;
                assert_eq!(
                    failure.stable_site.definition.0.name,
                    if from_call { "consume" } else { "Token" }
                );
                assert_eq!(failure.stable_site.line, if from_call { 6 } else { 3 });
            }
            if failures == 2 {
                assert_eq!(observation.failures[0].cause_indices, [1]);
                assert!(observation.failures[1].cause_indices.is_empty());
            }
            let mut probe = entry_path(&default_context());
            probe.0.name = "probe".into();
            let signatures = input
                .signatures
                .iter()
                .map(|(p, s)| (p.clone(), (**s).clone()))
                .collect();
            let mut reordered = input.clone();
            let definition = Arc::make_mut(reordered.definitions.get_mut(&probe).unwrap());
            let child = &mut definition.bodies[definition.entry.0 as usize];
            let chain = child.cleanups.iter().find_map(|cleanup| {
                let mut next = cleanup.head;
                let mut drops = Vec::new();
                while let Some(id) = next {
                    let node = &child.cleanup_nodes[id.0 as usize];
                    if let CleanupAction::Drop { place, .. } = node.action {
                        let ty = child.places[place.0 as usize].ty;
                        if matches!(&definition.types.get(ty).unwrap().kind,
                            hirsel_types::TypeKind::Named { definition, .. } if definition.0.name == "Token") {
                            drops.push(id);
                        }
                    }
                    next = node.parent;
                }
                (drops.len() >= 2).then_some(drops)
            }).unwrap();
            let first = child.cleanup_nodes[chain[0].0 as usize].action.clone();
            child.cleanup_nodes[chain[0].0 as usize].action =
                child.cleanup_nodes[chain[1].0 as usize].action.clone();
            child.cleanup_nodes[chain[1].0 as usize].action = first;
            let violations = hirsel_validate::validate(definition, &signatures).violations;
            assert!(
                violations.iter().any(|violation| matches!(
                    violation.kind,
                    hirsel_validate::ViolationKind::CleanupOrder(_)
                )),
                "{violations:?}"
            );
            assert!(producer.import_native_input(reordered).is_err());
            let mut forged = input.clone();
            let definition = Arc::make_mut(forged.definitions.get_mut(&probe).unwrap());
            let child = &mut definition.bodies[definition.entry.0 as usize];
            let moved = child
                .blocks
                .iter()
                .find_map(|block| match &block.terminator {
                    hirsel_tir::Terminator::Call {
                        callee: hirsel_tir::Callee::Definition { path, .. },
                        arguments,
                        ..
                    } if path.0.name == "consume" => Some(arguments[0].value.place),
                    _ => None,
                })
                .unwrap();
            let mut changed = 0;
            for node in &mut child.cleanup_nodes {
                if let CleanupAction::Drop { place, .. } = &mut node.action {
                    let ty = child.places[place.0 as usize].ty;
                    if *place != moved
                        && matches!(&definition.types.get(ty).unwrap().kind,hirsel_types::TypeKind::Named{definition,..} if definition.0.name=="Token")
                    {
                        *place = moved;
                        changed += 1
                    }
                }
            }
            assert!(changed > 0);
            assert!(
                !hirsel_validate::validate(definition, &signatures)
                    .violations
                    .is_empty()
            );
            assert!(producer.import_native_input(forged).is_err());
        }
    });
}

#[test]
fn native_call_result_snapshot_keeps_its_contents_during_unique_mutation() {
    with_compiler_stack(|| {
        let source = r#"
fn view(x:List[String])->List[String] in x{x}
fn change(mut xs:unique List[String]){xs[0]="new\n"}
fn send[T:sendable](take x:T){}
fn probe(rt:Stdout,take xs:unique List[String]){
 let kept=(copy view(xs),)
 let mut fresh=unique xs
 change(mut fresh)
 let _=out.print(rt,fresh[0])
 let first=copy fresh[0]
 send(take fresh)
 let (old,)=take kept
 let _=out.print(rt,old[0])
 let _=first
}
pub fn main(rt:Io)->Result[(),GrantError]{
  let grant_stdout = io.stdout(rt)?
let xs:List[String]=["old\n"];probe(grant_stdout,unique xs);Ok(())}
"#;
        let mut rejected = QueryEngine::new(default_environment(), compiler_adapters());
        let file = rejected.sources.add("reject.hir", source.into()).unwrap();
        let output = check_progress(&mut rejected, file, &default_context()).unwrap();
        assert_eq!(output.diagnostics[0].rule.as_str(), "MODE-4");
        assert_eq!(
            rejected.sources.slice(output.diagnostics[0].primary),
            Some("send(take fresh)")
        );
        let accepted = source.replace("send(take fresh)", "send(unique fresh)");
        let mut producer = engine(&accepted);
        let input = producer.export_native_input().unwrap();
        producer.import_native_input(input.clone()).unwrap();
        let mut probe = entry_path(&default_context());
        probe.0.name = "probe".into();
        let signatures = input
            .signatures
            .iter()
            .map(|(path, signature)| (path.clone(), (**signature).clone()))
            .collect();
        let mut forged = input.clone();
        let definition = Arc::make_mut(forged.definitions.get_mut(&probe).unwrap());
        let body = &mut definition.bodies[definition.entry.0 as usize];
        let value = body
            .rvalues
            .iter_mut()
            .rev()
            .find(|value| {
                matches!(
                    value,
                    hirsel_tir::Rvalue::Unary {
                        op: hirsel_syntax::UnaryOp::Unique,
                        ..
                    }
                )
            })
            .unwrap();
        let hirsel_tir::Rvalue::Unary { value: source, .. } = value else {
            unreachable!()
        };
        let mut source = source.clone();
        source.usage = hirsel_types::UseKind::Take;
        *value = hirsel_tir::Rvalue::Use(source);
        let violations = hirsel_validate::validate(definition, &signatures).violations;
        assert!(
            violations.iter().any(|violation| matches!(
                violation.kind,
                hirsel_validate::ViolationKind::TypeMismatch {
                    entity: "value modes"
                }
            )),
            "{violations:?}"
        );
        assert!(producer.import_native_input(forged).is_err());
        let observed = run_observed_snapshot(
            &mut producer,
            &RunConfiguration {
                entry: entry_path(&default_context()),
                hash_seed: 0,
                inputs: vec![],
            },
            SimulationConfig::default(),
        )
        .unwrap();
        assert_eq!(observed.outcome.unwrap(), RunOutcome::Exit(0));
        let observation = observed.observation.unwrap();
        assert!(observation.failures.is_empty());
        assert_eq!(observation.output, b"new\nold\n");
    });
}

use hirsel_driver::*;
use hirsel_engine::*;

fn check(source: &str, expected: Option<&str>) {
    with_compiler_stack(|| {
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
        let file = engine.sources.add("state.hir", source.into()).unwrap();
        let output = check_progress(&mut engine, file, &default_context()).unwrap();
        assert_eq!(
            output.diagnostics.first().map(|d| d.rule.as_str()),
            expected,
            "{source}\n{:?}",
            output.diagnostics
        );
        if expected.is_none() {
            assert_eq!(output.completion, CheckCompletion::Complete);
        }
    });
}
#[test]
fn a_preservation_only_callee_cannot_prove_cursor_progress() {
    check(
        "@preserves_captures(it) fn retain[T](mut it:Iter[T]) {}\n@advance_only(it) fn step[T](mut it:Iter[T]) { retain(mut it) }",
        Some("MEM-14"),
    );
}

#[test]
fn creation_uses_the_selected_operands_and_rejects_a_hidden_third_input() {
    check(
        "@captures(state, next) fn build[S,T](take state:S,take next:fn(mut S)->Option[T])->Iter[T]{iter.from(take state,take next)}",
        None,
    );
    check(
        "@captures(state) fn build[S,T](take state:S,take next:fn(mut S)->Option[T])->Iter[T]{iter.from(take state,take next)}",
        Some("CLO-4"),
    );
}

#[test]
fn a_fresh_allocating_finalizer_is_not_hidden_by_inert_constructor_operands() {
    let declaration = "resource type Token{value:u64} drop(t){}\n";
    check(
        &format!(
            "{declaration}@captures(state,next) fn build[S,T](take state:S,take next:fn(mut S)->Option[T])->Iter[T]{{iter.from(take state,take next)}}\nfn step(mut state:Token)->Option[u64]{{None}}\nfn probe(){{let it=build(Token(value:0u64),step)}}"
        ),
        None,
    );
    check(
        &format!(
            "{declaration}@captures(state,next) fn build[T](take state:u64,take next:fn(mut Token)->Option[T])->Iter[T]{{iter.from(Token(value:state),take next)}}"
        ),
        Some("CLO-4"),
    );
}

#[test]
fn source_creation_proof_rejects_hidden_inputs_before_lowering_or_v() {
    with_compiler_stack(|| {
        for (selectors, expected) in [("state,next,third", None), ("state,next", Some("CLO-4"))] {
            let source = format!(
                "type Relay[S,N]{{state:S,next:N}}\n@captures({selectors}) fn build[S,T](take state:S,take next:fn(mut S)->Option[T],take third:fn(mut S)->Option[T])->Iter[T]{{let relay=Relay(state:take state,next:take third);let Relay(state:s,next:n)=take relay;iter.from(take s,take n)}}"
            );
            let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
            let file = engine.sources.add("creation-source.hir", source).unwrap();
            let parsed = engine.parse_file(file, &default_context()).unwrap();
            let item = &parsed.value.items[1].item;
            let diagnostics = match engine.mode_definition(&item.definition) {
                Ok(output) => output.diagnostics,
                Err(hirsel_types::QueryError::Diagnostics(diagnostics)) => diagnostics,
                Err(error) => panic!("{error:?}"),
            };
            assert_eq!(
                diagnostics.first().map(|d| d.rule.as_str()),
                expected,
                "{diagnostics:?}"
            );
        }
    });
}

#[test]
fn a_copy_cannot_hide_crossing_evidence_lost_from_selected_state() {
    check(
        "@captures(state,next) fn build(take state:u64,take next:fn(mut u64)->Option[u64])->Iter[u64]{let shared=copy state;iter.from(take state,take next)}",
        None,
    );
    for (copy, expected) in [("", None), ("let shared=copy state;", Some("CLO-4"))] {
        check(
            &format!(
                "@captures(state,next) fn build[S:copyable,T](take state:unique S,take next:fn(mut S)->Option[T])->Iter[T]{{{copy}iter.from(take state,take next)}}"
            ),
            expected,
        );
    }
}

#[test]
fn ordinary_borrowed_calls_cannot_silently_preserve_a_creation_snapshot() {
    for (body, expected) in [("", None), ("inspect(state);", Some("CLO-4"))] {
        check(
            &format!(
                "fn inspect[S:copyable](state:S){{let shared=copy state}}\n@captures(state,next) fn build[S:copyable,T](take state:unique S,take next:fn(mut S)->Option[T])->Iter[T]{{{body}iter.from(take state,take next)}}"
            ),
            expected,
        );
    }
}

#[test]
fn call_entry_refreshes_late_shares_through_each_callable_form() {
    for (setup, call) in [
        ("", "keep"),
        (
            "let action:fn(mut unique List[u64],List[u64])->()=keep;",
            "action",
        ),
        (
            "let action=fn(mut out:unique List[u64],other:List[u64]){keep(mut out,other)};",
            "action",
        ),
    ] {
        for (argument, expected) in [("[2]", None), ("copy xs", Some("MODE-2"))] {
            check(
                &format!(
                    "fn keep(mut out:unique List[u64],other:List[u64]){{out[0]=9}}\nfn probe(){{let mut xs:unique List[u64]=[1];{setup}{call}(mut xs,{argument})}}"
                ),
                expected,
            );
        }
    }
}

#[test]
fn call_entry_refreshes_projected_and_both_mut_arguments() {
    for (arguments, expected) in [
        ("mut state.xs,mut ys,[3]", None),
        ("mut state.xs,mut ys,copy state.xs", Some("MODE-2")),
        ("mut state.xs,mut ys,copy ys", Some("MODE-2")),
    ] {
        check(
            &format!(
                "type State{{xs:List[u64]}}\nfn keep(mut xs:unique List[u64],mut ys:unique List[u64],other:List[u64]){{xs[0]=8;ys[0]=9}}\nfn probe(){{let mut state:unique State=State(xs:[1]);let mut ys:unique List[u64]=[2];keep({arguments})}}"
            ),
            expected,
        );
    }
}

#[test]
fn conditional_handback_uses_selected_current_evidence() {
    for promise in ["", "@preserves_unique(xs)"] {
        for (argument, expected) in [("[2]", None), ("copy xs", Some("MODE-2"))] {
            // A fixed unique receiver remains a precondition, even when a
            // preservation promise also appears on the declaration.
            check(
                &format!(
                    "{promise}\nfn keep(mut xs:unique List[u64],other:List[u64]){{xs[0]=9}}\nfn probe(){{let mut xs:unique List[u64]=[1];keep(mut xs,{argument})}}"
                ),
                expected,
            );
        }
    }
    for (setup, expected) in [
        ("", None),
        ("let saved=copy xs;", Some("MODE-2")),
        ("{let saved=copy xs;}", Some("MODE-2")),
    ] {
        check(
            &format!(
                "@preserves_unique(xs) fn keep(mut xs:List[u64],take other:List[u64]){{list.push(mut xs,1)}}\nfn need(take xs:unique List[u64]){{}}\nfn probe(){{let mut xs:unique List[u64]=[1];{setup}keep(mut xs,[2]);need(take xs)}}"
            ),
            expected,
        );
    }
}

#[test]
fn a_unique_sibling_cannot_replace_the_selected_handback_promise() {
    for (promise, expected) in [
        ("", Some("MODE-2")),
        ("@preserves_unique(ys)", Some("MODE-2")),
        ("@preserves_unique(xs)", None),
    ] {
        check(
            &format!(
                "{promise}\npub fn keep(mut xs:List[u64],mut ys:List[u64]){{list.push(mut xs,1)}}\nfn need(take xs:unique List[u64]){{}}\nfn probe(){{let mut xs:unique List[u64]=[1];let mut ys:unique List[u64]=[2];keep(mut xs,mut ys);need(take xs)}}"
            ),
            expected,
        );
    }
}

/// Each envelope axis of an unselected retained state makes it a hidden
/// input; selecting the state admits it. A retained borrowed source cannot
/// reach `iter.from` at all (MODE-1 requires global creation operands).
#[test]
fn every_envelope_axis_of_an_unselected_creation_state_is_a_hidden_input() {
    let token = "resource type Token{value:u64} drop(t){}\n";
    for (_axis, declarations, state) in [
        ("once", "", "once fn()->u64 uses{}"),
        ("authority", "", "Io"),
        ("authority bound", "[S:sendable+noalloc_drop]", "S"),
        ("sendability bound", "[S:inert+noalloc_drop]", "S"),
        ("allocating drop bound", "[S:inert+sendable]", "S"),
        ("allocating drop", token, "Token"),
    ] {
        let (prefix, generics) = if declarations.starts_with('[') {
            (
                "",
                declarations.trim_start_matches('[').trim_end_matches(']'),
            )
        } else {
            (declarations, "")
        };
        let generics = if generics.is_empty() {
            "T".to_string()
        } else {
            format!("{generics},T")
        };
        for (selectors, expected) in [("state,next", None), ("next", Some("CLO-4"))] {
            let source = format!(
                "{prefix}@captures({selectors}) fn build[{generics}](take state:{state},take next:fn(mut {state})->Option[T])->Iter[T]{{iter.from(take state,take next)}}"
            );
            check(&source, expected);
        }
    }
}

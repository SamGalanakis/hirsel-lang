//! Bounded MEM-5 model: a written take remains caller-owned until entry.
use hirsel_driver::*;
use hirsel_engine::*;

#[derive(Clone, Copy, Debug)]
enum Exit {
    Return,
    Argument,
    Requires,
    Ensures,
}

fn diagnostics(source: &str) -> (QueryEngine, Vec<hirsel_diag::Diagnostic>) {
    let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
    let file = engine
        .sources
        .add("cleanup-model.hir", source.into())
        .unwrap();
    let parsed = engine.parse_file(file, &default_context()).unwrap();
    assert!(
        parsed.diagnostics.is_empty(),
        "{source}\n{:?}",
        parsed.diagnostics
    );
    let mut path = entry_path(&default_context());
    path.0.name = "caller".into();
    let ds = match engine.mode_definition(&path) {
        Ok(result) => result.diagnostics,
        Err(QueryError::Diagnostics(ds)) => ds,
        Err(error) => panic!("{source}\n{error:?}"),
    };
    (engine, ds)
}

/// Independent ownership state. A take moves a binding into a retained
/// argument owned by this caller; only entry hands it to the consumer. Record
/// possible cleanup at the division, including when the chosen run returns.
fn cleanup_obligation(taken: bool, reverse: bool, exit: Exit) -> bool {
    let mut bindings = [taken; 2];
    let mut retained = Vec::new();
    let mut destroyed = Vec::new();
    if matches!(exit, Exit::Requires) {
        destroyed.extend((0..2).rev().filter(|&i| bindings[i]));
    }
    let operands = if reverse {
        [Some(1), Some(0), None]
    } else {
        [Some(0), None, Some(1)]
    };
    for operand in operands {
        match operand {
            Some(i) if taken => {
                assert!(bindings[i]);
                bindings[i] = false;
                retained.push(i);
            }
            None => {
                destroyed.extend(retained.iter().rev().copied());
                destroyed.extend((0..2).rev().filter(|&i| bindings[i]));
            }
            _ => (),
        }
    }
    // Entry transfers the retained arguments; return transfers the tuple.
    let returned = std::mem::take(&mut retained);
    if matches!(exit, Exit::Ensures) {
        destroyed.extend(returned.into_iter().rev());
    }
    !destroyed.is_empty()
}

fn validate_accepted(engine: &mut QueryEngine, nominal: bool) {
    let mut signatures = hirsel_validate::SignatureMap::new(engine.types().clone());
    for name in ["caller", "relay", "R"]
        .into_iter()
        .take(if nominal { 3 } else { 2 })
    {
        let mut path = entry_path(&default_context());
        path.0.name = name.into();
        let signature = engine.item_signature(&path).unwrap().value;
        signatures.insert(signature.definition, (*signature).clone());
    }
    let mut path = entry_path(&default_context());
    path.0.name = "caller".into();
    let definition = engine.lower_to_tir(&path).unwrap().value;
    hirsel_validate::assert_valid(&definition, engine.types(), &signatures);
}

#[test]
fn generic_and_nominal_arguments_match_the_ownership_model() {
    let mut count = 0;
    // 40 generic + 32 nominal cases. Bounds are universal evidence; nominal
    // cleanup is determined by the destructor declaration, never its body.
    for generic in [true, false] {
        for proof in [false, true] {
            for taken in [false, true] {
                for reverse in [false, true] {
                    for exit in [Exit::Return, Exit::Argument, Exit::Requires, Exit::Ensures] {
                        for nested in [false, true]
                            .into_iter()
                            .take(if generic && proof && taken { 2 } else { 1 })
                        {
                            let name = format!(
                                "{}_{proof}_{taken}_{reverse}_{exit:?}_{nested}",
                                if generic { "generic" } else { "nominal" }
                            );
                            let generic_header = if generic {
                                if proof { "[T: noalloc_drop]" } else { "[T]" }
                            } else {
                                ""
                            };
                            let resource = if generic {
                                String::new()
                            } else {
                                format!(
                                    "resource type R {{ n:u64 }} {} drop(x) {{ () }}\n",
                                    if proof { "@noalloc" } else { "" }
                                )
                            };
                            let leaf = if generic { "T" } else { "R" };
                            let value = if nested {
                                format!("(({leaf},),)")
                            } else {
                                leaf.into()
                            };
                            let mode = if taken { "take " } else { "" };
                            let usage = if taken { "take " } else { "" };
                            let result = if taken {
                                format!("({value},{value})")
                            } else {
                                "u64".into()
                            };
                            let body = if taken {
                                "(take first,take second)"
                            } else {
                                "n"
                            };
                            let (params, args) = if reverse {
                                (
                                    format!("{mode}first:{value},{mode}second:{value},n:u64"),
                                    format!("{usage}b,{usage}a,10u64/z"),
                                )
                            } else {
                                (
                                    format!("{mode}first:{value},n:u64,{mode}second:{value}"),
                                    format!("{usage}a,10u64/z,{usage}b"),
                                )
                            };
                            let contract = match exit {
                                Exit::Requires => "requires z > 0u64",
                                Exit::Ensures => "ensures z > 0u64",
                                _ => "",
                            };
                            let source = format!(
                                "{resource}@noalloc fn relay{generic_header}({params})->{result} {{{body}}}\n@noalloc fn caller{generic_header}({mode}a:{value},{mode}b:{value},z:u64)->{result} {contract} {{relay({args})}}"
                            );
                            let expected = !cleanup_obligation(taken, reverse, exit) || proof;
                            let (mut engine, ds) = diagnostics(&source);
                            assert_eq!(ds.is_empty(), expected, "{name}\n{source}\n{ds:?}");
                            if expected {
                                validate_accepted(&mut engine, !generic);
                            }
                            for diagnostic in ds {
                                assert_eq!(
                                    diagnostic.rule.as_str(),
                                    "MEM-5",
                                    "{name}: {diagnostic:?}"
                                );
                                // Cleanup belongs to the original owned parameters.
                                let site = engine.sources.slice(diagnostic.primary).unwrap();
                                assert!(
                                    site == value
                                        || (matches!(exit, Exit::Requires | Exit::Ensures)
                                            && site == format!("relay({args})")),
                                    "{name}: {site}"
                                );
                            }
                            count += 1;
                        }
                    }
                }
            }
        }
    }
    assert_eq!(count, 72);
    eprintln!(
        "noalloc_cleanup_model_16: {count} argument cases agreed with J; accepted cases passed V"
    );
}

#[test]
fn erased_invocation_and_cleanup_are_independent() {
    let mut count = 0;
    for noalloc_call in [false, true] {
        for owned in [false, true] {
            for invoke in [false, true] {
                let mode = if owned { "take " } else { "" };
                let effect = if noalloc_call { "@noalloc " } else { "" };
                let body = if invoke { "f()" } else { "7u64" };
                // An exported erased type has no proof for its captured resource.
                let source = format!(
                    "@noalloc fn caller({mode}f:{effect}fn()->u64 uses {{}})->u64 uses {{f}} {{{body}}}"
                );
                let (_, ds) = diagnostics(&source);
                let accepted = !owned && (!invoke || noalloc_call);
                assert_eq!(
                    ds.is_empty(),
                    accepted,
                    "owned={owned} call={noalloc_call} invoke={invoke}\n{source}\n{ds:?}"
                );
                assert!(ds.iter().all(|d| d.rule.as_str() == "MEM-5"));
                count += 1;
            }
        }
    }
    assert_eq!(count, 8);
}

#[test]
fn true_transfer_on_every_exit_needs_no_generic_cleanup_bound() {
    let source = "@noalloc fn caller[T](take a:T,take b:T,flag:bool)->((T,),T) {if flag {return ((take a,),take b)} else {((take a,),take b)}}";
    let (_, ds) = diagnostics(source);
    assert!(ds.is_empty(), "{ds:?}");
    let source = "resource type R {n:u64} drop(x) {()}\nfn caller()->@noalloc fn(take R,take R)->(R,R) uses {} {fn(take a:R,take b:R)->(R,R) {(take a,take b)}}\n@noalloc fn relay(take a:R,take b:R)->(R,R) {(take a,take b)}";
    let (mut engine, ds) = diagnostics(source);
    assert!(ds.is_empty(), "{ds:?}");
    validate_accepted(&mut engine, true);
}

/// Adapt the before/after call-return distinction in rustc_mir_dataflow's
/// initialized.rs:255-303 and disconnected call-block vectors in
/// framework/tests.rs:1-68 (19833af7224424e8b53e288097d56b6af43d5bcf).
/// The Rust Project Contributors, MIT OR Apache-2.0. These are test shapes;
/// Hirsel's owned callback result and MEM-5 determine the expected obligation.
#[test]
fn callback_results_exist_only_after_successful_return() {
    let mut count = 0;
    for proof in [false, true] {
        for failure in [None, Some(false), Some(true)] {
            let mut owned = false;
            let mut cleanup = false;
            // Model the call's disconnected failure and successful-return edges.
            // A failing call creates no returned value in this frame.
            assert!(!owned);
            if failure == Some(false) {
                cleanup |= owned;
            }
            owned = true;
            if failure == Some(true) {
                cleanup |= owned;
            }
            owned = false; // successful return transfers it
            assert!(!owned);
            let body = match failure {
                None => "read()",
                Some(false) => "let _=1u64/divisor;read()",
                Some(true) => "let output=read();let _=1u64/divisor;take output",
            };
            let bound = if proof { ":noalloc_drop" } else { "" };
            // The disconnected helper body is checked universally, even though
            // main does not call it; a separate direct caller has no obligation.
            let source = format!(
                "@noalloc fn caller[R{bound}](read:@noalloc fn()->R uses {{}},divisor:u64)->R uses {{}} {{{body}}}\n@noalloc fn relay[R](read:@noalloc fn()->R uses {{}})->R uses {{}} {{read()}}\npub fn main(){{}}"
            );
            let expected = !cleanup || proof;
            let (mut engine, ds) = diagnostics(&source);
            assert_eq!(ds.is_empty(), expected, "{source}\n{ds:?}");
            if expected {
                let input = engine.export_native_input().unwrap();
                engine.import_native_input(input).unwrap();
            } else {
                assert_eq!(ds.len(), 1);
                assert_eq!(ds[0].rule.as_str(), "MEM-5");
                assert_eq!(engine.sources.slice(ds[0].primary), Some("output"));
                assert!(
                    ds[0]
                        .secondary
                        .iter()
                        .any(|label| engine.sources.slice(label.span) == Some("1u64/divisor"))
                );
            }
            count += 1;
        }
    }
    assert_eq!(count, 6);
    eprintln!(
        "noalloc_cleanup_model_16: {count} callback return-edge cases matched the model and accepted V"
    );
}

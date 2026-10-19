use hirsel_driver::*;
use hirsel_engine::*;

fn checked(source: &str, name: &str) -> Result<(), QueryError> {
    let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
    let context = default_context();
    let file = engine.sources.add("transport.hir", source.into()).unwrap();
    engine.parse_file(file, &context).unwrap();
    let mut path = entry_path(&context);
    path.0.name = name.into();
    // Diagnostics come back at file offsets so callers can index the source.
    engine
        .lower_to_tir(&path)
        .map(|_| ())
        .map_err(|error| match error {
            QueryError::Diagnostics(mut diagnostics) => {
                for d in &mut diagnostics {
                    let at = engine.sources.resolve(d.primary);
                    d.primary =
                        hirsel_span::Span::new(engine.sources.file_anchor(file), at.start, at.end)
                            .unwrap();
                }
                QueryError::Diagnostics(diagnostics)
            }
            other => other,
        })
}

#[test]
fn closure_call_results_cannot_escape_the_storage_of_a_block_capture() {
    with_compiler_stack(|| {
        for expression in [
            "{let v=view(x);(fn(){v})()}",
            "{let v=view(x);let f=fn(){v};f()}",
        ] {
            let source = format!(
                "fn view(x:Bytes)->Bytes in x{{copy x}}\nfn probe(mut x:Bytes){{let slot={expression};()}}"
            );
            match checked(&source, "probe") {
                Err(QueryError::Diagnostics(diagnostics)) => {
                    assert_eq!(
                        diagnostics[0].rule.as_str(),
                        "MODE-1",
                        "{source}: {diagnostics:?}"
                    );
                }
                other => panic!("expected lexical MODE-1: {source}\n{other:?}"),
            }
        }
        for source in [
            "type Holder{value:Bytes}\nfn probe(take h:Holder,seed:Bytes)->Bytes in h{let mut r:Holder in r=take h;r.value=b\"fresh\";(fn(){r.value})()}",
        ] {
            match checked(source, "probe") {
                Err(QueryError::Diagnostics(diagnostics)) => assert_eq!(
                    diagnostics[0].rule.as_str(),
                    "MODE-1",
                    "{source}: {diagnostics:?}"
                ),
                other => panic!("source/IR disagreement: {source}\n{other:?}"),
            }
        }
        for source in [
            "fn view(x:Bytes)->Bytes in x{copy x}\nfn probe(mut x:Bytes){let slot=(fn(){view(x)})();()}",
            "fn view(x:Bytes)->Bytes in x{copy x}\nfn probe(mut x:Bytes){{let v=view(x);let slot=(fn(){v})();()};()}",
            // The closure reads `t` as a borrowed view: `t.value` is local to
            // the environment, which is `h`'s storage, as the function twin is.
            "type Holder{value:Bytes}\nfn identity[T](x:T)->T in x{x}\nfn probe(take h:Holder)->Bytes in h{let t=identity(h);(fn(){t.value})()}",
        ] {
            checked(source, "probe").unwrap_or_else(|e| panic!("{source}\n{e:?}"));
        }
    });
}

#[test]
fn mutable_iteration_through_a_closure_keeps_the_displaced_storage_contract() {
    with_compiler_stack(|| {
        for source in [
            "fn probe(mut xs:List[String],take y:String){for x in mut (fn(){xs})(){x=copy y}}",
            "fn probe(mut xs:List[Bytes],take y:Bytes){for x in mut (fn(){xs})(){x=copy y}}",
        ] {
            checked(source, "probe").unwrap_or_else(|e| panic!("{source}\n{e:?}"));
        }
    });
}

#[test]
fn entry_snapshots_check_call_modes_and_keep_contract_parameters_read_only() {
    with_compiler_stack(|| {
        for (passing, unique, rule) in [
            ("", "", "MODE-2"),
            ("take ", "unique ", "FACT-1"),
            ("mut ", "unique ", "FACT-1"),
        ] {
            let source = format!(
                "fn inspect({passing}x:unique Bytes)->bool{{true}}\nfn weaken(mut x:Bytes){{()}}\nfn probe(mut x:{unique}Bytes)->() ensures before(inspect({}x)){{weaken(mut x)}}",
                if passing == "mut " { "mut " } else { "" }
            );
            match checked(&source, "probe") {
                Err(QueryError::Diagnostics(diagnostics)) => assert_eq!(
                    diagnostics[0].rule.as_str(),
                    rule,
                    "{source}: {diagnostics:?}"
                ),
                other => panic!("expected {rule}: {source}\n{other:?}"),
            }
        }
        checked("fn inspect(x:unique Bytes)->bool{true}\nfn probe(mut x:unique Bytes,take y:unique Bytes)->() ensures before(inspect(x)){let kept=copy x;x=take y}", "probe").unwrap();
    });
}

#[test]
fn branch_result_loans_survive_a_named_receiver_and_tail_read() {
    with_compiler_stack(|| {
        for branch in ["if c{x}else{y}", "match c{true=>{x},false=>{y}}"] {
            let source = format!(
                "fn probe(x:Bytes,y:Bytes,c:bool)->Bytes in {{x, y}}{{let chosen={branch};chosen}}"
            );
            checked(&source, "probe").unwrap_or_else(|e| panic!("{source}\n{e:?}"));
        }
    });
}

#[test]
fn consuming_local_list_items_cannot_escape_the_cursor_into_an_outer_receiver() {
    with_compiler_stack(|| {
        for (body, accepted) in [
            ("keep=x", false),
            ("keep=copy x", true),
            ("keep=view(x)", false),
            ("keep=view(copy x)", false),
            ("let held=x;string.byte_len(held);()", true),
        ] {
            let source = format!(
                "fn view(x:String)->String in x{{x}}\nfn probe(take xs:List[String] in xs,seed:String)->u64{{let mut keep=view(seed);for x in take xs{{{body}}};string.byte_len(keep)}}"
            );
            match checked(&source, "probe") {
                Ok(()) if accepted => (),
                Err(QueryError::Diagnostics(diagnostics)) if !accepted => {
                    assert_eq!(
                        diagnostics[0].rule.as_str(),
                        "MODE-1",
                        "{source}: {diagnostics:?}"
                    );
                    assert!(
                        source[diagnostics[0].primary.start as usize..].starts_with("keep="),
                        "{source}: {diagnostics:?}"
                    );
                }
                other => panic!("accepted={accepted}: {source}\n{other:?}"),
            }
        }
    });
}

#[test]
fn a_copied_borrowed_callback_requires_a_nonallocating_cleanup_proof() {
    with_compiler_stack(|| {
        let source = "@noalloc fn through(g:@noalloc fn()->() uses{})->(){()}\nfn probe(){let f:@noalloc fn(g:@noalloc fn()->() uses{})->() uses{}=fn(g:@noalloc fn()->() uses{}){through(copy g)};()}";
        match checked(source, "probe") {
            Err(QueryError::Diagnostics(diagnostics)) => {
                assert_eq!(diagnostics[0].rule.as_str(), "MEM-5", "{diagnostics:?}");
                assert!(source[diagnostics[0].primary.start as usize..].starts_with("copy g"));
            }
            other => panic!("expected cleanup MEM-5: {other:?}"),
        }
        checked(&source.replace("through(copy g)", "through(g)"), "probe").unwrap();
        checked(
            "@noalloc fn through(x:Bytes){()}\n@noalloc fn probe(x:Bytes){through(copy x)}",
            "probe",
        )
        .unwrap();
    });
}
#[test]
fn named_owner_bounds_survive_local_views_and_nonflat_copies() {
    with_compiler_stack(|| {
        let helpers = "fn view(x:Bytes)->Bytes in x{copy x}\nfn wrap(x:Bytes)->List[Bytes] in x{[copy x]}\nfn len(x:Bytes)->u64{0u64}\nfn n(xs:List[Bytes])->u64{list.len(xs)}\n";
        for body in [
            "let slot={let t=copy x;let u=wrap(t);copy u};n(slot)",
            "let slot={let t=copy x;let u=view(t);copy u};len(slot)",
            "let mut i=0u64;let mut slot=view(x);while i<2u64{let t=copy x;slot=copy view(t);i=i+1u64};len(slot)",
            "let (slot,k)={let t=copy x;(copy view(t),1u8)};len(slot)",
            "let slot=if true{let t=copy x;copy view(t)}else{view(x)};len(slot)",
            "let slot={let t=copy x;view(t)};len(slot)",
            "let slot={let t=copy x;let u=view(t);u};len(slot)",
            "let slot={let t=copy x;let u=wrap(t);u};n(slot)",
            "let slot=match c{true=>{let t=copy x;view(t)},false=>view(x)};len(slot)",
            "let mut slot=view(x);{let t=copy x;slot=view(t)};len(slot)",
            "let mut p=H(a:view(x));{let t=copy x;p.a=view(t)};len(p.a)",
            "let slot={let t:List[Bytes]=[copy x];first(t)};len(slot)",
        ] {
            let source = format!(
                "{helpers}type H{{a:Bytes}}\nfn first(xs:List[Bytes])->Bytes in xs{{xs[0u64]}}\nfn probe(mut x:Bytes,c:bool)->u64{{{body}}}"
            );
            match checked(&source, "probe") {
                Err(QueryError::Diagnostics(diagnostics)) => assert_eq!(
                    diagnostics[0].rule.as_str(),
                    "MODE-1",
                    "{source}\n{diagnostics:?}"
                ),
                other => panic!("named owner escaped: {source}\n{other:?}"),
            }
        }
        for source in [
            "fn probe(take x:Bytes)->Bytes{let slot={let t=copy x;copy t};slot}",
            "fn view(x:u64)->u64 in x{x}\nfn probe(x:u64)->u64{let slot={let t=x;let u=view(t);copy u};slot}",
            "fn view(x:Bytes)->Bytes in x{copy x}\nfn probe(x:Bytes)->u64{{let t=copy x;let u=view(t);let slot=copy u;string.byte_len(\"ok\")};0u64}",
        ] {
            checked(source, "probe").unwrap_or_else(|e| panic!("{source}\n{e:?}"));
        }
    });
}

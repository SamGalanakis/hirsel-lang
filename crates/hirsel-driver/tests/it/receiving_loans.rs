#[test]
fn a_stored_local_result_keeps_its_call_loan_until_the_receiving_scope_ends() {
    use hirsel_engine::DefinitionQueries;
    for passing in ["", "mut "] {
        for alternate in [false, true] {
            for assignment in [
                "if c {slot=b\"yes\"} else {slot=view(x)}",
                "match c {true => {slot=b\"yes\"}, false => {slot=view(x)}}",
                "{slot=view(x)}",
                "let mut more=c;while more {slot=view(x);more=false}",
            ] {
                let assignment = if alternate {
                    assignment.replace("slot=view(x)", "slot=view(y)")
                } else {
                    assignment.into()
                };
                let source = format!(
                    "fn view(x:Bytes)->Bytes in x{{copy x}}\nfn probe({passing}x:Bytes,{passing}y:Bytes,c:bool)->Bytes in {{x, y}}{{let mut slot=view(x);{assignment};slot}}"
                );
                let mut engine = hirsel_engine::QueryEngine::new(
                    hirsel_engine::default_environment(),
                    hirsel_driver::compiler_adapters(),
                );
                let context = hirsel_engine::default_context();
                let file = engine.sources.add("loans.hir", source).unwrap();
                engine.parse_file(file, &context).unwrap();
                let mut path = hirsel_engine::entry_path(&context);
                path.0.name = "probe".into();
                engine.lower_to_tir(&path).unwrap();
            }
        }
    }
}

#[test]
fn replacing_result_holders_releases_only_their_displaced_source_loans() {
    use hirsel_engine::DefinitionQueries;
    for statements in [
        "let mut slot=view(x);slot=b\"fresh\";bump(mut x)",
        "let mut slot=view(x);observe(x,{slot=b\"fresh\";1u32});bump(mut x)",
        "let mut slot=(fn(){view(x)})();slot=b\"fresh\";bump(mut x)",
        "let mut slot=view(x);slot=view(x);slot=b\"fresh\";bump(mut x)",
        "let mut slot=view(x);slot=view(y);bump(mut x);slot=b\"fresh\";bump(mut y)",
        "let mut slot=view(x);if c {slot=b\"a\"}else{slot=b\"b\"};bump(mut x)",
        "let mut slot=view(x);match c {true=>{slot=b\"a\"},false=>{slot=b\"b\"}};bump(mut x)",
    ] {
        let source = format!(
            "fn view(x:Bytes)->Bytes in x{{copy x}}\nfn bump(mut x:Bytes){{()}}\nfn observe(x:Bytes,n:u32){{()}}\nfn probe(mut x:Bytes,mut y:Bytes,c:bool){{{statements};()}}"
        );
        let mut engine = hirsel_engine::QueryEngine::new(
            hirsel_engine::default_environment(),
            hirsel_driver::compiler_adapters(),
        );
        let context = hirsel_engine::default_context();
        let file = engine.sources.add("holder.hir", source.clone()).unwrap();
        engine.parse_file(file, &context).unwrap();
        let mut path = hirsel_engine::entry_path(&context);
        path.0.name = "probe".into();
        engine
            .lower_to_tir(&path)
            .unwrap_or_else(|error| panic!("{source}\n{error:?}"));
    }
}

#[test]
fn loop_feedback_preserves_permissions_on_continue_break_and_condition_calls() {
    use hirsel_engine::DefinitionQueries;
    for body in [
        "while n<3u32{slot=view(y);n+=1u32;continue}",
        "while n<3u32{slot=view(y);if c {break};n+=1u32}",
        "for i in 0u32..3u32{slot=view(y)}",
        "for i in 0u32..3u32{slot=view(y);continue}",
        "while ready(x,n){slot=view(y);n+=1u32}",
    ] {
        let source = format!(
            "fn view(x:Bytes)->Bytes in x{{copy x}}\nfn ready(x:Bytes,n:u32)->bool{{n<3u32}}\nfn bump(mut x:Bytes){{()}}\nfn probe(mut x:Bytes,mut y:Bytes,c:bool){{let mut slot=view(x);let mut n:u32=0u32;{body};slot=b\"fresh\";bump(mut y);()}}"
        );
        let mut engine = hirsel_engine::QueryEngine::new(
            hirsel_engine::default_environment(),
            hirsel_driver::compiler_adapters(),
        );
        let context = hirsel_engine::default_context();
        let file = engine.sources.add("feedback.hir", source.clone()).unwrap();
        engine.parse_file(file, &context).unwrap();
        let mut path = hirsel_engine::entry_path(&context);
        path.0.name = "probe".into();
        engine
            .lower_to_tir(&path)
            .unwrap_or_else(|error| panic!("{source}\n{error:?}"));
    }
}

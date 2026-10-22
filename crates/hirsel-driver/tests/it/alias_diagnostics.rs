use hirsel_driver::*;
use hirsel_engine::*;

#[test]
fn mutual_alias_cycles_are_language_failures_in_either_declaration_order() {
    with_compiler_stack(|| {
        for source in [
            "type A = B\ntype B = A\nfn f(a:A) {}",
            "type B = A\ntype A = B\nfn f(a:A) {}",
            "type A = B\ntype B = C\ntype C = A",
            "type A = A",
        ] {
            let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
            let file = engine.sources.add("aliases.hir", source.into()).unwrap();
            let output = check_progress(&mut engine, file, &default_context()).unwrap();
            assert_eq!(output.completion, CheckCompletion::Complete, "{source}");
            let diagnostic = output.diagnostics.first().unwrap();
            assert_eq!(
                diagnostic.rule.as_str(),
                if source == "type A = A" {
                    "MEM-7"
                } else {
                    "LANG-7"
                },
                "{source}"
            );
            assert_eq!(
                diagnostic.primary.end - diagnostic.primary.start,
                if source == "type A = A" {
                    source.len() as u32
                } else {
                    1
                },
                "{source}"
            );
        }
    });
}

#[test]
fn borrowed_capture_repairs_19_alias_declaration_order_keeps_the_checked_repair() {
    with_compiler_stack(|| {
        let alias = "type Row = List[String]";
        let body = "fn probe(row:Row,mut m:Map[u64,List[Row]]) { let _ = map.update_value(mut m,0,fn(mut rows:List[Row])->() uses {} { list.push(mut rows,copy row) }) }";
        for source in [format!("{alias}\n{body}"), format!("{body}\n{alias}")] {
            for (source, accepted) in [
                (source.clone(), false),
                (source.replace("copy row", "unique row"), true),
            ] {
                let mut engine = crate::source_std::engine();
                let file = engine.sources.add("aliases.hir", source.clone()).unwrap();
                let output = check_progress(&mut engine, file, &default_context()).unwrap();
                assert_eq!(output.completion, CheckCompletion::Complete);
                if accepted {
                    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
                    continue;
                }
                let [d] = &output.diagnostics[..] else {
                    panic!("{:?}", output.diagnostics)
                };
                assert_eq!(d.rule.as_str(), "MODE-1");
                assert_eq!(
                    output.sources.slice(d.primary),
                    Some("list.push(mut rows,copy row)")
                );
                assert_eq!(
                    output.sources.resolve(d.primary).start as usize,
                    source.find("list.push").unwrap()
                );
                assert!(
                    d.notes
                        .iter()
                        .any(|n| n.starts_with("help:") && n.contains("`unique row`"))
                );
                assert!(
                    d.secondary
                        .iter()
                        .any(|l| l.message.contains("borrowed parameter")
                            && output.sources.slice(l.span) == Some("Row"))
                );
            }
        }
    });
}

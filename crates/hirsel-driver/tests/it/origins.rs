use hirsel_driver::*;
use hirsel_engine::*;
use hirsel_syntax::*;

#[test]
fn header_sites_distinguish_names_from_nested_type_and_body_uses() {
    with_compiler_stack(|| {
        let source = "fn f[T:plain,n:size,U](from take a: fn(from b:u8)->u8 uses {nested}, from mut b:u8) -> (fn()->u8 uses {inner}) uses {zz,pkg@1/mod.cap} {}";
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
        let id = engine.sources.add("origins.hir", source.into()).unwrap();
        let file = engine.parse_file(id, &default_context()).unwrap().value;
        let item = &file.tree.items[file.roots[0].0 as usize];
        let ItemKind::Function { signature, .. } = &item.kind else {
            panic!()
        };
        let spellings = |spans: Vec<hirsel_span::Span>| {
            spans
                .into_iter()
                .map(|s| engine.sources.slice(s).unwrap().to_string())
                .collect::<Vec<_>>()
        };
        assert_eq!(
            spellings(file.generic_name_spans(item.origin)),
            ["T", "n", "U"]
        );
        assert_eq!(
            signature
                .parameters
                .iter()
                .map(|p| engine
                    .sources
                    .slice(file.parameter_name_span(item.origin, p).unwrap())
                    .unwrap())
                .collect::<Vec<_>>(),
            ["a", "b"]
        );
        let labels: Vec<_> = signature
            .parameters
            .iter()
            .map(|p| file.parameter_label_span(item.origin, p).unwrap())
            .collect();
        assert_eq!(spellings(labels.clone()), ["from", "from"]);
        assert_eq!(labels[1].start as usize, source.find("from mut").unwrap());
        assert_eq!(
            spellings(file.uses_path_spans(item.origin)),
            ["zz", "pkg@1/mod.cap"]
        );
    });
}

#[test]
fn variant_origins_ignore_generic_and_payload_names() {
    with_compiler_stack(|| {
        let source = "type V[T] = X(fn(x:T)->T uses {cap}) | X(label:T)";
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
        let id = engine.sources.add("origins.hir", source.into()).unwrap();
        let file = engine.parse_file(id, &default_context()).unwrap().value;
        let item = &file.tree.items[file.roots[0].0 as usize];
        let spans = file.variant_name_spans(item.origin);
        assert_eq!(spans.len(), 2);
        assert_eq!(spans[0].start as usize, source.find("X(").unwrap());
        assert_eq!(spans[1].start as usize, source.rfind("X(").unwrap());
    });
}

#[test]
fn closure_trailing_uses_keeps_its_exact_path_site() {
    with_compiler_stack(|| {
        let source = "fn f() { let c = fn(a:u8)->u8 { a } uses {cap}; }";
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
        let id = engine.sources.add("origins.hir", source.into()).unwrap();
        let file = engine.parse_file(id, &default_context()).unwrap().value;
        let closure = file
            .tree
            .expressions
            .iter()
            .find(|e| matches!(e.kind, ExprKind::Closure { .. }))
            .unwrap();
        let spans = file.uses_path_spans(closure.origin);
        assert_eq!(spans.len(), 1);
        assert_eq!(engine.sources.slice(spans[0]), Some("cap"));
    });
}

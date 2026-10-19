use hirsel_driver::*;
use hirsel_engine::*;
use hirsel_span::{FileId, SourceMap, Span};
use hirsel_syntax::*;
use std::rc::Rc;

/// A fixture parser for one empty function; all production lexing/formatting is real.
struct EmptyFunction;
impl ParserPhase for EmptyFunction {
    fn parse(
        &self,
        file: FileId,
        tokens: Vec<Token>,
        context: &FileContext,
        sources: &SourceMap,
    ) -> Result<QueryOutput<SyntaxFile>, QueryError> {
        let source = sources.file(file).unwrap().text();
        let name = tokens
            .iter()
            .find_map(|t| {
                if let TokenKind::Ident(s) = t.kind {
                    Some(s)
                } else {
                    None
                }
            })
            .unwrap();
        let origin = Origin {
            span: Span::new(file, 0, source.len() as u32).unwrap(),
            tokens: TokenRange {
                start: 0,
                end: tokens.len() as u32 - 1,
            },
        };
        let item = Item {
            name_span: Default::default(),

            origin,
            owned_tokens: origin.tokens,
            name,
            definition: DefPath(ItemPath {
                package: context.package.clone(),
                modules: context.modules.clone(),
                name: "main".into(),
            }),
            text_hash: ContentHash::of_bytes(source.as_bytes()),
            visibility: Visibility::Private,
            attributes: vec![],
            kind: ItemKind::Function {
                signature: FunctionSyntax::default(),
                body: Some(ExprId(0)),
                abi: None,
            },
            generated: None,
        };
        let expression = Expr {
            origin,
            kind: ExprKind::Block {
                statements: vec![],
                tail: None,
            },
        };
        Ok(QueryOutput {
            value: SyntaxFile {
                has_syntax_errors: false,

                file,
                tokens,
                tree: SyntaxTree {
                    items: vec![item],
                    expressions: vec![expression],
                    ..Default::default()
                },
                roots: vec![ItemId(0)],
            },
            diagnostics: vec![],
            dependencies: vec![],
        })
    }
}
#[test]
fn real_lexer_and_formatter_reach_a_fixed_point_with_a_fixture_parser() {
    hirsel_engine::with_compiler_stack(
        real_lexer_and_formatter_reach_a_fixed_point_with_a_fixture_parser_on_compiler_thread,
    );
}
fn real_lexer_and_formatter_reach_a_fixed_point_with_a_fixture_parser_on_compiler_thread() {
    let mut adapters = compiler_adapters();
    adapters.parser = Some(Rc::new(EmptyFunction));
    let mut engine = QueryEngine::new(default_environment(), adapters);
    let first = engine
        .sources
        .add("format.hir", "fn  main ( ) { } // keep\n".into())
        .unwrap();
    let formatted = format_snapshot(&mut engine, first, &default_context()).unwrap();
    assert_eq!(formatted, "fn main() {} // keep\n");
    let second = engine.sources.add("format.hir", formatted.clone()).unwrap();
    assert_eq!(
        format_snapshot(&mut engine, second, &default_context()).unwrap(),
        formatted
    );
    let a = engine
        .parse_file(first, &default_context())
        .unwrap()
        .value
        .tree
        .items[0]
        .definition
        .clone();
    let b = engine
        .parse_file(second, &default_context())
        .unwrap()
        .value
        .tree
        .items[0]
        .definition
        .clone();
    assert_eq!(a, b);
}
#[test]
fn formatter_fallback_is_unavailable_and_never_a_clean_format_check() {
    hirsel_engine::with_compiler_stack(
        formatter_fallback_is_unavailable_and_never_a_clean_format_check_on_compiler_thread,
    );
}
fn formatter_fallback_is_unavailable_and_never_a_clean_format_check_on_compiler_thread() {
    let mut sources = SourceMap::default();
    let file = sources.add("damaged.hir", "fn main() {\n".into()).unwrap();
    let damaged = SyntaxFile {
        has_syntax_errors: false,

        file,
        tokens: vec![],
        tree: SyntaxTree::default(),
        roots: vec![],
    };
    assert!(matches!(
        FormatterAdapter.format_query(&damaged, &sources),
        Err(QueryError::NotImplemented)
    ));
}

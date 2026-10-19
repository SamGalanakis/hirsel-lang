use hirsel_driver::*;
use hirsel_engine::*;

const KINDS: [QueryKind; 5] = [
    QueryKind::Resolve,
    QueryKind::Signature,
    QueryKind::Check,
    QueryKind::Modes,
    QueryKind::Lower,
];

/// A line inserted at the top of a 2,000-line file changes only the first item's
/// text: every other item keeps its syntax and recomputes nothing.
#[test]
fn a_line_above_a_large_file_recomputes_only_the_first_item() {
    with_compiler_stack(|| {
        let count = 500;
        let source: String = (0..count)
            .map(|i| format!("fn f{i}(x: u64) -> u64 {{\n  let y = x + {i}u64\n  y * 2u64\n}}\n"))
            .collect();
        assert_eq!(source.lines().count(), 4 * count);
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
        let file = engine.sources.add("large.hir", source.clone()).unwrap();
        let started = std::time::Instant::now();
        let cold = check_progress(&mut engine, file, &default_context()).unwrap();
        let cold_time = started.elapsed();
        assert!(cold.diagnostics.is_empty(), "{:?}", cold.diagnostics);
        let parsed = engine.parse_file(file, &default_context()).unwrap().value;
        let paths: Vec<_> = parsed
            .items
            .iter()
            .map(|syntax| syntax.item.definition.clone())
            .collect();
        let counts = |engine: &QueryEngine| -> Vec<[usize; 5]> {
            paths
                .iter()
                .map(|path| KINDS.map(|kind| engine.recompute_count(path, kind)))
                .collect()
        };
        let before = counts(&engine);
        let syntax: Vec<_> = paths
            .iter()
            .map(|path| engine.parse_definition(path).unwrap().value)
            .collect();
        let (mut nodes, mut tokens, mut bytes) = (0, 0, 0);
        for item in &syntax {
            let tree = &item.tree;
            nodes += tree.expressions.len()
                + tree.statements.len()
                + tree.patterns.len()
                + tree.types.len();
            tokens += tree.tokens.len();
            bytes += std::mem::size_of::<hirsel_syntax::ItemSyntax>()
                + tree.expressions.len() * std::mem::size_of::<hirsel_syntax::Expr>()
                + tree.statements.len() * std::mem::size_of::<hirsel_syntax::Stmt>()
                + tree.patterns.len() * std::mem::size_of::<hirsel_syntax::Pattern>()
                + tree.types.len() * std::mem::size_of::<hirsel_syntax::TypeExpr>()
                + tree.tokens.len() * std::mem::size_of::<hirsel_syntax::Token>();
        }
        let edited = format!("// a new first line\n{source}");
        let file = engine.sources.add("large.hir", edited).unwrap();
        let started = std::time::Instant::now();
        let warm = check_progress(&mut engine, file, &default_context()).unwrap();
        let warm_time = started.elapsed();
        assert!(warm.diagnostics.is_empty(), "{:?}", warm.diagnostics);
        let after = counts(&engine);
        let recomputed: Vec<_> = (0..count).filter(|&i| before[i] != after[i]).collect();
        let total: usize = before
            .iter()
            .zip(&after)
            .map(|(b, a)| a.iter().sum::<usize>() - b.iter().sum::<usize>())
            .sum();
        eprintln!(
            "{} lines: cold check {cold_time:?}, re-check after a top line {warm_time:?}, \
             {total} recomputes in {} items; arenas: {} items, {nodes} nodes, {tokens} tokens, \
             {bytes} bytes",
            4 * count,
            recomputed.len(),
            syntax.len()
        );
        // The first item owns the new line; nothing else is touched.
        assert_eq!(recomputed, [0]);
        for (path, old) in paths.iter().zip(&syntax).skip(1) {
            let new = engine.parse_definition(path).unwrap().value;
            assert!(std::sync::Arc::ptr_eq(old, &new), "{path}");
        }
    });
}

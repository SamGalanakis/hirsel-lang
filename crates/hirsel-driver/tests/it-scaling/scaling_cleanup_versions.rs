use hirsel_driver::*;
use hirsel_engine::*;
use hirsel_intern::work::{self, Counter};

#[test]
fn cleanup_version_joins_share_unchanged_dataflow() {
    with_compiler_stack(|| {
        let mut counts = Vec::new();
        for count in [4, 8, 16] {
            let mut source =
                String::from("type Rec { items: Array[u64] }\npub fn main() -> u64 {\n");
            for i in 0..count {
                source += &format!(
                    "let mut r{i} = Rec(items: [1, 2])\nlet old{i} = r{i}.items\nr{i}.items[0] = 9\n"
                );
            }
            source += &(0..count)
                .map(|i| format!("old{i}[0]"))
                .collect::<Vec<_>>()
                .join(" + ");
            source += "\n}\n";
            let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
            let file = engine.sources.add("version-joins.hir", source).unwrap();
            let output = check_progress(&mut engine, file, &default_context()).unwrap();
            assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
            let snapshot = engine.export_native_input().unwrap();
            let validated = engine.import_native_input(snapshot).unwrap();
            let mut options = native::Options::new(
                hirsel_lir::layout_types::TargetSpec::x86_64_unknown_linux_gnu(),
            );
            options.regions = false;
            let before = work::read(Counter::RcCleanupVersionRows);
            let program = native::build(&validated, &options).unwrap();
            let rows = work::read(Counter::RcCleanupVersionRows) - before;
            assert!(!program.units.is_empty());
            assert!(rows > 0, "cleanup version work must be measured");
            eprintln!("{count} independent version joins: {rows} rows");
            counts.push(rows);
        }
        // Each doubling adds independent slots and detach branches. The source
        // and every native stage are checked; the counter measures cleanup only.
        for pair in counts.windows(2) {
            assert!(
                pair[1] <= 3 * pair[0],
                "version-join work grows too fast: {counts:?}"
            );
        }
    });
}

use hirsel_driver::*;
use hirsel_engine::*;
fn engine(source: &str) -> QueryEngine {
    let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
    let file = engine.sources.add("callable.hir", source.into()).unwrap();
    engine.parse_file(file, &default_context()).unwrap();
    engine
}
fn path(name: &str) -> DefPath {
    let mut path = entry_path(&default_context());
    path.0.name = name.into();
    path
}
#[test]
fn inferred_literal_results_receive_the_specialized_promise() {
    with_compiler_stack(|| {
        for source in [
            "fn cross[R:sendable](take f:once fn()->R) {}\nfn example(take x:unique String) {cross(fn(){take x})}",
            "fn cross(take f:once fn()->String) {}\nfn example(take x:unique String) {cross(fn(){take x})}",
        ] {
            engine(source).lower_to_tir(&path("example")).unwrap();
        }
    });
}
#[test]
fn callable_header_modes_and_complete_literal_bounds_follow_their_variance() {
    with_compiler_stack(|| {
        for (source, rule) in [
            (
                "fn cross[R:sendable](take f:once fn()->R) {}\nfn example(take x:String) {cross(fn(){take x})}",
                "MODE-4",
            ),
            (
                "fn named()->String {\"x\"}\nfn cross(f:fn()->unique String) {}\nfn example() {cross(named)}",
                "MODE-2",
            ),
            (
                "fn cross(take f:once fn()->unique String) {}\nfn example(take x:String) {cross(fn(){take x})}",
                "MODE-2",
            ),
        ] {
            let mut engine = engine(source);
            let Err(QueryError::Diagnostics(diagnostics)) = engine.lower_to_tir(&path("example"))
            else {
                panic!("expected {rule}: {source}");
            };
            assert_eq!(
                diagnostics[0].rule.as_str(),
                rule,
                "{source}: {diagnostics:?}"
            );
            let line = source[..diagnostics[0].primary.start as usize]
                .bytes()
                .filter(|b| *b == b'\n')
                .count()
                + 1;
            assert_eq!(line, source.lines().count(), "{source}: {diagnostics:?}");
        }
        for source in [
            "fn cross[R:sendable](take f:once fn()->R) {}\nfn example(take x:unique String) {cross(fn(){take x})}",
            "fn cross(take f:once fn()->unique String) {}\nfn example(take x:unique String) {cross(fn()->unique String {take x})}",
            "fn named()->unique String {\"x\"}\nfn cross(f:fn()->unique String) {}\nfn example() {cross(named)}",
            "fn cross(take f:once fn()->unique String) {}\nfn example(take x:unique String) {cross(fn(){take x})}",
            // MODE-2: a header's result may promise uniqueness its type does not.
            "fn cross(take f:once fn()->String) {}\nfn example(take x:unique String) {cross(fn()->unique String {take x})}",
            "fn named()->unique String {\"x\"}\nfn cross(f:fn()->String) {}\nfn example() {cross(named)}",
        ] {
            engine(source).lower_to_tir(&path("example")).unwrap();
        }
    });
}

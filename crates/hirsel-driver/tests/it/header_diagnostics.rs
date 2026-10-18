use hirsel_driver::*;
use hirsel_engine::*;

#[test]
fn header_diagnostics_point_to_the_offending_name_and_earlier_binding() {
    with_compiler_stack(|| {
        for (source, rule, primary, secondary) in [
            ("fn f(a:u8) uses {zz} {}", "LANG-4", "zz", None),
            ("fn f[T,T](a:T) {}", "GEN-1", "T]", None),
            ("fn f(x a:u8,x b:u8) {}", "LANG-12", "x b", None),
            ("type A = X() | X", "LANG-7", "X", None),
            ("fn f(a:u8,a:u8) {}", "LANG-4", "a:u8)", Some("a:u8,")),
            (
                "fn f(a:u8)->u8 { let a:u8=2u8; a }",
                "LANG-4",
                "a:u8=",
                Some("a:u8)"),
            ),
        ] {
            let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
            let file = engine.sources.add("header.hir", source.into()).unwrap();
            let output = check_progress(&mut engine, file, &default_context()).unwrap();
            let diagnostic = output.diagnostics.first().unwrap();
            assert_eq!(diagnostic.rule.as_str(), rule, "{source}");
            let start = source.rfind(primary).unwrap() as u32;
            let name = primary.split([':', ' ', ']']).next().unwrap();
            assert_eq!(
                (diagnostic.primary.start, diagnostic.primary.end),
                (start, start + name.len() as u32),
                "{source}"
            );
            if let Some(previous) = secondary {
                assert_eq!(diagnostic.secondary.len(), 1, "{source}");
                let span = diagnostic.secondary[0].span;
                assert_eq!(
                    (span.start, span.end),
                    (
                        source.find(previous).unwrap() as u32,
                        source.find(previous).unwrap() as u32 + 1
                    ),
                    "{source}"
                );
            }
        }
    });
}

// Corpus headers pin rule/site; this pins the diagnostic's actionable text.
#[test]
fn unknown_variant_lists_the_expected_types_variants() {
    with_compiler_stack(|| {
        for (source, expected) in [
            (
                "fn lookup() -> Result[u64, FileError] { Error(Fault()) }",
                "Fault is not a variant of FileError; variants: FileFault, Canceled, NotGranted, NotFound, PermissionDenied, AlreadyExists, IsDirectory, NotDirectory, DirectoryNotEmpty, CrossDevice, Truncated",
            ),
            (
                "fn lookup() { let x = Fault() }",
                "bare variant needs an unambiguous expected type",
            ),
        ] {
            let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
            let file = engine.sources.add("variant.hir", source.into()).unwrap();
            let output = check_progress(&mut engine, file, &default_context()).unwrap();
            let diagnostic = output.diagnostics.first().unwrap();
            assert_eq!(diagnostic.rule.as_str(), "LANG-2");
            assert_eq!(diagnostic.message, expected);
        }
    });
}

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
                {
                    let at = output.sources.resolve(diagnostic.primary);
                    (at.start, at.end)
                },
                (start, start + name.len() as u32),
                "{source}"
            );
            if let Some(previous) = secondary {
                assert_eq!(diagnostic.secondary.len(), 1, "{source}");
                let span = output.sources.resolve(diagnostic.secondary[0].span);
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

#[test]
fn near_miss_names_cover_scopes_case_underscores_and_renames() {
    with_compiler_stack(|| {
        for (source, before, after, help) in [
            (
                "fn f(length:u64)->u64 { lenght }",
                "lenght",
                "length",
                "help: did you mean `length`?",
            ),
            (
                "type Record { value:u64 }\nfn f(x:Recrod) {}",
                "Recrod",
                "Record",
                "help: did you mean `Record`?",
            ),
            (
                "fn f[Element](x:Elemnet) {}",
                "Elemnet",
                "Element",
                "help: did you mean `Element`?",
            ),
            (
                "fn f(is_empty:bool)->bool { isEmpty }",
                "isEmpty",
                "is_empty",
                "help: did you mean `is_empty`?",
            ),
            (
                "fn f()->u64 { bytes.count(b\"abc\") }",
                "count",
                "len",
                "help: did you mean `bytes.len`?",
            ),
            (
                "fn f()->u64 { byets.len(b\"abc\") }",
                "byets",
                "bytes",
                "help: did you mean `bytes`?",
            ),
            (
                "fn f()->u64 { core@1/bytes/lne(b\"abc\") }",
                "lne",
                "len",
                "help: did you mean `core@1/bytes/len`?",
            ),
        ] {
            let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
            let file = engine.sources.add("near-miss.hir", source.into()).unwrap();
            let output = check_progress(&mut engine, file, &default_context()).unwrap();
            let diagnostic = output.diagnostics.first().unwrap();
            assert_eq!(
                diagnostic.notes,
                [help],
                "{source}: {:?}",
                output.diagnostics
            );
            let fix = diagnostic.fix.as_ref().expect("unambiguous edit");
            assert_eq!(engine.sources.slice(fix.span), Some(before), "{source}");
            assert_eq!(fix.replacement, after);
            let mut repaired = source.to_owned();
            let at = engine.sources.resolve(fix.span);
            repaired.replace_range(at.start as usize..at.end as usize, &fix.replacement);
            let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
            let file = engine.sources.add("near-miss-fixed.hir", repaired).unwrap();
            let fixed = check_progress(&mut engine, file, &default_context()).unwrap();
            assert!(
                !fixed
                    .diagnostics
                    .iter()
                    .any(|d| d.severity == hirsel_diag::Severity::Error),
                "{source}: {:?}",
                fixed.diagnostics
            );
        }
    });
}

#[test]
fn near_miss_help_refreshes_when_a_candidate_is_added_or_removed() {
    with_compiler_stack(|| {
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
        let main = hirsel_syntax::DefPath(hirsel_syntax::ItemPath {
            package: default_context().package,
            modules: vec![],
            name: "example".into(),
        });
        for (declaration, expected) in [
            ("", None),
            (
                "fn length()->u64 { 1 }",
                Some("help: did you mean `length`?"),
            ),
            ("", None),
        ] {
            let source = format!("{declaration}\nfn example()->u64 {{ lenght() }}");
            let file = engine.sources.add("candidate.hir", source).unwrap();
            engine.parse_file(file, &default_context()).unwrap();
            let Err(QueryError::Diagnostics(diagnostics)) = engine.resolve_definition(&main) else {
                panic!("typo must fail");
            };
            let diagnostic = diagnostics.first().unwrap();
            assert_eq!(diagnostic.notes.first().map(String::as_str), expected);
        }
    });
}

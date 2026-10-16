use super::*;

fn fixture(text: &str) -> (SourceMap, Interner, LexOutput) {
    let mut sources = SourceMap::default();
    let file = sources.add("test.hir", text.into()).unwrap();
    let mut interner = Interner::default();
    let output = lex(file, &sources, &mut interner);
    round_trip(text.as_bytes(), &output);
    (sources, interner, output)
}
fn round_trip(bytes: &[u8], output: &LexOutput) {
    let mut end = 0;
    let mut reconstructed = Vec::new();
    for token in &output.tokens {
        assert_eq!(token.span.start as usize, end, "gap or overlap: {token:?}");
        assert!(token.span.end >= token.span.start);
        if token.kind != TokenKind::Eof {
            assert!(token.span.end > token.span.start);
        }
        reconstructed.extend_from_slice(&bytes[token.span.start as usize..token.span.end as usize]);
        end = token.span.end as usize;
    }
    assert_eq!(end, bytes.len());
    assert_eq!(reconstructed, bytes);
    assert_eq!(output.tokens.last().unwrap().kind, TokenKind::Eof);
    assert_eq!(
        output
            .tokens
            .iter()
            .filter(|t| t.kind == TokenKind::Eof)
            .count(),
        1
    );
}
fn significant(output: &LexOutput) -> Vec<&TokenKind> {
    output
        .tokens
        .iter()
        .filter(|t| !matches!(t.kind, TokenKind::Trivia(_) | TokenKind::Eof))
        .map(|t| &t.kind)
        .collect()
}
fn single(text: &str, kind: TokenKind) {
    let (_, _, output) = fixture(text);
    assert!(
        output.diagnostics.is_empty(),
        "{text:?}: {:?}",
        output.diagnostics
    );
    assert_eq!(significant(&output), vec![&kind], "{text:?}");
}
fn error(text: &str, rule: &str, start: u32, end: u32) {
    let (sources, _, output) = fixture(text);
    let diagnostic = &output.diagnostics[0];
    assert_eq!(diagnostic.rule.as_str(), rule, "{text:?}");
    assert_eq!(
        diagnostic.primary,
        Span {
            file: FileId(0),
            start,
            end
        },
        "{text:?}"
    );
    assert!(sources.slice(diagnostic.primary).is_some());
    assert!(
        output
            .tokens
            .iter()
            .any(|t| t.kind == TokenKind::Invalid && t.span.start <= start && t.span.end >= end)
    );
}

#[test]
fn every_keyword_spelling_is_available_in_the_immutable_interner() {
    let text = "fn let mut match if else for in while break continue return take copy unique type distinct flat resource drop box pub opaque const comptime shape heap before requires ensures result where uses of local global once todo extern unsafe true false";
    let (_, interner, output) = fixture(text);
    assert!(output.diagnostics.is_empty());
    assert!(
        significant(&output)
            .iter()
            .all(|kind| matches!(kind, TokenKind::Keyword(_)))
    );
    assert_eq!(interner.len(), text.split_whitespace().count());
    for spelling in text.split_whitespace() {
        // Use only the immutable snapshot API, as the parser does. Calling
        // intern here would hide a missing keyword spelling in the lexer.
        let symbol = (0..interner.len())
            .map(|index| hirsel_intern::Symbol(index as u32))
            .find(|symbol| interner.resolve(*symbol) == Some(spelling));
        assert!(symbol.is_some(), "keyword {spelling:?} was not interned");
    }
}

#[test]
fn integer_bases_separators_and_every_suffix() {
    let suffixes = [
        "", "u8", "u16", "u32", "u64", "u128", "i8", "i16", "i32", "i64", "i128",
    ];
    for core in ["0", "123", "1_234", "00", "0xAb_CD", "0o7_01", "0b10_01"] {
        for suffix in suffixes {
            single(&format!("{core}{suffix}"), TokenKind::Integer);
        }
    }
    for core in [
        "0xdeadbeef",
        "0x1f32",
        "0x1e64",
        "9999999999999999999999999999999999999999999",
    ] {
        single(core, TokenKind::Integer);
    }
    let (_, _, output) = fixture("-128i8 1..2 1..=2 1. .1");
    assert_eq!(
        significant(&output),
        vec![
            &TokenKind::Punct(Punct::Minus),
            &TokenKind::Integer,
            &TokenKind::Integer,
            &TokenKind::Punct(Punct::DotDot),
            &TokenKind::Integer,
            &TokenKind::Integer,
            &TokenKind::Punct(Punct::DotDotEq),
            &TokenKind::Integer,
            &TokenKind::Integer,
            &TokenKind::Punct(Punct::Dot),
            &TokenKind::Punct(Punct::Dot),
            &TokenKind::Integer
        ]
    );
}

#[test]
fn all_float_forms_and_suffixes() {
    for core in [
        "0.0",
        "1_234.5_6",
        "1e2",
        "1E+2",
        "1e-2",
        "1.25e3",
        "1_0.0_1E-1_2",
    ] {
        for suffix in ["", "f32", "f64"] {
            single(&format!("{core}{suffix}"), TokenKind::Float);
        }
    }
    single("1f32", TokenKind::Float);
    single("1_000f64", TokenKind::Float);
}

#[test]
fn malformed_numbers_are_one_token_with_rule_and_span() {
    for text in [
        "0x", "0o", "0b", "0Xff", "0x_1", "0b2", "0o8", "0xG", "1_", "1__2", "1_u8", "0x1_u8",
        "1u7", "1i256", "1name", "123u8x", "0b1f32",
    ] {
        error(text, "SEM-1", 0, text.len() as u32);
        assert_eq!(significant(&fixture(text).2), vec![&TokenKind::Invalid]);
    }
    for text in [
        "1e", "1e+", "1e-", "1e_2", "1.2_", "1.2i32", "1e2u8", "1f16", "1f32x", "1_f64", "1e2_f32",
    ] {
        error(text, "SEM-2", 0, text.len() as u32);
        assert_eq!(significant(&fixture(text).2), vec![&TokenKind::Invalid]);
    }
}

#[test]
fn strings_bytes_and_every_escape() {
    single("\"é🙂 a\"", TokenKind::String);
    single("\"\"", TokenKind::String);
    single("b\"\"", TokenKind::Bytes);
    for escape in [r"\\", r#"\""#, r"\n", r"\r", r"\t", r"\0", r"\x00", r"\x7f"] {
        single(&format!("\"{escape}\""), TokenKind::String);
        single(&format!("b\"{escape}\""), TokenKind::Bytes);
    }
    for escape in [
        r"\u{0}",
        r"\u{A}",
        r"\u{00e9}",
        r"\u{1f642}",
        r"\u{10ffff}",
        r"\u{202e}",
    ] {
        single(&format!("\"{escape}\""), TokenKind::String);
    }
    for byte in 0..=255 {
        single(&format!("b\"\\x{byte:02X}\""), TokenKind::Bytes);
    }
}

#[test]
fn invalid_escapes_have_exact_spans() {
    for escape in [
        r"\q",
        r"\'",
        r"\x",
        r"\x0",
        r"\x80",
        r"\xFF",
        r"\u",
        r"\u{}",
        r"\u{d800}",
        r"\u{110000}",
        r"\u{0000000}",
        r"\u{ffffffffffffffffffff}",
        r"\u{123",
    ] {
        let text = format!("\"{escape}\"");
        error(&text, "SEM-3", 1, 1 + escape.len() as u32);
    }
    error(r#""\xGG""#, "SEM-3", 1, 3);
    error(r#""\u{G}""#, "SEM-3", 1, 4);
    error(r#"b"\u{41}""#, "SEM-3", 2, 8);
    error("\"\\\n", "SEM-3", 1, 2);
    error("\"\\", "SEM-3", 1, 2);
}

#[test]
fn invalid_literal_scalars_and_unterminated_literals() {
    error("\"abc", "SEM-3", 0, 4);
    error("b\"abc", "SEM-3", 0, 5);
    error("\"abc\nlet x = 1", "SEM-3", 0, 4);
    error("\"abc\r\nlet x = 1", "SEM-3", 0, 4);
    for ch in ['\0', '\u{1}', '\t', '\u{1f}', '\u{7f}'] {
        error(&format!("\"{ch}\""), "SEM-3", 1, 1 + ch.len_utf8() as u32);
    }
    error("b\"é\"", "SEM-3", 2, 4);
}

#[test]
fn trivia_doc_comments_nesting_and_physical_newlines() {
    let (_, _, output) =
        fixture(" \t\n\r\n// c\n/// d\r\n//// d\n/* a\n/* b */\r\nz */ /**/ /** d */");
    assert!(output.diagnostics.is_empty());
    assert_eq!(
        output
            .tokens
            .iter()
            .filter_map(|t| match t.kind {
                TokenKind::Trivia(kind) => Some(kind),
                _ => None,
            })
            .collect::<Vec<_>>(),
        vec![
            TriviaKind::Space,
            TriviaKind::Newline,
            TriviaKind::Newline,
            TriviaKind::LineComment,
            TriviaKind::Newline,
            TriviaKind::DocLine,
            TriviaKind::Newline,
            TriviaKind::DocLine,
            TriviaKind::Newline,
            TriviaKind::BlockComment,
            TriviaKind::Space,
            TriviaKind::DocBlock,
            TriviaKind::Space,
            TriviaKind::DocBlock
        ]
    );
    single("/", TokenKind::Punct(Punct::Slash));
    error("/* abc", "LANG-1", 0, 6);
    error("/** abc", "LANG-1", 0, 7);
    error("/* /* */", "LANG-1", 0, 8);
}

#[test]
fn forbidden_source_controls_everywhere_and_unicode_boundaries() {
    let controls = [
        '\u{061c}', '\u{200e}', '\u{200f}', '\u{202a}', '\u{202b}', '\u{202c}', '\u{202d}',
        '\u{202e}', '\u{2066}', '\u{2067}', '\u{2068}', '\u{2069}',
    ];
    for ch in controls {
        for (prefix, suffix) in [
            ("", ""),
            ("// ", ""),
            ("/* ", " */"),
            ("/** ", " */"),
            ("/// ", ""),
            ("\"", "\""),
        ] {
            error(
                &format!("{prefix}{ch}{suffix}"),
                "SEM-3",
                prefix.len() as u32,
                (prefix.len() + ch.len_utf8()) as u32,
            );
        }
    }
    error("\r", "SEM-3", 0, 1);
    error("// c\rnext", "SEM-3", 4, 5);
    error("/*\r*/", "SEM-3", 2, 3);
    error("\u{feff}fn", "SEM-3", 0, 3);
    for text in ["$", "'", "é", "🙂", "\u{b}"] {
        error(text, "LANG-1", 0, text.len() as u32);
    }
}

#[test]
fn errors_continue_to_eof_and_leave_following_statements() {
    let (_, _, output) = fixture("$ 0b2 \"bad\\q\"\n\"open\nlet valid = 2\n/* open");
    assert_eq!(
        output
            .diagnostics
            .iter()
            .map(|d| d.rule.as_str())
            .collect::<Vec<_>>(),
        ["LANG-1", "SEM-1", "SEM-3", "SEM-3", "LANG-1"]
    );
    assert!(
        output
            .tokens
            .iter()
            .any(|t| t.kind == TokenKind::Keyword(Keyword::Let))
    );
    assert!(output.tokens.iter().any(|t| t.kind == TokenKind::Integer));
}

#[test]
fn empty_input_and_stale_file_id_do_not_panic() {
    let (_, _, output) = fixture("");
    assert_eq!(output.tokens.len(), 1);
    assert!(output.diagnostics.is_empty());
    let output = lex(FileId(42), &SourceMap::default(), &mut Interner::default());
    assert_eq!(output.diagnostics[0].rule.as_str(), "LANG-1");
    assert_eq!(
        output.tokens,
        vec![Token {
            kind: TokenKind::Eof,
            span: Span {
                file: FileId(42),
                start: 0,
                end: 0
            }
        }]
    );
}

#[test]
fn deterministic_fuzz_arbitrary_bytes_never_panics_and_round_trips() {
    let mut state = 0x48495253454c_u64;
    for case in 0..10_000usize {
        let len = case % 513;
        let mut bytes = Vec::with_capacity(len);
        for _ in 0..len {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            bytes.push((state >> 24) as u8);
        }
        let output = scan(FileId(0), &bytes, &mut Interner::default());
        round_trip(&bytes, &output);
        for diagnostic in &output.diagnostics {
            assert!(diagnostic.primary.end as usize <= bytes.len());
        }
        // Valid UTF-8 random inputs also exercise the actual SourceMap entry point.
        if let Ok(text) = std::str::from_utf8(&bytes) {
            fixture(text);
        }
    }
    // Deep nesting and long malformed escapes/numbers exercise iterative paths.
    for text in [
        format!("{}{}", "/*".repeat(50_000), "*/".repeat(50_000)),
        format!("\"\\u{{{}}}\"", "f".repeat(100_000)),
        format!("1{}", "_".repeat(100_000)),
    ] {
        fixture(&text);
    }
}

#[test]
fn escaped_newline_reports_one_error_and_recovers_through_the_quote() {
    for head in ["\"", "b\""] {
        for newline in ["\n", "\r\n"] {
            let text = format!("{head}a\\{newline}b\"\nfn f() {{}}");
            let (_, _, output) = fixture(&text);
            assert_eq!(
                output.diagnostics.len(),
                1,
                "{text:?}: {:?}",
                output.diagnostics
            );
            assert_eq!(output.diagnostics[0].rule.as_str(), "SEM-3");
            assert_eq!(
                output.diagnostics[0].message,
                "escape line continuation is forbidden"
            );
            let token = &output.tokens[0];
            assert_eq!(token.kind, TokenKind::Invalid);
            assert_eq!(token.span.end as usize, text.find("\nfn").unwrap());
        }
    }
}

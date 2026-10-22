use super::*;

fn fixture(text: &str) -> (SourceMap, LexOutput) {
    let mut sources = SourceMap::default();
    let file = sources.add("test.hir", text.into()).unwrap();
    let output = lex(file, &sources);
    round_trip(text.as_bytes(), &output);
    (sources, output)
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
    let (_, output) = fixture(text);
    assert!(
        output.diagnostics.is_empty(),
        "{text:?}: {:?}",
        output.diagnostics
    );
    assert_eq!(significant(&output), vec![&kind], "{text:?}");
}
fn error(text: &str, rule: &str, start: u32, end: u32) {
    let (sources, output) = fixture(text);
    let diagnostic = &output.diagnostics[0];
    assert_eq!(diagnostic.rule.as_str(), rule, "{text:?}");
    assert_eq!(
        diagnostic.primary,
        Span {
            anchor: sources.file_anchor(FileId(0)),
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
    let (_, output) = fixture("-128i8 1..2 1..=2 1. .1");
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
        assert_eq!(significant(&fixture(text).1), vec![&TokenKind::Invalid]);
    }
    for text in [
        "1e", "1e+", "1e-", "1e_2", "1.2_", "1.2i32", "1e2u8", "1f16", "1f32x", "1_f64", "1e2_f32",
    ] {
        error(text, "SEM-2", 0, text.len() as u32);
        assert_eq!(significant(&fixture(text).1), vec![&TokenKind::Invalid]);
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
fn bytes_non_ascii_scalars_have_exact_utf8_repairs() {
    // Fixed UTF-8 oracles, independent of the diagnostic's encoder.
    for (scalar, expected, spelling) in [
        ('é', &[0xc3, 0xa9][..], r"\xc3\xa9"),
        ('雪', &[0xe9, 0x9b, 0xaa][..], r"\xe9\x9b\xaa"),
        ('🙂', &[0xf0, 0x9f, 0x99, 0x82][..], r"\xf0\x9f\x99\x82"),
        ('\u{301}', &[0xcc, 0x81][..], r"\xcc\x81"),
    ] {
        let text = format!(r#"b"a\n{scalar}\xffz""#);
        let (sources, output) = fixture(&text);
        assert_eq!(output.diagnostics.len(), 1);
        let d = &output.diagnostics[0];
        assert_eq!(d.rule.as_str(), "SEM-3");
        assert_eq!(d.message, "non-ASCII character in bytes literal");
        assert_eq!(d.primary.start, 5);
        assert_eq!(d.primary.end, 5 + scalar.len_utf8() as u32);
        assert_eq!(sources.slice(d.primary), Some(scalar.to_string().as_str()));
        assert_eq!(
            d.notes,
            [format!(
                "help: byte literals use ASCII plus byte escapes; replace this character with `{spelling}` for its UTF-8 bytes"
            )]
        );
        let suggested = d.notes[0].split('`').nth(1).unwrap();
        let decoded = suggested
            .as_bytes()
            .chunks_exact(4)
            .map(|chunk| {
                assert_eq!(&chunk[..2], b"\\x");
                u8::from_str_radix(std::str::from_utf8(&chunk[2..]).unwrap(), 16).unwrap()
            })
            .collect::<Vec<_>>();
        assert_eq!(decoded, expected);
        assert_eq!(scalar.to_string().as_bytes(), expected);
        single(&text.replace(scalar, suggested), TokenKind::Bytes);
        assert_eq!(significant(&output), [&TokenKind::Invalid]);
    }
    // Adjacent scalars remain separate causes and retain their original bytes.
    let (_, output) = fixture(r#"b"é雪🙂é""#);
    assert_eq!(output.diagnostics.len(), 4);
    assert_eq!(
        output
            .diagnostics
            .iter()
            .map(|d| (d.primary.start, d.primary.end))
            .collect::<Vec<_>>(),
        [(2, 4), (4, 7), (7, 11), (12, 14)]
    );
    // Rust tests/ui/array-slice-vec/byte-literals.rs:5,10 at
    // afe686de2708699b69dd31ed14be326f4e95edfc: ASCII + high hex + tab.
    // Copyright The Rust Project Contributors, MIT; notice: third-party/NOTICES.md.
    // The all-byte and basic escape vectors above already cover its other forms.
    single(r#"b"a\xF0\t""#, TokenKind::Bytes);
}

#[test]
fn bytes_utf8_repairs_do_not_relax_forbidden_characters() {
    for scalar in [
        '\0', '\t', '\u{1f}', '\u{7f}', '\u{85}', '\u{061c}', '\u{200e}', '\u{200f}', '\u{202a}',
        '\u{202b}', '\u{202c}', '\u{202d}', '\u{202e}', '\u{2066}', '\u{2067}', '\u{2068}',
        '\u{2069}',
    ] {
        let text = format!("b\"{scalar}\"");
        error(&text, "SEM-3", 2, 2 + scalar.len_utf8() as u32);
        let (_, output) = fixture(&text);
        assert_eq!(output.diagnostics.len(), 1);
        assert_eq!(output.diagnostics[0].message, "forbidden literal character");
        assert!(output.diagnostics[0].notes.is_empty());
    }
    let bytes = b"b\"\xff\"";
    let output = scan(Anchor(0), bytes);
    round_trip(bytes, &output);
    assert_eq!(output.diagnostics.len(), 1);
    let d = &output.diagnostics[0];
    assert_eq!(d.rule.as_str(), "SEM-3");
    assert_eq!((d.primary.start, d.primary.end), (2, 3));
    assert_eq!(d.message, "forbidden literal character");
    assert!(d.notes.is_empty());
    for ascii in ' '..='~' {
        if !matches!(ascii, '"' | '\\') {
            single(&format!("b\"{ascii}\""), TokenKind::Bytes);
        }
    }
    // Raw bytes have no escapes, so a scalar-only escape repair is inapplicable.
    let (_, output) = fixture("br\"雪\"");
    assert_eq!(
        output.diagnostics[0].message,
        "forbidden raw literal character"
    );
    assert!(output.diagnostics[0].notes.is_empty());
    error(r#"b"\u{e9}""#, "SEM-3", 2, 8);
}

#[test]
fn trivia_doc_comments_nesting_and_physical_newlines() {
    let (_, output) =
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
    let (_, output) = fixture("$ 0b2 \"bad\\q\"\n\"open\nlet valid = 2\n/* open");
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
    let (_, output) = fixture("");
    assert_eq!(output.tokens.len(), 1);
    assert!(output.diagnostics.is_empty());
    let output = lex(FileId(42), &SourceMap::default());
    assert_eq!(output.diagnostics[0].rule.as_str(), "LANG-1");
    assert_eq!(
        output.tokens,
        vec![Token {
            kind: TokenKind::Eof,
            span: Span {
                anchor: Anchor(0),
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
        let output = scan(Anchor(0), &bytes);
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
            let (_, output) = fixture(&text);
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

#[test]
fn malformed_character_and_byte_literals_recover_as_one_token() {
    for (literal, noun) in [
        ("'ab'", "character literal"),
        ("''", "character literal"),
        ("'\\u{d800}'", "character literal"),
        ("'\\x41'", "character literal"),
        ("b'ab'", "byte literal"),
        ("b''", "byte literal"),
        ("b'é'", "byte literal"),
        ("b'\\x80'", "byte literal"),
    ] {
        let (sources, output) = fixture(&format!("let scalar = {literal}\nfn later() {{}}"));
        assert_eq!(
            output.diagnostics.len(),
            1,
            "{literal}: {:?}",
            output.diagnostics
        );
        let diagnostic = &output.diagnostics[0];
        assert_eq!(diagnostic.rule.as_str(), "LANG-1");
        assert!(diagnostic.message.contains(noun), "{diagnostic:?}");
        let invalid: Vec<_> = output
            .tokens
            .iter()
            .filter(|t| t.kind == TokenKind::Invalid)
            .collect();
        assert_eq!(invalid.len(), 1);
        assert_eq!(sources.slice(invalid[0].span), Some(literal));
        assert!(
            output
                .tokens
                .iter()
                .any(|t| sources.slice(t.span) == Some("later"))
        );
    }
}

#[test]
fn character_literals_preserve_source_spellings() {
    for literal in [
        "'a'",
        "'é'",
        "'🐑'",
        "'\\n'",
        "'\\t'",
        "'\\r'",
        "'\\0'",
        "'\\\\'",
        "'\\''",
        "'\\u{10ffff}'",
    ] {
        single(literal, TokenKind::Character);
    }
}

#[test]
fn byte_literals_preserve_source_spellings() {
    for literal in [
        "b'a'",
        "b'~'",
        "b'\\n'",
        "b'\\0'",
        "b'\\\\'",
        "b'\\''",
        "b'\\x7f'",
        "b'\\u{41}'",
    ] {
        single(literal, TokenKind::Byte);
    }
}

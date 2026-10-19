//! Every corpus program must survive the lossless tape, independently of parsing.

hirsel_guard::install!();

use hirsel_lexer::lex;
use hirsel_span::SourceMap;
use hirsel_syntax::TokenKind;
use std::path::{Path, PathBuf};

fn programs(root: &Path) -> Vec<PathBuf> {
    let mut pending = vec![root.to_owned()];
    let mut files = Vec::new();
    while let Some(directory) = pending.pop() {
        for entry in std::fs::read_dir(directory).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                pending.push(path);
            } else if path.extension().is_some_and(|e| e == "hir") {
                files.push(path);
            }
        }
    }
    files.sort();
    files
}

#[test]
fn every_conformance_file_round_trips_and_positive_headers_lex_cleanly() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../conformance");
    let files = programs(&root);
    assert!(
        !files.is_empty(),
        "the corpus check must actually exercise programs"
    );
    let mut positive = 0;
    let mut lexical_rejects = Vec::new();
    for path in &files {
        let bytes = std::fs::read(path).unwrap();
        let text = String::from_utf8(bytes.clone()).expect("corpus source must be UTF-8");
        let header = text
            .lines()
            .find_map(|line| line.strip_prefix("// expect: "))
            .unwrap();
        let mut sources = SourceMap::default();
        let file = sources.add(path, text.clone()).unwrap();
        let output = lex(file, &sources);
        let mut reconstructed = Vec::new();
        let mut cursor = 0;
        for token in &output.tokens {
            assert_eq!(token.span.anchor, sources.file_anchor(file));
            assert_eq!(
                token.span.start,
                cursor,
                "{}: tape gap or overlap",
                path.display()
            );
            assert!(token.span.end >= cursor);
            assert!(token.kind == TokenKind::Eof || token.span.end > cursor);
            reconstructed.extend_from_slice(sources.slice(token.span).unwrap().as_bytes());
            cursor = token.span.end;
        }
        assert_eq!(cursor as usize, bytes.len());
        assert_eq!(reconstructed, bytes, "{}", path.display());
        assert_eq!(output.tokens.last().unwrap().kind, TokenKind::Eof);
        assert_eq!(
            output
                .tokens
                .iter()
                .filter(|t| t.kind == TokenKind::Eof)
                .count(),
            1
        );
        if matches!(header, "accept" | "run") {
            positive += 1;
            assert!(
                output.diagnostics.is_empty(),
                "{} ({header}): {:?}\nprovisional annotations: {:?}",
                path.display(),
                output.diagnostics,
                text.lines()
                    .filter(|line| line.starts_with("// spec:"))
                    .collect::<Vec<_>>()
            );
        }
        if let (Some(expected), Some(first)) =
            (header.strip_prefix("reject "), output.diagnostics.first())
        {
            let (rule, line) = expected.split_once(" at line ").unwrap();
            if first.rule.as_str() == rule
                && sources.location(first.primary).unwrap().line == line.parse::<u32>().unwrap()
            {
                lexical_rejects.push(format!(
                    "{}: {rule} at line {line}",
                    path.strip_prefix(&root).unwrap().display()
                ));
            }
        }
    }
    println!(
        "Corpus: {} files round-trip; {positive} accept/run files have zero lexer diagnostics; {} lexical rejects match the header.",
        files.len(),
        lexical_rejects.len()
    );
    for reject in lexical_rejects {
        println!("Lexical reject: {reject}");
    }
}

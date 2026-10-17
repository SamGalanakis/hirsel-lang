//! Deliberately small corpus fixture; product parsing never imports this tokenizer.
use hirsel_intern::Interner;
use hirsel_span::{FileId, SourceMap, Span};
use hirsel_syntax::*;

pub fn tokenize(file: FileId, source: &str, names: &mut Interner) -> Vec<Token> {
    let mut tokens = vec![];
    let mut pos = 0;
    while pos < source.len() {
        let start = pos;
        let rest = &source[pos..];
        let kind = if rest.starts_with("//") {
            pos += rest.find(['\r', '\n']).unwrap_or(rest.len());
            TokenKind::Trivia(if rest.starts_with("///") {
                TriviaKind::DocLine
            } else {
                TriviaKind::LineComment
            })
        } else if rest.starts_with("/*") {
            pos += 2;
            let mut depth = 1;
            while pos < source.len() && depth > 0 {
                if source[pos..].starts_with("/*") {
                    depth += 1;
                    pos += 2;
                } else if source[pos..].starts_with("*/") {
                    depth -= 1;
                    pos += 2;
                } else {
                    pos += source[pos..].chars().next().unwrap().len_utf8();
                }
            }
            TokenKind::Trivia(if rest.starts_with("/**") {
                TriviaKind::DocBlock
            } else {
                TriviaKind::BlockComment
            })
        } else if rest.starts_with("\r\n") || rest.starts_with('\n') {
            pos += if rest.starts_with("\r\n") { 2 } else { 1 };
            TokenKind::Trivia(TriviaKind::Newline)
        } else if rest.starts_with([' ', '\t']) {
            pos += rest
                .bytes()
                .take_while(|b| matches!(b, b' ' | b'\t'))
                .count();
            TokenKind::Trivia(TriviaKind::Space)
        } else if rest.starts_with('\'') {
            pos += 1;
            while pos < source.len() {
                let c = source[pos..].chars().next().unwrap();
                pos += c.len_utf8();
                if c == '\'' {
                    break;
                }
                if c == '\\' && pos < source.len() {
                    pos += source[pos..].chars().next().unwrap().len_utf8();
                }
            }
            TokenKind::Character
        } else if rest.starts_with('"') || rest.starts_with("b\"") {
            let bytes = rest.starts_with('b');
            pos += if bytes { 2 } else { 1 };
            while pos < source.len() {
                let c = source[pos..].chars().next().unwrap();
                pos += c.len_utf8();
                if c == '"' {
                    break;
                }
                if c == '\\' && pos < source.len() {
                    pos += source[pos..].chars().next().unwrap().len_utf8();
                }
            }
            if bytes {
                TokenKind::Bytes
            } else {
                TokenKind::String
            }
        } else if rest.as_bytes()[0].is_ascii_digit() {
            pos += 1;
            let mut float = false;
            while pos < source.len() {
                let b = source.as_bytes()[pos];
                if b == b'.'
                    && source
                        .as_bytes()
                        .get(pos + 1)
                        .is_some_and(u8::is_ascii_digit)
                {
                    float = true;
                    pos += 1;
                } else if b.is_ascii_alphanumeric() || b == b'_' {
                    if matches!(b, b'e' | b'E') && !rest.starts_with("0x") {
                        float = true;
                    }
                    pos += 1;
                    if matches!(b, b'e' | b'E')
                        && source
                            .as_bytes()
                            .get(pos)
                            .is_some_and(|b| matches!(b, b'+' | b'-'))
                    {
                        pos += 1;
                    }
                } else {
                    break;
                }
            }
            let text = &source[start..pos];
            if float || text.ends_with("f32") || text.ends_with("f64") {
                TokenKind::Float
            } else {
                TokenKind::Integer
            }
        } else if rest.as_bytes()[0].is_ascii_alphabetic() || rest.starts_with('_') {
            pos += rest
                .bytes()
                .take_while(|b| b.is_ascii_alphanumeric() || *b == b'_')
                .count();
            let text = &source[start..pos];
            let symbol = names.intern(text); // Keyword members need the same immutable snapshot.
            match text {
                "fn" => TokenKind::Keyword(Keyword::Fn),
                "let" => TokenKind::Keyword(Keyword::Let),
                "mut" => TokenKind::Keyword(Keyword::Mut),
                "match" => TokenKind::Keyword(Keyword::Match),
                "if" => TokenKind::Keyword(Keyword::If),
                "else" => TokenKind::Keyword(Keyword::Else),
                "for" => TokenKind::Keyword(Keyword::For),
                "in" => TokenKind::Keyword(Keyword::In),
                "while" => TokenKind::Keyword(Keyword::While),
                "break" => TokenKind::Keyword(Keyword::Break),
                "continue" => TokenKind::Keyword(Keyword::Continue),
                "return" => TokenKind::Keyword(Keyword::Return),
                "take" => TokenKind::Keyword(Keyword::Take),
                "copy" => TokenKind::Keyword(Keyword::Copy),
                "unique" => TokenKind::Keyword(Keyword::Unique),
                "type" => TokenKind::Keyword(Keyword::Type),
                "distinct" => TokenKind::Keyword(Keyword::Distinct),
                "flat" => TokenKind::Keyword(Keyword::Flat),
                "resource" => TokenKind::Keyword(Keyword::Resource),
                "drop" => TokenKind::Keyword(Keyword::Drop),
                "box" => TokenKind::Keyword(Keyword::Box),
                "pub" => TokenKind::Keyword(Keyword::Pub),
                "opaque" => TokenKind::Keyword(Keyword::Opaque),
                "const" => TokenKind::Keyword(Keyword::Const),
                "comptime" => TokenKind::Keyword(Keyword::Comptime),
                "shape" => TokenKind::Keyword(Keyword::Shape),
                "heap" => TokenKind::Keyword(Keyword::Heap),
                "before" => TokenKind::Keyword(Keyword::Before),
                "requires" => TokenKind::Keyword(Keyword::Requires),
                "ensures" => TokenKind::Keyword(Keyword::Ensures),
                "result" => TokenKind::Keyword(Keyword::Result),
                "where" => TokenKind::Keyword(Keyword::Where),
                "uses" => TokenKind::Keyword(Keyword::Uses),
                "of" => TokenKind::Keyword(Keyword::Of),
                "local" => TokenKind::Keyword(Keyword::Local),
                "global" => TokenKind::Keyword(Keyword::Global),
                "once" => TokenKind::Keyword(Keyword::Once),
                "todo" => TokenKind::Keyword(Keyword::Todo),
                "extern" => TokenKind::Keyword(Keyword::Extern),
                "unsafe" => TokenKind::Keyword(Keyword::Unsafe),
                "true" => TokenKind::Keyword(Keyword::True),
                "false" => TokenKind::Keyword(Keyword::False),
                _ => TokenKind::Ident(symbol),
            }
        } else {
            let punctuation = [
                ("..=", Punct::DotDotEq),
                ("<<=", Punct::ShlEq),
                (">>=", Punct::ShrEq),
                ("..", Punct::DotDot),
                ("->", Punct::Arrow),
                ("=>", Punct::FatArrow),
                ("&&", Punct::AndAnd),
                ("||", Punct::OrOr),
                ("<<", Punct::Shl),
                (">>", Punct::Shr),
                ("==", Punct::EqEq),
                ("!=", Punct::Ne),
                ("<=", Punct::Le),
                (">=", Punct::Ge),
                ("+=", Punct::PlusEq),
                ("-=", Punct::MinusEq),
                ("*=", Punct::StarEq),
                ("/=", Punct::SlashEq),
                ("%=", Punct::PercentEq),
                ("&=", Punct::AmpEq),
                ("|=", Punct::PipeEq),
                ("^=", Punct::CaretEq),
                ("(", Punct::LParen),
                (")", Punct::RParen),
                ("[", Punct::LBracket),
                ("]", Punct::RBracket),
                ("{", Punct::LBrace),
                ("}", Punct::RBrace),
                (",", Punct::Comma),
                (":", Punct::Colon),
                (";", Punct::Semicolon),
                (".", Punct::Dot),
                ("@", Punct::At),
                ("?", Punct::Question),
                ("+", Punct::Plus),
                ("-", Punct::Minus),
                ("*", Punct::Star),
                ("/", Punct::Slash),
                ("%", Punct::Percent),
                ("!", Punct::Bang),
                ("~", Punct::Tilde),
                ("&", Punct::Amp),
                ("|", Punct::Pipe),
                ("^", Punct::Caret),
                ("=", Punct::Eq),
                ("<", Punct::Lt),
                (">", Punct::Gt),
            ];
            if let Some((text, p)) = punctuation.iter().find(|(text, _)| rest.starts_with(text)) {
                pos += text.len();
                TokenKind::Punct(*p)
            } else {
                pos += rest.chars().next().unwrap().len_utf8();
                TokenKind::Invalid
            }
        };
        tokens.push(Token {
            kind,
            span: Span {
                file,
                start: start as u32,
                end: pos as u32,
            },
        });
    }
    tokens.push(Token {
        kind: TokenKind::Eof,
        span: Span {
            file,
            start: pos as u32,
            end: pos as u32,
        },
    });
    tokens
}

pub fn context() -> FileContext {
    FileContext {
        package: PackageId {
            name: "fixture".into(),
            version: "1".into(),
        },
        modules: vec!["examples".into()],
    }
}
pub fn parse(source: &str) -> (hirsel_parser::ParseOutput, SourceMap) {
    let mut sources = SourceMap::default();
    let file = sources.add("fixture.hir", source.into()).unwrap();
    let mut names = Interner::default();
    let tokens = tokenize(file, source, &mut names);
    (
        hirsel_parser::parse(file, tokens, &context(), &sources, &names),
        sources,
    )
}

/// Runs deep-nesting probes on a thread with the compiler's stack size
/// (`hirsel_engine::COMPILER_STACK_BYTES`), without linking the driver. The
/// driver's own tests check that the parser limit fits that stack.
pub fn with_compiler_stack<T: Send>(work: impl FnOnce() -> T + Send) -> T {
    std::thread::scope(|scope| {
        std::thread::Builder::new()
            .stack_size(128 * 1024 * 1024)
            .spawn_scoped(scope, work)
            .expect("probe thread creation failed")
            .join()
            .unwrap_or_else(|panic| std::panic::resume_unwind(panic))
    })
}

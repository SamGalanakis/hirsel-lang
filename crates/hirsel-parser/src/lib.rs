//! Recovery parser for the frozen grammar, independent of lexing and name lookup.

#[cfg(test)]
hirsel_guard::install!();

use hirsel_diag::{Diagnostic, RuleId};
use hirsel_intern::Symbol;
use hirsel_span::{FileId, SourceMap, Span};
use hirsel_syntax::*;

mod conditions;
mod declarations;
mod depth;
mod expressions;
mod formats;
mod types;

type Parsed<T> = Result<T, ()>;

/// Implementation safeguard, not a language rejection (spec issue #33).
/// Bounds constructed tree height and nested grammar entries, including grouping.
pub const MAX_PARSE_DEPTH: usize = 1024;

/// A lossless snapshot and diagnostics in source/recovery order.
#[derive(Debug)]
pub struct ParseOutput {
    pub syntax: SyntaxFile,
    pub diagnostics: Vec<Diagnostic>,
    /// Incomplete tooling outcome; the recovered tree must not be published.
    pub nesting_limit: Option<Span>,
}

/// Parse a lexer-owned tape without mutating the source.
/// Invalid/missing EOF and spans are handled without indexing panics.
pub fn parse(
    file: FileId,
    tokens: Vec<Token>,
    context: &FileContext,
    sources: &SourceMap,
) -> ParseOutput {
    let significant = tokens
        .iter()
        .enumerate()
        .filter_map(|(i, t)| (!matches!(t.kind, TokenKind::Trivia(_))).then_some(i))
        .collect();
    let mut parser = Parser {
        file,
        tokens,
        significant,
        pos: 0,
        sources,
        tree: SyntaxTree::default(),
        diagnostics: vec![],
        recovered_names: std::collections::BTreeSet::new(),
        whitespace: 0,
        depth: 0,
        heights: depth::Heights::default(),
        nesting_limit: None,
        formats: 0,
    };
    let mut roots = vec![];
    while !parser.eof() && parser.nesting_limit.is_none() {
        if parser.eat(";") {
            continue;
        }
        let start = parser.pos;
        if let Ok(item) = parser.item(context) {
            roots.push(ItemId(parser.tree.items.len() as u32));
            parser.tree.items.push(item);
            if parser.end(false).is_err() {
                parser.recover_item(start);
            }
        } else {
            parser.recover_item(start);
        }
    }
    parser.attach_trivia();
    ParseOutput {
        syntax: SyntaxFile {
            has_syntax_errors: parser.nesting_limit.is_some()
                || !parser.diagnostics.is_empty()
                || parser
                    .tokens
                    .iter()
                    .any(|t| matches!(t.kind, TokenKind::Invalid)),
            file,
            tokens: parser.tokens,
            tree: parser.tree,
            roots,
        },
        diagnostics: parser.diagnostics,
        nesting_limit: parser.nesting_limit,
    }
}

struct Parser<'a> {
    file: FileId,
    tokens: Vec<Token>,
    significant: Vec<usize>,
    pos: usize,
    sources: &'a SourceMap,
    tree: SyntaxTree,
    diagnostics: Vec<Diagnostic>,
    recovered_names: std::collections::BTreeSet<String>,
    whitespace: usize,
    depth: usize,
    heights: depth::Heights,
    nesting_limit: Option<Span>,
    /// How many f-strings enclose the current position (SEM-3).
    formats: usize,
}

impl Parser<'_> {
    fn token(&self, offset: usize) -> Option<&Token> {
        self.significant
            .get(self.pos.saturating_add(offset))
            .and_then(|&i| self.tokens.get(i))
    }
    fn eof(&self) -> bool {
        self.token(0).is_none_or(|t| t.kind == TokenKind::Eof)
    }
    fn text_at(&self, offset: usize) -> &str {
        self.token(offset)
            .and_then(|t| self.sources.slice(t.span))
            .unwrap_or("")
    }
    fn at(&self, word: &str) -> bool {
        // Spelling is only used for grammar terminals, never to classify identifiers.
        !self.eof() && self.text_at(0) == word
    }
    fn eat(&mut self, word: &str) -> bool {
        if self.at(word) {
            self.pos += 1;
            true
        } else {
            false
        }
    }
    fn expect(&mut self, word: &str, rule: RuleId) -> Parsed<()> {
        if self.eat(word) {
            Ok(())
        } else {
            let mut span = self.span();
            if matches!(word, ")" | "]" | "}") {
                span.end = span.start;
            }
            // Spec #55 retains specialized rule IDs pending the malformed-syntax ruling.
            self.fail_at(rule, span, format!("expected `{word}`"))
        }
    }
    fn span(&self) -> Span {
        self.token(0).map(|t| t.span).unwrap_or(Span {
            file: self.file,
            start: self
                .sources
                .file(self.file)
                .map_or(0, |s| s.text().len() as u32),
            end: self
                .sources
                .file(self.file)
                .map_or(0, |s| s.text().len() as u32),
        })
    }
    fn fail<T>(&mut self, rule: RuleId, message: impl Into<String>) -> Parsed<T> {
        self.fail_at(rule, self.span(), message)
    }
    fn fail_at<T>(&mut self, rule: RuleId, span: Span, message: impl Into<String>) -> Parsed<T> {
        self.diagnostics
            .push(Diagnostic::error(rule, span, message));
        Err(())
    }
    fn origin(&self, start: usize) -> Origin {
        let first = self
            .significant
            .get(start)
            .copied()
            .unwrap_or(self.tokens.len());
        let last = self
            .pos
            .checked_sub(1)
            .filter(|&p| p >= start)
            .and_then(|p| self.significant.get(p))
            .copied();
        let span = self.tokens.get(first).map_or(self.span(), |t| t.span);
        Origin {
            span: Span {
                end: last
                    .and_then(|i| self.tokens.get(i))
                    .map_or(span.start, |t| t.span.end)
                    .max(span.start),
                ..span
            },
            tokens: TokenRange {
                start: first as u32,
                end: last.map_or(first, |i| i + 1) as u32,
            },
        }
    }
    fn newline(&self) -> bool {
        self.newline_at(self.pos)
    }
    fn newline_at(&self, position: usize) -> bool {
        let first = position
            .checked_sub(1)
            .and_then(|p| self.significant.get(p))
            .map_or(0, |i| i + 1);
        let end = self
            .significant
            .get(position)
            .copied()
            .unwrap_or(self.tokens.len());
        self.tokens.get(first..end).unwrap_or(&[]).iter().any(|t| {
            matches!(t.kind, TokenKind::Trivia(TriviaKind::Newline))
                || self.sources.slice(t.span).is_some_and(|s| s.contains('\n'))
        })
    }
    fn continuing(&self) -> bool {
        self.whitespace > 0 || !self.newline()
    }
    fn ident(&self, offset: usize) -> bool {
        matches!(
            self.token(offset).map(|t| &t.kind),
            Some(TokenKind::Ident(_))
        ) || (matches!(
            self.token(offset).map(|t| &t.kind),
            Some(TokenKind::Keyword(_))
        ) && self.recovered_names.contains(self.text_at(offset)))
    }
    fn name(&mut self) -> Parsed<Symbol> {
        match self.token(0).map(|t| &t.kind) {
            Some(TokenKind::Ident(s)) if s.as_str() != "_" => {
                let s = *s;
                self.pos += 1;
                Ok(s)
            }
            Some(TokenKind::Keyword(_)) => {
                let spelling = self.text_at(0).to_owned();
                if self.recovered_names.insert(spelling.clone()) {
                    let role = match spelling.as_str() {
                        "todo" => "an unfinished-code placeholder",
                        "before" => "a pre-state snapshot in a postcondition",
                        "result" => "the returned value in a postcondition",
                        "distinct" => "a distinct type declaration",
                        "fn" => "a function declaration or closure",
                        "type" | "resource" | "flat" | "opaque" => "a type declaration",
                        "if" | "else" | "match" => "conditional control flow",
                        "for" | "while" | "break" | "continue" | "in" => "loop syntax",
                        "return" => "returning from a function",
                        "take" | "copy" | "unique" | "mut" => "an ownership operation",
                        "requires" | "ensures" => "a contract clause",
                        "uses" => "an authority restriction",
                        "of" | "local" | "global" | "once" => "a type qualifier",
                        "true" | "false" => "a boolean literal",
                        "let" | "const" | "pub" => "a declaration",
                        "box" => "boxed storage",
                        "drop" => "a resource finalizer",
                        "comptime" => "compile-time evaluation",
                        "shape" => "a type shape expression",
                        "heap" => "the heap allocation destination",
                        "where" => "a refinement predicate",
                        "extern" | "unsafe" => "foreign or unsafe operations",
                        _ => "language syntax",
                    };
                    self.diagnostics.push(Diagnostic::error(
                        const { RuleId::of("LANG-14") },
                        self.span(),
                        format!("`{spelling}` is reserved for {role}; choose another name"),
                    ));
                }
                self.pos += 1;
                Ok(Symbol::intern(&spelling))
            }
            _ => self.fail(const { RuleId::of("LANG-4") }, "expected a name"),
        }
    }
    fn member(&mut self) -> Parsed<Symbol> {
        if self.ident(0) {
            return self.name();
        }
        if matches!(self.token(0).map(|t| &t.kind), Some(TokenKind::Keyword(_))) {
            let member = Symbol::intern(self.text_at(0));
            self.pos += 1;
            return Ok(member);
        }
        self.name()
    }
    fn enter(&mut self) -> Parsed<()> {
        if self.depth >= MAX_PARSE_DEPTH {
            if self.nesting_limit.is_none() {
                self.nesting_limit = Some(self.span());
            }
            return Err(());
        }
        self.depth += 1;
        Ok(())
    }
    fn check_height(&mut self, height: usize) {
        if height > MAX_PARSE_DEPTH && self.nesting_limit.is_none() {
            self.nesting_limit = Some(self.span());
        }
    }
    fn complete<T>(&self, result: Parsed<T>) -> Parsed<T> {
        if self.nesting_limit.is_some() {
            Err(())
        } else {
            result
        }
    }
    fn list<T>(
        &mut self,
        close: &str,
        mut entry: impl FnMut(&mut Self) -> Parsed<T>,
    ) -> Parsed<Vec<T>> {
        self.whitespace += 1;
        let result = (|| {
            let mut values = vec![];
            while !self.eat(close) {
                if self.eof() {
                    return self.fail(
                        const { RuleId::of("LANG-1") },
                        format!("expected `{close}`"),
                    );
                }
                values.push(entry(self)?);
                if self.eat(close) {
                    break;
                }
                self.expect(",", const { RuleId::of("LANG-1") })?;
            }
            Ok(values)
        })();
        self.whitespace -= 1;
        result
    }
    fn required<T>(&mut self, parse: impl FnOnce(&mut Self) -> Parsed<T>) -> Parsed<T> {
        // This enclosing form cannot be complete until its required later token
        // (assignment RHS/control body) arrives: rule 2 treats NL as whitespace.
        self.whitespace += 1;
        let result = parse(self);
        self.whitespace -= 1;
        result
    }
    fn entry_end(&mut self, close: &str) -> Parsed<()> {
        if self.at(close) || self.eat(",") || self.newline() {
            Ok(())
        } else {
            self.fail(
                const { RuleId::of("LANG-1") },
                "expected comma or newline between entries",
            )
        }
    }
    fn end(&mut self, implicit: bool) -> Parsed<bool> {
        if self.eat(";") {
            return Ok(true);
        }
        if self.eof() || self.at("}") || self.newline() || implicit {
            return Ok(false);
        }
        self.fail(
            const { RuleId::of("LANG-1") },
            "expected newline or semicolon after complete statement/item",
        )
    }
    fn item_start(&self) -> bool {
        matches!(
            self.text_at(0),
            "@" | "pub" | "fn" | "extern" | "const" | "type" | "flat" | "resource" | "distinct"
        )
    }
    fn recover_item(&mut self, start: usize) {
        let mut braces = self.recovery_braces(start);
        if self.pos == start && !self.eof() {
            self.recovery_step(&mut braces);
        }
        while !self.eof() {
            if braces == 0 && self.item_start() && self.newline() {
                break;
            }
            if braces == 0 && self.eat(";") {
                break;
            }
            self.recovery_step(&mut braces);
        }
    }
    fn recover_statement(&mut self, start: usize) {
        let mut braces = self.recovery_braces(start);
        if self.pos == start && !self.eof() && !self.at("}") {
            self.recovery_step(&mut braces);
        }
        while !self.eof() {
            if braces == 0 {
                if self.at("}") || self.newline() {
                    break;
                }
                if self.eat(";") {
                    break;
                }
            }
            self.recovery_step(&mut braces);
        }
    }
    fn recovery_braces(&self, start: usize) -> usize {
        // Include delimiters already consumed by the failed production.
        self.significant[start..self.pos]
            .iter()
            .fold(0usize, |depth, &i| match self.tokens[i].kind {
                TokenKind::Punct(Punct::LBrace) | TokenKind::Format(FormatPiece::Head) => depth + 1,
                TokenKind::Punct(Punct::RBrace) | TokenKind::Format(FormatPiece::Tail) => {
                    depth.saturating_sub(1)
                }
                _ => depth,
            })
    }
    /// An f-string's head and tail pieces enclose its holes like braces.
    fn recovery_step(&mut self, braces: &mut usize) {
        match self.token(0).map(|t| &t.kind) {
            Some(TokenKind::Punct(Punct::LBrace) | TokenKind::Format(FormatPiece::Head)) => {
                *braces += 1
            }
            Some(TokenKind::Punct(Punct::RBrace) | TokenKind::Format(FormatPiece::Tail)) => {
                *braces = braces.saturating_sub(1)
            }
            _ => {}
        }
        self.pos += 1;
    }
    fn attach_trivia(&mut self) {
        let mut owned_start = 0usize;
        for item in &mut self.tree.items {
            let end = item.origin.tokens.end as usize;
            let mut owned_end = end;
            let mut comment = false;
            for (i, t) in self.tokens.iter().enumerate().skip(end) {
                match t.kind {
                    TokenKind::Punct(Punct::Semicolon) if i == end => {
                        owned_end = i + 1;
                    }
                    TokenKind::Trivia(TriviaKind::Newline) => {
                        if comment {
                            owned_end = i + 1;
                        }
                        break;
                    }
                    TokenKind::Trivia(kind) => {
                        if kind != TriviaKind::Space {
                            comment = true;
                            owned_end = i + 1;
                        }
                        if self.sources.slice(t.span).is_some_and(|s| s.contains('\n')) {
                            break;
                        }
                    }
                    _ => break,
                }
            }
            item.owned_tokens = TokenRange {
                start: owned_start as u32,
                end: owned_end as u32,
            };
            let bytes = self
                .tokens
                .get(owned_start)
                .zip(self.tokens.get(owned_end.saturating_sub(1)))
                .and_then(|(a, b)| {
                    self.sources.slice(Span {
                        file: self.file,
                        start: a.span.start,
                        end: b.span.end,
                    })
                })
                .unwrap_or("");
            item.text_hash = ContentHash::of_bytes(bytes.as_bytes());
            owned_start = owned_end;
        }
    }
    fn path(&mut self, type_context: bool) -> Parsed<Path> {
        let mut segments = vec![self.name()?];
        let mut package = None;
        if self.at("@") && self.text_at(2) == "/" {
            let head = self.pos - 1;
            for p in head..head + 3 {
                let a = self.significant.get(p).and_then(|&i| self.tokens.get(i));
                let b = self
                    .significant
                    .get(p + 1)
                    .and_then(|&i| self.tokens.get(i));
                if !a.zip(b).is_some_and(|(a, b)| a.span.end == b.span.start) {
                    return self.fail(
                        const { RuleId::of("LANG-4") },
                        "package/version head must be adjacent",
                    );
                }
            }
            self.pos += 1;
            if !matches!(self.token(0).map(|t| &t.kind), Some(TokenKind::Integer)) {
                return self.fail(
                    const { RuleId::of("LANG-4") },
                    "expected numeric package version",
                );
            }
            let version = self.text_at(0).to_owned();
            if size_value(&version).is_none() {
                return self.fail(
                    const { RuleId::of("LANG-4") },
                    "package version must be unsuffixed integer",
                );
            }
            self.pos += 1;
            self.expect("/", const { RuleId::of("LANG-4") })?;
            package = Some(PackageId {
                name: segments[0].as_str().into(),
                version,
            });
            segments = vec![self.name()?];
        }
        let mut dotted = false;
        while self.continuing() {
            let dot = self.at(".");
            // #59 retains greedy module separators in the ambiguous a / b
            // spelling. After a member dot or before a non-name, / is division.
            let slash =
                self.at("/") && (type_context || (package.is_some() && !dotted && self.ident(1)));
            if !dot && !slash {
                break;
            }
            dotted |= dot;
            self.pos += 1;
            segments.push(self.member()?);
        }
        Ok(Path { package, segments })
    }
    fn literal(&mut self) -> Option<Literal> {
        let text = self.text_at(0).to_owned();
        let literal = match &self.token(0)?.kind {
            TokenKind::Integer => Literal::Integer(text),
            TokenKind::Float => Literal::Float(text),
            TokenKind::Character => Literal::Character(text),
            TokenKind::Byte => Literal::Byte(text),
            TokenKind::String => Literal::String(text),
            TokenKind::Bytes => Literal::Bytes(text),
            TokenKind::Keyword(Keyword::True) => Literal::Bool(true),
            TokenKind::Keyword(Keyword::False) => Literal::Bool(false),
            _ => return None,
        };
        self.pos += 1;
        Some(literal)
    }
}

fn size_value(text: &str) -> Option<u64> {
    let text = text.replace('_', "");
    let (digits, radix) = if let Some(s) = text.strip_prefix("0x") {
        (s, 16)
    } else if let Some(s) = text.strip_prefix("0o") {
        (s, 8)
    } else if let Some(s) = text.strip_prefix("0b") {
        (s, 2)
    } else {
        (text.as_str(), 10)
    };
    u64::from_str_radix(digits, radix).ok()
}

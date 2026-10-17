//! Lossless, iterative tokenization of the M0 grammar (LANG-1, SEM-1/2/3).
//! Newlines remain physical source boundaries; statement completion is parser-owned.
use hirsel_diag::{Diagnostic, RuleId};
use hirsel_intern::Interner;
use hirsel_span::{FileId, SourceMap, Span};
use hirsel_syntax::{Keyword, Punct, Token, TokenKind, TriviaKind};

/// Complete source-order tape, including invalid text and an empty EOF token.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct LexOutput {
    pub tokens: Vec<Token>,
    pub diagnostics: Vec<Diagnostic>,
}

/// Tokenize an immutable UTF-8 source snapshot. A stale FileId yields an empty
/// EOF tape and a LANG-1 diagnostic, rather than indexing another snapshot.
pub fn lex(file: FileId, sources: &SourceMap, interner: &mut Interner) -> LexOutput {
    match sources.file(file) {
        Some(source) => scan(file, source.text().as_bytes(), interner),
        None => {
            let mut scanner = Scanner::new(file, &[], interner);
            scanner.error(
                const { RuleId::of("LANG-1") },
                0,
                0,
                "source file is unavailable",
            );
            scanner.finish()
        }
    }
}

// The same byte scanner is exercised on arbitrary bytes by fuzz tests. Production
// rejects invalid UTF-8/oversize files before SourceMap creation (SEM-3, #21).
fn scan(file: FileId, bytes: &[u8], interner: &mut Interner) -> LexOutput {
    let mut scanner = Scanner::new(file, bytes, interner);
    while scanner.pos < bytes.len() {
        scanner.next();
    }
    scanner.finish()
}

struct Scanner<'a> {
    file: FileId,
    bytes: &'a [u8],
    pos: usize,
    interner: &'a mut Interner,
    output: LexOutput,
}
impl<'a> Scanner<'a> {
    fn new(file: FileId, bytes: &'a [u8], interner: &'a mut Interner) -> Self {
        Self {
            file,
            bytes,
            pos: 0,
            interner,
            output: LexOutput::default(),
        }
    }
    fn span(&self, start: usize, end: usize) -> Span {
        Span {
            file: self.file,
            start: start as u32,
            end: end as u32,
        }
    }
    fn error(&mut self, rule: RuleId, start: usize, end: usize, message: &str) {
        self.output
            .diagnostics
            .push(Diagnostic::error(rule, self.span(start, end), message));
    }

    fn emit(&mut self, start: usize, kind: TokenKind) {
        self.output.tokens.push(Token {
            kind,
            span: self.span(start, self.pos),
        });
    }
    fn finish(mut self) -> LexOutput {
        self.output.tokens.push(Token {
            kind: TokenKind::Eof,
            span: self.span(self.pos, self.pos),
        });
        self.output
    }
    fn starts(&self, spelling: &[u8]) -> bool {
        self.bytes[self.pos..].starts_with(spelling)
    }
    fn next(&mut self) {
        let start = self.pos;
        let errors = self.output.diagnostics.len();
        let kind = match self.bytes[self.pos] {
            b' ' | b'\t' => {
                while self
                    .bytes
                    .get(self.pos)
                    .is_some_and(|b| matches!(b, b' ' | b'\t'))
                {
                    self.pos += 1;
                }
                TokenKind::Trivia(TriviaKind::Space)
            }
            b'\n' => {
                self.pos += 1;
                TokenKind::Trivia(TriviaKind::Newline)
            }
            b'\r' if self.starts(b"\r\n") => {
                self.pos += 2;
                TokenKind::Trivia(TriviaKind::Newline)
            }
            b'/' if self.starts(b"//") => self.line_comment(),
            b'/' if self.starts(b"/*") => self.block_comment(),
            b'"' => self.string(false),
            b'b' if self.starts(b"b\"") => self.string(true),
            b'0'..=b'9' => self.number(),
            b if letter(b) => {
                self.pos += 1;
                while self.bytes.get(self.pos).is_some_and(|b| name_byte(*b)) {
                    self.pos += 1;
                }
                // Identifier bytes are ASCII by construction.
                let text = std::str::from_utf8(&self.bytes[start..self.pos]).unwrap_or("");
                // Keyword members such as x.type need Symbols in the AST;
                // the parser receives only an immutable interner snapshot.
                let symbol = self.interner.intern(text);
                keyword(text).map_or(TokenKind::Ident(symbol), TokenKind::Keyword)
            }
            _ => {
                if let Some((len, punct)) = punctuation(&self.bytes[self.pos..]) {
                    self.pos += len;
                    TokenKind::Punct(punct)
                } else {
                    let (ch, len) = scalar(&self.bytes[self.pos..]);
                    self.pos += len;
                    let rule = if ch.is_none()
                        || ch.is_some_and(|c| {
                            bidi(c) || c == '\r' || (start == 0 && c == '\u{feff}')
                        }) {
                        const { RuleId::of("SEM-3") }
                    } else {
                        const { RuleId::of("LANG-1") }
                    };
                    self.error(rule, start, self.pos, "invalid source character");
                    TokenKind::Invalid
                }
            }
        };
        let kind = if self.output.diagnostics.len() > errors {
            TokenKind::Invalid
        } else {
            kind
        };
        self.emit(start, kind);
    }
    // Validate a source scalar while traversing a comment, including bare CR.
    fn comment_scalar(&mut self) {
        let start = self.pos;
        let (ch, len) = scalar(&self.bytes[start..]);
        self.pos += len;
        if ch.is_none()
            || ch
                .is_some_and(|c| bidi(c) || (c == '\r' && self.bytes.get(self.pos) != Some(&b'\n')))
        {
            self.error(
                const { RuleId::of("SEM-3") },
                start,
                self.pos,
                "forbidden character in comment",
            );
        }
    }
    fn line_comment(&mut self) -> TokenKind {
        let doc = self.starts(b"///");
        self.pos += 2;
        while self.pos < self.bytes.len() && !matches!(self.bytes[self.pos], b'\r' | b'\n') {
            self.comment_scalar();
        }
        TokenKind::Trivia(if doc {
            TriviaKind::DocLine
        } else {
            TriviaKind::LineComment
        })
    }
    fn block_comment(&mut self) -> TokenKind {
        let start = self.pos;
        let doc = self.starts(b"/**");
        // /**/ overlaps the doc prefix's star with the closing delimiter (#21).
        self.pos += 2;
        let mut depth = 1usize;
        while self.pos < self.bytes.len() {
            if self.starts(b"/*") {
                depth += 1;
                self.pos += 2;
            } else if self.starts(b"*/") {
                depth -= 1;
                self.pos += 2;
                if depth == 0 {
                    break;
                }
            } else {
                self.comment_scalar();
            }
        }
        if depth != 0 {
            self.error(
                const { RuleId::of("LANG-1") },
                start,
                self.pos,
                "unterminated block comment",
            );
        }
        TokenKind::Trivia(if doc {
            TriviaKind::DocBlock
        } else {
            TriviaKind::BlockComment
        })
    }
    // #52: exponent syntax is lexical; finite-float overflow remains the
    // body checker's existing numeric policy pending the SEM-2 ruling.
    fn number(&mut self) -> TokenKind {
        let start = self.pos;
        let base = if self.starts(b"0x") {
            16
        } else if self.starts(b"0o") {
            8
        } else if self.starts(b"0b") {
            2
        } else {
            10
        };
        if base != 10 {
            self.pos += 2;
        }
        let mut valid = self.digits(base);
        let mut float = false;
        if base == 10 {
            if self.bytes.get(self.pos) == Some(&b'.')
                && self.bytes.get(self.pos + 1).is_some_and(u8::is_ascii_digit)
            {
                float = true;
                self.pos += 1;
                valid &= self.digits(10);
            }
            if matches!(self.bytes.get(self.pos), Some(b'e' | b'E')) {
                float = true;
                self.pos += 1;
                if matches!(self.bytes.get(self.pos), Some(b'+' | b'-')) {
                    self.pos += 1;
                }
                valid &= self.digits(10);
            }
        }
        let suffix_start = self.pos;
        while self.bytes.get(self.pos).is_some_and(|b| name_byte(*b)) {
            self.pos += 1;
        }
        let suffix = &self.bytes[suffix_start..self.pos];
        if base == 10 && suffix.starts_with(b"f") {
            float = true;
        }
        valid &= if float {
            matches!(suffix, b"" | b"f32" | b"f64")
        } else {
            matches!(
                suffix,
                b"" | b"u8"
                    | b"u16"
                    | b"u32"
                    | b"u64"
                    | b"u128"
                    | b"i8"
                    | b"i16"
                    | b"i32"
                    | b"i64"
                    | b"i128"
            )
        };
        if !valid {
            self.error(
                if float {
                    const { RuleId::of("SEM-2") }
                } else {
                    const { RuleId::of("SEM-1") }
                },
                start,
                self.pos,
                "malformed numeric literal or suffix",
            );
        }
        if float {
            TokenKind::Float
        } else {
            TokenKind::Integer
        }
    }
    fn digits(&mut self, base: u32) -> bool {
        let start = self.pos;
        let mut previous_digit = false;
        let mut valid = true;
        while let Some(&byte) = self.bytes.get(self.pos) {
            if digit_value(byte).is_some_and(|d| d < base) {
                previous_digit = true;
            } else if byte == b'_' {
                valid &= previous_digit;
                previous_digit = false;
            } else {
                break;
            }
            self.pos += 1;
        }
        valid && previous_digit && self.pos > start
    }
    fn string(&mut self, bytes: bool) -> TokenKind {
        let start = self.pos;
        self.pos += if bytes { 2 } else { 1 };
        let mut closed = false;
        while self.pos < self.bytes.len() {
            match self.bytes[self.pos] {
                b'"' => {
                    self.pos += 1;
                    closed = true;
                    break;
                }
                // Leave the newline to the tape and resume on the next line.
                b'\r' | b'\n' => break,
                b'\\' => {
                    self.escape(bytes);
                    // A forbidden continuation is one invalid literal. Recover
                    // through its closing quote while retaining the physical
                    // newline bytes in that token's lossless span.
                    if self.bytes.get(self.pos) == Some(&b'\r') {
                        self.pos += 1;
                        if self.bytes.get(self.pos) == Some(&b'\n') {
                            self.pos += 1;
                        }
                    } else if self.bytes.get(self.pos) == Some(&b'\n') {
                        self.pos += 1;
                    }
                }
                _ => {
                    let at = self.pos;
                    let (ch, len) = scalar(&self.bytes[at..]);
                    self.pos += len;
                    if ch.is_none()
                        || ch.is_some_and(|c| {
                            c <= '\u{1f}' || c == '\u{7f}' || bidi(c) || (bytes && !c.is_ascii())
                        })
                    {
                        self.error(
                            const { RuleId::of("SEM-3") },
                            at,
                            self.pos,
                            "forbidden literal character",
                        );
                    }
                }
            }
        }
        if !closed {
            self.error(
                const { RuleId::of("SEM-3") },
                start,
                self.pos,
                "unterminated string or bytes literal",
            );
        }
        if bytes {
            TokenKind::Bytes
        } else {
            TokenKind::String
        }
    }
    fn escape(&mut self, bytes: bool) {
        let start = self.pos;
        self.pos += 1;
        let Some(&byte) = self.bytes.get(self.pos) else {
            self.error(
                const { RuleId::of("SEM-3") },
                start,
                self.pos,
                "incomplete escape",
            );
            return;
        };
        // Do not swallow a physical newline into a malformed escape.
        if matches!(byte, b'\r' | b'\n') {
            self.error(
                const { RuleId::of("SEM-3") },
                start,
                self.pos,
                "escape line continuation is forbidden",
            );
            return;
        }
        match byte {
            b'\\' | b'"' | b'n' | b'r' | b't' | b'0' => {
                self.pos += 1;
            }
            b'x' => {
                self.pos += 1;
                let mut value = 0;
                let mut count = 0;
                while count < 2 {
                    let Some(digit) = self.bytes.get(self.pos).and_then(|b| digit_value(*b)) else {
                        break;
                    };
                    value = value * 16 + digit;
                    count += 1;
                    self.pos += 1;
                }
                if count != 2 || (!bytes && value > 0x7f) {
                    self.error(
                        const { RuleId::of("SEM-3") },
                        start,
                        self.pos,
                        "invalid hexadecimal escape",
                    );
                }
            }
            b'u' => {
                self.pos += 1;
                let mut valid = !bytes;
                if self.bytes.get(self.pos) == Some(&b'{') {
                    self.pos += 1;
                    let mut count = 0usize;
                    let mut value = Some(0u32);
                    while let Some(digit) = self.bytes.get(self.pos).and_then(|b| digit_value(*b)) {
                        count += 1;
                        value = value.and_then(|v| v.checked_mul(16)?.checked_add(digit));
                        self.pos += 1;
                    }
                    valid &= (1..=6).contains(&count) && value.and_then(char::from_u32).is_some();
                    if self.bytes.get(self.pos) == Some(&b'}') {
                        self.pos += 1;
                    } else {
                        valid = false;
                    }
                } else {
                    valid = false;
                }
                if !valid {
                    self.error(
                        const { RuleId::of("SEM-3") },
                        start,
                        self.pos,
                        "invalid Unicode escape",
                    );
                }
            }
            _ => {
                let (_, len) = scalar(&self.bytes[self.pos..]);
                self.pos += len;
                self.error(
                    const { RuleId::of("SEM-3") },
                    start,
                    self.pos,
                    "unknown escape",
                );
            }
        }
    }
}
fn letter(byte: u8) -> bool {
    byte.is_ascii_alphabetic() || byte == b'_'
}
fn name_byte(byte: u8) -> bool {
    letter(byte) || byte.is_ascii_digit()
}
fn digit_value(byte: u8) -> Option<u32> {
    match byte {
        b'0'..=b'9' => Some((byte - b'0') as u32),
        b'a'..=b'f' => Some((byte - b'a' + 10) as u32),
        b'A'..=b'F' => Some((byte - b'A' + 10) as u32),
        _ => None,
    }
}
// Decode at most four bytes, so malformed byte streams still take linear time.
fn scalar(bytes: &[u8]) -> (Option<char>, usize) {
    let Some(&first) = bytes.first() else {
        return (None, 0);
    };
    let len = match first {
        0..=0x7f => 1,
        0xc2..=0xdf => 2,
        0xe0..=0xef => 3,
        0xf0..=0xf4 => 4,
        _ => return (None, 1),
    };
    let ch = bytes
        .get(..len)
        .and_then(|b| std::str::from_utf8(b).ok())
        .and_then(|s| s.chars().next());
    if ch.is_some() { (ch, len) } else { (None, 1) }
}
fn bidi(ch: char) -> bool {
    matches!(ch, '\u{061c}' | '\u{200e}'..='\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
}
fn keyword(text: &str) -> Option<Keyword> {
    Some(match text {
        "fn" => Keyword::Fn,
        "let" => Keyword::Let,
        "mut" => Keyword::Mut,
        "match" => Keyword::Match,
        "if" => Keyword::If,
        "else" => Keyword::Else,
        "for" => Keyword::For,
        "in" => Keyword::In,
        "while" => Keyword::While,
        "break" => Keyword::Break,
        "continue" => Keyword::Continue,
        "return" => Keyword::Return,
        "take" => Keyword::Take,
        "copy" => Keyword::Copy,
        "unique" => Keyword::Unique,
        "type" => Keyword::Type,
        "distinct" => Keyword::Distinct,
        "flat" => Keyword::Flat,
        "resource" => Keyword::Resource,
        "drop" => Keyword::Drop,
        "box" => Keyword::Box,
        "pub" => Keyword::Pub,
        "opaque" => Keyword::Opaque,
        "const" => Keyword::Const,
        "comptime" => Keyword::Comptime,
        "shape" => Keyword::Shape,
        "heap" => Keyword::Heap,
        "before" => Keyword::Before,
        "requires" => Keyword::Requires,
        "ensures" => Keyword::Ensures,
        "result" => Keyword::Result,
        "where" => Keyword::Where,
        "uses" => Keyword::Uses,
        "of" => Keyword::Of,
        "local" => Keyword::Local,
        "global" => Keyword::Global,
        "once" => Keyword::Once,
        "todo" => Keyword::Todo,
        "extern" => Keyword::Extern,
        "unsafe" => Keyword::Unsafe,
        "true" => Keyword::True,
        "false" => Keyword::False,
        _ => return None,
    })
}
fn punctuation(bytes: &[u8]) -> Option<(usize, Punct)> {
    // Fixed-size table, longest first: bounded work per source position (#21).
    const TABLE: &[(&[u8], Punct)] = &[
        (b"..=", Punct::DotDotEq),
        (b"<<=", Punct::ShlEq),
        (b">>=", Punct::ShrEq),
        (b"..", Punct::DotDot),
        (b"->", Punct::Arrow),
        (b"=>", Punct::FatArrow),
        (b"&&", Punct::AndAnd),
        (b"||", Punct::OrOr),
        (b"<<", Punct::Shl),
        (b">>", Punct::Shr),
        (b"==", Punct::EqEq),
        (b"!=", Punct::Ne),
        (b"<=", Punct::Le),
        (b">=", Punct::Ge),
        (b"+=", Punct::PlusEq),
        (b"-=", Punct::MinusEq),
        (b"*=", Punct::StarEq),
        (b"/=", Punct::SlashEq),
        (b"%=", Punct::PercentEq),
        (b"&=", Punct::AmpEq),
        (b"|=", Punct::PipeEq),
        (b"^=", Punct::CaretEq),
        (b"(", Punct::LParen),
        (b")", Punct::RParen),
        (b"[", Punct::LBracket),
        (b"]", Punct::RBracket),
        (b"{", Punct::LBrace),
        (b"}", Punct::RBrace),
        (b",", Punct::Comma),
        (b":", Punct::Colon),
        (b";", Punct::Semicolon),
        (b".", Punct::Dot),
        (b"@", Punct::At),
        (b"?", Punct::Question),
        (b"+", Punct::Plus),
        (b"-", Punct::Minus),
        (b"*", Punct::Star),
        (b"/", Punct::Slash),
        (b"%", Punct::Percent),
        (b"!", Punct::Bang),
        (b"~", Punct::Tilde),
        (b"&", Punct::Amp),
        (b"|", Punct::Pipe),
        (b"^", Punct::Caret),
        (b"=", Punct::Eq),
        (b"<", Punct::Lt),
        (b">", Punct::Gt),
    ];
    TABLE
        .iter()
        .find(|(text, _)| bytes.starts_with(text))
        .map(|(text, kind)| (text.len(), *kind))
}

#[cfg(test)]
mod tests;

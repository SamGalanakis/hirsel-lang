//! Recover declaration sites from the lossless tape without adding AST fields.
use crate::*;
impl SyntaxTree {
    fn origin_tokens(&self, origin: Origin) -> impl DoubleEndedIterator<Item = (usize, &Token)> {
        self.tokens
            .get(origin.tokens.start as usize..origin.tokens.end as usize)
            .unwrap_or(&[])
            .iter()
            .enumerate()
            .map(move |(i, token)| (i + origin.tokens.start as usize, token))
            .filter(|(_, t)| !matches!(t.kind, TokenKind::Trivia(_) | TokenKind::Eof))
    }
    fn parameter_token(&self, origin: Origin, parameter: &Parameter) -> Option<(usize, &Token)> {
        let anchor = self.types.get(parameter.ty.0 as usize)?.origin.tokens.start as usize;
        let name = parameter.name?;
        self.origin_tokens(origin)
            .rev()
            .find(|(i, t)| *i < anchor && t.kind == TokenKind::Ident(name))
    }
    /// Exact declared parameter name; None for anonymous slots or absent tape.
    pub fn parameter_name_span(&self, origin: Origin, parameter: &Parameter) -> Option<Span> {
        self.parameter_token(origin, parameter).map(|(_, t)| t.span)
    }
    /// Exact written label, distinct from the binding name and nested type labels.
    pub fn parameter_label_span(&self, origin: Origin, parameter: &Parameter) -> Option<Span> {
        let (name, _) = self.parameter_token(origin, parameter)?;
        let label = parameter.label?;
        self.origin_tokens(origin)
            .rev()
            .find(|(i, t)| *i < name && t.kind == TokenKind::Ident(label))
            .map(|(_, t)| t.span)
    }
    /// Generic declaration names in complete binder order (including size slots).
    pub fn generic_name_spans(&self, origin: Origin) -> Vec<Span> {
        let mut tokens = self.origin_tokens(origin).map(|(_, t)| t);
        if !tokens.any(|t| matches!(t.kind, TokenKind::Keyword(Keyword::Fn | Keyword::Type))) {
            return vec![];
        }
        for token in tokens.by_ref() {
            match token.kind {
                TokenKind::Punct(Punct::LBracket) => break,
                TokenKind::Punct(Punct::LParen | Punct::LBrace | Punct::Eq) => return vec![],
                _ => {}
            }
        }
        let mut spans = vec![];
        let mut depth = 1usize;
        let mut name = true;
        for token in tokens {
            if depth == 1 {
                match token.kind {
                    TokenKind::Ident(_) if name => {
                        spans.push(token.span);
                        name = false;
                    }
                    TokenKind::Punct(Punct::Comma) => name = true,
                    TokenKind::Punct(Punct::RBracket) => break,
                    _ => {}
                }
            }
            delimiter_depth(&token.kind, &mut depth);
        }
        spans
    }
    /// Outer uses paths, excluding clauses inside parameter/result types or bodies.
    /// A closure's trailing uses clause belongs to the same outer header.
    pub fn uses_path_spans(&self, origin: Origin) -> Vec<Span> {
        let mut tokens = self.origin_tokens(origin).map(|(_, t)| t).peekable();
        let mut depth = 0usize;
        while let Some(token) = tokens.next() {
            if depth == 0
                && token.kind == TokenKind::Keyword(Keyword::Uses)
                && tokens
                    .peek()
                    .is_some_and(|t| t.kind == TokenKind::Punct(Punct::LBrace))
            {
                tokens.next();
                let mut spans = vec![];
                let mut path: Option<Span> = None;
                for token in tokens {
                    match token.kind {
                        TokenKind::Punct(Punct::Comma | Punct::RBrace) => {
                            if let Some(span) = path.take() {
                                spans.push(span);
                            }
                            if token.kind == TokenKind::Punct(Punct::RBrace) {
                                break;
                            }
                        }
                        _ => {
                            if let Some(span) = &mut path {
                                span.end = token.span.end;
                            } else {
                                path = Some(token.span);
                            }
                        }
                    }
                }
                return spans;
            }
            delimiter_depth(&token.kind, &mut depth);
        }
        vec![]
    }
    /// Top-level variant names in declaration order, excluding payload types.
    pub fn variant_name_spans(&self, origin: Origin) -> Vec<Span> {
        let mut spans = vec![];
        let mut depth = 0usize;
        let mut body = false;
        let mut name = false;
        for (_, token) in self.origin_tokens(origin) {
            if depth == 0 {
                match token.kind {
                    TokenKind::Punct(Punct::Eq) => {
                        body = true;
                        name = true;
                    }
                    TokenKind::Punct(Punct::Pipe) if body => name = true,
                    TokenKind::Ident(_) if name => {
                        spans.push(token.span);
                        name = false;
                    }
                    _ => {}
                }
            }
            delimiter_depth(&token.kind, &mut depth);
        }
        spans
    }
}
fn delimiter_depth(kind: &TokenKind, depth: &mut usize) {
    match kind {
        TokenKind::Punct(Punct::LParen | Punct::LBracket | Punct::LBrace) => *depth += 1,
        TokenKind::Punct(Punct::RParen | Punct::RBracket | Punct::RBrace) => {
            *depth = depth.saturating_sub(1)
        }
        _ => {}
    }
}

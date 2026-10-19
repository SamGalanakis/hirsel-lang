//! Align generated syntax tokens with the tape and retain every comment in order.
use crate::doc::PrintedToken;
use hirsel_span::{FileSpan, SourceMap};
use hirsel_syntax::*;

/// The file's whole tape in its own offsets, and where each item's tokens start.
pub(crate) struct Tape<'a> {
    tokens: Vec<(&'a TokenKind, FileSpan)>,
    bases: Vec<usize>,
}
impl<'a> Tape<'a> {
    pub(crate) fn new(file: &'a SyntaxFile, sources: &SourceMap) -> Self {
        let mut tokens = vec![];
        let mut bases = vec![];
        for item in &file.items {
            bases.push(tokens.len());
            tokens.extend(
                item.tree
                    .tokens
                    .iter()
                    .map(|t| (&t.kind, sources.resolve(t.span))),
            );
        }
        tokens.extend(
            file.tokens
                .iter()
                .map(|t| (&t.kind, sources.resolve(t.span))),
        );
        Self { tokens, bases }
    }
}

pub(crate) fn valid_tape(file: &SyntaxFile, tape: &Tape<'_>, sources: &SourceMap) -> bool {
    let Some(source) = sources.file(file.file) else {
        return false;
    };
    let mut offset = 0;
    let mut delimiters = Vec::new();
    for (i, (kind, span)) in tape.tokens.iter().enumerate() {
        if span.file != file.file || span.start != offset || sources.file_slice(*span).is_none() {
            return false;
        }
        if matches!(kind, TokenKind::Eof) {
            if span.start != span.end || i + 1 != tape.tokens.len() {
                return false;
            }
        } else if span.start == span.end {
            return false;
        }
        offset = span.end;
        if let TokenKind::Punct(p) = kind {
            match p {
                Punct::LParen => delimiters.push(Punct::RParen),
                Punct::LBracket => delimiters.push(Punct::RBracket),
                Punct::LBrace => delimiters.push(Punct::RBrace),
                Punct::RParen | Punct::RBracket | Punct::RBrace if delimiters.pop() != Some(*p) => {
                    return false;
                }
                _ => {}
            }
        }
    }
    offset as usize == source.text().len()
        && delimiters.is_empty()
        && matches!(tape.tokens.last().map(|t| t.0), Some(TokenKind::Eof))
}
fn optional(s: &str) -> bool {
    matches!(s, "(" | ")" | "," | ";")
}
struct Comment<'a> {
    text: &'a str,
    trailing: bool,
    line: bool,
    header_gap: bool,
}
pub(crate) fn merge(
    file: &SyntaxFile,
    tape: &Tape<'_>,
    sources: &SourceMap,
    printed: Vec<PrintedToken>,
) -> Option<String> {
    let mut original: Vec<_> = tape
        .tokens
        .iter()
        .enumerate()
        .filter(|(_, (kind, _))| !matches!(kind, TokenKind::Trivia(_) | TokenKind::Eof))
        .map(|(index, (kind, span))| (index, *kind, *span))
        .collect();
    // Recovery may omit entire declarations without producing an Error node.
    // An item's leading analysis annotations precede its span (PS-19).
    for &(index, kind, _) in &original {
        if !matches!(kind, TokenKind::Punct(Punct::Semicolon))
            && !file.items.iter().zip(&tape.bases).any(|(item, &base)| {
                let within = |origin: Origin| {
                    base + origin.tokens.start as usize <= index
                        && index < base + origin.tokens.end as usize
                };
                within(item.item.origin) || item.tree.analyses.iter().any(|a| within(a.origin))
            })
        {
            return None;
        }
    }
    // Prefix authority is canonical. A trailing authority clause may move only
    // when the affected body/clause contains no comments; otherwise retain its
    // source slot so documentation attachment and comment order both survive.
    for (item, &base) in file.items.iter().zip(&tape.bases) {
        for e in &item.tree.expressions {
            if let ExprKind::Closure { body, .. } = e.kind {
                if let Some((start, end, false)) = trailing_uses(&item.tree, e.origin, body) {
                    let (start, end) = (base + start, base + end);
                    let body_start = base
                        + item
                            .tree
                            .expressions
                            .get(body.0 as usize)?
                            .origin
                            .tokens
                            .start as usize;
                    let moved: Vec<_> = original
                        .iter()
                        .copied()
                        .filter(|(i, _, _)| start <= *i && *i < end)
                        .collect();
                    original.retain(|(i, _, _)| !(start <= *i && *i < end));
                    let insertion = original.iter().position(|(i, _, _)| *i == body_start)?;
                    original.splice(insertion..insertion, moved);
                }
            }
        }
    }
    let mut anchors = vec![None; tape.tokens.len()];
    let (mut i, mut j) = (0, 0);
    while i < original.len() && j < printed.len() {
        let s = sources.file_slice(original[i].2)?;
        if equivalent(s, &printed[j].text) {
            anchors[original[i].0] = Some(j);
            i += 1;
            j += 1;
        } else if optional(s) {
            i += 1;
        } else if optional(&printed[j].text) {
            j += 1;
        } else {
            return None;
        }
    }
    if original[i..]
        .iter()
        .any(|(_, _, span)| !optional(sources.file_slice(*span).unwrap_or("")))
        || printed[j..].iter().any(|t| !optional(&t.text))
    {
        return None;
    }
    let mut comments: Vec<Vec<Comment<'_>>> = (0..=printed.len()).map(|_| Vec::new()).collect();
    let mut previous_significant = None;
    let mut previous_comment = None;
    let mut last_boundary = 0;
    let mut header_end = None;
    let mut header_gap_after = false;
    for (index, &(kind, span)) in tape.tokens.iter().enumerate() {
        match kind {
            TokenKind::Trivia(
                k @ (TriviaKind::LineComment
                | TriviaKind::DocLine
                | TriviaKind::BlockComment
                | TriviaKind::DocBlock),
            ) => {
                let prev = previous_comment.or(previous_significant);
                let trailing = prev.is_some_and(|end| {
                    sources
                        .file(file.file)
                        .unwrap()
                        .text()
                        .get(end as usize..span.start as usize)
                        .is_some_and(|s| !s.contains('\n'))
                });
                let mut boundary = anchors[index + 1..]
                    .iter()
                    .flatten()
                    .next()
                    .copied()
                    .unwrap_or(printed.len());
                boundary = boundary.max(last_boundary);
                last_boundary = boundary;
                comments[boundary].push(Comment {
                    text: sources.file_slice(span)?,
                    trailing,
                    header_gap: previous_significant.is_none()
                        && header_end.is_some_and(|end: u32| {
                            sources.file(file.file).unwrap().text()
                                [end as usize..span.start as usize]
                                .bytes()
                                .filter(|b| *b == b'\n')
                                .count()
                                >= 2
                        }),
                    line: matches!(*k, TriviaKind::LineComment | TriviaKind::DocLine),
                });
                previous_comment = Some(span.end);
                header_end = if previous_significant.is_none() && *k == TriviaKind::LineComment {
                    Some(span.end)
                } else {
                    None
                };
            }
            TokenKind::Trivia(_) => {}
            TokenKind::Eof => {}
            _ => {
                if previous_significant.is_none() {
                    header_gap_after = header_end.is_some_and(|end| {
                        sources.file(file.file).unwrap().text()[end as usize..span.start as usize]
                            .bytes()
                            .filter(|b| *b == b'\n')
                            .count()
                            >= 2
                    });
                }
                previous_significant = Some(span.end);
                previous_comment = None;
            }
        }
    }
    let mut out = String::new();
    for (i, t) in printed.iter().enumerate() {
        let indent = t.before.rsplit('\n').next().unwrap_or("");
        let indent = if t.before.contains('\n') {
            indent.len()
        } else {
            current_indent(&out)
        };
        let mut gap = t.before.clone();
        for c in &comments[i] {
            if c.header_gap {
                out.push('\n');
            }
            if c.trailing && !out.ends_with('\n') && !out.is_empty() {
                trim_spaces(&mut out);
                out.push(' ');
                out.push_str(c.text);
                if c.line || c.text.contains('\n') {
                    newline(&mut out, indent);
                }
            } else {
                append_gap(&mut out, &gap);
                gap.clear();
                if !out.is_empty() && !out.ends_with(' ') && !out.ends_with('\n') {
                    out.push(' ');
                }
                out.push_str(c.text);
                // Leading standalone comments keep a line of their own. Inline
                // block comments keep the expression's original continuation.
                if c.line || !c.trailing {
                    newline(&mut out, indent);
                } else {
                    out.push(' ');
                }
            }
        }
        if i == 0 && header_gap_after {
            out.push('\n');
        }
        append_gap(&mut out, &gap);
        out.push_str(&t.text);
    }
    for c in &comments[printed.len()] {
        if c.trailing && !out.is_empty() && !out.ends_with('\n') {
            trim_spaces(&mut out);
            out.push(' ');
        } else if !out.is_empty() {
            newline(&mut out, 0);
        }
        out.push_str(c.text);
        if c.line {
            newline(&mut out, 0);
        }
    }
    trim_spaces(&mut out);
    while out.ends_with('\n') {
        out.pop();
    }
    if !out.is_empty() {
        out.push('\n');
    }
    Some(out)
}
fn current_indent(s: &str) -> usize {
    s.rsplit('\n')
        .next()
        .unwrap_or("")
        .bytes()
        .take_while(|b| *b == b' ')
        .count()
}
fn trim_spaces(s: &mut String) {
    while s.ends_with(' ') {
        s.pop();
    }
}
fn newline(s: &mut String, indent: usize) {
    trim_spaces(s);
    if !s.ends_with('\n') {
        s.push('\n');
    }
    s.push_str(&" ".repeat(indent));
}
fn append_gap(out: &mut String, gap: &str) {
    if gap.contains('\n') && out.trim_end_matches(' ').ends_with('\n') {
        trim_spaces(out);
        out.push_str(gap.strip_prefix('\n').unwrap_or(gap));
    } else if out.ends_with(' ') && gap.starts_with(' ') {
        out.push_str(gap.trim_start_matches(' '));
    } else {
        out.push_str(gap);
    }
}

/// Returns the trailing authority range and whether trivia prevents moving it.
pub(crate) fn trailing_uses(
    tree: &SyntaxTree,
    origin: Origin,
    body: ExprId,
) -> Option<(usize, usize, bool)> {
    let b = tree.expressions.get(body.0 as usize)?.origin.tokens;
    let end = origin.tokens.end as usize;
    let start = (b.end as usize..end).find(|i| {
        tree.tokens
            .get(*i)
            .is_some_and(|t| !matches!(t.kind, TokenKind::Trivia(_)))
    })?;
    if !matches!(
        tree.tokens.get(start)?.kind,
        TokenKind::Keyword(Keyword::Uses)
    ) {
        return None;
    }
    let preserve = tree.tokens.get(b.start as usize..end)?.iter().any(|t| {
        matches!(
            t.kind,
            TokenKind::Trivia(
                TriviaKind::LineComment
                    | TriviaKind::DocLine
                    | TriviaKind::BlockComment
                    | TriviaKind::DocBlock
            )
        )
    });
    Some((start, end, preserve))
}
fn equivalent(a: &str, b: &str) -> bool {
    if a == b || (a == "/" && b == ".") {
        return true;
    }
    // TypeArgument::SizeLiteral stores a value rather than its source spelling.
    fn size(s: &str) -> Option<u64> {
        let s = s.replace('_', "");
        let (digits, radix) = if let Some(s) = s.strip_prefix("0x") {
            (s, 16)
        } else if let Some(s) = s.strip_prefix("0o") {
            (s, 8)
        } else if let Some(s) = s.strip_prefix("0b") {
            (s, 2)
        } else {
            (s.as_str(), 10)
        };
        u64::from_str_radix(digits, radix).ok()
    }
    a.as_bytes().first().is_some_and(u8::is_ascii_digit)
        && b.as_bytes().first().is_some_and(u8::is_ascii_digit)
        && size(a).is_some()
        && size(a) == size(b)
}

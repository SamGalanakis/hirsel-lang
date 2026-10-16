//! A small grouped document renderer. Only delimited lists use soft breaks.
#[derive(Clone)]
pub(crate) enum Doc {
    Token(String),
    Space,
    Line,
    Soft,
    EmptySoft,
    Concat(Vec<Doc>),
    Nest(Box<Doc>),
    Group(Box<Doc>),
}
use Doc::*;
pub(crate) fn token(text: impl Into<String>) -> Doc {
    Token(text.into())
}
pub(crate) fn cat(parts: impl IntoIterator<Item = Doc>) -> Doc {
    Concat(parts.into_iter().collect())
}
pub(crate) fn join(parts: impl IntoIterator<Item = Doc>, separator: Doc) -> Doc {
    let mut result = Vec::new();
    for part in parts {
        if !result.is_empty() {
            result.push(separator.clone());
        }
        result.push(part);
    }
    cat(result)
}
pub(crate) fn nest(doc: Doc) -> Doc {
    Nest(Box::new(doc))
}
pub(crate) fn group(doc: Doc) -> Doc {
    Group(Box::new(doc))
}
pub(crate) fn list(open: &str, parts: Vec<Doc>, close: &str, singleton_comma: bool) -> Doc {
    if parts.is_empty() {
        return cat([token(open), token(close)]);
    }
    let single = parts.len() == 1 && singleton_comma;
    group(cat([
        token(open),
        nest(cat([
            EmptySoft,
            join(parts, cat([token(","), Soft])),
            if single { token(",") } else { cat([]) },
        ])),
        EmptySoft,
        token(close),
    ]))
}
fn flat_width(doc: &Doc) -> usize {
    match doc {
        Token(s) => s.chars().count(),
        Space | Soft => 1,
        EmptySoft => 0,
        Line => usize::MAX / 2,
        Concat(ds) => ds
            .iter()
            .fold(0usize, |a, d| a.saturating_add(flat_width(d))),
        Nest(d) | Group(d) => flat_width(d),
    }
}
#[derive(Debug)]
pub(crate) struct PrintedToken {
    pub text: String,
    pub before: String,
}
pub(crate) fn render(doc: &Doc) -> Vec<PrintedToken> {
    fn walk(
        doc: &Doc,
        indent: usize,
        flat: bool,
        column: &mut usize,
        gap: &mut String,
        out: &mut Vec<PrintedToken>,
    ) {
        match doc {
            Token(s) => {
                out.push(PrintedToken {
                    text: s.clone(),
                    before: std::mem::take(gap),
                });
                *column += s.chars().count();
            }
            Space => {
                gap.push(' ');
                *column += 1;
            }
            Soft if flat => {
                gap.push(' ');
                *column += 1;
            }
            EmptySoft if flat => {}
            Line | Soft | EmptySoft => {
                while gap.ends_with(' ') {
                    gap.pop();
                }
                gap.push('\n');
                gap.push_str(&" ".repeat(indent));
                *column = indent;
            }
            Concat(ds) => {
                for d in ds {
                    walk(d, indent, flat, column, gap, out);
                }
            }
            Nest(d) => walk(d, indent + 2, flat, column, gap, out),
            Group(d) => walk(
                d,
                indent,
                flat || flat_width(d) <= 88usize.saturating_sub(*column),
                column,
                gap,
                out,
            ),
        }
    }
    let mut out = Vec::new();
    walk(doc, 0, false, &mut 0, &mut String::new(), &mut out);
    out
}

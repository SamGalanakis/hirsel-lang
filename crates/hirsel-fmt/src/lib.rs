//! Canonical Hirsel formatting (LANG-1/11/12). See the crate README for layout.
//! Names come from the immutable symbol snapshot; no parser or lexer is used here.

#[cfg(test)]
hirsel_guard::install!();

mod doc;
mod printer;
mod trivia;
use hirsel_span::SourceMap;
use hirsel_syntax::*;

/// A conservative failure leaves the entire source snapshot byte-for-byte intact.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FormatResult {
    pub source: String,
    /// True for syntax errors, incomplete tapes/trees, or unavailable spellings.
    pub unchanged: bool,
}

/// Format an indexed tree and its lossless tape. A missing source ID is an error.
/// #56: retain canonical, tree-equivalent formatting pending the byte-fidelity ruling.
/// SyntaxFile carries recovery error status even when no Error node exists.
pub fn format(file: &SyntaxFile, sources: &SourceMap) -> FormatResult {
    let original = sources
        .file(file.file)
        .expect("SyntaxFile must belong to SourceMap")
        .text();
    let unchanged = || FormatResult {
        source: original.to_owned(),
        unchanged: true,
    };
    let tape = trivia::Tape::new(file, sources);
    if file.has_syntax_errors || damaged(file) || !trivia::valid_tape(file, &tape, sources) {
        return unchanged();
    }
    let Some(doc) = printer::file(file) else {
        return unchanged();
    };
    let Some(source) = trivia::merge(file, &tape, sources, doc::render(&doc)) else {
        return unchanged();
    };
    FormatResult {
        source,
        unchanged: false,
    }
}

/// Canonical parsed item content for semantic revision keys. Reuse the AST
/// printer, excluding analysis annotations, trivia-protecting parentheses and
/// layout. Hard lines encode parsed statement boundaries; soft lines are only
/// layout. Literal spellings, names, contracts and finalizers remain content.
///
/// Like rustc's span-disabled stable AST hashing (rustc a7c37f9f,
/// compiler/rustc_middle/src/ich.rs:50-55,106-108), source positions are absent.
/// Hirsel also normalizes explicit/implicit END through the parsed structure.
pub fn semantic_item(syntax: &ItemSyntax) -> Option<Vec<Vec<u8>>> {
    fn fields(doc: &doc::Doc, out: &mut Vec<Vec<u8>>) {
        use doc::Doc;
        match doc {
            Doc::Token(text) => {
                let mut field = vec![1];
                field.extend_from_slice(text.as_bytes());
                out.push(field);
            }
            Doc::Line => out.push(vec![0]),
            Doc::Concat(parts) => {
                for part in parts {
                    fields(part, out);
                }
            }
            Doc::Nest(inner) | Doc::Group(inner) => fields(inner, out),
            Doc::Space | Doc::Soft | Doc::EmptySoft => {}
        }
    }
    let mut printer = printer::Printer::new(syntax);
    printer.semantic();
    let mut out = vec![b"hirsel.semantic-source.v2".to_vec()];
    fields(&printer.item()?, &mut out);
    Some(out)
}

fn damaged(file: &SyntaxFile) -> bool {
    file.tape().any(|t| matches!(t.kind, TokenKind::Invalid))
        || file.items.iter().any(|item| {
            let tree = &item.tree;
            tree.expressions
                .iter()
                .any(|e| matches!(e.kind, ExprKind::Error))
                || tree
                    .patterns
                    .iter()
                    .any(|p| matches!(p.kind, PatternKind::Error))
                || tree
                    .types
                    .iter()
                    .any(|t| matches!(t.kind, TypeExprKind::Error))
        })
}

#[cfg(test)]
mod tests;

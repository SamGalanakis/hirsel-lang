use super::*;
use hirsel_intern::Symbol;
use hirsel_span::{Anchor, Span};
use std::sync::Arc;

/// Test-only token fixtures: the trees themselves are built by hand, never parsed.
/// Every item sees the arenas built so far, so items may share nodes; the first
/// item owns the whole tape but its end, at the file's own anchor.
struct Fixture {
    file: SyntaxFile,
    /// The arenas built so far; each item takes a copy.
    tree: SyntaxTree,
    /// The whole tape, EOF included.
    tokens: Vec<Token>,
    anchor: Anchor,
    sources: SourceMap,
}
impl Fixture {
    fn new(source: &str) -> Self {
        let mut sources = SourceMap::default();
        let file = sources.add("test.hir", source.into()).unwrap();
        let anchor = sources.file_anchor(file);
        let tokens = tape(source, anchor);
        Self {
            file: SyntaxFile {
                has_syntax_errors: false,
                file,
                items: vec![],
                tokens: tokens.clone(),
            },
            tree: SyntaxTree::default(),
            tokens,
            anchor,
            sources,
        }
    }
    fn s(&mut self, s: &str) -> Symbol {
        Symbol::intern(s)
    }
    fn path(&mut self, s: &str) -> Path {
        let (package, s) = match s.split_once('@') {
            Some((package, rest)) => {
                let (version, path) = rest.split_once('/').unwrap();
                (
                    Some(PackageId {
                        name: package.into(),
                        version: version.into(),
                    }),
                    path,
                )
            }
            None => (None, s),
        };
        Path {
            package,
            segments: s.split(['.', '/']).map(|s| self.s(s)).collect(),
        }
    }
    fn origin(&self) -> Origin {
        Origin {
            span: Span {
                anchor: self.anchor,
                start: 0,
                end: self.sources.file(self.file.file).unwrap().text().len() as u32,
            },
            tokens: TokenRange {
                start: 0,
                end: self.tokens.len() as u32 - 1,
            },
        }
    }
    fn e(&mut self, kind: ExprKind) -> ExprId {
        let id = ExprId(self.tree.expressions.len() as u32);
        self.tree.expressions.push(Expr {
            origin: Origin {
                span: Span::default(),
                tokens: TokenRange::default(),
            },
            kind,
        });
        id
    }
    fn p(&mut self, kind: PatternKind) -> PatternId {
        let id = PatternId(self.tree.patterns.len() as u32);
        self.tree.patterns.push(Pattern {
            origin: self.origin(),
            kind,
        });
        id
    }
    fn t(&mut self, kind: TypeExprKind) -> TypeExprId {
        let id = TypeExprId(self.tree.types.len() as u32);
        self.tree.types.push(TypeExpr {
            origin: Origin {
                span: Span::default(),
                tokens: TokenRange::default(),
            },
            kind,
        });
        id
    }
    fn st(&mut self, kind: StmtKind) -> StmtId {
        let id = StmtId(self.tree.statements.len() as u32);
        self.tree.statements.push(Stmt {
            origin: self.origin(),
            kind,
        });
        id
    }
    fn named(&mut self, s: &str) -> TypeExprId {
        let path = self.path(s);
        self.t(TypeExprKind::Named {
            path,
            arguments: vec![],
        })
    }
    fn value(&mut self, s: &str) -> ExprId {
        let path = self.path(s);
        let id = self.e(ExprKind::Path(path));
        if s.contains('.') {
            if let Some(start) = self.sources.file(self.file.file).unwrap().text().find(s) {
                let first = self
                    .tokens
                    .iter()
                    .position(|t| t.span.start as usize == start)
                    .unwrap();
                let end = self
                    .tokens
                    .iter()
                    .position(|t| t.span.end as usize == start + s.len())
                    .unwrap()
                    + 1;
                self.tree.expressions[id.0 as usize].origin.tokens = TokenRange {
                    start: first as u32,
                    end: end as u32,
                };
            }
        }
        id
    }
    fn int(&mut self, s: &str) -> ExprId {
        self.e(ExprKind::Literal(Literal::Integer(s.into())))
    }
    fn block(&mut self, statements: Vec<StmtId>, tail: Option<ExprId>) -> ExprId {
        self.e(ExprKind::Block {
            label: None,
            statements,
            tail,
        })
    }
    fn item(&mut self, name: &str, kind: ItemKind) {
        let s = self.s(name);
        let first = self.file.items.is_empty();
        let mut tree = self.tree.clone();
        if first {
            tree.tokens = self.tokens[..self.tokens.len() - 1].to_vec();
            self.file.tokens = self.tokens[self.tokens.len() - 1..].to_vec();
        }
        let mut origin = self.origin();
        if !first {
            origin.tokens = TokenRange::default();
        }
        let item = Item {
            name_span: Default::default(),
            origin,
            name: s,
            definition: DefPath(ItemPath {
                package: PackageId {
                    name: "test".into(),
                    version: "1".into(),
                },
                modules: vec![],
                name: name.into(),
            }),
            visibility: Visibility::Private,
            attributes: vec![],
            kind,
            generated: None,
        };
        self.file.items.push(Arc::new(ItemSyntax {
            item,
            tree,
            anchor: self.anchor,
            text_hash: ContentHash::default(),
            occurrence: 0,
        }));
    }
    /// The item built `index`th, to adjust after building it: it sees every
    /// node built so far.
    fn built(&mut self, index: usize) -> &mut Item {
        let syntax = Arc::make_mut(&mut self.file.items[index]);
        let tokens = std::mem::take(&mut syntax.tree.tokens);
        syntax.tree = self.tree.clone();
        syntax.tree.tokens = tokens;
        &mut syntax.item
    }
    fn main(&mut self, e: ExprId) {
        let b = self.block(vec![], Some(e));
        self.item(
            "main",
            ItemKind::Function {
                signature: FunctionSyntax::default(),
                body: Some(b),
                abi: None,
            },
        );
    }
    fn check(&self, expected: &str) {
        let result = format(&self.file, &self.sources);
        assert!(!result.unchanged, "unexpected fallback: {}", result.source);
        assert_eq!(result.source, expected);
    }
    fn expr_case(source: &str, expected: &str, build: impl FnOnce(&mut Self) -> ExprId) {
        let mut f = Self::new(&format!("fn main() {{ {source} }}"));
        let e = build(&mut f);
        f.main(e);
        f.check(&format!("fn main() {{\n  {expected}\n}}\n"));
    }
}
// This fixture scanner only supplies tape spans and names, not AST structure.
fn tape(source: &str, file: Anchor) -> Vec<Token> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < source.len() {
        let start = i;
        let rest = &source[i..];
        let kind = if rest.starts_with("//") {
            i += rest.find(['\r', '\n']).unwrap_or(rest.len());
            TokenKind::Trivia(if rest.starts_with("///") {
                TriviaKind::DocLine
            } else {
                TriviaKind::LineComment
            })
        } else if rest.starts_with("/*") {
            let mut depth = 1;
            i += 2;
            while depth > 0 && i < source.len() {
                if source[i..].starts_with("/*") {
                    depth += 1;
                    i += 2;
                } else if source[i..].starts_with("*/") {
                    depth -= 1;
                    i += 2;
                } else {
                    i += source[i..].chars().next().unwrap().len_utf8();
                }
            }
            TokenKind::Trivia(if rest.starts_with("/**") {
                TriviaKind::DocBlock
            } else {
                TriviaKind::BlockComment
            })
        } else if rest.starts_with('\n') || rest.starts_with("\r\n") {
            i += if rest.starts_with('\r') { 2 } else { 1 };
            TokenKind::Trivia(TriviaKind::Newline)
        } else if rest.starts_with(' ') || rest.starts_with('\t') {
            while i < source.len() && matches!(source.as_bytes()[i], b' ' | b'\t') {
                i += 1;
            }
            TokenKind::Trivia(TriviaKind::Space)
        } else if rest.starts_with('\'') {
            i += 1;
            while i < source.len() {
                let b = source.as_bytes()[i];
                i += 1;
                if b == b'\\' {
                    i += 1;
                } else if b == b'\'' {
                    break;
                }
            }
            TokenKind::Character
        } else if rest.starts_with('"') || rest.starts_with("b\"") {
            let bytes = rest.starts_with('b');
            i += if bytes { 2 } else { 1 };
            while i < source.len() {
                let b = source.as_bytes()[i];
                i += 1;
                if b == b'\\' {
                    i += 1;
                } else if b == b'"' {
                    break;
                }
            }
            if bytes {
                TokenKind::Bytes
            } else {
                TokenKind::String
            }
        } else if rest.as_bytes()[0].is_ascii_digit() {
            while i < source.len()
                && (source.as_bytes()[i].is_ascii_alphanumeric()
                    || source.as_bytes()[i] == b'_'
                    || (source.as_bytes()[i] == b'.'
                        && source.as_bytes().get(i + 1).is_some_and(u8::is_ascii_digit)))
            {
                i += 1;
            }
            if source[start..i].contains('.') {
                TokenKind::Float
            } else {
                TokenKind::Integer
            }
        } else if rest.as_bytes()[0].is_ascii_alphabetic() || rest.starts_with('_') {
            while i < source.len()
                && (source.as_bytes()[i].is_ascii_alphanumeric() || source.as_bytes()[i] == b'_')
            {
                i += 1;
            }
            let word = &source[start..i];
            use Keyword::*;
            let kw = match word {
                "fn" => Some(Fn),
                "let" => Some(Let),
                "mut" => Some(Mut),
                "match" => Some(Match),
                "if" => Some(If),
                "else" => Some(Else),
                "for" => Some(For),
                "in" => Some(In),
                "while" => Some(While),
                "break" => Some(Break),
                "continue" => Some(Continue),
                "return" => Some(Return),
                "take" => Some(Take),
                "copy" => Some(Copy),
                "unique" => Some(Unique),
                "type" => Some(Type),
                "distinct" => Some(Distinct),
                "flat" => Some(Flat),
                "resource" => Some(Resource),
                "drop" => Some(Drop),
                "box" => Some(Box),
                "pub" => Some(Pub),
                "opaque" => Some(Opaque),
                "const" => Some(Const),
                "comptime" => Some(Comptime),
                "shape" => Some(Shape),
                "heap" => Some(Heap),
                "before" => Some(Before),
                "requires" => Some(Requires),
                "ensures" => Some(Ensures),
                "result" => Some(Result),
                "where" => Some(Where),
                "uses" => Some(Uses),
                "of" => Some(Of),
                "local" => Some(Local),
                "global" => Some(Global),
                "once" => Some(Once),
                "todo" => Some(Todo),
                "extern" => Some(Extern),
                "unsafe" => Some(Unsafe),
                "true" => Some(True),
                "false" => Some(False),
                _ => None,
            };
            kw.map_or_else(
                || TokenKind::Ident(Symbol::intern(word)),
                TokenKind::Keyword,
            )
        } else {
            use Punct::*;
            let punctuation = [
                ("<<=", ShlEq),
                (">>=", ShrEq),
                ("..=", DotDotEq),
                ("..", DotDot),
                ("->", Arrow),
                ("=>", FatArrow),
                ("&&", AndAnd),
                ("||", OrOr),
                ("<<", Shl),
                (">>", Shr),
                ("==", EqEq),
                ("!=", Ne),
                ("<=", Le),
                (">=", Ge),
                ("+=", PlusEq),
                ("-=", MinusEq),
                ("*=", StarEq),
                ("/=", SlashEq),
                ("%=", PercentEq),
                ("&=", AmpEq),
                ("|=", PipeEq),
                ("^=", CaretEq),
                ("(", LParen),
                (")", RParen),
                ("[", LBracket),
                ("]", RBracket),
                ("{", LBrace),
                ("}", RBrace),
                (",", Comma),
                (":", Colon),
                (";", Semicolon),
                (".", Dot),
                ("@", At),
                ("?", Question),
                ("+", Plus),
                ("-", Minus),
                ("*", Star),
                ("/", Slash),
                ("%", Percent),
                ("!", Bang),
                ("~", Tilde),
                ("&", Amp),
                ("|", Pipe),
                ("^", Caret),
                ("=", Eq),
                ("<", Lt),
                (">", Gt),
            ];
            if let Some((s, p)) = punctuation.iter().find(|(s, _)| rest.starts_with(s)) {
                i += s.len();
                TokenKind::Punct(*p)
            } else {
                i += rest.chars().next().unwrap().len_utf8();
                TokenKind::Invalid
            }
        };
        out.push(Token {
            kind,
            span: Span {
                anchor: file,
                start: start as u32,
                end: i as u32,
            },
        });
    }
    out.push(Token {
        kind: TokenKind::Eof,
        span: Span {
            anchor: file,
            start: i as u32,
            end: i as u32,
        },
    });
    out
}

#[test]
fn precedence_associativity_and_nonassociative_comparisons() {
    Fixture::expr_case("(a+b)*c", "(a + b) * c", |f| {
        let a = f.value("a");
        let b = f.value("b");
        let c = f.value("c");
        let left = f.e(ExprKind::Binary {
            op: BinaryOp::Add,
            left: a,
            right: b,
        });
        f.e(ExprKind::Binary {
            op: BinaryOp::Mul,
            left,
            right: c,
        })
    });
    Fixture::expr_case("a-(b-c)", "a - (b - c)", |f| {
        let a = f.value("a");
        let b = f.value("b");
        let c = f.value("c");
        let right = f.e(ExprKind::Binary {
            op: BinaryOp::Sub,
            left: b,
            right: c,
        });
        f.e(ExprKind::Binary {
            op: BinaryOp::Sub,
            left: a,
            right,
        })
    });
    Fixture::expr_case("(a<b)==c", "(a < b) == c", |f| {
        let a = f.value("a");
        let b = f.value("b");
        let c = f.value("c");
        let left = f.e(ExprKind::Binary {
            op: BinaryOp::Lt,
            left: a,
            right: b,
        });
        f.e(ExprKind::Binary {
            op: BinaryOp::Eq,
            left,
            right: c,
        })
    });
    Fixture::expr_case("-(a+b)", "-(a + b)", |f| {
        let a = f.value("a");
        let b = f.value("b");
        let value = f.e(ExprKind::Binary {
            op: BinaryOp::Add,
            left: a,
            right: b,
        });
        f.e(ExprKind::Unary {
            op: UnaryOp::Neg,
            value,
        })
    });
    Fixture::expr_case("(return a)+b", "(return a) + b", |f| {
        let a = f.value("a");
        let left = f.e(ExprKind::Return(Some(a)));
        let right = f.value("b");
        f.e(ExprKind::Binary {
            op: BinaryOp::Add,
            left,
            right,
        })
    });
}
#[test]
fn comments_in_grouped_continuations_keep_parentheses() {
    let mut f = Fixture::new("fn main() { (a // operand\n + b) }");
    let a = f.value("a");
    let b = f.value("b");
    let e = f.e(ExprKind::Binary {
        op: BinaryOp::Add,
        left: a,
        right: b,
    });
    let start = f
        .tokens
        .iter()
        .position(|t| f.sources.slice(t.span) == Some("(") && t.span.start > 10)
        .unwrap();
    let end = f
        .tokens
        .iter()
        .rposition(|t| f.sources.slice(t.span) == Some(")"))
        .unwrap()
        + 1;
    f.tree.expressions[e.0 as usize].origin.tokens = TokenRange {
        start: start as u32,
        end: end as u32,
    };
    f.main(e);
    f.check("fn main() {\n  (a // operand\n  + b)\n}\n");
}
#[test]
fn inline_comments_do_not_move_to_another_operand() {
    Fixture::expr_case(
        "a /* left */ + /* right */ b",
        "a /* left */ + /* right */ b",
        |f| {
            let left = f.value("a");
            let right = f.value("b");
            f.e(ExprKind::Binary {
                op: BinaryOp::Add,
                left,
                right,
            })
        },
    );
}
#[test]
fn empty_and_comment_only_files() {
    Fixture::new("").check("");
    Fixture::new(" \r\n").check("");
    Fixture::new("// hello\r\n\r\n/** end */").check("// hello\n/** end */\n");
}
#[test]
fn malformed_or_unrepresented_syntax_is_byte_identical() {
    for source in [
        "fn main() { $ }",
        "fn main() { x",
        "fn a() {} garbage",
        "fn a(] {}",
    ] {
        let mut f = Fixture::new(source);
        let body = f.block(vec![], None);
        f.item(
            "a",
            ItemKind::Function {
                signature: FunctionSyntax::default(),
                body: Some(body),
                abi: None,
            },
        );
        let r = format(&f.file, &f.sources);
        assert!(r.unchanged);
        assert_eq!(r.source, source);
    }
    let mut f = Fixture::new("fn main() { x }");
    let e = f.e(ExprKind::Error);
    f.main(e);
    assert!(format(&f.file, &f.sources).unchanged);
    let mut f = Fixture::new("fn main() {}");
    let body = f.block(vec![], None);
    f.item(
        "main",
        ItemKind::Function {
            signature: FunctionSyntax::default(),
            body: Some(body),
            abi: None,
        },
    );
    f.file.has_syntax_errors = true;
    let r = format(&f.file, &f.sources);
    assert!(r.unchanged);
    assert_eq!(r.source, "fn main() {}");
    Arc::make_mut(&mut f.file.items[0]).tree.tokens.remove(0);
    assert!(format(&f.file, &f.sources).unchanged);
}
#[test]
fn trailing_closure_authority_moves_only_when_comment_order_allows() {
    for (source, expected) in [
        ("fn() { x } uses {}", "fn() uses {} {\n    x\n  }"),
        (
            "fn() { /* body */ x } uses {}",
            "fn() { /* body */\n    x\n  } uses {}",
        ),
    ] {
        let mut f = Fixture::new(&format!("fn main() {{ {source} }}"));
        let x = f.value("x");
        let body = f.block(vec![], Some(x));
        let e = f.e(ExprKind::Closure {
            signature: FunctionSyntax {
                uses: Some(vec![]),
                ..Default::default()
            },
            body,
        });
        let opens: Vec<_> = f
            .tokens
            .iter()
            .enumerate()
            .filter(|(_, t)| matches!(t.kind, TokenKind::Punct(Punct::LBrace)))
            .map(|(i, _)| i)
            .collect();
        let body_start = opens[1];
        let body_end = (body_start + 1..f.tokens.len())
            .find(|i| matches!(f.tokens[*i].kind, TokenKind::Punct(Punct::RBrace)))
            .unwrap()
            + 1;
        let closure_start = (0..body_start)
            .rev()
            .find(|i| matches!(f.tokens[*i].kind, TokenKind::Keyword(Keyword::Fn)))
            .unwrap();
        let closure_end = (body_end..f.tokens.len())
            .find(|i| matches!(f.tokens[*i].kind, TokenKind::Punct(Punct::RBrace)))
            .unwrap()
            + 1;
        f.tree.expressions[body.0 as usize].origin.tokens = TokenRange {
            start: body_start as u32,
            end: body_end as u32,
        };
        f.tree.expressions[e.0 as usize].origin.tokens = TokenRange {
            start: closure_start as u32,
            end: closure_end as u32,
        };
        f.main(e);
        f.check(&format!("fn main() {{\n  {expected}\n}}\n"));
    }
}
#[test]
fn mapped_try_projection_requires_grouping() {
    Fixture::expr_case("(x ? Err).field", "(x ? Err).field", |f| {
        let value = f.value("x");
        let map_error = Some(f.path("Err"));
        let base = f.e(ExprKind::Try { value, map_error });
        let field = f.s("field");
        f.e(ExprKind::Field { base, field })
    });
}
mod spec;

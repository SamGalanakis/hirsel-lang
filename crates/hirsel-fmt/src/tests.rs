use super::*;
use hirsel_intern::{Interner, Symbol};
use hirsel_span::{FileId, Span};

/// Test-only token fixtures: the trees themselves are built by hand, never parsed.
struct Fixture {
    file: SyntaxFile,
    sources: SourceMap,
    names: Interner,
}
impl Fixture {
    fn new(source: &str) -> Self {
        let mut sources = SourceMap::default();
        let file = sources.add("test.hir", source.into()).unwrap();
        let mut names = Interner::default();
        let tokens = tape(source, file, &mut names);
        Self {
            file: SyntaxFile {
                has_syntax_errors: false,
                symbols: std::sync::Arc::new(names.clone()),

                file,
                tokens,
                tree: SyntaxTree::default(),
                roots: vec![],
            },
            sources,
            names,
        }
    }
    fn s(&mut self, s: &str) -> Symbol {
        let symbol = self.names.intern(s);
        self.file.symbols = std::sync::Arc::new(self.names.clone());
        symbol
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
                file: self.file.file,
                start: 0,
                end: self.sources.file(self.file.file).unwrap().text().len() as u32,
            },
            tokens: TokenRange {
                start: 0,
                end: self.file.tokens.len() as u32 - 1,
            },
        }
    }
    fn e(&mut self, kind: ExprKind) -> ExprId {
        let id = ExprId(self.file.tree.expressions.len() as u32);
        self.file.tree.expressions.push(Expr {
            origin: Origin {
                span: Span::default(),
                tokens: TokenRange::default(),
            },
            kind,
        });
        id
    }
    fn p(&mut self, kind: PatternKind) -> PatternId {
        let id = PatternId(self.file.tree.patterns.len() as u32);
        self.file.tree.patterns.push(Pattern {
            origin: self.origin(),
            kind,
        });
        id
    }
    fn t(&mut self, kind: TypeExprKind) -> TypeExprId {
        let id = TypeExprId(self.file.tree.types.len() as u32);
        self.file.tree.types.push(TypeExpr {
            origin: Origin {
                span: Span::default(),
                tokens: TokenRange::default(),
            },
            kind,
        });
        id
    }
    fn st(&mut self, kind: StmtKind) -> StmtId {
        let id = StmtId(self.file.tree.statements.len() as u32);
        self.file.tree.statements.push(Stmt {
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
                    .file
                    .tokens
                    .iter()
                    .position(|t| t.span.start as usize == start)
                    .unwrap();
                let end = self
                    .file
                    .tokens
                    .iter()
                    .position(|t| t.span.end as usize == start + s.len())
                    .unwrap()
                    + 1;
                self.file.tree.expressions[id.0 as usize].origin.tokens = TokenRange {
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
        self.e(ExprKind::Block { statements, tail })
    }
    fn item(&mut self, name: &str, kind: ItemKind) {
        let s = self.s(name);
        let id = ItemId(self.file.tree.items.len() as u32);
        self.file.tree.items.push(Item {
            name_span: Default::default(),

            origin: self.origin(),
            owned_tokens: self.origin().tokens,
            name: s,
            definition: DefPath(ItemPath {
                package: PackageId {
                    name: "test".into(),
                    version: "1".into(),
                },
                modules: vec![],
                name: name.into(),
            }),
            text_hash: ContentHash::default(),
            visibility: Visibility::Private,
            attributes: vec![],
            kind,
        });
        self.file.roots.push(id);
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
fn tape(source: &str, file: FileId, names: &mut Interner) -> Vec<Token> {
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
            kw.map_or_else(|| TokenKind::Ident(names.intern(word)), TokenKind::Keyword)
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
                file,
                start: start as u32,
                end: i as u32,
            },
        });
    }
    out.push(Token {
        kind: TokenKind::Eof,
        span: Span {
            file,
            start: i as u32,
            end: i as u32,
        },
    });
    out
}

#[test]
fn literal_spellings_unit_and_sequences() {
    for (s, l) in [
        ("'é'", Literal::Character("'é'".into())),
        ("'\\u{1f411}'", Literal::Character("'\\u{1f411}'".into())),
        ("0xffu8", Literal::Integer("0xffu8".into())),
        ("1.25f32", Literal::Float("1.25f32".into())),
        ("\"é\\n\"", Literal::String("\"é\\n\"".into())),
        ("b\"\\xff\"", Literal::Bytes("b\"\\xff\"".into())),
        ("true", Literal::Bool(true)),
        ("false", Literal::Bool(false)),
    ] {
        Fixture::expr_case(s, s, |f| f.e(ExprKind::Literal(l)));
    }
    Fixture::expr_case("()", "()", |f| f.e(ExprKind::Tuple(vec![])));
    Fixture::expr_case("(1,)", "(1,)", |f| {
        let a = f.int("1");
        f.e(ExprKind::Tuple(vec![a]))
    });
    Fixture::expr_case("[1,2]", "[1, 2]", |f| {
        let a = f.int("1");
        let b = f.int("2");
        f.e(ExprKind::Sequence(vec![a, b]))
    });
    Fixture::expr_case("[]", "[]", |f| f.e(ExprKind::Sequence(vec![])));
}
#[test]
fn every_prefix_operator_and_comptime() {
    for (op, s) in [
        (UnaryOp::Neg, "-x"),
        (UnaryOp::Not, "!x"),
        (UnaryOp::BitNot, "~x"),
        (UnaryOp::Take, "take x"),
        (UnaryOp::Copy, "copy x"),
        (UnaryOp::Mut, "mut x"),
        (UnaryOp::Unique, "unique x"),
        (UnaryOp::Box, "box x"),
    ] {
        Fixture::expr_case(s, s, |f| {
            let v = f.value("x");
            f.e(ExprKind::Unary { op, value: v })
        });
    }
    Fixture::expr_case("comptime x", "comptime x", |f| {
        let v = f.value("x");
        f.e(ExprKind::Comptime(v))
    });
}
#[test]
fn every_binary_operator() {
    use BinaryOp::*;
    for op in [
        Add,
        Sub,
        Mul,
        Div,
        Rem,
        Shl,
        Shr,
        BitAnd,
        BitOr,
        BitXor,
        Eq,
        Ne,
        Lt,
        Le,
        Gt,
        Ge,
        And,
        Or,
        Range,
        RangeInclusive,
    ] {
        let (s, p) = crate::printer::binary_for_test(op);
        let text = if p == 1 {
            format!("a{s}b")
        } else {
            format!("a {s} b")
        };
        Fixture::expr_case(&text, &text, |f| {
            let left = f.value("a");
            let right = f.value("b");
            f.e(ExprKind::Binary { op, left, right })
        });
    }
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
fn calls_labels_fields_indices_and_error_mapping() {
    Fixture::expr_case(
        "f(into: mut xs)[0].part? Err",
        "f(into: mut xs)[0].part ? Err",
        |f| {
            let callee = f.value("f");
            let xs = f.value("xs");
            let value = f.e(ExprKind::Unary {
                op: UnaryOp::Mut,
                value: xs,
            });
            let label = f.s("into");
            let base = f.e(ExprKind::Call {
                callee,
                arguments: vec![Argument {
                    label: Some(label),
                    value,
                }],
            });
            let index = f.int("0");
            let base = f.e(ExprKind::Index { base, index });
            let field = f.s("part");
            let value = f.e(ExprKind::Field { base, field });
            let map_error = Some(f.path("Err"));
            f.e(ExprKind::Try { value, map_error })
        },
    );
    Fixture::expr_case("f()?", "f()?", |f| {
        let callee = f.value("f");
        let value = f.e(ExprKind::Call {
            callee,
            arguments: vec![],
        });
        f.e(ExprKind::Try {
            value,
            map_error: None,
        })
    });
    Fixture::expr_case("(a+b)(x)", "(a + b)(x)", |f| {
        let left = f.value("a");
        let right = f.value("b");
        let callee = f.e(ExprKind::Binary {
            op: BinaryOp::Add,
            left,
            right,
        });
        let value = f.value("x");
        f.e(ExprKind::Call {
            callee,
            arguments: vec![Argument { label: None, value }],
        })
    });
}
#[test]
fn versioned_paths_and_reserved_members() {
    Fixture::expr_case("json@1/value.parse", "json@1/value.parse", |f| {
        let mut p = f.path("value.parse");
        p.package = Some(PackageId {
            name: "json".into(),
            version: "1".into(),
        });
        f.e(ExprKind::Path(p))
    });
    let mut f = Fixture::new("fn main() { (x).type }");
    let base = f.value("x");
    let field = f.s("type");
    let e = f.e(ExprKind::Field { base, field });
    let start = f
        .file
        .tokens
        .iter()
        .position(|t| f.sources.slice(t.span) == Some("x"))
        .unwrap();
    let end = f
        .file
        .tokens
        .iter()
        .position(|t| f.sources.slice(t.span) == Some("type"))
        .unwrap()
        + 1;
    f.file.tree.expressions[e.0 as usize].origin.tokens = TokenRange {
        start: start as u32,
        end: end as u32,
    };
    f.main(e);
    f.check("fn main() {\n  (x).type\n}\n");
}
#[test]
fn flow_control_closures_and_special_expressions() {
    Fixture::expr_case(
        "if x { a } else if y { b } else { c }",
        "if x {\n    a\n  } else if y {\n    b\n  } else {\n    c\n  }",
        |f| {
            let x = f.value("x");
            let y = f.value("y");
            let a = f.value("a");
            let b = f.value("b");
            let c = f.value("c");
            let a = f.block(vec![], Some(a));
            let b = f.block(vec![], Some(b));
            let c = f.block(vec![], Some(c));
            let other = f.e(ExprKind::If {
                condition: y,
                then_branch: b,
                else_branch: Some(c),
            });
            f.e(ExprKind::If {
                condition: x,
                then_branch: a,
                else_branch: Some(other),
            })
        },
    );
    Fixture::expr_case("fn() uses {} { x }", "fn() uses {} {\n    x\n  }", |f| {
        let v = f.value("x");
        let body = f.block(vec![], Some(v));
        let signature = FunctionSyntax {
            uses: Some(vec![]),
            ..Default::default()
        };
        f.e(ExprKind::Closure { signature, body })
    });
    for (text, kind) in [
        ("break", ExprKind::Break(None)),
        ("continue", ExprKind::Continue(None)),
        ("return", ExprKind::Return(None)),
        ("heap", ExprKind::Heap),
        ("result", ExprKind::Result),
        ("todo", ExprKind::Todo(None)),
        ("todo(\"later\")", ExprKind::Todo(Some("\"later\"".into()))),
    ] {
        Fixture::expr_case(text, text, |f| f.e(kind));
    }
    Fixture::expr_case("return x", "return x", |f| {
        let v = f.value("x");
        f.e(ExprKind::Return(Some(v)))
    });
    Fixture::expr_case("shape(T)", "shape(T)", |f| {
        let t = f.named("T");
        f.e(ExprKind::Shape(t))
    });
    Fixture::expr_case("before(x)", "before(x)", |f| {
        let x = f.value("x");
        f.e(ExprKind::Before(x))
    });
    Fixture::expr_case("{}", "{}", |f| f.block(vec![], None));
}
#[test]
fn pattern_forms_and_match_guards() {
    Fixture::expr_case(
        "match x { Rec(a: (v, _), ..) if ok => v, -1 => 0, () => 1, true => 2 }",
        "match x {\n    Rec(a: (v, _), ..) if ok => v,\n    -1 => 0,\n    () => 1,\n    true => 2,\n  }",
        |f| {
            let v = f.s("v");
            let binding = f.p(PatternKind::Binding(v));
            let wild = f.p(PatternKind::Wildcard);
            let tuple = f.p(PatternKind::Tuple(vec![binding, wild]));
            let label = f.s("a");
            let path = f.path("Rec");
            let rec = f.p(PatternKind::Constructor {
                path,
                fields: vec![PatternField {
                    label: Some(label),
                    pattern: tuple,
                }],
                rest: true,
            });
            let minus = f.p(PatternKind::Literal(Literal::Integer("-1".into())));
            let unit = f.p(PatternKind::Tuple(vec![]));
            let boolean = f.p(PatternKind::Literal(Literal::Bool(true)));
            let guard = f.value("ok");
            let body = f.value("v");
            let zero = f.int("0");
            let one = f.int("1");
            let two = f.int("2");
            let value = f.value("x");
            f.e(ExprKind::Match {
                value,
                arms: vec![
                    MatchArm {
                        pattern: rec,
                        guard: Some(guard),
                        body,
                    },
                    MatchArm {
                        pattern: minus,
                        guard: None,
                        body: zero,
                    },
                    MatchArm {
                        pattern: unit,
                        guard: None,
                        body: one,
                    },
                    MatchArm {
                        pattern: boolean,
                        guard: None,
                        body: two,
                    },
                ],
            })
        },
    );
    Fixture::expr_case("match x {}", "match x {}", |f| {
        let value = f.value("x");
        f.e(ExprKind::Match {
            value,
            arms: vec![],
        })
    });
}
#[test]
fn all_statements_and_final_expression_discard() {
    let mut f = Fixture::new(
        "fn main() { let mut (x,): u8 = 0; x += 1; while ok { break; } for _ in xs { continue; } fn helper() {} x; }",
    );
    let x = f.s("x");
    let p = f.p(PatternKind::Binding(x));
    let p = f.p(PatternKind::Tuple(vec![p]));
    let ty = f.named("u8");
    let zero = f.int("0");
    let one = f.int("1");
    let x = f.value("x");
    let condition = f.value("ok");
    let br = f.e(ExprKind::Break(None));
    let br = f.st(StmtKind::Expr(br));
    let body = f.block(vec![br], None);
    let cont = f.e(ExprKind::Continue(None));
    let cont = f.st(StmtKind::Expr(cont));
    let cb = f.block(vec![cont], None);
    let wild = f.p(PatternKind::Wildcard);
    let source = f.value("xs");
    let local = f.s("helper");
    let lb = f.block(vec![], None);
    let stmts = vec![
        f.st(StmtKind::Let {
            mutable: true,
            pattern: p,
            ty: Some(ty),
            value: zero,
        }),
        f.st(StmtKind::Assign {
            place: x,
            op: AssignOp(Some(BinaryOp::Add)),
            value: one,
        }),
        f.st(StmtKind::While {
            condition,
            body,
            label: None,
        }),
        f.st(StmtKind::For {
            pattern: wild,
            source,
            body: cb,
            label: None,
        }),
        f.st(StmtKind::Function {
            name: local,
            signature: FunctionSyntax::default(),
            body: lb,
        }),
        f.st(StmtKind::Expr(x)),
    ];
    let body = f.block(stmts, None);
    f.item(
        "main",
        ItemKind::Function {
            signature: FunctionSyntax::default(),
            body: Some(body),
            abi: None,
        },
    );
    f.check("fn main() {\n  let mut (x,): u8 = 0\n  x += 1\n  while ok {\n    break;\n  }\n  for _ in xs {\n    continue;\n  }\n  fn helper() {}\n  x;\n}\n");
}
#[test]
fn every_assignment_operator() {
    use BinaryOp::*;
    for op in [
        None,
        Some(Add),
        Some(Sub),
        Some(Mul),
        Some(Div),
        Some(Rem),
        Some(Shl),
        Some(Shr),
        Some(BitAnd),
        Some(BitOr),
        Some(BitXor),
    ] {
        let operator = op.map_or("=".into(), |op| {
            format!("{}=", crate::printer::binary_for_test(op).0)
        });
        let mut f = Fixture::new(&format!("fn main() {{ x {operator} y }}"));
        let place = f.value("x");
        let value = f.value("y");
        let s = f.st(StmtKind::Assign {
            place,
            op: AssignOp(op),
            value,
        });
        let body = f.block(vec![s], None);
        f.item(
            "main",
            ItemKind::Function {
                signature: FunctionSyntax::default(),
                body: Some(body),
                abi: None,
            },
        );
        f.check(&format!("fn main() {{\n  x {operator} y\n}}\n"));
    }
}
#[test]
fn named_tuple_box_function_and_qualified_types() {
    for text in [
        "()",
        "(T,)",
        "box T",
        "List[T, 4]",
        "@noalloc fn(take T) -> U uses {}",
        "once (fn() -> T) in x",
        "unique sendable T of .x in heap",
        "T in arena",
        "T in {x.a, env}",
    ] {
        let mut f = Fixture::new(&format!("fn main() -> {text}"));
        let t = f.named("T");
        let result = match text {
            "()" => f.t(TypeExprKind::Tuple(vec![])),
            "(T,)" => f.t(TypeExprKind::Tuple(vec![t])),
            "box T" => f.t(TypeExprKind::Box(t)),
            "List[T, 4]" => {
                let path = f.path("List");
                f.t(TypeExprKind::Named {
                    path,
                    arguments: vec![TypeArgument::Type(t), TypeArgument::SizeLiteral(4)],
                })
            }
            "@noalloc fn(take T) -> U uses {}" => {
                let u = f.named("U");
                let name = f.s("noalloc");
                let a = Attribute {
                    name,
                    arguments: vec![],
                    origin: f.origin(),
                };
                f.t(TypeExprKind::Function(FunctionSyntax {
                    attributes: vec![a],
                    parameters: vec![Parameter {
                        label: None,
                        name: None,
                        passing: PassingMode::Take,
                        ty: t,
                    }],
                    result: Some(u),
                    uses: Some(vec![]),
                    ..Default::default()
                }))
            }
            "once (fn() -> T) in x" => {
                let base = f.t(TypeExprKind::Function(FunctionSyntax {
                    result: Some(t),
                    ..Default::default()
                }));
                let x = f.path("x");
                let origin = f.origin();
                f.t(TypeExprKind::Qualified {
                    base,
                    modifiers: vec![TypeModifier::Once],
                    brand: None,
                    relation: Some(RelationSyntax {
                        places: vec![RelationPlace::Path(x)],
                        origin,
                    }),
                })
            }
            "unique sendable T of .x in heap" => {
                let x = f.path("x");
                let origin = f.origin();
                f.t(TypeExprKind::Qualified {
                    base: t,
                    modifiers: vec![TypeModifier::Unique, TypeModifier::Sendable],
                    brand: Some(BrandSyntax {
                        sibling: true,
                        path: x,
                    }),
                    relation: Some(RelationSyntax {
                        places: vec![RelationPlace::Heap],
                        origin,
                    }),
                })
            }
            "T in {x.a, env}" => {
                let x = f.path("x");
                let a = f.s("a");
                let mut xa = x;
                xa.segments.push(a);
                let origin = f.origin();
                f.t(TypeExprKind::Qualified {
                    base: t,
                    modifiers: vec![],
                    brand: None,
                    relation: Some(RelationSyntax {
                        places: vec![RelationPlace::Path(xa), RelationPlace::Environment],
                        origin,
                    }),
                })
            }
            _ => {
                let arena = f.path("arena");
                let origin = f.origin();
                f.t(TypeExprKind::Qualified {
                    base: t,
                    modifiers: vec![],
                    brand: None,
                    relation: Some(RelationSyntax {
                        places: vec![RelationPlace::Path(arena)],
                        origin,
                    }),
                })
            }
        };
        f.item(
            "main",
            ItemKind::Function {
                signature: FunctionSyntax {
                    result: Some(result),
                    ..Default::default()
                },
                body: None,
                abi: None,
            },
        );
        f.check(&format!("fn main() -> {text}\n"));
    }
}
#[test]
fn generic_labelled_signatures_contracts_and_visibility() {
    let mut f = Fixture::new(
        "pub fn append[T: plain + flat + data + key + sendable + inert + copyable, n: size](into mut xs: T, take x: T) -> T uses {xs} requires x ensures result",
    );
    let t = f.named("T");
    let tname = f.s("T");
    let n = f.s("n");
    let xs = f.s("xs");
    let x = f.s("x");
    let into = f.s("into");
    let xp = f.path("xs");
    let req = f.value("x");
    let ens = f.e(ExprKind::Result);
    let signature = FunctionSyntax {
        generics: vec![
            GenericParam {
                name: tname,
                kind: GenericKind::Type(vec![
                    Bound::Plain,
                    Bound::Flat,
                    Bound::Data,
                    Bound::Key,
                    Bound::Sendable,
                    Bound::Inert,
                    Bound::Copyable,
                ]),
            },
            GenericParam {
                name: n,
                kind: GenericKind::Size,
            },
        ],
        parameters: vec![
            Parameter {
                label: Some(into),
                name: Some(xs),
                passing: PassingMode::Mut,
                ty: t,
            },
            Parameter {
                label: None,
                name: Some(x),
                passing: PassingMode::Take,
                ty: t,
            },
        ],
        result: Some(t),
        uses: Some(vec![xp]),
        contracts: vec![
            ContractSyntax {
                kind: ContractKind::Requires,
                expression: req,
            },
            ContractSyntax {
                kind: ContractKind::Ensures,
                expression: ens,
            },
        ],
        ..Default::default()
    };
    f.item(
        "append",
        ItemKind::Function {
            signature,
            body: None,
            abi: None,
        },
    );
    f.file.tree.items[0].visibility = Visibility::Public;
    f.check("pub fn append[T: plain + flat + data + key + sendable + inert + copyable, n: size](\n  into mut xs: T,\n  take x: T\n) -> T uses {xs}\n  requires x\n  ensures result\n");
}
#[test]
fn all_declaration_bodies_and_resource_drop() {
    let mut f = Fixture::new(
        "flat type Item { value: u8 } resource type File { dir: Dir } drop(f) {} distinct type Percent(v: u8) where ok type Tree = Leaf | Node(box Tree) type Only = Only() type Alias = u8 pub opaque type Cap: inert uses {} pub const X: u8 = 1 extern \"c\" fn foreign() fn generated() = comptime x",
    );
    let u = f.named("u8");
    let dir = f.named("Dir");
    let tree = f.named("Tree");
    let boxed = f.t(TypeExprKind::Box(tree));
    let v = f.s("value");
    let d = f.s("dir");
    let vf = FieldSyntax {
        growable: false,

        name: v,
        ty: u,
        origin: f.origin(),
    };
    let df = FieldSyntax {
        growable: false,

        name: d,
        ty: dir,
        origin: f.origin(),
    };
    let type_kind = |flavor, body, drop| ItemKind::Type {
        flavor,
        generics: vec![],
        bounds: None,
        authority: None,
        body,
        drop,
    };
    f.item(
        "Item",
        type_kind(TypeFlavor::Flat, TypeBody::Record(vec![vf]), None),
    );
    let binding = f.s("f");
    let body = f.block(vec![], None);
    f.item(
        "File",
        type_kind(
            TypeFlavor::Resource,
            TypeBody::Record(vec![df]),
            Some(DropSyntax {
                noalloc: false,
                binding,
                body,
            }),
        ),
    );
    let v = f.s("v");
    let fact = f.value("ok");
    let field = FieldSyntax {
        growable: false,

        name: v,
        ty: u,
        origin: f.origin(),
    };
    f.item(
        "Percent",
        type_kind(
            TypeFlavor::Distinct,
            TypeBody::Distinct {
                field,
                fact: Some(fact),
            },
            None,
        ),
    );
    let leaf = f.s("Leaf");
    let node = f.s("Node");
    f.item(
        "Tree",
        type_kind(
            TypeFlavor::Ordinary,
            TypeBody::Variants(vec![
                VariantSyntax {
                    name: leaf,
                    fields: vec![],
                },
                VariantSyntax {
                    name: node,
                    fields: vec![(None, boxed)],
                },
            ]),
            None,
        ),
    );
    let only = f.s("Only");
    f.item(
        "Only",
        type_kind(
            TypeFlavor::Ordinary,
            TypeBody::Variants(vec![VariantSyntax {
                name: only,
                fields: vec![],
            }]),
            None,
        ),
    );
    f.item(
        "Alias",
        type_kind(TypeFlavor::Ordinary, TypeBody::Alias(u), None),
    );
    f.item(
        "Cap",
        ItemKind::Type {
            flavor: TypeFlavor::Ordinary,
            generics: vec![],
            bounds: Some(vec![Bound::Inert]),
            authority: Some(vec![]),
            body: TypeBody::Opaque,
            drop: None,
        },
    );
    f.file.tree.items.last_mut().unwrap().visibility = Visibility::PublicOpaque;
    let value = f.int("1");
    f.item("X", ItemKind::Const { ty: u, value });
    f.file.tree.items.last_mut().unwrap().visibility = Visibility::Public;
    f.item(
        "foreign",
        ItemKind::Function {
            signature: FunctionSyntax::default(),
            body: None,
            abi: Some(hirsel_syntax::ForeignAbi {
                literal: "\"c\"".into(),
                contract: None,
            }),
        },
    );
    let x = f.value("x");
    let body = f.e(ExprKind::Comptime(x));
    f.item(
        "generated",
        ItemKind::Function {
            signature: FunctionSyntax::default(),
            body: Some(body),
            abi: None,
        },
    );
    f.check("flat type Item {\n  value: u8,\n}\n\nresource type File {\n  dir: Dir,\n}\ndrop(f) {}\n\ndistinct type Percent(v: u8) where ok\n\ntype Tree = Leaf | Node(box Tree)\n\ntype Only = Only()\n\ntype Alias = u8\n\npub opaque type Cap: inert uses {}\n\npub const X: u8 = 1\n\nextern \"c\" fn foreign()\n\nfn generated() = comptime x\n");
}
#[test]
fn attribute_payloads_and_named_variant_fields() {
    let mut f = Fixture::new("@layout(c) @align(8) @packed @custom(\"x\") type Rec = Rec(x: T)");
    let t = f.named("T");
    let name = f.s("Rec");
    let x = f.s("x");
    f.item(
        "Rec",
        ItemKind::Type {
            flavor: TypeFlavor::Ordinary,
            generics: vec![],
            bounds: None,
            authority: None,
            body: TypeBody::Variants(vec![VariantSyntax {
                name,
                fields: vec![(Some(x), t)],
            }]),
            drop: None,
        },
    );
    let c = f.s("c");
    let attrs = [
        ("layout", vec![AttributeArgument::Name(c)]),
        ("align", vec![AttributeArgument::Integer("8".into())]),
        ("packed", vec![]),
        ("custom", vec![AttributeArgument::String("\"x\"".into())]),
    ]
    .into_iter()
    .map(|(s, arguments)| Attribute {
        name: f.s(s),
        arguments,
        origin: f.origin(),
    })
    .collect();
    f.file.tree.items[0].attributes = attrs;
    f.check("@layout(c)\n@align(8)\n@packed\n@custom(\"x\")\ntype Rec = Rec(x: T)\n");
}
#[test]
fn line_doc_nested_block_and_final_comments_survive_in_order() {
    let mut f = Fixture::new(
        "/// docs\r\n/** block docs */\r\nfn a() {\r\n// inside\r\nx /* one /* nested */ two */\r\n} // trailing\r\n\r\n/// next\r\nfn b() {}\r\n// eof\r\n",
    );
    let x = f.value("x");
    let body = f.block(vec![], Some(x));
    f.item(
        "a",
        ItemKind::Function {
            signature: FunctionSyntax::default(),
            body: Some(body),
            abi: None,
        },
    );
    let body = f.block(vec![], None);
    f.item(
        "b",
        ItemKind::Function {
            signature: FunctionSyntax::default(),
            body: Some(body),
            abi: None,
        },
    );
    f.check("/// docs\n/** block docs */\nfn a() {\n  // inside\n  x /* one /* nested */ two */\n} // trailing\n\n/// next\nfn b() {}\n// eof\n");
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
        .file
        .tokens
        .iter()
        .position(|t| f.sources.slice(t.span) == Some("(") && t.span.start > 10)
        .unwrap();
    let end = f
        .file
        .tokens
        .iter()
        .rposition(|t| f.sources.slice(t.span) == Some(")"))
        .unwrap()
        + 1;
    f.file.tree.expressions[e.0 as usize].origin.tokens = TokenRange {
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
    f.file.tokens.remove(0);
    assert!(format(&f.file, &f.sources).unchanged);
}
#[test]
fn lists_break_only_within_delimiters() {
    let a = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    let b = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    Fixture::expr_case(
        &format!("f({a},{b})"),
        &format!("f(\n    {a},\n    {b}\n  )"),
        |f| {
            let callee = f.value("f");
            let a = f.value(a);
            let b = f.value(b);
            f.e(ExprKind::Call {
                callee,
                arguments: vec![
                    Argument {
                        label: None,
                        value: a,
                    },
                    Argument {
                        label: None,
                        value: b,
                    },
                ],
            })
        },
    );
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
            .file
            .tokens
            .iter()
            .enumerate()
            .filter(|(_, t)| matches!(t.kind, TokenKind::Punct(Punct::LBrace)))
            .map(|(i, _)| i)
            .collect();
        let body_start = opens[1];
        let body_end = (body_start + 1..f.file.tokens.len())
            .find(|i| matches!(f.file.tokens[*i].kind, TokenKind::Punct(Punct::RBrace)))
            .unwrap()
            + 1;
        let closure_start = (0..body_start)
            .rev()
            .find(|i| matches!(f.file.tokens[*i].kind, TokenKind::Keyword(Keyword::Fn)))
            .unwrap();
        let closure_end = (body_end..f.file.tokens.len())
            .find(|i| matches!(f.file.tokens[*i].kind, TokenKind::Punct(Punct::RBrace)))
            .unwrap()
            + 1;
        f.file.tree.expressions[body.0 as usize].origin.tokens = TokenRange {
            start: body_start as u32,
            end: body_end as u32,
        };
        f.file.tree.expressions[e.0 as usize].origin.tokens = TokenRange {
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
#[test]
fn type_sizes_and_path_separators_have_one_spelling() {
    let mut f = Fixture::new("fn main() -> pkg@1/mod/T[0x10]");
    let mut path = f.path("mod.T");
    path.package = Some(PackageId {
        name: "pkg".into(),
        version: "1".into(),
    });
    let result = f.t(TypeExprKind::Named {
        path,
        arguments: vec![TypeArgument::SizeLiteral(16)],
    });
    f.item(
        "main",
        ItemKind::Function {
            signature: FunctionSyntax {
                result: Some(result),
                ..Default::default()
            },
            body: None,
            abi: None,
        },
    );
    f.check("fn main() -> pkg@1/mod.T[16]\n");
}
#[test]
fn error_patterns_types_missing_ids_and_names_leave_source_intact() {
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
    for case in 0..4 {
        let mut file = f.file.clone();
        match case {
            0 => file.tree.patterns.push(Pattern {
                origin: f.origin(),
                kind: PatternKind::Error,
            }),
            1 => file.tree.types.push(TypeExpr {
                origin: f.origin(),
                kind: TypeExprKind::Error,
            }),
            2 => file.roots = vec![ItemId(99)],
            _ => {
                file.tree.expressions[body.0 as usize].kind = ExprKind::Path(Path {
                    package: None,
                    segments: vec![Symbol(99)],
                })
            }
        };
        let r = format(&file, &f.sources);
        assert!(r.unchanged);
        assert_eq!(r.source, "fn main() {}");
    }
}
#[test]
fn exact_width_boundary_and_overflow_are_deterministic() {
    for len in [40, 41] {
        let a = "a".repeat(len);
        let b = "b".repeat(len);
        let expected = if len == 40 {
            format!("f({a}, {b})")
        } else {
            format!("f(\n    {a},\n    {b}\n  )")
        };
        Fixture::expr_case(&format!("f({a}, {b})"), &expected, |f| {
            let callee = f.value("f");
            let a = f.value(&a);
            let b = f.value(&b);
            f.e(ExprKind::Call {
                callee,
                arguments: vec![
                    Argument {
                        label: None,
                        value: a,
                    },
                    Argument {
                        label: None,
                        value: b,
                    },
                ],
            })
        });
    }
}
mod spec;
#[test]
fn capability_keyword_payload_and_remaining_qualifier_pattern_forms() {
    let mut f = Fixture::new("@capability(global) pub opaque type Cap: inert uses {}");
    f.item(
        "Cap",
        ItemKind::Type {
            flavor: TypeFlavor::Ordinary,
            generics: vec![],
            bounds: Some(vec![Bound::Inert]),
            authority: Some(vec![]),
            body: TypeBody::Opaque,
            drop: None,
        },
    );
    f.file.tree.items[0].visibility = Visibility::PublicOpaque;
    let attribute = Attribute {
        name: f.s("capability"),
        arguments: vec![AttributeArgument::Keyword(Keyword::Global)],
        origin: f.origin(),
    };
    f.file.tree.items[0].attributes.push(attribute);
    f.check("@capability(global)\npub opaque type Cap: inert uses {}\n");
    let mut f = Fixture::new("fn main() -> T of arena in env");
    let base = f.named("T");
    let path = f.path("arena");
    let origin = f.origin();
    let result = f.t(TypeExprKind::Qualified {
        base,
        modifiers: vec![],
        brand: Some(BrandSyntax {
            sibling: false,
            path,
        }),
        relation: Some(RelationSyntax {
            places: vec![RelationPlace::Environment],
            origin,
        }),
    });
    f.item(
        "main",
        ItemKind::Function {
            signature: FunctionSyntax {
                result: Some(result),
                ..Default::default()
            },
            body: None,
            abi: None,
        },
    );
    f.check("fn main() -> T of arena in env\n");
    Fixture::expr_case(
        "match v { Some(x) => x, None() => 0 }",
        "match v {\n    Some(x) => x,\n    None() => 0,\n  }",
        |f| {
            let x = f.s("x");
            let binding = f.p(PatternKind::Binding(x));
            let path = f.path("Some");
            let some = f.p(PatternKind::Constructor {
                path,
                fields: vec![PatternField {
                    label: None,
                    pattern: binding,
                }],
                rest: false,
            });
            let path = f.path("None");
            let none = f.p(PatternKind::Constructor {
                path,
                fields: vec![],
                rest: false,
            });
            let body = f.value("x");
            let zero = f.int("0");
            let value = f.value("v");
            f.e(ExprKind::Match {
                value,
                arms: vec![
                    MatchArm {
                        pattern: some,
                        guard: None,
                        body,
                    },
                    MatchArm {
                        pattern: none,
                        guard: None,
                        body: zero,
                    },
                ],
            })
        },
    );
}
#[test]
fn hand_tree_comment_layout_is_a_fixed_point() {
    let mut f = Fixture::new("/// docs\nfn main() {\n let x = 1; // a\n x // tail\n}\n// end\n");
    let one = f.int("1");
    let x = f.s("x");
    let pattern = f.p(PatternKind::Binding(x));
    let s = f.st(StmtKind::Let {
        mutable: false,
        pattern,
        ty: None,
        value: one,
    });
    let tail = f.value("x");
    let body = f.block(vec![s], Some(tail));
    f.item(
        "main",
        ItemKind::Function {
            signature: FunctionSyntax::default(),
            body: Some(body),
            abi: None,
        },
    );
    let first = format(&f.file, &f.sources);
    assert!(!first.unchanged);
    let file = f
        .sources
        .add("formatted.hir", first.source.clone())
        .unwrap();
    f.file.file = file;
    f.file.tokens = tape(&first.source, file, &mut f.names);
    let origin = f.origin();
    for item in &mut f.file.tree.items {
        item.origin = origin;
        item.owned_tokens = origin.tokens;
    }
    let second = format(&f.file, &f.sources);
    assert!(!second.unchanged);
    assert_eq!(first.source, second.source);
}

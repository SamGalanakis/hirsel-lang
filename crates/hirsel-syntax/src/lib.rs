//! Indexed AST plus a lossless token/trivia tape (LANG-1, PROG-5).
//! See docs/spec/syntax.md for syntax and README.md for ownership of comments.

#[cfg(test)]
hirsel_guard::install!();

use hirsel_intern::Symbol;
use hirsel_span::{Anchor, FileId, Span};
use sha2::{Digest, Sha256};
use std::sync::Arc;
mod ids;
mod origins;

pub use hirsel_intern::IdHasher;
#[cfg(feature = "id-salt")]
pub use hirsel_intern::salt_ids;
pub use ids::{DefId, DefMap, with_def_numbering, with_def_source};

/// Stable SHA-256 digest of domain-separated, length-framed inputs (PROG-5).
#[derive(
    serde::Serialize,
    serde::Deserialize,
    Clone,
    Copy,
    Debug,
    Default,
    Eq,
    PartialEq,
    Hash,
    Ord,
    PartialOrd,
)]
pub struct ContentHash(pub [u8; 32]);
impl ContentHash {
    pub fn of_bytes(bytes: &[u8]) -> Self {
        Self::of_parts("hirsel.text.v1", [bytes])
    }
    pub fn of_parts<'a>(domain: &'a str, parts: impl IntoIterator<Item = &'a [u8]>) -> Self {
        let mut hash = Sha256::new();
        for part in std::iter::once(domain.as_bytes()).chain(parts) {
            hash.update((part.len() as u64).to_le_bytes());
            hash.update(part);
        }
        Self(hash.finalize().into())
    }
}
/// Package identity includes version; package names alone do not identify types (PROG-1).
#[derive(
    serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq, Hash, Ord, PartialOrd,
)]
pub struct PackageId {
    pub name: String,
    pub version: String,
}
impl PackageId {
    /// Match only the package name, preserving callers that ignore versions.
    pub fn name_is(&self, name: &str) -> bool {
        self.name == name
    }

    /// Match the exact package name and version.
    pub fn package_is(&self, name: &str, version: &str) -> bool {
        self.name_is(name) && self.version == version
    }
}
/// Canonical name; module components come from the definition manifest, not files (PROG-1).
#[derive(
    serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq, Hash, Ord, PartialOrd,
)]
pub struct ItemPath {
    pub package: PackageId,
    pub modules: Vec<String>,
    pub name: String,
}
/// Stable definition identity, unaffected by source positions, hashes or intern order (PROG-5).
#[derive(
    serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq, Hash, Ord, PartialOrd,
)]
pub struct DefPath(pub ItemPath);
impl DefPath {
    /// The reserved core@1 identity, with dot-separated module and item names.
    pub fn core(name: &str) -> Self {
        let mut modules: Vec<_> = name.split('.').map(str::to_owned).collect();
        let name = modules.pop().expect("core name");
        Self(ItemPath {
            package: PackageId {
                name: "core".into(),
                version: "1".into(),
            },
            modules,
            name,
        })
    }
    /// Compare a core@1 identity without constructing or allocating a path.
    pub fn is_core(&self, name: &str) -> bool {
        self.package_is("core", "1")
            && self
                .0
                .modules
                .iter()
                .map(String::as_str)
                .chain(std::iter::once(self.0.name.as_str()))
                .eq(name.split('.'))
    }
    /// Match a complete package identity, independent of modules and item name.
    pub fn package_is(&self, name: &str, version: &str) -> bool {
        self.0.package.package_is(name, version)
    }
    pub fn fingerprint(&self) -> ContentHash {
        let p = &self.0;
        let parts = std::iter::once(p.package.name.as_bytes())
            .chain(std::iter::once(p.package.version.as_bytes()))
            .chain(p.modules.iter().map(String::as_bytes))
            .chain(std::iter::once(p.name.as_bytes()));
        ContentHash::of_parts("hirsel.defpath.v1", parts)
    }
}
/// The canonical definition spelling used by semantic entries and proof exports.
impl std::fmt::Display for DefPath {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let item = &self.0;
        write!(f, "{}@{}", item.package.name, item.package.version)?;
        for part in item.modules.iter().chain(std::iter::once(&item.name)) {
            write!(f, "/{part}")?;
        }
        Ok(())
    }
}
macro_rules! ids { ($($name:ident),* $(,)?) => {$(
    /// Arena-local u32 index; not a stable definition identity (LANG-1, PROG-5).
    #[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Ord, PartialOrd, serde::Serialize, serde::Deserialize)]
    pub struct $name(pub u32);
)*}; }
ids!(TokenId, ExprId, PatternId, TypeExprId, StmtId);
/// Half-open indices into one item's token tape (LANG-1).
#[derive(serde::Serialize, serde::Deserialize, Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct TokenRange {
    pub start: u32,
    pub end: u32,
}
/// Source origin shared by AST nodes; spans exclude surrounding trivia (LANG-1).
#[derive(serde::Serialize, serde::Deserialize, Clone, Copy, Debug, Eq, PartialEq)]
pub struct Origin {
    pub span: Span,
    pub tokens: TokenRange,
}
/// Reserved words, exactly LANG-14; other special names are contextual.
#[derive(serde::Serialize, serde::Deserialize, Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum Keyword {
    Fn,
    Let,
    Mut,
    Match,
    If,
    Else,
    For,
    In,
    While,
    Break,
    Continue,
    Return,
    Take,
    Copy,
    Unique,
    Type,
    Distinct,
    Flat,
    Resource,
    Drop,
    Box,
    Pub,
    Opaque,
    Const,
    Comptime,
    Shape,
    Heap,
    Before,
    Requires,
    Ensures,
    Result,
    Where,
    Uses,
    Of,
    Local,
    Global,
    Once,
    Todo,
    Extern,
    Unsafe,
    True,
    False,
}
impl Keyword {
    /// The lexer and diagnostic repair admission use the same reserved words.
    pub fn from_spelling(text: &str) -> Option<Keyword> {
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
}

/// A legal ordinary identifier (LANG-14), excluding the discard pattern.
pub fn is_binding_name(name: &str) -> bool {
    let mut bytes = name.bytes();
    name != "_"
        && bytes
            .next()
            .is_some_and(|b| b.is_ascii_alphabetic() || b == b'_')
        && bytes.all(|b| b.is_ascii_alphanumeric() || b == b'_')
        && Keyword::from_spelling(name).is_none()
}
/// Longest-match punctuation, independent of type information (LANG-1/2; issue #21).
#[derive(serde::Serialize, serde::Deserialize, Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum Punct {
    LParen,
    RParen,
    LBracket,
    RBracket,
    LBrace,
    RBrace,
    Comma,
    Colon,
    Semicolon,
    Dot,
    DotDot,
    DotDotEq,
    At,
    Question,
    Arrow,
    FatArrow,
    Plus,
    Minus,
    Star,
    Slash,
    Percent,
    Bang,
    Tilde,
    Amp,
    Pipe,
    Caret,
    AndAnd,
    OrOr,
    Shl,
    Shr,
    Eq,
    EqEq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    PlusEq,
    MinusEq,
    StarEq,
    SlashEq,
    PercentEq,
    AmpEq,
    PipeEq,
    CaretEq,
    ShlEq,
    ShrEq,
}
/// Trivia remains in the tape, including CRLF and doc comments (LANG-1; issue #21).
#[derive(serde::Serialize, serde::Deserialize, Clone, Copy, Debug, Eq, PartialEq)]
pub enum TriviaKind {
    Space,
    Newline,
    LineComment,
    BlockComment,
    DocLine,
    DocBlock,
}
/// Tokens carry literal source spelling through their spans (SEM-1/2/3).
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq)]
pub enum TokenKind {
    Ident(Symbol),
    Label(Symbol),
    Keyword(Keyword),
    Punct(Punct),
    Integer,
    Float,
    Character,
    Byte,
    String,
    Bytes,
    /// One piece of an f-string literal (SEM-3); hole expressions are ordinary tokens.
    Format(FormatPiece),
    Trivia(TriviaKind),
    Invalid,
    Eof,
}
/// The pieces of `f"a{x}b{y}c"` are `f"a{` (Head), `}b{` (Middle) and `}c"` (Tail);
/// an f-string without holes is one `f"abc"` (Whole). Text keeps its spelling.
#[derive(serde::Serialize, serde::Deserialize, Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum FormatPiece {
    /// A colon and its raw format specification, ending before `}`.
    Spec,
    Whole,
    Head,
    Middle,
    Tail,
}
/// Every source byte belongs to exactly one non-EOF token, even on errors (LANG-1).
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq)]
pub struct Token {
    pub kind: TokenKind,
    pub span: Span,
}
/// Unresolved source path. Dot projections and module qualification share syntax (LANG-4).
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq)]
pub struct Path {
    pub package: Option<PackageId>,
    pub segments: Vec<Symbol>,
}
/// Parameter passing mode; copy/unique are expressions, not parameter conventions (OWN-1/2/3).
#[derive(serde::Serialize, serde::Deserialize, Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum PassingMode {
    Borrow,
    Take,
    Mut,
}
/// Unary operations including explicit ownership requests (LANG-14, SEM-1).
#[derive(serde::Serialize, serde::Deserialize, Clone, Copy, Debug, Eq, PartialEq)]
pub enum UnaryOp {
    Neg,
    Not,
    BitNot,
    Take,
    Copy,
    Mut,
    Unique,
    Box,
}
/// Binary operators, evaluated left-to-right except short-circuit booleans (LANG-3; #21).
#[derive(serde::Serialize, serde::Deserialize, Clone, Copy, Debug, Eq, PartialEq)]
pub enum BinaryOp {
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
}
/// Assignment is statement-only; None is plain '=' (LANG-5).
#[derive(serde::Serialize, serde::Deserialize, Clone, Copy, Debug, Eq, PartialEq)]
pub struct AssignOp(pub Option<BinaryOp>);
/// Literal spelling is preserved independently of type inference and numeric conversion (LANG-2).
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq)]
pub enum Literal {
    Integer(String),
    Float(String),
    Character(String),
    Byte(String),
    String(String),
    Bytes(String),
    Bool(bool),
}
/// The two's-complement bits, at `width`, of an integer, character or byte
/// literal; an integer spelled with a leading `-` is negated. None for any other
/// literal or an unreadable spelling. Range checks belong to the type checker.
pub fn scalar_bits(literal: &Literal, width: u32) -> Option<u128> {
    match literal {
        Literal::Character(s) => character_value(s).map(u128::from),
        Literal::Byte(s) => byte_value(s).map(u128::from),
        Literal::Integer(s) => {
            let (negative, s) = match s.strip_prefix('-') {
                Some(rest) => (true, rest),
                None => (false, s.as_str()),
            };
            let s = s.replace('_', "");
            let end = s.find(|c| c == 'u' || c == 'i').unwrap_or(s.len());
            let s = &s[..end];
            let (radix, digits) = if let Some(d) = s.strip_prefix("0x") {
                (16, d)
            } else if let Some(d) = s.strip_prefix("0o") {
                (8, d)
            } else if let Some(d) = s.strip_prefix("0b") {
                (2, d)
            } else {
                (10, s)
            };
            let n = u128::from_str_radix(digits, radix).ok()?;
            let mask = u128::MAX >> (128 - width.clamp(1, 128));
            Some(if negative {
                0u128.wrapping_sub(n) & mask
            } else {
                n & mask
            })
        }
        _ => None,
    }
}
/// Decode a single-quoted Unicode scalar spelling, retaining no source state.
/// Invalid delimiters, escapes and scalar counts return None.
pub fn character_value(spelling: &str) -> Option<char> {
    quoted_scalar(spelling.strip_prefix('\'')?.strip_suffix('\'')?, false)
}
/// Decode a `b'...'` byte spelling: one ASCII scalar, spelled as a character
/// literal's is, or a `\xNN` escape from 00 to 7F. Anything else returns None.
pub fn byte_value(spelling: &str) -> Option<u8> {
    let body = spelling.strip_prefix("b'")?.strip_suffix('\'')?;
    u8::try_from(quoted_scalar(body, true)?)
        .ok()
        .filter(u8::is_ascii)
}
/// The one scalar a quoted literal's body denotes; `hex` admits `\xNN`.
fn quoted_scalar(body: &str, hex: bool) -> Option<char> {
    let mut chars = body.chars();
    let first = chars.next()?;
    let value = if first == '\\' {
        match chars.next()? {
            'n' => '\n',
            't' => '\t',
            'r' => '\r',
            '0' => '\0',
            '\\' => '\\',
            '\'' => '\'',
            'x' if hex => {
                let high = chars.next()?.to_digit(16)?;
                let low = chars.next()?.to_digit(16)?;
                char::from_u32(high * 16 + low)?
            }
            'u' => {
                if chars.next()? != '{' {
                    return None;
                }
                let mut value = 0u32;
                let mut count = 0;
                loop {
                    let c = chars.next()?;
                    if c == '}' {
                        break;
                    }
                    count += 1;
                    if count > 6 || !c.is_ascii_hexdigit() {
                        return None;
                    }
                    value = value.checked_mul(16)?.checked_add(c.to_digit(16)?)?;
                }
                if count == 0 {
                    return None;
                }
                char::from_u32(value)?
            }
            _ => return None,
        }
    } else {
        if first <= '\u{1f}'
            || first == '\u{7f}'
            || matches!(first, '\'' | '\u{61c}' | '\u{200e}' | '\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
        {
            return None;
        }
        first
    };
    chars.next().is_none().then_some(value)
}

/// A labelled or positional argument; labels precede ownership expressions (LANG-12).
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq)]
pub struct Argument {
    pub label: Option<Symbol>,
    pub value: ExprId,
}
/// Match arm; refutability/exhaustiveness belongs to checking (LANG-7/13).
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq)]
pub struct MatchArm {
    pub pattern: PatternId,
    pub guard: Option<ExprId>,
    pub body: ExprId,
}
/// Indexed expression; references only nodes in its owning SyntaxTree (LANG-1/3).
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq)]
pub struct Expr {
    pub origin: Origin,
    pub kind: ExprKind,
}
/// All surface expressions; scope/task/arena operations are ordinary calls (CONC-2, MEM-4).
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq)]
pub enum ExprKind {
    Literal(Literal),
    Path(Path),
    Tuple(Vec<ExprId>),
    Sequence(Vec<ExprId>),
    Call {
        callee: ExprId,
        arguments: Vec<Argument>,
    },
    Field {
        base: ExprId,
        field: Symbol,
    },
    Index {
        base: ExprId,
        index: ExprId,
    },
    Unary {
        op: UnaryOp,
        value: ExprId,
    },
    Binary {
        op: BinaryOp,
        left: ExprId,
        right: ExprId,
    },
    Try {
        value: ExprId,
        map_error: Option<Path>,
    },
    /// A block; with a label it is a labelled block (`'l: { .. }`), whose value is
    /// its tail or the operand of a `break 'l` inside it (LANG-11).
    Block {
        label: Option<Symbol>,
        statements: Vec<StmtId>,
        tail: Option<ExprId>,
    },
    If {
        condition: ExprId,
        then_branch: ExprId,
        else_branch: Option<ExprId>,
    },
    Match {
        value: ExprId,
        arms: Vec<MatchArm>,
    },
    Closure {
        signature: FunctionSyntax,
        body: ExprId,
    },
    Return(Option<ExprId>),
    /// `break`, `break 'l` or `break 'l value`; only a labelled block takes a value (LANG-11).
    Break {
        label: Option<Symbol>,
        value: Option<ExprId>,
    },
    Continue(Option<Symbol>),
    Shape(TypeExprId),
    Before(ExprId),
    Comptime(ExprId),
    Heap,
    Result,
    Todo(Option<String>),
    /// The callee of one step of a desugared f-string (SEM-3), only ever called:
    /// the body checker selects its std operation from the step and, for an
    /// append, the appended value's type.
    Formatter(FormatStep),
    /// `f with (H = path, ..)`: a callee with its named implementation actuals
    /// (GEN-6), only ever called. Each actual is an implementation path or an
    /// enclosing implementation binder.
    With {
        callee: ExprId,
        bindings: Vec<StaticBinding>,
    },
    Error,
}
/// One `H = path` actual of a `with` clause (GEN-6).
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq)]
pub struct StaticBinding {
    pub name: Symbol,
    pub implementation: Path,
    pub origin: Origin,
}
/// `Append(value, mut builder)` appends a hole or text piece; `Finish(take builder)`
/// returns the built `String`. A hole whose options are only fill, alignment and
/// width is `Mark(builder)` (the builder's length), its plain `Append`, then
/// `Pad(mark, fill, align, width, mut builder)` padding the hole's text; any other
/// specified hole is one `AppendSpec(value, fill, align, width, precision, flags,
/// kind, mut builder)`. `Pad` names the hole whose type selects its operation.
#[derive(serde::Serialize, serde::Deserialize, Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum FormatStep {
    Append,
    AppendSpec(FormatSpec),
    Mark,
    Pad(ExprId),
    Finish,
}
/// Written formatting options. Alignment: 0 default, 1 left, 2 right, 3 center.
/// Flags: 1 plus, 2 alternate, 4 sign-aware zero padding, 8 precision present.
/// Kind: 0 display or the ASCII radix letter. A width or precision written as
/// `name$` is a runtime argument; its field here is then 0.
#[derive(serde::Serialize, serde::Deserialize, Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub struct FormatSpec {
    pub fill: u64,
    pub align: u64,
    pub width: u64,
    pub precision: u64,
    pub flags: u64,
    pub kind: u64,
}
/// A written f-string (SEM-3). The parser desugars it to `expression`, an ordinary
/// block every later phase reads; the formatter prints these written pieces instead.
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq)]
pub struct FormatString {
    pub expression: ExprId,
    /// Piece spellings, one more than `holes`: `f"a{`, `}b"`, or a single `f"ab"`.
    pub pieces: Vec<String>,
    pub holes: Vec<ExprId>,
    /// One spelling per hole, empty for a plain hole, otherwise starting with `:`.
    pub specs: Vec<String>,
}
/// A written `if let` or `while let` (LANG-13). The parser desugars each to an
/// ordinary match every later phase reads; the formatter prints the written form.
#[derive(serde::Serialize, serde::Deserialize, Clone, Copy, Debug, Eq, PartialEq)]
pub enum LetCondition {
    /// `if let P = e A`, optionally `else B`: `matching` is
    /// `match e { P => A, _ => B }`, with `{}` for an omitted else.
    If { matching: ExprId, otherwise: bool },
    /// `while let P = e A`: the `while true` statement whose body block `body`
    /// holds only `matching`, `match e { P => A, _ => break }`.
    While { body: ExprId, matching: ExprId },
}
impl LetCondition {
    /// The desugared match.
    pub fn matching(self) -> ExprId {
        match self {
            Self::If { matching, .. } | Self::While { matching, .. } => matching,
        }
    }
}
/// A parenthesized expression, kept so a parse can tell `(if ..)` from `if ..`;
/// grouping is otherwise implied by the tree.
#[derive(serde::Serialize, serde::Deserialize, Clone, Copy, Debug, Eq, PartialEq)]
pub struct Group {
    pub expression: ExprId,
    /// The parentheses and everything between them.
    pub origin: Origin,
}
/// Statement origin includes its terminator; the block tail does not (LANG-11).
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq)]
pub struct Stmt {
    pub origin: Origin,
    pub kind: StmtKind,
}
/// Local definitions are ordered and cannot recurse (LANG-10/15).
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq)]
pub enum StmtKind {
    Let {
        mutable: bool,
        pattern: PatternId,
        ty: Option<TypeExprId>,
        value: ExprId,
        /// `else { .. }` of a refutable let: a block that diverges, run when
        /// the pattern does not match; the bindings are not in scope there (LANG-13).
        otherwise: Option<ExprId>,
    },
    Assign {
        place: ExprId,
        op: AssignOp,
        value: ExprId,
    },
    Expr(ExprId),
    While {
        label: Option<Symbol>,
        condition: ExprId,
        body: ExprId,
    },
    For {
        label: Option<Symbol>,
        pattern: PatternId,
        source: ExprId,
        body: ExprId,
    },
    Function {
        name: Symbol,
        signature: FunctionSyntax,
        body: ExprId,
    },
}
impl StmtKind {
    /// The written loop label, in a namespace separate from value bindings.
    pub fn loop_label(&self) -> Option<Symbol> {
        match self {
            Self::While { label, .. } | Self::For { label, .. } => *label,
            _ => None,
        }
    }
}
/// Pattern tree node, with constructor names resolved using the expected type (LANG-2/13).
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq)]
pub struct Pattern {
    pub origin: Origin,
    pub kind: PatternKind,
}
/// Record '..' ignores remaining fields; no expression ellipsis is permitted (OWN-8; #25).
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq)]
pub enum PatternKind {
    Wildcard,
    Binding(Symbol),
    Literal(Literal),
    /// Closed scalar range `start..=end`; bounds are literals, a negative integer
    /// spelled with its sign as in `Literal` (LANG-13).
    Range {
        start: Literal,
        end: Literal,
    },
    Tuple(Vec<PatternId>),
    Constructor {
        path: Path,
        fields: Vec<PatternField>,
        rest: bool,
    },
    /// Alternatives `P | Q`, two or more, none itself an `Or`; each binds the
    /// same names (LANG-13).
    Or(Vec<PatternId>),
    Error,
}
/// Constructor pattern field; Some(label) for records, None for positional variants (LANG-12/13).
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq)]
pub struct PatternField {
    pub label: Option<Symbol>,
    pub pattern: PatternId,
}
/// Written type syntax is separate from expressions, including generic brackets (LANG-2).
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq)]
pub struct TypeExpr {
    pub origin: Origin,
    pub kind: TypeExprKind,
}
/// Numeric generic arguments are size literals; name arguments resolve as type or size (GEN-1).
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq)]
pub enum TypeArgument {
    Type(TypeExprId),
    SizeLiteral(u64),
}
/// Prefix value modifiers (LANG-7, MODE-2/3/4). `sendable` is contextual.
#[derive(
    serde::Serialize, serde::Deserialize, Clone, Copy, Debug, Eq, PartialEq, Hash, Ord, PartialOrd,
)]
pub enum TypeModifier {
    Unique,
    Once,
    Sendable,
}
/// One place a relation names (MODE-1, PROV-4; #77).
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq)]
pub enum RelationPlace {
    /// A parameter, binding or arena, optionally with a field path (`p.a`).
    Path(Path),
    /// `heap`, the empty relation; valid only alone.
    Heap,
    /// The contextual `env`: the environment of a closure or function type.
    Environment,
}
/// `in x` or `in {x, a}`. Formatting prints braces exactly when there is more than one place.
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq)]
pub struct RelationSyntax {
    pub places: Vec<RelationPlace>,
    pub origin: Origin,
}
/// Sibling-field origin is distinct from an ordinary path (PROV-5).
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq)]
pub struct BrandSyntax {
    pub sibling: bool,
    pub path: Path,
}
/// Prefix modifiers cover the whole type; a brand and relation bind to the nearest type (LANG-7, R5).
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq)]
pub enum TypeExprKind {
    Named {
        path: Path,
        arguments: Vec<TypeArgument>,
    },
    Tuple(Vec<TypeExprId>),
    Function(FunctionSyntax),
    Box(TypeExprId),
    Qualified {
        base: TypeExprId,
        /// Source order; duplicates are kept so the checker reports LANG-7.
        modifiers: Vec<TypeModifier>,
        brand: Option<BrandSyntax>,
        relation: Option<RelationSyntax>,
    },
    Error,
}
/// Structural bound names are contextual identifiers (GEN-1).
#[derive(
    serde::Serialize, serde::Deserialize, Clone, Copy, Debug, Eq, PartialEq, Hash, Ord, PartialOrd,
)]
pub enum Bound {
    Plain,
    Flat,
    Data,
    Key,
    Sendable,
    Inert,
    Copyable,
    NoallocDrop,
    /// Explicit canonical data-schema qualification (SER-1); not a value mode.
    Serializable,
}
/// Generic parameter kind with equality-only sizes (LANG-7, GEN-1).
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq)]
pub enum GenericKind {
    Type(Vec<Bound>),
    Size,
    /// `H: implementation Sig where H.X = A.Y, ..` (GEN-6): an implementation
    /// binder. Sharing equations are written here, never inferred from bodies.
    Implementation {
        signature: Path,
        sharing: Vec<Sharing>,
    },
}
/// `H.member = Other.member` on an implementation binder (GEN-6): this
/// binder's type or size member is the earlier binder's.
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq)]
pub struct Sharing {
    pub member: Symbol,
    pub binder: Symbol,
    pub other: Symbol,
    pub origin: Origin,
}
/// One member specification of a `signature` (GEN-6).
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq)]
pub enum MemberSpec {
    /// `type X: bounds uses {}`: an abstract type with its required bounds.
    Type {
        name: Symbol,
        bounds: Vec<Bound>,
        authority: Option<Vec<Path>>,
        origin: Origin,
    },
    /// `size n`.
    Size { name: Symbol, origin: Origin },
    /// `fn op(..) -> R uses {..}`: a complete operation header.
    Operation {
        name: Symbol,
        signature: FunctionSyntax,
        origin: Origin,
    },
}
impl MemberSpec {
    pub fn name(&self) -> Symbol {
        match self {
            Self::Type { name, .. } | Self::Size { name, .. } | Self::Operation { name, .. } => {
                *name
            }
        }
    }
    pub fn origin(&self) -> Origin {
        match self {
            Self::Type { origin, .. }
            | Self::Size { origin, .. }
            | Self::Operation { origin, .. } => *origin,
        }
    }
}
/// One member mapping of an `implementation` (GEN-6).
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq)]
pub enum MemberDefinition {
    /// `type X = T`.
    Type {
        name: Symbol,
        ty: TypeExprId,
        origin: Origin,
    },
    /// `size n = 32`.
    Size {
        name: Symbol,
        value: u64,
        origin: Origin,
    },
    /// `op = path`: the exact function the member names.
    Operation {
        name: Symbol,
        target: Path,
        origin: Origin,
    },
}
impl MemberDefinition {
    pub fn name(&self) -> Symbol {
        match self {
            Self::Type { name, .. } | Self::Size { name, .. } | Self::Operation { name, .. } => {
                *name
            }
        }
    }
    pub fn origin(&self) -> Origin {
        match self {
            Self::Type { origin, .. }
            | Self::Size { origin, .. }
            | Self::Operation { origin, .. } => *origin,
        }
    }
}
/// Generic binders are in scope after declaration (LANG-2).
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq)]
pub struct GenericParam {
    pub name: Symbol,
    pub kind: GenericKind,
}
/// Anonymous names are allowed only in function-type slots (CLO-1; #24).
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq)]
pub struct Parameter {
    pub label: Option<Symbol>,
    pub name: Option<Symbol>,
    pub passing: PassingMode,
    pub ty: TypeExprId,
}
/// Attribute payloads are lexical values, not arbitrary expressions (MEM-7, MODE-6; #24).
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq)]
pub enum AttributeArgument {
    Name(Symbol),
    LoopSource(LoopSourceSyntax),
    Format(FormatAssociation),
    SerializableCheck(CheckAssociation),
    StatePromise(StatePromiseSyntax),
    Keyword(Keyword),
    Integer(String),
    String(String),
}
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq)]
pub struct LoopSourceSyntax {
    pub open: Path,
    pub next: Path,
    pub origin: Origin,
}
/// `@format(function)`: the function an f-string hole of this type calls (SEM-3).
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq)]
pub struct FormatAssociation {
    pub function: Path,
    pub origin: Origin,
}
/// `@serializable(check = function)`: the function that admits a decoded opaque
/// representation (SER-1).
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq)]
pub struct CheckAssociation {
    pub function: Path,
    pub origin: Origin,
}
#[derive(serde::Serialize, serde::Deserialize, Clone, Copy, Debug, Eq, PartialEq)]
pub enum StatePromiseKind {
    AdvanceOnly,
    PreserveCaptures,
    PreserveUnique,
    Captures,
}
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq)]
pub struct StatePromiseSyntax {
    pub kind: StatePromiseKind,
    pub selectors: Vec<Path>,
    pub origin: Origin,
}
/// Supported attributes are grammar-defined and retain source origin (MEM-7).
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq)]
pub struct Attribute {
    pub name: Symbol,
    pub arguments: Vec<AttributeArgument>,
    pub origin: Origin,
}
/// Contract kind; before() snapshots are expressions (FACT-1).
#[derive(serde::Serialize, serde::Deserialize, Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum ContractKind {
    Requires,
    Ensures,
}
/// Executable public signature expression, never inferred from the body (FACT-1, LANG-6).
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq)]
pub struct ContractSyntax {
    pub kind: ContractKind,
    pub expression: ExprId,
}
/// None uses is distinct from explicitly empty uses (CLO-2/4, CAP-3).
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq, Default)]
pub struct FunctionSyntax {
    pub attributes: Vec<Attribute>,
    pub generics: Vec<GenericParam>,
    pub parameters: Vec<Parameter>,
    pub result: Option<TypeExprId>,
    pub uses: Option<Vec<Path>>,
    pub contracts: Vec<ContractSyntax>,
}
/// Private/public/opaque visibility is package-based (PROG-2).
#[derive(serde::Serialize, serde::Deserialize, Clone, Copy, Debug, Eq, PartialEq)]
pub enum Visibility {
    Private,
    Public,
    PublicOpaque,
}
/// Type storage/ownership declaration (MEM-12, DROP-2, FACT-2).
#[derive(serde::Serialize, serde::Deserialize, Clone, Copy, Debug, Eq, PartialEq)]
pub enum TypeFlavor {
    Ordinary,
    Distinct,
    Flat,
    Resource,
}
/// Named record field (LANG-12, PROV-5).
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq)]
pub struct FieldSyntax {
    /// Field layout marker, never a value mode (MEM-12).
    pub growable: bool,
    pub name: Symbol,
    pub ty: TypeExprId,
    pub origin: Origin,
}
/// Variant declaration; fields may be positional or all labelled (LANG-7/12).
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq)]
pub struct VariantSyntax {
    pub name: Symbol,
    pub fields: Vec<(Option<Symbol>, TypeExprId)>,
}
/// Structural type declaration, including opaque external types (PROG-2; #24).
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq)]
pub enum TypeBody {
    Record(Vec<FieldSyntax>),
    Variants(Vec<VariantSyntax>),
    Alias(TypeExprId),
    Distinct {
        field: FieldSyntax,
        fact: Option<ExprId>,
    },
    Opaque,
}
/// Resource finalizer belongs to the declaring type (DROP-2/3).
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq)]
pub struct DropSyntax {
    /// Written promise covering finalizer and implicit field cleanup (DROP-2).
    pub noalloc: bool,
    pub binding: Symbol,
    pub body: ExprId,
}
/// An `extern` function's ABI string and the doc comment attached to the
/// declaration, which states its aliasing contract (PROG-4, MEM-9).
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq)]
pub struct ForeignAbi {
    /// The ABI string literal as written, quotes included.
    pub literal: String,
    /// The attached doc comment's text without its delimiters, or None.
    pub contract: Option<String>,
}
impl ForeignAbi {
    /// The ABI name inside the literal's quotes.
    pub fn name(&self) -> &str {
        self.literal
            .strip_prefix('"')
            .and_then(|s| s.strip_suffix('"'))
            .unwrap_or(&self.literal)
    }
}
/// Top-level declaration payload (LANG-6, PROG-4, CT-1).
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq)]
pub enum ItemKind {
    Function {
        signature: FunctionSyntax,
        body: Option<ExprId>,
        abi: Option<ForeignAbi>,
    },
    Const {
        ty: TypeExprId,
        value: ExprId,
    },
    Type {
        flavor: TypeFlavor,
        generics: Vec<GenericParam>,
        bounds: Option<Vec<Bound>>,
        authority: Option<Vec<Path>>,
        body: TypeBody,
        drop: Option<DropSyntax>,
    },
    /// `signature Name { members }` (GEN-6).
    Signature {
        members: Vec<MemberSpec>,
    },
    /// `implementation name: Sig { members }` (GEN-6).
    Implementation {
        signature: Path,
        members: Vec<MemberDefinition>,
    },
}
/// Where an analysis annotation attaches within its item (PS-17). Indexes
/// follow declaration order: a function definition's parameters, a record's
/// fields (a distinct type's field is field 0), a variant's payloads.
#[derive(
    serde::Serialize, serde::Deserialize, Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash,
)]
pub enum AnalysisSite {
    Item,
    Parameter(u32),
    Field(u32),
    Payload { variant: u32, field: u32 },
}
/// `@analysis(name: payload)` (PS-17): a fact owned by an out-of-process
/// analysis. The compiler parses, formats and exports it and reads it for
/// nothing else, so it lives beside its item's tree, never inside a declaration
/// node (PS-19).
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq)]
pub struct AnalysisAnnotation {
    pub site: AnalysisSite,
    /// The analysis's dotted name, such as `taint` or `acme.units`.
    pub name: String,
    /// The payload's token spellings, without trivia.
    pub payload: Vec<String>,
    /// Whether trivia precedes each payload token, for the formatter.
    pub spaced: Vec<bool>,
    pub origin: Origin,
}
/// A top-level declaration; identity is a manifest-qualified path (PROG-5).
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq)]
pub struct Item {
    /// Exact declaration name, including for duplicate diagnostics (LANG-7).
    pub name_span: Span,
    pub origin: Origin,
    pub name: Symbol,
    pub definition: DefPath,
    pub visibility: Visibility,
    pub attributes: Vec<Attribute>,
    pub kind: ItemKind,
    /// The item's role in typed generation (CT-5), if any.
    pub generated: Option<Generated>,
}
/// Items the compiler builds for a `= comptime e` declaration (CT-5).
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq)]
pub enum Generated {
    /// The declaration with its generated body: the function literal's
    /// parameter names (None for `_`) and their sites, which bind the declared
    /// parameters by position in the body. The header and its contracts keep
    /// the declared names.
    Body(Vec<(Option<Symbol>, Span)>),
    /// The generator `e`, checked as a constant: the declared parameter names.
    /// They are run-time values of the generated function, which `e` cannot read.
    Generator(Vec<Symbol>),
    /// A specialization of a declaration with implementation binders (GEN-6):
    /// each bound binder's name and its terminal implementation. The item is
    /// the declaration's own syntax; inside it a bound binder's members name
    /// the implementation's exact targets.
    Specialization(Vec<(Symbol, DefPath)>),
}
/// Written repeat literal, represented by a typed count binding and a one-element
/// sequence. Lowering repeats that element's prepared value, never its evaluation.
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq)]
pub struct RepeatLiteral {
    pub expression: ExprId,
    pub sequence: ExprId,
    pub value: ExprId,
    pub count: ExprId,
    pub evaluated_count: ExprId,
}
/// One top-level item's owning AST arenas and token tape. All IDs address these
/// Vecs, so they are item-local; no AST node borrows source storage (LANG-1).
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Default, Eq, PartialEq)]
pub struct SyntaxTree {
    /// The tokens the item owns, leading trivia and trailing same-line comment
    /// included, with spans relative to the item's anchor.
    pub tokens: Vec<Token>,
    pub expressions: Vec<Expr>,
    pub statements: Vec<Stmt>,
    pub patterns: Vec<Pattern>,
    pub types: Vec<TypeExpr>,
    pub formats: Vec<FormatString>,
    pub repeats: Vec<RepeatLiteral>,
    pub conditions: Vec<LetCondition>,
    pub groups: Vec<Group>,
    /// Analysis annotations in source order (PS-17).
    pub analyses: Vec<AnalysisAnnotation>,
}
/// The most ways one pattern may match through its or-patterns (LANG-13): the
/// checker rejects more, so every later phase may enumerate them.
pub const MAX_PATTERN_CHOICES: usize = 256;
/// One way a pattern matches: the alternative taken at each or-pattern it
/// reaches, keyed by the or-pattern.
pub type PatternChoice = std::collections::BTreeMap<PatternId, PatternId>;
impl SyntaxTree {
    /// The labelled block each `break 'l` under `root` leaves, keyed by the
    /// break (LANG-11). Labels resolve lexically; a closure or local function
    /// body starts a new label scope, and a break naming a loop or no enclosing
    /// label is absent.
    pub fn block_exits(&self, root: ExprId) -> std::collections::BTreeMap<ExprId, ExprId> {
        enum Step {
            Expr(ExprId),
            Stmt(StmtId),
            Leave,
            Restore(Vec<(Symbol, ExprId)>),
        }
        let mut exits = std::collections::BTreeMap::new();
        let mut active: Vec<(Symbol, ExprId)> = vec![];
        let mut work = vec![Step::Expr(root)];
        while let Some(step) = work.pop() {
            let id = match step {
                Step::Expr(id) => id,
                Step::Stmt(id) => {
                    match &self.statements[id.0 as usize].kind {
                        StmtKind::Let {
                            value, otherwise, ..
                        } => work.extend(otherwise.iter().chain([value]).map(|e| Step::Expr(*e))),
                        StmtKind::Assign { place, value, .. } => {
                            work.extend([Step::Expr(*value), Step::Expr(*place)])
                        }
                        StmtKind::Expr(value) => work.push(Step::Expr(*value)),
                        StmtKind::While {
                            condition, body, ..
                        } => work.extend([Step::Expr(*body), Step::Expr(*condition)]),
                        StmtKind::For { source, body, .. } => {
                            work.extend([Step::Expr(*body), Step::Expr(*source)])
                        }
                        StmtKind::Function { body, .. } => {
                            work.push(Step::Restore(std::mem::take(&mut active)));
                            work.push(Step::Expr(*body));
                        }
                    }
                    continue;
                }
                Step::Leave => {
                    active.pop();
                    continue;
                }
                Step::Restore(outer) => {
                    active = outer;
                    continue;
                }
            };
            let mut children = vec![];
            match &self.expressions[id.0 as usize].kind {
                ExprKind::Block {
                    label,
                    statements,
                    tail,
                } => {
                    if let Some(label) = label {
                        active.push((*label, id));
                        work.push(Step::Leave);
                    }
                    children.extend(statements.iter().map(|s| Step::Stmt(*s)));
                    children.extend(tail.map(Step::Expr));
                }
                ExprKind::Break { label, value } => {
                    if let Some(&(_, block)) = active.iter().rev().find(|(l, _)| Some(*l) == *label)
                    {
                        exits.insert(id, block);
                    }
                    children.extend(value.map(Step::Expr));
                }
                ExprKind::Closure { body, .. } => {
                    work.push(Step::Restore(std::mem::take(&mut active)));
                    children.push(Step::Expr(*body));
                }
                ExprKind::Tuple(values) | ExprKind::Sequence(values) => {
                    children.extend(values.iter().map(|e| Step::Expr(*e)))
                }
                ExprKind::Call { callee, arguments } => {
                    children.push(Step::Expr(*callee));
                    children.extend(arguments.iter().map(|a| Step::Expr(a.value)));
                }
                ExprKind::Field { base: value, .. }
                | ExprKind::With { callee: value, .. }
                | ExprKind::Unary { value, .. }
                | ExprKind::Try { value, .. }
                | ExprKind::Before(value)
                | ExprKind::Comptime(value)
                | ExprKind::Return(Some(value)) => children.push(Step::Expr(*value)),
                ExprKind::Index { base, index } => {
                    children.extend([Step::Expr(*base), Step::Expr(*index)])
                }
                ExprKind::Binary { left, right, .. } => {
                    children.extend([Step::Expr(*left), Step::Expr(*right)])
                }
                ExprKind::If {
                    condition,
                    then_branch,
                    else_branch,
                } => {
                    children.extend([Step::Expr(*condition), Step::Expr(*then_branch)]);
                    children.extend(else_branch.map(Step::Expr));
                }
                ExprKind::Match { value, arms } => {
                    children.push(Step::Expr(*value));
                    for arm in arms {
                        children.extend(arm.guard.map(Step::Expr));
                        children.push(Step::Expr(arm.body));
                    }
                }
                ExprKind::Literal(_)
                | ExprKind::Path(_)
                | ExprKind::Return(None)
                | ExprKind::Continue(_)
                | ExprKind::Shape(_)
                | ExprKind::Heap
                | ExprKind::Result
                | ExprKind::Todo(_)
                | ExprKind::Formatter(_)
                | ExprKind::Error => {}
            }
            work.extend(children.into_iter().rev());
        }
        exits
    }
    /// Whether an expression's last token closes a block, closure or match, so
    /// a following `else` would continue it (LANG-13). With `grouped`, a
    /// parenthesized expression ends in `)`; without, parentheses are ignored.
    pub fn ends_in_block(&self, mut e: ExprId, grouped: bool) -> bool {
        loop {
            // An f-string's desugared block ends in its closing quote.
            if grouped && self.groups.iter().any(|g| g.expression == e)
                || self.formats.iter().any(|f| f.expression == e)
            {
                return false;
            }
            match &self.expressions[e.0 as usize].kind {
                ExprKind::Block { .. }
                | ExprKind::If { .. }
                | ExprKind::Match { .. }
                | ExprKind::Closure { .. } => return true,
                ExprKind::Unary { value, .. }
                | ExprKind::Return(Some(value))
                | ExprKind::Break {
                    value: Some(value), ..
                } => e = *value,
                ExprKind::Binary { right, .. } => e = *right,
                _ => return false,
            }
        }
    }
    /// A pattern's direct subpatterns: tuple elements, constructor fields or
    /// alternatives.
    pub fn pattern_children(&self, id: PatternId) -> Vec<PatternId> {
        match &self.patterns[id.0 as usize].kind {
            PatternKind::Tuple(parts) | PatternKind::Or(parts) => parts.clone(),
            PatternKind::Constructor { fields, .. } => fields.iter().map(|f| f.pattern).collect(),
            _ => vec![],
        }
    }
    /// Every way `root` matches, alternatives in source order, left to right;
    /// one empty choice when it has no or-pattern. None past `MAX_PATTERN_CHOICES`.
    pub fn pattern_choices(&self, root: PatternId) -> Option<Vec<PatternChoice>> {
        let choices = match &self.patterns[root.0 as usize].kind {
            PatternKind::Or(alternatives) => {
                let mut all = vec![];
                for alternative in alternatives {
                    for mut choice in self.pattern_choices(*alternative)? {
                        choice.insert(root, *alternative);
                        all.push(choice);
                    }
                    if all.len() > MAX_PATTERN_CHOICES {
                        return None;
                    }
                }
                all
            }
            _ => {
                let mut all = vec![PatternChoice::new()];
                for child in self.pattern_children(root) {
                    let parts = self.pattern_choices(child)?;
                    if parts.len() > 1 {
                        if all.len() * parts.len() > MAX_PATTERN_CHOICES {
                            return None;
                        }
                        all = all
                            .iter()
                            .flat_map(|choice| {
                                parts.iter().map(move |part| {
                                    let mut c = choice.clone();
                                    c.extend(part.iter().map(|(k, v)| (*k, *v)));
                                    c
                                })
                            })
                            .collect();
                    }
                }
                all
            }
        };
        Some(choices)
    }
    /// The subpatterns one choice reaches from `root`, `root` first: an
    /// or-pattern contributes only its chosen alternative.
    pub fn chosen_patterns(&self, root: PatternId, choice: &PatternChoice) -> Vec<PatternId> {
        let mut out = vec![];
        let mut todo = vec![root];
        while let Some(id) = todo.pop() {
            out.push(id);
            match &self.patterns[id.0 as usize].kind {
                PatternKind::Or(alternatives) => {
                    todo.extend(choice.get(&id).or(alternatives.first()));
                }
                _ => todo.extend(self.pattern_children(id).into_iter().rev()),
            }
        }
        out
    }
}
/// One top-level item occurrence with its own arenas. Nothing in it depends on
/// where the item stands: its IDs are item-local and its spans are relative to
/// its anchor, so inserting other items or moving it between files keeps it.
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq)]
pub struct ItemSyntax {
    pub item: Item,
    pub tree: SyntaxTree,
    #[serde(with = "snapshot_anchor")]
    pub anchor: Anchor,
    /// Hashes the exact bytes of the item's owned tokens, including attributes,
    /// contracts and drop; no newline normalization.
    pub text_hash: ContentHash,
    /// Ordinal among this DefPath's candidates, in (file path, position) order.
    /// Parsing persists before package-wide occurrence placement.
    #[serde(skip)]
    pub occurrence: u32,
}
impl ItemSyntax {
    /// The owner of the lexical sites in this item.
    pub fn owner(&self) -> LexicalOwner {
        LexicalOwner {
            def: DefId::intern(&self.item.definition),
            occurrence: self.occurrence,
        }
    }
    /// Make every span relative to `anchor` instead of this item's own: a
    /// re-parsed item takes over its predecessor's anchor, so results that
    /// survive the edit keep naming the same place.
    pub fn reanchor(&mut self, anchor: Anchor) {
        let from = self.anchor;
        let span = &mut |s: &mut Span| {
            if s.anchor == from {
                s.anchor = anchor;
            }
        };
        fn attributes(attributes: &mut [Attribute], span: &mut impl FnMut(&mut Span)) {
            for attribute in attributes {
                span(&mut attribute.origin.span);
                for argument in &mut attribute.arguments {
                    match argument {
                        AttributeArgument::LoopSource(s) => span(&mut s.origin.span),
                        AttributeArgument::Format(s) => span(&mut s.origin.span),
                        AttributeArgument::StatePromise(s) => span(&mut s.origin.span),
                        _ => (),
                    }
                }
            }
        }
        let tree = &mut self.tree;
        for token in &mut tree.tokens {
            span(&mut token.span);
        }
        for e in &mut tree.expressions {
            span(&mut e.origin.span);
            match &mut e.kind {
                ExprKind::Closure { signature, .. } => attributes(&mut signature.attributes, span),
                ExprKind::With { bindings, .. } => {
                    for binding in bindings {
                        span(&mut binding.origin.span);
                    }
                }
                _ => (),
            }
        }
        for s in &mut tree.statements {
            span(&mut s.origin.span);
            if let StmtKind::Function { signature, .. } = &mut s.kind {
                attributes(&mut signature.attributes, span);
            }
        }
        for p in &mut tree.patterns {
            span(&mut p.origin.span);
        }
        for t in &mut tree.types {
            span(&mut t.origin.span);
            match &mut t.kind {
                TypeExprKind::Function(signature) => attributes(&mut signature.attributes, span),
                TypeExprKind::Qualified {
                    relation: Some(relation),
                    ..
                } => span(&mut relation.origin.span),
                _ => (),
            }
        }
        for g in &mut tree.groups {
            span(&mut g.origin.span);
        }
        let item = &mut self.item;
        span(&mut item.name_span);
        span(&mut item.origin.span);
        attributes(&mut item.attributes, span);
        fn sharing(signature: &mut FunctionSyntax, span: &mut impl FnMut(&mut Span)) {
            for generic in &mut signature.generics {
                if let GenericKind::Implementation { sharing, .. } = &mut generic.kind {
                    for equation in sharing {
                        span(&mut equation.origin.span);
                    }
                }
            }
        }
        match &mut item.kind {
            ItemKind::Function { signature, .. } => {
                attributes(&mut signature.attributes, span);
                sharing(signature, span);
            }
            ItemKind::Signature { members } => {
                for member in members {
                    match member {
                        MemberSpec::Type { origin, .. } | MemberSpec::Size { origin, .. } => {
                            span(&mut origin.span)
                        }
                        MemberSpec::Operation {
                            origin, signature, ..
                        } => {
                            span(&mut origin.span);
                            attributes(&mut signature.attributes, span);
                        }
                    }
                }
            }
            ItemKind::Implementation { members, .. } => {
                for member in members {
                    match member {
                        MemberDefinition::Type { origin, .. }
                        | MemberDefinition::Size { origin, .. }
                        | MemberDefinition::Operation { origin, .. } => span(&mut origin.span),
                    }
                }
            }
            ItemKind::Type { body, .. } => match body {
                TypeBody::Record(fields) => {
                    for field in fields {
                        span(&mut field.origin.span);
                    }
                }
                TypeBody::Distinct { field, .. } => span(&mut field.origin.span),
                _ => (),
            },
            ItemKind::Const { .. } => (),
        }
        if let Some(Generated::Body(names)) = &mut item.generated {
            for (_, site) in names {
                span(site);
            }
        }
        self.anchor = anchor;
    }
}
/// The definition occurrence whose item-local syntax a lexical site belongs to.
/// Persisted as (DefPath, occurrence).
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Ord, PartialOrd)]
pub struct LexicalOwner {
    pub def: DefId,
    pub occurrence: u32,
}
impl serde::Serialize for LexicalOwner {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        serde::Serialize::serialize(&(self.def, self.occurrence), s)
    }
}
impl<'de> serde::Deserialize<'de> for LexicalOwner {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let (def, occurrence): (DefId, u32) = serde::Deserialize::deserialize(d)?;
        Ok(Self { def, occurrence })
    }
}
/// A lexical site inside one item's arenas. `Item` is the item itself.
#[derive(
    serde::Serialize, serde::Deserialize, Clone, Copy, Debug, Eq, PartialEq, Hash, Ord, PartialOrd,
)]
pub enum NameSite {
    Expression(ExprId),
    Pattern(PatternId),
    Type(TypeExprId),
    Statement(StmtId),
    Item,
}
/// A lexical site qualified by its owner: how type rows and anything crossing a
/// definition boundary name a site.
#[derive(
    serde::Serialize, serde::Deserialize, Clone, Copy, Debug, Eq, PartialEq, Hash, Ord, PartialOrd,
)]
pub struct SiteRef {
    pub owner: LexicalOwner,
    pub site: NameSite,
}
impl LexicalOwner {
    /// The first, and unless LANG-7 rejects duplicates the only, occurrence of `path`.
    pub fn first(path: &DefPath) -> Self {
        Self {
            def: DefId::intern(path),
            occurrence: 0,
        }
    }
    pub const fn site(self, site: NameSite) -> SiteRef {
        SiteRef { owner: self, site }
    }
}
/// Parser output retains invalid tokens and orphan/EOF trivia for round-trip formatting (LANG-1).
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq)]
pub struct SyntaxFile {
    /// Includes recovery diagnostics which have no Error node.
    pub has_syntax_errors: bool,
    #[serde(skip)]
    pub file: FileId,
    /// Top-level items in source order; their owned tokens tile the tape's start.
    pub items: Vec<Arc<ItemSyntax>>,
    /// The tokens after the last item's: trailing trivia and EOF, file-anchored.
    pub tokens: Vec<Token>,
}
impl SyntaxFile {
    /// The complete token tape, in source order: every source byte once (LANG-1).
    pub fn tape(&self) -> impl Iterator<Item = &Token> {
        self.items
            .iter()
            .flat_map(|item| &item.tree.tokens)
            .chain(&self.tokens)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    pub fn path(name: &str) -> DefPath {
        DefPath(ItemPath {
            package: PackageId {
                name: "sample".into(),
                version: "1".into(),
            },
            modules: vec![],
            name: name.into(),
        })
    }
    #[test]
    fn core_identity_and_display_preserve_package_versions_and_module_boundaries() {
        for (name, spelling) in [
            ("Option", "core@1/Option"),
            ("list.push", "core@1/list/push"),
            ("a.b.c", "core@1/a/b/c"),
        ] {
            let path = DefPath::core(name);
            assert!(path.is_core(name));
            assert_eq!(path.to_string(), spelling);
            assert!(path.package_is("core", "1"));
            assert!(!path.is_core("other"));
        }
        let mut path = DefPath::core("list.push");
        path.0.package.version = "2".into();
        assert!(path.0.package.name_is("core"));
        assert!(!path.package_is("core", "1"));
        assert!(!path.is_core("list.push"));
        path.0.package.name = "sample".into();
        assert!(!path.0.package.name_is("core"));
        assert_eq!(path.to_string(), "sample@2/list/push");
        path.0.modules.clear();
        path.0.name = "list.push".into();
        path.0.package.name = "core".into();
        path.0.package.version = "1".into();
        assert!(!path.is_core("list.push"));
    }
    #[test]
    fn hand_built_unit_function_has_no_pointer_edges() {
        let origin = Origin {
            span: Span::default(),
            tokens: TokenRange::default(),
        };
        let mut tree = SyntaxTree::default();
        tree.expressions.push(Expr {
            origin,
            kind: ExprKind::Tuple(vec![]),
        });
        tree.expressions.push(Expr {
            origin,
            kind: ExprKind::Block {
                label: None,
                statements: vec![],
                tail: Some(ExprId(0)),
            },
        });
        let item = Item {
            name_span: Default::default(),
            origin,
            name: Symbol::intern("main"),
            definition: path("main"),
            visibility: Visibility::Public,
            attributes: vec![],
            kind: ItemKind::Function {
                signature: FunctionSyntax::default(),
                body: Some(ExprId(1)),
                abi: None,
            },
            generated: None,
        };
        assert!(matches!(
            item.kind,
            ItemKind::Function {
                body: Some(ExprId(1)),
                ..
            }
        ));
        assert_eq!(tree.expressions.len(), 2);
        assert_ne!(path("a").fingerprint(), path("b").fingerprint());
        assert_ne!(
            ContentHash::of_parts("test", [b"ab".as_slice(), b"c"]),
            ContentHash::of_parts("test", [b"a".as_slice(), b"bc"])
        );
    }
}

/// Definition manifest assigns module identity independently of file layout (PROG-1/5; issue #4).
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq, Hash)]
pub struct FileContext {
    pub package: PackageId,
    pub modules: Vec<String>,
}

#[cfg(test)]
mod revision_tests {
    use super::*;
}

// Item ownership anchors use exactly the same portable numbering as spans.
mod snapshot_anchor {
    use super::*;
    use serde::{Deserialize, Serialize};
    pub fn serialize<S: serde::Serializer>(
        anchor: &Anchor,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        Span {
            anchor: *anchor,
            start: 0,
            end: 0,
        }
        .serialize(serializer)
    }
    pub fn deserialize<'de, D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Anchor, D::Error> {
        let span = Span::deserialize(deserializer)?;
        if span.start != 0 || span.end != 0 {
            return Err(serde::de::Error::custom("invalid ownership anchor"));
        }
        Ok(span.anchor)
    }
}
mod structure;

//! Indexed AST plus a lossless token/trivia tape (LANG-1, PROG-5).
//! See docs/spec/syntax.md for syntax and README.md for ownership of comments.
use hirsel_intern::Symbol;
use hirsel_span::{FileId, Span};
use sha2::{Digest, Sha256};
mod origins;

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
ids!(TokenId, ItemId, ExprId, PatternId, TypeExprId, StmtId);
/// Half-open indices into a file's complete token tape (LANG-1).
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct TokenRange {
    pub start: u32,
    pub end: u32,
}
/// Source origin shared by AST nodes; spans exclude surrounding trivia (LANG-1).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Origin {
    pub span: Span,
    pub tokens: TokenRange,
}
/// Reserved words, exactly LANG-14; other special names are contextual.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
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
/// Longest-match punctuation, independent of type information (LANG-1/2; issue #21).
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
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
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TriviaKind {
    Space,
    Newline,
    LineComment,
    BlockComment,
    DocLine,
    DocBlock,
}
/// Tokens carry literal source spelling through their spans (SEM-1/2/3).
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TokenKind {
    Ident(Symbol),
    Keyword(Keyword),
    Punct(Punct),
    Integer,
    Float,
    Character,
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
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum FormatPiece {
    /// A colon and its raw format specification, ending before `}`.
    Spec,
    Whole,
    Head,
    Middle,
    Tail,
}
/// Every source byte belongs to exactly one non-EOF token, even on errors (LANG-1).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Token {
    pub kind: TokenKind,
    pub span: Span,
}
/// Unresolved source path. Dot projections and module qualification share syntax (LANG-4).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Path {
    pub package: Option<PackageId>,
    pub segments: Vec<Symbol>,
}
/// Parameter passing mode; copy/unique are expressions, not parameter conventions (OWN-1/2/3).
#[derive(serde::Serialize, serde::Deserialize, Clone, Copy, Debug, Eq, PartialEq)]
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
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AssignOp(pub Option<BinaryOp>);
/// Literal spelling is preserved independently of type inference and numeric conversion (LANG-2).
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Literal {
    Integer(String),
    Float(String),
    Character(String),
    String(String),
    Bytes(String),
    Bool(bool),
}
/// Decode a single-quoted Unicode scalar spelling, retaining no source state.
/// Invalid delimiters, escapes and scalar counts return None.
pub fn character_value(spelling: &str) -> Option<char> {
    let body = spelling.strip_prefix('\'')?.strip_suffix('\'')?;
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
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Argument {
    pub label: Option<Symbol>,
    pub value: ExprId,
}
/// Match arm; refutability/exhaustiveness belongs to checking (LANG-7/13).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MatchArm {
    pub pattern: PatternId,
    pub guard: Option<ExprId>,
    pub body: ExprId,
}
/// Indexed expression; references only nodes in its owning SyntaxTree (LANG-1/3).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Expr {
    pub origin: Origin,
    pub kind: ExprKind,
}
/// All surface expressions; scope/task/arena operations are ordinary calls (CONC-2, MEM-4).
#[derive(Clone, Debug, Eq, PartialEq)]
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
    Block {
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
    Break,
    Continue,
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
    Error,
}
/// `Append(value, mut builder)` appends a hole or text piece; `Finish(take builder)`
/// returns the built `String`.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum FormatStep {
    Append,
    AppendSpec(FormatSpec),
    Finish,
}
/// Constant formatting options. Alignment: 0 default, 1 left, 2 right, 3 center.
/// Flags: 1 plus, 2 alternate, 4 sign-aware zero padding, 8 precision present.
/// Kind: 0 display or the ASCII radix letter.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
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
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FormatString {
    pub expression: ExprId,
    /// Piece spellings, one more than `holes`: `f"a{`, `}b"`, or a single `f"ab"`.
    pub pieces: Vec<String>,
    pub holes: Vec<ExprId>,
    /// One spelling per hole, empty for a plain hole, otherwise starting with `:`.
    pub specs: Vec<String>,
}
/// Statement origin includes its terminator; the block tail does not (LANG-11).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Stmt {
    pub origin: Origin,
    pub kind: StmtKind,
}
/// Local definitions are ordered and cannot recurse (LANG-10/15).
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum StmtKind {
    Let {
        mutable: bool,
        pattern: PatternId,
        ty: Option<TypeExprId>,
        value: ExprId,
    },
    Assign {
        place: ExprId,
        op: AssignOp,
        value: ExprId,
    },
    Expr(ExprId),
    While {
        condition: ExprId,
        body: ExprId,
    },
    For {
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
/// Pattern tree node, with constructor names resolved using the expected type (LANG-2/13).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Pattern {
    pub origin: Origin,
    pub kind: PatternKind,
}
/// Record '..' ignores remaining fields; no expression ellipsis is permitted (OWN-8; #25).
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PatternKind {
    Wildcard,
    Binding(Symbol),
    Literal(Literal),
    Tuple(Vec<PatternId>),
    Constructor {
        path: Path,
        fields: Vec<PatternField>,
        rest: bool,
    },
    Error,
}
/// Constructor pattern field; Some(label) for records, None for positional variants (LANG-12/13).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PatternField {
    pub label: Option<Symbol>,
    pub pattern: PatternId,
}
/// Written type syntax is separate from expressions, including generic brackets (LANG-2).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TypeExpr {
    pub origin: Origin,
    pub kind: TypeExprKind,
}
/// Numeric generic arguments are size literals; name arguments resolve as type or size (GEN-1).
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TypeArgument {
    Type(TypeExprId),
    SizeLiteral(u64),
}
/// Prefix value modifiers (LANG-7, MODE-2/3/4). `sendable` is contextual.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Ord, PartialOrd)]
pub enum TypeModifier {
    Unique,
    Once,
    Sendable,
}
/// One place a relation names (MODE-1, PROV-4; #77).
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RelationPlace {
    /// A parameter, binding or arena, optionally with a field path (`p.a`).
    Path(Path),
    /// `heap`, the empty relation; valid only alone.
    Heap,
    /// The contextual `env`: the environment of a closure or function type.
    Environment,
}
/// `in x` or `in {x, a}`. Formatting prints braces exactly when there is more than one place.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RelationSyntax {
    pub places: Vec<RelationPlace>,
    pub origin: Origin,
}
/// Sibling-field origin is distinct from an ordinary path (PROV-5).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BrandSyntax {
    pub sibling: bool,
    pub path: Path,
}
/// Prefix modifiers cover the whole type; a brand and relation bind to the nearest type (LANG-7, R5).
#[derive(Clone, Debug, Eq, PartialEq)]
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
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum GenericKind {
    Type(Vec<Bound>),
    Size,
}
/// Generic binders are in scope after declaration (LANG-2).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GenericParam {
    pub name: Symbol,
    pub kind: GenericKind,
}
/// Anonymous names are allowed only in function-type slots (CLO-1; #24).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Parameter {
    pub label: Option<Symbol>,
    pub name: Option<Symbol>,
    pub passing: PassingMode,
    pub ty: TypeExprId,
}
/// Attribute payloads are lexical values, not arbitrary expressions (MEM-7, MODE-6; #24).
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AttributeArgument {
    Name(Symbol),
    LoopSource(LoopSourceSyntax),
    StatePromise(StatePromiseSyntax),
    Keyword(Keyword),
    Integer(String),
    String(String),
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LoopSourceSyntax {
    pub open: Path,
    pub next: Path,
    pub origin: Origin,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StatePromiseKind {
    AdvanceOnly,
    PreserveCaptures,
    PreserveUnique,
    Captures,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StatePromiseSyntax {
    pub kind: StatePromiseKind,
    pub selectors: Vec<Path>,
    pub origin: Origin,
}
/// Supported attributes are grammar-defined and retain source origin (MEM-7).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Attribute {
    pub name: Symbol,
    pub arguments: Vec<AttributeArgument>,
    pub origin: Origin,
}
/// Contract kind; before() snapshots are expressions (FACT-1).
#[derive(serde::Serialize, serde::Deserialize, Clone, Copy, Debug, Eq, PartialEq)]
pub enum ContractKind {
    Requires,
    Ensures,
}
/// Executable public signature expression, never inferred from the body (FACT-1, LANG-6).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ContractSyntax {
    pub kind: ContractKind,
    pub expression: ExprId,
}
/// None uses is distinct from explicitly empty uses (CLO-2/4, CAP-3).
#[derive(Clone, Debug, Eq, PartialEq, Default)]
pub struct FunctionSyntax {
    pub attributes: Vec<Attribute>,
    pub generics: Vec<GenericParam>,
    pub parameters: Vec<Parameter>,
    pub result: Option<TypeExprId>,
    pub uses: Option<Vec<Path>>,
    pub contracts: Vec<ContractSyntax>,
}
/// Private/public/opaque visibility is package-based (PROG-2).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Visibility {
    Private,
    Public,
    PublicOpaque,
}
/// Type storage/ownership declaration (MEM-12, DROP-2, FACT-2).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TypeFlavor {
    Ordinary,
    Distinct,
    Flat,
    Resource,
}
/// Named record field (LANG-12, PROV-5).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FieldSyntax {
    /// Field layout marker, never a value mode (MEM-12).
    pub growable: bool,
    pub name: Symbol,
    pub ty: TypeExprId,
    pub origin: Origin,
}
/// Variant declaration; fields may be positional or all labelled (LANG-7/12).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VariantSyntax {
    pub name: Symbol,
    pub fields: Vec<(Option<Symbol>, TypeExprId)>,
}
/// Structural type declaration, including opaque external types (PROG-2; #24).
#[derive(Clone, Debug, Eq, PartialEq)]
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
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DropSyntax {
    /// Written promise covering finalizer and implicit field cleanup (DROP-2).
    pub noalloc: bool,
    pub binding: Symbol,
    pub body: ExprId,
}
/// An `extern` function's ABI string and the doc comment attached to the
/// declaration, which states its aliasing contract (PROG-4, MEM-9).
#[derive(Clone, Debug, Eq, PartialEq)]
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
#[derive(Clone, Debug, Eq, PartialEq)]
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
}
/// Item text hash covers owned trivia and exact text; identity is a manifest-qualified path (PROG-5).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Item {
    /// Exact declaration name, including for duplicate diagnostics (LANG-7).
    pub name_span: Span,
    pub origin: Origin,
    pub owned_tokens: TokenRange,
    pub name: Symbol,
    pub definition: DefPath,
    pub text_hash: ContentHash,
    pub visibility: Visibility,
    pub attributes: Vec<Attribute>,
    pub kind: ItemKind,
}
/// Owning AST arenas. All IDs address these Vecs; no AST node borrows source storage (LANG-1).
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SyntaxTree {
    pub items: Vec<Item>,
    pub expressions: Vec<Expr>,
    pub statements: Vec<Stmt>,
    pub patterns: Vec<Pattern>,
    pub types: Vec<TypeExpr>,
    pub formats: Vec<FormatString>,
}
/// Parser output retains invalid tokens and orphan/EOF trivia for round-trip formatting (LANG-1).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SyntaxFile {
    /// Owning symbol snapshot; all AST symbols refer here.
    pub symbols: std::sync::Arc<hirsel_intern::Interner>,
    /// Includes recovery diagnostics which have no Error node.
    pub has_syntax_errors: bool,
    pub file: FileId,
    pub tokens: Vec<Token>,
    pub tree: SyntaxTree,
    pub roots: Vec<ItemId>,
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
                statements: vec![],
                tail: Some(ExprId(0)),
            },
        });
        tree.items.push(Item {
            name_span: Default::default(),

            origin,
            owned_tokens: TokenRange::default(),
            name: Symbol(0),
            definition: path("main"),
            text_hash: ContentHash::of_bytes(b"fn main() { () }"),
            visibility: Visibility::Public,
            attributes: vec![],
            kind: ItemKind::Function {
                signature: FunctionSyntax::default(),
                body: Some(ExprId(1)),
                abi: None,
            },
        });
        assert_eq!(tree.expressions.len(), 2);
        assert_ne!(path("a").fingerprint(), path("b").fingerprint());
        assert_ne!(
            ContentHash::of_parts("test", [b"ab".as_slice(), b"c"]),
            ContentHash::of_parts("test", [b"a".as_slice(), b"bc"])
        );
    }
}

/// Definition manifest assigns module identity independently of file layout (PROG-1/5; issue #4).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FileContext {
    pub package: PackageId,
    pub modules: Vec<String>,
}
/// One item in an immutable parsed file snapshot (PROG-5).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DefinitionSource {
    pub definition: DefPath,
    pub file: FileId,
    pub item: ItemId,
    pub text_hash: ContentHash,
}

#[cfg(test)]
mod revision_tests {
    use super::*;
    #[test]
    fn v20_header_facts_and_parse_status_survive_snapshot_transport() {
        let mut symbols = hirsel_intern::Interner::default();
        let name = symbols.intern("Marker");
        let span = Span::new(FileId(0), 16, 22).unwrap();
        let mut item = Item {
            name_span: span,
            origin: Origin {
                span,
                tokens: TokenRange::default(),
            },
            owned_tokens: TokenRange::default(),
            name,
            definition: super::tests::path("Marker"),
            text_hash: ContentHash::default(),
            visibility: Visibility::PublicOpaque,
            attributes: vec![],
            kind: ItemKind::Type {
                flavor: TypeFlavor::Resource,
                generics: vec![],
                bounds: Some(vec![]),
                authority: Some(vec![]),
                body: TypeBody::Record(vec![FieldSyntax {
                    name,
                    ty: TypeExprId(0),
                    origin: Origin {
                        span,
                        tokens: TokenRange::default(),
                    },
                    growable: true,
                }]),
                drop: Some(DropSyntax {
                    binding: name,
                    body: ExprId(0),
                    noalloc: true,
                }),
            },
        };
        let stated = item.clone();
        if let ItemKind::Type { bounds, .. } = &mut item.kind {
            *bounds = None;
        }
        assert_ne!(item, stated);
        if let ItemKind::Type { bounds, .. } = &mut item.kind {
            *bounds = Some(vec![Bound::NoallocDrop]);
        }
        assert_ne!(item, stated);
        let file = SyntaxFile {
            symbols: std::sync::Arc::new(symbols.clone()),
            has_syntax_errors: true,
            file: FileId(0),
            tokens: vec![],
            roots: vec![ItemId(0)],
            tree: SyntaxTree {
                items: vec![stated],
                ..Default::default()
            },
        };
        symbols.intern("later");
        assert_eq!(file.symbols.len(), 1);
        assert_eq!(
            file.symbols.resolve(file.tree.items[0].name),
            Some("Marker")
        );
        assert_eq!(file.tree.items[0].name_span, span);
        assert!(file.has_syntax_errors);
        assert_eq!(file, file.clone());
    }
}

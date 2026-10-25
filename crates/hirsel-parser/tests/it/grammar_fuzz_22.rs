//! Bounded grammar recovery, adapted from rust-analyzer parser tests.rs:61-130,
//! tests/{top_entries.rs:1-47,prefix_entries.rs:1-89}, generic_args.rs:1-110
//! (3131751eaf1c3b5da6e618acdc2148ad7db65bc7), and Proptest recursive.rs:
//! 50-100,150-208 and tutorial/{recursive,shrinking-basics}.md
//! (70cf37d46b7d83393b6ed94c611f8c52209f585d). Hirsel has one file entry,
//! square generic brackets and lexer-owned interpolation; no Rust rules ported.
//!
//! Upstream test shapes are used under MIT (rust-analyzer contributors,
//! FullContact, Inc). Copyright (c) 2026 FullContact, Inc (Proptest).
//! Permission is hereby granted, free of charge, to any
//! person obtaining a copy of this software and associated documentation files
//! (the "Software"), to deal in the Software without restriction, including
//! without limitation the rights to use, copy, modify, merge, publish,
//! distribute, sublicense, and/or sell copies of the Software, and to permit
//! persons to whom the Software is furnished to do so, subject to the following
//! conditions: The above copyright notice and this permission notice shall be
//! included in all copies or substantial portions of the Software.
//! THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
//! IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
//! FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
//! AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
//! LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING
//! FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER
//! DEALINGS IN THE SOFTWARE.

use crate::support;
use hirsel_diag::Diagnostic;
use hirsel_span::{Anchor, SourceMap, Span};
use hirsel_syntax::*;
use proptest::{
    prelude::*,
    strategy::ValueTree,
    test_runner::{Config, RngSeed, TestRunner},
};
use std::{
    fs,
    path::{Path as FsPath, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant},
};

const SEED: u64 = 0x22_6772_616d_6d61;
const CASES: usize = 128;
const TOKENS: usize = 256; // Includes trivia and EOF, not just significant tokens.
const DEPTH: usize = 4; // Written delimiter nesting, including the function body.
const SHRINKS: usize = 256;

#[derive(Clone, Debug)]
enum Form {
    Leaf(bool),
    Group(Box<Form>),
    Sequence(Box<Form>, Box<Form>),
    Let(Box<Form>),
    Match(Box<Form>),
    Format(Box<Form>),
}
impl Form {
    fn render(&self) -> String {
        match self {
            Self::Leaf(false) => "qx".into(),
            Self::Leaf(true) => "1u32".into(),
            Self::Group(x) => format!("({})", x.render()),
            Self::Sequence(x, y) => format!("[{},{}]", x.render(), y.render()),
            Self::Let(x) => format!("{{let qv = {}; qv}}", x.render()),
            Self::Match(x) => format!("match qx {{ qv => {}, }}", x.render()),
            Self::Format(x) => format!("f\"é{{ {} }}\"", x.render()),
        }
    }
    fn depth(&self) -> usize {
        match self {
            Self::Leaf(_) => 0,
            Self::Sequence(x, y) => 1 + x.depth().max(y.depth()),
            Self::Group(x) | Self::Let(x) | Self::Match(x) | Self::Format(x) => 1 + x.depth(),
        }
    }
}
fn forms() -> impl Strategy<Value = Form> {
    any::<bool>()
        .prop_map(Form::Leaf)
        .prop_recursive(3, 24, 2, |x| {
            prop_oneof![
                x.clone().prop_map(|x| Form::Group(Box::new(x))),
                (x.clone(), x.clone()).prop_map(|(x, y)| Form::Sequence(Box::new(x), Box::new(y))),
                x.clone().prop_map(|x| Form::Let(Box::new(x))),
                x.clone().prop_map(|x| Form::Match(Box::new(x))),
                x.prop_map(|x| Form::Format(Box::new(x))),
            ]
        })
}
fn runner() -> TestRunner {
    TestRunner::new(Config {
        cases: CASES as u32,
        rng_seed: RngSeed::Fixed(SEED),
        failure_persistence: None, // Bytes and seed go outside the checkout below.
        max_shrink_iters: SHRINKS as u32,
        ..Config::default()
    })
}
fn program(form: &Form, header: bool, comment: bool) -> String {
    let header = if header {
        "fn qf[qT, qn:size](qa:qMissing[qT,qn])"
    } else {
        "fn qf()"
    };
    let comment = if comment {
        "/* outer /* inner */ é */\n"
    } else {
        "// é\n"
    };
    format!(
        "{comment}{header} {{ let qout = {}; qout }}\nfn qafter() {{}}\n",
        form.render()
    )
}
fn lex(source: &str) -> (hirsel_lexer::LexOutput, SourceMap) {
    let mut map = SourceMap::default();
    let file = map.add("grammar.hir", source.into()).unwrap();
    (hirsel_lexer::lex(file, &map), map)
}
// Independent hard checks: recursive desired_size is only a distribution hint.
fn within_caps(source: &str) -> bool {
    let (lexed, _) = lex(source);
    if lexed.tokens.len() > TOKENS {
        return false;
    }
    let mut depth = 0usize;
    for t in lexed.tokens {
        match t.kind {
            TokenKind::Punct(Punct::LParen | Punct::LBracket | Punct::LBrace)
            | TokenKind::Format(FormatPiece::Head) => depth += 1,
            TokenKind::Punct(Punct::RParen | Punct::RBracket | Punct::RBrace)
            | TokenKind::Format(FormatPiece::Tail) => depth = depth.saturating_sub(1),
            _ => (),
        }
        if depth > DEPTH {
            return false;
        }
    }
    true
}
// Exactly one edit: remove a delimiter/separator/keyword, or truncate at a
// token (also within a format/comment token). No insertion can increase depth.
fn mutate(source: &str, kind: usize, choice: usize) -> String {
    let (lexed, _) = lex(source);
    let mut edits = vec![];
    for t in lexed.tokens {
        let a = t.span.start as usize;
        let b = t.span.end as usize;
        match (kind, t.kind) {
            (
                0,
                TokenKind::Punct(
                    Punct::LParen
                    | Punct::RParen
                    | Punct::LBracket
                    | Punct::RBracket
                    | Punct::LBrace
                    | Punct::RBrace,
                ),
            ) => edits.push((a, b)),
            (0, TokenKind::Format(_)) if b > a => edits.push((b - 1, b)),
            (
                1,
                TokenKind::Punct(Punct::Comma | Punct::Semicolon | Punct::Colon | Punct::FatArrow),
            ) => edits.push((a, b)),
            (2, TokenKind::Keyword(_)) => edits.push((a, b)),
            (3, _) if a < source.len() => {
                edits.push((a, source.len()));
                if b > a && source.is_char_boundary(b - 1) {
                    edits.push((b - 1, source.len()));
                }
            }
            _ => (),
        }
    }
    assert!(!edits.is_empty());
    for i in 0..edits.len() {
        let (a, b) = edits[(choice + i) % edits.len()];
        let candidate = format!("{}{}", &source[..a], &source[b..]);
        if within_caps(&candidate) {
            return candidate;
        }
    }
    panic!("no mutation under caps");
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Failure {
    Panic,
    Span,
    Arena,
    Tape,
    Determinism,
    Rename,
    Tooling,
    Timeout,
    Injected,
}
type Checked<T> = Result<T, Failure>;
fn require(ok: bool, failure: Failure) -> Checked<()> {
    if ok { Ok(()) } else { Err(failure) }
}
fn span(map: &SourceMap, s: Span) -> Checked<()> {
    require(map.slice(s).is_some(), Failure::Span)
}
fn diagnostics(map: &SourceMap, ds: &[Diagnostic]) -> Checked<Vec<Diagnostic>> {
    let mut ds = ds.to_vec();
    for d in &mut ds {
        let normalize = |s: &mut Span| -> Checked<()> {
            span(map, *s)?;
            let placed = map.resolve(*s);
            *s = Span {
                anchor: Anchor(0),
                start: placed.start,
                end: placed.end,
            };
            Ok(())
        };
        normalize(&mut d.primary)?;
        for l in &mut d.secondary {
            normalize(&mut l.span)?;
        }
        if let Some(f) = &mut d.fix {
            normalize(&mut f.span)?;
        }
    }
    Ok(ds)
}
fn origin(map: &SourceMap, o: Origin, tree: &SyntaxTree) -> Checked<()> {
    span(map, o.span)?;
    require(
        o.tokens.start <= o.tokens.end && o.tokens.end as usize <= tree.tokens.len(),
        Failure::Span,
    )
}
// Exhaustive arena-edge visitor: do not rely on the parser's height visitor,
// and check abandoned nodes as well as nodes reachable from a published item.
fn arenas(item: &ItemSyntax, map: &SourceMap) -> Checked<()> {
    let t = &item.tree;
    let e = |x: ExprId| require((x.0 as usize) < t.expressions.len(), Failure::Arena);
    let s = |x: StmtId| require((x.0 as usize) < t.statements.len(), Failure::Arena);
    let p = |x: PatternId| require((x.0 as usize) < t.patterns.len(), Failure::Arena);
    let ty = |x: TypeExprId| require((x.0 as usize) < t.types.len(), Failure::Arena);
    let sig = |f: &FunctionSyntax| -> Checked<()> {
        for a in &f.parameters {
            ty(a.ty)?;
        }
        if let Some(x) = f.result {
            ty(x)?;
        }
        for c in &f.contracts {
            e(c.expression)?;
        }
        Ok(())
    };
    origin(map, item.item.origin, t)?;
    span(map, item.item.name_span)?;
    match &item.item.kind {
        ItemKind::Function {
            signature, body, ..
        } => {
            sig(signature)?;
            if let Some(x) = body {
                e(*x)?;
            }
        }
        ItemKind::Const { ty: x, value } => {
            ty(*x)?;
            e(*value)?;
        }
        ItemKind::Type { body, drop, .. } => {
            match body {
                TypeBody::Alias(x) => ty(*x)?,
                TypeBody::Record(xs) => {
                    for x in xs {
                        ty(x.ty)?;
                        origin(map, x.origin, t)?;
                    }
                }
                TypeBody::Variants(xs) => {
                    for x in xs {
                        for (_, x) in &x.fields {
                            ty(*x)?;
                        }
                    }
                }
                TypeBody::Distinct { field, fact } => {
                    ty(field.ty)?;
                    origin(map, field.origin, t)?;
                    if let Some(x) = fact {
                        e(*x)?;
                    }
                }
                TypeBody::Opaque => (),
            }
            if let Some(x) = drop {
                e(x.body)?;
            }
        }
        ItemKind::Signature { members } => {
            for member in members {
                origin(map, member.origin(), t)?;
                if let MemberSpec::Operation { signature, .. } = member {
                    sig(signature)?;
                }
            }
        }
        ItemKind::Implementation { members, .. } => {
            for member in members {
                origin(map, member.origin(), t)?;
                if let MemberDefinition::Type { ty: x, .. } = member {
                    ty(*x)?;
                }
            }
        }
    }
    for x in &t.expressions {
        origin(map, x.origin, t)?;
        match &x.kind {
            ExprKind::Tuple(xs) | ExprKind::Sequence(xs) => {
                for x in xs {
                    e(*x)?;
                }
            }
            ExprKind::Call { callee, arguments } => {
                e(*callee)?;
                for a in arguments {
                    e(a.value)?;
                }
            }
            ExprKind::Field { base, .. } => e(*base)?,
            ExprKind::With { callee, bindings } => {
                e(*callee)?;
                for b in bindings {
                    origin(map, b.origin, t)?;
                }
            }
            ExprKind::Index { base, index } => {
                e(*base)?;
                e(*index)?;
            }
            ExprKind::Unary { value, .. }
            | ExprKind::Try { value, .. }
            | ExprKind::Before(value)
            | ExprKind::Comptime(value) => e(*value)?,
            ExprKind::Binary { left, right, .. } => {
                e(*left)?;
                e(*right)?;
            }
            ExprKind::Block {
                statements, tail, ..
            } => {
                for x in statements {
                    s(*x)?;
                }
                if let Some(x) = tail {
                    e(*x)?;
                }
            }
            ExprKind::If {
                condition,
                then_branch,
                else_branch,
            } => {
                e(*condition)?;
                e(*then_branch)?;
                if let Some(x) = else_branch {
                    e(*x)?;
                }
            }
            ExprKind::Match { value, arms } => {
                e(*value)?;
                for a in arms {
                    p(a.pattern)?;
                    if let Some(x) = a.guard {
                        e(x)?;
                    }
                    e(a.body)?;
                }
            }
            ExprKind::Closure { signature, body } => {
                sig(signature)?;
                e(*body)?;
            }
            ExprKind::Return(x) | ExprKind::Break { value: x, .. } => {
                if let Some(x) = x {
                    e(*x)?;
                }
            }
            ExprKind::Shape(x) => ty(*x)?,
            ExprKind::Formatter(FormatStep::Pad(x)) => e(*x)?,
            ExprKind::Literal(_)
            | ExprKind::Path(_)
            | ExprKind::Continue(_)
            | ExprKind::Heap
            | ExprKind::Result
            | ExprKind::Todo(_)
            | ExprKind::Formatter(_)
            | ExprKind::Error => (),
        }
    }
    for x in &t.statements {
        origin(map, x.origin, t)?;
        match &x.kind {
            StmtKind::Let {
                pattern,
                ty: x,
                value,
                otherwise,
                ..
            } => {
                p(*pattern)?;
                if let Some(x) = x {
                    ty(*x)?;
                }
                e(*value)?;
                if let Some(x) = otherwise {
                    e(*x)?;
                }
            }
            StmtKind::Assign { place, value, .. } => {
                e(*place)?;
                e(*value)?;
            }
            StmtKind::Expr(x) => e(*x)?,
            StmtKind::While {
                condition, body, ..
            } => {
                e(*condition)?;
                e(*body)?;
            }
            StmtKind::For {
                pattern,
                source,
                body,
                ..
            } => {
                p(*pattern)?;
                e(*source)?;
                e(*body)?;
            }
            StmtKind::Function {
                signature, body, ..
            } => {
                sig(signature)?;
                e(*body)?;
            }
        }
    }
    for x in &t.patterns {
        origin(map, x.origin, t)?;
        match &x.kind {
            PatternKind::Tuple(xs) | PatternKind::Or(xs) => {
                for x in xs {
                    p(*x)?;
                }
            }
            PatternKind::Constructor { fields, .. } => {
                for x in fields {
                    p(x.pattern)?;
                }
            }
            PatternKind::Wildcard
            | PatternKind::Binding(_)
            | PatternKind::Literal(_)
            | PatternKind::Range { .. }
            | PatternKind::Error => (),
        }
    }
    for x in &t.types {
        origin(map, x.origin, t)?;
        match &x.kind {
            TypeExprKind::Named { arguments, .. } => {
                for x in arguments {
                    if let TypeArgument::Type(x) = x {
                        ty(*x)?;
                    }
                }
            }
            TypeExprKind::Tuple(xs) => {
                for x in xs {
                    ty(*x)?;
                }
            }
            TypeExprKind::Function(x) => sig(x)?,
            TypeExprKind::Box(x) => ty(*x)?,
            TypeExprKind::Qualified { base, relation, .. } => {
                ty(*base)?;
                if let Some(x) = relation {
                    origin(map, x.origin, t)?;
                }
            }
            TypeExprKind::Error => (),
        }
    }
    for x in &t.formats {
        e(x.expression)?;
        for x in &x.holes {
            e(*x)?;
        }
        require(
            x.pieces.len() == x.holes.len() + 1 && x.specs.len() == x.holes.len(),
            Failure::Arena,
        )?;
    }
    for x in &t.groups {
        e(x.expression)?;
        origin(map, x.origin, t)?;
    }
    for x in &t.repeats {
        for x in [
            x.expression,
            x.sequence,
            x.value,
            x.count,
            x.evaluated_count,
        ] {
            e(x)?;
        }
    }
    for x in &t.conditions {
        e(x.matching())?;
        if let LetCondition::While { body, .. } = x {
            e(*body)?;
        }
    }
    Ok(())
}

#[derive(Debug, Eq, PartialEq)]
struct Snapshot {
    disposition: &'static str,
    diagnostics: Vec<Diagnostic>,
    trees: String,
}
fn inspect(source: &str) -> Checked<Snapshot> {
    require(within_caps(source), Failure::Tooling)?; // Always before parse.
    let (lexed, mut map) = lex(source);
    let file = map.anchors().next().unwrap().1;
    let lex_diags = diagnostics(&map, &lexed.diagnostics)?;
    for t in &lexed.tokens {
        span(&map, t.span)?;
    }
    require(lexed == hirsel_lexer::lex(file, &map), Failure::Determinism)?;
    if !lex_diags.is_empty() {
        return Ok(Snapshot {
            disposition: "lexical",
            diagnostics: lex_diags,
            trees: String::new(),
        });
    }
    let original = lexed.tokens.clone();
    let parsed = hirsel_parser::parse(file, lexed.tokens, &support::context(), &mut map);
    require(parsed.nesting_limit.is_none(), Failure::Tooling)?;
    let tape = parsed
        .syntax
        .tape()
        .map(|t| (t.kind.clone(), map.resolve(t.span)))
        .collect::<Vec<_>>();
    require(
        tape == original
            .iter()
            .map(|t| (t.kind.clone(), map.resolve(t.span)))
            .collect::<Vec<_>>(),
        Failure::Tape,
    )?;
    let ds = diagnostics(&map, &parsed.diagnostics)?;
    let mut trees = String::new();
    for item in &parsed.syntax.items {
        arenas(item, &map)?;
        let mut item = (**item).clone();
        item.reanchor(Anchor(0));
        // Hashes encode source spelling, so omit them from alpha comparisons.
        trees.push_str(&format!("{:?}{:?}", item.item.kind, item.tree));
    }
    Ok(Snapshot {
        disposition: if ds.is_empty() { "accepted" } else { "parser" },
        diagnostics: ds,
        trees,
    })
}
fn rename(source: &str) -> (String, Vec<(String, String)>) {
    let (lexed, _) = lex(source);
    let mut out = source.to_owned();
    let mut names = vec![];
    // Only whole lexer identifiers with our prefix; contextual grammar names
    // (size, sendable, env, bounds), keywords and literal text stay untouched.
    for t in lexed.tokens.iter().rev() {
        if let TokenKind::Ident(s) = t.kind {
            let name = s.as_str();
            if let Some(rest) = name.strip_prefix('q') {
                let renamed = format!("Q{rest}");
                out.replace_range(t.span.start as usize..t.span.end as usize, &renamed);
                names.push((name.to_owned(), renamed));
            }
        }
    }
    (out, names)
}
fn check(source: &str) -> Checked<&'static str> {
    let first = inspect(source)?;
    require(first == inspect(source)?, Failure::Determinism)?;
    if first.disposition != "lexical" {
        let (renamed, names) = rename(source);
        let renamed = inspect(&renamed)?;
        let mut normalized = format!("{renamed:?}");
        for (name, renamed) in names {
            normalized = normalized.replace(&renamed, &name);
        }
        require(format!("{first:?}") == normalized, Failure::Rename)?;
    }
    Ok(first.disposition)
}

// One disposable process per input: a parser hang/abort cannot wedge libtest
// or prevent saving the original and reduced bytes. Children run serially.
#[test]
#[ignore = "worker entry, invoked by the bounded parent"]
fn worker() {
    let input = std::env::var_os("HIRSEL_GRAMMAR_INPUT").expect("worker input");
    let result_path = std::env::var_os("HIRSEL_GRAMMAR_RESULT").expect("worker result");
    let source = fs::read_to_string(input).unwrap();
    let result = std::panic::catch_unwind(|| check(&source)).unwrap_or(Err(Failure::Panic));
    fs::write(
        result_path,
        match result {
            Ok(x) => x.to_owned(),
            Err(x) => format!("{x:?}"),
        },
    )
    .unwrap();
}
fn isolated(source: &str, dir: &FsPath) -> Checked<String> {
    fs::write(dir.join("active.hir"), source).unwrap();
    let result = dir.join("active.result");
    if result.exists() {
        fs::remove_file(&result).unwrap();
    }
    let log = fs::File::create(dir.join("worker.log")).unwrap();
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "grammar_fuzz_22::worker",
            "--ignored",
            "--nocapture",
        ])
        .env("HIRSEL_GRAMMAR_INPUT", dir.join("active.hir"))
        .env("HIRSEL_GRAMMAR_RESULT", &result)
        .env("HIRSEL_MEMORY_LIMIT", "2G")
        .stdout(Stdio::from(log.try_clone().unwrap()))
        .stderr(Stdio::from(log))
        .spawn()
        .map_err(|_| Failure::Tooling)?;
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        if let Some(status) = child.try_wait().map_err(|_| Failure::Tooling)? {
            if !status.success() {
                return Err(Failure::Tooling);
            }
            break;
        }
        if Instant::now() >= deadline {
            child.kill().unwrap();
            child.wait().unwrap();
            return Err(Failure::Timeout);
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    let result = fs::read_to_string(result).map_err(|_| Failure::Tooling)?;
    match result.as_str() {
        "accepted" | "parser" | "lexical" => Ok(result),
        "Panic" => Err(Failure::Panic),
        "Span" => Err(Failure::Span),
        "Arena" => Err(Failure::Arena),
        "Tape" => Err(Failure::Tape),
        "Determinism" => Err(Failure::Determinism),
        "Rename" => Err(Failure::Rename),
        _ => Err(Failure::Tooling),
    }
}
fn evidence(label: &str) -> PathBuf {
    let root = std::env::var_os("HIRSEL_GRAMMAR_EVIDENCE")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    let dir = root.join(format!("grammar-fuzz-22-{label}-{}", std::process::id()));
    fs::create_dir(&dir).unwrap(); // Never overwrite another attempt.
    fs::write(
        dir.join("seed.txt"),
        format!(
            "seed={SEED:#x}\ncases={CASES}\ntokens={TOKENS}\ndepth={DEPTH}\nshrinks={SHRINKS}\n"
        ),
    )
    .unwrap();
    dir
}

// Deterministic structural deletion: whole lines/items, balanced delimiter
// regions, then contiguous token chunks down to one token. Relex after each
// accepted reduction; all attempts remain capped and preserve the exact category.
fn candidates(source: &str) -> Vec<String> {
    let mut ranges = vec![];
    let mut start = 0;
    for line in source.split_inclusive('\n') {
        ranges.push((start, start + line.len()));
        start += line.len();
    }
    let (lexed, _) = lex(source);
    let mut stack = vec![];
    for t in &lexed.tokens {
        match t.kind {
            TokenKind::Punct(Punct::LParen | Punct::LBracket | Punct::LBrace)
            | TokenKind::Format(FormatPiece::Head) => stack.push(t.span.start as usize),
            TokenKind::Punct(Punct::RParen | Punct::RBracket | Punct::RBrace)
            | TokenKind::Format(FormatPiece::Tail) => {
                if let Some(a) = stack.pop() {
                    ranges.push((a, t.span.end as usize));
                }
            }
            _ => (),
        }
    }
    let ts = &lexed.tokens;
    let mut width = ts.len() / 2;
    while width > 0 {
        for chunk in ts.chunks(width) {
            ranges.push((
                chunk[0].span.start as usize,
                chunk.last().unwrap().span.end as usize,
            ));
        }
        width /= 2;
    }
    let mut out = vec![];
    for (a, b) in ranges {
        if a < b {
            let x = format!("{}{}", &source[..a], &source[b..]);
            if within_caps(&x) && !out.contains(&x) {
                out.push(x);
            }
        }
    }
    out
}
fn reduce(
    original: &str,
    category: Failure,
    mut predicate: impl FnMut(&str) -> Option<Failure>,
) -> (String, usize) {
    let mut reduced = original.to_owned();
    let mut attempts = 0;
    loop {
        let mut changed = false;
        for candidate in candidates(&reduced) {
            if attempts == SHRINKS {
                return (reduced, attempts);
            }
            attempts += 1;
            if predicate(&candidate) == Some(category) {
                reduced = candidate;
                changed = true;
                break;
            }
        }
        if !changed {
            return (reduced, attempts);
        }
    }
}

#[test]
fn bounded_grammar_recovery() {
    let dir = evidence("cases");
    let strategy = (forms(), any::<bool>(), any::<bool>(), any::<usize>());
    let mut runner = runner();
    let mut counts = [0; 3];
    let mut deepest = 0;
    let mut largest = 0;
    for i in 0..CASES {
        let (form, header, comment, choice) = strategy.new_tree(&mut runner).unwrap().current();
        assert!(1 + form.depth() <= DEPTH);
        let valid = program(&form, header, comment);
        let original = mutate(&valid, i % 4, choice);
        for (label, source) in [("valid", &valid), ("original", &original)] {
            assert!(within_caps(source), "generator cap: {source}");
            largest = largest.max(lex(source).0.tokens.len());
            fs::write(dir.join(format!("case-{i:03}-{label}.hir")), source).unwrap();
            let outcome = isolated(source, &dir);
            if let Err(category) = outcome {
                fs::write(dir.join("failure-original.hir"), source).unwrap();
                fs::write(
                    dir.join("failure.txt"),
                    format!("case={i} variant={label} category={category:?}\n"),
                )
                .unwrap();
                let (reduced, n) = reduce(source, category, |s| isolated(s, &dir).err());
                fs::write(dir.join("failure-reduced.hir"), &reduced).unwrap();
                panic!(
                    "{category:?}: case {i}/{label}, {n} shrink attempts; evidence {}",
                    dir.display()
                );
            }
            let outcome = outcome.unwrap();
            fs::write(dir.join(format!("case-{i:03}-{label}.result")), &outcome).unwrap();
            if label == "valid" {
                assert_eq!(outcome, "accepted", "{source}");
            } else {
                counts[match outcome.as_str() {
                    "accepted" => 0,
                    "parser" => 1,
                    "lexical" => 2,
                    _ => unreachable!(),
                }] += 1;
            }
        }
        deepest = deepest.max(1 + form.depth());
    }
    assert_eq!(deepest, DEPTH, "seed must exercise the recursive boundary");
    assert!(
        counts[1] > 0 && counts[2] > 0,
        "exercise both rejection stages"
    );
    let receipt = format!(
        "cases={CASES} valid={CASES} mutated accepted={} parser={} lexical={} max_tokens={largest} max_depth={deepest}\n",
        counts[0], counts[1], counts[2]
    );
    fs::write(dir.join("result.txt"), &receipt).unwrap();
    eprintln!("{receipt}evidence: {}", dir.display());
}

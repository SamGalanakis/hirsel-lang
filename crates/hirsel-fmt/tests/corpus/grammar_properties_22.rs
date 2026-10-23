//! Bounded recursive generation and structural shrinking, adapted from Proptest
//! 70cf37d4, strategy/recursive.rs:50-100,150-208 and the book's recursive and
//! shrinking-basics tutorials. This deterministic cover enumerates choices
//! instead of sampling them; unary branches give a hard size/depth bound.
//! Parser context/nearby-input vectors follow rust-analyzer 3131751e,
//! parser/src/tests.rs:61-130, tests/{top,prefix}_entries.rs and
//! grammar/generic_args.rs:1-110. Only Hirsel's documented grammar is generated.
//! The existing corpus normalization remains the AST oracle.

use crate::{comments, normalized, parse};
use hirsel_syntax::SyntaxFile;

const MAX_DEPTH: usize = 4;
const MAX_TOKENS: usize = 256;

#[derive(Clone, Debug)]
enum Fragment {
    Leaf(String),
    Around(&'static str, Box<Fragment>, &'static str),
}

impl Fragment {
    fn around(self, before: &'static str, after: &'static str) -> Self {
        Self::Around(before, Box::new(self), after)
    }

    fn source(&self) -> String {
        match self {
            Self::Leaf(text) => text.clone(),
            Self::Around(before, inner, after) => format!("{before}{}{after}", inner.source()),
        }
    }

    fn depth(&self) -> usize {
        match self {
            Self::Leaf(_) => 0,
            Self::Around(_, inner, _) => 1 + inner.depth(),
        }
    }

    /// Try the child first, then simplify inside the current branch. A passing
    /// candidate is discarded, just as ValueTree::complicate restores it.
    fn simpler(&self) -> Vec<Self> {
        match self {
            Self::Leaf(text) if text == "x" => vec![],
            Self::Leaf(_) => vec![Self::Leaf("x".into())],
            Self::Around(before, inner, after) => {
                let mut candidates = vec![(**inner).clone()];
                candidates.extend(
                    inner
                        .simpler()
                        .into_iter()
                        .map(|child| child.around(before, after)),
                );
                candidates
            }
        }
    }
}

#[derive(Clone, Debug)]
struct Case {
    context: (&'static str, &'static str),
    fragment: Fragment,
}

impl Case {
    fn source(&self) -> String {
        format!(
            "{}{}{}",
            self.context.0,
            self.fragment.source(),
            self.context.1
        )
    }
}

fn expression(kind: usize, inner: Fragment) -> Fragment {
    match kind % 10 {
        0 => inner.around("(", " // group\n)"),
        1 => inner.around("(", " /* left */\n + /* right */ 1)"),
        2 => inner.around("(", " // projection\n).type"),
        3 => inner.around("[", " /* value */; /* count */ 2]"),
        4 => inner.around("f\"value={ ", " } end={x}\""),
        5 => inner.around("if let Some(x) = (", ") { x } else { 0 }"),
        6 => inner.around("match (", ") { _ => 1 }"),
        7 => inner.around("{ let Some(x) = (", ") else { return }\n x }"),
        8 => inner.around("{ while let Some(x) = (", ") { x; }\n 0 }"),
        _ => inner.around("if (", ") { 1 }\n/* else */ else if c { 2 } else { 3 }"),
    }
}

fn generated_cases() -> Vec<Case> {
    let mut cases = Vec::new();
    for seed in 0..64 {
        let mut fragment = Fragment::Leaf(match seed % 4 {
            0 => format!("x{seed}"),
            1 => format!("[0u8; {}]", seed + 1),
            2 => format!("f\"{{x{seed}}}\""),
            _ => format!("{{ x{seed} }}"),
        });
        for level in (0..1 + seed % MAX_DEPTH).rev() {
            fragment = expression(seed + level * 3, fragment);
        }
        cases.push(Case {
            context: [
                ("fn f() -> (fn() -> fn()) uses {a} { ", " }"),
                ("fn f() { fn nested() -> (fn()) uses {a} { ", " } }"),
                ("fn f() { call(fn() -> (fn()) uses {a} { ", " }) }"),
                ("fn f() { call(fn() -> (fn()) { ", " } uses {a}) }"),
            ][(seed / 4) % 4],
            fragment,
        });
    }
    // Each result is explicitly closed before the outer authority clause.
    // Exercise declarations, local functions, closures and type aliases.
    for context in [
        ("fn f() -> ", " uses {a} { g }"),
        ("fn f() { fn nested() -> ", "\nuses {a} { g } }"),
        ("fn f() { call(fn() -> ", " { /* body */ g } uses {a}) }"),
        ("type F = fn() -> ", " uses {a}"),
    ] {
        for variant in 0..4 {
            let mut fragment = Fragment::Leaf("fn()".into());
            for _ in 0..variant % 3 {
                fragment = fragment.around("fn() -> (", ") uses {}");
            }
            fragment = fragment.around("(", " // result\n)");
            if variant == 3 {
                fragment = fragment.around("box ", "");
            }
            cases.push(Case { context, fragment });
        }
    }
    cases
}

fn semantic(file: &SyntaxFile) -> Option<Vec<Vec<Vec<u8>>>> {
    file.items
        .iter()
        .map(|item| hirsel_fmt::semantic_item(item))
        .collect()
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Failure {
    SourceParse,
    Refused,
    Reparse,
    Ast,
    Comments,
    Semantic,
    FixedPoint,
}

fn check(source: &str) -> Result<(), Failure> {
    let (original, sources, errors) = parse(source);
    if errors {
        return Err(Failure::SourceParse);
    }
    let first = hirsel_fmt::format(&original, &sources);
    if first.unchanged {
        return Err(Failure::Refused);
    }
    let (reparsed, new_sources, errors) = parse(&first.source);
    if errors {
        return Err(Failure::Reparse);
    }
    if normalized(&original) != normalized(&reparsed) {
        return Err(Failure::Ast);
    }
    if comments(&original, &sources) != comments(&reparsed, &new_sources) {
        return Err(Failure::Comments);
    }
    let before = semantic(&original).ok_or(Failure::Semantic)?;
    if Some(before) != semantic(&reparsed) {
        return Err(Failure::Semantic);
    }
    let second = hirsel_fmt::format(&reparsed, &new_sources);
    if second.unchanged || first.source != second.source {
        return Err(Failure::FixedPoint);
    }
    Ok(())
}

fn shrink(case: &Case, failure: Failure) -> Case {
    let mut reduced = case.clone();
    loop {
        let next = reduced.fragment.simpler().into_iter().find_map(|fragment| {
            let candidate = Case {
                context: reduced.context,
                fragment,
            };
            (candidate.source().len() < reduced.source().len()
                && check(&candidate.source()) == Err(failure))
            .then_some(candidate)
        });
        match next {
            Some(next) => reduced = next,
            None => return reduced,
        }
    }
}

fn verify(id: &str, case: &Case) -> Option<String> {
    assert!(case.fragment.depth() <= MAX_DEPTH, "{id}: generation depth");
    let source = case.source();
    let (file, _, _) = parse(&source);
    let tokens = file.tape().count();
    assert!(tokens <= MAX_TOKENS, "{id}: {tokens} tokens\n{source}");
    if let Err(failure) = check(&source) {
        let reduced = shrink(case, failure).source();
        let (file, sources, _) = parse(&source);
        let formatted = hirsel_fmt::format(&file, &sources).source;
        return Some(format!(
            "{id}: {failure:?}\noriginal:\n{source}\nreduced:\n{reduced}\nformatted original:\n{formatted}"
        ));
    }
    None
}

#[test]
fn grammar_properties_22_generated_roundtrip() {
    let cases = generated_cases();
    assert_eq!(cases.len(), 80);
    let mut sources = std::collections::BTreeSet::new();
    let mut failures = Vec::new();
    for (index, case) in cases.iter().enumerate() {
        assert!(sources.insert(case.source()), "duplicate case {index}");
        if let Some(failure) = verify(&format!("generated-{index:02}"), case) {
            failures.push(failure);
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
}

#[test]
fn grammar_properties_22_semantic_neighbors() {
    // Sixteen additional programs: syntax-valid one-edit neighbors must stay
    // distinct even when comments/grouping and canonical ENDs are normalized.
    for (index, (first, second)) in [
        ("return x", "return\n x"),
        ("x", "x;"),
        ("a - b", "a\n -b"),
        ("{ x }", "{ x; }"),
        ("[x; 2]", "[x; 3]"),
        ("f\"{x}\"", "f\"{y}\""),
        ("if let A(x) = y { x }", "if let B(x) = y { x }"),
        ("let x = f()?\n C", "let x = f()? C"),
    ]
    .into_iter()
    .enumerate()
    {
        let mut contents = Vec::new();
        for (side, text) in [first, second].into_iter().enumerate() {
            let case = Case {
                context: ("fn f() { ", " }"),
                fragment: Fragment::Leaf(text.into()),
            };
            assert_eq!(verify(&format!("neighbor-{index}-{side}"), &case), None);
            let (file, _, errors) = parse(&case.source());
            assert!(!errors);
            contents.push((normalized(&file), semantic(&file).unwrap()));
        }
        assert_ne!(contents[0].0, contents[1].0, "neighbor {index}: AST");
        assert_ne!(contents[0].1, contents[1].1, "neighbor {index}: semantic");
    }
}

#[test]
fn grammar_properties_22_minimized_regressions() {
    for source in [
        "fn f() { (f\"{x}\" // projection\n).type }",
        "fn f() { ([x; 2] // projection\n).type }",
        "fn f() { (if let A(x) = y { x } // projection\n).type }",
        "fn f() { if x { 1 }\n/* else */ else { 2 } }",
        "fn f() -> box (fn() // result\n) uses {} { g }",
        "type F = fn() -> box (fn() // result\n) uses {}",
    ] {
        let case = Case {
            context: ("", ""),
            fragment: Fragment::Leaf(source.into()),
        };
        assert_eq!(verify("minimized", &case), None);
    }
}

/*
Proptest: Copyright (c) 2026 FullContact, Inc
rust-analyzer: MIT license (upstream also offers Apache-2.0).

Permission is hereby granted, free of charge, to any
person obtaining a copy of this software and associated
documentation files (the "Software"), to deal in the
Software without restriction, including without
limitation the rights to use, copy, modify, merge,
publish, distribute, sublicense, and/or sell copies of
the Software, and to permit persons to whom the Software
is furnished to do so, subject to the following
conditions:

The above copyright notice and this permission notice
shall be included in all copies or substantial portions
of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF
ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED
TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A
PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT
SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY
CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION
OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR
IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER
DEALINGS IN THE SOFTWARE.
*/

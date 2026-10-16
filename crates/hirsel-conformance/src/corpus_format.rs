//! Token alignment and a guarded migration through the release CLI formatter.
use crate::{
    Compiler, Driver, Expect, Observation, Report, Status, discover, parse_header, run_suite,
    spec_rules,
};
use hirsel_engine::QueryEngine;
use hirsel_span::{FileId, Span};
use hirsel_syntax::*;
use std::{collections::BTreeMap, fs, path::Path, process::Command};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Token {
    pub spelling: String,
    pub start: usize,
    pub end: usize,
    pub line: usize,
    // The formatter generates grouping, separators and terminators from the AST.
    flexible: bool,
}
fn tape(engine: &mut QueryEngine, file: FileId) -> Result<Vec<Token>, String> {
    let lexed = engine
        .adapters
        .lexer
        .as_ref()
        .ok_or("missing lexer")?
        .lex(file, &engine.sources, &mut engine.symbols)
        .map_err(|e| format!("{e:?}"))?;
    let source = engine.sources.file(file).ok_or("missing source")?;
    let syntax = if lexed
        .value
        .iter()
        .any(|t| t.kind == TokenKind::Keyword(Keyword::Uses))
    {
        engine
            .adapters
            .parser
            .as_ref()
            .and_then(|parser| {
                parser
                    .parse(
                        file,
                        lexed.value.clone(),
                        &hirsel_engine::default_context(),
                        &engine.sources,
                        &engine.symbols,
                    )
                    .ok()
            })
            .map(|p| p.value)
    } else {
        None
    };
    let mut result: Vec<_> = lexed
        .value
        .iter()
        .filter_map(|t| {
            if matches!(t.kind, TokenKind::Trivia(_) | TokenKind::Eof) {
                return None;
            }
            Some(Token {
                spelling: source.text()[t.span.start as usize..t.span.end as usize].into(),
                start: t.span.start as usize,
                end: t.span.end as usize,
                line: source.location(t.span.start)?.line as usize,
                flexible: matches!(
                    t.kind,
                    TokenKind::Punct(
                        Punct::LParen | Punct::RParen | Punct::Comma | Punct::Semicolon
                    )
                ),
            })
        })
        .collect();
    if let Some(syntax) = syntax {
        order_closure_clauses(&mut result, &syntax);
    }
    Ok(result)
}
// A closure's optional trailing uses clause moves before its body. The AST
// names that exact body; braces belonging to a result relation are never moved.
fn order_closure_clauses(tokens: &mut Vec<Token>, syntax: &SyntaxFile) {
    for expr in &syntax.tree.expressions {
        let ExprKind::Closure { body, .. } = expr.kind else {
            continue;
        };
        let body = syntax.tree.expressions[body.0 as usize].origin.span;
        let Some(start) = tokens.iter().position(|t| {
            t.start >= body.end as usize
                && t.start < expr.origin.span.end as usize
                && t.spelling == "uses"
        }) else {
            continue;
        };
        if tokens.get(start + 1).is_none_or(|t| t.spelling != "{") {
            continue;
        }
        let Some(end) = tokens[start + 2..]
            .iter()
            .position(|t| t.spelling == "}")
            .map(|i| start + 2 + i)
        else {
            continue;
        };
        let clause: Vec<_> = tokens.drain(start..=end).collect();
        let at = tokens
            .iter()
            .position(|t| t.start == body.start as usize)
            .unwrap();
        tokens.splice(at..at, clause);
    }
}
fn parsed(engine: &mut QueryEngine, source: &str) -> Result<SyntaxFile, String> {
    let file = engine
        .sources
        .add("tree-proof.hir", source.into())
        .ok_or("source too large")?;
    let lexed = engine
        .adapters
        .lexer
        .as_ref()
        .ok_or("missing lexer")?
        .lex(file, &engine.sources, &mut engine.symbols)
        .map_err(|e| format!("{e:?}"))?;
    if !lexed.diagnostics.is_empty() {
        return Err("formatter input/output has lexical errors".into());
    }
    let parsed = engine
        .adapters
        .parser
        .as_ref()
        .ok_or("missing parser")?
        .parse(
            file,
            lexed.value,
            &hirsel_engine::default_context(),
            &engine.sources,
            &engine.symbols,
        )
        .map_err(|e| format!("{e:?}"))?;
    if parsed.value.has_syntax_errors || !parsed.diagnostics.is_empty() {
        return Err("formatter input/output has syntax errors".into());
    }
    Ok(parsed.value)
}
fn normalized(mut tree: SyntaxTree) -> SyntaxTree {
    let origin = Origin {
        span: Span::default(),
        tokens: TokenRange::default(),
    };
    fn attrs(attrs: &mut [Attribute], origin: Origin) {
        for attr in attrs {
            attr.origin = origin;
        }
    }
    for item in &mut tree.items {
        item.origin = origin;
        item.name_span = Span::default();
        item.owned_tokens = TokenRange::default();
        item.text_hash = ContentHash::default();
        attrs(&mut item.attributes, origin);
        match &mut item.kind {
            ItemKind::Function { signature, .. } => attrs(&mut signature.attributes, origin),
            ItemKind::Type { body, .. } => match body {
                TypeBody::Record(fields) => {
                    for field in fields {
                        field.origin = origin;
                    }
                }
                TypeBody::Distinct { field, .. } => field.origin = origin,
                _ => {}
            },
            _ => {}
        }
    }
    for expr in &mut tree.expressions {
        expr.origin = origin;
        if let ExprKind::Closure { signature, .. } = &mut expr.kind {
            attrs(&mut signature.attributes, origin);
        }
    }
    for stmt in &mut tree.statements {
        stmt.origin = origin;
        if let StmtKind::Function { signature, .. } = &mut stmt.kind {
            attrs(&mut signature.attributes, origin);
        }
    }
    for pattern in &mut tree.patterns {
        pattern.origin = origin;
    }
    for ty in &mut tree.types {
        ty.origin = origin;
        match &mut ty.kind {
            TypeExprKind::Function(signature) => attrs(&mut signature.attributes, origin),
            TypeExprKind::Qualified {
                relation: Some(relation),
                ..
            } => relation.origin = origin,
            _ => {}
        }
    }
    tree
}
/// Grouping has no AST node. Erase only source coordinates/trivia/hashes;
/// operators, qualifiers, clauses, roots and every indexed child must agree.
pub fn verify_tree(old: &str, new: &str) -> Result<(), String> {
    let mut engine = QueryEngine::new(
        hirsel_engine::default_environment(),
        hirsel_driver::compiler_adapters(),
    );
    let a = parsed(&mut engine, old)?;
    let b = parsed(&mut engine, new)?;
    if a.roots != b.roots || normalized(a.tree.clone()) != normalized(b.tree.clone()) {
        return Err("formatter changed the parsed AST".into());
    }
    let comments = |file: &SyntaxFile| {
        file.tokens
            .iter()
            .filter(|t| {
                matches!(
                    t.kind,
                    TokenKind::Trivia(
                        TriviaKind::LineComment
                            | TriviaKind::BlockComment
                            | TriviaKind::DocLine
                            | TriviaKind::DocBlock
                    )
                )
            })
            .map(|t| engine.sources.slice(t.span).unwrap())
            .collect::<Vec<_>>()
    };
    if comments(&a) != comments(&b) {
        return Err("formatter changed or reordered comment bytes".into());
    }
    Ok(())
}
pub fn tokens(source: &str) -> Result<Vec<Token>, String> {
    let mut engine = QueryEngine::new(
        hirsel_engine::default_environment(),
        hirsel_driver::compiler_adapters(),
    );
    let file = engine
        .sources
        .add("alignment.hir", source.into())
        .ok_or("source too large")?;
    tape(&mut engine, file)
}

/// Mandatory tokens must agree in order, including repeated spellings. Align
/// generated punctuation only within its two neighboring mandatory anchors.
/// This is a token alignment, never a diff of source lines.
pub fn align(old: &[Token], new: &[Token]) -> Result<Vec<Option<usize>>, String> {
    let anchors = |ts: &[Token]| {
        ts.iter()
            .enumerate()
            .filter(|(_, t)| !t.flexible)
            .map(|(i, _)| i)
            .collect::<Vec<_>>()
    };
    let (a, b) = (anchors(old), anchors(new));
    if a.len() != b.len()
        || a.iter()
            .zip(&b)
            .any(|(&i, &j)| old[i].spelling != new[j].spelling)
    {
        return Err("formatter changed or reordered significant tokens".into());
    }
    let mut mapping = vec![None; old.len()];
    let (mut left, mut right) = (0, 0);
    for (i, j) in a
        .into_iter()
        .zip(b)
        .chain(std::iter::once((old.len(), new.len())))
    {
        // Equal prefixes retain outer grouping even when redundant inner
        // parentheses disappear. Other removed punctuation stays unmapped.
        for offset in 0..(i - left).min(j - right) {
            if old[left + offset].spelling != new[right + offset].spelling {
                break;
            }
            mapping[left + offset] = Some(right + offset);
        }
        if i < old.len() {
            mapping[i] = Some(j);
        }
        left = i + 1;
        right = j + 1;
    }
    Ok(mapping)
}

/// Every old physical line gets its token's new line, or None for trivia-only
/// lines or removed punctuation. Diagnostic remapping uses the precise token.
pub fn line_map(
    source: &str,
    old: &[Token],
    new: &[Token],
    alignment: &[Option<usize>],
) -> Vec<Option<usize>> {
    let mut lines = vec![None; source.split('\n').count()];
    let mut physical: Vec<_> = old.iter().enumerate().collect();
    physical.sort_by_key(|(_, token)| token.start);
    for (i, token) in physical {
        if lines[token.line - 1].is_none() {
            lines[token.line - 1] = alignment[i].map(|j| new[j].line);
        }
    }
    lines
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Primary {
    pub rule: String,
    pub spelling: String,
    pub raw_token: String,
    /// Mandatory-token ordinal, or the preceding anchor plus gap punctuation.
    pub identity: String,
    pub line: usize,
    pub column: usize,
}
impl Primary {
    pub fn json(&self) -> serde_json::Value {
        serde_json::json!({"rule":self.rule,"token":self.spelling,"raw_token":self.raw_token,"identity":self.identity,"line":self.line,"column":self.column})
    }
    fn pair(&self) -> (&str, &str, &str) {
        (&self.rule, &self.spelling, &self.identity)
    }
}
fn token_at(ts: &[Token], offset: usize) -> Option<usize> {
    // Empty insertion spans, including EOF, name the next token or EOF itself.
    ts.iter()
        .position(|t| t.start <= offset && offset < t.end)
        .or_else(|| {
            ts.iter()
                .enumerate()
                .filter(|(_, t)| t.start >= offset)
                .min_by_key(|(_, t)| t.start)
                .map(|(i, _)| i)
        })
}
fn primary(
    engine: &mut QueryEngine,
    file: FileId,
    rule: &str,
    offset: usize,
    limit: Option<usize>,
) -> Option<Primary> {
    let ts = tape(engine, file).ok()?;
    let site = engine.sources.file(file)?.location(offset as u32)?;
    let raw_index = token_at(&ts, offset);
    let raw_token = raw_index
        .map(|i| ts[i].spelling.clone())
        .unwrap_or_else(|| "<EOF>".into());
    // First non-grouping primary token, with the raw
    // opening token retained in evidence. Tree equality guards this projection.
    let index = raw_index.map(|i| {
        if ts[i].spelling != "(" {
            return i;
        }
        ts.iter()
            .enumerate()
            .filter(|(_, t)| {
                t.start >= offset
                    && limit.is_none_or(|end| t.start < end)
                    && !matches!(t.spelling.as_str(), "(" | ")")
            })
            .min_by_key(|(_, t)| t.start)
            .map_or(i, |(j, _)| j)
    });
    let (spelling, identity) = match index {
        Some(i) => {
            let anchor = ts[..i].iter().filter(|t| !t.flexible).count();
            let identity = if ts[i].flexible {
                let gap = ts[..i]
                    .iter()
                    .rev()
                    .take_while(|t| t.flexible)
                    .map(|t| t.spelling.as_str())
                    .collect::<Vec<_>>();
                format!("gap {anchor}: {gap:?}")
            } else {
                format!("token {anchor}")
            };
            (ts[i].spelling.clone(), identity)
        }
        None => (
            "<EOF>".into(),
            format!("eof {}", ts.iter().filter(|t| !t.flexible).count()),
        ),
    };
    Some(Primary {
        rule: rule.into(),
        spelling,
        raw_token,
        identity,
        line: site.line as usize,
        column: site.column as usize,
    })
}
pub(crate) fn primary_at_span(engine: &mut QueryEngine, rule: &str, span: Span) -> Option<Primary> {
    primary(
        engine,
        span.file,
        rule,
        span.start as usize,
        Some(span.end as usize),
    )
}
pub(crate) fn primary_at_site(
    engine: &mut QueryEngine,
    path: &Path,
    rule: &str,
    line: usize,
    column: usize,
) -> Option<Primary> {
    let text = fs::read_to_string(path).ok()?;
    let offset = site_offset(&text, line, column)?;
    let syntax = parsed(engine, &text).ok();
    let limit = syntax.as_ref().and_then(|syntax| {
        syntax
            .tree
            .expressions
            .iter()
            .filter(|e| e.origin.span.start as usize == offset)
            .map(|e| e.origin.span.end as usize)
            .max()
    });
    let file = syntax
        .map(|s| s.file)
        .or_else(|| engine.sources.add(path, text))?;
    primary(engine, file, rule, offset, limit)
}
fn site_offset(source: &str, line: usize, column: usize) -> Option<usize> {
    let mut offset = 0;
    for (i, text) in source.split_inclusive('\n').enumerate() {
        if i + 1 == line {
            return text
                .char_indices()
                .nth(column.checked_sub(1)?)
                .map(|(j, _)| offset + j)
                .or_else(|| (column == text.chars().count() + 1).then_some(offset + text.len()));
        }
        offset += text.len();
    }
    (line == source.split('\n').count() && column == 1).then_some(source.len())
}

fn header_end(source: &str) -> usize {
    // Keep the comment block, including blank lines *inside* it; trailing blank
    // lines between the header and program belong to canonical source layout.
    let mut offset = 0;
    let mut end = 0;
    for line in source.split_inclusive('\n') {
        if !line.trim().is_empty() && !line.trim_start().starts_with("//") {
            break;
        }
        offset += line.len();
        if line.trim_start().starts_with("//") {
            end = offset;
        }
    }
    end
}
fn directives(source: &str) -> Vec<&str> {
    source
        .lines()
        .filter(|line| {
            let Some(comment) = line.trim_start().strip_prefix("//") else {
                return false;
            };
            comment
                .trim_start()
                .split_once(':')
                .is_some_and(|(key, _)| {
                    matches!(key, "rule" | "expect" | "stdout" | "exit" | "spec")
                })
        })
        .collect()
}
fn remap_header(source: &str, line: usize) -> Result<String, String> {
    let mut found = false;
    let mut result = String::new();
    for raw in source.split_inclusive('\n') {
        let comment = raw.trim_start().strip_prefix("//").map(str::trim_start);
        if comment
            .is_some_and(|c| c.starts_with("expect: reject ") || c.starts_with("expect: fail "))
        {
            let at = raw.rfind("at line ").ok_or("missing expectation line")? + 8;
            let count = raw[at..].bytes().take_while(u8::is_ascii_digit).count();
            result.push_str(&raw[..at]);
            result.push_str(&line.to_string());
            result.push_str(&raw[at + count..]);
            found = true;
        } else {
            result.push_str(raw);
        }
    }
    if !found {
        return Err("missing diagnostic expectation".into());
    }
    Ok(result)
}

pub fn rewrite(
    old: &str,
    formatted: &str,
    expect: &Expect,
    observed: &Observation,
) -> Result<String, String> {
    let header_comments = |text: &str| {
        text[..header_end(text)]
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(str::to_owned)
            .collect::<Vec<_>>()
    };
    if header_comments(old) != header_comments(formatted)
        || directives(old) != directives(formatted)
    {
        return Err("formatter would move or change a header comment block or runner directive; kept intact".into());
    }
    verify_tree(old, formatted)?;
    let (old_tokens, new_tokens) = (tokens(old)?, tokens(formatted)?);
    let mapping = align(&old_tokens, &new_tokens)?;
    let line = match expect {
        Expect::Reject { line, .. } | Expect::Fail { line, .. } => *line,
        _ => return Ok(formatted.into()),
    };
    let target = observed
        .primary
        .as_ref()
        .filter(|p| p.line == line)
        .and_then(|p| site_offset(old, p.line, p.column))
        .and_then(|offset| token_at(&old_tokens, offset))
        .or_else(|| old_tokens.iter().position(|t| t.line == line));
    let Some(target) = target else {
        // EOF insertion diagnostics can point to the final empty physical line.
        if observed
            .primary
            .as_ref()
            .is_some_and(|p| p.line == line && p.spelling == "<EOF>")
        {
            return remap_header(formatted, formatted.split('\n').count());
        }
        return Err(format!("expected line {line} has no source token"));
    };
    let target = if mapping[target].is_none() && old_tokens[target].spelling == "(" {
        old_tokens
            .iter()
            .enumerate()
            .filter(|(_, t)| {
                t.start >= old_tokens[target].start && !matches!(t.spelling.as_str(), "(" | ")")
            })
            .min_by_key(|(_, t)| t.start)
            .map_or(target, |(i, _)| i)
    } else {
        target
    };
    let mapped = mapping[target].ok_or_else(|| {
        format!(
            "expected token {:?} was removed or has ambiguous punctuation alignment",
            old_tokens[target].spelling
        )
    })?;
    remap_header(formatted, new_tokens[mapped].line)
}
fn equivalent(before: &Observation, after: &Observation) -> bool {
    before.primary.as_ref().map(Primary::pair) == after.primary.as_ref().map(Primary::pair)
        && before.checked == after.checked
        && (before.primary.is_some() || before.actual == after.actual)
}
fn same_verdict(before: &Status, after: &Status) -> bool {
    matches!(
        (before, after),
        (Status::Pass, Status::Pass)
            | (Status::Fail(_), Status::Fail(_))
            | (Status::NotImplemented, Status::NotImplemented)
    )
}

/// Check that the corpus files of one shard (every file without one) are canonical.
/// Format comparisons use the driver's same formatter seam as `hirsel fmt`.
pub fn check_canonical_shard(
    root: &Path,
    report: &mut Report,
    shard: Option<crate::Shard>,
) -> Result<(), String> {
    let mut files = Vec::new();
    discover(&root.join("conformance"), &mut files)?;
    files.retain(|file| shard.is_none_or(|shard| shard.holds(&crate::relative(root, file))));
    files.sort();
    for path in files {
        let old = fs::read_to_string(&path).map_err(|e| e.to_string())?;
        let canonical = match hirsel_driver::format_path(&path) {
            Ok(source) => source,
            Err(hirsel_engine::ToolError::Query {
                error: hirsel_engine::QueryError::Diagnostics(diagnostics),
                sources,
                ..
            }) => {
                let rules = spec_rules(root)?;
                let header = parse_header(path.strip_prefix(root).unwrap(), &old, &rules)?;
                // `format_path` stops on lexer/parser errors.
                // Exempt only the exact syntax rejection and site in the header.
                if let Expect::Reject { rule, line } = header.expect {
                    if diagnostics.first().is_some_and(|d| {
                        d.rule == rule
                            && sources
                                .location(d.primary)
                                .is_some_and(|p| p.line as usize == line)
                    }) {
                        report.format_exemptions.insert(
                            path.strip_prefix(root).unwrap().display().to_string(),
                            format!(
                                "formatter refuses expected syntax rejection {rule} at line {line}"
                            ),
                        );
                        continue;
                    }
                }
                report.errors.push(format!(
                    "{}: formatter refusal does not match an expected syntax rejection",
                    path.display()
                ));
                continue;
            }
            Err(error) => {
                report.errors.push(format!("{}: {error}", path.display()));
                continue;
            }
        };
        if old != canonical {
            report.errors.push(format!("{}: corpus source is not formatter-canonical; run corpus-fmt with the release hirsel", path.strip_prefix(root).unwrap().display()));
        }
    }
    Ok(())
}

/// The migration records whole-run before/after evidence and restores any file
/// whose runner verdict, rule or primary token changed. No ratchet is updated.
pub fn migrate(root: &Path, hirsel: &Path, evidence: &Path) -> Result<serde_json::Value, String> {
    if hirsel
        .parent()
        .and_then(Path::file_name)
        .is_none_or(|name| name != "release")
        || !hirsel.is_file()
    {
        return Err("--hirsel must name the built release/hirsel binary".into());
    }
    fs::create_dir_all(evidence).map_err(|e| e.to_string())?;
    let before = run_suite(root, &Compiler, false)?;
    fs::write(evidence.join("before.json"), before.json().to_string())
        .map_err(|e| e.to_string())?;
    if !before.success() {
        return Err("baseline conformance gate failed; see before.json".into());
    }
    let rules = spec_rules(root)?;
    let mut declined = BTreeMap::new();
    let mut changed = Vec::new();
    let mut canonical = 0;
    let mut header_blank_lines_normalized = 0;
    let mut line_maps = BTreeMap::new();
    let temporary = evidence.join("format-input.hir");
    for test in &before.tests {
        let path = root.join(&test.path);
        let old = fs::read_to_string(&path).map_err(|e| e.to_string())?;
        fs::write(&temporary, &old).map_err(|e| e.to_string())?;
        let output = Command::new(hirsel)
            .arg("fmt")
            .arg(&temporary)
            .output()
            .map_err(|e| e.to_string())?;
        if !output.status.success() {
            declined.insert(
                test.path.clone(),
                format!(
                    "release hirsel fmt failed: {}",
                    String::from_utf8_lossy(&output.stderr)
                        .replace(&temporary.display().to_string(), &test.path)
                ),
            );
            continue;
        }
        let formatted = fs::read_to_string(&temporary).map_err(|e| e.to_string())?;
        if old == formatted {
            canonical += 1;
            continue;
        }
        let header = parse_header(Path::new(&test.path), &old, &rules)?;
        let observation = &before.observations[&test.path];
        let candidate = match rewrite(&old, &formatted, &header.expect, observation) {
            Ok(candidate) => candidate,
            Err(reason) => {
                declined.insert(test.path.clone(), reason);
                continue;
            }
        };
        let new_header = parse_header(Path::new(&test.path), &candidate, &rules)?;
        fs::write(&path, &candidate).map_err(|e| e.to_string())?;
        let evaluated = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            Compiler.evaluate(&path, &new_header.expect)
        }));
        let (status, after) = match evaluated {
            Ok(value) => value,
            Err(_) => {
                fs::write(&path, &old).map_err(|e| e.to_string())?;
                declined.insert(
                    test.path.clone(),
                    "runner panicked on formatted source; original restored".into(),
                );
                continue;
            }
        };
        let canonical_candidate = match hirsel_driver::format_path(&path) {
            Ok(source) => source,
            Err(error) => {
                fs::write(&path, &old).map_err(|e| e.to_string())?;
                declined.insert(
                    test.path.clone(),
                    format!("formatter refused remapped source: {error}"),
                );
                continue;
            }
        };
        if !same_verdict(&test.status, &status) || !equivalent(observation, &after) {
            fs::write(&path, &old).map_err(|e| e.to_string())?;
            declined.insert(
                test.path.clone(),
                format!(
                    "runner meaning changed: {:?} / {:?} -> {:?} / {:?}",
                    test.status, observation.primary, status, after.primary
                ),
            );
        } else if canonical_candidate != candidate {
            fs::write(&path, &old).map_err(|e| e.to_string())?;
            declined.insert(
                test.path.clone(),
                "remapped header did not reach formatter fixed point".into(),
            );
        } else {
            if old[..header_end(&old)] != formatted[..header_end(&formatted)] {
                header_blank_lines_normalized += 1;
            }
            let a = tokens(&old)?;
            let b = tokens(&candidate)?;
            line_maps.insert(test.path.clone(), line_map(&old, &a, &b, &align(&a, &b)?));
            changed.push(test.path.clone());
        }
    }
    fs::remove_file(temporary).map_err(|e| e.to_string())?;
    let after = run_suite(root, &Compiler, false)?;
    fs::write(evidence.join("after.json"), after.json().to_string()).map_err(|e| e.to_string())?;
    if !after.success()
        || before.tests.len() != after.tests.len()
        || before.tests.iter().zip(&after.tests).any(|(a, b)| {
            a.path != b.path
                || !same_verdict(&a.status, &b.status)
                || !equivalent(&before.observations[&a.path], &after.observations[&b.path])
        })
    {
        return Err(
            "whole-corpus before/after verdict/rule/primary-token comparison failed".into(),
        );
    }
    let result = serde_json::json!({"compared":before.tests.len(),"already_canonical":canonical,"formatted":changed.len(),"changed":changed,"header_blank_lines_normalized":header_blank_lines_normalized,"line_maps":line_maps,"declined":declined,"before":before.counts.json(),"after":after.counts.json()});
    fs::write(evidence.join("migration.json"), result.to_string()).map_err(|e| e.to_string())?;
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn on_stack(f: impl FnOnce() + Send + 'static) {
        hirsel_engine::with_compiler_stack(f)
    }
    fn rejection(line: usize) -> Expect {
        Expect::Reject {
            rule: hirsel_diag::RuleId::new("LANG-1").unwrap(),
            line,
        }
    }
    #[test]
    fn repeated_tokens_on_one_line_map_to_the_same_occurrence() {
        on_stack(|| {
            let old = "// rule: LANG-1\n// expect: reject LANG-1 at line 3\nfn f() { let x = 1u8; let y = 1u8; x }\n";
            let new = "// rule: LANG-1\n// expect: reject LANG-1 at line 3\nfn f() {\n  let x = 1u8\n  let y = 1u8\n  x\n}\n";
            let observation = Observation {
                primary: Some(Primary {
                    rule: "LANG-1".into(),
                    spelling: "1u8".into(),
                    identity: "".into(),
                    raw_token: "".into(),
                    line: 3,
                    column: 30,
                }),
                ..Default::default()
            };
            // The column points to the second literal, not the first equal text.
            let column = old.lines().nth(2).unwrap().rfind("1u8").unwrap() + 1;
            let observation = Observation {
                primary: observation.primary.map(|p| Primary { column, ..p }),
                ..Default::default()
            };
            assert!(
                rewrite(old, new, &rejection(3), &observation)
                    .unwrap()
                    .contains("at line 5")
            );
            let a = tokens(old).unwrap();
            let b = tokens(new).unwrap();
            let mapping = align(&a, &b).unwrap();
            assert_eq!(line_map(old, &a, &b, &mapping)[2], Some(3));
        });
    }
    #[test]
    fn grouping_and_semicolons_do_not_shift_mandatory_identities() {
        on_stack(|| {
            let a = tokens("fn f(x: u8) -> u8 { (x); x }").unwrap();
            let b = tokens("fn f(x: u8) -> u8 { x\n x }").unwrap();
            let mapping = align(&a, &b).unwrap();
            let xs = a
                .iter()
                .enumerate()
                .filter(|(_, t)| t.spelling == "x")
                .map(|(i, _)| b[mapping[i].unwrap()].spelling.as_str())
                .collect::<Vec<_>>();
            assert_eq!(xs, vec!["x", "x", "x"]);
            assert!(align(&a, &tokens("fn f(x: u8) -> u8 { x\n y }").unwrap()).is_err());
        });
    }
    #[test]
    fn header_blank_lines_normalize_but_comments_and_directives_stay_exact() {
        on_stack(|| {
            let old = "// rule: LANG-1\n\n// expect: reject LANG-1 at line 4\nfn f() {}\n";
            let new = old.replace("\n\n", "\n");
            assert!(
                rewrite(old, &new, &rejection(4), &Observation::default())
                    .unwrap()
                    .contains("at line 3")
            );
            assert!(
                rewrite(
                    old,
                    &new.replace("rule: LANG-1", "rule: LANG-2"),
                    &rejection(4),
                    &Observation::default()
                )
                .is_err()
            );
            let old = "// rule: LANG-1\n// expect: run\n// stdout:  padded  \nfn f() {}\n";
            assert!(
                rewrite(
                    old,
                    &old.replace("padded  ", "padded"),
                    &Expect::Accept,
                    &Observation::default()
                )
                .is_err()
            );
        });
    }
    #[test]
    fn unicode_columns_and_runtime_failure_lines_remap() {
        on_stack(|| {
            let old = "// rule: LANG-1\n// expect: fail LANG-1 at line 3\nfn f() { let s = \"é\"; boom() }\n";
            let new = "// rule: LANG-1\n// expect: fail LANG-1 at line 3\nfn f() {\n  let s = \"é\"\n  boom()\n}\n";
            let column = old
                .lines()
                .nth(2)
                .unwrap()
                .split("boom")
                .next()
                .unwrap()
                .chars()
                .count()
                + 1;
            let observed = Observation {
                primary: Some(Primary {
                    rule: "LANG-1".into(),
                    spelling: "boom".into(),
                    identity: "".into(),
                    raw_token: "".into(),
                    line: 3,
                    column,
                }),
                ..Default::default()
            };
            let expect = Expect::Fail {
                rule: hirsel_diag::RuleId::new("LANG-1").unwrap(),
                line: 3,
                stdout: None,
            };
            assert!(
                rewrite(old, new, &expect, &observed)
                    .unwrap()
                    .contains("at line 5")
            );
        });
    }
    #[test]
    fn changed_discard_semantics_are_declined() {
        on_stack(|| {
            assert!(
                verify_tree("fn f(){ 1u8; }", "fn f(){ 1u8 }")
                    .unwrap_err()
                    .contains("AST")
            );
            assert!(
                verify_tree("fn f(){ 1u8 + 2u8 * 3u8 }", "fn f(){ (1u8 + 2u8) * 3u8 }").is_err()
            );
        });
    }
    #[test]
    fn redundant_overflow_grouping_keeps_tree_and_semantic_primary_token() {
        on_stack(|| {
            for op in ["+ -1", "- 1"] {
                let a = format!("fn f(){{ let value:i8 = (-127 - 1) {op} }}");
                let b = format!("fn f(){{ let value:i8 = -127 - 1 {op} }}");
                verify_tree(&a, &b).unwrap();
                let mut engine = QueryEngine::new(
                    hirsel_engine::default_environment(),
                    hirsel_driver::compiler_adapters(),
                );
                let left = engine.sources.add("a.hir", a.clone()).unwrap();
                let right = engine.sources.add("b.hir", b.clone()).unwrap();
                let old = primary(
                    &mut engine,
                    left,
                    "SEM-1",
                    a.find("(-127").unwrap(),
                    Some(a.len() - 2),
                )
                .unwrap();
                let new = primary(
                    &mut engine,
                    right,
                    "SEM-1",
                    b.find("-127").unwrap(),
                    Some(b.len() - 2),
                )
                .unwrap();
                assert_eq!(old.pair(), new.pair());
                assert_eq!(old.raw_token, "(");
                assert_eq!(new.raw_token, "-");
            }
        });
    }
    #[test]
    fn a_unit_primary_does_not_claim_a_token_after_its_span() {
        on_stack(|| {
            let mut engine = QueryEngine::new(
                hirsel_engine::default_environment(),
                hirsel_driver::compiler_adapters(),
            );
            let text = "fn f(){ () }";
            let file = engine.sources.add("unit.hir", text.into()).unwrap();
            let offset = text.find("() }").unwrap();
            let witness = primary(&mut engine, file, "LANG-7", offset, Some(offset + 2)).unwrap();
            assert_eq!(witness.spelling, "(");
            assert_eq!(witness.raw_token, "(");
        });
    }
    #[test]
    fn trailing_closure_uses_aligns_to_its_own_body_even_when_nested() {
        on_stack(|| {
            let a = "fn f(){fn()->u64 { (fn()->u64 {1u64} uses {})() } uses {}}";
            let b = "fn f(){fn()->u64 uses {} { (fn()->u64 uses {} {1u64})() }}";
            verify_tree(a, b).unwrap();
            let old = tokens(a).unwrap();
            let new = tokens(b).unwrap();
            let map = align(&old, &new).unwrap();
            for (i, t) in old.iter().enumerate().filter(|(_, t)| !t.flexible) {
                assert_eq!(t.spelling, new[map[i].unwrap()].spelling);
            }
            assert!(verify_tree(a, &b.replace("uses {}", "uses {cap}")).is_err());
        });
    }
    #[test]
    fn syntax_exemption_requires_matching_reject_rule_and_site() {
        on_stack(|| {
            let root =
                std::env::temp_dir().join(format!("hirsel-format-syntax-{}", std::process::id()));
            fs::create_dir_all(root.join("conformance/LANG")).unwrap();
            fs::create_dir_all(root.join(crate::SPEC_DIR)).unwrap();
            fs::write(
                root.join(crate::SPEC_DIR).join("language.md"),
                "### LANG-1: Syntax\n\n### LANG-7: Types\n",
            )
            .unwrap();
            let path = root.join("conformance/LANG/syntax.hir");
            fs::write(
                &path,
                "// rule: LANG-1\n// expect: reject LANG-1 at line 3\nfn f<T>() {}\n",
            )
            .unwrap();
            let mut report = Report::default();
            check_canonical_shard(&root, &mut report, None).unwrap();
            assert_eq!(report.format_exemptions.len(), 1);
            assert!(report.errors.is_empty());
            for expect in [
                "accept",
                "reject LANG-7 at line 3",
                "reject LANG-1 at line 2",
            ] {
                fs::write(
                    &path,
                    format!("// rule: LANG-1\n// expect: {expect}\nfn f<T>() {{}}\n"),
                )
                .unwrap();
                let mut report = Report::default();
                check_canonical_shard(&root, &mut report, None).unwrap();
                assert!(report.format_exemptions.is_empty());
                assert_eq!(report.errors.len(), 1);
            }
            fs::remove_dir_all(root).unwrap();
        });
    }
    #[test]
    fn gate_checks_even_unratcheted_files_and_ignores_archived_reports() {
        on_stack(|| {
            let root =
                std::env::temp_dir().join(format!("hirsel-format-gate-{}", std::process::id()));
            fs::create_dir_all(root.join("conformance/LANG/reports")).unwrap();
            fs::write(root.join("conformance/LANG/new.hir"), "fn f(){1u8}").unwrap();
            fs::write(
                root.join("conformance/LANG/reports/archive.hir"),
                "fn old(){1u8}",
            )
            .unwrap();
            let mut report = Report::default();
            check_canonical_shard(&root, &mut report, None).unwrap();
            assert_eq!(report.errors.len(), 1);
            assert!(report.errors[0].contains("new.hir"));
            fs::remove_dir_all(root).unwrap();
        });
    }
}

//! O5, annotation invariance (PS-19): analysis annotations never change what
//! compiles or what a program means. Each single-file corpus program gets an
//! `@analysis(o5.probe: n)` at every site PS-17 allows (each item, each
//! parameter of a function definition, each record or distinct field, each
//! variant payload). That annotated program and its blanked twin, the same
//! text with every probe replaced by spaces so every position agrees, must
//! give byte-identical diagnostics, definition hashes, interpreted outcome
//! and output, and, with `native`, the same LLVM and Cranelift executable
//! runs; the TIR export must carry every probe and, with its annotations
//! removed, be the twin's export byte for byte (so the program revision, TIR
//! digests, obligation ids and checksums agree).
//!
//! Native objects are not compared byte for byte: their failure-site codes
//! hash each item's source text revision, which any byte of an item moves,
//! a comment as much as an annotation. What the executables do is compared.
use super::{Artifacts, Config, Report, Verdict, difference};
use crate::{Expect, Rules};
use hirsel_engine::QueryEngine;
use hirsel_syntax::*;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// A program with a probe at every annotation site, and its blanked twin.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Probed {
    pub annotated: String,
    pub blanked: String,
    pub sites: usize,
}

/// The byte offset of every annotation site of `source`, in order, or why it
/// has none to probe (it does not parse).
pub fn sites(source: &str) -> Result<Vec<usize>, String> {
    let mut engine = QueryEngine::new(
        hirsel_engine::default_environment(),
        hirsel_driver::compiler_adapters(),
    );
    let file = crate::corpus_format::parsed(&mut engine, source)?;
    let sources = &engine.sources;
    // Sites are file offsets: each item's spans resolve through its anchor,
    // and its tokens are rebased onto the file's own anchor (base 0).
    let at = |span: hirsel_span::Span| sources.resolve(span);
    let base = sources.file_anchor(file.file);
    let tokens: Vec<Token> = file
        .items
        .iter()
        .flat_map(|item| &item.tree.tokens)
        .chain(&file.tokens)
        .filter(|t| !matches!(t.kind, TokenKind::Trivia(_) | TokenKind::Eof))
        .map(|t| {
            let span = at(t.span);
            Token {
                kind: t.kind.clone(),
                span: hirsel_span::Span::new(base, span.start, span.end).unwrap(),
            }
        })
        .collect();
    let significant: Vec<&Token> = tokens.iter().collect();
    let text = |t: &Token| &source[t.span.start as usize..t.span.end as usize];
    // The entries of the bracketed list opening at significant index `open`:
    // the start of each entry at depth one, except an empty tail.
    let entries = |open: usize, out: &mut Vec<usize>| {
        let mut depth = 0usize;
        for (i, token) in significant.iter().enumerate().skip(open) {
            match text(token) {
                "(" | "[" | "{" => depth += 1,
                ")" | "]" | "}" => {
                    depth -= 1;
                    if depth == 0 {
                        return;
                    }
                }
                _ => {}
            }
            if depth == 1
                && matches!(text(token), "(" | ",")
                && let Some(next) = significant.get(i + 1)
                && text(next) != ")"
            {
                out.push(next.span.start as usize);
            }
        }
    };
    let mut out = vec![];
    for syntax in &file.items {
        let item = &syntax.item;
        out.push(at(item.origin.span).start as usize);
        let after_name = significant
            .iter()
            .position(|t| t.span.start >= at(item.name_span).end)
            .unwrap_or(significant.len());
        let end = at(item.origin.span).end;
        match &item.kind {
            ItemKind::Function { .. } => {
                let mut depth = 0usize;
                for (i, token) in significant.iter().enumerate().skip(after_name) {
                    match text(token) {
                        "[" => depth += 1,
                        "]" => depth -= 1,
                        "(" if depth == 0 => {
                            entries(i, &mut out);
                            break;
                        }
                        _ => {}
                    }
                }
            }
            ItemKind::Type { body, .. } => match body {
                TypeBody::Record(fields) => {
                    out.extend(fields.iter().map(|f| at(f.origin.span).start as usize));
                }
                TypeBody::Distinct { field, .. } => out.push(at(field.origin.span).start as usize),
                TypeBody::Variants(_) => {
                    let mut depth = 0usize;
                    for (i, &t) in significant.iter().enumerate().skip(after_name) {
                        if t.span.start >= end {
                            break;
                        }
                        match text(t) {
                            "drop" | "@" if depth == 0 => break,
                            "(" if depth == 0
                                && i > 0
                                && matches!(significant[i - 1].kind, TokenKind::Ident(_)) =>
                            {
                                entries(i, &mut out);
                                depth += 1;
                            }
                            "(" | "[" | "{" => depth += 1,
                            ")" | "]" | "}" => depth -= 1,
                            _ => {}
                        }
                    }
                }
                _ => {}
            },
            ItemKind::Signature { members } => {
                out.extend(members.iter().map(|m| at(m.origin().span).start as usize));
            }
            ItemKind::Implementation { members, .. } => {
                out.extend(members.iter().map(|m| at(m.origin().span).start as usize));
            }
            ItemKind::Const { .. } => {}
        }
    }
    out.sort();
    out.dedup();
    Ok(out)
}

/// Insert a probe at every site of `source`; its twin gets spaces instead.
pub fn probe(source: &str) -> Result<Probed, String> {
    let sites = sites(source)?;
    let (mut annotated, mut blanked) = (source.to_string(), source.to_string());
    for (n, &at) in sites.iter().enumerate().rev() {
        let probe = format!("@analysis(o5.probe: {n} [site]) ");
        blanked.insert_str(at, &" ".repeat(probe.len()));
        annotated.insert_str(at, &probe);
    }
    Ok(Probed {
        annotated,
        blanked,
        sites: sites.len(),
    })
}

/// What one variant gave: `produce`'s artifacts, with the export's
/// annotations moved out of `tir` into `annotations`, then `hashes` and
/// `evaluation`.
fn observe(path: &Path, expect: &Expect, native: bool) -> Artifacts {
    let config = Config::default();
    let mut artifacts =
        super::produce(path, &config).expect("the default configuration needs no switch");
    if let Some(text) = artifacts.get("tir").cloned() {
        let text = String::from_utf8_lossy(&text).into_owned();
        match split_export(&text) {
            Ok((tir, annotations, roots)) => {
                artifacts.insert("tir".into(), tir.into_bytes());
                artifacts.insert("annotations".into(), annotations.into_bytes());
                artifacts.insert("hashes".into(), hashes(path, roots).into_bytes());
            }
            // An export its own import refuses is compared by that refusal:
            // its text differs by the annotations section it carries.
            Err(error) => {
                artifacts.insert("tir".into(), format!("{error}\n").into_bytes());
            }
        }
    }
    let (status, observation) = crate::compiler::evaluate(path, expect);
    let primary = observation
        .primary
        .map(|p| format!("{} {} {}:{}", p.rule, p.spelling, p.line, p.column));
    if native {
        artifacts.insert("native".into(), native_runs(path).into_bytes());
    }
    artifacts.insert(
        "evaluation".into(),
        format!(
            "{status:?}\n{}\n{}\n{:?}\n{primary:?}\n",
            observation.phase, observation.actual, observation.checked
        )
        .into_bytes(),
    );
    artifacts
}

/// The export re-encoded without its annotations, the annotations as text
/// (the distinct probes exported, the count and digest), and the entry
/// package's definitions, or why the text does not import.
fn split_export(text: &str) -> Result<(String, String, Vec<DefPath>), String> {
    let refused = |e: hirsel_proof_export::ImportError| format!("import: {e:?}");
    let header = hirsel_proof_export::read_header(text).map_err(refused)?;
    let export = hirsel_proof_export::import(
        text,
        &header.environment,
        &hirsel_types::TypeInterner::new(None),
    )
    .map_err(refused)?;
    let mut input = export.to_input();
    let annotations = std::mem::take(&mut input.annotations);
    let roots = export
        .signatures
        .iter()
        .map(|s| s.path.clone())
        .filter(|p| p.0.package == header.entry.0.package)
        .collect();
    // Each instance of a generic generated declaration (CT-5) carries the
    // declaration's annotations, so a probe may be exported more than once.
    let probes: BTreeSet<&String> = annotations
        .iter()
        .filter(|a| a.name == "o5.probe")
        .filter_map(|a| a.payload.first())
        .collect();
    let written = format!(
        "{} probes, {} annotations, digest {}\n",
        probes.len(),
        annotations.len(),
        hirsel_proof_export::findings::spell(&export.annotation_digest)
    );
    Ok((hirsel_proof_export::export(&input), written, roots))
}

/// The LLVM O0 and Cranelift executables' runs, each judged against
/// interpretation, and that reference.
fn native_runs(path: &Path) -> String {
    use crate::native;
    let matrix =
        native::cranelift::matrix(hirsel_driver::native::TargetSpec::x86_64_unknown_linux_gnu());
    let toolchain = match native::runtime_cache().and_then(|cache| {
        hirsel_driver::native::Toolchain::for_matrix(&cache, &matrix).map_err(|e| e.to_string())
    }) {
        Ok(toolchain) => toolchain,
        Err(error) => return format!("toolchain: {error}\n"),
    };
    let source = std::fs::read_to_string(path).unwrap_or_default();
    let dir = path.with_extension("native");
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::create_dir_all(&dir);
    let result = hirsel_engine::with_compiler_stack(|| {
        native::run_source("o5", &source, &matrix, &toolchain, &dir)
    });
    let mut text = format!(
        "snapshot {:?}\nreference {:?}\n",
        result.snapshot, result.reference
    );
    for (name, class, outcome) in &result.outcomes {
        text.push_str(&format!("{name} {class:?} {outcome:?}\n"));
    }
    text
}

/// LANG-6 definition hashes of `roots`, one `path hash` line each.
fn hashes(path: &Path, roots: Vec<DefPath>) -> String {
    hirsel_engine::with_compiler_stack(|| {
        let mut program = match hirsel_driver::load_program_source(path, None) {
            Ok(program) => program,
            Err(error) => return format!("load: {error}\n"),
        };
        if let Err(error) = program.check(hirsel_engine::LoweringPolicy::Reachable) {
            return format!("check: {error:?}\n");
        }
        match program.engine.definition_hashes(roots) {
            Ok(hashes) => hashes
                .hashes
                .iter()
                .map(|(path, hash)| {
                    format!("{path} {}\n", hirsel_proof_export::findings::spell(hash))
                })
                .collect(),
            Err(error) => format!("hashes: {error:?}\n"),
        }
    })
}

/// Compare one annotated program with its twin, both written to `path` in
/// turn so diagnostics name the same file.
pub fn compare(
    path: &Path,
    expect: &Expect,
    annotated: &str,
    blanked: &str,
    sites: usize,
    native: bool,
) -> Verdict {
    let write =
        |text: &str| std::fs::write(path, text).map_err(|e| format!("{}: {e}", path.display()));
    if let Err(why) = write(annotated) {
        return Verdict::Fail(why);
    }
    let with = observe(path, expect, native);
    if let Err(why) = write(blanked) {
        return Verdict::Fail(why);
    }
    let mut without = observe(path, expect, native);
    // Non-vacuous: an exported program carries every probe.
    if let (Some(a), Some(b)) = (with.get("annotations"), without.get("annotations")) {
        let a = String::from_utf8_lossy(a);
        if !a.starts_with(&format!("{sites} probes,")) {
            return Verdict::Fail(format!("the export carries {a} for {sites} probes"));
        }
        if !b.starts_with(b"0 probes,") {
            return Verdict::Fail(format!(
                "the blanked export carries {}",
                String::from_utf8_lossy(b)
            ));
        }
        without.insert("annotations".into(), a.as_bytes().to_vec());
    }
    // Non-vacuous: both executables ran and agreed with interpretation.
    if let Some(runs) = with.get("native") {
        let runs = String::from_utf8_lossy(runs);
        if runs.lines().filter(|l| l.ends_with(" Agree")).count() < 2 {
            return Verdict::Fail(format!("the native runs did not both agree:\n{runs}"));
        }
    }
    match difference(&without, &with) {
        None => Verdict::Pass,
        Some(why) => Verdict::Fail(format!("annotations changed {why}")),
    }
}

/// O5 over `programs`: each single-file program that parses, natively for
/// the first `native` of them that run.
pub fn run(root: &Path, rules: &Rules, programs: &[PathBuf], native: usize, work: &Path) -> Report {
    let mut report = Report::new("O5");
    let mut native_left = native;
    for (index, program) in programs.iter().enumerate() {
        let name = program
            .strip_prefix(root)
            .unwrap_or(program)
            .display()
            .to_string();
        if program.with_extension("hir.json").exists()
            || program.extension().is_none_or(|e| e != "hir")
        {
            continue;
        }
        let Ok(source) = std::fs::read_to_string(program) else {
            report.add(name, Verdict::Fail("unreadable".into()));
            continue;
        };
        let Ok(probed) = probe(&source) else {
            // A program that does not parse has no annotation site.
            continue;
        };
        let expect = match crate::parse_header(Path::new(&name), &source, rules) {
            Ok(header) => header.expect,
            Err(why) => {
                report.add(name, Verdict::Fail(format!("header: {why}")));
                continue;
            }
        };
        let native = native_left > 0 && matches!(expect, Expect::Run { .. });
        native_left -= usize::from(native);
        let dir = work.join(index.to_string());
        if let Err(e) = std::fs::create_dir_all(&dir) {
            report.add(name, Verdict::Fail(e.to_string()));
            continue;
        }
        let path = dir.join(program.file_name().unwrap());
        let verdict = compare(
            &path,
            &expect,
            &probed.annotated,
            &probed.blanked,
            probed.sites,
            native,
        );
        let label = format!(
            "{name} [{} probes{}]",
            probed.sites,
            if native { ", native" } else { "" }
        );
        report.add(label, verdict);
    }
    report
}

//! The spec in docs/spec defines every rule ID exactly once, every corpus
//! header and every rule ID the code cites resolves to that definition, and
//! every relative link in the spec reaches an existing heading or file.
//! It keeps its own test binary: it reads every crate's sources, which only
//! this binary's inputs carry.

hirsel_guard::install!();

use hirsel_conformance::{SPEC_DIR, parse_header, spec_definitions, spec_rules};
use hirsel_diag::RuleId;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn walk(dir: &Path, keep: &dyn Fn(&Path) -> bool, out: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            walk(&path, keep, out);
        } else if keep(&path) {
            out.push(path);
        }
    }
}

/// Lines outside fenced code blocks, with inline code spans removed.
fn prose(text: &str) -> impl Iterator<Item = String> + '_ {
    let mut fenced = false;
    text.lines().filter_map(move |line| {
        if line.starts_with("```") {
            fenced = !fenced;
            return None;
        }
        (!fenced).then(|| line.split('`').step_by(2).collect())
    })
}

/// A GitHub heading anchor: lowercase, punctuation dropped, spaces as hyphens.
fn slug(heading: &str) -> String {
    heading
        .trim()
        .to_lowercase()
        .chars()
        .filter(|c| c.is_alphanumeric() || matches!(c, ' ' | '-' | '_'))
        .map(|c| if c == ' ' { '-' } else { c })
        .collect()
}

fn anchors(text: &str) -> BTreeSet<String> {
    let mut seen = BTreeMap::<String, usize>::new();
    let mut anchors = BTreeSet::new();
    let mut fenced = false;
    for line in text.lines() {
        if line.starts_with("```") {
            fenced = !fenced;
        }
        let Some(heading) = line
            .strip_prefix('#')
            .filter(|_| !fenced)
            .map(|h| h.trim_start_matches('#'))
        else {
            continue;
        };
        let Some(heading) = heading.strip_prefix(' ') else {
            continue;
        };
        let base = slug(heading);
        let count = seen.entry(base.clone()).or_default();
        anchors.insert(match *count {
            0 => base,
            n => format!("{base}-{n}"),
        });
        *count += 1;
    }
    anchors
}

fn spec_parts() -> BTreeMap<String, String> {
    let dir = root().join(SPEC_DIR);
    let mut parts = vec![];
    walk(
        &dir,
        &|p| p.extension().is_some_and(|e| e == "md"),
        &mut parts,
    );
    parts
        .into_iter()
        .map(|p| {
            let name = p.strip_prefix(&dir).unwrap().to_string_lossy().into_owned();
            (name, fs::read_to_string(&p).unwrap())
        })
        .collect()
}

#[test]
fn every_spec_rule_is_defined_once_with_a_statement() {
    let definitions = spec_definitions(&root()).unwrap();
    assert!(definitions.len() > 100, "{} rules", definitions.len());
    for (name, text) in spec_parts() {
        let lines: Vec<String> = prose(&text).collect();
        for (i, line) in lines.iter().enumerate() {
            let Some((id, title)) = line.strip_prefix("### ").and_then(|h| h.split_once(": "))
            else {
                continue;
            };
            if id.contains(' ') {
                continue;
            }
            assert!(!title.trim().is_empty(), "{name}: {id} has no title");
            let statement = lines[i + 1..].iter().find(|l| !l.trim().is_empty());
            assert!(
                statement.is_some_and(|l| !l.starts_with('#')),
                "{name}: {id} has no normative statement"
            );
        }
    }
}

#[test]
fn every_corpus_header_cites_a_defined_rule() {
    let root = root();
    let rules = spec_rules(&root).unwrap();
    let mut files = vec![];
    walk(
        &root.join("conformance"),
        &|p| p.extension().is_some_and(|e| e == "hir"),
        &mut files,
    );
    assert!(files.len() > 1000, "{} corpus programs", files.len());
    let failures: Vec<_> = files
        .iter()
        .filter_map(|file| {
            let source = fs::read_to_string(file).unwrap();
            parse_header(file.strip_prefix(&root).unwrap(), &source, &rules).err()
        })
        .collect();
    assert!(failures.is_empty(), "{failures:#?}");
}

/// Rule IDs the code cites: a defined family prefix, `-`, and a valid number,
/// as a whole word.
fn cited(text: &str, prefixes: &BTreeSet<&str>) -> Vec<String> {
    let word = |c: char| c.is_ascii_alphanumeric() || c == '_';
    let mut ids = vec![];
    for (dash, _) in text.match_indices('-') {
        let start = text[..dash]
            .char_indices()
            .rfind(|(_, c)| !c.is_ascii_uppercase())
            .map_or(0, |(i, c)| i + c.len_utf8());
        let end = dash
            + 1
            + text[dash + 1..]
                .find(|c: char| !c.is_ascii_digit())
                .unwrap_or(text.len() - dash - 1);
        if start == dash
            || end == dash + 1
            || text[..start].chars().next_back().is_some_and(word)
            || text[end..].chars().next().is_some_and(word)
        {
            continue;
        }
        if prefixes.contains(&text[start..dash]) && RuleId::new(&text[start..end]).is_ok() {
            ids.push(text[start..end].to_owned());
        }
    }
    ids
}

#[test]
fn every_rule_id_the_code_cites_is_defined() {
    let root = root();
    let rules: BTreeSet<String> = spec_rules(&root)
        .unwrap()
        .iter()
        .map(|r| r.as_str().to_owned())
        .collect();
    let prefixes: BTreeSet<&str> = rules.iter().map(|r| r.split_once('-').unwrap().0).collect();
    let mut files = vec![];
    let source = |p: &Path| {
        p.extension()
            .is_some_and(|e| ["rs", "hir", "h", "c", "md"].contains(&e.to_str().unwrap()))
    };
    for dir in ["crates", "stdlib"] {
        walk(&root.join(dir), &source, &mut files);
    }
    let mut undefined = BTreeMap::<String, Vec<String>>::new();
    let mut count = 0;
    for file in files {
        let Ok(text) = fs::read_to_string(&file) else {
            continue;
        };
        for id in cited(&text, &prefixes) {
            count += 1;
            // The runner tests use OWN-99 as an ID the spec does not define.
            if !rules.contains(&id) && id != "OWN-99" {
                let at = file.strip_prefix(&root).unwrap().display().to_string();
                undefined.entry(id).or_default().push(at);
            }
        }
    }
    assert!(count > 1000, "{count} cited rule IDs");
    assert!(
        undefined.is_empty(),
        "rule IDs the spec does not define: {undefined:#?}"
    );
}

#[test]
fn every_relative_spec_link_resolves() {
    let root = root();
    let parts = spec_parts();
    let anchors: BTreeMap<_, _> = parts
        .iter()
        .map(|(n, t)| (n.as_str(), anchors(t)))
        .collect();
    let mut links = 0;
    let mut broken = vec![];
    for (name, text) in &parts {
        for line in prose(text) {
            for (i, _) in line.match_indices("](") {
                let rest = &line[i + 2..];
                let target = &rest[..rest.find(')').unwrap()];
                if target.contains("://") || target.starts_with("mailto:") {
                    continue;
                }
                links += 1;
                let (file, fragment) = target.split_once('#').unwrap_or((target, ""));
                let file = if file.is_empty() { name.as_str() } else { file };
                match anchors.get(file) {
                    Some(set) if fragment.is_empty() || set.contains(fragment) => {}
                    Some(_) => broken.push(format!("{name}: {target}")),
                    None if fragment.is_empty() && root.join(SPEC_DIR).join(file).exists() => {}
                    None => broken.push(format!("{name}: {target}")),
                }
            }
        }
    }
    assert!(links > 50, "{links} spec links");
    assert!(
        broken.is_empty(),
        "spec links that resolve nowhere: {broken:#?}"
    );
}

#[test]
fn cited_rejects_a_non_ascii_rule_prefix() {
    assert!(cited("ÖWN-6", &BTreeSet::from(["OWN"])).is_empty());
}

#[test]
fn cited_accepts_an_ascii_rule_id_next_to_utf8() {
    assert_eq!(cited("🐑 OWN-6 é", &BTreeSet::from(["OWN"])), ["OWN-6"]);
}

#[test]
fn rule_id_scanning_and_anchors_follow_their_conventions() {
    let prefixes = BTreeSet::from(["OWN", "PS"]);
    assert_eq!(
        cited("OWN-6, PS-12 and xOWN-7 OWN-8x OWN-01 MEM-5", &prefixes),
        ["OWN-6", "PS-12"]
    );
    assert_eq!(slug("OWN-3: Exclusive access"), "own-3-exclusive-access");
    assert_eq!(
        slug("The export: `hirsel.tir.export.v3`"),
        "the-export-hirseltirexportv3"
    );
    assert!(anchors("## A\n```\n## B\n```\n## A\n").contains("a-1"));
}

#[test]
fn the_spec_hash_covers_every_spec_part() {
    let hashed: BTreeSet<_> = hirsel_engine::SPEC_PARTS
        .iter()
        .map(|(name, _)| *name)
        .collect();
    let parts = spec_parts();
    assert_eq!(hashed, parts.keys().map(String::as_str).collect());
    for (name, text) in hirsel_engine::SPEC_PARTS {
        assert_eq!(text, parts[name].as_bytes(), "{name}");
    }
}

//! Scripted edit scenarios for O2 and O3 (`crates/hirsel-conformance/oracles/`).
//!
//! A scenario is one text file: `//` directive lines, then file sections. Each
//! section starts with a marker line at column 0 and runs to the next marker:
//!
//! - `=== <path>`: a file of the tree before the edit;
//! - `+++ <path>`: the file after the edit (replaced or added);
//! - `--- <path>`: the file is deleted by the edit (no content);
//! - `~~~ <path>`: an in-place edit, as `- <old>` / `+ <new>` line pairs, each
//!   old text occurring exactly once in the file.
//!
//! Directives: `// o2: <what>` or `// o3: <what>` (first line), `// entry:
//! <path>` (default `main.hir`), `// with: std` (a copy of `stdlib/std@1` at
//! `std@1/`, so edits can touch std), `// before:` and `// after:` with
//! `limits=steps=<n|unbounded>,depth=<n>` and `verify=on|off`, and for O3
//! `// unchanged: <names>` and `// recomputed: <names>`.
use super::{Config, Limits};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

#[derive(Clone, Debug)]
pub struct Scenario {
    pub name: String,
    pub oracle: String,
    pub what: String,
    pub entry: String,
    pub with_std: bool,
    pub before: BTreeMap<String, String>,
    pub after: BTreeMap<String, String>,
    pub config_before: Config,
    pub config_after: Config,
    pub unchanged: Vec<String>,
    pub recomputed: Vec<String>,
}

enum Section {
    Base(String),
    Replace(String),
    Delete(String),
    Patch(String),
}

impl Scenario {
    /// The scenario at `path`; std patches read std from the checkout at `root`.
    pub fn read(path: &Path, root: &Path) -> Result<Self, String> {
        let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let name = path.file_stem().unwrap().to_string_lossy().into_owned();
        Self::parse(&name, &text, root).map_err(|e| format!("{}: {e}", path.display()))
    }

    pub fn parse(name: &str, text: &str, root: &Path) -> Result<Self, String> {
        let mut scenario = Scenario {
            name: name.into(),
            oracle: String::new(),
            what: String::new(),
            entry: "main.hir".into(),
            with_std: false,
            before: BTreeMap::new(),
            after: BTreeMap::new(),
            config_before: Config::default(),
            config_after: Config::default(),
            unchanged: vec![],
            recomputed: vec![],
        };
        let mut lines = text.lines().peekable();
        while let Some(line) = lines.next_if(|l| l.starts_with("// ")) {
            let (key, value) = line[3..]
                .split_once(": ")
                .ok_or_else(|| format!("directive without a value: {line}"))?;
            match key {
                "o2" | "o3" if scenario.oracle.is_empty() => {
                    scenario.oracle = key.into();
                    scenario.what = value.into();
                }
                "entry" => scenario.entry = value.into(),
                "with" if value == "std" => scenario.with_std = true,
                "before" => configure(&mut scenario.config_before, value)?,
                "after" => configure(&mut scenario.config_after, value)?,
                "unchanged" => scenario
                    .unchanged
                    .extend(value.split_whitespace().map(String::from)),
                "recomputed" => scenario
                    .recomputed
                    .extend(value.split_whitespace().map(String::from)),
                _ => return Err(format!("unknown directive: {line}")),
            }
        }
        if scenario.oracle.is_empty() {
            return Err("the first line must be `// o2: <what>` or `// o3: <what>`".into());
        }
        let mut sections: Vec<(Section, Vec<&str>)> = vec![];
        for line in lines {
            let marker = |prefix: &str| line.strip_prefix(prefix).map(str::to_string);
            let section = marker("=== ")
                .map(Section::Base)
                .or_else(|| marker("+++ ").map(Section::Replace))
                .or_else(|| marker("--- ").map(Section::Delete))
                .or_else(|| marker("~~~ ").map(Section::Patch));
            match section {
                Some(section) => sections.push((section, vec![])),
                None => match sections.last_mut() {
                    Some((_, body)) => body.push(line),
                    None if line.is_empty() => {}
                    None => return Err(format!("text before the first file: {line}")),
                },
            }
        }
        for (section, body) in &sections {
            match section {
                Section::Base(path) => {
                    if scenario.before.insert(path.clone(), joined(body)).is_some() {
                        return Err(format!("{path}: two base sections"));
                    }
                }
                // A patched std file is part of both trees.
                Section::Patch(path) if scenario.with_std && path.starts_with("std@1/") => {
                    let file = root.join("stdlib").join(path);
                    let text = std::fs::read_to_string(&file)
                        .map_err(|e| format!("{}: {e}", file.display()))?;
                    scenario.before.insert(path.clone(), text);
                }
                _ => {}
            }
        }
        scenario.after = scenario.before.clone();
        for (section, body) in sections {
            match section {
                Section::Base(_) => {}
                Section::Replace(path) => {
                    scenario.after.insert(path, joined(&body));
                }
                Section::Delete(path) => {
                    if scenario.after.remove(&path).is_none() || body.iter().any(|l| !l.is_empty())
                    {
                        return Err(format!(
                            "{path}: a deletion names a file of the tree and has no text"
                        ));
                    }
                }
                Section::Patch(path) => {
                    let mut text = scenario
                        .after
                        .get(&path)
                        .ok_or_else(|| format!("{path}: patch of a file the tree lacks"))?
                        .clone();
                    let mut body = body.iter().filter(|l| !l.is_empty());
                    while let Some(old) = body.next() {
                        let (Some(old), Some(new)) = (
                            old.strip_prefix("- "),
                            body.next().and_then(|l| l.strip_prefix("+ ")),
                        ) else {
                            return Err(format!(
                                "{path}: a patch is `- <old>` then `+ <new>` lines"
                            ));
                        };
                        if text.matches(old).count() != 1 {
                            return Err(format!("{path}: {old:?} does not occur exactly once"));
                        }
                        text = text.replacen(old, new, 1);
                    }
                    scenario.after.insert(path, text);
                }
            }
        }
        if scenario.before == scenario.after && scenario.config_before == scenario.config_after {
            return Err("the edit changes nothing".into());
        }
        Ok(scenario)
    }

    /// Write the tree before (`after == false`) or after the edit into `dir`,
    /// removing what the other tree had and this one lacks. With `with: std`,
    /// `dir/std@1` starts as a copy of the checkout's std.
    pub fn write(&self, dir: &Path, root: &Path, after: bool) -> Result<(), String> {
        let (tree, other) = if after {
            (&self.after, &self.before)
        } else {
            (&self.before, &self.after)
        };
        let io = |path: &Path, e: std::io::Error| format!("{}: {e}", path.display());
        if self.with_std && !dir.join("std@1").exists() {
            copy_dir(&root.join("stdlib/std@1"), &dir.join("std@1"))?;
        }
        for path in other.keys().filter(|p| !tree.contains_key(*p)) {
            let path = dir.join(path);
            if path.exists() {
                std::fs::remove_file(&path).map_err(|e| io(&path, e))?;
            }
        }
        for (path, text) in tree {
            let path = dir.join(path);
            std::fs::create_dir_all(path.parent().unwrap()).map_err(|e| io(&path, e))?;
            std::fs::write(&path, text).map_err(|e| io(&path, e))?;
        }
        Ok(())
    }

    /// The program to compile in a tree written to `dir`.
    pub fn entry(&self, dir: &Path) -> PathBuf {
        dir.join(&self.entry)
    }
}

/// A section's text: its lines without trailing blank lines, ending in a newline.
fn joined(body: &[&str]) -> String {
    let end = body
        .iter()
        .rposition(|l| !l.is_empty())
        .map_or(0, |i| i + 1);
    let mut text = body[..end].join("\n");
    text.push('\n');
    text
}

fn configure(config: &mut Config, value: &str) -> Result<(), String> {
    for setting in value.split_whitespace() {
        match setting.split_once('=') {
            Some(("limits", limits)) => config.limits = Some(limits.parse::<Limits>()?),
            Some(("verify", "on")) => config.verify = true,
            Some(("verify", "off")) => config.verify = false,
            _ => return Err(format!("unknown setting: {setting}")),
        }
    }
    Ok(())
}

fn copy_dir(from: &Path, to: &Path) -> Result<(), String> {
    std::fs::create_dir_all(to).map_err(|e| format!("{}: {e}", to.display()))?;
    for entry in std::fs::read_dir(from).map_err(|e| format!("{}: {e}", from.display()))? {
        let path = entry.map_err(|e| e.to_string())?.path();
        let target = to.join(path.file_name().unwrap());
        if path.is_dir() {
            copy_dir(&path, &target)?;
        } else {
            // Pool inputs are read-only. This is an editable fixture, so copy
            // bytes into a fresh writable file instead of preserving permissions.
            let bytes = std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
            std::fs::write(&target, bytes).map_err(|e| format!("{}: {e}", target.display()))?;
        }
    }
    Ok(())
}

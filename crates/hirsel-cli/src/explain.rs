//! Catalogue-only dispatch, before any compiler or workspace initialization.
//!
//! Structure: rust a7c37f9f0ee5e93396106e9de20f4a86b4154e63,
//! compiler/rustc_driver_impl/src/lib.rs:208-211,427-473 and
//! compiler/rustc_errors/src/codes.rs:29-58. See docs/rule-explain.md.

use hirsel_cli_args::lexopt::{self, Arg::*, Parser};
use hirsel_diag::{RuleGuidance, RuleId};
use std::{ffi::OsString, io::Write};

const USAGE: &str = "usage: hirsel explain RULE [--json] | hirsel explain --list [--json]\nSee docs/rule-explain.md";

struct Options {
    rule: Option<RuleId>,
    json: bool,
}

fn parse(args: &[OsString]) -> Result<Options, lexopt::Error> {
    let mut args = Parser::from_args(args.iter().cloned());
    let (mut rule, mut list, mut json) = (None, false, false);
    while let Some(arg) = args.next()? {
        match arg {
            Long("list") if !list && rule.is_none() => list = true,
            Long("json") if !json => json = true,
            Value(value) if rule.is_none() && !list => {
                let value = value.into_string().map_err(|_| "rule ID must be UTF-8")?;
                rule = Some(RuleId::new(value).map_err(|error| error.to_string())?);
            }
            arg => return Err(arg.unexpected()),
        }
    }
    if rule.is_none() && !list {
        return Err("explain requires a rule ID or --list".into());
    }
    Ok(Options { rule, json })
}

fn json_entry(entry: &RuleGuidance) -> serde_json::Value {
    serde_json::json!({
        "rule": entry.rule,
        "title": entry.title,
        "meaning": entry.meaning,
        "repair": entry.repair.map(|s| s.strip_prefix("help: ").unwrap_or(s)),
        "related": entry.related.iter().map(json_entry).collect::<Vec<_>>()
    })
}

fn render_text(entry: &RuleGuidance) -> String {
    let mut text = format!("{}: {}\n\n{}\n\n", entry.rule, entry.title, entry.meaning);
    match entry.repair {
        Some(repair) => text.push_str(&format!(
            "Typical repair (check its conditions): {}\n",
            repair.strip_prefix("help: ").unwrap_or(repair)
        )),
        None => text.push_str("No specific repair guidance is catalogued for this rule.\n"),
    }
    for related in &entry.related {
        text.push_str(&format!("\n{}", render_text(related)));
    }
    text
}

fn render(options: Options) -> Result<String, String> {
    let catalogue = hirsel_diag::rule_guidance();
    if let Some(rule) = options.rule {
        let entry = catalogue
            .iter()
            .find(|entry| entry.rule == rule.as_str())
            .ok_or_else(|| {
                format!("no catalogued guidance for rule {rule}; use hirsel explain --list")
            })?;
        Ok(if options.json {
            let mut value = json_entry(entry);
            value["schema_version"] = 1.into();
            format!("{value}\n")
        } else {
            render_text(entry)
        })
    } else if options.json {
        Ok(format!(
            "{}\n",
            serde_json::json!({"schema_version": 1, "rules": catalogue.iter().map(|entry| entry.rule).collect::<Vec<_>>()})
        ))
    } else {
        Ok(catalogue
            .iter()
            .map(|entry| format!("{}\n", entry.rule))
            .collect())
    }
}

pub fn execute(args: &[OsString]) -> u8 {
    let result = parse(args)
        .map_err(|error| error.to_string())
        .and_then(render);
    match result {
        Ok(output) => match std::io::stdout().lock().write_all(output.as_bytes()) {
            Ok(()) => 0,
            Err(error) => {
                eprintln!("hirsel: explain: {error}");
                2
            }
        },
        Err(error) => {
            eprintln!("hirsel: explain: {error}\n{USAGE}");
            2
        }
    }
}

#[cfg(test)]
mod rule_explain_tests {
    use super::*;

    #[test]
    fn json_escapes_catalogue_strings_and_absent_repair_is_explicit() {
        let entry = RuleGuidance {
            rule: "A-1",
            title: "\"quoted\"\\title",
            meaning: "line\n\t\r\0 λ",
            repair: None,
            related: vec![],
        };
        let encoded = json_entry(&entry).to_string();
        let value: serde_json::Value = serde_json::from_str(&encoded).unwrap();
        assert_eq!(value["title"], entry.title);
        assert_eq!(value["meaning"], entry.meaning);
        assert!(value["repair"].is_null());
        assert!(render_text(&entry).contains("No specific repair guidance"));
        assert!(!encoded.contains('\0'));
        assert!(!encoded.contains('\n'));
    }
}

// Query a JSON-lines or plain log: filter by level, message substring and JSON field,
// then print counts per level and the most frequent messages.
//
// usage: logq [--level LEVEL] [--match TEXT] [--field KEY=VALUE] [--top N] FILE
use serde_json::{Map, Value};
use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::fs::File;
use std::io::{BufRead, BufReader, Read};

#[derive(Debug, PartialEq)]
pub struct Query {
    level: Option<String>,
    pattern: Option<String>,
    field: Option<(String, String)>,
    top: u64,
    file: String,
}

#[derive(Debug, PartialEq)]
pub struct Entry {
    level: String,
    message: String,
    fields: Map<String, Value>,
}

pub struct Ranked {
    message: String,
    count: u64,
}

const USAGE: &str = "usage: logq [--level LEVEL] [--match TEXT] [--field KEY=VALUE] [--top N] FILE";

const MAX_LINE: u64 = 65536;

fn flag_value(args: &[String], i: usize, flag: &str) -> Result<String, String> {
    if i >= args.len() {
        return Err(format!("{flag} needs a value"));
    }
    Ok(args[i].clone())
}

fn parse_args(args: &[String]) -> Result<Query, String> {
    let mut level: Option<String> = None;
    let mut pattern: Option<String> = None;
    let mut field: Option<(String, String)> = None;
    let mut top = 5u64;
    let mut file: Option<String> = None;
    let mut i = 0;
    while i < args.len() {
        let text = &args[i];
        i += 1;
        if text == "--level" {
            level = Some(flag_value(args, i, text)?.to_ascii_uppercase());
            i += 1;
        } else if text == "--match" {
            pattern = Some(flag_value(args, i, text)?);
            i += 1;
        } else if text == "--field" {
            let value = flag_value(args, i, text)?;
            let Some((key, wanted)) = value.split_once('=') else {
                return Err("--field needs KEY=VALUE".into());
            };
            field = Some((key.to_string(), wanted.to_string()));
            i += 1;
        } else if text == "--top" {
            let Ok(n) = flag_value(args, i, text)?.parse::<u64>() else {
                return Err("--top needs a number".into());
            };
            top = n;
            i += 1;
        } else if text.starts_with("--") {
            return Err(format!("unknown flag {text}"));
        } else if file.is_some() {
            return Err(USAGE.into());
        } else {
            file = Some(text.clone());
        }
    }
    match file {
        Some(f) => Ok(Query { level, pattern, field, top, file: f }),
        None => Err(USAGE.into()),
    }
}

fn text_member(fields: &Map<String, Value>, key: &str) -> Option<String> {
    match fields.get(key) {
        Some(Value::String(s)) => Some(s.clone()),
        Some(Value::Number(n)) => Some(n.to_string()),
        Some(Value::Bool(b)) => Some(if *b { "true" } else { "false" }.to_string()),
        _ => None,
    }
}

// A line starting with `{` is a JSON object with `level` and `msg` (or `message`)
// members; any other line is `LEVEL message...`.
fn parse_line(number: u64, line: &str) -> Result<Option<Entry>, String> {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    if trimmed.starts_with('{') {
        let fields = match serde_json::from_str(trimmed) {
            Ok(Value::Object(fields)) => fields,
            _ => return Err(format!("line {number}: invalid JSON object")),
        };
        let level = match text_member(&fields, "level") {
            Some(l) => l.to_ascii_uppercase(),
            None => "UNKNOWN".to_string(),
        };
        let message = text_member(&fields, "msg")
            .unwrap_or_else(|| text_member(&fields, "message").unwrap_or_default());
        return Ok(Some(Entry { level, message, fields }));
    }
    let words: Vec<&str> = trimmed.split_ascii_whitespace().collect();
    let level = words[0].to_ascii_uppercase();
    let rest = match trimmed.split_once(' ') {
        Some((_, after)) => after.trim().to_string(),
        None => String::new(),
    };
    Ok(Some(Entry { level, message: rest, fields: Map::new() }))
}

fn keep(query: &Query, entry: &Entry) -> bool {
    if let Some(l) = &query.level {
        if &entry.level != l {
            return false;
        }
    }
    if let Some(p) = &query.pattern {
        if !entry.message.contains(p.as_str()) {
            return false;
        }
    }
    match &query.field {
        Some((key, wanted)) => match text_member(&entry.fields, key) {
            Some(v) => &v == wanted,
            None => false,
        },
        None => true,
    }
}

fn bump(counts: &mut BTreeMap<String, u64>, key: String) {
    let n = counts.get(&key).copied().unwrap_or(0);
    counts.insert(key, n + 1);
}

fn by_count(a: &Ranked, b: &Ranked) -> Ordering {
    if a.count > b.count {
        return Ordering::Less;
    }
    if a.count < b.count {
        return Ordering::Greater;
    }
    a.message.cmp(&b.message)
}

fn report(query: &Query, levels: &BTreeMap<String, u64>, messages: &BTreeMap<String, u64>) -> String {
    let mut names: Vec<String> = Vec::new();
    for name in levels.keys() {
        names.push(name.clone());
    }
    names.sort();
    let mut out = String::new();
    let mut total = 0u64;
    for n in levels.values() {
        total += n;
    }
    out.push_str(&format!("matched {total}\n"));
    for name in &names {
        let n = levels.get(name).copied().unwrap_or(0);
        out.push_str(&format!("{name} {n}\n"));
    }
    let mut ranked: Vec<Ranked> = Vec::new();
    for (message, n) in messages {
        ranked.push(Ranked { message: message.clone(), count: *n });
    }
    ranked.sort_by(by_count);
    out.push_str("top messages:\n");
    let mut shown = 0u64;
    for r in &ranked {
        if shown == query.top {
            break;
        }
        shown += 1;
        out.push_str(&format!("  {} {}\n", r.count, r.message));
    }
    out
}

fn read_line(reader: &mut BufReader<File>, file: &str, number: u64) -> Result<Option<String>, String> {
    let mut line = Vec::new();
    let read = reader
        .by_ref()
        .take(MAX_LINE + 1)
        .read_until(b'\n', &mut line)
        .map_err(|_| format!("cannot read {file}"))?;
    if read == 0 {
        return Ok(None);
    }
    if line.len() as u64 > MAX_LINE {
        return Err(format!("line {}: too long", number + 1));
    }
    if line.last() == Some(&b'\n') {
        line.pop();
    }
    String::from_utf8(line)
        .map(Some)
        .map_err(|_| format!("line {}: invalid UTF-8", number + 1))
}

fn run(args: &[String]) -> Result<String, String> {
    let query = parse_args(args)?;
    let f = match File::open(&query.file) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(format!("no such file: {}", query.file));
        }
        Err(_) => return Err(format!("cannot open {}", query.file)),
    };
    let mut reader = BufReader::with_capacity(4096, f);
    let mut levels: BTreeMap<String, u64> = BTreeMap::new();
    let mut messages: BTreeMap<String, u64> = BTreeMap::new();
    let mut number = 0u64;
    while let Some(line) = read_line(&mut reader, &query.file, number)? {
        number += 1;
        let Some(entry) = parse_line(number, &line)? else {
            continue;
        };
        if keep(&query, &entry) {
            bump(&mut levels, entry.level.clone());
            bump(&mut messages, entry.message.clone());
        }
    }
    Ok(report(&query, &levels, &messages))
}

pub fn main(args: &[String]) -> Result<(), String> {
    match run(args) {
        Ok(text) => {
            print!("{text}");
            Ok(())
        }
        Err(message) => Err(message),
    }
}

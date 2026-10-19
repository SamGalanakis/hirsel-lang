// Print a table of contents for a Markdown file: one nested list item per ATX
// heading outside fenced code, linking to its GitHub-style anchor.
//
// usage: mdtoc FILE
use std::collections::BTreeMap;
use std::fs;
use unicode_normalization::UnicodeNormalization;

#[derive(Debug, PartialEq)]
pub struct Heading {
    level: usize,
    title: String,
}

// Lowercase NFC text with Unicode simple case mapping. Spaces become `-`,
// `-` and `_` stay, other ASCII punctuation is dropped. Non-ASCII scalars stay.
fn slug(title: &str) -> String {
    let mut out = String::new();
    for scalar in title.nfc() {
        let mut lower = scalar.to_lowercase();
        let c = if lower.len() == 1 { lower.next().unwrap_or(scalar) } else { scalar };
        match c {
            'a'..='z' | '0'..='9' | '-' | '_' | '\u{80}'..='\u{10FFFF}' => out.push(c),
            ' ' => out.push('-'),
            _ => (),
        }
    }
    out
}

// `#` to `######`, then a space or the end of the line; the closing `#`
// sequence and surrounding spaces are not part of the title.
fn heading(line: &str) -> Option<Heading> {
    let bytes = line.as_bytes();
    let mut level = 0;
    while level < bytes.len() && bytes[level] == b'#' {
        level += 1;
    }
    if level == 0 || level > 6 {
        return None;
    }
    if level < bytes.len() && !blank(bytes[level]) {
        return None;
    }
    let mut end = bytes.len();
    while end > level && blank(bytes[end - 1]) {
        end -= 1;
    }
    let mut closing = end;
    while closing > level && bytes[closing - 1] == b'#' {
        closing -= 1;
    }
    if closing == level || blank(bytes[closing - 1]) {
        end = closing;
    }
    let raw = match std::str::from_utf8(&bytes[level..end]) {
        Ok(s) => s,
        Err(_) => return None,
    };
    Some(Heading { level, title: raw.trim().to_string() })
}

// A space or a tab.
fn blank(byte: u8) -> bool {
    matches!(byte, b' ' | b'\t')
}

// A fence opens with three or more backticks or tildes and closes with the same.
fn fence(line: &str) -> Option<&'static str> {
    let trimmed = line.trim();
    if trimmed.starts_with("```") {
        return Some("```");
    }
    if trimmed.starts_with("~~~") {
        return Some("~~~");
    }
    None
}

fn headings(text: &str) -> Vec<Heading> {
    let mut found = Vec::new();
    let mut open: Option<&str> = None;
    for raw in text.split('\n') {
        let line = match raw.ends_with('\r') {
            true => match raw.split_once('\r') {
                Some((content, _)) => content,
                None => raw,
            },
            false => raw,
        };
        if let Some(marker) = open {
            if line.trim().starts_with(marker) {
                open = None;
            }
            continue;
        }
        if let Some(marker) = fence(line) {
            open = Some(marker);
            continue;
        }
        if let Some(h) = heading(line) {
            found.push(h);
        }
    }
    found
}

fn toc(found: &[Heading]) -> String {
    let mut top = 6;
    for h in found {
        if h.level < top {
            top = h.level;
        }
    }
    let mut seen: BTreeMap<String, u64> = BTreeMap::new();
    let mut out = String::new();
    for h in found {
        let base = slug(&h.title);
        let anchor = match seen.get(&base) {
            Some(n) => format!("{base}-{n}"),
            None => base.clone(),
        };
        let count = seen.get(&base).copied().unwrap_or(0) + 1;
        seen.insert(base, count);
        for _ in top..h.level {
            out.push_str("  ");
        }
        out.push_str(&format!("- [{}](#{anchor})\n", h.title));
    }
    out
}

fn run(args: &[String]) -> Result<String, String> {
    if args.len() != 1 {
        return Err("usage: mdtoc FILE".into());
    }
    let name = &args[0];
    let data = match fs::read(name) {
        Ok(d) => d,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(format!("no such file: {name}"));
        }
        Err(_) => return Err(format!("cannot read {name}")),
    };
    let Ok(text) = String::from_utf8(data) else {
        return Err(format!("{name} is not UTF-8"));
    };
    Ok(toc(&headings(&text)))
}

pub fn main(args: &[String]) -> Result<(), String> {
    let text = run(args)?;
    print!("{text}");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn anchors_normalize_unicode_and_punctuation() {
        assert_eq!(slug("Hello, World!"), "hello-world");
        assert_eq!(slug("CAFÉ"), "café");
    }

    #[test]
    fn headings_require_a_separator() {
        let found = heading("## title ##").unwrap();
        assert_eq!(found.level, 2);
        assert_eq!(found.title, "title");
        assert!(heading("##not-a-heading").is_none());
    }
}

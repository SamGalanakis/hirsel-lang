//! Comparison helper: canonical type rows, numbered at first use in the stable dump.
//! All type fields are retained. Equal recursive graphs share a comparison number;
//! operational errors and source diagnostics never pass through this helper.
use hirsel_types::{TypeId, TypeInterner};
use std::collections::{BTreeMap, HashMap};

fn replace_ids(text: &str, compact: bool, mut number: impl FnMut(usize) -> usize) -> String {
    let bytes = text.as_bytes();
    let mut out = String::new();
    let mut start = 0;
    let mut i = 0;
    let mut quoted = false;
    while i < bytes.len() {
        if bytes[i] == b'"' {
            quoted = !quoted;
            i += 1;
            continue;
        }
        if quoted {
            if bytes[i] == b'\\' {
                i += 1;
            }
            i += 1;
            continue;
        }
        let boundary = i == 0 || !(bytes[i - 1].is_ascii_alphanumeric() || bytes[i - 1] == b'_');
        let (prefix, suffix) = if boundary && bytes[i..].starts_with(b"TypeId(") {
            (7, true)
        } else if compact && boundary && bytes[i] == b't' {
            (1, false)
        } else {
            i += 1;
            continue;
        };
        let digits = i + prefix;
        let mut end = digits;
        while end < bytes.len() && bytes[end].is_ascii_digit() {
            end += 1;
        }
        if end == digits
            || (suffix && bytes.get(end) != Some(&b')'))
            || (!suffix
                && bytes
                    .get(end)
                    .is_some_and(|c| c.is_ascii_alphanumeric() || *c == b'_'))
        {
            i += 1;
            continue;
        }
        let id = text[digits..end].parse().unwrap();
        out.push_str(&text[start..digits]);
        out.push_str(&number(id).to_string());
        if suffix {
            out.push(')');
            end += 1;
        }
        start = end;
        i = end;
    }
    out.push_str(&text[start..]);
    out
}
fn classes(labels: Vec<String>) -> (Vec<usize>, usize) {
    let mut names: BTreeMap<_, _> = labels.iter().cloned().map(|s| (s, 0)).collect();
    for (i, number) in names.values_mut().enumerate() {
        *number = i;
    }
    let count = names.len();
    (labels.iter().map(|s| names[s]).collect(), count)
}
/// `text` names types by position in `ids` (`tN`); rows name their edges by
/// session id (`TypeId(N)`), renumbered here to positions too.
pub fn canonical_text(t: &TypeInterner, ids: &[TypeId], text: &str) -> String {
    let position: HashMap<usize, usize> = ids
        .iter()
        .enumerate()
        .map(|(i, id)| {
            (
                format!("{id:?}")[7..]
                    .trim_end_matches(')')
                    .parse()
                    .unwrap(),
                i,
            )
        })
        .collect();
    let rows: Vec<_> = ids
        .iter()
        .map(|&id| replace_ids(&format!("{:?}", t.row(id)), false, |raw| position[&raw]))
        .collect();
    let (mut partition, mut count) = classes(
        rows.iter()
            .map(|row| replace_ids(row, false, |_| 0))
            .collect(),
    );
    loop {
        let (next, n) = classes(
            rows.iter()
                .map(|row| replace_ids(row, false, |i| partition[i]))
                .collect(),
        );
        assert!(n >= count, "type partition must refine");
        partition = next;
        if n == count {
            break;
        }
        count = n;
    }
    let mut representatives = vec![0; count];
    for (i, class) in partition.iter().copied().enumerate().rev() {
        representatives[class] = i;
    }
    let mut numbers = BTreeMap::new();
    let mut queue = vec![];
    let mut assign = |i: usize| {
        let class = partition[i];
        let next = numbers.len();
        *numbers.entry(class).or_insert_with(|| {
            queue.push(class);
            next
        })
    };
    // The definition name is unquoted and may itself be named t123.
    let (heading, rest) = text.split_once('\n').unwrap_or((text, ""));
    let mut output = format!("{heading}\n{}", replace_ids(rest, true, &mut assign));
    let mut cursor = 0;
    while cursor < queue.len() {
        let class = queue[cursor];
        let row = &rows[representatives[class]];
        let normalized = replace_ids(row, false, |i| {
            let class = partition[i];
            let next = numbers.len();
            *numbers.entry(class).or_insert_with(|| {
                queue.push(class);
                next
            })
        });
        output.push_str(&format!("type t{cursor}: {normalized}\n"));
        cursor += 1;
    }
    output
}
pub fn definition(t: &TypeInterner, definition: &hirsel_tir::Definition) -> String {
    canonical_text(
        t,
        &definition.types,
        &hirsel_lower::print::definition(definition),
    )
}

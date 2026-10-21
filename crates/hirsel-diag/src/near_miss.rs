//! Name similarity, ported from rustc (MIT OR Apache-2.0).
//! https://github.com/rust-lang/rust/blob/a7c37f9f0ee5e93396106e9de20f4a86b4154e63/compiler/rustc_span/src/edit_distance.rs
//! Source lines 24-104: restricted Damerau-Levenshtein, including buffer rotation.
//! Lines 235-334: case-first ranking, max(char_count, 3)/3 threshold and sorted words.
//! Adaptations: strings instead of rustc Symbols; keep up to three tied best names;
//! ignore underscores alongside case; recognize count -> len only when len exists.
use crate::{Diagnostic, Fix};
use hirsel_span::Span;
use std::{cmp, mem};

fn edit_distance(a: &str, b: &str, limit: usize) -> Option<usize> {
    let mut a = &a.chars().collect::<Vec<_>>()[..];
    let mut b = &b.chars().collect::<Vec<_>>()[..];

    // Ensure that `b` is the shorter string, minimizing memory use.
    if a.len() < b.len() {
        mem::swap(&mut a, &mut b);
    }

    let min_dist = a.len() - b.len();
    // If we know the limit will be exceeded, we can return early.
    if min_dist > limit {
        return None;
    }

    // Strip common prefix.
    while let Some(((b_char, b_rest), (a_char, a_rest))) = b.split_first().zip(a.split_first())
        && a_char == b_char
    {
        a = a_rest;
        b = b_rest;
    }
    // Strip common suffix.
    while let Some(((b_char, b_rest), (a_char, a_rest))) = b.split_last().zip(a.split_last())
        && a_char == b_char
    {
        a = a_rest;
        b = b_rest;
    }

    // If either string is empty, the distance is the length of the other.
    // We know that `b` is the shorter string, so we don't need to check `a`.
    if b.is_empty() {
        return Some(min_dist);
    }

    let mut prev_prev = vec![usize::MAX; b.len() + 1];
    let mut prev = (0..=b.len()).collect::<Vec<_>>();
    let mut current = vec![0; b.len() + 1];

    // row by row
    for i in 1..=a.len() {
        current[0] = i;
        let a_idx = i - 1;

        // column by column
        for j in 1..=b.len() {
            let b_idx = j - 1;

            // There is no cost to substitute a character with itself.
            let substitution_cost = if a[a_idx] == b[b_idx] { 0 } else { 1 };

            current[j] = cmp::min(
                // deletion
                prev[j] + 1,
                cmp::min(
                    // insertion
                    current[j - 1] + 1,
                    // substitution
                    prev[j - 1] + substitution_cost,
                ),
            );

            if (i > 1) && (j > 1) && (a[a_idx] == b[b_idx - 1]) && (a[a_idx - 1] == b[b_idx]) {
                // transposition
                current[j] = cmp::min(current[j], prev_prev[j - 2] + 1);
            }
        }

        // Rotate the buffers, reusing the memory.
        [prev_prev, prev, current] = [prev, current, prev_prev];
    }

    // `prev` because we already rotated the buffers.
    let distance = prev[b.len()];
    (distance <= limit).then_some(distance)
}

fn normalized(name: &str) -> String {
    name.chars()
        .filter(|c| *c != '_')
        .flat_map(char::to_uppercase)
        .collect()
}
fn sorted_words(name: &str) -> Vec<&str> {
    let mut words: Vec<_> = name.split('_').collect();
    words.sort_unstable();
    words
}

/// Best accessible spellings, in deterministic order. Retain ties so ambiguity
/// never produces a machine fix. A distant candidate cannot fill a spare slot.
fn best_names(name: &str, candidates: impl IntoIterator<Item = String>) -> Vec<String> {
    let limit = cmp::max(name.chars().count(), 3) / 3;
    let normalized_name = normalized(name);
    let mut ranked: Vec<_> = candidates
        .into_iter()
        .filter(|c| c != name)
        .filter_map(|candidate| {
            let rank = if normalized(&candidate) == normalized_name {
                (0, 0)
            } else if name == "count" && candidate == "len" {
                (1, 0)
            } else if let Some(distance) = edit_distance(name, &candidate, limit) {
                (2, distance)
            } else if sorted_words(name) == sorted_words(&candidate) {
                (3, 0)
            } else {
                return None;
            };
            Some((rank, candidate))
        })
        .collect();
    ranked.sort();
    ranked.dedup();
    let Some((best, _)) = ranked.first() else {
        return vec![];
    };
    let best = *best;
    ranked
        .into_iter()
        .take_while(|(rank, _)| *rank == best)
        .take(3)
        .map(|(_, name)| name)
        .collect()
}

/// First fresh spelling admitted by the caller's syntax and scope rules.
/// Declaration repairs must avoid capturing existing references as well as
/// duplicate bindings; the caller supplies that contextual admission check.
pub fn fresh_name(base: &str, mut available: impl FnMut(&str) -> bool) -> String {
    if available(base) {
        return base.into();
    }
    for suffix in 2u64.. {
        let name = format!("{base}_{suffix}");
        if available(&name) {
            return name;
        }
    }
    unreachable!("a finite source cannot occupy every suffix")
}

/// Add human/JSON help and, for one candidate at a precise name token, a fix.
/// `prefix` is presentation only (e.g. `x.`); the edit replaces just the name.
pub fn suggest_names(
    diagnostic: &mut Diagnostic,
    name: &str,
    candidates: impl IntoIterator<Item = String>,
    prefix: &str,
    name_span: Option<Span>,
) {
    let suggestions = best_names(name, candidates);
    if suggestions.is_empty() {
        return;
    }
    diagnostic
        .notes
        .retain(|note| !note.starts_with("help: did you mean "));
    diagnostic.fix = None;
    diagnostic.notes.push(format!(
        "help: did you mean {}?",
        suggestions
            .iter()
            .map(|n| format!("`{prefix}{n}`"))
            .collect::<Vec<_>>()
            .join(" or ")
    ));
    if suggestions.len() == 1
        && let Some(span) = name_span
    {
        diagnostic.fix = Some(Fix {
            span,
            replacement: suggestions[0].clone(),
            message: "use the suggested name".into(),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn best(name: &str, candidates: &[&str]) -> Vec<String> {
        best_names(name, candidates.iter().map(|s| (*s).into()))
    }
    #[test]
    fn declaration_candidates_are_filtered_before_selection() {
        let occupied = ["result", "value", "value_2"];
        assert_eq!(fresh_name("result", |n| !occupied.contains(&n)), "result_2");
        assert_eq!(fresh_name("value", |n| !occupied.contains(&n)), "value_3");
    }
    #[test]
    fn rustc_distance_vectors() {
        for c in (0..char::MAX as u32).filter_map(char::from_u32) {
            let s = c.to_string();
            assert_eq!(edit_distance(&s, &s, 0), Some(0));
        }
        let a = "\nMäry häd ä little lämb\n\nLittle lämb\n";
        let b = "\nMary häd ä little lämb\n\nLittle lämb\n";
        let c = "Mary häd ä little lämb\n\nLittle lämb\n";
        for (x, y, d) in [
            (a, b, 1),
            (b, a, 1),
            (a, c, 2),
            (c, a, 2),
            (b, c, 1),
            (c, b, 1),
            ("abc", "abcd", 1),
            ("abc", "xyz", 3),
            ("ab", "ba", 1),
            ("abde", "bcaed", 4),
        ] {
            assert_eq!(edit_distance(x, y, d), Some(d));
            assert_eq!(edit_distance(x, y, d - 1), None);
        }
    }
    #[test]
    fn rustc_matching_vectors_and_extensions() {
        assert_eq!(best("aaaa", &["aaab", "aaabc"]), ["aaab"]);
        assert!(best("1111111111", &["aaab", "aaabc"]).is_empty());
        assert_eq!(best("aaaa", &["AAAA"]), ["AAAA"]);
        assert_eq!(
            best("a_variable_longer_name", &["a_longer_variable_name"]),
            ["a_longer_variable_name"]
        );
        assert_eq!(best("isEmpty", &["is_empty"]), ["is_empty"]);
        assert_eq!(best("count", &["len", "mount"]), ["len"]);
        assert!(best("count", &["length"]).is_empty());
        assert_eq!(
            best("cat", &["cut", "bat", "car", "cap", "bat", "distant"]),
            ["bat", "cap", "car"]
        );
        assert_eq!(best("lne", &["len", "line"]), ["len", "line"]);
        assert!(best("a", &["long_name"]).is_empty());
    }
    #[test]
    fn ambiguous_names_never_get_a_fix() {
        let mut d = Diagnostic::error(crate::RuleId::of("LANG-4"), Span::default(), "missing");
        suggest_names(
            &mut d,
            "cat",
            ["bat".into(), "cut".into()],
            "x.",
            Some(Span::default()),
        );
        assert_eq!(d.notes, ["help: did you mean `x.bat` or `x.cut`?"]);
        assert!(d.fix.is_none());
        suggest_names(&mut d, "count", ["len".into()], "x.", Some(Span::default()));
        assert_eq!(d.fix.unwrap().replacement, "len");
    }
}

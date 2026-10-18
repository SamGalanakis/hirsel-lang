//! Anchored unified diffs for snapshot failures, adapted from Go's internal/diff
//! (commit ebe2feefc400f813aefdc8a036e66d2619da6649; BSD-3-Clause, third-party/NOTICES.md),
//! with its testdata. The chosen matches are the longest common subsequence of lines that
//! occur exactly once in each text (Szymanski's Algorithm A), so the diff is deterministic,
//! runs in O(n log n) and never anchors on repeated blank lines or braces.
use std::collections::HashMap;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct Pair {
    x: usize,
    y: usize,
}

/// The unified diff of `old` and `new` with three lines of context, or None when they are
/// equal. A last line without a newline is marked as BSD and GNU diff mark it.
pub fn unified(old_name: &str, old: &str, new_name: &str, new: &str) -> Option<String> {
    if old == new {
        return None;
    }
    const C: usize = 3;
    let x = lines(old);
    let y = lines(new);
    let mut out = format!("diff {old_name} {new_name}\n--- {old_name}\n+++ {new_name}\n");
    let mut done = Pair::default();
    let mut chunk = Pair::default();
    let mut count = Pair::default();
    let mut text: Vec<String> = Vec::new();
    for m in tgs(&x, &y) {
        if m.x < done.x {
            continue;
        }
        let mut start = m;
        while start.x > done.x && start.y > done.y && x[start.x - 1] == y[start.y - 1] {
            start.x -= 1;
            start.y -= 1;
        }
        let mut end = m;
        while end.x < x.len() && end.y < y.len() && x[end.x] == y[end.y] {
            end.x += 1;
            end.y += 1;
        }
        for s in &x[done.x..start.x] {
            text.push(format!("-{s}"));
            count.x += 1;
        }
        for s in &y[done.y..start.y] {
            text.push(format!("+{s}"));
            count.y += 1;
        }
        // Too few common lines before the end: the chunk keeps them and continues.
        if (end.x < x.len() || end.y < y.len())
            && (end.x - start.x < C || (!text.is_empty() && end.x - start.x < 2 * C))
        {
            for s in &x[start.x..end.x] {
                text.push(format!(" {s}"));
                count.x += 1;
                count.y += 1;
            }
            done = end;
            continue;
        }
        if !text.is_empty() {
            let n = (end.x - start.x).min(C);
            for s in &x[start.x..start.x + n] {
                text.push(format!(" {s}"));
                count.x += 1;
                count.y += 1;
            }
            done = Pair {
                x: start.x + n,
                y: start.y + n,
            };
            // One-based line numbers; an empty side shows as 0,0.
            if count.x > 0 {
                chunk.x += 1;
            }
            if count.y > 0 {
                chunk.y += 1;
            }
            out.push_str(&format!(
                "@@ -{},{} +{},{} @@\n",
                chunk.x, count.x, chunk.y, count.y
            ));
            for s in text.drain(..) {
                out.push_str(&s);
            }
            count = Pair::default();
        }
        if end.x >= x.len() && end.y >= y.len() {
            break;
        }
        chunk = Pair {
            x: end.x - C,
            y: end.y - C,
        };
        for s in &x[chunk.x..end.x] {
            text.push(format!(" {s}"));
            count.x += 1;
            count.y += 1;
        }
        done = end;
    }
    Some(out)
}

/// Lines including their newlines; a missing final newline is supplied with a marker.
fn lines(text: &str) -> Vec<String> {
    let mut lines: Vec<String> = text.split_inclusive('\n').map(str::to_owned).collect();
    if let Some(last) = lines.last_mut()
        && !last.ends_with('\n')
    {
        last.push_str("\n\\ No newline at end of file\n");
    }
    lines
}

/// The index pairs of the longest common subsequence of lines unique in both texts, between
/// the sentinels {0, 0} and {len(x), len(y)} (Thomas G. Szymanski, "A Special Case of the
/// Maximal Common Subsequence Problem", Princeton TR #170, 1975).
fn tgs(x: &[String], y: &[String]) -> Vec<Pair> {
    // Occurrences saturate: 0, -1, -2 for x and 0, -4, -8 added for y.
    let mut m: HashMap<&str, isize> = HashMap::new();
    for s in x {
        let c = m.entry(s).or_insert(0);
        if *c > -2 {
            *c -= 1;
        }
    }
    for s in y {
        let c = m.entry(s).or_insert(0);
        if *c > -8 {
            *c -= 4;
        }
    }
    let (mut xi, mut yi, mut inv) = (Vec::new(), Vec::new(), Vec::new());
    for (i, s) in y.iter().enumerate() {
        let c = m.get_mut(s.as_str()).unwrap();
        if *c == -5 {
            *c = yi.len() as isize;
            yi.push(i);
        }
    }
    for (i, s) in x.iter().enumerate() {
        if let Some(&j) = m.get(s.as_str())
            && j >= 0
        {
            xi.push(i);
            inv.push(j as usize);
        }
    }
    let n = xi.len();
    let mut t = vec![n + 1; n];
    let mut l = vec![0; n];
    for i in 0..n {
        let k = t.partition_point(|&v| v < inv[i]);
        t[k] = inv[i];
        l[i] = k + 1;
    }
    let mut k = l.iter().copied().max().unwrap_or(0);
    let mut seq = vec![Pair::default(); 2 + k];
    seq[1 + k] = Pair {
        x: x.len(),
        y: y.len(),
    };
    let mut last = n;
    for i in (0..n).rev() {
        if l[i] == k && inv[i] < last {
            seq[k] = Pair {
                x: xi[i],
                y: yi[inv[i]],
            };
            last = inv[i];
            k -= 1;
        }
    }
    seq
}

#[cfg(test)]
mod tests {
    /// Go's txtar testdata: `$` before a newline marks the line end, a final `^D` drops the
    /// newline, and the three files are old, new and the expected diff.
    fn clean(text: &str) -> String {
        let text = text.replace("$\n", "\n");
        text.strip_suffix("^D\n")
            .map_or(text.clone(), str::to_owned)
    }

    fn files(archive: &str) -> Vec<(String, String)> {
        let mut files: Vec<(String, String)> = Vec::new();
        for line in archive.split_inclusive('\n') {
            let marker = line.trim_end_matches(['\n']).trim_end_matches('$');
            if let Some(name) = marker
                .strip_prefix("-- ")
                .and_then(|rest| rest.strip_suffix(" --"))
            {
                files.push((name.to_owned(), String::new()));
            } else if let Some((_, data)) = files.last_mut() {
                data.push_str(line);
            }
        }
        files
    }

    #[test]
    fn go_internal_diff_testdata() {
        let cases = [
            ("allnew", include_str!("diff-testdata/allnew.txt")),
            ("allold", include_str!("diff-testdata/allold.txt")),
            ("basic", include_str!("diff-testdata/basic.txt")),
            ("dups", include_str!("diff-testdata/dups.txt")),
            ("end", include_str!("diff-testdata/end.txt")),
            ("eof", include_str!("diff-testdata/eof.txt")),
            ("eof1", include_str!("diff-testdata/eof1.txt")),
            ("eof2", include_str!("diff-testdata/eof2.txt")),
            ("long", include_str!("diff-testdata/long.txt")),
            ("same", include_str!("diff-testdata/same.txt")),
            ("start", include_str!("diff-testdata/start.txt")),
            ("triv", include_str!("diff-testdata/triv.txt")),
        ];
        for (name, archive) in cases {
            let files = files(archive);
            assert_eq!(files.len(), 3, "{name}");
            assert_eq!(files[2].0, "diff", "{name}");
            let have = super::unified(
                &files[0].0,
                &clean(&files[0].1),
                &files[1].0,
                &clean(&files[1].1),
            )
            .unwrap_or_default();
            assert_eq!(have, clean(&files[2].1), "{name}");
        }
    }
}

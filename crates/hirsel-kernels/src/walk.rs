//! Sorted, non-following directory traversal. Providers supply one directory at a time.
//!
//! The cursor stack follows walkdir 2's sorted DirList structure (src/lib.rs,
//! IntoIter::push). Hirsel orders directory *work* as `path/`, rather than `path`:
//! every descendant has that prefix, so a depth-first traversal then yields
//! exactly global UTF-8 byte order, including `a-`, `a.c`, `a/b`, `a0`.
//! At most one page per active depth is retained: O(depth * maximum directory
//! width) entries (and their path bytes), independent of the number of subtrees.
use std::collections::VecDeque;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Entry {
    pub path: String,
    /// Regular = 0, Directory = 1, Symlink = 2, Other = 3.
    pub kind: u32,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Walk {
    // Public for representation adapters; each page is in work-key order.
    pub pages: Vec<VecDeque<Entry>>,
}
impl Walk {
    pub fn push(&mut self, mut entries: Vec<Entry>) {
        entries.sort_by(|a, b| {
            a.path
                .bytes()
                .chain((a.kind == 1).then_some(b'/'))
                .cmp(b.path.bytes().chain((b.kind == 1).then_some(b'/')))
        });
        self.pages.push(entries.into());
    }
    /// A directory is work for the provider, never a yielded item. The caller
    /// must push its direct children before asking for the next entry.
    pub fn next_entry(&mut self) -> Option<Entry> {
        loop {
            let page = self.pages.last_mut()?;
            if let Some(entry) = page.pop_front() {
                return Some(entry);
            }
            self.pages.pop();
        }
    }
    pub fn stop(&mut self) {
        self.pages.clear();
    }
}

/// Direct children of one flat ordered path index. `seek` is an inclusive
/// lower-bound lookup (BTreeMap/BTreeSet::range). Skip a child's entire `name/`
/// interval by seeking `name0`: slash's next byte is ASCII zero. This avoids
/// scanning or copying every descendant just to list its parent.
pub fn direct_children(path: &str, mut seek: impl FnMut(&str) -> Option<String>) -> Vec<String> {
    let prefix = if path.is_empty() {
        String::new()
    } else {
        format!("{path}/")
    };
    let mut bound = prefix.clone();
    let mut children = vec![];
    while let Some(key) = seek(&bound) {
        let Some(local) = key.strip_prefix(&prefix) else {
            break;
        };
        if let Some(slash) = local.find('/') {
            bound = format!("{}{}0", prefix, &local[..slash]);
        } else {
            bound = format!("{key}\0");
            if !local.is_empty() {
                children.push(key);
            }
        }
    }
    children
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn direct_children_skip_descendant_intervals_without_skipping_byte_neighbors() {
        let mut paths: std::collections::BTreeSet<String> = ["", "a", "a-", "a.c", "a0", "a1", "é"]
            .map(str::to_owned)
            .into();
        paths.extend((0..10000).map(|n| format!("a/deep/{n}")));
        let mut seeks = 0;
        assert_eq!(
            direct_children("", |bound| {
                seeks += 1;
                paths.range(bound.to_owned()..).next().cloned()
            }),
            ["a", "a-", "a.c", "a0", "a1", "é"]
        );
        assert!(seeks < 12, "{seeks} seeks scanned a subtree");
        assert!(
            direct_children("absent", |bound| paths
                .range(bound.to_owned()..)
                .next()
                .cloned())
            .is_empty()
        );
    }
    #[test]
    fn directory_work_sorts_at_slash_not_at_the_end_of_its_name() {
        let mut walk = Walk::default();
        walk.push(
            [("a", 1), ("a.c", 0), ("a-", 0), ("a0", 0), ("é", 0)]
                .into_iter()
                .map(|(path, kind)| Entry {
                    path: path.into(),
                    kind,
                })
                .collect(),
        );
        let mut output = vec![];
        while let Some(entry) = walk.next_entry() {
            if entry.kind == 1 {
                walk.push(vec![Entry {
                    path: "a/b".into(),
                    kind: 0,
                }]);
            } else {
                output.push(entry.path);
            }
        }
        assert_eq!(output, ["a-", "a.c", "a/b", "a0", "é"]);
    }
    #[test]
    fn differential_generated_trees_match_eager_global_sort() {
        use std::collections::BTreeMap;
        for seed in 0..32u64 {
            let names = ["a", "a-", "a.c", "a0", "a..", "é", "中", "z"];
            let mut tree = BTreeMap::<String, Vec<Entry>>::new();
            let mut todo = vec![(String::new(), 0)];
            let mut old = vec![];
            while let Some((parent, depth)) = todo.pop() {
                let mut children = vec![];
                for (i, name) in names.iter().enumerate() {
                    let path = if parent.is_empty() {
                        name.to_string()
                    } else {
                        format!("{parent}/{name}")
                    };
                    let directory = depth < 4 && (seed + i as u64 + depth) % 4 == 0;
                    children.push(Entry {
                        path: path.clone(),
                        kind: u32::from(directory),
                    });
                    if directory {
                        todo.push((path, depth + 1));
                    } else {
                        old.push(path);
                    }
                }
                tree.insert(parent, children);
            }
            tree.get_mut("").unwrap().push(Entry {
                path: "empty".into(),
                kind: 1,
            });
            tree.insert("empty".into(), vec![]);
            let mut walk = Walk::default();
            walk.push(tree[""].clone());
            let mut new = vec![];
            while let Some(entry) = walk.next_entry() {
                if entry.kind == 1 {
                    walk.push(tree[&entry.path].clone());
                } else {
                    new.push(entry.path);
                }
                assert!(walk.pages.len() <= 6);
                assert!(walk.pages.iter().map(VecDeque::len).sum::<usize>() <= 6 * 9);
            }
            old.sort();
            assert_eq!(new, old, "seed {seed}");
        }
    }
}

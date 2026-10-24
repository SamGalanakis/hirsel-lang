//! Sorted, non-following directory traversal. Providers supply one directory at a time.
//!
//! The cursor stack follows walkdir 2's sorted DirList structure (src/lib.rs,
//! IntoIter::push). Hirsel orders directory *work* as `path/`, rather than `path`:
//! every descendant has that prefix, so a depth-first traversal then yields
//! exactly global UTF-8 byte order, including `a-`, `a.c`, `a/b`, `a0`.
//! At most one bounded page per active depth is retained: O(depth * 128)
//! entries and their path bytes, independent of directory width. Refills scan
//! names after an exclusive cursor: O(width * ceil(consumed / 128)) per directory.
use std::collections::{BTreeMap, VecDeque};

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
pub fn direct_children(path: &str, seek: impl FnMut(&str) -> Option<String>) -> Vec<String> {
    let mut children = vec![];
    visit_children(path, seek, |p| children.push(p));
    children
}
/// Streaming form for bounded selectors: skips descendant intervals without
/// retaining a full direct-child list.
pub fn visit_children(
    path: &str,
    mut seek: impl FnMut(&str) -> Option<String>,
    mut visit: impl FnMut(String),
) {
    let prefix = if path.is_empty() {
        String::new()
    } else {
        format!("{path}/")
    };
    let mut bound = prefix.clone();
    while let Some(key) = seek(&bound) {
        let Some(local) = key.strip_prefix(&prefix) else {
            break;
        };
        if let Some(slash) = local.find('/') {
            bound = format!("{}{}0", prefix, &local[..slash]);
        } else {
            bound = format!("{key}\0");
            if !local.is_empty() {
                visit(key);
            }
        }
    }
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

/// Fixed internal page bound shared by read_dir and walk. No allocation scales
/// with directory width. A refill rescans names after the previous work key.
pub const PAGE_SIZE: usize = 128;
pub fn work_key(path: &str, kind: u32, recursive: bool) -> String {
    if recursive && kind == 1 {
        format!("{path}/")
    } else {
        path.into()
    }
}
/// Keep only the smallest PAGE_SIZE distinct keys. Providers feed this directly
/// from lazy readdir / flat indexes, never from an intermediate directory Vec.
#[derive(Default)]
pub struct PageSelector(BTreeMap<String, Entry>);
impl PageSelector {
    pub fn offer(&mut self, entry: Entry, after: &str, recursive: bool) {
        let key = work_key(&entry.path, entry.kind, recursive);
        if key.as_str() <= after {
            return;
        }
        if self.0.len() == PAGE_SIZE && self.0.last_key_value().is_some_and(|(last, _)| &key > last)
        {
            return;
        }
        self.0.insert(key, entry);
        if self.0.len() > PAGE_SIZE {
            self.0.pop_last();
        }
    }
    pub fn finish(self) -> Vec<Entry> {
        self.0.into_values().collect()
    }
}
impl Walk {
    /// Entries are already in provider work-key order. A private sentinel
    /// requests the next bounded page after the last key, after consuming this
    /// page. Descending a directory pushes its page above this continuation.
    pub fn push_page(&mut self, directory: &str, mut entries: Vec<Entry>, recursive: bool) {
        // A consumed refill marker must not retain an empty page below its
        // replacement; otherwise a wide directory would accumulate empty pages.
        while self.pages.last().is_some_and(VecDeque::is_empty) {
            self.pages.pop();
        }
        entries.sort_by_cached_key(|e| work_key(&e.path, e.kind, recursive));
        let next = (entries.len() == PAGE_SIZE).then(|| {
            let last = entries.last().unwrap();
            Entry {
                path: format!(
                    "{directory}\0{}",
                    work_key(
                        last.path
                            .rsplit_once('/')
                            .map_or(last.path.as_str(), |(_, n)| n),
                        last.kind,
                        recursive
                    )
                ),
                kind: 4,
            }
        });
        let mut page: VecDeque<_> = entries.into();
        page.extend(next);
        self.pages.push(page);
    }
}

#[cfg(test)]
mod bounded_tests {
    use super::*;
    #[test]
    fn bounded_pages_match_a_full_sort_with_directory_byte_boundaries() {
        let mut all: Vec<_> = (0..521)
            .map(|n| Entry {
                path: format!("{n:04}"),
                kind: u32::from(n % 3 == 0),
            })
            .collect();
        all.extend(
            [("a", 1), ("a-", 0), ("a.c", 0), ("a0", 0), ("é", 2)].map(|(path, kind)| Entry {
                path: path.into(),
                kind,
            }),
        );
        for recursive in [false, true] {
            let mut expected = all.clone();
            expected.sort_by_key(|e| work_key(&e.path, e.kind, recursive));
            let mut actual = vec![];
            let mut after = String::new();
            loop {
                let mut page = PageSelector::default();
                for e in all.iter().rev().chain(all.iter()) {
                    page.offer(e.clone(), &after, recursive);
                    assert!(page.0.len() <= PAGE_SIZE);
                }
                let entries = page.finish();
                let Some(last) = entries.last() else {
                    break;
                };
                after = work_key(&last.path, last.kind, recursive);
                actual.extend(entries);
            }
            assert_eq!(actual, expected);
        }
    }
}

#[cfg(test)]
mod refill_storage_tests {
    use super::*;
    #[test]
    fn repeated_refills_do_not_accumulate_empty_pages() {
        let mut walk = Walk::default();
        for page in 0..32 {
            walk.push_page(
                "",
                (0..PAGE_SIZE)
                    .map(|n| Entry {
                        path: format!("{:08}", page * PAGE_SIZE + n),
                        kind: 0,
                    })
                    .collect(),
                false,
            );
            assert_eq!(walk.pages.len(), 1);
            for _ in 0..PAGE_SIZE {
                assert_ne!(walk.next_entry().unwrap().kind, 4);
            }
            assert_eq!(walk.next_entry().unwrap().kind, 4);
        }
        walk.push_page("", vec![], false);
        assert!(walk.next_entry().is_none());
        assert!(walk.pages.is_empty());
    }
}

//! Independent checked-heap model of confined directory lifecycle operations.
use super::{Io, IoGrant, io_error, io_not_granted, io_ok, io_portable, io_relative};
use hirsel_heapcheck::{OpOutcome, OpValue};
use hirsel_lir::layout_types::ScalarKind;
use hirsel_run_config::Access;

fn parent(path: &str) -> &str {
    path.rsplit_once('/').map_or("", |(p, _)| p)
}
fn tag(tag: u32) -> OpValue {
    OpValue::Variant {
        tag,
        value: Box::new(OpValue::Unit),
    }
}
fn kind(io: &Io, tree: u64, path: &str) -> Option<u32> {
    if io.trees[&tree].0.contains(path) {
        Some(1)
    } else if io.trees[&tree].1.contains_key(path) {
        Some(0)
    } else {
        None
    }
}
fn children(io: &Io, tree: u64, path: &str) -> Vec<String> {
    io.trees[&tree]
        .0
        .iter()
        .chain(io.trees[&tree].1.keys())
        .filter(|p| !p.is_empty() && parent(p) == path)
        .cloned()
        .collect()
}
impl Io {
    pub(super) fn physical_path(&self, directory: u64, path: &str) -> Option<String> {
        if self.detached_directories.contains(&directory) {
            return None;
        }
        let IoGrant::Dir { root, .. } = self.grants.get(&directory)? else {
            return None;
        };
        let local = if root.is_empty() {
            path
        } else if root == path {
            ""
        } else {
            path.strip_prefix(&format!("{root}/"))?
        };
        io_relative(
            self.resolved_directories.get(&directory).unwrap_or(root),
            local,
        )
    }
    fn directory_at(&self, tree: u64, path: &str) -> Vec<u64> {
        self.grants
            .iter()
            .filter_map(|(id, grant)| match grant {
                IoGrant::Dir { tree: t, root, .. }
                    if *t == tree
                        && !self.detached_directories.contains(id)
                        && self.resolved_directories.get(id).unwrap_or(root) == path =>
                {
                    Some(*id)
                }
                _ => None,
            })
            .collect()
    }
    pub(super) fn filesystem(
        &mut self,
        module: Option<&str>,
        name: &str,
        args: &[OpValue],
    ) -> Option<OpOutcome> {
        if module != Some("file")
            || !matches!(
                name,
                "read_dir" | "metadata" | "mkdir" | "rename" | "remove_dir"
            )
        {
            return None;
        }
        let [OpValue::Handle(directory), OpValue::Handle(path), rest @ ..] = args else {
            return Some(OpOutcome::Unsupported);
        };
        let Some(IoGrant::Dir { tree, root, access }) = self.grants.get(directory).cloned() else {
            return Some(OpOutcome::Fail("CAP-2".into()));
        };
        let mutation = matches!(name, "mkdir" | "rename" | "remove_dir");
        if mutation && access == Access::Read {
            return Some(OpOutcome::Value(io_not_granted()));
        }
        let text = self.paths.get(path.checked_sub(1)? as usize)?.clone();
        let root = self
            .resolved_directories
            .get(directory)
            .cloned()
            .unwrap_or(root);
        let Some(path) = io_relative(&root, &text) else {
            return Some(OpOutcome::Value(io_error(1)));
        };
        if mutation && path == root {
            return Some(OpOutcome::Value(io_error(1)));
        }
        let mut prefix = String::new();
        let parts: Vec<_> = path.split('/').filter(|p| !p.is_empty()).collect();
        for part in parts.iter().take(parts.len().saturating_sub(1)) {
            if !prefix.is_empty() {
                prefix.push('/');
            }
            prefix.push_str(part);
            if kind(self, tree, &prefix) != Some(1) {
                return Some(OpOutcome::Value(io_portable(
                    if kind(self, tree, &prefix).is_some() {
                        7
                    } else {
                        3
                    },
                )));
            }
        }
        let result = match name {
            "metadata" => match kind(self, tree, &path) {
                Some(k) => io_ok(OpValue::Aggregate(vec![
                    tag(k),
                    OpValue::Bits {
                        scalar: ScalarKind::U64,
                        bits: self.trees[&tree]
                            .1
                            .get(&path)
                            .map_or(0, |b| b.len() as u128),
                    },
                    tag(0),
                ])),
                None => io_portable(3),
            },
            "read_dir" => match kind(self, tree, &path) {
                Some(1) => {
                    let mut paths = children(self, tree, &path);
                    paths.sort();
                    let prefix = io_relative("", &text)?;
                    let entries = paths
                        .into_iter()
                        .map(|p| {
                            let name = p.rsplit_once('/').map_or(p.as_str(), |(_, n)| n);
                            let local = if prefix.is_empty() {
                                name.into()
                            } else {
                                format!("{prefix}/{name}")
                            };
                            let k = kind(self, tree, &p).unwrap();
                            OpValue::Aggregate(vec![OpValue::Handle(self.path(&local)), tag(k)])
                        })
                        .collect();
                    io_ok(OpValue::Sequence(entries))
                }
                Some(_) => io_portable(7),
                None => io_portable(3),
            },
            "mkdir" => {
                if kind(self, tree, &path).is_some() {
                    io_portable(5)
                } else if kind(self, tree, parent(&path)) != Some(1) {
                    io_portable(3)
                } else {
                    self.trees.get_mut(&tree).unwrap().0.insert(path);
                    io_ok(OpValue::Unit)
                }
            }
            "remove_dir" => match kind(self, tree, &path) {
                Some(1) if !children(self, tree, &path).is_empty() => io_portable(8),
                Some(1) => {
                    self.trees.get_mut(&tree).unwrap().0.remove(&path);
                    self.detached_directories
                        .extend(self.directory_at(tree, &path));
                    io_ok(OpValue::Unit)
                }
                Some(_) => io_portable(7),
                None => io_portable(3),
            },
            "rename" => {
                let [OpValue::Handle(to)] = rest else {
                    return Some(OpOutcome::Unsupported);
                };
                let text = self.paths.get(to.checked_sub(1)? as usize)?;
                let Some(to) = io_relative(&root, text) else {
                    return Some(OpOutcome::Value(io_error(1)));
                };
                let from_kind = kind(self, tree, &path);
                let to_kind = kind(self, tree, &to);
                if to == root || (from_kind == Some(1) && to.starts_with(&format!("{path}/"))) {
                    io_error(1)
                } else if from_kind.is_none() {
                    io_portable(3)
                } else if kind(self, tree, parent(&to)) != Some(1) {
                    io_portable(if kind(self, tree, parent(&to)).is_some() {
                        7
                    } else {
                        3
                    })
                } else if path == to {
                    io_ok(OpValue::Unit)
                } else if from_kind == Some(1) && to_kind == Some(0) {
                    io_portable(7)
                } else if from_kind == Some(0) && to_kind == Some(1) {
                    io_portable(6)
                } else if to_kind == Some(1) && !children(self, tree, &to).is_empty() {
                    io_portable(8)
                } else {
                    let moved = |p: &str| p == path || p.starts_with(&format!("{path}/"));
                    let dest = |p: &str| format!("{to}{}", &p[path.len()..]);
                    let (dirs, files) = self.trees.get_mut(&tree).unwrap();
                    dirs.remove(&to);
                    files.remove(&to);
                    let paths: Vec<_> = dirs.iter().filter(|p| moved(p)).cloned().collect();
                    for p in paths {
                        dirs.remove(&p);
                        dirs.insert(dest(&p));
                    }
                    let paths: Vec<_> = files.keys().filter(|p| moved(p)).cloned().collect();
                    for p in paths {
                        let value = files.remove(&p).unwrap();
                        files.insert(dest(&p), value);
                    }
                    self.files.remove(&(tree, to.clone()));
                    let paths: Vec<_> = self
                        .files
                        .keys()
                        .filter(|(t, p)| *t == tree && moved(p))
                        .cloned()
                        .collect();
                    for (t, p) in paths {
                        let id = self.files.remove(&(t, p.clone())).unwrap();
                        self.files.insert((t, dest(&p)), id);
                    }
                    self.detached_directories
                        .extend(self.directory_at(tree, &to));
                    for (id, grant) in &self.grants {
                        if self.detached_directories.contains(id) {
                            continue;
                        }
                        if let IoGrant::Dir { tree: t, root, .. } = grant
                            && *t == tree
                        {
                            let physical = self.resolved_directories.get(id).unwrap_or(root);
                            if moved(physical) {
                                self.resolved_directories.insert(*id, dest(physical));
                            }
                        }
                    }
                    io_ok(OpValue::Unit)
                }
            }
            _ => unreachable!(),
        };
        Some(OpOutcome::Value(result))
    }
}

impl Io {
    pub(super) fn detached_filesystem(
        &mut self,
        module: Option<&str>,
        name: &str,
        args: &[OpValue],
    ) -> Option<OpOutcome> {
        let [OpValue::Handle(directory), rest @ ..] = args else {
            return None;
        };
        if !self.detached_directories.contains(directory) {
            return None;
        }
        if module == Some("file") && name == "walk" {
            return Some(OpOutcome::Value(io_ok(OpValue::Iterator(vec![]))));
        }
        if (module != Some("file")
            || !matches!(
                name,
                "metadata"
                    | "read_dir"
                    | "mkdir"
                    | "remove_dir"
                    | "rename"
                    | "open"
                    | "create"
                    | "read_bytes"
                    | "write_bytes"
                    | "size"
                    | "remove"
                    | "create_atomic"
            ))
            && !(module == Some("dir") && name == "narrow")
        {
            return None;
        }
        if matches!(
            self.grants.get(directory),
            Some(IoGrant::Dir {
                access: Access::Read,
                ..
            })
        ) && module == Some("file")
            && matches!(
                name,
                "mkdir"
                    | "remove_dir"
                    | "rename"
                    | "create"
                    | "write_bytes"
                    | "remove"
                    | "create_atomic"
            )
        {
            return Some(OpOutcome::Value(io_not_granted()));
        }
        let [OpValue::Handle(path), ..] = rest else {
            return Some(OpOutcome::Unsupported);
        };
        let local = self.paths.get(path.checked_sub(1)? as usize)?;
        let result = match io_relative("", local) {
            None => io_error(1),
            Some(p) if !p.is_empty() => io_portable(3),
            Some(_) => match (module, name) {
                (Some("dir"), "narrow") => {
                    let handle = self.grant(self.grants.get(directory)?.clone());
                    self.detached_directories.insert(handle);
                    io_ok(OpValue::Handle(handle))
                }
                (_, "metadata") => io_ok(OpValue::Aggregate(vec![
                    tag(1),
                    OpValue::Bits {
                        scalar: ScalarKind::U64,
                        bits: 0,
                    },
                    tag(0),
                ])),
                (_, "read_dir") => io_ok(OpValue::Sequence(vec![])),
                (_, "mkdir" | "remove_dir" | "rename") => io_error(1),
                _ => io_portable(6),
            },
        };
        Some(OpOutcome::Value(result))
    }
}

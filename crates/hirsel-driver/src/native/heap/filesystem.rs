//! Independent checked-heap model of confined directory lifecycle operations.
use super::{Io, IoGrant, io_error, io_not_granted, io_ok, io_portable, io_relative};
use hirsel_heapcheck::{OpOutcome, OpValue, Operations};
use hirsel_lir::layout_types::ScalarKind;
use hirsel_run_schema::Access;

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
    /// File program checks use only grants and lexical paths; filesystem observations
    /// remain after cancellation, including a missing file or a detached directory.
    pub(super) fn cancel_file(
        &mut self,
        name: &str,
        args: &[OpValue],
        cause: u32,
    ) -> Option<OpOutcome> {
        if name == "narrow" {
            return None;
        }
        let [OpValue::Handle(directory), rest @ ..] = args else {
            return Some(OpOutcome::Unsupported);
        };
        let Some(IoGrant::Dir { root, access, .. }) = self.grants.get(directory) else {
            return Some(OpOutcome::Fail("CAP-2".into()));
        };
        let mut bytes = 1u32.to_le_bytes().to_vec();
        bytes.extend_from_slice(&cause.to_le_bytes());
        let mut value = OpValue::Variant {
            tag: 1,
            value: Box::new(OpValue::Plain(bytes)),
        };
        let writing = matches!(
            name,
            "create"
                | "write_bytes"
                | "write"
                | "append"
                | "remove"
                | "mkdir"
                | "remove_dir"
                | "remove_all"
                | "set_permissions"
                | "rename"
                | "create_atomic"
                | "write_all"
                | "commit"
                | "put_new"
        );
        if writing && *access == Access::Read {
            value = io_not_granted();
        } else if matches!(name, "read" | "write" | "append" | "seek" | "sync") {
            let Some(OpValue::Handle(file)) = rest.first() else {
                return Some(OpOutcome::Unsupported);
            };
            let Some(IoGrant::File {
                directory: owner,
                writable,
                ..
            }) = self.grants.get(file)
            else {
                return Some(OpOutcome::Fail("CAP-2".into()));
            };
            if owner != directory {
                return Some(OpOutcome::Fail("CAP-2".into()));
            }
            if name == "read" && *writable
                || matches!(name, "write" | "append") && !*writable
                || name == "seek"
                    && matches!(rest.get(1), Some(OpValue::Bits { bits, .. }) if *bits > i64::MAX as u128)
            {
                value = io_error(1);
            }
        } else if matches!(name, "write_all" | "commit") {
            let Some(OpValue::Handle(file)) = rest.first() else {
                return Some(OpOutcome::Unsupported);
            };
            if !matches!(self.grants.get(file), Some(IoGrant::Atomic { directory: owner, .. }) if owner == directory)
            {
                return Some(OpOutcome::Fail("CAP-2".into()));
            }
        } else if name != "walk" {
            for operand in rest.iter().take(if name == "rename" { 2 } else { 1 }) {
                let OpValue::Handle(path) = operand else {
                    return Some(OpOutcome::Unsupported);
                };
                let text = self.paths.get(path.checked_sub(1)? as usize)?;
                if io_relative(root, text).is_none() {
                    value = io_error(1);
                }
            }
        }
        if writing && *access != Access::Read {
            if name == "set_permissions"
                && matches!(rest.get(1), Some(OpValue::Bits { bits, .. }) if *bits > 0o777)
            {
                value = io_error(1);
            }
            if name == "remove_all"
                && let Some(OpValue::Handle(path)) = rest.first()
                && let Some(text) = path.checked_sub(1).and_then(|p| self.paths.get(p as usize))
                && io_relative(root, text).as_ref() == Some(root)
            {
                value = io_error(1);
            }
        }
        if name == "commit" {
            let Some(OpValue::Handle(file)) = rest.first() else {
                return Some(OpOutcome::Unsupported);
            };
            if self
                .close_handle(*file, &mut Vec::new())
                .ok()
                .flatten()
                .is_some()
            {
                return Some(OpOutcome::Unsupported);
            }
            return Some(OpOutcome::Complete {
                result: Ok(value),
                consumed: vec![1],
            });
        }
        Some(OpOutcome::Value(value))
    }

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
        if !(module == Some("dir") && name == "remove_all")
            && (module != Some("file")
                || !matches!(
                    name,
                    "read_dir" | "metadata" | "mkdir" | "rename" | "remove_dir" | "set_permissions"
                ))
        {
            return None;
        }
        let [OpValue::Handle(directory), OpValue::Handle(path), rest @ ..] = args else {
            return Some(OpOutcome::Unsupported);
        };
        let Some(IoGrant::Dir { tree, root, access }) = self.grants.get(directory).cloned() else {
            return Some(OpOutcome::Fail("CAP-2".into()));
        };
        let mutation = matches!(
            name,
            "mkdir" | "rename" | "remove_dir" | "remove_all" | "set_permissions"
        );
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
        if mutation && name != "set_permissions" && path == root {
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
                    OpValue::Bits {
                        scalar: ScalarKind::U32,
                        bits: self
                            .permissions
                            .get(&(tree, path.clone()))
                            .copied()
                            .unwrap_or(if k == 0 { 0o666 } else { 0o777 })
                            as u128,
                    },
                ])),
                None => io_portable(3),
            },
            "read_dir" => match kind(self, tree, &path) {
                Some(1) => {
                    io_ok(self.directory_iterator(*directory, &io_relative("", &text)?, false))
                }
                Some(_) => io_portable(7),
                None => io_portable(3),
            },
            "set_permissions" => {
                let [OpValue::Bits { bits, .. }] = rest else {
                    return Some(OpOutcome::Unsupported);
                };
                if *bits > 0o777 {
                    io_error(1)
                } else if kind(self, tree, &path).is_none() {
                    io_portable(3)
                } else {
                    self.permissions.insert((tree, path.clone()), *bits as u32);
                    io_ok(OpValue::Unit)
                }
            }
            "remove_all" => match kind(self, tree, &path) {
                Some(1) => {
                    let removed = |p: &str| p == path || p.starts_with(&format!("{path}/"));
                    let ids: Vec<_> = self
                        .grants
                        .iter()
                        .filter_map(|(id, grant)| match grant {
                            IoGrant::Dir { tree: t, root, .. }
                                if *t == tree
                                    && removed(
                                        self.resolved_directories.get(id).unwrap_or(root),
                                    ) =>
                            {
                                Some(*id)
                            }
                            _ => None,
                        })
                        .collect();
                    self.detached_directories.extend(ids);
                    let (dirs, files) = self.trees.get_mut(&tree).unwrap();
                    dirs.retain(|p| !removed(p));
                    files.retain(|p, _| !removed(p));
                    self.files.retain(|(t, p), _| *t != tree || !removed(p));
                    self.permissions
                        .retain(|(t, p), _| *t != tree || !removed(p));
                    io_ok(OpValue::Unit)
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
                    self.permissions.remove(&(tree, path.clone()));
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
                    self.permissions.remove(&(tree, to.clone()));
                    let modes: Vec<_> = self
                        .permissions
                        .keys()
                        .filter(|(t, p)| *t == tree && moved(p))
                        .cloned()
                        .collect();
                    for key in modes {
                        let bits = self.permissions.remove(&key).unwrap();
                        self.permissions.insert((tree, dest(&key.1)), bits);
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
                    OpValue::Bits {
                        scalar: ScalarKind::U32,
                        bits: 0o777,
                    },
                ])),
                (_, "read_dir") => io_ok(self.directory_iterator(*directory, local, false)),
                (_, "mkdir" | "remove_dir" | "rename") => io_error(1),
                _ => io_portable(6),
            },
        };
        Some(OpOutcome::Value(result))
    }
}

impl Io {
    pub(super) fn directory_iterator(
        &self,
        directory: u64,
        path: &str,
        recursive: bool,
    ) -> OpValue {
        let mut walk = hirsel_rt::kernels::walk::Walk::default();
        let entries = self
            .directory_page(directory, path, "", recursive)
            .expect("validated directory root");
        walk.push_page(path, entries, recursive);
        OpValue::LazyIterator(Box::new(OpValue::Aggregate(vec![
            OpValue::Handle(directory),
            OpValue::Bits {
                scalar: ScalarKind::U8,
                bits: u128::from(recursive),
            },
            Self::directory_pages(walk),
        ])))
    }
    fn directory_pages(walk: hirsel_rt::kernels::walk::Walk) -> OpValue {
        OpValue::Sequence(
            walk.pages
                .into_iter()
                .map(|page| {
                    OpValue::Sequence(
                        page.into_iter()
                            .map(|entry| {
                                OpValue::Aggregate(vec![
                                    OpValue::Bytes(entry.path.into_bytes()),
                                    OpValue::Bits {
                                        scalar: ScalarKind::U32,
                                        bits: entry.kind as u128,
                                    },
                                ])
                            })
                            .collect(),
                    )
                })
                .collect(),
        )
    }
    fn directory_page(
        &self,
        directory: u64,
        path: &str,
        after: &str,
        recursive: bool,
    ) -> Result<Vec<hirsel_rt::kernels::walk::Entry>, OpValue> {
        let Some(IoGrant::Dir { tree, root, .. }) = self.grants.get(&directory) else {
            return Err(io_error(1));
        };
        if self.detached_directories.contains(&directory) {
            return Ok(vec![]);
        }
        let logical = io_relative(root, path).ok_or_else(|| io_error(1))?;
        let physical = self
            .physical_path(directory, &logical)
            .ok_or_else(|| io_error(1))?;
        match kind(self, *tree, &physical) {
            Some(1) => (),
            Some(_) => return Err(io_portable(7)),
            None => return Err(io_portable(3)),
        };
        let prefix = if physical.is_empty() {
            String::new()
        } else {
            format!("{physical}/")
        };
        let mut page = hirsel_rt::kernels::walk::PageSelector::default();
        let mut offer = |p: String| {
            let name = p.strip_prefix(&prefix).expect("direct child");
            page.offer(
                hirsel_rt::kernels::walk::Entry {
                    path: name.into(),
                    kind: kind(self, *tree, &p).expect("indexed directory entry"),
                },
                after,
                recursive,
            );
        };
        hirsel_rt::kernels::walk::visit_children(
            &physical,
            |bound| self.trees[tree].0.range(bound.to_owned()..).next().cloned(),
            &mut offer,
        );
        hirsel_rt::kernels::walk::visit_children(
            &physical,
            |bound| {
                self.trees[tree]
                    .1
                    .range(bound.to_owned()..)
                    .next()
                    .map(|(p, _)| p.clone())
            },
            &mut offer,
        );
        Ok(page
            .finish()
            .into_iter()
            .map(|mut e| {
                if !path.is_empty() {
                    e.path = format!("{path}/{}", e.path);
                }
                e
            })
            .collect())
    }
    pub(super) fn directory_next(
        &mut self,
        state: &mut OpValue,
        canceled: Option<u32>,
    ) -> Result<Option<OpValue>, String> {
        let OpValue::Aggregate(fields) = state else {
            return Err("directory iterator state".into());
        };
        let OpValue::Handle(directory) = fields[0] else {
            return Err("directory iterator grant".into());
        };
        let recursive = matches!(fields[1], OpValue::Bits { bits: 1, .. });
        let OpValue::Sequence(pages) = &fields[2] else {
            return Err("directory pages".into());
        };
        let mut walk = hirsel_rt::kernels::walk::Walk::default();
        for page in pages {
            let OpValue::Sequence(entries) = page else {
                return Err("directory page".into());
            };
            let mut p = std::collections::VecDeque::new();
            for entry in entries {
                let OpValue::Aggregate(pair) = entry else {
                    return Err("directory entry".into());
                };
                let (OpValue::Bytes(path), OpValue::Bits { bits, .. }) = (&pair[0], &pair[1])
                else {
                    return Err("directory entry fields".into());
                };
                p.push_back(hirsel_rt::kernels::walk::Entry {
                    path: String::from_utf8(path.clone()).map_err(|_| "directory path")?,
                    kind: *bits as u32,
                });
            }
            walk.pages.push(p);
        }
        let mut result = None;
        while let Some(entry) = walk.next_entry() {
            if entry.kind == 4 || (recursive && entry.kind == 1) {
                self.yielded = true;
                if let Some(cause) = canceled {
                    walk.stop();
                    let mut bytes = 1u32.to_le_bytes().to_vec();
                    bytes.extend_from_slice(&cause.to_le_bytes());
                    result = Some(OpValue::Variant {
                        tag: 1,
                        value: Box::new(OpValue::Plain(bytes)),
                    });
                    break;
                }

                let (path, after) = if entry.kind == 4 {
                    entry
                        .path
                        .split_once('\0')
                        .unwrap_or((entry.path.as_str(), ""))
                } else {
                    (entry.path.as_str(), "")
                };
                let entries = match self.directory_page(directory, path, after, recursive) {
                    Ok(entries) => entries,
                    Err(error) => {
                        walk.stop();
                        result = Some(error);
                        break;
                    }
                };
                walk.push_page(path, entries, recursive);
            } else {
                result = Some(io_ok(if recursive {
                    OpValue::Handle(self.path(&entry.path))
                } else {
                    OpValue::Aggregate(vec![
                        OpValue::Handle(self.path(&entry.path)),
                        tag(entry.kind),
                    ])
                }));
                break;
            }
        }
        fields[2] = Self::directory_pages(walk);
        Ok(result)
    }
}

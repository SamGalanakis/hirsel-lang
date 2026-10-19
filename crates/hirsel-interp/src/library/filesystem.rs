//! Directory lifecycle model, independent of host descriptors and native storage.
use super::{
    Library,
    dispatch::*,
    simulation::{FileError, FileMetadata},
};
use crate::{Error, Integer, Value};
use hirsel_types::{BuiltinType, Primitive};
use std::collections::BTreeSet;

fn record(name: &str, fields: Vec<Value>) -> Value {
    Value::Record {
        definition: hirsel_syntax::DefPath::core(name),
        fields: fields.into_iter().map(Some).collect(),
    }
}
pub(super) fn metadata_value(meta: FileMetadata) -> Value {
    record(
        "Metadata",
        vec![
            variant(meta.kind, vec![]),
            u64_value(meta.size),
            meta.modified_unix_ns.map_or_else(none, |n| {
                some(Value::Integer(
                    Integer::new(Primitive::I128, n as u128).unwrap(),
                ))
            }),
            u32_value(meta.permissions),
        ],
    )
}
impl super::Simulation {
    fn update_mount_pins(&mut self, from: &str, to: Option<&str>) {
        for (selector, root) in &mut self.mount_pins {
            if self.detached_mounts.contains(selector) {
                continue;
            }
            if (to.is_none() && root == from) || to.is_some_and(|to| to != from && root == to) {
                self.detached_mounts.insert(selector.clone());
                self.config.links.remove(selector);
                self.trees.get_mut(&0).unwrap().0.insert(selector.clone());
                *root = selector.clone();
            } else if let Some(to) = to
                && (*root == from || root.starts_with(&format!("{from}/")))
            {
                *root = format!("{to}{}", &root[from.len()..]);
            }
            if self.config.links.contains_key(selector) {
                let parent = selector.rsplit_once('/').map_or("", |(p, _)| p);
                let prefix = "../".repeat(parent.split('/').filter(|p| !p.is_empty()).count());
                self.config
                    .links
                    .insert(selector.clone(), format!("{prefix}{root}"));
            }
        }
    }
    fn metadata(&self, tree: u64, path: &str) -> Result<FileMetadata, FileError> {
        if tree == 0
            && let Some(meta) = self.config.metadata.get(path)
        {
            let mut meta = meta.clone();
            if let Some(bits) = self.file_permissions.get(&(tree, path.into())) {
                meta.permissions = *bits;
            }
            return Ok(meta);
        }
        let (dirs, files) = &self.trees[&tree];
        let (kind, size) = if let Some(bytes) = files.get(path) {
            (0, bytes.len() as u64)
        } else if dirs.contains(path) {
            (1, 0)
        } else if tree == 0
            && let Some(target) = self.config.links.get(path)
        {
            (2, target.len() as u64)
        } else {
            return Err(FileError::NotFound);
        };
        Ok(FileMetadata {
            kind,
            size,
            permissions: self
                .file_permissions
                .get(&(tree, path.into()))
                .copied()
                .unwrap_or(if kind == 0 { 0o666 } else { 0o777 }),
            modified_unix_ns: None,
        })
    }
    fn children(&self, tree: u64, path: &str) -> Result<Vec<String>, FileError> {
        if self.metadata(tree, path)?.kind != 1 {
            return Err(FileError::NotDirectory);
        }
        let (dirs, files) = &self.trees[&tree];
        let mut paths: BTreeSet<_> = dirs.iter().chain(files.keys()).cloned().collect();
        if tree == 0 {
            paths.extend(self.config.links.keys().cloned());
            paths.extend(self.config.metadata.keys().cloned());
        }
        Ok(paths
            .into_iter()
            .filter(|p| !p.is_empty() && p.rsplit_once('/').map_or("", |(p, _)| p) == path)
            .collect())
    }
    pub(super) fn remove_tree(
        &mut self,
        tree: u64,
        path: &str,
        limit: Option<usize>,
    ) -> Result<(), FileError> {
        if !matches!(self.metadata(tree, path)?.kind, 1 | 2) {
            return Err(FileError::NotDirectory);
        }
        let mut pending = vec![(path.to_owned(), false)];
        let mut removed = 0;
        while let Some((path, visited)) = pending.pop() {
            if limit == Some(removed) {
                break;
            }
            if self.metadata(tree, &path)?.kind == 1 && !visited {
                let children = self.children(tree, &path)?;
                pending.push((path, true));
                for child in children.into_iter().rev() {
                    pending.push((child, false));
                }
                continue;
            }
            let (dirs, files) = self.trees.get_mut(&tree).unwrap();
            dirs.remove(&path);
            files.remove(&path);
            self.unlink_identity(tree, &path);
            if tree == 0 {
                self.config.links.remove(&path);
                self.config.metadata.remove(&path);
                self.update_mount_pins(&path, None);
            }
            self.detached_directories.extend(self.resolved_directories.iter().filter_map(|(id, root)|
                (*root == path && matches!(self.grants.get(id), Some(super::simulation::Grant::Dir { tree: t, .. }) if *t == tree)).then_some(*id)));
            removed += 1;
        }
        Ok(())
    }
    fn move_entry(&mut self, tree: u64, from: &str, to: &str) -> Result<(), FileError> {
        let kind = self.metadata(tree, from)?.kind;
        if kind == 0
            && self.file_identity(tree, from).is_some()
            && self.file_identity(tree, from) == self.file_identity(tree, to)
        {
            return Ok(());
        }
        if self.metadata(tree, parent(to))?.kind != 1 {
            return Err(FileError::NotDirectory);
        }
        if from == to {
            return Ok(());
        }
        if kind == 1 && to.starts_with(&format!("{from}/")) {
            return Err(FileError::Fault(self.config.invalid_path_code));
        }
        match self.metadata(tree, to) {
            Ok(meta) => {
                if kind == 1 && meta.kind != 1 {
                    return Err(FileError::NotDirectory);
                }
                if kind != 1 && meta.kind == 1 {
                    return Err(FileError::IsDirectory);
                }
                if meta.kind == 1 && !self.children(tree, to)?.is_empty() {
                    return Err(FileError::DirectoryNotEmpty);
                }
            }
            Err(FileError::NotFound) => {}
            Err(e) => return Err(e),
        }
        let moved = |p: &str| p == from || p.starts_with(&format!("{from}/"));
        let destination = |p: &str| format!("{to}{}", &p[from.len()..]);
        let (dirs, files) = self.trees.get_mut(&tree).unwrap();
        dirs.remove(to);
        files.remove(to);
        let paths: Vec<_> = dirs.iter().filter(|p| moved(p)).cloned().collect();
        for path in paths {
            dirs.remove(&path);
            dirs.insert(destination(&path));
        }
        macro_rules! relocate {
            ($map:expr) => {{
                $map.remove(to);
                let paths: Vec<_> = $map.keys().filter(|p| moved(p)).cloned().collect();
                for path in paths {
                    let value = $map.remove(&path).unwrap();
                    $map.insert(destination(&path), value);
                }
            }};
        }
        relocate!(files);
        if tree == 0 {
            relocate!(self.config.links);
            relocate!(self.config.metadata);
            self.config.metadata.remove(parent(from));
            self.config.metadata.remove(parent(to));
        }
        self.file_identities.remove(&(tree, to.into()));
        let paths: Vec<_> = self
            .file_identities
            .keys()
            .filter(|(t, p)| *t == tree && moved(p))
            .cloned()
            .collect();
        for (t, path) in paths {
            let value = self.file_identities.remove(&(t, path.clone())).unwrap();
            self.file_identities.insert((t, destination(&path)), value);
        }
        self.file_permissions.remove(&(tree, to.into()));
        let modes: Vec<_> = self
            .file_permissions
            .keys()
            .filter(|(t, p)| *t == tree && moved(p))
            .cloned()
            .collect();
        for key in modes {
            let bits = self.file_permissions.remove(&key).unwrap();
            self.file_permissions
                .insert((tree, destination(&key.1)), bits);
        }
        if tree == 0 {
            self.update_mount_pins(from, Some(to));
        }
        if from != to {
            self.detached_directories.extend(self.resolved_directories.iter().filter_map(|(id, root)|
                (*root == to && matches!(self.grants.get(id), Some(super::simulation::Grant::Dir { tree: t, .. }) if *t == tree)).then_some(*id)));
        }
        for (id, root) in &mut self.resolved_directories {
            if self.detached_directories.contains(id) {
                continue;
            }
            if matches!(self.grants.get(id), Some(super::simulation::Grant::Dir { tree: t, .. }) if *t == tree)
                && moved(root)
            {
                *root = destination(root);
            }
        }
        Ok(())
    }
}
impl Library {
    pub(super) fn file_lifecycle(
        &mut self,
        name: &str,
        a: &[Value],
        directory: (u64, u64, &str),
        logical: &str,
        path: &str,
    ) -> Result<Value, Error> {
        let (tree, dir, root) = directory;
        if self.simulation.detached_directories.contains(&dir) {
            match name {
                "file.metadata" => {
                    return Ok(ok(metadata_value(FileMetadata {
                        kind: 1,
                        size: 0,
                        modified_unix_ns: None,
                        permissions: 0o777,
                    })));
                }
                "file.read_dir" => {
                    return Ok(ok(Value::Sequence {
                        kind: BuiltinType::List,
                        elements: Default::default(),
                        arena: None,
                    }));
                }
                _ => {}
            }
        }
        let result = match name {
            "file.metadata" => self.simulation.metadata(tree, path).map(metadata_value),
            "file.read_dir" => self.simulation.children(tree, path).map(|children| {
                let prefix = if root.is_empty() {
                    logical
                } else if logical == root {
                    ""
                } else {
                    logical.strip_prefix(&format!("{root}/")).unwrap()
                };
                Value::Sequence {
                    kind: BuiltinType::List,
                    elements: children
                        .into_iter()
                        .map(|p| {
                            let name = p.rsplit_once('/').map_or(p.as_str(), |(_, n)| n);
                            let local = if prefix.is_empty() {
                                name.into()
                            } else {
                                format!("{prefix}/{name}")
                            };
                            Some(record(
                                "DirEntry",
                                vec![
                                    super::path(&local),
                                    variant(
                                        self.simulation.metadata(tree, &p).unwrap().kind,
                                        vec![],
                                    ),
                                ],
                            ))
                        })
                        .collect(),
                    arena: None,
                }
            }),
            "file.set_permissions" => {
                let Value::Integer(bits) = arg(a, 2)? else { return Err(crate::invalid("permission bits")); };
                let bits = bits.bits() as u32;
                if bits > 0o777 { Err(FileError::Fault(self.simulation.config.invalid_path_code)) }
                else { self.simulation.metadata(tree, path).and_then(|meta| {
                    if !matches!(meta.kind, 0 | 1) { return Err(FileError::Fault(self.simulation.config.invalid_path_code)); }
                    self.simulation.file_permissions.insert((tree, path.into()), bits);
                    if let Some(identity) = self.simulation.file_identity(tree, path) {
                        for (key, id) in &self.simulation.file_identities {
                            if *id == identity { self.simulation.file_permissions.insert(key.clone(), bits); }
                        }
                    }
                    Ok(Value::Unit)
                }) }
            }
            _ if logical == root => Err(FileError::Fault(self.simulation.config.invalid_path_code)),
            "file.mkdir" => match self.simulation.metadata(tree, path) {
                Ok(_) => Err(FileError::AlreadyExists),
                Err(FileError::NotFound) => {
                    self.simulation
                        .metadata(tree, parent(path))
                        .and_then(|meta| {
                            if meta.kind != 1 {
                                return Err(FileError::NotDirectory);
                            }
                            self.simulation
                                .trees
                                .get_mut(&tree)
                                .unwrap()
                                .0
                                .insert(path.into());
                            if tree == 0 {
                                self.simulation.config.metadata.remove(parent(path));
                            }
                            Ok(Value::Unit)
                        })
                }
                Err(e) => Err(e),
            },
            "dir.remove_all" => self.simulation.remove_tree(tree, path, None).map(|()| Value::Unit),
            "file.remove_dir" => self.simulation.children(tree, path).and_then(|children| {
                if !children.is_empty() {
                    return Err(FileError::DirectoryNotEmpty);
                }
                self.simulation.trees.get_mut(&tree).unwrap().0.remove(path);
                self.simulation.file_permissions.remove(&(tree, path.into()));
                if tree == 0 { self.simulation.update_mount_pins(path, None); }
                self.simulation.detached_directories.extend(self.simulation.resolved_directories.iter().filter_map(|(id, root)|
                    (root == path && matches!(self.simulation.grants.get(id), Some(super::simulation::Grant::Dir { tree: t, .. }) if *t == tree)).then_some(*id)));
                if tree == 0 {
                    self.simulation.config.metadata.remove(path);
                    self.simulation.config.metadata.remove(parent(path));
                }
                Ok(Value::Unit)
            }),
            "file.rename" => {
                let target = path_text(arg(a, 2)?)?;
                if super::simulation::relative(root, target).as_deref() == Some(root) {
                    Err(FileError::Fault(self.simulation.config.invalid_path_code))
                } else {
                    self.simulation
                        .resolve_in(tree, dir, root, target, false)
                        .and_then(|to| {
                            self.simulation
                                .move_entry(tree, path, &to)
                                .map(|()| Value::Unit)
                        })
                }
            }
            _ => unreachable!(),
        };
        Ok(result.map_or_else(file_error, ok))
    }
}

//! The checked heap executor over a Ready program: the entry runs
//! with the launch handles and a model of the Io operations native code has,
//! and the terminal audit must pass on success and on every failure path.
use super::observe::{Observation, Sites};
use super::stages::{NativeError, ReadyProgram, StageName};
use hirsel_core_contract::OperationId;
use hirsel_heapcheck::{Arg, Config, Counters, Observed, OpOutcome, OpValue, Operations};
use hirsel_lir::ir::SiteRange;
use hirsel_lir::layout_types::ScalarKind;
use hirsel_rt::io::UnitResult;
use hirsel_rt::launch::{ARGS_HANDLE, IO_HANDLE};
use hirsel_run_config::Access;
use hirsel_syntax::DefPath;
use std::collections::{BTreeMap, BTreeSet};

mod net;
mod process;

/// The Io operations with a native entry (`hirsel_rt::operations`), over
/// logical handles numbered as the runtime's adapter numbers them.
#[derive(Clone)]
struct Io {
    next: u64,
    test_seed: u64,
    stdout: Vec<u64>,
    paths: Vec<String>,
    mounts: BTreeMap<String, Access>,
    grants: BTreeMap<u64, IoGrant>,
    trees: BTreeMap<u64, (BTreeSet<String>, BTreeMap<String, Vec<u8>>)>,
    files: BTreeMap<(u64, String), std::sync::Arc<()>>,
    network: hirsel_rt::io_net_script::ScriptIo,
    network_handles: BTreeMap<u64, u64>,
}

impl Io {
    fn for_launch(kind: &super::link::LaunchKind, test_seed: u64) -> Self {
        let mut io = Self::new();
        io.test_seed = test_seed;
        io.network = hirsel_rt::io_net_script::ScriptIo::new(
            matches!(kind, super::link::LaunchKind::Test).then_some(test_seed),
        );
        match kind {
            super::link::LaunchKind::Zero => io.grants.clear(),
            super::link::LaunchKind::Test => {
                io.grants.insert(IO_HANDLE, IoGrant::Test);
            }
            super::link::LaunchKind::Main(_) => {}
        }
        io
    }
    fn new() -> Self {
        Self {
            next: IO_HANDLE + 1,
            test_seed: 0,
            stdout: Vec::new(),
            paths: Vec::new(),
            mounts: BTreeMap::from([(String::new(), Access::ReadWrite)]),
            grants: BTreeMap::from([(IO_HANDLE, IoGrant::Root)]),
            trees: BTreeMap::from([(0, (BTreeSet::from([String::new()]), BTreeMap::new()))]),
            files: BTreeMap::new(),
            network: Default::default(),
            network_handles: BTreeMap::new(),
        }
    }
}

impl Operations for Io {
    fn domain(&self) -> hirsel_heapcheck::OperationDomain {
        hirsel_heapcheck::OperationDomain::Simulation
    }
    fn fork_simulation(&self) -> Option<Box<dyn Operations>> {
        Some(Box::new(self.clone()))
    }
    fn path_text(&self, id: u64) -> Option<&str> {
        self.paths
            .get(id.checked_sub(1)? as usize)
            .map(String::as_str)
    }
    fn close_handle(&mut self, handle: u64) -> Result<(), String> {
        if let Some(id) = self.network_handles.remove(&handle) {
            return self
                .network
                .call(hirsel_rt::io::Request::Close { handle: id })
                .map(|_| ())
                .map_err(|e| format!("network close: {e:?}"));
        }
        match self.grants.remove(&handle) {
            Some(IoGrant::File { .. }) => Ok(()),
            Some(IoGrant::Atomic {
                tree, temporary, ..
            }) => {
                self.trees.get_mut(&tree).unwrap().1.remove(&temporary);
                self.files.remove(&(tree, temporary));
                Ok(())
            }
            _ => Err(format!("model close of unknown handle {handle}")),
        }
    }
    fn call(&mut self, op: &OperationId, args: &[OpValue], output: &mut Vec<u8>) -> OpOutcome {
        let path = &op.definition.0;
        if !path.package.name_is("core") {
            return OpOutcome::Unsupported;
        }
        let module = path.modules.first().map(String::as_str);
        if module == Some("$intrinsic") {
            if let Some(outcome) = views(&path.name, args) {
                return outcome;
            }
        }
        if let Some(outcome) = self.net(module, &path.name, args) {
            return outcome;
        }
        if let Some(outcome) = self.process(module, &path.name, args) {
            return outcome;
        }
        if let Some(outcome) = self.io(module, &path.name, args) {
            return outcome;
        }
        if let Some(outcome) = text(module, &path.name, args) {
            return outcome;
        }
        match (module, path.name.as_str(), args) {
            (
                Some("test"),
                "check_case",
                [
                    OpValue::Handle(test),
                    OpValue::Bits {
                        scalar: ScalarKind::Bool,
                        bits,
                    },
                    OpValue::Bits {
                        scalar: ScalarKind::U64,
                        bits: case,
                    },
                ],
            ) if matches!(self.grants.get(test), Some(IoGrant::Test)) => {
                if *bits == 0 {
                    output.extend_from_slice(
                        format!("test failure: seed={} case={case}\n", self.test_seed).as_bytes(),
                    );
                    OpOutcome::Fail("LANG-9".into())
                } else {
                    OpOutcome::Value(OpValue::Unit)
                }
            }
            (Some("io"), "stdout", [OpValue::Handle(IO_HANDLE)]) => {
                let handle = self.next;
                self.next += 1;
                self.stdout.push(handle);
                OpOutcome::Value(io_ok(OpValue::Handle(handle)))
            }
            (Some("out"), "print", [OpValue::Handle(s), OpValue::Bytes(text)])
                if self.stdout.contains(s) =>
            {
                output.extend_from_slice(text);
                let ok = UnitResult::of(Ok(()));
                let mut bytes = Vec::with_capacity(12);
                for word in [ok.tag, ok.error, ok.code] {
                    bytes.extend_from_slice(&word.to_le_bytes());
                }
                OpOutcome::Value(OpValue::Plain(bytes))
            }
            _ => OpOutcome::Unsupported,
        }
    }
}

#[derive(Clone)]
enum IoGrant {
    Stdin,
    Stderr,
    Root,
    Random,
    Tasks,
    Test,
    Dir {
        tree: u64,
        root: String,
        access: Access,
    },
    File {
        tree: u64,
        path: String,
        directory: u64,
        position: u64,
        writable: bool,
        contents: Vec<u8>,
        identity: std::sync::Arc<()>,
    },
    Atomic {
        tree: u64,
        directory: u64,
        temporary: String,
        destination: String,
        identity: std::sync::Arc<()>,
    },
}
fn io_ok(value: OpValue) -> OpValue {
    OpValue::Variant {
        tag: 0,
        value: Box::new(value),
    }
}
fn io_error(code: u32) -> OpValue {
    let mut bytes = vec![0; 8];
    bytes[4..].copy_from_slice(&code.to_le_bytes());
    OpValue::Variant {
        tag: 1,
        value: Box::new(OpValue::Plain(bytes)),
    }
}
fn io_portable(ordinal: u32) -> OpValue {
    let mut bytes = vec![0; 8];
    bytes[..4].copy_from_slice(&ordinal.to_le_bytes());
    OpValue::Variant {
        tag: 1,
        value: Box::new(OpValue::Plain(bytes)),
    }
}
fn io_not_granted() -> OpValue {
    let mut bytes = vec![0; 8];
    bytes[..4].copy_from_slice(&2u32.to_le_bytes());
    OpValue::Variant {
        tag: 1,
        value: Box::new(OpValue::Plain(bytes)),
    }
}
fn io_relative(root: &str, path: &str) -> Option<String> {
    if path.starts_with('/') || path.contains(['\0', '\\']) {
        return None;
    }
    let mut parts: Vec<_> = root.split('/').filter(|p| !p.is_empty()).collect();
    let base = parts.len();
    for p in path.split('/') {
        match p {
            "" | "." => {}
            ".." => {
                if parts.len() == base {
                    return None;
                }
                parts.pop();
            }
            _ => parts.push(p),
        }
    }
    Some(parts.join("/"))
}
impl Io {
    fn grant(&mut self, grant: IoGrant) -> u64 {
        let id = self.next;
        self.next += 1;
        self.grants.insert(id, grant);
        id
    }
    fn update_files(&mut self, identity: &std::sync::Arc<()>, bytes: &[u8], append: bool) {
        for grant in self.grants.values_mut() {
            if let IoGrant::File {
                identity: opened,
                contents,
                ..
            } = grant
                && std::sync::Arc::ptr_eq(identity, opened)
            {
                if !append {
                    contents.clear();
                }
                contents.extend_from_slice(bytes);
            }
        }
    }
    fn path(&mut self, path: &str) -> u64 {
        if let Some(i) = self.paths.iter().position(|p| p == path) {
            i as u64 + 1
        } else {
            self.paths.push(path.into());
            self.paths.len() as u64
        }
    }
    fn io(&mut self, module: Option<&str>, name: &str, args: &[OpValue]) -> Option<OpOutcome> {
        let value = match (module, name, args) {
            (Some("path"), "text", [OpValue::Handle(path)]) => OpValue::ByteSequence(
                self.paths
                    .get(path.checked_sub(1)? as usize)?
                    .as_bytes()
                    .to_vec(),
            ),
            (Some("path"), "of", [OpValue::Bytes(bytes)]) => {
                OpValue::Handle(self.path(std::str::from_utf8(bytes).ok()?))
            }
            (
                Some("arg"),
                "path" | "text",
                [
                    OpValue::Handle(ARGS_HANDLE),
                    OpValue::Bits {
                        scalar: ScalarKind::U64,
                        ..
                    },
                ],
            ) => OpValue::Variant {
                tag: 1,
                value: Box::new(OpValue::Variant {
                    tag: 0,
                    value: Box::new(OpValue::Unit),
                }),
            },
            (Some("io"), kind @ ("stdin" | "stderr"), [OpValue::Handle(IO_HANDLE)]) => {
                let grant = if kind == "stdin" {
                    IoGrant::Stdin
                } else {
                    IoGrant::Stderr
                };
                io_ok(OpValue::Handle(self.grant(grant)))
            }
            (
                Some("stdin"),
                "read",
                [
                    OpValue::Handle(stream),
                    buffer,
                    OpValue::Bits {
                        scalar: ScalarKind::U64,
                        bits: max,
                    },
                ],
            ) => {
                if !matches!(self.grants.get(stream), Some(IoGrant::Stdin)) {
                    return Some(OpOutcome::Fail("CAP-2".into()));
                }
                if *max == 0 {
                    OpValue::Variant {
                        tag: 1,
                        value: Box::new(OpValue::Variant {
                            tag: 2,
                            value: Box::new(OpValue::Unit),
                        }),
                    }
                } else {
                    return Some(OpOutcome::Updated {
                        value: io_ok(OpValue::Bits {
                            scalar: ScalarKind::U64,
                            bits: 0,
                        }),
                        arguments: vec![(1, buffer.clone())],
                    });
                }
            }
            (Some("err"), "print", [OpValue::Handle(stream), OpValue::Bytes(_)]) => {
                if !matches!(self.grants.get(stream), Some(IoGrant::Stderr)) {
                    return Some(OpOutcome::Fail("CAP-2".into()));
                }
                io_ok(OpValue::Unit)
            }
            (
                Some("env"),
                "get",
                [
                    OpValue::Handle(hirsel_rt::io::ENV_HANDLE),
                    OpValue::Bytes(_),
                ],
            ) => OpValue::Variant {
                tag: 1,
                value: Box::new(OpValue::Variant {
                    tag: 0,
                    value: Box::new(OpValue::Unit),
                }),
            },
            (Some("arg"), "count", [OpValue::Handle(ARGS_HANDLE)]) => OpValue::Bits {
                scalar: ScalarKind::U64,
                bits: 0,
            },
            (Some("io"), "monotonic_clock" | "wall_clock", [OpValue::Handle(IO_HANDLE)]) => {
                OpValue::Variant {
                    tag: 1,
                    value: Box::new(OpValue::Variant {
                        tag: 0,
                        value: Box::new(OpValue::Unit),
                    }),
                }
            }
            (Some("io"), name @ ("random" | "tasks"), [OpValue::Handle(IO_HANDLE)]) => {
                let grant = match name {
                    "random" => IoGrant::Random,
                    _ => IoGrant::Tasks,
                };
                io_ok(OpValue::Handle(self.grant(grant)))
            }
            (Some("instant"), "ticks", [OpValue::Handle(ticks)]) => OpValue::Bits {
                scalar: ScalarKind::U64,
                bits: u128::from(*ticks),
            },
            (
                Some("io" | "dir"),
                kind @ ("dir" | "narrow"),
                [OpValue::Handle(parent), OpValue::Handle(path)],
            ) => {
                let requested = self.paths.get(path.checked_sub(1)? as usize)?;
                let selected = if kind == "dir" && *parent == IO_HANDLE {
                    self.mounts.iter().find_map(|(selector, access)| {
                        let local = if selector.is_empty() {
                            Some(requested.as_str())
                        } else if requested == selector {
                            Some("")
                        } else {
                            requested.strip_prefix(selector)?.strip_prefix('/')
                        }?;
                        Some((0, selector.clone(), *access, local.to_owned()))
                    })
                } else {
                    match self.grants.get(parent) {
                        Some(IoGrant::Dir { tree, root, access }) => {
                            Some((*tree, root.clone(), *access, requested.clone()))
                        }
                        _ => return Some(OpOutcome::Fail("CAP-2".into())),
                    }
                };
                if let Some((tree, root, access, path)) = selected {
                    match io_relative(&root, &path) {
                        Some(root) if self.trees[&tree].0.contains(&root) => {
                            io_ok(OpValue::Handle(self.grant(IoGrant::Dir {
                                tree,
                                root,
                                access,
                            })))
                        }
                        Some(root) => io_portable(if self.trees[&tree].1.contains_key(&root) {
                            7
                        } else {
                            3
                        }),
                        None => io_error(1),
                    }
                } else {
                    io_not_granted()
                }
            }
            (
                Some("file"),
                "read",
                [
                    OpValue::Handle(directory),
                    OpValue::Handle(file),
                    buffer,
                    OpValue::Bits {
                        scalar: ScalarKind::U64,
                        bits: max,
                    },
                ],
            ) => {
                if !matches!(self.grants.get(directory), Some(IoGrant::Dir { .. })) {
                    return Some(OpOutcome::Fail("CAP-2".into()));
                }
                let Some(IoGrant::File {
                    directory: owner,
                    position,
                    writable,
                    contents,
                    ..
                }) = self.grants.get_mut(file)
                else {
                    return Some(OpOutcome::Fail("CAP-2".into()));
                };
                if owner != directory {
                    return Some(OpOutcome::Fail("CAP-2".into()));
                }
                if *writable {
                    return Some(OpOutcome::Value(io_error(1)));
                }
                let mut bytes = match buffer {
                    OpValue::Elements {
                        scalar: ScalarKind::U8,
                        bits,
                    } => bits.clone(),
                    OpValue::Bytes(bytes) => bytes.iter().copied().map(u128::from).collect(),
                    _ => return Some(OpOutcome::Unsupported),
                };
                let start = (*position).min(contents.len() as u64) as usize;
                let n = (*max).min(65536).min((contents.len() - start) as u128) as usize;
                bytes.extend(contents[start..start + n].iter().copied().map(u128::from));
                *position += n as u64;
                return Some(OpOutcome::Updated {
                    value: io_ok(OpValue::Bits {
                        scalar: ScalarKind::U64,
                        bits: n as u128,
                    }),
                    arguments: vec![(
                        2,
                        OpValue::Elements {
                            scalar: ScalarKind::U8,
                            bits: bytes,
                        },
                    )],
                });
            }
            (
                Some("file"),
                "seek",
                [
                    OpValue::Handle(directory),
                    OpValue::Handle(file),
                    OpValue::Bits {
                        scalar: ScalarKind::U64,
                        bits: offset,
                    },
                ],
            ) => {
                if !matches!(self.grants.get(directory), Some(IoGrant::Dir { .. })) {
                    return Some(OpOutcome::Fail("CAP-2".into()));
                }
                let Some(IoGrant::File {
                    directory: owner,
                    position,
                    ..
                }) = self.grants.get_mut(file)
                else {
                    return Some(OpOutcome::Fail("CAP-2".into()));
                };
                if owner != directory {
                    return Some(OpOutcome::Fail("CAP-2".into()));
                }
                if *offset > i64::MAX as u128 {
                    io_error(1)
                } else {
                    *position = *offset as u64;
                    io_ok(OpValue::Unit)
                }
            }
            (
                Some("file"),
                name @ ("write" | "append"),
                [
                    OpValue::Handle(directory),
                    OpValue::Handle(file),
                    OpValue::Bytes(bytes),
                ],
            ) => {
                let Some(IoGrant::Dir { tree, access, .. }) = self.grants.get(directory).cloned()
                else {
                    return Some(OpOutcome::Fail("CAP-2".into()));
                };
                let Some(IoGrant::File {
                    tree: owner_tree,
                    directory: owner,
                    writable,
                    identity,
                    contents,
                    ..
                }) = self.grants.get(file).cloned()
                else {
                    return Some(OpOutcome::Fail("CAP-2".into()));
                };
                if tree != owner_tree || owner != *directory {
                    return Some(OpOutcome::Fail("CAP-2".into()));
                }
                if access == Access::Read {
                    io_not_granted()
                } else if !writable {
                    io_error(1)
                } else {
                    let path = self
                        .files
                        .iter()
                        .find(|(_, id)| std::sync::Arc::ptr_eq(id, &identity))
                        .map(|((_, path), _)| path.clone());
                    let mut data = contents;
                    data.extend_from_slice(bytes);
                    if let Some(path) = path {
                        self.trees.get_mut(&tree).unwrap().1.insert(path, data);
                    }
                    self.update_files(&identity, bytes, true);
                    io_ok(if name == "write" {
                        OpValue::Bits {
                            scalar: ScalarKind::U64,
                            bits: bytes.len() as u128,
                        }
                    } else {
                        OpValue::Unit
                    })
                }
            }
            (Some("file"), "narrow", [OpValue::Handle(directory), OpValue::Handle(file)]) => {
                let old = self.grants.remove(file);
                let result = match (self.grants.get(directory).cloned(), old) {
                    (
                        Some(IoGrant::Dir { tree, root, .. }),
                        Some(IoGrant::File {
                            tree: old,
                            path,
                            identity,
                            position,
                            writable,
                            contents,
                            ..
                        }),
                    ) => {
                        let fits = tree == old
                            && (root.is_empty() || path.starts_with(&format!("{root}/")))
                            && self
                                .files
                                .get(&(tree, path.clone()))
                                .is_some_and(|current| std::sync::Arc::ptr_eq(current, &identity));
                        if fits {
                            OpValue::Variant {
                                tag: 1,
                                value: Box::new(OpValue::OpenHandle(self.grant(IoGrant::File {
                                    tree,
                                    path,
                                    identity,
                                    directory: *directory,
                                    position,
                                    writable,
                                    contents,
                                }))),
                            }
                        } else {
                            OpValue::Variant {
                                tag: 0,
                                value: Box::new(OpValue::Unit),
                            }
                        }
                    }
                    _ => {
                        return Some(OpOutcome::Complete {
                            result: Err("CAP-2".into()),
                            consumed: vec![1],
                        });
                    }
                };
                return Some(OpOutcome::Complete {
                    result: Ok(result),
                    consumed: vec![1],
                });
            }
            (
                Some("file"),
                "commit" | "write_all",
                [OpValue::Handle(directory), OpValue::Handle(file), rest @ ..],
            ) => {
                let Some(IoGrant::Dir { tree, access, .. }) = self.grants.get(directory) else {
                    return Some(OpOutcome::Fail("CAP-2".into()));
                };
                if *access == Access::Read {
                    return Some(OpOutcome::Value(io_not_granted()));
                }
                let tree = *tree;
                let Some(IoGrant::Atomic {
                    tree: owner,
                    directory: grant,
                    temporary,
                    destination,
                    identity,
                }) = self.grants.get(file).cloned()
                else {
                    return Some(OpOutcome::Fail("CAP-2".into()));
                };
                if tree != owner || *directory != grant {
                    return Some(OpOutcome::Fail("CAP-2".into()));
                }
                if name == "commit" {
                    if !self
                        .files
                        .get(&(tree, temporary.clone()))
                        .is_some_and(|current| std::sync::Arc::ptr_eq(current, &identity))
                    {
                        self.grants.remove(file);
                        self.trees.get_mut(&tree).unwrap().1.remove(&temporary);
                        self.files.remove(&(tree, temporary));
                        return Some(OpOutcome::Complete {
                            result: Ok(io_error(1)),
                            consumed: vec![1],
                        });
                    }
                    let bytes = self.trees.get_mut(&tree).unwrap().1.remove(&temporary)?;
                    self.trees
                        .get_mut(&tree)
                        .unwrap()
                        .1
                        .insert(destination.clone(), bytes);
                    let identity = self
                        .files
                        .remove(&(tree, temporary))
                        .expect("atomic file identity");
                    self.files.insert((tree, destination), identity);
                    self.grants.remove(file);
                    return Some(OpOutcome::Complete {
                        result: Ok(io_ok(OpValue::Unit)),
                        consumed: vec![1],
                    });
                }
                let [OpValue::Bytes(bytes)] = rest else {
                    return Some(OpOutcome::Unsupported);
                };
                self.trees
                    .get_mut(&tree)
                    .unwrap()
                    .1
                    .get_mut(&temporary)?
                    .extend_from_slice(bytes);
                self.update_files(&identity, bytes, true);
                io_ok(OpValue::Unit)
            }
            (Some("file"), "walk", [OpValue::Handle(directory)]) => {
                let Some(IoGrant::Dir { tree, root, .. }) = self.grants.get(directory) else {
                    return Some(OpOutcome::Fail("CAP-2".into()));
                };
                let prefix = if root.is_empty() {
                    String::new()
                } else {
                    format!("{root}/")
                };
                let paths: Vec<_> = self.trees[tree]
                    .1
                    .keys()
                    .filter_map(|p| p.strip_prefix(&prefix).map(str::to_owned))
                    .collect();
                let items = paths
                    .iter()
                    .map(|p| io_ok(OpValue::Handle(self.path(p))))
                    .collect();
                io_ok(OpValue::Iterator(items))
            }
            (
                Some("file"),
                kind @ ("open" | "create" | "size" | "read_bytes" | "write_bytes" | "remove"
                | "create_atomic"),
                [OpValue::Handle(directory), OpValue::Handle(path), rest @ ..],
            ) => {
                let Some(IoGrant::Dir { tree, root, access }) = self.grants.get(directory).cloned()
                else {
                    return Some(OpOutcome::Fail("CAP-2".into()));
                };
                if access == Access::Read
                    && matches!(kind, "create" | "write_bytes" | "remove" | "create_atomic")
                {
                    return Some(OpOutcome::Value(io_not_granted()));
                }
                let text = self.paths.get(path.checked_sub(1)? as usize)?;
                let Some(path) = io_relative(&root, text) else {
                    return Some(OpOutcome::Value(io_error(1)));
                };
                let mut prefix = String::new();
                let parts: Vec<_> = path.split('/').filter(|p| !p.is_empty()).collect();
                for part in parts.iter().take(parts.len().saturating_sub(1)) {
                    if !prefix.is_empty() {
                        prefix.push('/');
                    }
                    prefix.push_str(part);
                    if !self.trees[&tree].0.contains(&prefix) {
                        return Some(OpOutcome::Value(io_portable(
                            if self.trees[&tree].1.contains_key(&prefix) {
                                7
                            } else {
                                3
                            },
                        )));
                    }
                }
                match kind {
                    "open" if self.trees[&tree].1.contains_key(&path) => {
                        io_ok(OpValue::OpenHandle(self.grant(IoGrant::File {
                            identity: self.files[&(tree, path.clone())].clone(),
                            directory: *directory,
                            position: 0,
                            writable: false,
                            contents: self.trees[&tree].1[&path].clone(),
                            tree,
                            path,
                        })))
                    }
                    "create" => {
                        let parent = path.rsplit_once('/').map_or("", |(p, _)| p);
                        if self.trees[&tree].0.contains(&path)
                            || !self.trees[&tree].0.contains(parent)
                        {
                            io_portable(if self.trees[&tree].0.contains(&path) {
                                6
                            } else {
                                3
                            })
                        } else {
                            let identity = self
                                .files
                                .entry((tree, path.clone()))
                                .or_insert_with(|| std::sync::Arc::new(()))
                                .clone();
                            self.trees
                                .get_mut(&tree)
                                .unwrap()
                                .1
                                .insert(path.clone(), vec![]);
                            self.update_files(&identity, &[], false);
                            io_ok(OpValue::OpenHandle(self.grant(IoGrant::File {
                                tree,
                                path,
                                directory: *directory,
                                position: 0,
                                writable: true,
                                contents: vec![],
                                identity,
                            })))
                        }
                    }
                    "size" => self.trees[&tree]
                        .1
                        .get(&path)
                        .map(|bytes| {
                            io_ok(OpValue::Bits {
                                scalar: ScalarKind::U64,
                                bits: bytes.len() as u128,
                            })
                        })
                        .unwrap_or_else(|| {
                            io_portable(if self.trees[&tree].0.contains(&path) {
                                6
                            } else {
                                3
                            })
                        }),
                    "read_bytes" => self.trees[&tree]
                        .1
                        .get(&path)
                        .map(|bytes| io_ok(OpValue::ByteSequence(bytes.clone())))
                        .unwrap_or_else(|| {
                            io_portable(if self.trees[&tree].0.contains(&path) {
                                6
                            } else {
                                3
                            })
                        }),
                    "remove" => {
                        if self.trees.get_mut(&tree).unwrap().1.remove(&path).is_some() {
                            self.files.remove(&(tree, path));
                            io_ok(OpValue::Unit)
                        } else {
                            io_portable(if self.trees[&tree].0.contains(&path) {
                                6
                            } else {
                                3
                            })
                        }
                    }
                    "write_bytes" | "create_atomic" => {
                        let parent = path.rsplit_once('/').map_or("", |(p, _)| p);
                        if self.trees[&tree].0.contains(&path)
                            || !self.trees[&tree].0.contains(parent)
                        {
                            io_portable(if self.trees[&tree].0.contains(&path) {
                                6
                            } else {
                                3
                            })
                        } else if kind == "write_bytes" {
                            let [OpValue::Bytes(bytes)] = rest else {
                                return Some(OpOutcome::Unsupported);
                            };
                            self.trees
                                .get_mut(&tree)
                                .unwrap()
                                .1
                                .insert(path.clone(), bytes.clone());
                            self.files.insert((tree, path), std::sync::Arc::new(()));
                            io_ok(OpValue::Unit)
                        } else {
                            let mut serial = self.next;
                            let temporary = loop {
                                let p = format!("{path}.hirsel-tmp-{serial}");
                                if !self.trees[&tree].1.contains_key(&p)
                                    && !self.trees[&tree].0.contains(&p)
                                {
                                    break p;
                                }
                                serial += 1;
                            };
                            self.trees
                                .get_mut(&tree)
                                .unwrap()
                                .1
                                .insert(temporary.clone(), vec![]);
                            self.files
                                .insert((tree, temporary.clone()), std::sync::Arc::new(()));
                            io_ok(OpValue::OpenHandle(self.grant(IoGrant::Atomic {
                                tree,
                                directory: *directory,
                                temporary: temporary.clone(),
                                destination: path,
                                identity: self.files[&(tree, temporary.clone())].clone(),
                            })))
                        }
                    }
                    _ => io_portable(if self.trees[&tree].0.contains(&path) {
                        6
                    } else {
                        3
                    }),
                }
            }
            (Some("test"), "dir", [OpValue::Handle(test), OpValue::Sequence(entries)])
                if matches!(self.grants.get(test), Some(IoGrant::Test)) =>
            {
                let mut files = BTreeMap::new();
                let mut directories = BTreeSet::from([String::new()]);
                for entry in entries {
                    let OpValue::Aggregate(fields) = entry else {
                        return None;
                    };
                    let [OpValue::Handle(path), OpValue::Bytes(bytes)] = fields.as_slice() else {
                        return None;
                    };
                    let path = io_relative("", self.paths.get(path.checked_sub(1)? as usize)?)?;
                    let mut parent = path.as_str();
                    while let Some((p, _)) = parent.rsplit_once('/') {
                        directories.insert(p.into());
                        parent = p;
                    }
                    files.insert(path, bytes.clone());
                }
                if files.keys().any(|p| directories.contains(p)) {
                    return Some(OpOutcome::Unsupported);
                }
                let tree = self.trees.keys().next_back().copied().unwrap() + 1;
                for path in files.keys() {
                    self.files
                        .insert((tree, path.clone()), std::sync::Arc::new(()));
                }
                self.trees.insert(tree, (directories, files));
                OpValue::Handle(self.grant(IoGrant::Dir {
                    tree,
                    root: String::new(),
                    access: Access::ReadWrite,
                }))
            }
            (
                Some("test"),
                "check",
                [
                    OpValue::Handle(test),
                    OpValue::Bits {
                        scalar: ScalarKind::Bool,
                        bits,
                    },
                ],
            ) if matches!(self.grants.get(test), Some(IoGrant::Test)) => {
                if *bits == 0 {
                    return Some(OpOutcome::Fail("LANG-9".into()));
                }
                OpValue::Unit
            }
            (Some("test"), "fail", [OpValue::Handle(test)])
                if matches!(self.grants.get(test), Some(IoGrant::Test)) =>
            {
                return Some(OpOutcome::Fail("LANG-9".into()));
            }
            _ => return None,
        };
        Some(OpOutcome::Value(value))
    }
}

/// Semantic text kernels return bytes or input-relative ranges; H owns every block.
fn text(module: Option<&str>, name: &str, args: &[OpValue]) -> Option<OpOutcome> {
    use hirsel_lir::layout_types::ScalarKind;
    let value = match (module, name, args) {
        (Some("string"), "as_bytes", [OpValue::Bytes(bytes)]) => OpValue::Slice {
            argument: 0,
            start: 0,
            length: bytes.len() as u64,
        },
        (
            Some("bytes"),
            "from_list",
            [
                OpValue::Elements {
                    scalar: ScalarKind::U8,
                    bits,
                },
            ],
        ) => OpValue::ByteSequence(bits.iter().map(|&b| b as u8).collect()),
        (
            Some("bytes"),
            "slice",
            [
                OpValue::Bytes(bytes),
                OpValue::Bits {
                    scalar: ScalarKind::U64,
                    bits: start,
                },
                OpValue::Bits {
                    scalar: ScalarKind::U64,
                    bits: end,
                },
            ],
        ) => {
            if start > end || *end > bytes.len() as u128 {
                return Some(OpOutcome::Fail("SEM-3".into()));
            }
            OpValue::Slice {
                argument: 0,
                start: *start as u64,
                length: (*end - *start) as u64,
            }
        }
        (
            Some("string"),
            "position",
            [
                OpValue::Bytes(bytes),
                OpValue::Bits {
                    scalar: ScalarKind::U64,
                    bits: offset,
                },
            ],
        ) => {
            let text = std::str::from_utf8(bytes).expect("valid string");
            let valid = usize::try_from(*offset)
                .ok()
                .is_some_and(|offset| text.is_char_boundary(offset));
            OpValue::Variant {
                tag: u32::from(valid),
                value: Box::new(if valid {
                    OpValue::Plain((*offset as u64).to_le_bytes().to_vec())
                } else {
                    OpValue::Unit
                }),
            }
        }
        (
            Some("string"),
            "byte_offset",
            [
                OpValue::Bytes(_),
                OpValue::Bits {
                    scalar: ScalarKind::U64,
                    bits,
                },
            ],
        ) => OpValue::Bits {
            scalar: ScalarKind::U64,
            bits: *bits,
        },
        (Some("string"), "from_bytes", [OpValue::Bytes(bytes)]) => OpValue::Variant {
            tag: u32::from(hirsel_rt::kernels::string_from_bytes(bytes.clone()).is_err()),
            value: Box::new(OpValue::Slice {
                argument: 0,
                start: 0,
                length: bytes.len() as u64,
            }),
        },
        (
            Some("string"),
            "slice",
            [
                OpValue::Bytes(bytes),
                OpValue::Bits {
                    scalar: ScalarKind::U64,
                    bits: from,
                },
                OpValue::Bits {
                    scalar: ScalarKind::U64,
                    bits: to,
                },
            ],
        ) => {
            let text = std::str::from_utf8(bytes).expect("SEM-3: String is valid UTF-8");
            let (Ok(from), Ok(to)) = (u64::try_from(*from), u64::try_from(*to)) else {
                return Some(OpOutcome::Fail("SEM-3".into()));
            };
            let Some(part) = hirsel_rt::kernels::string_slice(text, from, to) else {
                return Some(OpOutcome::Fail("SEM-3".into()));
            };
            OpValue::Slice {
                argument: 0,
                start: from,
                length: part.len() as u64,
            }
        }
        _ => return None,
    };
    Some(OpOutcome::Value(value))
}

/// Independent byte-level model; no production cursor/allocator/count helper runs here.
fn views(name: &str, args: &[OpValue]) -> Option<OpOutcome> {
    let (protocol, step) = name.rsplit_once('_')?;
    if !matches!(protocol, "UnicodeScalars" | "ScalarPositions") {
        return None;
    }
    if step == "open" {
        let [OpValue::Bytes(bytes)] = args else {
            return Some(OpOutcome::Unsupported);
        };
        return Some(OpOutcome::Value(OpValue::ViewCursor {
            bytes: bytes.clone(),
            position: 0,
        }));
    }
    let [OpValue::ViewCursor { bytes, position }] = args else {
        return Some(OpOutcome::Unsupported);
    };
    let text = std::str::from_utf8(bytes).expect("SEM-3: String is valid UTF-8");
    let mut next = *position;
    let value = match protocol {
        "UnicodeScalars" => {
            let mut words = vec![0u8; 8];
            if let Some(c) = text
                .get(*position as usize..)
                .and_then(|s| s.chars().next())
            {
                next += c.len_utf8() as u64;
                words[..4].copy_from_slice(&1u32.to_le_bytes());
                words[4..].copy_from_slice(&u32::from(c).to_le_bytes());
            }
            OpValue::Plain(words)
        }
        "ScalarPositions" => {
            let mut words = vec![0u8; 16];
            if *position <= bytes.len() as u64 {
                words[..4].copy_from_slice(&1u32.to_le_bytes());
                words[8..].copy_from_slice(&position.to_le_bytes());
                next += text[*position as usize..]
                    .chars()
                    .next()
                    .map_or(1, |c| c.len_utf8() as u64);
            }
            OpValue::Plain(words)
        }
        _ => unreachable!(),
    };
    Some(OpOutcome::Updated {
        value,
        arguments: vec![(
            0,
            OpValue::ViewCursor {
                bytes: bytes.clone(),
                position: next,
            },
        )],
    })
}

/// One audited heap-executor run.
pub struct HeapRun {
    pub observation: Observation,
    /// Failure-capable sites the run executed: the fault campaign's domain.
    pub sites: u64,
    /// Instrumentation only; no check reads them.
    pub counters: Counters,
}

fn config(inject: Option<u64>, kind: &super::link::LaunchKind) -> Config {
    Config {
        inject,
        context_handles: match kind {
            super::link::LaunchKind::Zero => vec![],
            super::link::LaunchKind::Test => vec![(IO_HANDLE, "Test".into())],
            super::link::LaunchKind::Main(roots) => roots
                .iter()
                .map(|root| (super::link::root_handle(*root), format!("{root:?}")))
                .collect(),
        },
        ..Config::default()
    }
}

fn heap_program(program: &ReadyProgram) -> hirsel_heapcheck::Program<hirsel_lir::stage::Ready> {
    hirsel_heapcheck::Program {
        target: program.target.clone(),
        layouts: program.table.clone(),
        units: program.raw_units(),
        statics: program.statics.clone(),
    }
}

fn violation(entry: &DefPath, v: &hirsel_heapcheck::Violation, site: Option<u64>) -> NativeError {
    let injected = site.map_or(String::new(), |s| {
        format!(" (failure injected at site {s})")
    });
    NativeError {
        definition: Some(entry.clone()),
        ..NativeError::new(StageName::Heap, format!("{:?}: {v}{injected}", v.kind))
    }
}

fn observation(
    audit: &hirsel_heapcheck::Audit,
    sites: &Sites<'_>,
) -> Result<Observation, NativeError> {
    let o = &audit.observation;
    let exit_code = match &o.returned {
        None => None,
        Some(Observed::Variant { tag, .. }) => Some(i32::from(*tag != 0)),
        Some(other) => {
            return Err(NativeError::new(
                StageName::Heap,
                format!("the entry returned {other:?}, not a Result"),
            ));
        }
    };
    Ok(Observation {
        exit_code,
        output: o.output.clone(),
        failures: o
            .failures
            .iter()
            .map(|f| {
                let site = SiteRange {
                    start: f.site.0,
                    end: f.site.1,
                };
                sites.failure(&f.rule, &f.definition, site)
            })
            .collect::<Result<_, _>>()?,
    })
}

/// Run `entry` once and audit it.
pub fn run_heap(
    program: &ReadyProgram,
    entry: &DefPath,
    sites: &Sites<'_>,
) -> Result<HeapRun, NativeError> {
    let heap = heap_program(program);
    let kind = program
        .launches
        .get(entry)
        .ok_or_else(|| NativeError::new(StageName::Launch, "no typed launch kind"))?;
    let args = match kind {
        super::link::LaunchKind::Zero => vec![],
        super::link::LaunchKind::Test => vec![Arg::Handle(IO_HANDLE)],
        super::link::LaunchKind::Main(roots) => roots
            .iter()
            .map(|root| Arg::Handle(super::link::root_handle(*root)))
            .collect(),
    };
    let run = hirsel_heapcheck::run_ready(
        &heap,
        entry,
        &args,
        &mut Io::for_launch(kind, program.launch.test_seed),
        &config(None, kind),
    );
    let audit = run.result.as_ref().map_err(|v| violation(entry, v, None))?;
    Ok(HeapRun {
        observation: observation(audit, sites)?,
        sites: run.sites,
        counters: run.counters,
    })
}

/// A clean run, then one run per failure-capable site with that site failed:
/// every failure path must pass the terminal audit. Returns the clean run.
pub fn heap_campaign(
    program: &ReadyProgram,
    entry: &DefPath,
    sites: &Sites<'_>,
) -> Result<HeapRun, NativeError> {
    let heap = heap_program(program);
    let kind = program
        .launches
        .get(entry)
        .ok_or_else(|| NativeError::new(StageName::Launch, "no typed launch kind"))?;
    let args = match kind {
        super::link::LaunchKind::Zero => vec![],
        super::link::LaunchKind::Test => vec![Arg::Handle(IO_HANDLE)],
        super::link::LaunchKind::Main(roots) => roots
            .iter()
            .map(|root| Arg::Handle(super::link::root_handle(*root)))
            .collect(),
    };
    let campaign = hirsel_heapcheck::fault_campaign_ready(
        &heap,
        entry,
        &args,
        &mut || Box::new(Io::for_launch(kind, program.launch.test_seed)),
        &config(None, kind),
    );
    if let Some((site, v)) = campaign.first_violation() {
        return Err(violation(entry, v, site));
    }
    let audit = campaign.clean.result.as_ref().expect("checked above");
    Ok(HeapRun {
        observation: observation(audit, sites)?,
        sites: campaign.clean.sites,
        counters: campaign.clean.counters,
    })
}

#[cfg(test)]
mod mount_rights {
    use super::*;

    fn value(io: &mut Io, module: &str, operation: &str, args: &[OpValue]) -> OpValue {
        match io
            .io(Some(module), operation, args)
            .expect("supported mount operation")
        {
            OpOutcome::Value(value) => value,
            _ => panic!("expected a Result value"),
        }
    }
    fn mounted(access: Access) -> (Io, u64) {
        let mut io = Io::new();
        io.mounts = BTreeMap::from([("work".into(), access)]);
        io.trees
            .get_mut(&0)
            .unwrap()
            .0
            .extend(["work".into(), "work/sub".into()]);
        let path = io.path("work");
        let OpValue::Variant {
            tag: 0,
            value: handle,
        } = value(
            &mut io,
            "io",
            "dir",
            &[OpValue::Handle(IO_HANDLE), OpValue::Handle(path)],
        )
        else {
            panic!("mount acquisition");
        };
        let OpValue::Handle(directory) = *handle else {
            panic!("directory grant");
        };
        (io, directory)
    }

    #[test]
    fn heap_mount_selection_denies_absent_selectors_with_an_authorized_twin() {
        let (mut io, directory) = mounted(Access::Read);
        for denied in ["", "other", "working"] {
            let path = io.path(denied);
            let before = io.next;
            assert_eq!(
                value(
                    &mut io,
                    "io",
                    "dir",
                    &[OpValue::Handle(IO_HANDLE), OpValue::Handle(path)]
                ),
                io_not_granted()
            );
            assert_eq!(io.next, before);
        }
        assert!(matches!(
            io.grants[&directory],
            IoGrant::Dir {
                access: Access::Read,
                ..
            }
        ));
        io.mounts = BTreeMap::from([("other".into(), Access::ReadWrite)]);
        let work = io.path("work");
        assert_eq!(
            value(
                &mut io,
                "io",
                "dir",
                &[OpValue::Handle(IO_HANDLE), OpValue::Handle(work)]
            ),
            io_not_granted()
        );
    }

    #[test]
    fn heap_narrow_inherits_read_rights_and_denies_mutation_before_effects() {
        for access in [Access::Read, Access::ReadWrite] {
            let (mut io, directory) = mounted(access);
            let sub = io.path("sub");
            let OpValue::Variant {
                tag: 0,
                value: handle,
            } = value(
                &mut io,
                "dir",
                "narrow",
                &[OpValue::Handle(directory), OpValue::Handle(sub)],
            )
            else {
                panic!();
            };
            let OpValue::Handle(narrowed) = *handle else {
                panic!();
            };
            let a = io.path("a");
            let outcome = value(
                &mut io,
                "file",
                "write_bytes",
                &[
                    OpValue::Handle(narrowed),
                    OpValue::Handle(a),
                    OpValue::Bytes(b"written".to_vec()),
                ],
            );
            assert_eq!(
                outcome,
                if access == Access::Read {
                    io_not_granted()
                } else {
                    io_ok(OpValue::Unit)
                }
            );
            assert_eq!(
                io.trees[&0].1.contains_key("work/sub/a"),
                access == Access::ReadWrite
            );
            for operation in ["remove", "create_atomic"] {
                let outcome = value(
                    &mut io,
                    "file",
                    operation,
                    &[OpValue::Handle(narrowed), OpValue::Handle(a)],
                );
                if access == Access::Read {
                    assert_eq!(outcome, io_not_granted());
                    assert!(io.trees[&0].1.is_empty());
                } else if let OpValue::Variant {
                    tag: 0,
                    value: file,
                } = outcome
                {
                    if let OpValue::OpenHandle(id) = *file {
                        io.close_handle(id).unwrap();
                    }
                } else {
                    panic!("writable twin");
                }
            }
        }
    }

    #[test]
    fn heap_file_narrow_requires_virtual_containment_even_for_shared_storage() {
        let (mut io, parent) = mounted(Access::ReadWrite);
        io.mounts.insert("child".into(), Access::Read);
        io.trees.get_mut(&0).unwrap().0.insert("child".into());
        let child = io.grant(IoGrant::Dir {
            tree: 0,
            root: "child".into(),
            access: Access::Read,
        });
        let sub = io.grant(IoGrant::Dir {
            tree: 0,
            root: "work/sub".into(),
            access: Access::ReadWrite,
        });
        // One identity accessible under physically overlapping mounts still has
        // distinct virtual names. The target selector must contain the old name.
        let identity = std::sync::Arc::new(());
        io.files.insert((0, "work/sub/a".into()), identity.clone());
        io.files.insert((0, "child/a".into()), identity.clone());
        for (target, expected_tag) in [(child, 0), (sub, 1), (parent, 1)] {
            let file = io.grant(IoGrant::File {
                tree: 0,
                path: "work/sub/a".into(),
                directory: parent,
                position: 0,
                writable: false,
                contents: vec![],
                identity: identity.clone(),
            });
            let Some(OpOutcome::Complete {
                result: Ok(OpValue::Variant { tag, value }),
                consumed,
            }) = io.io(
                Some("file"),
                "narrow",
                &[OpValue::Handle(target), OpValue::Handle(file)],
            )
            else {
                panic!("narrow completion");
            };
            assert_eq!(tag, expected_tag);
            assert_eq!(consumed, [1]);
            if let OpValue::OpenHandle(file) = *value {
                io.close_handle(file).unwrap();
            }
        }
    }

    #[test]
    fn heap_atomic_updates_deny_read_rights_with_a_writable_twin() {
        for access in [Access::Read, Access::ReadWrite] {
            let (mut io, directory) = mounted(access);
            let identity = std::sync::Arc::new(());
            io.trees
                .get_mut(&0)
                .unwrap()
                .1
                .insert("work/tmp".into(), vec![]);
            io.files.insert((0, "work/tmp".into()), identity.clone());
            let atomic = io.grant(IoGrant::Atomic {
                tree: 0,
                directory,
                temporary: "work/tmp".into(),
                destination: "work/done".into(),
                identity,
            });
            let outcome = value(
                &mut io,
                "file",
                "write_all",
                &[
                    OpValue::Handle(directory),
                    OpValue::Handle(atomic),
                    OpValue::Bytes(b"written".to_vec()),
                ],
            );
            assert_eq!(
                outcome,
                if access == Access::Read {
                    io_not_granted()
                } else {
                    io_ok(OpValue::Unit)
                }
            );
            assert_eq!(
                io.trees[&0].1["work/tmp"].as_slice(),
                if access == Access::Read {
                    b"".as_slice()
                } else {
                    b"written".as_slice()
                }
            );
            if access == Access::Read {
                assert_eq!(
                    value(
                        &mut io,
                        "file",
                        "commit",
                        &[OpValue::Handle(directory), OpValue::Handle(atomic)]
                    ),
                    io_not_granted()
                );
                assert!(io.grants.contains_key(&atomic));
            } else {
                assert!(matches!(
                    io.io(
                        Some("file"),
                        "commit",
                        &[OpValue::Handle(directory), OpValue::Handle(atomic)]
                    ),
                    Some(OpOutcome::Complete { result: Ok(_), .. })
                ));
                assert_eq!(io.trees[&0].1["work/done"], b"written");
            }
        }
    }
}

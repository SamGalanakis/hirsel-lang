//! Exact tape replay: validation precedes every visible state change, even handle consumption.
use crate::handles::Handles;
use crate::io::*;
#[derive(Clone, Debug)]
pub struct SimIo {
    state: State,
    tree: Tree,
    tape: Vec<Event>,
    position: usize,
    roots: std::collections::BTreeMap<u64, String>,
    identities: FileIdentities,
    detached_roots: std::collections::BTreeSet<u64>,
    mount_pins: MountPins,
    /// The requests the tape records as `blocked`, parked again in replay.
    waits: crate::readiness::WaitLog,
}
impl SimIo {
    pub fn set_env(&mut self, env: std::collections::BTreeMap<String, Option<String>>) {
        self.state.env = env;
    }
    pub fn stderr(&self) -> &[u8] {
        &self.state.stderr
    }

    /// Configure the root Net's isolated scripted world before entry. Rules are
    /// supplied separately by set_net_rules; peer data never widens them.
    pub fn set_net_script(
        &mut self,
        peers: Vec<hirsel_run_config::NetPeer>,
        listeners: Vec<hirsel_run_config::NetListener>,
    ) -> Result<(), AdapterError> {
        self.state.set_net_script(peers, listeners)
    }
    /// Replay uses the admitted projection set, independently of tape completions.
    pub fn set_projections(&mut self, projections: std::collections::BTreeSet<String>) {
        self.state.projections = projections;
    }
    pub fn new(tree: Tree, tape: Vec<Event>) -> Self {
        Self {
            state: State::default(),
            detached_roots: Default::default(),
            mount_pins: MountPins {
                roots: std::collections::BTreeMap::from([(String::new(), String::new())]),
                detached: Default::default(),
            },
            identities: FileIdentities::new(&tree),
            tree,
            tape,
            position: 0,
            roots: Default::default(),
            waits: Default::default(),
        }
    }
    /// Configure only the virtual selectors and rights; replay acquires no host authority.
    pub fn set_mounts(
        &mut self,
        mounts: impl IntoIterator<Item = (String, hirsel_run_config::Access)>,
    ) -> Result<(), hirsel_run_config::RunRefusal> {
        let mounts = mount_access(mounts)?;
        let mut directories = self.tree.directories.clone();
        for selector in mounts.keys() {
            let mut path = selector.as_str();
            loop {
                if self.tree.files.contains_key(path) {
                    return Err(hirsel_run_config::RunRefusal::Invalid {
                        field: "io.dirs".into(),
                        detail: "mount directory collides with a snapshot file".into(),
                    });
                }
                // A root alias identifies shared snapshot storage; it must not
                // be replaced by an independent empty virtual directory.
                if !self.tree.links.contains_key(path) {
                    directories.insert(path.into());
                }
                let Some((parent, _)) = path.rsplit_once('/') else {
                    break;
                };
                path = parent;
            }
        }
        self.tree.directories = directories;
        self.mount_pins.roots = mounts
            .keys()
            .filter_map(|selector| {
                self.tree
                    .resolve("", selector, true)
                    .ok()
                    .map(|root| (selector.clone(), root))
            })
            .collect();
        self.mount_pins.detached.clear();
        self.state.mounts = mounts;
        Ok(())
    }
    /// The admitted `io.process` grant. Absent, the root Proc runs nothing.
    pub fn set_process_grant(&mut self, grant: &hirsel_run_config::ProcessGrant) {
        self.state.set_process_grant(grant);
    }
    /// The bound RunConfig rule set; replay re-derives every grant decision from it.
    pub fn set_net_rules(&mut self, rules: std::collections::BTreeSet<NetRule>) {
        self.state.net_rules = rules;
    }
    pub fn clock_grants(&mut self, monotonic: bool, wall: bool) {
        self.state.monotonic_granted = monotonic;
        self.state.wall_granted = wall;
    }
    pub fn test_context(&mut self, seed: u64) {
        self.state.handles = Handles::test(seed);
    }
    pub fn grant_test(&mut self, seed: u64) -> u64 {
        self.state.grant_test(seed)
    }
    pub fn fake_trees(&self) -> &std::collections::BTreeMap<u64, Tree> {
        &self.state.fake_trees
    }
    pub fn finish(&self) -> Result<(), AdapterError> {
        if self.position == self.tape.len() {
            Ok(())
        } else {
            Err(AdapterError::UnconsumedInput(self.position))
        }
    }
    pub fn tree(&self) -> &Tree {
        &self.tree
    }
    pub fn handles(&self) -> &Handles {
        &self.state.handles
    }
    pub fn stdout(&self) -> &[u8] {
        &self.state.stdout
    }
    pub fn position(&self) -> usize {
        self.position
    }
    /// The last consumed event; its fault states a committed prefix.
    pub fn last_event(&self) -> Option<&Event> {
        self.position.checked_sub(1).map(|index| &self.tape[index])
    }
}
impl Adapter for SimIo {
    fn preflight(&self, request: &Request) -> Result<Option<Completion>, AdapterError> {
        self.state.preflight(request)
    }

    fn enter(&mut self, token: u64) {
        self.waits.enter(token);
    }
    fn finish(&mut self, token: u64) {
        self.waits.finish(token);
    }
    fn cancellable(&self, token: u64) -> bool {
        self.waits.rows_of(token).is_none_or(|row| !row.committed)
    }
    /// Ready exactly at the poll the recording made ready: the next event is its `ready`
    /// with this many failed polls.
    fn ready(&mut self, token: u64) -> Result<bool, AdapterError> {
        let next = self.tape.get(self.position).map(|event| &event.request);
        let Some(row) = self.waits.row(token) else {
            return Ok(true);
        };
        if next
            == Some(&Request::Ready {
                wait: row.ordinal,
                polls: row.polls,
            })
        {
            row.polls = 0;
            self.position += 1;
            return Ok(true);
        }
        row.polls += 1;
        Ok(false)
    }
    /// No host wait: idle polls continue only toward the next recorded `ready`, which must
    /// belong to a parked request that has not been polled past it.
    fn wait(&mut self) -> Result<bool, AdapterError> {
        if self.waits.rows().next().is_none() {
            return Ok(false);
        }
        let Some(Request::Ready { wait, polls }) =
            self.tape.get(self.position).map(|event| &event.request)
        else {
            return Err(AdapterError::MissingInput(self.position));
        };
        if self
            .waits
            .rows()
            .any(|row| row.ordinal == *wait && row.polls <= *polls)
        {
            Ok(true)
        } else {
            Err(AdapterError::ReplayMismatch(self.position))
        }
    }
    fn replay_stderr(&self) -> Vec<u8> {
        self.state.stderr.clone()
    }
    fn replay_stdout(&self) -> Vec<u8> {
        self.state.stdout.clone()
    }
    fn call(&mut self, request: Request) -> Result<Completion, AdapterError> {
        let event = self
            .tape
            .get(self.position)
            .ok_or(AdapterError::MissingInput(self.position))?;
        if event.request != request {
            return Err(AdapterError::ReplayMismatch(self.position));
        }
        // The recorded run parked this request: its task parks again (DET-1).
        if let Completion::Blocked { committed } = event.completion {
            self.position += 1;
            self.waits.park(committed);
            return Err(AdapterError::Blocked);
        }
        let mut state = self.state.clone();
        let mut tree = self.tree.clone();
        let mut roots = self.roots.clone();
        let mut identities = self.identities.clone();
        let mut detached = self.detached_roots.clone();
        let mut mount_pins = self.mount_pins.clone();
        let mut storage = LinkedTree {
            mount_pins: Some(&mut mount_pins),
            active_id: 0,
            detached: &mut detached,
            tree: &mut tree,
            roots: &mut roots,
            active: Default::default(),
            identities: &mut identities,
            metadata_input: match &event.completion {
                Completion::Metadata(Ok(meta)) => Some(meta.clone()),
                _ => None,
            },
            read_limit: match (&request, &event.completion) {
                (Request::FileRead { .. }, Completion::Bytes(Ok(b))) => Some(b.len() as u64),
                _ => None,
            },
            write_limit: match (&request, &event.completion) {
                (Request::FileWrite { .. }, Completion::Size(Ok(n))) => Some(*n),
                _ => None,
            },
        };
        let result = if state.is_process(&request) {
            if event.fault.is_some() {
                return Err(AdapterError::InvalidCompletion);
            }
            // The recorded completion stands in for the host; the checks are re-derived.
            state.execute_process(&mut |_| Ok(event.completion.clone()), &request)
        } else if matches!(
            request,
            Request::Monotonic { .. }
                | Request::Wall { .. }
                | Request::ReadStdin { .. }
                | Request::RandomBytes { .. }
                | Request::SleepUntil { .. }
        ) {
            if event.fault.is_some() {
                return Err(AdapterError::InvalidCompletion);
            }
            state.sample(&request, event.completion.clone())
        } else {
            let mut recorded = Recorded(&event.completion);
            state.execute(&mut storage, &mut recorded, &request, event.fault.as_ref())
        };
        let completion = match &result {
            Ok(completion) => completion.clone(),
            Err(AdapterError::InvalidHandle(id)) => Completion::Refused(*id),
            Err(error) => return Err(error.clone()),
        };
        if event.completion != completion {
            return Err(AdapterError::ReplayMismatch(self.position));
        }
        self.state = state;
        self.tree = tree;
        self.roots = roots;
        self.identities = identities;
        self.detached_roots = detached;
        self.mount_pins = mount_pins;
        self.position += 1;
        result
    }
}
/// Replay reconstructs local inode numbers from initial entries and ordered
/// publication, independently of logical handles and host inode values.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct FileIdentities {
    serial: u64,
    reachable: std::collections::BTreeMap<String, u64>,
    opened: std::collections::BTreeMap<u64, u64>,
    prepared: std::collections::BTreeMap<String, u64>,
    contents: std::collections::BTreeMap<u64, Vec<u8>>,
}
impl FileIdentities {
    pub(crate) fn new(tree: &Tree) -> Self {
        let mut files = Self::default();
        for path in tree.regular_paths() {
            files.publish(path);
            files.contents.insert(
                files.serial,
                tree.files.get(path).cloned().unwrap_or_default(),
            );
        }
        for (alias, canonical) in &tree.file_aliases {
            if let Some(identity) = files.reachable.get(canonical).copied() {
                files.reachable.insert(alias.clone(), identity);
            }
        }
        files
    }
    fn publish(&mut self, path: &str) {
        self.serial = self
            .serial
            .checked_add(1)
            .expect("file identities exhausted");
        self.reachable.insert(path.into(), self.serial);
    }
    pub(crate) fn close(&mut self, id: u64) {
        self.opened.remove(&id);
    }
}
pub(crate) struct MemoryTree<'a> {
    pub(crate) tree: &'a mut Tree,
    pub(crate) identities: &'a mut FileIdentities,
}
impl MemoryTree<'_> {
    fn sync_aliases(&mut self) {
        self.tree.file_aliases.clear();
        let mut canonical = std::collections::BTreeMap::<u64, String>::new();
        for (path, identity) in &self.identities.reachable {
            if let Some(first) = canonical.get(identity) {
                self.tree.file_aliases.insert(path.clone(), first.clone());
            } else {
                canonical.insert(*identity, path.clone());
            }
        }
    }
    fn sync_contents(&mut self, identity: u64) {
        for (path, _) in self
            .identities
            .reachable
            .iter()
            .filter(|(_, id)| **id == identity)
        {
            self.tree.metadata.remove(path);
            self.tree
                .files
                .insert(path.clone(), self.identities.contents[&identity].clone());
        }
    }
}
impl Storage for MemoryTree<'_> {
    fn metadata(&mut self, path: &str) -> Result<Metadata, IoError> {
        self.tree.resolve("", path, false)?;
        if let Some(meta) = self.tree.metadata.get(path) {
            return Ok(meta.clone());
        }
        let (kind, size) = if let Some(bytes) = self.tree.files.get(path) {
            (FileKind::Regular, bytes.len() as u64)
        } else if self.tree.directories.contains(path) {
            (FileKind::Directory, 0)
        } else if let Some(target) = self.tree.links.get(path) {
            (FileKind::Symlink, target.len() as u64)
        } else {
            return Err(IoError::NotFound);
        };
        Ok(Metadata {
            kind,
            size,
            modified_unix_ns: None,
        })
    }
    fn read_dir(&mut self, path: &str) -> Result<Vec<DirEntry>, IoError> {
        self.directory(path, 0)?;
        let children: std::collections::BTreeSet<_> = self
            .tree
            .directories
            .iter()
            .chain(self.tree.files.keys())
            .chain(self.tree.links.keys())
            .chain(self.tree.metadata.keys())
            .filter(|p| !p.is_empty() && p.rsplit_once('/').map_or("", |(p, _)| p) == path)
            .cloned()
            .collect();
        children
            .into_iter()
            .map(|path| {
                let kind = self.metadata(&path)?.kind;
                Ok(DirEntry {
                    path: path
                        .rsplit_once('/')
                        .map_or(path.as_str(), |(_, n)| n)
                        .into(),
                    kind,
                })
            })
            .collect()
    }
    fn mkdir(&mut self, path: &str) -> Result<(), IoError> {
        if self.metadata(path).is_ok() {
            return Err(IoError::AlreadyExists);
        }
        let parent = path.rsplit_once('/').map_or("", |(p, _)| p);
        self.directory(parent, 0)?;
        self.tree.directories.insert(path.into());
        self.tree.metadata.remove(parent);
        Ok(())
    }
    fn remove_dir(&mut self, path: &str) -> Result<(), IoError> {
        if self.metadata(path)?.kind != FileKind::Directory {
            return Err(IoError::NotDirectory);
        }
        if !self.read_dir(path)?.is_empty() {
            return Err(IoError::DirectoryNotEmpty);
        }
        self.tree.directories.remove(path);
        self.tree.metadata.remove(path);
        self.tree
            .metadata
            .remove(path.rsplit_once('/').map_or("", |(p, _)| p));
        Ok(())
    }
    fn move_entry(&mut self, from: &str, to: &str) -> Result<(), IoError> {
        let source = self.metadata(from)?.kind;
        if source == FileKind::Regular
            && self.identities.reachable.contains_key(from)
            && self.identities.reachable.get(from) == self.identities.reachable.get(to)
        {
            return Ok(());
        }
        let parent = to.rsplit_once('/').map_or("", |(p, _)| p);
        self.directory(parent, 0)?;
        if from == to {
            return Ok(());
        }
        if source == FileKind::Directory && to.starts_with(&format!("{from}/")) {
            return Err(IoError::Fault(1));
        }
        match self.metadata(to) {
            Ok(meta) => {
                if source == FileKind::Directory && meta.kind != FileKind::Directory {
                    return Err(IoError::NotDirectory);
                }
                if source != FileKind::Directory && meta.kind == FileKind::Directory {
                    return Err(IoError::IsDirectory);
                }
                if meta.kind == FileKind::Directory && !self.read_dir(to)?.is_empty() {
                    return Err(IoError::DirectoryNotEmpty);
                }
            }
            Err(IoError::NotFound) => {}
            Err(e) => return Err(e),
        }
        self.tree.files.remove(to);
        self.tree.links.remove(to);
        self.tree.directories.remove(to);
        self.tree.metadata.remove(to);
        self.identities.reachable.remove(to);
        self.identities.prepared.remove(to);
        let moved = |p: &str| p == from || p.starts_with(&format!("{from}/"));
        let destination = |p: &str| format!("{to}{}", &p[from.len()..]);
        let dirs: Vec<_> = self
            .tree
            .directories
            .iter()
            .filter(|p| moved(p))
            .cloned()
            .collect();
        for path in dirs {
            self.tree.directories.remove(&path);
            self.tree.directories.insert(destination(&path));
        }
        macro_rules! relocate {
            ($map:expr) => {{
                let paths: Vec<_> = $map.keys().filter(|p| moved(p)).cloned().collect();
                for path in paths {
                    let value = $map.remove(&path).unwrap();
                    $map.insert(destination(&path), value);
                }
            }};
        }
        relocate!(self.tree.files);
        relocate!(self.tree.links);
        relocate!(self.tree.metadata);
        relocate!(self.identities.reachable);
        relocate!(self.identities.prepared);
        self.tree.metadata.remove(parent);
        self.tree
            .metadata
            .remove(from.rsplit_once('/').map_or("", |(p, _)| p));
        Ok(())
    }

    fn open_file(&mut self, path: &str, id: u64) -> Result<(), IoError> {
        self.destination(path)?;
        let identity = self
            .identities
            .reachable
            .get(path)
            .copied()
            .ok_or(IoError::NotFound)?;
        self.identities.opened.insert(id, identity);
        Ok(())
    }
    fn create_file(&mut self, path: &str, id: u64) -> Result<(), IoError> {
        self.destination(path)?;
        let identity = if let Some(identity) = self.identities.reachable.get(path) {
            *identity
        } else {
            self.identities.publish(path);
            self.identities.serial
        };
        self.identities.contents.insert(identity, vec![]);
        self.tree.metadata.remove(path);
        self.tree.files.insert(path.into(), vec![]);
        self.identities.opened.insert(id, identity);
        self.sync_contents(identity);
        self.sync_aliases();
        Ok(())
    }
    fn read_file(&mut self, id: u64, offset: u64, max: u64) -> Result<Vec<u8>, IoError> {
        let identity = self.identities.opened.get(&id).ok_or(IoError::Fault(1))?;
        let bytes = &self.identities.contents[identity];
        let start = offset.min(bytes.len() as u64) as usize;
        let n = max.min(65536).min((bytes.len() - start) as u64) as usize;
        Ok(bytes[start..start + n].to_vec())
    }
    fn write_file(&mut self, id: u64, bytes: &[u8], _all: bool) -> Result<u64, IoError> {
        let identity = *self.identities.opened.get(&id).ok_or(IoError::Fault(1))?;
        self.identities
            .contents
            .get_mut(&identity)
            .unwrap()
            .extend_from_slice(bytes);
        self.sync_contents(identity);
        Ok(bytes.len() as u64)
    }
    fn file_fits(&mut self, id: u64, path: &str) -> bool {
        self.identities
            .opened
            .get(&id)
            .is_some_and(|identity| self.identities.reachable.get(path) == Some(identity))
    }
    fn transfer_file(&mut self, from: u64, to: Option<u64>) {
        if let Some(identity) = self.identities.opened.remove(&from)
            && let Some(to) = to
        {
            self.identities.opened.insert(to, identity);
        }
    }
    fn destination(&mut self, path: &str) -> Result<(), IoError> {
        let parent = path.rsplit_once('/').map_or("", |(p, _)| p);
        self.tree.resolve("", path, false)?;
        if self.tree.directories.contains(path) {
            Err(IoError::IsDirectory)
        } else if !self.tree.directories.contains(parent) {
            Err(if self.tree.files.contains_key(parent) {
                IoError::NotDirectory
            } else {
                IoError::NotFound
            })
        } else {
            Ok(())
        }
    }
    fn directory(&mut self, path: &str, _id: u64) -> Result<(), IoError> {
        if self.tree.directories.contains(path) {
            Ok(())
        } else {
            self.tree.resolve("", path, true)?;
            Err(if self.tree.files.contains_key(path) {
                IoError::NotDirectory
            } else {
                IoError::NotFound
            })
        }
    }
    fn read(&mut self, path: &str) -> Result<Vec<u8>, IoError> {
        self.destination(path)?;
        self.tree.files.get(path).cloned().ok_or(IoError::NotFound)
    }
    fn write(
        &mut self,
        path: &str,
        bytes: &[u8],
        append: bool,
        exclusive: bool,
    ) -> Result<(), IoError> {
        if exclusive && (self.tree.files.contains_key(path) || self.tree.directories.contains(path))
        {
            return Err(IoError::AlreadyExists);
        }
        self.destination(path)?;
        if append {
            self.tree
                .files
                .get_mut(path)
                .ok_or(IoError::NotFound)?
                .extend_from_slice(bytes);
        } else {
            self.tree.files.insert(path.into(), bytes.to_vec());
            self.identities.publish(path);
            if exclusive {
                self.identities
                    .prepared
                    .insert(path.into(), self.identities.serial);
            }
        }
        self.tree.metadata.remove(path);
        let identity = self.identities.reachable[path];
        self.identities
            .contents
            .insert(identity, self.tree.files[path].clone());
        self.sync_aliases();
        Ok(())
    }
    fn remove(&mut self, path: &str) -> Result<(), IoError> {
        self.destination(path)?;
        if self.tree.files.remove(path).is_none() && !self.tree.metadata.contains_key(path) {
            return Err(IoError::NotFound);
        }
        self.tree.metadata.remove(path);
        self.identities.reachable.remove(path);
        self.identities.prepared.remove(path);
        self.sync_aliases();
        Ok(())
    }
    fn rename(&mut self, from: &str, to: &str) -> Result<(), IoError> {
        if self.tree.directories.contains(to) {
            return Err(IoError::IsDirectory);
        }
        if self.identities.prepared.get(from).is_none()
            || self.identities.prepared.get(from) != self.identities.reachable.get(from)
        {
            return Err(IoError::Fault(1));
        }
        let bytes = self.tree.files.remove(from).ok_or(IoError::NotFound)?;
        self.tree.files.insert(to.into(), bytes);
        self.tree.metadata.remove(to);
        if let Some(meta) = self.tree.metadata.remove(from) {
            self.tree.metadata.insert(to.into(), meta);
        }
        let identity = self
            .identities
            .reachable
            .remove(from)
            .expect("published identity");
        self.identities.reachable.insert(to.into(), identity);
        self.identities.prepared.remove(from);
        self.sync_aliases();
        Ok(())
    }
    fn walk(&mut self, root: &str) -> Result<Vec<String>, IoError> {
        let prefix = if root.is_empty() {
            String::new()
        } else {
            format!("{root}/")
        };
        Ok(self
            .tree
            .files
            .keys()
            .filter_map(|p| p.strip_prefix(&prefix).map(str::to_owned))
            .collect())
    }
}

/// A replay storage view keeps each issued directory's resolved root. Link
/// expansion is scoped to that pinned root rather than the launch tree.
#[derive(Clone, Debug, Default)]
struct MountPins {
    roots: std::collections::BTreeMap<String, String>,
    detached: std::collections::BTreeSet<String>,
}
impl MountPins {
    fn update(&mut self, tree: &mut Tree, from: &str, to: Option<&str>) {
        for (selector, root) in &mut self.roots {
            if self.detached.contains(selector) {
                continue;
            }
            if (to.is_none() && root == from) || to.is_some_and(|to| to != from && root == to) {
                self.detached.insert(selector.clone());
                tree.links.remove(selector);
                tree.directories.insert(selector.clone());
                *root = selector.clone();
            } else if let Some(to) = to
                && (*root == from || root.starts_with(&format!("{from}/")))
            {
                *root = format!("{to}{}", &root[from.len()..]);
            }
            if tree.links.contains_key(selector) {
                let parent = selector.rsplit_once('/').map_or("", |(p, _)| p);
                let prefix = "../".repeat(parent.split('/').filter(|p| !p.is_empty()).count());
                tree.links
                    .insert(selector.clone(), format!("{prefix}{root}"));
            }
        }
    }
}
pub(crate) struct LinkedTree<'a> {
    mount_pins: Option<&'a mut MountPins>,
    active_id: u64,
    detached: &'a mut std::collections::BTreeSet<u64>,
    metadata_input: Option<Metadata>,
    read_limit: Option<u64>,
    write_limit: Option<u64>,
    tree: &'a mut Tree,
    roots: &'a mut std::collections::BTreeMap<u64, String>,
    active: (String, String),
    identities: &'a mut FileIdentities,
}
impl<'a> LinkedTree<'a> {
    pub(crate) fn new(
        tree: &'a mut Tree,
        roots: &'a mut std::collections::BTreeMap<u64, String>,
        identities: &'a mut FileIdentities,
        detached: &'a mut std::collections::BTreeSet<u64>,
    ) -> Self {
        Self {
            mount_pins: None,
            active_id: 0,
            detached,
            tree,
            roots,
            identities,
            active: Default::default(),
            metadata_input: None,
            read_limit: None,
            write_limit: None,
        }
    }

    fn detached(&self) -> bool {
        self.detached.contains(&self.active_id)
            || (self.active_id == 0
                && self
                    .mount_pins
                    .as_ref()
                    .is_some_and(|p| p.detached.contains(&self.active.0)))
    }
    fn memory(&mut self) -> MemoryTree<'_> {
        MemoryTree {
            tree: self.tree,
            identities: self.identities,
        }
    }
    fn path(&self, path: &str, follow_leaf: bool) -> Result<String, IoError> {
        let local = if self.active.0.is_empty() {
            path
        } else if path == self.active.0 {
            ""
        } else {
            path.strip_prefix(&format!("{}/", self.active.0))
                .ok_or(IoError::Fault(1))?
        };
        if self.detached() {
            return if relative("", local).as_deref() == Some("") {
                Ok(self.active.1.clone())
            } else {
                Err(IoError::NotFound)
            };
        }
        self.tree.resolve(&self.active.1, local, follow_leaf)
    }
}
impl Storage for LinkedTree<'_> {
    fn metadata(&mut self, path: &str) -> Result<Metadata, IoError> {
        let path = self.path(path, false)?;
        let derived = if self.detached() {
            Metadata {
                kind: FileKind::Directory,
                size: 0,
                modified_unix_ns: None,
            }
        } else {
            self.memory().metadata(&path)?
        };
        match &self.metadata_input {
            Some(input)
                if input.kind == derived.kind
                    && (!matches!(input.kind, FileKind::Regular | FileKind::Symlink)
                        || input.size == derived.size) =>
            {
                Ok(input.clone())
            }
            Some(_) => Err(IoError::Fault(1)),
            None => Ok(derived),
        }
    }
    fn read_dir(&mut self, path: &str) -> Result<Vec<DirEntry>, IoError> {
        let physical = self.path(path, true)?;
        if self.detached() {
            return Ok(vec![]);
        }
        self.memory().read_dir(&physical)
    }
    fn mkdir(&mut self, path: &str) -> Result<(), IoError> {
        let path = self.path(path, false)?;
        self.memory().mkdir(&path)
    }
    fn remove_dir(&mut self, path: &str) -> Result<(), IoError> {
        let path = self.path(path, false)?;
        self.memory().remove_dir(&path)?;
        if let Some(pins) = &mut self.mount_pins {
            pins.update(self.tree, &path, None);
        }
        self.detached.extend(
            self.roots
                .iter()
                .filter_map(|(id, root)| (*root == path).then_some(*id)),
        );
        Ok(())
    }
    fn move_entry(&mut self, from: &str, to: &str) -> Result<(), IoError> {
        let from = self.path(from, false)?;
        let to = self.path(to, false)?;
        self.memory().move_entry(&from, &to)?;
        if let Some(pins) = &mut self.mount_pins {
            pins.update(self.tree, &from, Some(&to));
        }
        if from != to {
            self.detached.extend(
                self.roots
                    .iter()
                    .filter_map(|(id, root)| (*root == to).then_some(*id)),
            );
        }
        for (id, root) in self.roots.iter_mut() {
            if self.detached.contains(id) {
                continue;
            }
            if *root == from || root.starts_with(&format!("{from}/")) {
                *root = format!("{to}{}", &root[from.len()..]);
            }
        }
        Ok(())
    }

    fn open_file(&mut self, path: &str, id: u64) -> Result<(), IoError> {
        let path = self.path(path, true)?;
        if self.detached() {
            return Err(IoError::IsDirectory);
        }
        self.memory().open_file(&path, id)
    }
    fn create_file(&mut self, path: &str, id: u64) -> Result<(), IoError> {
        let path = self.path(path, true)?;
        if self.detached() {
            return Err(IoError::IsDirectory);
        }
        self.memory().create_file(&path, id)
    }
    fn read_file(&mut self, id: u64, offset: u64, max: u64) -> Result<Vec<u8>, IoError> {
        let max = self.read_limit.unwrap_or(max).min(max);
        self.memory().read_file(id, offset, max)
    }
    fn write_file(&mut self, id: u64, bytes: &[u8], all: bool) -> Result<u64, IoError> {
        let n = self
            .write_limit
            .unwrap_or(bytes.len() as u64)
            .min(bytes.len() as u64) as usize;
        self.memory().write_file(id, &bytes[..n], all)
    }
    fn file_fits(&mut self, id: u64, path: &str) -> bool {
        self.path(path, true)
            .ok()
            .is_some_and(|path| self.memory().file_fits(id, &path))
    }
    fn transfer_file(&mut self, from: u64, to: Option<u64>) {
        self.memory().transfer_file(from, to);
    }
    fn activate(&mut self, id: u64, root: &str) {
        self.active_id = id;
        self.active = (
            root.into(),
            self.roots
                .get(&id)
                .cloned()
                .or_else(|| {
                    (id == 0)
                        .then(|| self.mount_pins.as_ref()?.roots.get(root).cloned())
                        .flatten()
                })
                .unwrap_or_else(|| {
                    self.tree
                        .resolve("", root, true)
                        .unwrap_or_else(|_| root.into())
                }),
        );
    }
    fn directory(&mut self, path: &str, id: u64) -> Result<(), IoError> {
        let path = self.path(path, true)?;
        if self.detached() {
            self.detached.insert(id);
        } else {
            self.memory().directory(&path, id)?;
        }
        self.roots.insert(id, path);
        Ok(())
    }
    fn destination(&mut self, path: &str) -> Result<(), IoError> {
        let path = self.path(path, true)?;
        if self.detached() {
            return Err(IoError::IsDirectory);
        }
        self.memory().destination(&path)
    }
    fn read(&mut self, path: &str) -> Result<Vec<u8>, IoError> {
        let path = self.path(path, true)?;
        if self.detached() {
            return Err(IoError::IsDirectory);
        }
        self.memory().read(&path)
    }
    fn write(
        &mut self,
        path: &str,
        bytes: &[u8],
        append: bool,
        exclusive: bool,
    ) -> Result<(), IoError> {
        let path = self.path(path, append)?;
        if self.detached() {
            return Err(IoError::IsDirectory);
        }
        if exclusive && self.tree.links.contains_key(&path) {
            return Err(IoError::AlreadyExists);
        }
        self.memory().write(&path, bytes, append, exclusive)?;
        if !append {
            self.tree.links.remove(&path);
        }
        Ok(())
    }
    fn remove(&mut self, path: &str) -> Result<(), IoError> {
        let path = self.path(path, false)?;
        if self.detached() {
            return Err(IoError::IsDirectory);
        }
        if self.tree.links.remove(&path).is_some() {
            self.tree.metadata.remove(&path);
            Ok(())
        } else {
            self.memory().remove(&path)
        }
    }
    fn rename(&mut self, from: &str, to: &str) -> Result<(), IoError> {
        let from = self.path(from, false)?;
        let to = self.path(to, false)?;
        self.memory().rename(&from, &to)?;
        self.tree.links.remove(&to);
        Ok(())
    }
    fn walk(&mut self, root: &str) -> Result<Vec<String>, IoError> {
        let root = self.path(root, true)?;
        if self.detached() {
            return Ok(vec![]);
        }
        let mut pending = vec![(root.clone(), String::new(), vec![root.clone()])];
        let mut files = vec![];
        while let Some((directory, prefix, visited)) = pending.pop() {
            let children: std::collections::BTreeSet<_> = self
                .tree
                .directories
                .iter()
                .chain(self.tree.files.keys())
                .chain(self.tree.links.keys())
                .filter_map(|p| {
                    let (parent, name) = p.rsplit_once('/').unwrap_or(("", p));
                    (parent == directory && !name.is_empty()).then(|| name.to_owned())
                })
                .collect();
            for name in children {
                // The traversal's physical ancestry remains distinct from its
                // logical alias path, as it does in the confined host walker.
                let relative = if prefix.is_empty() {
                    name.clone()
                } else {
                    format!("{prefix}/{name}")
                };
                let path = self.tree.resolve(&root, &relative, true)?;
                if self.tree.directories.contains(&path) {
                    if visited.contains(&path) {
                        return Err(IoError::Fault(1));
                    }
                    let mut visited = visited.clone();
                    visited.push(path.clone());
                    pending.push((path, relative, visited));
                } else if self.tree.files.contains_key(&path) {
                    files.push(relative);
                } else {
                    return Err(IoError::Fault(1));
                }
            }
        }
        files.sort();
        Ok(files)
    }
}

/// Host network replay: the recorded completion is the provider's only input. No socket,
/// resolver or TLS runs; policy was already re-derived, and the completion it builds must
/// equal the recorded one (handles included) or replay refuses.
struct Recorded<'a>(&'a Completion);
impl Recorded<'_> {
    fn unit(&self) -> crate::net::Provided<()> {
        match self.0 {
            Completion::NetUnit(value) => Ok(*value),
            Completion::NetHandle(value) => Ok(value.map(|_| ())),
            Completion::Listened(value) => Ok(value.map(|_| ())),
            _ => Err(AdapterError::InvalidCompletion),
        }
    }
    fn address(&self) -> crate::net::Provided<SocketAddr> {
        match self.0 {
            Completion::Address(value) => Ok(*value),
            Completion::Listened(value) => Ok(value.map(|(_, local)| local)),
            Completion::Accepted(value) => Ok(value.map(|(_, peer)| peer)),
            _ => Err(AdapterError::InvalidCompletion),
        }
    }
}
impl NetProvider for Recorded<'_> {
    fn resolve(&mut self, _: &str, _: u16) -> crate::net::Provided<Vec<SocketAddr>> {
        match self.0 {
            Completion::Addresses(value) => Ok(value.clone()),
            _ => Err(AdapterError::InvalidCompletion),
        }
    }
    fn connect(&mut self, _: u64, _: &str, _: u16, _: NetTimeout) -> crate::net::Provided<()> {
        self.unit()
    }
    fn connect_addr(&mut self, _: u64, _: SocketAddr, _: NetTimeout) -> crate::net::Provided<()> {
        self.unit()
    }
    fn read(&mut self, _: u64, _: u64) -> crate::net::Provided<Vec<u8>> {
        match self.0 {
            Completion::NetBytes(value) => Ok(value.clone()),
            _ => Err(AdapterError::InvalidCompletion),
        }
    }
    fn write(&mut self, _: u64, bytes: &[u8]) -> crate::net::Provided<u64> {
        match self.0 {
            Completion::Count(Ok(n))
                if *n > bytes.len() as u64 || (*n == 0 && !bytes.is_empty()) =>
            {
                Err(AdapterError::InvalidCompletion)
            }
            Completion::Count(value) => Ok(*value),
            _ => Err(AdapterError::InvalidCompletion),
        }
    }
    fn write_all(&mut self, _: u64, _: &[u8]) -> crate::net::Provided<()> {
        self.unit()
    }
    fn shutdown(&mut self, _: u64, _: ShutdownHow) -> crate::net::Provided<()> {
        self.unit()
    }
    fn set_timeout(&mut self, _: u64, _: TimeoutKind, _: NetTimeout) -> crate::net::Provided<()> {
        self.unit()
    }
    fn peer_addr(&mut self, _: u64) -> crate::net::Provided<SocketAddr> {
        self.address()
    }
    fn tls_connect(&mut self, _: u64, _: &str, _: &RootsDigest) -> crate::net::Provided<()> {
        self.unit()
    }
    fn tls_close(&mut self, _: u64) -> crate::net::Provided<()> {
        self.unit()
    }
    fn listen(&mut self, _: u64, _: SocketAddr) -> crate::net::Provided<()> {
        self.unit()
    }
    fn accept(&mut self, _: u64, _: u64) -> crate::net::Provided<SocketAddr> {
        self.address()
    }
    fn local_addr(&mut self, _: u64) -> crate::net::Provided<SocketAddr> {
        self.address()
    }
    fn close(&mut self, _: u64) {}
}

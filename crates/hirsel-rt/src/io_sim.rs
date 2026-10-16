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
}
impl SimIo {
    /// Configure the root Net's isolated scripted world before entry. Rules are
    /// supplied separately by set_net_rules; peer data never widens them.
    pub fn set_net_script(
        &mut self,
        peers: Vec<hirsel_run_config::NetPeer>,
    ) -> Result<(), AdapterError> {
        self.state.set_net_script(peers)
    }
    /// Replay uses the admitted projection set, independently of tape completions.
    pub fn set_projections(&mut self, projections: std::collections::BTreeSet<String>) {
        self.state.projections = projections;
    }
    pub fn new(tree: Tree, tape: Vec<Event>) -> Self {
        Self {
            state: State::default(),
            identities: FileIdentities::new(&tree),
            tree,
            tape,
            position: 0,
            roots: Default::default(),
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
        self.state.mounts = mounts;
        Ok(())
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
}
impl Adapter for SimIo {
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
        let mut state = self.state.clone();
        let mut tree = self.tree.clone();
        let mut roots = self.roots.clone();
        let mut identities = self.identities.clone();
        let mut storage = LinkedTree {
            tree: &mut tree,
            roots: &mut roots,
            active: Default::default(),
            identities: &mut identities,
        };
        let result = if matches!(request, Request::Monotonic { .. } | Request::Wall { .. }) {
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
}
impl FileIdentities {
    pub(crate) fn new(tree: &Tree) -> Self {
        let mut files = Self::default();
        for path in tree.files.keys() {
            files.publish(path);
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
impl Storage for MemoryTree<'_> {
    fn open_file(&mut self, path: &str, id: u64) -> Result<(), IoError> {
        let identity = self
            .identities
            .reachable
            .get(path)
            .copied()
            .ok_or(IoError::Fault(2))?;
        self.identities.opened.insert(id, identity);
        Ok(())
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
        if self.tree.directories.contains(path) || !self.tree.directories.contains(parent) {
            Err(IoError::Fault(2))
        } else {
            Ok(())
        }
    }
    fn directory(&mut self, path: &str, _id: u64) -> Result<(), IoError> {
        if self.tree.directories.contains(path) {
            Ok(())
        } else {
            Err(IoError::Fault(1))
        }
    }
    fn read(&mut self, path: &str) -> Result<Vec<u8>, IoError> {
        self.tree.files.get(path).cloned().ok_or(IoError::Fault(2))
    }
    fn write(
        &mut self,
        path: &str,
        bytes: &[u8],
        append: bool,
        exclusive: bool,
    ) -> Result<(), IoError> {
        let parent = path.rsplit_once('/').map_or("", |(p, _)| p);
        if !self.tree.directories.contains(parent) || self.tree.directories.contains(path) {
            return Err(IoError::Fault(2));
        }
        if exclusive && self.tree.files.contains_key(path) {
            return Err(IoError::Fault(3));
        }
        if append {
            self.tree
                .files
                .get_mut(path)
                .ok_or(IoError::Fault(2))?
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
        Ok(())
    }
    fn remove(&mut self, path: &str) -> Result<(), IoError> {
        self.tree.files.remove(path).ok_or(IoError::Fault(2))?;
        self.identities.reachable.remove(path);
        self.identities.prepared.remove(path);
        Ok(())
    }
    fn rename(&mut self, from: &str, to: &str) -> Result<(), IoError> {
        if self.tree.directories.contains(to) {
            return Err(IoError::Fault(2));
        }
        if self.identities.prepared.get(from).is_none()
            || self.identities.prepared.get(from) != self.identities.reachable.get(from)
        {
            return Err(IoError::Fault(1));
        }
        let bytes = self.tree.files.remove(from).ok_or(IoError::Fault(2))?;
        self.tree.files.insert(to.into(), bytes);
        let identity = self
            .identities
            .reachable
            .remove(from)
            .expect("published identity");
        self.identities.reachable.insert(to.into(), identity);
        self.identities.prepared.remove(from);
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
struct LinkedTree<'a> {
    tree: &'a mut Tree,
    roots: &'a mut std::collections::BTreeMap<u64, String>,
    active: (String, String),
    identities: &'a mut FileIdentities,
}
impl LinkedTree<'_> {
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
        self.tree.resolve(&self.active.1, local, follow_leaf)
    }
}
impl Storage for LinkedTree<'_> {
    fn open_file(&mut self, path: &str, id: u64) -> Result<(), IoError> {
        let path = self.path(path, true)?;
        self.memory().open_file(&path, id)
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
        self.active = (
            root.into(),
            self.roots.get(&id).cloned().unwrap_or_else(|| {
                self.tree
                    .resolve("", root, true)
                    .unwrap_or_else(|_| root.into())
            }),
        );
    }
    fn directory(&mut self, path: &str, id: u64) -> Result<(), IoError> {
        let path = self.path(path, true)?;
        self.memory().directory(&path, id)?;
        self.roots.insert(id, path);
        Ok(())
    }
    fn destination(&mut self, path: &str) -> Result<(), IoError> {
        let path = self.path(path, true)?;
        self.memory().destination(&path)
    }
    fn read(&mut self, path: &str) -> Result<Vec<u8>, IoError> {
        let path = self.path(path, true)?;
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
        if exclusive && self.tree.links.contains_key(&path) {
            return Err(IoError::Fault(3));
        }
        self.memory().write(&path, bytes, append, exclusive)?;
        if !append {
            self.tree.links.remove(&path);
        }
        Ok(())
    }
    fn remove(&mut self, path: &str) -> Result<(), IoError> {
        let path = self.path(path, false)?;
        if self.tree.links.remove(&path).is_some() {
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
            _ => Err(AdapterError::InvalidCompletion),
        }
    }
    fn address(&self) -> crate::net::Provided<SocketAddr> {
        match self.0 {
            Completion::Address(value) => Ok(*value),
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

//! Closed candidate inputs, separate from v1's observed-operation cache.
//!
//! Structure/vectors: TypeScript-Go 89d5d5b2849a0db0957065889ca58536fa6d2e4a,
//! internal/project/snapshotfs.go:20-30,56-117,160-180,199 and
//! snapshotfs_test.go:24-257,1113-1144,1377-1432. Here directory tables are
//! authoritative, empty directories survive, missing parents refuse, and
//! neither a lookup nor an overlay has a host fallback.
use super::{
    DirectoryEntry, InputKind, InputLimit, InputUnavailable, IoFailure, LoaderInputs, Operation,
};
use hirsel_syntax::ContentHash;
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Root {
    pub id: String,
    /// Absolute UTF-8, component-normal spelling, with no symlink ancestors.
    pub path: PathBuf,
    pub editable: bool,
}
#[derive(Clone, Copy, Debug)]
pub struct Limits {
    pub bytes: usize,
    pub nodes: usize,
    pub aliases: usize,
    pub depth: usize,
    pub link_hops: usize,
    pub edits: usize,
    pub attempts: usize,
}
#[derive(Clone, Debug)]
pub struct CapturePlan {
    pub roots: Vec<Root>,
    pub excluded: BTreeSet<PathBuf>,
    /// Exact failed operations outside closed roots; never creation authority.
    pub failed_probes: Vec<Operation>,
    pub entry: PathBuf,
    pub std_manifest: Option<PathBuf>,
    /// Caller-encoded explicit compiler/config/toolchain/policy identity.
    pub environment: Vec<u8>,
    pub limits: Limits,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Facts {
    pub kind: InputKind,
    pub device: u64,
    pub inode: u64,
    pub links: u64,
    pub len: u64,
}
/// Capture-only no-follow seam. A published lease retains no implementation of
/// this trait. Implementations must bound allocation and never open special files.
pub trait CaptureSource {
    fn facts(&self, path: &Path) -> io::Result<Facts>;
    fn read(&self, path: &Path, maximum: usize) -> io::Result<Arc<[u8]>>;
    fn directory(&self, path: &Path, maximum: usize) -> io::Result<Vec<DirectoryEntry>>;
    fn read_link(&self, path: &Path, maximum: usize) -> io::Result<PathBuf>;
    fn case_sensitive(&self, path: &Path) -> io::Result<bool>;
    fn failed_probe(&self, op: &Operation, maximum: usize) -> io::Result<()>;
}
pub struct DiskCapture;

#[derive(Clone, Debug, Eq, PartialEq)]
enum Node {
    File {
        bytes: Arc<[u8]>,
        editable: bool,
        facts: Facts,
    },
    Directory {
        children: BTreeMap<String, InputKind>,
        editable: bool,
        facts: Facts,
    },
    Symlink {
        target: PathBuf,
        facts: Facts,
    },
    Opaque {
        kind: InputKind,
        excluded: bool,
    },
    /// Explicit traversal edges, not complete directory coverage.
    Ancestor(BTreeSet<String>),
}
impl Node {
    fn kind(&self) -> InputKind {
        match self {
            Self::File { .. } => InputKind::File,
            Self::Directory { .. } | Self::Ancestor(_) => InputKind::Directory,
            Self::Symlink { .. } => InputKind::Symlink,
            Self::Opaque { kind, .. } => *kind,
        }
    }
}
#[derive(Clone, Debug, Eq, PartialEq)]
struct State {
    roots: BTreeMap<String, Root>,
    nodes: BTreeMap<PathBuf, Node>,
    failures: BTreeMap<Operation, IoFailure>,
    entry: PathBuf,
    std_manifest: Option<PathBuf>,
    environment: Vec<u8>,
}
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Usage {
    pub roots: usize,
    pub nodes: usize,
    pub directories: usize,
    pub directory_entries: usize,
    pub aliases: usize,
    pub file_bytes: usize,
    pub fixed_probes: usize,
    /// Conservative retained charge: full tables/bytes for each live snapshot,
    /// even when immutable byte Arcs are shared. Clones of one lease are free.
    pub charged_bytes: usize,
}
struct BudgetState {
    used: usize,
    leases: usize,
}
#[derive(Clone)]
pub struct LeaseBudget {
    state: Arc<Mutex<BudgetState>>,
    maximum: usize,
}
impl LeaseBudget {
    pub fn new(maximum: usize) -> Self {
        Self {
            state: Arc::new(Mutex::new(BudgetState { used: 0, leases: 0 })),
            maximum,
        }
    }
    pub fn retained(&self) -> (usize, usize) {
        let s = self.state.lock().expect("namespace budget poisoned");
        (s.used, s.leases)
    }
    fn reserve(&self, charge: usize) -> io::Result<Reservation> {
        let mut s = self.state.lock().expect("namespace budget poisoned");
        let used = s
            .used
            .checked_add(charge)
            .filter(|v| *v <= self.maximum)
            .ok_or_else(limit)?;
        s.used = used;
        s.leases += 1;
        Ok(Reservation {
            budget: self.clone(),
            charge,
        })
    }
}
struct Reservation {
    budget: LeaseBudget,
    charge: usize,
}
impl Drop for Reservation {
    fn drop(&mut self) {
        let mut s = self.budget.state.lock().expect("namespace budget poisoned");
        s.used -= self.charge;
        s.leases -= 1;
    }
}
struct Namespace {
    state: State,
    digest: ContentHash,
    inputs: ContentHash,
    usage: Usage,
    limits: Limits,
    reservation: Reservation,
}
/// Owned immutable snapshot. No parent chain, disk adapter, cwd or env lookup.
#[derive(Clone)]
pub struct NamespaceLease(Arc<Namespace>);
impl std::fmt::Debug for NamespaceLease {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NamespaceLease")
            .field("digest", &self.digest())
            .field("usage", &self.usage())
            .finish()
    }
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CaptureError {
    InvalidPlan(String),
    Io(PathBuf, io::ErrorKind, String),
    InputsChanged,
    InvalidSelection(String),
    Limit,
}
fn limit() -> io::Error {
    io::Error::other(InputLimit)
}
fn invalid(text: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, text)
}
fn symlink_loop_error() -> io::Error {
    #[cfg(unix)]
    {
        io::Error::from_raw_os_error(libc::ELOOP)
    }
    #[cfg(not(unix))]
    {
        invalid("captured symlink loop")
    }
}
fn unavailable(op: Operation) -> io::Error {
    io::Error::other(InputUnavailable(op))
}
fn capture_error(path: &Path, error: io::Error) -> CaptureError {
    if error.get_ref().is_some_and(|e| e.is::<InputLimit>()) {
        CaptureError::Limit
    } else {
        CaptureError::Io(path.to_owned(), error.kind(), error.to_string())
    }
}
fn field(out: &mut Vec<u8>, data: &[u8]) {
    out.extend_from_slice(&(data.len() as u64).to_le_bytes());
    out.extend_from_slice(data);
}
fn path_bytes(path: &Path) -> &[u8] {
    path.as_os_str().as_encoded_bytes()
}
fn strict_absolute(path: &Path) -> bool {
    path.to_str().is_some_and(|s| {
        s.starts_with('/')
            && !s.contains(['\\', '\0'])
            && (s == "/"
                || s[1..]
                    .split('/')
                    .all(|v| !v.is_empty() && v != "." && v != ".."))
    })
}
fn edit_path(root: &Root, path: &str) -> io::Result<PathBuf> {
    if path.is_empty()
        || path.contains(['\\', '\0'])
        || path
            .split('/')
            .any(|s| s.is_empty() || s == "." || s == "..")
    {
        return Err(invalid(
            "edit path must have nonempty normal UTF-8 components",
        ));
    }
    Ok(root.path.join(path))
}
impl CapturePlan {
    fn validate(&self) -> Result<(), CaptureError> {
        let bad = |s: &str| CaptureError::InvalidPlan(s.into());
        if self.roots.is_empty()
            || self.roots.len() > self.limits.nodes
            || self.limits.attempts == 0
            || self.limits.attempts > 8
            || self.limits.link_hops > 256
        {
            return Err(bad("empty/oversized plan or unsupported attempt/hop limit"));
        }
        if self.environment.len() > self.limits.bytes {
            return Err(CaptureError::Limit);
        }
        let plan_bytes = self
            .roots
            .iter()
            .try_fold(self.environment.len(), |n, r| {
                n.checked_add(r.id.len())?
                    .checked_add(path_bytes(&r.path).len())
            })
            .and_then(|n| {
                self.failed_probes
                    .iter()
                    .try_fold(n, |n, op| n.checked_add(op.charge()))
            })
            .and_then(|n| {
                self.excluded
                    .iter()
                    .try_fold(n, |n, p| n.checked_add(path_bytes(p).len()))
            });
        if self.excluded.len() > self.limits.nodes
            || plan_bytes.is_none_or(|n| n > self.limits.bytes)
        {
            return Err(CaptureError::Limit);
        }
        let mut ids = BTreeSet::new();
        for (i, root) in self.roots.iter().enumerate() {
            if root.id.is_empty() || !ids.insert(&root.id) || !strict_absolute(&root.path) {
                return Err(bad("invalid root identity"));
            }
            if self.roots[..i]
                .iter()
                .any(|r| root.path.starts_with(&r.path) || r.path.starts_with(&root.path))
            {
                return Err(bad("overlapping roots"));
            }
        }
        if !strict_absolute(&self.entry)
            || !self.roots.iter().any(|r| self.entry.starts_with(&r.path))
        {
            return Err(bad("entry outside explicit roots"));
        }
        for path in &self.excluded {
            if !strict_absolute(path)
                || !self
                    .roots
                    .iter()
                    .any(|r| path != &r.path && path.starts_with(&r.path))
            {
                return Err(bad("exclusion outside roots or excluding root"));
            }
        }
        if self.failed_probes.len() > self.limits.nodes
            || self.failed_probes.iter().any(|op| {
                !strict_absolute(op.path())
                    || self.roots.iter().any(|r| op.path().starts_with(&r.path))
            })
        {
            return Err(bad("fixed probes must be outside closed roots"));
        }
        if self
            .std_manifest
            .as_ref()
            .is_some_and(|p| !strict_absolute(p))
        {
            return Err(bad("invalid std manifest"));
        }
        Ok(())
    }
}
impl NamespaceLease {
    pub fn capture(
        plan: &CapturePlan,
        source: &dyn CaptureSource,
        budget: &LeaseBudget,
    ) -> Result<Self, CaptureError> {
        plan.validate()?;
        for _ in 0..plan.limits.attempts {
            let first = match scan(plan, source) {
                Ok(s) => s,
                Err(CaptureError::InputsChanged) => continue,
                Err(e) => return Err(e),
            };
            let second = match scan(plan, source) {
                Ok(s) => s,
                Err(CaptureError::InputsChanged) => continue,
                Err(e) => return Err(e),
            };
            if first == second {
                return Self::publish(second, plan.limits, budget)
                    .map_err(|e| capture_error(&plan.entry, e));
            }
        }
        Err(CaptureError::InputsChanged)
    }
    /// Program-base admission: validate the complete selected closure through
    /// the published adapter before returning a lease. Source syntax remains
    /// repairable; malformed manifests and unavailable selection do not.
    pub fn capture_program(
        plan: &CapturePlan,
        source: &dyn CaptureSource,
        budget: &LeaseBudget,
        compiler: &crate::Compiler,
        config: crate::CompileConfig,
    ) -> Result<Self, CaptureError> {
        let lease = Self::capture(plan, source, budget)?;
        lease
            .load(compiler, config, false)
            .map_err(|e| CaptureError::InvalidSelection(e.to_string()))?;
        Ok(lease)
    }
    fn publish(state: State, limits: Limits, budget: &LeaseBudget) -> io::Result<Self> {
        let usage = usage(&state)?;
        if usage.charged_bytes > limits.bytes
            || usage.nodes > limits.nodes
            || usage.aliases > limits.aliases
        {
            return Err(limit());
        }
        let reservation = budget.reserve(usage.charged_bytes)?;
        let digest = namespace_digest(&state);
        let input_parts = [
            digest.0.to_vec(),
            path_bytes(&state.entry).to_vec(),
            state.environment.clone(),
            2u64.to_le_bytes().to_vec(),
        ];
        let inputs = ContentHash::of_parts(
            "hirsel.candidate.inputs.v3",
            input_parts.iter().map(Vec::as_slice),
        );
        Ok(Self(Arc::new(Namespace {
            state,
            digest,
            inputs,
            usage,
            limits,
            reservation,
        })))
    }
    pub fn digest(&self) -> ContentHash {
        self.0.digest
    }
    pub fn inputs_digest(&self) -> ContentHash {
        self.0.inputs
    }
    pub fn usage(&self) -> Usage {
        self.0.usage
    }
    pub fn entry(&self) -> &Path {
        &self.0.state.entry
    }
    pub fn same_namespace(&self, other: &Self) -> bool {
        self.0.digest == other.0.digest && self.0.state == other.0.state
    }

    /// Pure graph load with explicit policy, no lazy result store/environment.
    /// The v2 owner still owns whole-closure checking and result/adoption logic.
    pub fn load(
        &self,
        compiler: &crate::Compiler,
        config: crate::CompileConfig,
        parse_sources: bool,
    ) -> Result<crate::LoadedProgram, crate::ToolError> {
        compiler
            .clone()
            .with_owned_engines()
            .with_config(crate::CompileConfig {
                store: Arc::new(crate::store::NoneStore),
                verify: crate::Verify::On,
                lowering: crate::LoweringPolicy::Comptime,
                ..config
            })
            .load_program_inputs(self.entry(), self, parse_sources)
    }
}

fn scan(plan: &CapturePlan, source: &dyn CaptureSource) -> Result<State, CaptureError> {
    let mut state = State {
        roots: BTreeMap::new(),
        nodes: BTreeMap::new(),
        failures: BTreeMap::new(),
        entry: plan.entry.clone(),
        std_manifest: plan.std_manifest.clone(),
        environment: plan.environment.clone(),
    };
    // Charge explicit context before scanning anything. Per-operation bounds
    // also apply before allocation, including complete directory enumeration.
    let mut charge = usage(&state)
        .map_err(|e| capture_error(&plan.entry, e))?
        .charged_bytes;
    if charge > plan.limits.bytes {
        return Err(CaptureError::Limit);
    }
    for root in &plan.roots {
        if root.path.ancestors().count() > plan.limits.nodes {
            return Err(CaptureError::Limit);
        }
        state.roots.insert(root.id.clone(), root.clone());
        for ancestor in root.path.ancestors().skip(1) {
            let facts = source
                .facts(ancestor)
                .map_err(|e| capture_error(ancestor, e))?;
            if facts.kind != InputKind::Directory {
                return Err(CaptureError::InvalidPlan(
                    "symlink/non-directory root ancestor".into(),
                ));
            }
            if !source
                .case_sensitive(ancestor)
                .map_err(|e| capture_error(ancestor, e))?
            {
                return Err(CaptureError::InvalidPlan(
                    "case-insensitive ancestor identity unsupported".into(),
                ));
            }
            let child = root
                .path
                .strip_prefix(ancestor)
                .unwrap()
                .components()
                .next()
                .unwrap()
                .as_os_str()
                .to_str()
                .unwrap()
                .to_owned();
            match state
                .nodes
                .entry(ancestor.to_owned())
                .or_insert_with(|| Node::Ancestor(BTreeSet::new()))
            {
                Node::Ancestor(children) => {
                    children.insert(child);
                }
                _ => return Err(CaptureError::InvalidPlan("ambiguous root ancestry".into())),
            }
        }
        let mut pending = vec![(root.path.clone(), 0usize)];
        while let Some((path, depth)) = pending.pop() {
            if state.nodes.len() >= plan.limits.nodes || depth > plan.limits.depth {
                return Err(CaptureError::Limit);
            }
            let facts = source.facts(&path).map_err(|e| {
                if path != root.path
                    && matches!(
                        e.kind(),
                        io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
                    )
                {
                    CaptureError::InputsChanged
                } else {
                    capture_error(&path, e)
                }
            })?;
            if path == root.path
                && (!matches!(facts.kind, InputKind::File | InputKind::Directory)
                    || (root.editable && facts.kind != InputKind::Directory))
            {
                return Err(CaptureError::InvalidPlan(
                    "root must be a directory or read-only regular file".into(),
                ));
            }
            let available = plan.limits.bytes.saturating_sub(charge);
            let node = if plan.excluded.iter().any(|p| path.starts_with(p)) {
                Node::Opaque {
                    kind: facts.kind,
                    excluded: true,
                }
            } else {
                match facts.kind {
                    InputKind::File => Node::File {
                        bytes: source
                            .read(&path, available)
                            .map_err(|e| capture_error(&path, e))?,
                        editable: root.editable && facts.links == 1,
                        facts,
                    },
                    InputKind::Directory => {
                        if !source
                            .case_sensitive(&path)
                            .map_err(|e| capture_error(&path, e))?
                        {
                            return Err(CaptureError::InvalidPlan(
                                "case-insensitive identity unsupported".into(),
                            ));
                        }
                        let entries = source
                            .directory(&path, available)
                            .map_err(|e| capture_error(&path, e))?;
                        let mut children = BTreeMap::new();
                        for entry in entries {
                            let name = entry
                                .name
                                .to_str()
                                .filter(|n| {
                                    !n.is_empty()
                                        && !n.contains(['/', '\\', '\0'])
                                        && *n != "."
                                        && *n != ".."
                                })
                                .ok_or_else(|| {
                                    CaptureError::InvalidPlan(
                                        "non-UTF8/unsupported directory name".into(),
                                    )
                                })?
                                .to_owned();
                            if children.insert(name.clone(), entry.kind).is_some() {
                                return Err(CaptureError::InputsChanged);
                            }
                            if pending
                                .len()
                                .saturating_add(state.nodes.len())
                                .saturating_add(children.len())
                                > plan.limits.nodes
                            {
                                return Err(CaptureError::Limit);
                            }
                            pending.push((path.join(&name), depth + 1));
                        }
                        Node::Directory {
                            children,
                            editable: root.editable,
                            facts,
                        }
                    }
                    InputKind::Symlink => Node::Symlink {
                        target: source
                            .read_link(&path, available)
                            .map_err(|e| capture_error(&path, e))?,
                        facts,
                    },
                    InputKind::Other => Node::Opaque {
                        kind: InputKind::Other,
                        excluded: false,
                    },
                }
            };
            if let Node::Symlink { target, .. } = &node {
                if target.to_str().is_none_or(|p| p.contains(['\\', '\0'])) {
                    return Err(CaptureError::InvalidPlan(
                        "non-UTF8/unsupported link text".into(),
                    ));
                }
            }
            charge = charge
                .checked_add(node_charge(&path, &node).map_err(|e| capture_error(&path, e))?)
                .ok_or(CaptureError::Limit)?;
            if charge > plan.limits.bytes {
                return Err(CaptureError::Limit);
            }
            state.nodes.insert(path, node);
        }
    }
    for op in &plan.failed_probes {
        match source.failed_probe(op, plan.limits.bytes.saturating_sub(charge)) {
            Ok(()) => {
                return Err(CaptureError::InvalidPlan(
                    "fixed probe was successful".into(),
                ));
            }
            Err(e) if e.get_ref().is_some_and(|e| e.is::<InputLimit>()) => {
                return Err(CaptureError::Limit);
            }
            Err(e) => {
                let failure = IoFailure::capture(e);
                charge = charge
                    .checked_add(op.charge())
                    .and_then(|n| n.checked_add(failure.text.len()))
                    .and_then(|n| n.checked_add(128))
                    .ok_or(CaptureError::Limit)?;
                if charge > plan.limits.bytes {
                    return Err(CaptureError::Limit);
                }
                state.failures.insert(op.clone(), failure);
            }
        }
    }
    let usage = usage(&state).map_err(|e| capture_error(&plan.entry, e))?;
    if usage.charged_bytes > plan.limits.bytes
        || usage.nodes > plan.limits.nodes
        || usage.aliases > plan.limits.aliases
    {
        return Err(CaptureError::Limit);
    }
    // Cross-check tables and no-follow child kinds; a race must not publish a
    // directory entry claiming a different kind from the captured node.
    for (path, node) in &state.nodes {
        if let Node::Directory { children, .. } = node {
            for (name, kind) in children {
                if state
                    .nodes
                    .get(&path.join(name))
                    .is_none_or(|n| n.kind() != *kind)
                {
                    return Err(CaptureError::InputsChanged);
                }
            }
        }
    }
    Ok(state)
}

fn node_encoding(node: &Node) -> Vec<u8> {
    let mut out = vec![node.kind() as u8];
    match node {
        Node::File {
            bytes,
            editable,
            facts,
        } => {
            out.push(u8::from(*editable));
            field(&mut out, &ContentHash::of_bytes(bytes).0);
            encode_facts(&mut out, facts);
        }
        Node::Directory {
            children,
            editable,
            facts,
        } => {
            out.push(u8::from(*editable));
            encode_facts(&mut out, facts);
            out.extend_from_slice(&(children.len() as u64).to_le_bytes());
            for (n, k) in children {
                field(&mut out, n.as_bytes());
                out.push(*k as u8);
            }
        }
        Node::Symlink { target, facts } => {
            field(&mut out, path_bytes(target));
            encode_facts(&mut out, facts);
        }
        Node::Opaque { excluded, .. } => {
            out.push(4);
            out.push(u8::from(*excluded));
        }
        Node::Ancestor(children) => {
            out.push(5);
            for name in children {
                field(&mut out, name.as_bytes());
            }
        }
    }
    out
}
fn encode_facts(out: &mut Vec<u8>, facts: &Facts) {
    // Directory size is an observation for race detection, not content identity.
    for v in [facts.device, facts.inode, facts.links] {
        out.extend_from_slice(&v.to_le_bytes());
    }
}
fn node_digest(node: &Node) -> ContentHash {
    ContentHash::of_parts("hirsel.candidate.node.v2", [node_encoding(node).as_slice()])
}
fn namespace_digest(state: &State) -> ContentHash {
    let mut parts = vec![b"utf8-sensitive-explicit-mounts-no-mkdir".to_vec()];
    for (id, root) in &state.roots {
        let mut p = vec![0];
        field(&mut p, id.as_bytes());
        field(&mut p, path_bytes(&root.path));
        p.push(u8::from(root.editable));
        parts.push(p);
    }
    for (path, node) in &state.nodes {
        let mut p = vec![1];
        field(&mut p, path_bytes(path));
        field(&mut p, &node_digest(node).0);
        parts.push(p);
    }
    for (op, error) in &state.failures {
        let mut p = vec![2];
        p.push(match op {
            Operation::Read(_) => 0,
            Operation::Canonicalize(_) => 1,
            Operation::Metadata(_) => 2,
            Operation::Directory(_) => 3,
        });
        field(&mut p, path_bytes(op.path()));
        field(&mut p, format!("{:?}", error.kind).as_bytes());
        p.push(u8::from(error.raw.is_some()));
        p.extend_from_slice(&error.raw.unwrap_or_default().to_le_bytes());
        field(&mut p, error.text.as_bytes());
        parts.push(p);
    }
    if let Some(std) = &state.std_manifest {
        let mut p = vec![3];
        field(&mut p, path_bytes(std));
        parts.push(p);
    }
    ContentHash::of_parts(
        "hirsel.candidate.namespace.v2",
        parts.iter().map(Vec::as_slice),
    )
}
fn node_charge(path: &Path, node: &Node) -> io::Result<usize> {
    let extra = match node {
        Node::File { bytes, .. } => bytes.len(),
        Node::Directory { children, .. } => children
            .keys()
            .try_fold(0usize, |n, s| n.checked_add(s.len())?.checked_add(96))
            .ok_or_else(limit)?,
        Node::Symlink { target, .. } => path_bytes(target).len(),
        Node::Ancestor(children) => children
            .iter()
            .try_fold(0usize, |n, s| n.checked_add(s.len())?.checked_add(96))
            .ok_or_else(limit)?,
        Node::Opaque { .. } => 0,
    };
    256usize
        .checked_add(path_bytes(path).len())
        .and_then(|v| v.checked_add(extra))
        .ok_or_else(limit)
}
fn usage(state: &State) -> io::Result<Usage> {
    let mut u = Usage {
        roots: state.roots.len(),
        nodes: state.nodes.len(),
        fixed_probes: state.failures.len(),
        ..Usage::default()
    };
    let mut add = |n: usize| -> io::Result<()> {
        u.charged_bytes = u.charged_bytes.checked_add(n).ok_or_else(limit)?;
        Ok(())
    };
    add(state.environment.len())?;
    add(path_bytes(&state.entry).len())?;
    if let Some(p) = &state.std_manifest {
        add(path_bytes(p).len())?;
    }
    for root in state.roots.values() {
        add(128)?;
        add(root.id.len())?;
        add(path_bytes(&root.path).len())?;
    }
    for (path, node) in &state.nodes {
        add(node_charge(path, node)?)?;
        match node {
            Node::File { bytes, .. } => {
                u.file_bytes = u.file_bytes.checked_add(bytes.len()).ok_or_else(limit)?
            }
            Node::Directory { children, .. } => {
                u.directories += 1;
                u.directory_entries += children.len();
            }
            Node::Symlink { .. } => u.aliases += 1,
            _ => {}
        }
    }
    for (op, error) in &state.failures {
        add(op.charge())?;
        add(error.text.len())?;
        add(128)?;
    }
    Ok(u)
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Key {
    pub root: String,
    pub path: String,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Inspection {
    Present {
        key: Key,
        kind: InputKind,
        editable: bool,
        present: ContentHash,
        bytes: Option<ContentHash>,
    },
    Absent {
        key: Key,
        parent: Key,
        absent: ContentHash,
    },
    MissingParent,
    DanglingSymlink,
}
enum Resolution {
    Present(PathBuf),
    Missing {
        path: PathBuf,
        parent: PathBuf,
        leaf: String,
        final_leaf: bool,
        from_link: bool,
    },
}
impl NamespaceLease {
    fn resolve(&self, path: &Path, op: &Operation) -> io::Result<Resolution> {
        if path_bytes(path).len() > self.0.limits.bytes {
            return Err(limit());
        }
        let text = path
            .to_str()
            .filter(|s| s.starts_with('/') && !s.contains(['\\', '\0']))
            .ok_or_else(|| unavailable(op.clone()))?;
        let mut todo: VecDeque<_> = text
            .split('/')
            .filter(|s| !s.is_empty())
            .map(|s| (s.to_owned(), false))
            .collect();
        // A trailing slash has directory semantics, including after a symlink.
        if text.ends_with('/') && text != "/" {
            todo.push_back((".".into(), false));
        }
        let mut current = PathBuf::from("/");
        let mut hops = 0usize;
        while let Some((component, from_link)) = todo.pop_front() {
            let node = self
                .0
                .state
                .nodes
                .get(&current)
                .ok_or_else(|| unavailable(op.clone()))?;
            let allowed = match node {
                Node::Directory { children, .. } => Some(children.contains_key(&component)),
                Node::Ancestor(children) => {
                    if component != "." && component != ".." && !children.contains(&component) {
                        return Err(unavailable(op.clone()));
                    }
                    None
                }
                Node::Opaque { .. } => return Err(unavailable(op.clone())),
                _ => return Err(io::Error::from(io::ErrorKind::NotADirectory)),
            };
            if component == "." {
                continue;
            }
            if component == ".." {
                current.pop();
                if current.as_os_str().is_empty() {
                    current.push("/");
                }
                continue;
            }
            let next = current.join(&component);
            if allowed == Some(false) {
                return Ok(Resolution::Missing {
                    path: next,
                    parent: current,
                    leaf: component,
                    final_leaf: todo.is_empty(),
                    from_link,
                });
            }
            match self
                .0
                .state
                .nodes
                .get(&next)
                .ok_or_else(|| unavailable(op.clone()))?
            {
                Node::Symlink { target, .. } => {
                    hops += 1;
                    if hops > self.0.limits.link_hops {
                        return Err(symlink_loop_error());
                    }
                    let target = target.to_str().ok_or_else(|| unavailable(op.clone()))?;
                    if target.is_empty() {
                        return Err(invalid("empty captured symlink"));
                    }
                    if target.starts_with('/') {
                        current = PathBuf::from("/");
                    }
                    let mut parts: Vec<_> = target
                        .split('/')
                        .filter(|s| !s.is_empty())
                        .map(|s| (s.to_owned(), true))
                        .collect();
                    if target.ends_with('/') {
                        parts.push((".".into(), true));
                    }
                    for part in parts.into_iter().rev() {
                        todo.push_front(part);
                    }
                }
                Node::Opaque { .. } => return Err(unavailable(op.clone())),
                _ => current = next,
            }
        }
        match self.0.state.nodes.get(&current) {
            Some(Node::Opaque { .. }) | None => Err(unavailable(op.clone())),
            _ => Ok(Resolution::Present(current)),
        }
    }
    fn existing(&self, path: &Path, op: &Operation) -> io::Result<PathBuf> {
        if let Some(failure) = self.0.state.failures.get(op) {
            return Err(failure.error());
        }
        match self.resolve(path, op)? {
            Resolution::Present(path) => Ok(path),
            Resolution::Missing { .. } => Err(io::Error::from(io::ErrorKind::NotFound)),
        }
    }
    fn key(&self, path: &Path) -> io::Result<Key> {
        self.0
            .state
            .roots
            .values()
            .find_map(|r| {
                path.strip_prefix(&r.path).ok().map(|p| Key {
                    root: r.id.clone(),
                    path: p.to_str().expect("captured UTF-8").into(),
                })
            })
            .ok_or_else(|| invalid("not a root-relative node"))
    }
    fn present(&self, path: &Path) -> ContentHash {
        ContentHash::of_parts(
            "hirsel.candidate.present.v2",
            [
                self.0.digest.0.as_slice(),
                path_bytes(path),
                node_digest(&self.0.state.nodes[path]).0.as_slice(),
            ],
        )
    }
    fn absent(&self, parent: &Path, leaf: &str) -> ContentHash {
        ContentHash::of_parts(
            "hirsel.candidate.absent.v2",
            [
                self.0.digest.0.as_slice(),
                path_bytes(parent),
                node_digest(&self.0.state.nodes[parent]).0.as_slice(),
                leaf.as_bytes(),
            ],
        )
    }
    /// Explicit bounded host observation, independent of frozen loading and
    /// check eligibility. Missing nodes are differences; I/O failure is incomplete.
    pub fn observe_disk(&self) -> io::Result<Vec<PathBuf>> {
        let source = DiskCapture;
        let mut differences = vec![];
        let mut bytes_left = self.0.limits.bytes;
        for (path, node) in &self.0.state.nodes {
            if matches!(node, Node::Ancestor(_)) {
                continue;
            }
            let facts = match source.facts(path) {
                Ok(facts) => facts,
                Err(e) if e.kind() == io::ErrorKind::NotFound => {
                    differences.push(path.clone());
                    continue;
                }
                Err(e) => return Err(e),
            };
            let equal = if facts.kind != node.kind() {
                false
            } else {
                match node {
                    Node::File { bytes, .. } => {
                        let actual = source.read(path, bytes_left)?;
                        bytes_left = bytes_left.checked_sub(actual.len()).ok_or_else(limit)?;
                        *bytes == actual
                    }
                    Node::Directory { children, .. } => {
                        let actual = source.directory(path, bytes_left)?;
                        let charge = actual.iter().map(|e| e.name.len() + 128).sum::<usize>();
                        bytes_left = bytes_left.checked_sub(charge).ok_or_else(limit)?;
                        actual
                            .into_iter()
                            .map(|e| {
                                e.name
                                    .into_string()
                                    .map(|name| (name, e.kind))
                                    .map_err(|_| invalid("non-UTF-8 name"))
                            })
                            .collect::<io::Result<BTreeMap<_, _>>>()?
                            == *children
                    }
                    Node::Symlink { target, .. } => {
                        source.read_link(path, self.0.limits.bytes)? == *target
                    }
                    Node::Opaque { .. } => true,
                    _ => unreachable!(),
                }
            };
            if !equal {
                differences.push(path.clone());
            }
        }
        for (operation, expected) in &self.0.state.failures {
            let current = match source.failed_probe(operation, bytes_left) {
                Ok(()) => None,
                Err(error) => Some(IoFailure::capture(error)),
            };
            if current.as_ref() != Some(expected) {
                differences.push(operation.path().to_owned());
            }
        }
        differences.sort();
        differences.dedup();
        Ok(differences)
    }
    /// Pure admission fact for a missing regular-file leaf. Does not grant
    /// directory creation or expose an unclosed ancestor as writable coverage.
    pub fn can_create(&self, key: &Key) -> io::Result<bool> {
        let path = self.path_for_key(key)?;
        Ok(matches!(
            self.0.state.nodes.get(path.parent().unwrap()),
            Some(Node::Directory { editable: true, .. })
        ))
    }
    /// Resolve an inspection key to its captured physical spelling, without I/O.
    pub fn path_for_key(&self, key: &Key) -> io::Result<PathBuf> {
        let root = self
            .0
            .state
            .roots
            .get(&key.root)
            .ok_or_else(|| unavailable(Operation::Metadata(PathBuf::from(&key.root))))?;
        edit_path(root, &key.path)
    }
    pub fn inspect(&self, root: &str, relative: &str) -> io::Result<Inspection> {
        let root = self
            .0
            .state
            .roots
            .get(root)
            .ok_or_else(|| unavailable(Operation::Metadata(PathBuf::from(root))))?;
        if relative.len() > self.0.limits.bytes {
            return Err(limit());
        }
        let path = edit_path(root, relative)?;
        match self.resolve(&path, &Operation::Metadata(path.clone()))? {
            Resolution::Present(path) => {
                let node = &self.0.state.nodes[&path];
                let (editable, bytes) = match node {
                    Node::File {
                        bytes, editable, ..
                    } => (*editable, Some(ContentHash::of_bytes(bytes))),
                    Node::Directory { editable, .. } => (*editable, None),
                    _ => (false, None),
                };
                Ok(Inspection::Present {
                    key: self.key(&path)?,
                    kind: node.kind(),
                    editable,
                    present: self.present(&path),
                    bytes,
                })
            }
            Resolution::Missing {
                path,
                parent,
                leaf,
                final_leaf,
                from_link,
            } => {
                if from_link {
                    Ok(Inspection::DanglingSymlink)
                } else if !final_leaf {
                    Ok(Inspection::MissingParent)
                } else {
                    Ok(Inspection::Absent {
                        key: self.key(&path)?,
                        parent: self.key(&parent)?,
                        absent: self.absent(&parent, &leaf),
                    })
                }
            }
        }
    }
}
impl LoaderInputs for NamespaceLease {
    fn read(&self, path: &Path) -> io::Result<Arc<[u8]>> {
        let path = self.existing(path, &Operation::Read(path.to_owned()))?;
        match &self.0.state.nodes[&path] {
            Node::File { bytes, .. } => Ok(bytes.clone()),
            Node::Directory { .. } | Node::Ancestor(_) => {
                Err(io::Error::from(io::ErrorKind::IsADirectory))
            }
            _ => unreachable!("resolved file or directory"),
        }
    }
    fn metadata(&self, path: &Path) -> io::Result<InputKind> {
        let path = self.existing(path, &Operation::Metadata(path.to_owned()))?;
        Ok(self.0.state.nodes[&path].kind())
    }
    fn canonicalize(&self, path: &Path) -> io::Result<PathBuf> {
        self.existing(path, &Operation::Canonicalize(path.to_owned()))
    }
    fn directory(&self, path: &Path) -> io::Result<Vec<DirectoryEntry>> {
        let op = Operation::Directory(path.to_owned());
        let canonical = self.existing(path, &op)?;
        match &self.0.state.nodes[&canonical] {
            Node::Directory { children, .. } => Ok(children
                .iter()
                .map(|(name, kind)| DirectoryEntry {
                    name: name.into(),
                    kind: *kind,
                })
                .collect()),
            Node::Ancestor(_) => Err(unavailable(op)),
            _ => Err(io::Error::from(io::ErrorKind::NotADirectory)),
        }
    }
    fn toolchain_std_manifest(&self) -> Option<io::Result<PathBuf>> {
        Some(match &self.0.state.std_manifest {
            Some(p) => self.canonicalize(p),
            None => Err(unavailable(Operation::Canonicalize(PathBuf::from(
                "<uncaptured-toolchain-std>",
            )))),
        })
    }
}
#[derive(Clone, Debug)]
pub enum Change {
    Create {
        key: Key,
        absent: ContentHash,
        bytes: Arc<[u8]>,
    },
    Replace {
        key: Key,
        present: ContentHash,
        expected_bytes: ContentHash,
        bytes: Arc<[u8]>,
    },
    Delete {
        key: Key,
        present: ContentHash,
        expected_bytes: ContentHash,
    },
}
impl Change {
    fn key(&self) -> &Key {
        match self {
            Self::Create { key, .. } | Self::Replace { key, .. } | Self::Delete { key, .. } => key,
        }
    }
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum OverlayError {
    Unavailable,
    InvalidPath,
    MissingParent,
    Unsupported,
    Precondition,
    StaleBytes,
    DuplicateTarget,
    InvalidUtf8,
    Limit,
}
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct NamespaceDelta {
    pub changed_objects: Vec<Key>,
    pub changed_directory_tables: Vec<Key>,
    /// Conservative: all captured symlink nodes when regular targets change.
    pub alias_resolution_changes: Vec<Key>,
}
impl NamespaceLease {
    /// Low-level final-state file overlay, not role/request admission. The v2
    /// owner validates source/manifest roles and uses existing apply_bytes first.
    /// All operations validate against this one base before allocating tables.
    pub fn overlay(&self, changes: &[Change]) -> Result<(Self, NamespaceDelta), OverlayError> {
        if changes.len() > self.0.limits.edits {
            return Err(OverlayError::Limit);
        }
        let mut targets = BTreeMap::new();
        let mut validations = BTreeMap::new();
        let mut ordered: Vec<_> = changes.iter().collect();
        ordered.sort_by(|a, b| (&a.key().root, &a.key().path).cmp(&(&b.key().root, &b.key().path)));
        for change in ordered {
            let key = change.key();
            let root = self
                .0
                .state
                .roots
                .get(&key.root)
                .ok_or(OverlayError::Unavailable)?;
            if key.path.len() > self.0.limits.bytes {
                return Err(OverlayError::Limit);
            }
            let declared = edit_path(root, &key.path).map_err(|_| OverlayError::InvalidPath)?;
            let inspection = self.inspect(&key.root, &key.path).map_err(|e| {
                if e.get_ref().is_some_and(|e| e.is::<InputUnavailable>()) {
                    OverlayError::Unavailable
                } else {
                    OverlayError::Unsupported
                }
            })?;
            let canonical_key = match &inspection {
                Inspection::Present { key, .. } | Inspection::Absent { key, .. } => key,
                Inspection::MissingParent => return Err(OverlayError::MissingParent),
                Inspection::DanglingSymlink => return Err(OverlayError::Unsupported),
            };
            let canonical_root = &self.0.state.roots[&canonical_key.root];
            let target = canonical_root.path.join(&canonical_key.path);
            if targets.insert(target.clone(), change).is_some() {
                return Err(OverlayError::DuplicateTarget);
            }
            validations.insert(target.clone(), (change, inspection, declared));
        }
        for (target, (change, inspection, declared)) in validations {
            match (change, inspection) {
                (
                    Change::Create { absent, bytes, .. },
                    Inspection::Absent {
                        absent: expected, ..
                    },
                ) => {
                    if *absent != expected {
                        return Err(OverlayError::Precondition);
                    }
                    if !matches!(
                        self.0.state.nodes.get(target.parent().unwrap()),
                        Some(Node::Directory { editable: true, .. })
                    ) {
                        return Err(OverlayError::Unsupported);
                    }
                    if bytes.len() > u32::MAX as usize {
                        return Err(OverlayError::Limit);
                    }
                    if std::str::from_utf8(bytes).is_err() {
                        return Err(OverlayError::InvalidUtf8);
                    }
                }
                (
                    Change::Replace {
                        present,
                        expected_bytes,
                        ..
                    }
                    | Change::Delete {
                        present,
                        expected_bytes,
                        ..
                    },
                    Inspection::Present {
                        present: expected,
                        bytes: Some(expected_hash),
                        editable: true,
                        kind: InputKind::File,
                        ..
                    },
                ) => {
                    if *present != expected {
                        return Err(OverlayError::Precondition);
                    }
                    if *expected_bytes != expected_hash {
                        return Err(OverlayError::StaleBytes);
                    }
                    if matches!(change, Change::Delete { .. }) && declared != target {
                        return Err(OverlayError::Unsupported);
                    }
                    if let Change::Replace { bytes, .. } = change {
                        if bytes.len() > u32::MAX as usize {
                            return Err(OverlayError::Limit);
                        }
                        if std::str::from_utf8(bytes).is_err() {
                            return Err(OverlayError::InvalidUtf8);
                        }
                    }
                }
                (
                    _,
                    Inspection::Present {
                        editable: false, ..
                    },
                ) => return Err(OverlayError::Unsupported),
                _ => return Err(OverlayError::Precondition),
            }
        }
        // Bound text bytes before table/byte allocation; no mutable ancestor chain.
        let added = targets
            .values()
            .try_fold(0usize, |n, c| match c {
                Change::Create { bytes, .. } | Change::Replace { bytes, .. } => {
                    n.checked_add(bytes.len())
                }
                _ => Some(n),
            })
            .ok_or(OverlayError::Limit)?;
        if added > self.0.limits.bytes {
            return Err(OverlayError::Limit);
        }
        let mut state = self.0.state.clone();
        let mut delta = NamespaceDelta::default();
        let mut parents = BTreeSet::new();
        for (target, change) in targets {
            delta
                .changed_objects
                .push(self.key(&target).map_err(|_| OverlayError::Unavailable)?);
            match change {
                Change::Create { bytes, .. } => {
                    let facts = Facts {
                        kind: InputKind::File,
                        device: 0,
                        inode: 0,
                        links: 1,
                        len: bytes.len() as u64,
                    };
                    state.nodes.insert(
                        target.clone(),
                        Node::File {
                            bytes: bytes.clone(),
                            editable: true,
                            facts,
                        },
                    );
                    parents.insert(target.parent().unwrap().to_owned());
                    let Node::Directory { children, .. } =
                        state.nodes.get_mut(target.parent().unwrap()).unwrap()
                    else {
                        unreachable!()
                    };
                    children.insert(
                        target.file_name().unwrap().to_str().unwrap().into(),
                        InputKind::File,
                    );
                }
                Change::Replace { bytes, .. } => {
                    let Node::File {
                        bytes: old, facts, ..
                    } = state.nodes.get_mut(&target).unwrap()
                    else {
                        unreachable!()
                    };
                    *old = bytes.clone();
                    facts.len = bytes.len() as u64;
                }
                Change::Delete { .. } => {
                    state.nodes.remove(&target);
                    parents.insert(target.parent().unwrap().to_owned());
                    let Node::Directory { children, .. } =
                        state.nodes.get_mut(target.parent().unwrap()).unwrap()
                    else {
                        unreachable!()
                    };
                    children.remove(target.file_name().unwrap().to_str().unwrap());
                }
            }
        }
        for path in parents {
            delta
                .changed_directory_tables
                .push(self.key(&path).map_err(|_| OverlayError::Unavailable)?);
        }
        if !delta.changed_objects.is_empty() {
            for (path, node) in &state.nodes {
                if matches!(node, Node::Symlink { .. }) {
                    delta
                        .alias_resolution_changes
                        .push(self.key(path).map_err(|_| OverlayError::Unavailable)?);
                }
            }
        }
        let lease = Self::publish(state, self.0.limits, &self.0.reservation.budget)
            .map_err(|_| OverlayError::Limit)?;
        Ok((lease, delta))
    }
}

// The host capture backend is intentionally Linux-only initially: descriptor
// traversal uses openat/O_NOFOLLOW for every ancestor, and rejects filesystem
// identity policies that it cannot prove case-sensitive. Other backends can
// implement CaptureSource explicitly; ordinary/v1 LoaderInputs are unchanged.
#[cfg(target_os = "linux")]
mod disk;
#[cfg(not(target_os = "linux"))]
impl CaptureSource for DiskCapture {
    fn facts(&self, _: &Path) -> io::Result<Facts> {
        Err(invalid("unsupported capture host"))
    }
    fn read(&self, _: &Path, _: usize) -> io::Result<Arc<[u8]>> {
        Err(invalid("unsupported capture host"))
    }
    fn directory(&self, _: &Path, _: usize) -> io::Result<Vec<DirectoryEntry>> {
        Err(invalid("unsupported capture host"))
    }
    fn read_link(&self, _: &Path, _: usize) -> io::Result<PathBuf> {
        Err(invalid("unsupported capture host"))
    }
    fn case_sensitive(&self, _: &Path) -> io::Result<bool> {
        Ok(false)
    }
    fn failed_probe(&self, _: &Operation, _: usize) -> io::Result<()> {
        Err(invalid("unsupported capture host"))
    }
}
#[cfg(all(test, target_os = "linux"))]
mod tests;

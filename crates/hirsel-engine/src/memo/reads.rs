//! Typed reads: what a memo observed, keyed by what it read (design §1b).
//! A memo is valid exactly when every read still observes the same value.
use hirsel_intern::Symbol;
use hirsel_syntax::{ContentHash, DefId, DefPath, FileContext, PackageId};
use std::num::NonZeroU64;

/// One revision of a session's inputs. Revision 1 is the first.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Revision(NonZeroU64);
impl Revision {
    pub const FIRST: Revision = Revision(NonZeroU64::MIN);
    pub fn get(self) -> u64 {
        self.0.get()
    }
    pub fn next(self) -> Revision {
        Revision(self.0.checked_add(1).expect("revision overflow"))
    }
    pub fn from_u64(n: u64) -> Option<Revision> {
        NonZeroU64::new(n).map(Revision)
    }
}

/// How often an input changes. Low is user package files, Medium is dependency
/// packages, High is std, the core registry and the environment. A memo's
/// durability is the minimum over everything it read.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum Durability {
    Low,
    Medium,
    High,
}
impl Durability {
    pub const ALL: [Durability; 3] = [Durability::Low, Durability::Medium, Durability::High];
}

/// A package, by its `name@version` spelling.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct PackageKey(pub Symbol);
impl PackageKey {
    pub fn of(package: &PackageId) -> Self {
        Self(Symbol::intern(&format!(
            "{}@{}",
            package.name, package.version
        )))
    }
    pub fn id(self) -> PackageId {
        let spelling = self.0.as_str();
        let (name, version) = spelling.rsplit_once('@').unwrap_or((spelling, ""));
        PackageId {
            name: name.into(),
            version: version.into(),
        }
    }
}
/// A module inside a package, by its dot-joined spelling (empty for the root).
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ModuleKey(pub Symbol);
impl ModuleKey {
    pub fn of(modules: &[String]) -> Self {
        Self(Symbol::intern(&modules.join(".")))
    }
}
/// Which lookup a read observed: the signature provider's, the resolver's
/// manifest index, or whether a module head is exposed.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum LookupSource {
    Provider,
    Manifest,
    Head,
    /// Accessible spelling repairs, observed only by missing-name diagnostics.
    Candidates,
}

/// One name lookup: the file context it ran in and the path it spelled.
/// Negative lookups are reads too.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct LookupKey {
    pub source: LookupSource,
    pub package: PackageKey,
    pub module: ModuleKey,
    /// The package a path names explicitly, if any.
    pub qualifier: Option<PackageKey>,
    /// The path's segments, dot-joined.
    pub path: Symbol,
}
impl LookupKey {
    pub fn of(
        source: LookupSource,
        context: &FileContext,
        qualifier: Option<&PackageId>,
        segments: &[String],
    ) -> Self {
        Self {
            source,
            package: PackageKey::of(&context.package),
            module: ModuleKey::of(&context.modules),
            qualifier: qualifier.map(PackageKey::of),
            path: Symbol::intern(&segments.join(".")),
        }
    }
    /// The context the lookup ran in.
    pub fn context(&self) -> FileContext {
        FileContext {
            package: self.package.id(),
            modules: split(self.module.0.as_str()),
        }
    }
    pub fn qualifier(&self) -> Option<PackageId> {
        self.qualifier.map(PackageKey::id)
    }
    pub fn segments(&self) -> Vec<String> {
        split(self.path.as_str())
    }
}
fn split(dotted: &str) -> Vec<String> {
    if dotted.is_empty() {
        vec![]
    } else {
        dotted.split('.').map(str::to_owned).collect()
    }
}

/// Every input a memo can observe. Each read records the observed value's
/// content hash (`Reads`).
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum ReadKey {
    /// A signature's fingerprint.
    Signature(DefId),
    /// A failed signature's failed key.
    FailedSignature(DefId),
    /// A lookup's output, including a negative one.
    Lookup(LookupKey),
    /// The set of names a module declares.
    NameSet(PackageKey, ModuleKey),
    /// The private view's `ViewDigest`.
    Representation(DefId),
    /// An executed body's text hash.
    ExecutedBody(DefId),
    /// An executed definition's signature fingerprint.
    ExecutedSignature(DefId),
    /// A cited item's text hash.
    CitedText(DefId),
    /// H(count, then (file path, header hash) per candidate in source order);
    /// read by every query keyed on the definition.
    DeclarationCandidates(DefId),
    /// V's digest of a view.
    ValidationView(DefId),
    /// The aggregate digest of the signature universe's V errors.
    UniverseErrors,
    /// The selected first heap role.
    FirstHeap,
    /// The query environment's hash.
    Environment,
    /// All V input views in the originating package's private universe.
    /// The package is explicit because execution-read unions cross packages.
    ValidationUniverse(PackageKey),
    /// A representation body consumed by H, without interpreting its code.
    LayoutBody(DefId),
    /// Normalized declaration structure, before private inference evidence.
    DeclaredSignature(DefId),
    /// Settled private inference output, independent of its module inputs.
    PrivateSummary(DefId),
}

/// The kind of a read, for counters and durability rules.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum ReadKind {
    Signature,
    FailedSignature,
    Lookup,
    NameSet,
    Representation,
    ExecutedBody,
    ExecutedSignature,
    CitedText,
    DeclarationCandidates,
    ValidationView,
    UniverseErrors,
    FirstHeap,
    Environment,
    ValidationUniverse,
    LayoutBody,
    DeclaredSignature,
    PrivateSummary,
}
impl ReadKind {
    pub const ALL: [ReadKind; 17] = [
        ReadKind::Signature,
        ReadKind::FailedSignature,
        ReadKind::Lookup,
        ReadKind::NameSet,
        ReadKind::Representation,
        ReadKind::ExecutedBody,
        ReadKind::ExecutedSignature,
        ReadKind::CitedText,
        ReadKind::DeclarationCandidates,
        ReadKind::ValidationView,
        ReadKind::UniverseErrors,
        ReadKind::FirstHeap,
        ReadKind::Environment,
        ReadKind::ValidationUniverse,
        ReadKind::LayoutBody,
        ReadKind::DeclaredSignature,
        ReadKind::PrivateSummary,
    ];
    pub fn name(self) -> &'static str {
        match self {
            ReadKind::Signature => "signature",
            ReadKind::DeclaredSignature => "declared signature",
            ReadKind::PrivateSummary => "private summary",
            ReadKind::FailedSignature => "failed signature",
            ReadKind::Lookup => "lookup",
            ReadKind::NameSet => "name set",
            ReadKind::Representation => "representation",
            ReadKind::ExecutedBody => "executed body",
            ReadKind::LayoutBody => "layout body",
            ReadKind::ExecutedSignature => "executed signature",
            ReadKind::CitedText => "cited text",
            ReadKind::DeclarationCandidates => "declaration candidates",
            ReadKind::ValidationView => "validation view",
            ReadKind::UniverseErrors => "universe errors",
            ReadKind::FirstHeap => "first heap",
            ReadKind::Environment => "environment",
            ReadKind::ValidationUniverse => "validation universe",
        }
    }
}

impl ReadKey {
    pub fn kind(&self) -> ReadKind {
        match self {
            ReadKey::Signature(_) => ReadKind::Signature,
            ReadKey::DeclaredSignature(_) => ReadKind::DeclaredSignature,
            ReadKey::PrivateSummary(_) => ReadKind::PrivateSummary,
            ReadKey::FailedSignature(_) => ReadKind::FailedSignature,
            ReadKey::Lookup(_) => ReadKind::Lookup,
            ReadKey::NameSet(..) => ReadKind::NameSet,
            ReadKey::Representation(_) => ReadKind::Representation,
            ReadKey::ExecutedBody(_) => ReadKind::ExecutedBody,
            ReadKey::LayoutBody(_) => ReadKind::LayoutBody,
            ReadKey::ExecutedSignature(_) => ReadKind::ExecutedSignature,
            ReadKey::CitedText(_) => ReadKind::CitedText,
            ReadKey::DeclarationCandidates(_) => ReadKind::DeclarationCandidates,
            ReadKey::ValidationView(_) => ReadKind::ValidationView,
            ReadKey::UniverseErrors => ReadKind::UniverseErrors,
            ReadKey::FirstHeap => ReadKind::FirstHeap,
            ReadKey::Environment => ReadKind::Environment,
            ReadKey::ValidationUniverse(_) => ReadKind::ValidationUniverse,
        }
    }
    /// The definition this read names, if any.
    pub fn definition(&self) -> Option<DefId> {
        match *self {
            ReadKey::Signature(d)
            | ReadKey::DeclaredSignature(d)
            | ReadKey::PrivateSummary(d)
            | ReadKey::FailedSignature(d)
            | ReadKey::Representation(d)
            | ReadKey::ExecutedBody(d)
            | ReadKey::LayoutBody(d)
            | ReadKey::ExecutedSignature(d)
            | ReadKey::CitedText(d)
            | ReadKey::DeclarationCandidates(d)
            | ReadKey::ValidationView(d) => Some(d),
            _ => None,
        }
    }
    /// The package whose inputs this read observes; `None` for the environment
    /// and V's universe-wide reads.
    pub fn package(&self) -> Option<PackageKey> {
        match self {
            ReadKey::Lookup(l) => Some(l.package),
            ReadKey::NameSet(p, _) => Some(*p),
            _ => self
                .definition()
                .map(|d| PackageKey::of(&d.path().0.package)),
        }
    }
    /// Exact persisted key length without allocating a byte buffer.
    pub fn encoded_bytes(&self) -> Option<usize> {
        let text = |s: &str| 4usize.checked_add(s.len());
        let payload = match *self {
            Self::Signature(d)
            | Self::DeclaredSignature(d)
            | Self::PrivateSummary(d)
            | Self::FailedSignature(d)
            | Self::Representation(d)
            | Self::ExecutedBody(d)
            | Self::LayoutBody(d)
            | Self::ExecutedSignature(d)
            | Self::CitedText(d)
            | Self::DeclarationCandidates(d)
            | Self::ValidationView(d) => encoded_def_path_bytes(d.path())?,
            Self::Lookup(l) => {
                let mut bytes = 2usize
                    .checked_add(text(l.package.0.as_str())?)?
                    .checked_add(text(l.module.0.as_str())?)?
                    .checked_add(text(l.path.as_str())?)?;
                if let Some(q) = l.qualifier {
                    bytes = bytes.checked_add(text(q.0.as_str())?)?;
                }
                bytes
            }
            Self::NameSet(p, m) => text(p.0.as_str())?.checked_add(text(m.0.as_str())?)?,
            Self::ValidationUniverse(p) => text(p.0.as_str())?,
            Self::UniverseErrors | Self::FirstHeap | Self::Environment => 0,
        };
        payload.checked_add(1)
    }
    /// Persisted form: kind tag, then DefPaths and spellings, never session ids.
    pub fn encode(&self, out: &mut Vec<u8>) {
        fn text(out: &mut Vec<u8>, s: &str) {
            out.extend((s.len() as u32).to_le_bytes());
            out.extend(s.as_bytes());
        }
        fn path(out: &mut Vec<u8>, d: DefId) {
            encode_def_path(out, d.path());
        }
        out.push(self.kind() as u8);
        match *self {
            ReadKey::Signature(d)
            | ReadKey::DeclaredSignature(d)
            | ReadKey::PrivateSummary(d)
            | ReadKey::FailedSignature(d)
            | ReadKey::Representation(d)
            | ReadKey::ExecutedBody(d)
            | ReadKey::LayoutBody(d)
            | ReadKey::ExecutedSignature(d)
            | ReadKey::CitedText(d)
            | ReadKey::DeclarationCandidates(d)
            | ReadKey::ValidationView(d) => path(out, d),
            ReadKey::Lookup(l) => {
                out.push(l.source as u8);
                text(out, l.package.0.as_str());
                text(out, l.module.0.as_str());
                match l.qualifier {
                    Some(q) => {
                        out.push(1);
                        text(out, q.0.as_str());
                    }
                    None => out.push(0),
                }
                text(out, l.path.as_str());
            }
            ReadKey::NameSet(p, m) => {
                text(out, p.0.as_str());
                text(out, m.0.as_str());
            }
            ReadKey::ValidationUniverse(package) => text(out, package.0.as_str()),
            ReadKey::UniverseErrors | ReadKey::FirstHeap | ReadKey::Environment => {}
        }
    }
}

/// A memo's reads: `(ReadKey, observed value)` sorted by key, one entry per key.
#[derive(Clone, Debug, Default)]
pub struct Reads {
    rows: std::sync::Arc<[(ReadKey, ContentHash)]>,
    // A sorted view into the same canonical rows. Normal receipts stay flat;
    // completed predecessor phases can share the final phase's allocation.
    indices: Option<std::sync::Arc<[u32]>>,
}
impl PartialEq for Reads {
    fn eq(&self, other: &Self) -> bool {
        self.iter().eq(other.iter())
    }
}
impl Eq for Reads {}
impl std::hash::Hash for Reads {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.len().hash(state);
        for row in self.iter() {
            row.hash(state);
        }
    }
}

/// Two observations of one key disagreed inside one evaluation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Inconsistent(pub ReadKey);

#[cfg(test)]
thread_local! { static SORT_COMPARISONS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) }; }

impl Reads {
    pub fn empty() -> Self {
        Self::default()
    }
    /// Sorts and de-duplicates; conflicting observations of one key are an error.
    pub fn from_vec(mut reads: Vec<(ReadKey, ContentHash)>) -> Result<Self, Inconsistent> {
        // Rust ea137335 std/collections/hash/map.rs:1002-1014: Entry
        // deduplicates by identity. Canonical spelling order is needed only
        // for the final unique set, not every inherited observation.
        if reads.len() >= 32 {
            use std::collections::{HashMap, hash_map::Entry};
            let mut unique = HashMap::with_hasher(hirsel_intern::IdHasher::default());
            let mut conflict: Option<ReadKey> = None;
            for (key, value) in reads {
                match unique.entry(key) {
                    Entry::Vacant(entry) => {
                        entry.insert(value);
                    }
                    Entry::Occupied(entry) => {
                        if *entry.get() != value && conflict.is_none_or(|old| key < old) {
                            conflict = Some(key);
                        }
                    }
                }
            }
            // Match the first conflicting key of the original canonical sort,
            // regardless of observation order or session identity assignment.
            if let Some(key) = conflict {
                return Err(Inconsistent(key));
            }
            reads = unique.into_iter().collect();
        }
        reads.sort_unstable_by(|a, b| {
            #[cfg(test)]
            SORT_COMPARISONS.with(|n| n.set(n.get() + 1));
            a.0.cmp(&b.0).then(a.1.0.cmp(&b.1.0))
        });
        reads.dedup();
        if let Some(w) = reads.windows(2).find(|w| w[0].0 == w[1].0) {
            return Err(Inconsistent(w[0].0));
        }
        Ok(Self {
            rows: reads.into(),
            indices: None,
        })
    }
    pub fn iter(&self) -> impl ExactSizeIterator<Item = &(ReadKey, ContentHash)> {
        (0..self.len())
            .map(|at| &self.rows[self.indices.as_ref().map_or(at, |rows| rows[at] as usize)])
    }
    pub fn len(&self) -> usize {
        self.indices
            .as_ref()
            .map_or(self.rows.len(), |rows| rows.len())
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
    pub fn get(&self, key: &ReadKey) -> Option<ContentHash> {
        let at = self.rows.binary_search_by(|(k, _)| k.cmp(key)).ok()?;
        if let Some(indices) = &self.indices {
            let index = u32::try_from(at).ok()?;
            indices.binary_search(&index).ok()?;
        }
        Some(self.rows[at].1)
    }
    /// A lossless view of these exact observations in a complete superset.
    /// Standard Arc slice ownership and sorted integer indices keep the flat
    /// binary-search/order/codec contract; no lookup can see an excluded row.
    pub(crate) fn share_rows(&self, complete: &Self) -> Option<Self> {
        if complete.indices.is_some() || self.len() > complete.len() {
            return None;
        }
        let mut indices = Vec::with_capacity(self.len());
        for (key, value) in self.iter() {
            let at = complete
                .rows
                .binary_search_by(|(candidate, _)| candidate.cmp(key))
                .ok()?;
            if complete.rows[at].1 != *value {
                return None;
            }
            indices.push(u32::try_from(at).ok()?);
        }
        Some(Self {
            rows: complete.rows.clone(),
            indices: (indices.len() != complete.len()).then(|| indices.into()),
        })
    }
    /// Actual distinct Arc backing allocations; the containing Reads value is
    /// accounted separately by its owner. This exposes no semantic identity.
    pub(crate) fn allocations(&self) -> impl Iterator<Item = (usize, usize)> {
        let rows = Some((
            self.rows.as_ptr() as usize,
            2 * size_of::<usize>() + size_of_val(&*self.rows),
        ));
        let indices = self.indices.as_ref().map(|indices| {
            (
                indices.as_ptr() as usize,
                2 * size_of::<usize>() + size_of_val(&**indices),
            )
        });
        [rows, indices].into_iter().flatten()
    }
    /// The union of two read sets (execution closure, design §1b).
    pub fn union(&self, other: &Reads) -> Result<Reads, Inconsistent> {
        let (mut a, mut b) = (self.iter().peekable(), other.iter().peekable());
        let mut out = Vec::with_capacity(self.len() + other.len());
        loop {
            match (a.peek(), b.peek()) {
                (Some(x), Some(y)) => match x.0.cmp(&y.0) {
                    std::cmp::Ordering::Less => out.push(*a.next().unwrap()),
                    std::cmp::Ordering::Greater => out.push(*b.next().unwrap()),
                    std::cmp::Ordering::Equal => {
                        if x.1 != y.1 {
                            return Err(Inconsistent(x.0));
                        }
                        out.push(*a.next().unwrap());
                        b.next();
                    }
                },
                (Some(_), None) => out.extend(a.by_ref().copied()),
                (None, Some(_)) => out.extend(b.by_ref().copied()),
                (None, None) => break,
            }
        }
        Ok(Self {
            rows: out.into(),
            indices: None,
        })
    }
    /// The minimum durability over every read, given each read's durability.
    pub fn durability(&self, of: impl Fn(&ReadKey) -> Durability) -> Durability {
        self.iter()
            .map(|(k, _)| of(k))
            .min()
            .unwrap_or(Durability::High)
    }
    /// The first read whose current value differs, by `current`.
    pub fn first_changed(
        &self,
        mut current: impl FnMut(&ReadKey) -> Option<ContentHash>,
    ) -> Option<ReadKey> {
        self.iter()
            .find(|(k, v)| current(k) != Some(*v))
            .map(|(k, _)| *k)
    }
    /// The canonical persisted encoding.
    pub fn encode(&self, out: &mut Vec<u8>) {
        // Version 2 separates lookup identity from diagnostic spelling repairs.
        // Reject pre-separation receipts even when they contain no new tags.
        out.extend(b"HRD2");
        out.extend((self.len() as u32).to_le_bytes());
        for (key, value) in self.iter() {
            key.encode(out);
            out.extend(value.0);
        }
    }
    pub fn decode(input: &mut &[u8]) -> Option<Reads> {
        if take(input, 4)? != b"HRD2" {
            return None;
        }
        let count = take_u32(input)? as usize;
        let mut reads = Vec::with_capacity(count.min(1 << 16));
        for _ in 0..count {
            let key = decode_key(input)?;
            let value = ContentHash(take(input, 32)?.try_into().ok()?);
            reads.push((key, value));
        }
        Reads::from_vec(reads).ok()
    }
    /// The content hash of the canonical encoding.
    pub fn digest(&self) -> ContentHash {
        let mut bytes = vec![];
        self.encode(&mut bytes);
        ContentHash::of_parts("hirsel.reads.v2", [bytes.as_slice()])
    }
}

/// Records reads during one evaluation (a task-local accumulator), then
/// freezes them into `Reads`.
#[derive(Clone, Debug, Default)]
pub struct ReadsBuilder(Vec<(ReadKey, ContentHash)>);
impl ReadsBuilder {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn record(&mut self, key: ReadKey, value: ContentHash) {
        self.0.push((key, value));
    }
    /// Merges another memo's complete reads (execution closure by union).
    pub fn merge(&mut self, reads: &Reads) {
        self.0.extend(reads.iter().copied());
    }
    /// Merges another evaluation's recorded reads.
    pub fn extend(&mut self, other: ReadsBuilder) {
        self.0.extend(other.0);
    }
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
    pub fn finish(self) -> Result<Reads, Inconsistent> {
        Reads::from_vec(self.0)
    }
}

impl serde::Serialize for Reads {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let mut bytes = vec![];
        self.encode(&mut bytes);
        s.serialize_bytes(&bytes)
    }
}
impl<'de> serde::Deserialize<'de> for Reads {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let bytes: Vec<u8> = serde::Deserialize::deserialize(d)?;
        let mut input = bytes.as_slice();
        Reads::decode(&mut input)
            .filter(|_| input.is_empty())
            .ok_or_else(|| serde::de::Error::custom("invalid reads"))
    }
}

fn take<'a>(input: &mut &'a [u8], n: usize) -> Option<&'a [u8]> {
    if input.len() < n {
        return None;
    }
    let (head, tail) = input.split_at(n);
    *input = tail;
    Some(head)
}
fn take_u32(input: &mut &[u8]) -> Option<u32> {
    Some(u32::from_le_bytes(take(input, 4)?.try_into().ok()?))
}
fn take_text<'a>(input: &mut &'a [u8]) -> Option<&'a str> {
    let n = take_u32(input)? as usize;
    std::str::from_utf8(take(input, n)?).ok()
}
fn take_def(input: &mut &[u8]) -> Option<DefId> {
    Some(DefId::intern(&decode_def_path(input)?))
}
fn put_text(out: &mut Vec<u8>, s: &str) {
    out.extend((s.len() as u32).to_le_bytes());
    out.extend(s.as_bytes());
}
/// A DefPath's persisted form: package name and version, module count, modules, name.
pub fn encode_def_path(out: &mut Vec<u8>, path: &DefPath) {
    let item = &path.0;
    put_text(out, &item.package.name);
    put_text(out, &item.package.version);
    out.extend((item.modules.len() as u32).to_le_bytes());
    for module in &item.modules {
        put_text(out, module);
    }
    put_text(out, &item.name);
}
/// Exact persisted path length; framing follows encode_def_path.
pub fn encoded_def_path_bytes(path: &DefPath) -> Option<usize> {
    let item = &path.0;
    std::iter::once(&item.package.name)
        .chain(std::iter::once(&item.package.version))
        .chain(&item.modules)
        .chain(std::iter::once(&item.name))
        .try_fold(4usize, |bytes, text| {
            bytes.checked_add(4)?.checked_add(text.len())
        })
}
pub fn decode_def_path(input: &mut &[u8]) -> Option<DefPath> {
    let name = take_text(input)?.to_owned();
    let version = take_text(input)?.to_owned();
    let count = take_u32(input)? as usize;
    let mut modules = Vec::with_capacity(count.min(64));
    for _ in 0..count {
        modules.push(take_text(input)?.to_owned());
    }
    Some(DefPath(hirsel_syntax::ItemPath {
        package: PackageId { name, version },
        modules,
        name: take_text(input)?.to_owned(),
    }))
}
fn decode_key(input: &mut &[u8]) -> Option<ReadKey> {
    let tag = *take(input, 1)?.first()?;
    let kind = *ReadKind::ALL.get(tag as usize)?;
    Some(match kind {
        ReadKind::Signature => ReadKey::Signature(take_def(input)?),
        ReadKind::DeclaredSignature => ReadKey::DeclaredSignature(take_def(input)?),
        ReadKind::PrivateSummary => ReadKey::PrivateSummary(take_def(input)?),
        ReadKind::FailedSignature => ReadKey::FailedSignature(take_def(input)?),
        ReadKind::Representation => ReadKey::Representation(take_def(input)?),
        ReadKind::ExecutedBody => ReadKey::ExecutedBody(take_def(input)?),
        ReadKind::LayoutBody => ReadKey::LayoutBody(take_def(input)?),
        ReadKind::ExecutedSignature => ReadKey::ExecutedSignature(take_def(input)?),
        ReadKind::CitedText => ReadKey::CitedText(take_def(input)?),
        ReadKind::DeclarationCandidates => ReadKey::DeclarationCandidates(take_def(input)?),
        ReadKind::ValidationView => ReadKey::ValidationView(take_def(input)?),
        ReadKind::Lookup => {
            let source = match *take(input, 1)?.first()? {
                0 => LookupSource::Provider,
                1 => LookupSource::Manifest,
                2 => LookupSource::Head,
                3 => LookupSource::Candidates,
                _ => return None,
            };
            let package = PackageKey(Symbol::intern(take_text(input)?));
            let module = ModuleKey(Symbol::intern(take_text(input)?));
            let qualifier = match *take(input, 1)?.first()? {
                0 => None,
                1 => Some(PackageKey(Symbol::intern(take_text(input)?))),
                _ => return None,
            };
            let path = Symbol::intern(take_text(input)?);
            ReadKey::Lookup(LookupKey {
                source,
                package,
                module,
                qualifier,
                path,
            })
        }
        ReadKind::NameSet => ReadKey::NameSet(
            PackageKey(Symbol::intern(take_text(input)?)),
            ModuleKey(Symbol::intern(take_text(input)?)),
        ),
        ReadKind::UniverseErrors => ReadKey::UniverseErrors,
        ReadKind::FirstHeap => ReadKey::FirstHeap,
        ReadKind::Environment => ReadKey::Environment,
        ReadKind::ValidationUniverse => {
            ReadKey::ValidationUniverse(PackageKey(Symbol::intern(take_text(input)?)))
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use hirsel_syntax::ItemPath;
    fn def(name: &str) -> DefId {
        DefId::intern(&DefPath(ItemPath {
            package: PackageId {
                name: "p".into(),
                version: "1".into(),
            },
            modules: vec!["m".into()],
            name: name.into(),
        }))
    }
    fn h(s: &str) -> ContentHash {
        ContentHash::of_bytes(s.as_bytes())
    }

    #[test]
    fn reads_sort_by_spelling_union_and_reject_conflicts() {
        let (a, b) = (def("a"), def("b"));
        let one = Reads::from_vec(vec![
            (ReadKey::Signature(b), h("b")),
            (ReadKey::Signature(a), h("a")),
            (ReadKey::Signature(a), h("a")),
        ])
        .unwrap();
        assert_eq!(one.len(), 2);
        assert_eq!(one.iter().next().unwrap().0, ReadKey::Signature(a));
        let two = Reads::from_vec(vec![(ReadKey::ExecutedBody(a), h("body"))]).unwrap();
        let both = one.union(&two).unwrap();
        assert_eq!(both.len(), 3);
        assert_eq!(both, two.union(&one).unwrap());
        let conflict = Reads::from_vec(vec![(ReadKey::Signature(a), h("other"))]).unwrap();
        assert_eq!(
            one.union(&conflict),
            Err(Inconsistent(ReadKey::Signature(a)))
        );
        assert!(
            Reads::from_vec(vec![
                (ReadKey::Signature(a), h("x")),
                (ReadKey::Signature(a), h("y"))
            ])
            .is_err()
        );
    }

    #[test]
    fn duplicate_read_observations_preserve_canonical_results_with_unique_sort_work() {
        fn original(mut rows: Vec<(ReadKey, ContentHash)>) -> Result<Reads, Inconsistent> {
            rows.sort_unstable_by(|a, b| a.0.cmp(&b.0).then(a.1.0.cmp(&b.1.0)));
            rows.dedup();
            if let Some(w) = rows.windows(2).find(|w| w[0].0 == w[1].0) {
                return Err(Inconsistent(w[0].0));
            }
            Ok(Reads {
                rows: rows.into(),
                indices: None,
            })
        }
        let (a, b) = (def("read_z"), def("read_a"));
        let ctx = FileContext {
            package: PackageId {
                name: "p".into(),
                version: "1".into(),
            },
            modules: vec!["m".into()],
        };
        let package = PackageKey::of(&ctx.package);
        let keys = [
            ReadKey::Signature(a),
            ReadKey::Signature(b),
            ReadKey::FailedSignature(b),
            ReadKey::Lookup(LookupKey::of(
                LookupSource::Provider,
                &ctx,
                None,
                &["x".into()],
            )),
            ReadKey::Lookup(LookupKey::of(
                LookupSource::Manifest,
                &ctx,
                Some(&ctx.package),
                &["y".into()],
            )),
            ReadKey::Lookup(LookupKey::of(LookupSource::Head, &ctx, None, &["m".into()])),
            ReadKey::NameSet(package, ModuleKey::of(&ctx.modules)),
            ReadKey::Representation(a),
            ReadKey::ExecutedBody(a),
            ReadKey::LayoutBody(a),
            ReadKey::ExecutedSignature(a),
            ReadKey::CitedText(b),
            ReadKey::DeclarationCandidates(b),
            ReadKey::ValidationView(a),
            ReadKey::UniverseErrors,
            ReadKey::FirstHeap,
            ReadKey::Environment,
            ReadKey::ValidationUniverse(package),
        ];
        let base: Vec<_> = keys
            .into_iter()
            .enumerate()
            .map(|(i, k)| (k, h(&i.to_string())))
            .collect();
        for seed in 0..24u64 {
            let mut rows: Vec<_> = (0..128).flat_map(|_| base.iter().copied()).collect();
            if seed % 2 != 0 {
                // Discovering a later conflict first must still report the
                // least key in the original canonical ordering.
                rows.extend([
                    (keys[12], h("bad-v")),
                    (keys[0], h("bad-z")),
                    (keys[1], h("bad-a")),
                    (keys[16], h("bad-u")),
                ]);
            }
            let mut random = seed + 1;
            for i in (1..rows.len()).rev() {
                random = random.wrapping_mul(6364136223846793005).wrapping_add(1);
                rows.swap(i, random as usize % (i + 1));
            }
            let expected = original(rows.clone());
            let got = Reads::from_vec(rows.clone());
            assert_eq!(got, expected, "seed {seed}");
            if let (Ok(got), Ok(expected)) = (got, expected) {
                let mut bytes = vec![];
                got.encode(&mut bytes);
                let mut reference = vec![];
                expected.encode(&mut reference);
                assert_eq!(bytes, reference);
                assert_eq!(got.digest(), expected.digest());
                for (key, value) in &base {
                    assert_eq!(got.get(key), Some(*value));
                }
                let mut builder = ReadsBuilder::new();
                builder.merge(&got);
                builder.merge(&got);
                assert_eq!(builder.clone().finish(), Ok(expected));
                assert_eq!(builder.finish(), Ok(got));
            }
        }
        assert_eq!(Reads::from_vec(vec![]), Ok(Reads::empty()));
        let repeated = (0..128).flat_map(|_| base.iter().copied()).collect();
        let before = SORT_COMPARISONS.with(std::cell::Cell::get);
        assert_eq!(Reads::from_vec(repeated), original(base));
        let comparisons = SORT_COMPARISONS.with(std::cell::Cell::get) - before;
        assert!(
            comparisons < 256,
            "sort unique read keys, not duplicate observations: {comparisons}"
        );
    }

    #[test]
    fn read_allocation_layout_includes_even_the_empty_arc_header() {
        let empty = Reads::empty();
        assert_eq!(
            empty.allocations().map(|(_, bytes)| bytes).sum::<usize>(),
            2 * size_of::<usize>()
        );
        let complete = Reads::from_vec(vec![(ReadKey::Environment, h("env"))]).unwrap();
        let subset = empty.share_rows(&complete).unwrap();
        assert_eq!(
            subset.allocations().map(|(_, bytes)| bytes).sum::<usize>(),
            4 * size_of::<usize>() + size_of::<(ReadKey, ContentHash)>()
        );
    }
    #[test]
    fn shared_subsets_keep_order_get_encoding_hash_and_exclude_unobserved_rows() {
        use std::hash::{Hash, Hasher};
        let a = ReadKey::Signature(def("a"));
        let b = ReadKey::ExecutedBody(def("b"));
        let c = ReadKey::Signature(def("c"));
        let complete = Reads::from_vec(vec![(a, h("a")), (b, h("b")), (c, h("c"))]).unwrap();
        let hash = |reads: &Reads| {
            let mut h = std::collections::hash_map::DefaultHasher::new();
            reads.hash(&mut h);
            h.finish()
        };
        for values in [
            vec![],
            vec![(a, h("a"))],
            vec![(a, h("a")), (b, h("b"))],
            complete.iter().copied().collect(),
        ] {
            let original = Reads::from_vec(values).unwrap();
            let shared = original.share_rows(&complete).unwrap();
            assert_eq!(shared, original);
            assert_eq!(shared.len(), original.len());
            assert_eq!(shared.iter().len(), original.len());
            assert_eq!(hash(&shared), hash(&original));
            assert_eq!(shared.digest(), original.digest());
            let mut bytes = vec![];
            shared.encode(&mut bytes);
            assert_eq!(Reads::decode(&mut bytes.as_slice()), Some(original.clone()));
            for key in [a, b, c] {
                assert_eq!(shared.get(&key), original.get(&key));
            }
            assert_eq!(shared.union(&complete), original.union(&complete));
            assert_eq!(shared.rows.as_ptr(), complete.rows.as_ptr());
        }
        let inconsistent = Reads::from_vec(vec![(a, h("other"))]).unwrap();
        assert!(inconsistent.share_rows(&complete).is_none());
        let missing = Reads::from_vec(vec![(ReadKey::Environment, h("e"))]).unwrap();
        assert!(missing.share_rows(&complete).is_none());
    }
    #[test]
    fn encoding_round_trips_through_paths_not_ids() {
        let context = FileContext {
            package: PackageId {
                name: "p".into(),
                version: "1".into(),
            },
            modules: vec!["m".into(), "n".into()],
        };
        let reads = Reads::from_vec(vec![
            (ReadKey::Signature(def("a")), h("1")),
            (ReadKey::DeclarationCandidates(def("a")), h("2")),
            (
                ReadKey::Lookup(LookupKey::of(
                    LookupSource::Manifest,
                    &context,
                    None,
                    &["x".into(), "y".into()],
                )),
                h("3"),
            ),
            (
                ReadKey::NameSet(PackageKey::of(&context.package), ModuleKey::of(&[])),
                h("4"),
            ),
            (ReadKey::Environment, h("5")),
            (ReadKey::UniverseErrors, h("6")),
            (
                ReadKey::ValidationUniverse(PackageKey::of(&context.package)),
                h("7"),
            ),
        ])
        .unwrap();
        let mut bytes = vec![];
        reads.encode(&mut bytes);
        let mut input = bytes.as_slice();
        assert_eq!(Reads::decode(&mut input), Some(reads.clone()));
        assert!(input.is_empty());
        let ReadKey::Lookup(lookup) = reads
            .iter()
            .find(|(k, _)| k.kind() == ReadKind::Lookup)
            .unwrap()
            .0
        else {
            unreachable!()
        };
        assert_eq!(lookup.context(), context);
        assert_eq!(lookup.segments(), vec!["x".to_string(), "y".to_string()]);
        assert_eq!(lookup.qualifier(), None);
    }

    #[test]
    fn diagnostic_lookup_reads_are_distinct_and_reject_old_encodings() {
        let context = FileContext {
            package: def("owner").path().0.package.clone(),
            modules: vec!["m".into()],
        };
        let segments = ["missing".into()];
        let identity = ReadKey::Lookup(LookupKey::of(
            LookupSource::Manifest,
            &context,
            None,
            &segments,
        ));
        let candidates = ReadKey::Lookup(LookupKey::of(
            LookupSource::Candidates,
            &context,
            None,
            &segments,
        ));
        let reads =
            Reads::from_vec(vec![(identity, h("absent")), (candidates, h("repairs"))]).unwrap();
        assert_eq!(reads.len(), 2);
        let mut bytes = vec![];
        reads.encode(&mut bytes);
        assert_eq!(&bytes[..4], b"HRD2");
        assert_eq!(Reads::decode(&mut bytes.as_slice()), Some(reads));
        // The old encoding began directly with the row count.
        assert!(Reads::decode(&mut &bytes[4..]).is_none());
        assert!(Reads::decode(&mut &[0u8; 4][..]).is_none());
        let mut unknown = [ReadKind::Lookup as u8, 4];
        assert!(decode_key(&mut unknown.as_slice()).is_none());
        unknown[1] = LookupSource::Candidates as u8;
        assert!(
            decode_key(&mut unknown.as_slice()).is_none(),
            "truncated lookup"
        );
    }

    #[test]
    fn durability_is_the_minimum_and_changes_are_found() {
        let reads = Reads::from_vec(vec![
            (ReadKey::Signature(def("a")), h("a")),
            (ReadKey::Environment, h("e")),
        ])
        .unwrap();
        let of = |k: &ReadKey| match k {
            ReadKey::Environment => Durability::High,
            _ => Durability::Low,
        };
        assert_eq!(reads.durability(of), Durability::Low);
        assert_eq!(Reads::empty().durability(of), Durability::High);
        assert_eq!(
            reads.first_changed(|k| reads.get(k)),
            None,
            "unchanged reads verify"
        );
        assert_eq!(
            reads.first_changed(|k| match k {
                ReadKey::Environment => Some(h("e2")),
                k => reads.get(k),
            }),
            Some(ReadKey::Environment)
        );
    }
}

#[cfg(test)]
mod admission_tests {
    use super::*;
    #[test]
    fn persisted_key_lengths_cover_every_read_kind_and_unicode_lookup_frame() {
        let path = DefPath(hirsel_syntax::ItemPath {
            package: PackageId {
                name: "pkg🐑".into(),
                version: "2".into(),
            },
            modules: vec!["α".into(), "module".into()],
            name: "field\"".into(),
        });
        let id = DefId::intern(&path);
        let ctx = FileContext {
            package: path.0.package.clone(),
            modules: path.0.modules.clone(),
        };
        let package = PackageKey::of(&ctx.package);
        let mut keys = vec![
            ReadKey::Signature(id),
            ReadKey::FailedSignature(id),
            ReadKey::Representation(id),
            ReadKey::ExecutedBody(id),
            ReadKey::LayoutBody(id),
            ReadKey::DeclaredSignature(id),
            ReadKey::PrivateSummary(id),
            ReadKey::ExecutedSignature(id),
            ReadKey::CitedText(id),
            ReadKey::DeclarationCandidates(id),
            ReadKey::ValidationView(id),
            ReadKey::NameSet(package, ModuleKey::of(&ctx.modules)),
            ReadKey::ValidationUniverse(package),
            ReadKey::UniverseErrors,
            ReadKey::FirstHeap,
            ReadKey::Environment,
        ];
        for source in [
            LookupSource::Provider,
            LookupSource::Manifest,
            LookupSource::Head,
            LookupSource::Candidates,
        ] {
            for qualifier in [None, Some(&ctx.package)] {
                keys.push(ReadKey::Lookup(LookupKey::of(
                    source,
                    &ctx,
                    qualifier,
                    &["δ".into(), "x🐑".into()],
                )));
            }
        }
        for key in keys {
            let mut bytes = vec![];
            key.encode(&mut bytes);
            assert_eq!(key.encoded_bytes(), Some(bytes.len()), "{key:?}");
        }
        let mut bytes = vec![];
        encode_def_path(&mut bytes, &path);
        assert_eq!(encoded_def_path_bytes(&path), Some(bytes.len()));
    }
}

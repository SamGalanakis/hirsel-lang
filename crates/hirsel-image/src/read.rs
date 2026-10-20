//! Reading an image: the file is mapped, the header and the type rows are read
//! when it opens, and everything else when asked for.
use crate::map::Bytes;
use crate::*;
use bincode::Options;
use hirsel_intern::Symbol;
use hirsel_intern::persist::Source;
use hirsel_span::{Anchor, AnchorCodec};
use hirsel_syntax::{ContentHash, DefId};
use hirsel_types::{TypeBase, TypeTable};
use serde::de::DeserializeOwned;
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::{Arc, OnceLock};

/// An open image. Cloning shares it.
#[derive(Clone)]
pub struct Image(Arc<Inner>);

struct Inner {
    tables: Arc<Tables>,
    key: ContentHash,
    /// The anchor of each number from 1, with its source's manifest ordinal
    /// and its base there.
    anchors: Vec<(Anchor, u32, u32)>,
    base: Arc<TypeBase>,
}

/// The mapped bytes and the id tables decoding reads through.
struct Tables {
    bytes: Bytes,
    /// Each section's count and byte range, by kind.
    sections: BTreeMap<u32, (u32, std::ops::Range<usize>)>,
    symbols: Box<[OnceLock<Symbol>]>,
    defs: Box<[OnceLock<DefId>]>,
}

fn u32_at(bytes: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(bytes.get(at..at + 4)?.try_into().ok()?))
}
fn u64_at(bytes: &[u8], at: usize) -> Option<u64> {
    Some(u64::from_le_bytes(bytes.get(at..at + 8)?.try_into().ok()?))
}

/// The key and section directory of an image's first bytes.
type Directory = (ContentHash, BTreeMap<u32, (u32, std::ops::Range<usize>)>);
fn directory(bytes: &[u8]) -> Result<Directory, String> {
    if bytes.get(..8) != Some(&MAGIC[..]) {
        return Err("not a Hirsel std image".into());
    }
    let version = u32_at(bytes, 8).ok_or("truncated image header")?;
    if version != VERSION {
        return Err(format!("image format {version}, expected {VERSION}"));
    }
    let count = u32_at(bytes, 12).ok_or("truncated image header")? as usize;
    let key = ContentHash(
        bytes
            .get(16..HEADER)
            .ok_or("truncated image header")?
            .try_into()
            .map_err(|_| "truncated image header")?,
    );
    let mut sections = BTreeMap::new();
    for i in 0..count {
        let at = HEADER + i * DIRECTORY_ENTRY;
        let field = |n: Option<u64>| n.ok_or("truncated image directory");
        let kind = field(u32_at(bytes, at).map(u64::from))? as u32;
        let entries = field(u32_at(bytes, at + 4).map(u64::from))? as u32;
        let start = field(u64_at(bytes, at + 8))?;
        let len = field(u64_at(bytes, at + 16))?;
        let end = start
            .checked_add(len)
            .filter(|end| *end <= bytes.len() as u64)
            .ok_or_else(|| format!("image section {kind} lies outside the file"))?;
        sections.insert(kind, (entries, start as usize..end as usize));
    }
    Ok((key, sections))
}

impl Image {
    /// The key of the image at `path`, reading only its header: whether an
    /// image is current is decided before it is mapped and decoded.
    pub fn read_key(path: &Path) -> Result<ContentHash, String> {
        use std::io::Read;
        let mut head = [0u8; HEADER];
        std::fs::File::open(path)
            .and_then(|mut file| file.read_exact(&mut head))
            .map_err(|e| format!("{}: {e}", path.display()))?;
        if head[..8] != MAGIC {
            return Err(format!("{}: not a Hirsel std image", path.display()));
        }
        Ok(ContentHash(head[16..].try_into().expect("32 bytes")))
    }

    /// Map the image at `path` and decode its tables and type rows.
    pub fn open(path: &Path) -> Result<Self, String> {
        let bytes = Bytes::map(path).map_err(|e| format!("{}: {e}", path.display()))?;
        Self::new(bytes).map_err(|e| format!("{}: {e}", path.display()))
    }

    /// An image held in memory.
    pub fn from_bytes(bytes: Vec<u8>) -> Result<Self, String> {
        Self::new(Bytes::Owned(bytes))
    }

    fn new(bytes: Bytes) -> Result<Self, String> {
        let (key, sections) = directory(bytes.as_slice())?;
        let count = |kind: u32| sections.get(&kind).map_or(0, |s| s.0 as usize);
        let anchors = match sections.get(&ANCHORS) {
            Some((n, range)) => {
                let body = &bytes.as_slice()[range.clone()];
                (0..*n as usize)
                    .map(|i| {
                        let ordinal = u32_at(body, i * 8).ok_or("truncated anchor table")?;
                        let base = u32_at(body, i * 8 + 4).ok_or("truncated anchor table")?;
                        Ok((Anchor::fresh(), ordinal, base))
                    })
                    .collect::<Result<_, String>>()?
            }
            None => vec![],
        };
        let tables = Arc::new(Tables {
            symbols: (0..count(STRINGS)).map(|_| OnceLock::new()).collect(),
            defs: (0..count(DEFS)).map(|_| OnceLock::new()).collect(),
            bytes,
            sections,
        });
        let types: TypeTable = match tables.sections.get(&TYPES) {
            Some((rows, range)) => {
                let table: TypeTable = with_tables(&tables, &[], 0, || {
                    decode(&tables.bytes.as_slice()[range.clone()])
                })?;
                if table.len() != *rows as usize {
                    return Err("the type table's row count differs from its header".into());
                }
                table
            }
            None => return Err("an image without a type table".into()),
        };
        Ok(Image(Arc::new(Inner {
            tables,
            key,
            anchors,
            base: types.into_base(),
        })))
    }

    pub fn key(&self) -> ContentHash {
        self.0.key
    }
    /// The type rows, as the base of each session interner that decodes
    /// values from this image.
    pub fn base(&self) -> Arc<TypeBase> {
        self.0.base.clone()
    }
    /// The anchors decoded spans name, each with its source's manifest
    /// ordinal and its base there; a session places them.
    pub fn anchors(&self) -> &[(Anchor, u32, u32)] {
        &self.0.anchors
    }
    /// The image's size in bytes.
    pub fn len(&self) -> usize {
        self.0.tables.bytes.as_slice().len()
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The value of blob section `kind`, if the image has one.
    pub fn blob<T: DeserializeOwned>(&self, kind: u32) -> Result<Option<T>, String> {
        match self.0.tables.sections.get(&kind) {
            Some((_, range)) => self.0.decode(range.clone()).map(Some),
            None => Ok(None),
        }
    }
    /// How many entries keyed section `kind` holds.
    pub fn entries(&self, kind: u32) -> usize {
        self.0
            .tables
            .sections
            .get(&kind)
            .map_or(0, |s| s.0 as usize)
    }
    /// The definition of each entry of keyed section `kind`, in entry
    /// (DefPath) order.
    pub fn keys(&self, kind: u32) -> Result<Vec<DefId>, String> {
        (0..self.entries(kind))
            .map(|i| {
                let (number, _) = self.0.tables.entry(kind, i)?;
                self.0
                    .tables
                    .def(number)
                    .ok_or_else(|| format!("section {kind} entry {i} names no definition"))
            })
            .collect()
    }
    /// The entry ordinal of `d` in keyed section `kind`.
    pub fn find(&self, kind: u32, d: DefId) -> Option<usize> {
        let tables = &self.0.tables;
        let number = tables.number(d)?;
        let (count, range) = tables.sections.get(&kind)?;
        let body = &tables.bytes.as_slice()[range.clone()];
        let (mut low, mut high) = (0usize, *count as usize);
        while low < high {
            let mid = (low + high) / 2;
            let at = u32_at(body, mid * KEYED_ENTRY)?;
            match at.cmp(&number) {
                std::cmp::Ordering::Less => low = mid + 1,
                std::cmp::Ordering::Greater => high = mid,
                std::cmp::Ordering::Equal => return Some(mid),
            }
        }
        None
    }
    /// Decode entry `ordinal` of keyed section `kind`.
    pub fn decode<T: DeserializeOwned>(&self, kind: u32, ordinal: usize) -> Result<T, String> {
        let (_, range) = self.0.tables.entry(kind, ordinal)?;
        self.0.decode(range)
    }
    /// Decode the entry for `d` in keyed section `kind`, if it has one.
    pub fn entry<T: DeserializeOwned>(&self, kind: u32, d: DefId) -> Result<Option<T>, String> {
        self.find(kind, d)
            .map(|ordinal| self.decode(kind, ordinal))
            .transpose()
    }
}

fn decode<T: DeserializeOwned>(bytes: &[u8]) -> Result<T, String> {
    options()
        .with_limit(bytes.len() as u64)
        .deserialize(bytes)
        .map_err(|e| format!("corrupt image value: {e}"))
}

impl Inner {
    fn decode<T: DeserializeOwned>(&self, range: std::ops::Range<usize>) -> Result<T, String> {
        with_tables(&self.tables, &self.anchors, self.base.len(), || {
            decode(&self.tables.bytes.as_slice()[range])
        })
    }
}

/// Run `f` reading ids through `tables`, spans' anchors as `anchors` and
/// `TypeId`s as rows of a base of `rows` rows.
fn with_tables<R>(
    tables: &Arc<Tables>,
    anchors: &[(Anchor, u32, u32)],
    rows: usize,
    f: impl FnOnce() -> R,
) -> R {
    let mut codec = AnchorCodec::Decode(anchors.iter().map(|a| a.0).collect());
    hirsel_intern::persist::with_symbol_source(tables.clone(), || {
        hirsel_syntax::with_def_source(tables.clone(), || {
            hirsel_types::with_base_decoding(rows, || hirsel_span::with_anchor_codec(&mut codec, f))
        })
    })
}

impl Tables {
    fn bytes(&self, kind: u32) -> &[u8] {
        match self.sections.get(&kind) {
            Some((_, range)) => &self.bytes.as_slice()[range.clone()],
            None => &[],
        }
    }
    fn string(&self, number: u32) -> Option<&str> {
        let body = self.bytes(STRINGS);
        let count = self.symbols.len();
        let at = |i: usize| u32_at(body, i * 4).map(|o| o as usize);
        let (start, end) = (at(number as usize)?, at(number as usize + 1)?);
        let text = (count + 1) * 4;
        std::str::from_utf8(body.get(text + start..text + end)?).ok()
    }
    /// Definition `number`'s words: package name, version, item name, modules.
    fn words(&self, number: u32) -> Option<impl Iterator<Item = u32> + '_> {
        let body = self.bytes(DEFS);
        let area = (self.defs.len() + 1) * 4;
        let start = u32_at(body, number as usize * 4)? as usize;
        let end = u32_at(body, number as usize * 4 + 4)? as usize;
        if end < start + 3 || area + end * 4 > body.len() {
            return None;
        }
        Some((start..end).map(move |w| u32_at(body, area + w * 4).expect("checked range")))
    }
    fn path(&self, number: u32) -> Option<hirsel_syntax::DefPath> {
        let mut words = self.words(number)?;
        let mut next = || -> Option<String> { Some(self.string(words.next()?)?.to_owned()) };
        let package = hirsel_syntax::PackageId {
            name: next()?,
            version: next()?,
        };
        let name = next()?;
        let mut modules = vec![];
        while let Some(module) = next() {
            modules.push(module);
        }
        Some(hirsel_syntax::DefPath(hirsel_syntax::ItemPath {
            package,
            modules,
            name,
        }))
    }
    /// The definition number of `d`, if the image's table has it.
    fn number(&self, d: DefId) -> Option<u32> {
        let (slots, range) = self.sections.get(&DEF_INDEX)?;
        let slots = *slots as usize;
        if slots == 0 || !slots.is_power_of_two() {
            return None;
        }
        let body = &self.bytes.as_slice()[range.clone()];
        let path = &d.path().0;
        let mut slot = first_slot(d.fingerprint(), slots);
        for _ in 0..slots {
            let number = u32_at(body, slot * 4)?.checked_sub(1)?;
            if let Some(known) = self.defs.get(number as usize)?.get() {
                if *known == d {
                    return Some(number);
                }
            } else {
                let mut words = self.words(number)?;
                let mut part = || words.next().and_then(|w| self.string(w));
                let matches = part() == Some(&path.package.name)
                    && part() == Some(&path.package.version)
                    && part() == Some(&path.name)
                    && path.modules.iter().all(|m| part() == Some(m))
                    && part().is_none();
                if matches {
                    let _ = self.defs[number as usize].set(d);
                    return Some(number);
                }
            }
            slot = (slot + 1) & (slots - 1);
        }
        None
    }
    /// Entry `ordinal` of keyed section `kind`: its definition number and byte range.
    fn entry(&self, kind: u32, ordinal: usize) -> Result<(u32, std::ops::Range<usize>), String> {
        let (count, range) = self
            .sections
            .get(&kind)
            .ok_or_else(|| format!("the image has no section {kind}"))?;
        if ordinal >= *count as usize {
            return Err(format!("section {kind} has no entry {ordinal}"));
        }
        let body = &self.bytes.as_slice()[range.clone()];
        let at = ordinal * KEYED_ENTRY;
        let field = |i: usize| u32_at(body, at + i * 4).ok_or("truncated section index");
        let (number, start, end) = (field(0)?, field(1)? as usize, field(2)? as usize);
        if start > end || end > body.len() {
            return Err(format!("section {kind} entry {ordinal} lies outside it"));
        }
        Ok((number, range.start + start..range.start + end))
    }
    fn symbol(&self, number: u32) -> Option<Symbol> {
        let cell = self.symbols.get(number as usize)?;
        if let Some(symbol) = cell.get() {
            return Some(*symbol);
        }
        let symbol = Symbol::intern(self.string(number)?);
        Some(*cell.get_or_init(|| symbol))
    }
    fn def(&self, number: u32) -> Option<DefId> {
        let cell = self.defs.get(number as usize)?;
        if let Some(d) = cell.get() {
            return Some(*d);
        }
        let d = DefId::intern(&self.path(number)?);
        Some(*cell.get_or_init(|| d))
    }
}

impl Source<Symbol> for Tables {
    fn id(&self, number: u32) -> Option<Symbol> {
        self.symbol(number)
    }
}
impl Source<DefId> for Tables {
    fn id(&self, number: u32) -> Option<DefId> {
        self.def(number)
    }
}

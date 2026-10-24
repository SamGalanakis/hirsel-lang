//! Byte-oriented source locations (LANG-1, FAIL-1); columns count Unicode scalars.
//!
//! A [`Span`] is relative to an [`Anchor`]: a file's (base 0) or one top-level
//! item occurrence's. Moving an item moves its anchor; the spans inside it never
//! change. [`FileSpan`] is the resolved form, produced only for rendering and
//! output.

#[cfg(test)]
hirsel_guard::install!();

use hirsel_intern::Scramble;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

/// Session-local index into a source map, never a definition identity (PROG-1).
#[derive(
    Clone,
    Copy,
    Debug,
    Default,
    Eq,
    PartialEq,
    Hash,
    Ord,
    PartialOrd,
    serde::Serialize,
    serde::Deserialize,
)]
pub struct FileId(pub u32);

/// Position base of spans: a file's (base 0) or one top-level item
/// occurrence's. Ids are unique in the process, so a span keeps its meaning in
/// every source map that places its anchor; each map places it at that map's
/// own file and offset. Never persisted or printed. `Anchor(0)`, the default,
/// is no anchor: its spans resolve to no file.
#[derive(
    Clone,
    Copy,
    Debug,
    Default,
    Eq,
    PartialEq,
    Hash,
    Ord,
    PartialOrd,
    serde::Serialize,
    serde::Deserialize,
)]
pub struct Anchor(pub u32);

/// Half-open UTF-8 byte range relative to an anchor's base (FAIL-1). Empty
/// ranges mark insertions. Persisted spans name their anchors through an
/// [`AnchorCodec`].
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Hash)]
pub struct Span {
    pub anchor: Anchor,
    pub start: u32,
    pub end: u32,
}
impl Span {
    pub fn new(anchor: Anchor, start: u32, end: u32) -> Option<Self> {
        (start <= end).then_some(Self { anchor, start, end })
    }
}

/// How spans name anchors while serialized inside [`with_anchor_codec`]:
/// anchors are process-local, so a persisted artifact numbers the anchors it
/// holds and maps them back to anchors of the reading process. Number 0 is no
/// anchor. Outside a codec a span carries its anchor id as is.
pub enum AnchorCodec {
    /// Number anchors in first-encounter order while collecting a record.
    Collect(Vec<Anchor>),
    /// Each anchor's number; another anchor fails the serialization.
    Encode(BTreeMap<Anchor, u32>),
    /// The anchor of each number, from 1.
    Decode(Vec<Anchor>),
    /// An immutable image's anchors, shared by all its entry decoders.
    DecodeShared(std::sync::Arc<[Anchor]>),
}
thread_local! {
    static CODEC: std::cell::RefCell<Option<AnchorCodec>> =
        const { std::cell::RefCell::new(None) };
}
/// Serialize or deserialize spans in `f` through `codec`.
pub fn with_anchor_codec<R>(codec: &mut AnchorCodec, f: impl FnOnce() -> R) -> R {
    let empty = AnchorCodec::Decode(vec![]);
    let outer = CODEC.replace(Some(std::mem::replace(codec, empty)));
    let result = f();
    *codec = CODEC.replace(outer).expect("anchor codec");
    result
}
impl serde::Serialize for Span {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let anchor = CODEC.with_borrow_mut(|codec| match codec {
            Some(AnchorCodec::Collect(anchors)) if self.anchor != Anchor(0) => {
                let index = anchors
                    .iter()
                    .position(|a| *a == self.anchor)
                    .unwrap_or_else(|| {
                        anchors.push(self.anchor);
                        anchors.len() - 1
                    });
                Ok(index as u32 + 1)
            }
            Some(AnchorCodec::Collect(_)) => Ok(0),
            Some(AnchorCodec::Encode(numbers)) if self.anchor != Anchor(0) => numbers
                .get(&self.anchor)
                .copied()
                .ok_or_else(|| serde::ser::Error::custom("span outside the encoded sources")),
            Some(AnchorCodec::Encode(_)) => Ok(0),
            _ => Ok(self.anchor.0),
        })?;
        (anchor, self.start, self.end).serialize(serializer)
    }
}
impl<'de> serde::Deserialize<'de> for Span {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let (number, start, end) = <(u32, u32, u32)>::deserialize(deserializer)?;
        let anchor = CODEC.with_borrow(|codec| match codec {
            Some(AnchorCodec::Decode(anchors)) if number != 0 => anchors
                .get(number as usize - 1)
                .copied()
                .ok_or_else(|| serde::de::Error::custom("span anchor out of range")),
            Some(AnchorCodec::Decode(_)) => Ok(Anchor(0)),
            Some(AnchorCodec::DecodeShared(anchors)) if number != 0 => anchors
                .get(number as usize - 1)
                .copied()
                .ok_or_else(|| serde::de::Error::custom("span anchor out of range")),
            Some(AnchorCodec::DecodeShared(_)) => Ok(Anchor(0)),
            _ => Ok(Anchor(number)),
        })?;
        Ok(Span { anchor, start, end })
    }
}

/// A span resolved to its file's own byte offsets, for rendering and output only.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Hash, Ord, PartialOrd)]
pub struct FileSpan {
    pub file: FileId,
    pub start: u32,
    pub end: u32,
}

/// The file an anchor the map never allocated resolves to; no file has it.
const NO_FILE: FileId = FileId(u32::MAX);

/// Anchor ids are permuted under `id-salt`, like every session id (oracle O1).
static ANCHORS: Scramble = Scramble::new("anchor");

/// One-based line and Unicode-scalar column, not terminal display cells (SEM-3).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LineColumn {
    pub line: u32,
    pub column: u32,
}

/// Immutable source and its line-start index (LANG-1, SEM-3).
#[derive(Clone)]
pub struct SourceFile {
    /// Physical path for diagnostics and reading source; never emit into artifacts.
    pub path: PathBuf,
    logical_path: PathBuf,
    /// This file's own anchor, with base 0.
    pub anchor: Anchor,
    /// Shared: a source map is cloned into every check's output, and a
    /// clone copies no text.
    text: std::sync::Arc<str>,
    line_starts: std::sync::Arc<[u32]>,
    /// The id of this source's first snapshot, the id a fresh load gives it:
    /// a resident session's later snapshots of a source keep it, so output
    /// naming a file by id is the same as a fresh process's.
    ordinal: u32,
    owner: Option<RetainedSourceOwner>,
}
// Source ownership is representation/lifetime metadata, never source identity.
#[derive(Clone)]
struct RetainedSourceOwner(std::sync::Arc<dyn Send + Sync>);
impl std::fmt::Debug for SourceFile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SourceFile")
            .field("path", &self.path)
            .field("logical_path", &self.logical_path)
            .field("anchor", &self.anchor)
            .field("text", &self.text)
            .field("line_starts", &self.line_starts)
            .field("ordinal", &self.ordinal)
            .finish()
    }
}
impl SourceFile {
    /// Owned path buffers only. Text and line indexes retain their shared
    /// source owner; cloning a SourceFile does not copy those allocations.
    pub fn retained_metadata_bytes(&self) -> u64 {
        self.path
            .capacity()
            .saturating_add(self.logical_path.capacity()) as u64
    }

    pub fn ordinal(&self) -> u32 {
        self.ordinal
    }
    /// Synthetic DWARF root, identical on every host. GDB remaps the package
    /// prefix in relative file names to that package's local manifest directory.
    pub const COMPILATION_DIRECTORY: &str = "/hirsel";
    /// Root-independent source name for artifact strings and DWARF. Debuggers
    /// remap the package prefix to the local manifest directory (like
    /// `-fdebug-prefix-map`); the physical path remains available to diagnostics.
    pub fn logical_path(&self) -> &Path {
        &self.logical_path
    }
    pub fn text(&self) -> &str {
        &self.text
    }
    pub fn location(&self, offset: u32) -> Option<LineColumn> {
        let offset_usize = offset as usize;
        if !self.text.is_char_boundary(offset_usize) {
            return None;
        }
        let line = self.line_starts.partition_point(|&start| start <= offset) - 1;
        let start = self.line_starts[line] as usize;
        Some(LineColumn {
            line: line as u32 + 1,
            column: self.text[start..offset_usize].chars().count() as u32 + 1,
        })
    }
    pub fn line(&self, line: u32) -> Option<&str> {
        let index = line.checked_sub(1)? as usize;
        let start = *self.line_starts.get(index)? as usize;
        let end = self
            .line_starts
            .get(index + 1)
            .map_or(self.text.len(), |&v| v as usize);
        Some(self.text[start..end].trim_end_matches(['\r', '\n']))
    }
}

/// Vec-backed immutable file snapshots and where each anchor stands in them;
/// FileIds are valid only in this map (PROG-5). Both tables are shared on
/// clone and copied on the next change, so every check's output carries the
/// map at no cost.
#[derive(Clone, Default)]
pub struct SourceMap {
    files: std::sync::Arc<Vec<SourceFile>>,
    /// Each placed anchor's file and base offset in it.
    anchors: std::sync::Arc<BTreeMap<Anchor, (FileId, u32)>>,
    owner: Option<RetainedSourceOwner>,
}
impl std::fmt::Debug for SourceMap {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SourceMap")
            .field("files", &self.files)
            .field("anchors", &self.anchors)
            .finish()
    }
}
impl Anchor {
    /// A new anchor, distinct from every other in the process.
    pub fn fresh() -> Self {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        Anchor(ANCHORS.encode(NEXT.fetch_add(1, Ordering::Relaxed)).get())
    }
}
impl SourceMap {
    /// Copied file-table storage after decorating a cloned snapshot with an
    /// owner. Text, line indices and anchors remain shared with the source.
    /// Excludes this inline value and the supplied owner's existing charge.
    pub fn retained_snapshot_metadata_bytes(&self) -> u64 {
        let mut bytes = (2 * size_of::<usize>())
            .saturating_add(size_of::<Vec<SourceFile>>())
            .saturating_add(
                self.files
                    .capacity()
                    .saturating_mul(size_of::<SourceFile>()),
            );
        for file in self.files.iter() {
            bytes = bytes
                .saturating_add(file.path.capacity())
                .saturating_add(file.logical_path.capacity());
        }
        if self.owner.is_some() {
            // retain_owner adds one Arc containing the prior and supplied
            // owners. Prior owners already retain their own original grants.
            type Owner = std::sync::Arc<dyn Send + Sync>;
            bytes = bytes
                .saturating_add(2 * size_of::<usize>())
                .saturating_add(size_of::<(Option<Owner>, Owner)>());
        }
        bytes as u64
    }
    /// Retain an admitted source owner through every clone of this map or its
    /// files. The owner changes no text, anchors, locations or debug encoding.
    /// Attach it to the external snapshot, leaving the producer's own map
    /// undecorated so the owner cannot retain itself through its sources.
    pub fn retain_owner(mut self, owner: std::sync::Arc<dyn Send + Sync>) -> Self {
        let previous = self.owner.take().map(|owner| owner.0);
        let owner = RetainedSourceOwner(std::sync::Arc::new((previous, owner)));
        for file in std::sync::Arc::make_mut(&mut self.files) {
            file.owner = Some(owner.clone());
        }
        self.owner = Some(owner);
        self
    }

    /// Backing source text, line indexes, paths and placed-anchor tables owned
    /// by this immutable map. A shared clone is charged by its one owner.
    pub fn input_bytes(&self) -> Option<u64> {
        let mut n = (std::mem::size_of::<Self>()
            + self.files.capacity() * std::mem::size_of::<SourceFile>()) as u64;
        for file in self.files.iter() {
            n = n
                .checked_add(file.text.len() as u64)?
                .checked_add((file.line_starts.len() * std::mem::size_of::<u32>()) as u64)?
                .checked_add(file.path.capacity() as u64)?
                .checked_add(file.logical_path.capacity() as u64)?;
        }
        // Rust std's B-tree degree is six (alloc/collections/btree/node.rs):
        // at most eleven pairs/twelve edges, at least five pairs per non-root
        // node. Include an internal-node/header bound for every possible node.
        let nodes = self.anchors.len().div_ceil(5).checked_add(1)?;
        let node = 18 * std::mem::size_of::<usize>()
            + 11 * (std::mem::size_of::<Anchor>() + std::mem::size_of::<(FileId, u32)>())
            + std::mem::align_of::<usize>();
        n.checked_add(nodes.checked_mul(node)? as u64)
    }

    /// Returns None when the u32 representation cannot contain this file/map.
    pub fn add(&mut self, path: impl AsRef<Path>, text: String) -> Option<FileId> {
        let logical = Path::new("standalone@0").join(
            path.as_ref()
                .file_name()
                .unwrap_or_else(|| std::ffi::OsStr::new("<source>")),
        );
        self.add_logical(path, logical, text)
    }
    /// Add a snapshot with a package-qualified, manifest-relative artifact name.
    pub fn add_logical(
        &mut self,
        path: impl AsRef<Path>,
        logical_path: impl AsRef<Path>,
        text: String,
    ) -> Option<FileId> {
        u32::try_from(text.len()).ok()?;
        let id = FileId(u32::try_from(self.files.len()).ok()?);
        let anchor = self.item_anchor(id, 0);
        let mut line_starts = vec![0];
        for (i, b) in text.bytes().enumerate() {
            if b == b'\n' {
                line_starts.push((i + 1) as u32);
            }
        }
        std::sync::Arc::make_mut(&mut self.files).push(SourceFile {
            path: path.as_ref().to_owned(),
            logical_path: logical_path.as_ref().to_owned(),
            anchor,
            text: text.into(),
            line_starts: line_starts.into(),
            ordinal: id.0,
            owner: self.owner.clone(),
        });
        Some(id)
    }
    /// A new snapshot of `file`'s source with `text`: the same path, logical
    /// path and ordinal at a new id, as a resident session replaces an edited
    /// source.
    pub fn add_snapshot(&mut self, file: FileId, text: String) -> Option<FileId> {
        let old = self.file(file)?;
        let (path, logical, ordinal) = (old.path.clone(), old.logical_path.clone(), old.ordinal);
        let id = self.add_logical(path, logical, text)?;
        std::sync::Arc::make_mut(&mut self.files)[id.0 as usize].ordinal = ordinal;
        Some(id)
    }
    pub fn file(&self, id: FileId) -> Option<&SourceFile> {
        self.files.get(id.0 as usize)
    }
    /// The anchor of `f` itself, with base 0. A file the map does not hold has
    /// no anchor.
    pub fn file_anchor(&self, f: FileId) -> Anchor {
        self.file(f).map_or(Anchor(0), |file| file.anchor)
    }
    /// A new anchor at byte `base` of `f`, for one top-level item occurrence.
    pub fn item_anchor(&mut self, f: FileId, base: u32) -> Anchor {
        let anchor = Anchor::fresh();
        std::sync::Arc::make_mut(&mut self.anchors).insert(anchor, (f, base));
        anchor
    }
    /// Place `a` at byte `base` of `f`: every span relative to it moves with
    /// it. The anchor may come from another map holding the same text.
    pub fn move_anchor(&mut self, a: Anchor, f: FileId, base: u32) {
        std::sync::Arc::make_mut(&mut self.anchors).insert(a, (f, base));
    }
    /// The file and base an anchor stands at in this map.
    pub fn anchor(&self, a: Anchor) -> Option<(FileId, u32)> {
        self.anchors.get(&a).copied()
    }
    /// Every anchor this map places, for another map holding the same files at
    /// the same FileIds.
    pub fn anchors(&self) -> impl Iterator<Item = (Anchor, FileId, u32)> + '_ {
        self.anchors.iter().map(|(&a, &(f, base))| (a, f, base))
    }
    /// The span in its file's own offsets. An anchor this map does not place
    /// resolves to a file the map does not hold.
    pub fn resolve(&self, s: Span) -> FileSpan {
        match self.anchor(s.anchor) {
            Some((file, base)) => FileSpan {
                file,
                start: base.saturating_add(s.start),
                end: base.saturating_add(s.end),
            },
            None => FileSpan {
                file: NO_FILE,
                start: s.start,
                end: s.end,
            },
        }
    }
    pub fn location(&self, span: Span) -> Option<LineColumn> {
        self.slice(span)?;
        let span = self.resolve(span);
        self.file(span.file)?.location(span.start)
    }
    pub fn slice(&self, span: Span) -> Option<&str> {
        self.file_slice(self.resolve(span))
    }
    pub fn file_slice(&self, span: FileSpan) -> Option<&str> {
        self.file(span.file)?
            .text
            .get(span.start as usize..span.end as usize)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unicode_crlf_eof_and_invalid_ranges() {
        let mut sources = SourceMap::default();
        let f = sources.add("a.hir", "é\r\nx\n".into()).unwrap();
        let source = sources.file(f).unwrap();
        assert_eq!(source.location(4), Some(LineColumn { line: 2, column: 1 }));
        assert_eq!(source.location(2).unwrap().column, 2);
        assert_eq!(source.location(6).unwrap().line, 3);
        assert_eq!(source.location(1), None);
        assert_eq!(source.location(7), None);
        assert_eq!(source.line(1), Some("é"));
        assert_eq!(
            sources.slice(Span {
                anchor: sources.file_anchor(f),
                start: 2,
                end: 1
            }),
            None
        );
    }
    #[test]
    fn moving_an_item_anchor_moves_only_its_spans() {
        let mut sources = SourceMap::default();
        let a = sources
            .add("a.hir", "fn f() {}\nfn g() {}\n".into())
            .unwrap();
        let b = sources
            .add("b.hir", "// moved\nfn g() {}\n".into())
            .unwrap();
        let f = sources.item_anchor(a, 0);
        let g = sources.item_anchor(a, 10);
        let name = Span::new(g, 3, 4).unwrap();
        assert_eq!(sources.slice(name), Some("g"));
        assert_eq!(
            sources.resolve(name),
            FileSpan {
                file: a,
                start: 13,
                end: 14
            }
        );
        sources.move_anchor(g, b, 9);
        assert_eq!(sources.slice(name), Some("g"));
        assert_eq!(
            sources.location(name),
            Some(LineColumn { line: 2, column: 4 })
        );
        assert_eq!(sources.slice(Span::new(f, 3, 4).unwrap()), Some("f"));
        // Another map holding the same text places the anchor at its own file.
        let mut other = SourceMap::default();
        let c = other.add("c.hir", "fn g() {}\n".into()).unwrap();
        other.move_anchor(g, c, 0);
        assert_eq!(other.slice(name), Some("g"));
        assert_eq!(other.resolve(name).file, c);
        // A file's own anchor has base 0; an unknown anchor names no file.
        let file = Span::new(sources.file_anchor(b), 0, 8).unwrap();
        assert_eq!(sources.slice(file), Some("// moved"));
        let nowhere = Span::new(Anchor(0), 0, 1).unwrap();
        assert_eq!(sources.slice(nowhere), None);
        assert!(sources.file(sources.resolve(nowhere).file).is_none());
        assert_eq!(sources.slice(Span::default()), None);
    }
}

#[cfg(test)]
mod retained_source_owner_tests {
    use super::*;
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    struct Owner(Arc<AtomicUsize>);
    impl Drop for Owner {
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }
    #[test]
    fn owned_snapshot_metadata_counts_copied_paths_without_shared_text() {
        let mut original = SourceMap::default();
        original.add("main.hir", "x".repeat(64 << 10)).unwrap();
        let mut decorated = original.clone().retain_owner(Arc::new(()));
        let file = &mut Arc::get_mut(&mut decorated.files).unwrap()[0];
        file.path.reserve(4096);
        file.logical_path.reserve(2048);
        assert!(Arc::ptr_eq(&file.text, &original.files[0].text));
        assert!(Arc::ptr_eq(
            &file.line_starts,
            &original.files[0].line_starts
        ));
        assert!(Arc::ptr_eq(&decorated.anchors, &original.anchors));
        let bytes = decorated.retained_snapshot_metadata_bytes();
        assert!(bytes >= 6144, "copied path capacities must remain charged");
        assert!(
            bytes < 64 << 10,
            "shared source text must not be charged again"
        );
        drop(original);
        assert_eq!(decorated.files[0].text.len(), 64 << 10);
        assert_eq!(decorated.retained_snapshot_metadata_bytes(), bytes);
    }
    #[test]
    fn extracted_source_file_keeps_snapshot_owner_after_all_maps_drop() {
        let dropped = Arc::new(AtomicUsize::new(0));
        let mut sources = SourceMap::default();
        let file = sources
            .add("main.hir", "pub fn main() {}\n".into())
            .unwrap();
        let original_debug = format!("{sources:?}");
        let sources = sources.retain_owner(Arc::new(Owner(dropped.clone())));
        assert_eq!(format!("{sources:?}"), original_debug);
        let clone = sources.clone();
        let extracted = sources.file(file).unwrap().clone();
        drop(sources);
        drop(clone);
        assert_eq!(dropped.load(Ordering::SeqCst), 0);
        assert_eq!(extracted.text(), "pub fn main() {}\n");
        drop(extracted);
        assert_eq!(dropped.load(Ordering::SeqCst), 1);
    }
    #[test]
    fn empty_and_extended_maps_retain_all_admitted_owners() {
        let first = Arc::new(AtomicUsize::new(0));
        let second = Arc::new(AtomicUsize::new(0));
        let mut sources = SourceMap::default().retain_owner(Arc::new(Owner(first.clone())));
        let empty = sources.clone();
        let file = sources.add("added.hir", "x".into()).unwrap();
        let sources = sources.retain_owner(Arc::new(Owner(second.clone())));
        let extracted = sources.file(file).unwrap().clone();
        drop(sources);
        drop(empty);
        assert_eq!(first.load(Ordering::SeqCst), 0);
        assert_eq!(second.load(Ordering::SeqCst), 0);
        drop(extracted);
        assert_eq!(first.load(Ordering::SeqCst), 1);
        assert_eq!(second.load(Ordering::SeqCst), 1);
    }
}

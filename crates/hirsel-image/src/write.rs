//! Writing an image: values are collected, then encoded in canonical order
//! (`ImageWriter::finish`), so the order products were computed in, and the
//! session's id assignment, never reach the bytes.
use crate::*;
use bincode::Options;
use hirsel_intern::Symbol;
use hirsel_intern::persist::Numbering;
use hirsel_span::{Anchor, AnchorCodec};
use hirsel_syntax::{ContentHash, DefId};
use hirsel_types::{TableBuilder, TypeInterner};
use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use std::sync::Arc;

/// A value an image section holds.
pub trait Encode: Send {
    /// Serialize the value. A `TypeId`, `Symbol`, `DefId` or span in it is
    /// written through the image's tables.
    fn encode(&self, out: &mut Vec<u8>) -> Result<(), String>;
    /// A smaller value to hold instead when this one does not encode: for
    /// instance a record without its TIR when a span in it names a source
    /// outside the image's package. None drops the entry.
    fn reduced(&self) -> Option<Box<dyn Encode>> {
        None
    }
}

/// Serialize `value` as an image value.
pub fn encode_value<T: serde::Serialize + ?Sized>(
    value: &T,
    out: &mut Vec<u8>,
) -> Result<(), String> {
    options()
        .serialize_into(out, value)
        .map_err(|e| e.to_string())
}

/// One keyed section's entries.
type Keyed = Vec<(DefId, Box<dyn Encode>)>;

/// An image being written.
pub struct ImageWriter {
    key: ContentHash,
    types: Arc<TypeInterner>,
    anchors: Vec<(Anchor, u32, u32)>,
    blobs: BTreeMap<u32, Box<dyn Encode>>,
    keyed: BTreeMap<u32, Keyed>,
}

/// A finished image and the entries it left out.
pub struct Written {
    pub bytes: Vec<u8>,
    /// Each entry that encoded neither whole nor reduced: its section kind,
    /// its definition and why.
    pub dropped: Vec<(u32, DefId, String)>,
}

impl ImageWriter {
    /// An image keyed by `key`, whose values' `TypeId`s are rows of `types`.
    /// The image's base begins with the rows of `types`' own base, in its
    /// order, so ids naming them mean the same over either base.
    pub fn new(key: ContentHash, types: Arc<TypeInterner>) -> Self {
        Self {
            key,
            types,
            anchors: vec![],
            blobs: BTreeMap::new(),
            keyed: BTreeMap::new(),
        }
    }
    /// The anchors spans in values may name: each with its source's manifest
    /// ordinal and its base there. A span naming another anchor does not encode.
    pub fn anchors(&mut self, anchors: impl IntoIterator<Item = (Anchor, u32, u32)>) {
        self.anchors.extend(anchors);
    }
    /// Section `kind` holds one value.
    pub fn blob(&mut self, kind: u32, value: Box<dyn Encode>) {
        assert!(kind >= FIRST_KIND, "section kind {kind} is reserved");
        assert!(!self.keyed.contains_key(&kind), "section {kind} is keyed");
        self.blobs.insert(kind, value);
    }
    /// Section `kind` holds `value` for `d`.
    pub fn entry(&mut self, kind: u32, d: DefId, value: Box<dyn Encode>) {
        assert!(kind >= FIRST_KIND, "section kind {kind} is reserved");
        assert!(!self.blobs.contains_key(&kind), "section {kind} is a blob");
        self.keyed.entry(kind).or_default().push((d, value));
    }

    /// Encode every section in canonical order: definitions keyed in any
    /// section are numbered first, in DefPath order; then sections in kind
    /// order, keyed entries in DefPath order, numbering each id they meet;
    /// then the type rows those reach, and the tables.
    pub fn finish(self) -> Result<Written, String> {
        let mut tables = Tables {
            symbols: Numbering::default(),
            defs: Numbering::default(),
            types: match self.types.base() {
                Some(base) => TableBuilder::over(self.types.clone(), base),
                None => TableBuilder::new(self.types.clone()),
            },
            anchors: anchor_numbers(&self.anchors),
        };
        let mut keys: Vec<DefId> = self
            .keyed
            .values()
            .flat_map(|entries| entries.iter().map(|(d, _)| *d))
            .collect();
        keys.sort();
        keys.dedup();
        for d in keys {
            tables.defs.number(d);
        }
        let mut dropped = vec![];
        let mut sections: Vec<(u32, u32, Vec<u8>)> = vec![];
        let kinds: std::collections::BTreeSet<u32> = self
            .blobs
            .keys()
            .chain(self.keyed.keys())
            .copied()
            .collect();
        let mut blobs = self.blobs;
        let mut keyed = self.keyed;
        for kind in kinds {
            if let Some(value) = blobs.remove(&kind) {
                let mut body = vec![];
                tables
                    .encode(&*value, &mut body)
                    .map_err(|e| format!("section {kind}: {e}"))?;
                sections.push((kind, 0, body));
                continue;
            }
            let mut entries = keyed.remove(&kind).unwrap_or_default();
            entries.sort_by_key(|entry| entry.0);
            if let Some(pair) = entries.windows(2).find(|pair| pair[0].0 == pair[1].0) {
                return Err(format!("section {kind} holds {} twice", pair[0].0));
            }
            let mut index = Vec::with_capacity(entries.len());
            let mut data = vec![];
            for (d, value) in entries {
                let start = data.len();
                let mut value = Some(value);
                while let Some(current) = value.take() {
                    match tables.encode(&*current, &mut data) {
                        Ok(()) => index.push((tables.defs.number(d), start, data.len())),
                        Err(e) => {
                            data.truncate(start);
                            value = current.reduced();
                            if value.is_none() {
                                dropped.push((kind, d, e));
                            }
                        }
                    }
                }
            }
            let table = index.len() * KEYED_ENTRY;
            let mut body = Vec::with_capacity(table + data.len());
            for (number, start, end) in &index {
                for word in [*number, offset(table + start)?, offset(table + end)?] {
                    body.extend_from_slice(&word.to_le_bytes());
                }
            }
            body.extend_from_slice(&data);
            sections.push((kind, index.len() as u32, body));
        }
        // The rows values named and the rows those reach.
        let Tables {
            mut symbols,
            mut defs,
            types,
            anchors,
        } = tables;
        let table = types.finish();
        let rows = table.len() as u32;
        let mut types_body = vec![];
        hirsel_intern::persist::with_symbol_numbering(&mut symbols, || {
            hirsel_syntax::with_def_numbering(&mut defs, || encode_value(&table, &mut types_body))
        })
        .map_err(|e| format!("type rows: {e}"))?;
        // Definitions, whose path components join the strings.
        let mut words: Vec<Vec<u32>> = vec![];
        for d in defs.ids() {
            let path = &d.path().0;
            let mut parts = vec![
                path.package.name.as_str(),
                path.package.version.as_str(),
                path.name.as_str(),
            ];
            parts.extend(path.modules.iter().map(String::as_str));
            words.push(
                parts
                    .into_iter()
                    .map(|part| symbols.number(Symbol::intern(part)))
                    .collect(),
            );
        }
        let mut reserved = vec![
            (
                STRINGS,
                symbols.ids().len() as u32,
                strings_body(symbols.ids())?,
            ),
            (DEFS, words.len() as u32, defs_body(&words)?),
        ];
        let slots = (defs.ids().len() * 2).next_power_of_two().max(8);
        reserved.push((DEF_INDEX, slots as u32, def_index_body(defs.ids(), slots)));
        let mut anchors_body = vec![];
        for (ordinal, base) in &anchors.places {
            anchors_body.extend_from_slice(&ordinal.to_le_bytes());
            anchors_body.extend_from_slice(&base.to_le_bytes());
        }
        reserved.push((ANCHORS, anchors.places.len() as u32, anchors_body));
        reserved.push((TYPES, rows, types_body));
        reserved.extend(sections);
        Ok(Written {
            bytes: assemble(self.key, &reserved)?,
            dropped,
        })
    }
}

/// An offset or length within a section, which the format keeps in a u32.
fn offset(n: usize) -> Result<u32, String> {
    u32::try_from(n).map_err(|_| "an image section exceeds 4 GiB".to_string())
}

/// The anchor numbering: each distinct (ordinal, base) in that order from 1,
/// and every anchor at it.
struct AnchorNumbers {
    numbers: BTreeMap<Anchor, u32>,
    places: Vec<(u32, u32)>,
}
fn anchor_numbers(anchors: &[(Anchor, u32, u32)]) -> AnchorNumbers {
    let mut places: Vec<(u32, u32)> = anchors.iter().map(|&(_, o, b)| (o, b)).collect();
    places.sort();
    places.dedup();
    let number: HashMap<(u32, u32), u32> = places
        .iter()
        .enumerate()
        .map(|(i, place)| (*place, i as u32 + 1))
        .collect();
    AnchorNumbers {
        numbers: anchors
            .iter()
            .map(|&(a, o, b)| (a, number[&(o, b)]))
            .collect(),
        places,
    }
}

struct Tables {
    symbols: Numbering<Symbol>,
    defs: Numbering<DefId>,
    types: TableBuilder,
    anchors: AnchorNumbers,
}
impl Tables {
    /// Encode `value` with every id written through the tables.
    fn encode(&mut self, value: &dyn Encode, out: &mut Vec<u8>) -> Result<(), String> {
        let Tables {
            symbols,
            defs,
            types,
            anchors,
        } = self;
        let mut codec = AnchorCodec::Encode(std::mem::take(&mut anchors.numbers));
        let result = hirsel_intern::persist::with_symbol_numbering(symbols, || {
            hirsel_syntax::with_def_numbering(defs, || {
                types.with_encoding(|| {
                    hirsel_span::with_anchor_codec(&mut codec, || value.encode(out))
                })
            })
        });
        let AnchorCodec::Encode(numbers) = codec else {
            unreachable!("an encoding codec stays one");
        };
        anchors.numbers = numbers;
        result
    }
}

fn strings_body(symbols: &[Symbol]) -> Result<Vec<u8>, String> {
    let mut offsets = Vec::with_capacity(symbols.len() + 1);
    let mut bytes = vec![];
    offsets.push(0);
    for symbol in symbols {
        bytes.extend_from_slice(symbol.as_str().as_bytes());
        offsets.push(offset(bytes.len())?);
    }
    let mut body = Vec::with_capacity(offsets.len() * 4 + bytes.len());
    for o in offsets {
        body.extend_from_slice(&o.to_le_bytes());
    }
    body.extend_from_slice(&bytes);
    Ok(body)
}

fn defs_body(words: &[Vec<u32>]) -> Result<Vec<u8>, String> {
    let mut offsets = vec![0u32];
    let mut area = vec![];
    for def in words {
        area.extend_from_slice(def);
        offsets.push(offset(area.len())?);
    }
    Ok(offsets
        .iter()
        .chain(&area)
        .flat_map(|word| word.to_le_bytes())
        .collect())
}

fn def_index_body(defs: &[DefId], slots: usize) -> Vec<u8> {
    let mut table = vec![0u32; slots];
    for (number, d) in defs.iter().enumerate() {
        let mut slot = first_slot(d.fingerprint(), slots);
        while table[slot] != 0 {
            slot = (slot + 1) & (slots - 1);
        }
        table[slot] = number as u32 + 1;
    }
    table.iter().flat_map(|slot| slot.to_le_bytes()).collect()
}

fn assemble(key: ContentHash, sections: &[(u32, u32, Vec<u8>)]) -> Result<Vec<u8>, String> {
    let directory = HEADER + sections.len() * DIRECTORY_ENTRY;
    let total = directory + sections.iter().map(|s| s.2.len()).sum::<usize>();
    let mut bytes = Vec::with_capacity(total);
    bytes.extend_from_slice(&MAGIC);
    bytes.extend_from_slice(&VERSION.to_le_bytes());
    bytes.extend_from_slice(&(sections.len() as u32).to_le_bytes());
    bytes.extend_from_slice(&key.0);
    let mut at = directory as u64;
    for (kind, count, body) in sections {
        bytes.extend_from_slice(&kind.to_le_bytes());
        bytes.extend_from_slice(&count.to_le_bytes());
        bytes.extend_from_slice(&at.to_le_bytes());
        bytes.extend_from_slice(&(body.len() as u64).to_le_bytes());
        at += body.len() as u64;
    }
    for (_, _, body) in sections {
        bytes.extend_from_slice(body);
    }
    debug_assert_eq!(bytes.len(), total);
    Ok(bytes)
}

/// Write `bytes` to `path` through a temporary file in its directory and a
/// rename, so a reader (or a mapping) sees the old image or the new one whole.
pub fn write_atomically(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let directory = path.parent().unwrap_or(Path::new("."));
    let name = path
        .file_name()
        .ok_or_else(|| std::io::Error::other("an image path names a file"))?;
    let temporary = directory.join(format!(
        ".{}.{}.{}.tmp",
        name.to_string_lossy(),
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed),
    ));
    // As tempfile's Unix create_named does, reserve the name exclusively.
    // Concurrent rebuilds must never truncate a temporary file another writer
    // may already have renamed into an image that readers have mapped.
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)?;
    let written = file.write_all(bytes).and_then(|()| file.sync_all());
    if let Err(e) = written.and_then(|()| std::fs::rename(&temporary, path)) {
        let _ = std::fs::remove_file(&temporary);
        return Err(e);
    }
    Ok(())
}

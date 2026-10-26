//! The canonical SER-2 codec over native layouts (runtime ABI 9; LIR schema revision 29).
//!
//! `hirsel_rt_v2_codec_encode` and `_decode` walk a compiler-emitted codec program
//! (`hirsel_rt_v2.h`, "The canonical SER-2 codec") that pairs each schema node with the layout
//! of its values. Both walks are iterative over runtime-owned work stacks, so no native stack
//! depth follows the data. Decoding validates the envelope and the whole payload before it
//! allocates anything, then builds fresh blocks in reading order; nothing partially decoded
//! exists. Error classes and their order are the reference codec's (`hirsel_types::wire`).
//! The reference's operational budgets (bytes, visited values) terminate environmentally.
pub mod access;
mod borrowed;
pub(crate) mod structural;
pub(crate) mod value_access;

use crate::alloc::{
    allocate_sequence, hirsel_rt_v2_alloc, payload, sequence_elements_offset, sequence_slice_start,
};
use crate::stop::{Stop, Stopped};
use crate::{DOMAIN_HEAP, Header, TaskCtx, fault};
use std::collections::BTreeSet;
use std::ffi::c_void;

/// `hirsel_rt_v2_codec`.
#[repr(C)]
pub struct Codec {
    pub descriptor: *const u8,
    pub descriptor_length: u64,
    pub program: *const u64,
    pub program_length: u64,
    /// The checks a decode's `CHECKED` nodes call (runtime ABI 23), `check_count` of them.
    pub checks: *const CheckThunk,
    pub check_count: u64,
}

/// `hirsel_rt_v2_check_thunk`: a checked opaque type's check (SER-1) on the borrowed value
/// at `value`, writing its verdict, 0 or 1, at `verdict`. Returns 0, or 1 with the check's
/// failure recorded.
pub type CheckThunk =
    unsafe extern "C" fn(ctx: *mut TaskCtx, value: *const u8, verdict: *mut u8) -> u8;

pub const CODEC_VERSION: u64 = 5;
pub const NODE_SCALAR: u64 = 0;
pub const NODE_TEXT: u64 = 1;
pub const NODE_FIELDS: u64 = 2;
pub const NODE_VARIANT: u64 = 3;
pub const NODE_BOX: u64 = 4;
pub const NODE_SEQUENCE: u64 = 5;
pub const NODE_REPEAT: u64 = 6;
pub const NODE_MAP: u64 = 7;
pub const NODE_SET: u64 = 8;
pub const NODE_FLAT_TEXT: u64 = 9;
pub const NODE_FRAMED: u64 = 10;
pub const NODE_FLAT_FIELDS: u64 = 11;
pub const NODE_FLAT_VARIANT: u64 = 12;
pub const NODE_FLAT_SEQUENCE: u64 = 13;
pub const NODE_SEQUENTIAL: u64 = 14;
pub const NODE_COLUMNS: u64 = 15;
pub const NODE_PATH: u64 = 16;
pub const NODE_CHECKED: u64 = 17;
pub const NO_NODE: u64 = u64::MAX;

const MAGIC: &[u8; 4] = b"HSC1";
const WIRE_VERSION: u32 = 1;
/// The reference codec's operational budgets (`hirsel_types::wire::Limits::default`).
const LIMIT_BYTES: u64 = 64 * 1024 * 1024;
const LIMIT_VALUES: u64 = 1_000_000;

/// `DecodeError`'s ordinals (core-library.md).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum DecodeError {
    Version = 0,
    SchemaMismatch = 1,
    Truncated = 2,
    Trailing = 3,
    Scalar = 4,
    Utf8 = 5,
    Tag = 6,
    Length = 7,
    Duplicate = 8,
    Order = 9,
    /// A checked opaque's check rejected a decoded representation (SER-1).
    Invalid = 10,
}

#[derive(Clone, Copy)]
struct Text {
    utf8: bool,
    owner: u64,
    start: u64,
    length: u64,
    index: u32,
}

#[derive(Clone, Copy)]
struct Elements {
    index: u32,
    align: u64,
    offset: u64,
    stride: u64,
}

#[derive(Clone, Copy)]
struct Container {
    set: bool,
    index: u32,
    size: u64,
    align: u64,
    entries_at: u64,
    index_at: u64,
    entries: Elements,
    index_index: u32,
    index_align: u64,
    index_stride: u64,
    key: usize,
    key_at: u64,
    value: Option<usize>,
    value_at: u64,
    map: Option<MapStorage>,
}
#[derive(Clone, Copy)]
struct MapStorage {
    hashes_at: u64,
    live_at: u64,
    tag_at: u64,
    tag_form: u64,
    tag_bit: u64,
}
impl MapStorage {
    unsafe fn live(self, at: *const u8) -> bool {
        unsafe {
            let tag = at.add(self.tag_at as usize);
            match self.tag_form {
                0 => tag.cast::<u32>().read_unaligned() == 1,
                1 => word(tag) & (1 << self.tag_bit) != 0,
                2 => word(tag) != 0,
                _ => fault("a map entry tag form"),
            }
        }
    }
    unsafe fn publish(self, at: *mut u8) {
        unsafe {
            let tag = at.add(self.tag_at as usize);
            match self.tag_form {
                0 => tag.cast::<u32>().write_unaligned(1),
                1 => tag.cast::<u64>().write_unaligned(1 << self.tag_bit),
                2 => {} // the decoded non-null payload supplies the niche tag
                _ => fault("a map entry tag form"),
            }
        }
    }
}

/// One parsed program node. Field lists are `(offset, node)` pairs.
enum Node {
    Scalar(Scalar),
    Text(Text),
    Fields(Vec<(u64, usize)>),
    Variant {
        niche: bool,
        tag: u64,
        nullary: u32,
        arms: Vec<Vec<(u64, usize)>>,
        extents: Vec<u64>,
    },
    Box {
        index: u32,
        size: u64,
        align: u64,
        payload: usize,
    },
    Sequence {
        elements: Elements,
        element: usize,
    },
    Repeat {
        count: u64,
        stride: u64,
        element: usize,
    },
    Container(Container),
    FlatText {
        utf8: bool,
    },
    Framed(usize),
    FlatFields(Vec<usize>),
    FlatVariant(Vec<Vec<usize>>),
    FlatSequence {
        offset: u64,
        stride: u64,
        element: usize,
    },
    Sequential {
        index: u32,
        align: u64,
        offset: u64,
        element: usize,
    },
    Columns {
        index: u32,
        align: u64,
        data: u64,
        quantum: u64,
        element: usize,
        columns: Vec<(u64, u64, usize)>,
    },
    /// A core Path's run-context identity, on the wire as its text.
    Path,
    /// A checked opaque type: its representation `value` at the same address, and the
    /// check a decode calls on it afterwards.
    Checked {
        check: usize,
        value: usize,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Scalar {
    Bool,
    Integer(usize),
    F32,
    F64,
    /// Four bytes holding a Unicode scalar value.
    Char,
}

struct Program {
    nodes: Vec<Node>,
    // Keep integer signedness for access-only value ordering. Wire bytes are unchanged.
    scalar_codes: Vec<u64>,
    out_size: u64,
    root: usize,
    out: usize,
    bytes: usize,
    /// The root layout's descriptor index, which a decode releases a value at when a
    /// check rejects it; None for a program without checked nodes.
    root_descriptor: Option<u32>,
}

/// A cursor over program words; a malformed program is an invariant fault.
struct Words<'a> {
    words: &'a [u64],
    at: usize,
}

impl Words<'_> {
    fn next(&mut self) -> u64 {
        let word = *self
            .words
            .get(self.at)
            .unwrap_or_else(|| fault("a codec program ends inside a node"));
        self.at += 1;
        word
    }
    fn node(&mut self, count: usize) -> usize {
        let node = self.next();
        if node >= count as u64 {
            fault("a codec program names a node outside it");
        }
        node as usize
    }
    fn index(&mut self) -> u32 {
        u32::try_from(self.next()).unwrap_or_else(|_| fault("a codec descriptor index overflows"))
    }
    fn fields(&mut self, count: usize) -> Vec<(u64, usize)> {
        let n = self.next();
        (0..n).map(|_| (self.next(), self.node(count))).collect()
    }
    fn elements(&mut self) -> Elements {
        Elements {
            index: self.index(),
            align: self.next(),
            offset: self.next(),
            stride: self.next(),
        }
    }
}

impl Program {
    fn parse(words: &[u64]) -> Program {
        Self::parse_mode(words, false)
    }

    fn parse_access(words: &[u64]) -> Program {
        Self::parse_mode(words, true)
    }

    fn parse_mode(words: &[u64], access: bool) -> Program {
        let mut w = Words { words, at: 0 };
        if w.next() != CODEC_VERSION {
            fault("a codec program of another version");
        }
        let count = usize::try_from(w.next()).unwrap_or_else(|_| fault("codec node count"));
        let out_size = w.next();
        let (root, out, bytes) = (w.node(count), w.node(count), w.node(count));
        let root_descriptor = match w.next() {
            NO_NODE => None,
            index => Some(
                u32::try_from(index)
                    .unwrap_or_else(|_| fault("a codec descriptor index overflows")),
            ),
        };
        let starts: Vec<u64> = (0..count).map(|_| w.next()).collect();
        let mut nodes = Vec::with_capacity(count);
        let mut scalar_codes = Vec::with_capacity(count);
        for start in starts {
            w.at = usize::try_from(start).unwrap_or_else(|_| fault("codec node offset"));
            let tag = w.next();
            scalar_codes.push(if tag == NODE_SCALAR {
                w.words[w.at]
            } else {
                u64::MAX
            });
            let node = match tag {
                NODE_SCALAR => Node::Scalar(match w.next() {
                    0 => Scalar::Bool,
                    1 | 6 => Scalar::Integer(1),
                    2 | 7 => Scalar::Integer(2),
                    3 | 8 => Scalar::Integer(4),
                    4 | 9 => Scalar::Integer(8),
                    5 | 10 => Scalar::Integer(16),
                    11 => Scalar::F32,
                    12 => Scalar::F64,
                    13 => Scalar::Char,
                    _ => fault("an unknown codec scalar"),
                }),
                NODE_TEXT => Node::Text(Text {
                    utf8: w.next() != 0,
                    owner: w.next(),
                    start: w.next(),
                    length: w.next(),
                    index: w.index(),
                }),
                NODE_FIELDS => Node::Fields(w.fields(count)),
                NODE_VARIANT => {
                    let niche = match w.next() {
                        0 => false,
                        1 => true,
                        _ => fault("an unknown codec tag form"),
                    };
                    let tag = w.next();
                    let nullary = u32::try_from(w.next()).unwrap_or_else(|_| fault("niche arm"));
                    let n = w.next();
                    let arms = (0..n).map(|_| w.fields(count)).collect::<Vec<_>>();
                    let extents = (0..n).map(|_| w.next()).collect();
                    if niche && (arms.len() != 2 || nullary > 1) {
                        fault("a codec niche that is not two arms");
                    }
                    Node::Variant {
                        niche,
                        tag,
                        nullary,
                        arms,
                        extents,
                    }
                }
                NODE_BOX => Node::Box {
                    index: w.index(),
                    size: w.next(),
                    align: w.next(),
                    payload: w.node(count),
                },
                NODE_SEQUENCE => Node::Sequence {
                    elements: w.elements(),
                    element: w.node(count),
                },
                NODE_REPEAT => Node::Repeat {
                    count: w.next(),
                    stride: w.next(),
                    element: w.node(count),
                },
                kind @ (NODE_MAP | NODE_SET) => {
                    let (index, size, align, entries_at, index_at) =
                        (w.index(), w.next(), w.next(), w.next(), w.next());
                    let entries = w.elements();
                    let (index_index, index_align, index_stride) = (w.index(), w.next(), w.next());
                    let key = w.node(count);
                    let key_at = w.next();
                    let value = match w.next() {
                        NO_NODE => None,
                        node if node < count as u64 => Some(node as usize),
                        _ => fault("a codec program names a node outside it"),
                    };
                    let value_at = w.next();
                    if (kind == NODE_SET) != value.is_none() {
                        fault("a codec container whose entries disagree with its kind");
                    }
                    let map = Some(MapStorage {
                        hashes_at: w.next(),
                        live_at: w.next(),
                        tag_at: w.next(),
                        tag_form: w.next(),
                        tag_bit: w.next(),
                    });
                    Node::Container(Container {
                        map,
                        set: kind == NODE_SET,
                        index,
                        size,
                        align,
                        entries_at,
                        index_at,
                        entries,
                        index_index,
                        index_align,
                        index_stride,
                        key,
                        key_at,
                        value,
                        value_at,
                    })
                }
                NODE_FLAT_TEXT => Node::FlatText {
                    utf8: w.next() != 0,
                },
                NODE_FRAMED => Node::Framed(w.node(count)),
                NODE_FLAT_FIELDS => {
                    let n = w.next();
                    Node::FlatFields((0..n).map(|_| w.node(count)).collect())
                }
                NODE_FLAT_VARIANT => {
                    let n = w.next();
                    Node::FlatVariant(
                        (0..n)
                            .map(|_| {
                                let fields = w.next();
                                (0..fields).map(|_| w.node(count)).collect()
                            })
                            .collect(),
                    )
                }
                NODE_FLAT_SEQUENCE => Node::FlatSequence {
                    offset: w.next(),
                    stride: w.next(),
                    element: w.node(count),
                },
                NODE_SEQUENTIAL => Node::Sequential {
                    index: w.index(),
                    align: w.next(),
                    offset: w.next(),
                    element: w.node(count),
                },
                NODE_COLUMNS => {
                    let (index, align, data, quantum, element, n) = (
                        w.index(),
                        w.next(),
                        w.next(),
                        w.next(),
                        w.node(count),
                        w.next(),
                    );
                    Node::Columns {
                        index,
                        align,
                        data,
                        quantum,
                        element,
                        columns: (0..n)
                            .map(|_| (w.next(), w.next(), w.node(count)))
                            .collect(),
                    }
                }
                NODE_PATH => Node::Path,
                NODE_CHECKED => Node::Checked {
                    check: usize::try_from(w.next())
                        .unwrap_or_else(|_| fault("a codec check index overflows")),
                    value: w.node(count),
                },
                _ => fault("an unknown codec node kind"),
            };
            nodes.push(node);
        }
        let checked = nodes.iter().any(|n| matches!(n, Node::Checked { .. }));
        if !access && checked != root_descriptor.is_some() {
            fault("a codec program whose checks and root descriptor disagree");
        }
        let program = Program {
            nodes,
            scalar_codes,
            out_size,
            root,
            out,
            bytes,
            root_descriptor,
        };
        if !access
            && !matches!(
                program.nodes[program.bytes],
                Node::Text(Text { utf8: false, .. })
            )
        {
            fault("a codec program whose bytes node is not Bytes");
        }
        program
    }

    /// A variant node's arm `ordinal`, after writing its tag at `at`.
    unsafe fn write_arm(&self, node: usize, at: *mut u8, ordinal: u32) -> &[(u64, usize)] {
        let Node::Variant {
            niche,
            tag,
            nullary,
            arms,
            ..
        } = &self.nodes[node]
        else {
            fault("a codec arm of a node that is not a variant");
        };
        let arm = arms
            .get(ordinal as usize)
            .unwrap_or_else(|| fault("a codec arm outside its variant"));
        // A niche's nullary arm is the null word the zeroed value already holds;
        // its dataful arm's first field writes the pointer.
        if !niche {
            unsafe { write(at.add(*tag as usize), &ordinal.to_le_bytes()) };
        } else if ordinal != *nullary && arm.is_empty() {
            fault("a codec niche whose dataful arm has no field");
        }
        arm
    }
}

unsafe fn program<'a>(codec: *const Codec) -> (Program, &'a [u8], &'a [CheckThunk]) {
    unsafe {
        let codec = &*codec;
        let words = std::slice::from_raw_parts(codec.program, codec.program_length as usize);
        let descriptor =
            std::slice::from_raw_parts(codec.descriptor, codec.descriptor_length as usize);
        let checks = if codec.check_count == 0 {
            &[]
        } else {
            std::slice::from_raw_parts(codec.checks, codec.check_count as usize)
        };
        (Program::parse(words), descriptor, checks)
    }
}

unsafe fn read(at: *const u8, n: usize) -> &'static [u8] {
    unsafe { std::slice::from_raw_parts(at, n) }
}
unsafe fn write(at: *mut u8, bytes: &[u8]) {
    unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), at, bytes.len()) }
}
unsafe fn word(at: *const u8) -> u64 {
    unsafe { at.cast::<u64>().read_unaligned() }
}
unsafe fn pointer(at: *const u8) -> *mut Header {
    unsafe { at.cast::<*mut Header>().read_unaligned() }
}
unsafe fn set_pointer(at: *mut u8, block: *mut Header) {
    unsafe { at.cast::<*mut Header>().write_unaligned(block) }
}

/// The bytes a String or Bytes slice at `at` shows.
unsafe fn text_bytes<'a>(text: &Text, at: *const u8) -> &'a [u8] {
    unsafe {
        let owner = pointer(at.add(text.owner as usize));
        let start = word(at.add(text.start as usize));
        let length = word(at.add(text.length as usize));
        if start
            .checked_add(length)
            .is_none_or(|end| end > isize::MAX as u64)
        {
            fault("a codec slice range overflows");
        }
        std::slice::from_raw_parts(payload(owner).add(8 + start as usize), length as usize)
    }
}

/// A fresh byte sequence holding `bytes`, written as a whole slice at `at`.
unsafe fn write_text(ctx: *mut TaskCtx, text: &Text, at: *mut u8, bytes: &[u8]) {
    unsafe {
        let owner = allocate_sequence(ctx, text.index, bytes.len() as u64, 1, 1);
        write(
            payload(owner).add(sequence_elements_offset(1) as usize),
            bytes,
        );
        set_pointer(at.add(text.owner as usize), owner);
        write(
            at.add(text.start as usize),
            &sequence_slice_start(1).to_le_bytes(),
        );
        write(
            at.add(text.length as usize),
            &(bytes.len() as u64).to_le_bytes(),
        );
    }
}

/// The stop past `hirsel_types::wire::Limits::default`'s byte or value budget: the run ends
/// environmentally, as the interpreter's operational limit ends it.
fn budget() -> Stop {
    Stop::Environmental(hirsel_run_schema::Environmental::CodecLimit)
}

/// Why a payload does not decode: its first `DecodeError`, or the operational budget.
enum Invalid {
    Decode(DecodeError),
    Budget,
}
impl From<DecodeError> for Invalid {
    fn from(error: DecodeError) -> Self {
        Invalid::Decode(error)
    }
}

fn nan32(bits: u32) -> bool {
    bits & 0x7fff_ffff > 0x7f80_0000
}
fn nan64(bits: u64) -> bool {
    bits & 0x7fff_ffff_ffff_ffff > 0x7ff0_0000_0000_0000
}

/// The active native extent of a pointer-free flat value. Framed fields make
/// variable children self delimiting; no maximum Packed storage enters a walk.
unsafe fn flat_extent(program: &Program, node: usize, at: *const u8) -> u64 {
    let mut extent = 0u64;
    let mut work = vec![node];
    while let Some(node) = work.pop() {
        let cursor = at.wrapping_add(extent as usize);
        let n = unsafe {
            match &program.nodes[node] {
                Node::Scalar(Scalar::Bool) => 1,
                Node::Scalar(Scalar::Integer(n)) => *n as u64,
                Node::Scalar(Scalar::F32 | Scalar::Char) => 4,
                Node::Scalar(Scalar::F64) => 8,
                Node::FlatText { .. } | Node::Framed(_) | Node::FlatSequence { .. } => word(cursor)
                    .checked_add(8)
                    .unwrap_or_else(|| fault("flat extent overflow")),
                Node::FlatFields(fields) => {
                    work.extend(fields.iter().rev());
                    0
                }
                Node::Fields(fields) => {
                    work.extend(fields.iter().rev().map(|&(_, child)| child));
                    0
                }
                Node::FlatVariant(arms) => {
                    let ordinal = cursor.cast::<u32>().read_unaligned() as usize;
                    let arm = arms
                        .get(ordinal)
                        .unwrap_or_else(|| fault("flat extent tag"));
                    work.extend(arm.iter().rev());
                    4
                }
                Node::Variant { tag, extents, .. } => {
                    let ordinal = cursor.add(*tag as usize).cast::<u32>().read_unaligned() as usize;
                    *extents
                        .get(ordinal)
                        .unwrap_or_else(|| fault("Packed extent tag"))
                }
                Node::Repeat { count, stride, .. } => count
                    .checked_mul(*stride)
                    .unwrap_or_else(|| fault("flat repeat extent")),
                Node::Checked { value, .. } => {
                    work.push(*value);
                    0
                }
                _ => fault("a non-flat codec node in a flat extent"),
            }
        };
        extent = extent
            .checked_add(n)
            .unwrap_or_else(|| fault("flat extent overflow"));
    }
    extent
}

unsafe fn flat_fields(program: &Program, fields: &[usize], at: *const u8, work: &mut Vec<Put>) {
    let mut cursor = at;
    let mut children = Vec::with_capacity(fields.len());
    for &child in fields {
        children.push(Put::Visit(child, cursor));
        cursor = cursor.wrapping_add(unsafe { flat_extent(program, child, cursor) } as usize);
    }
    work.extend(children.into_iter().rev());
}

/// Rebuild pointer-free ordinary Packed storage, respecting field offsets,
/// native padding and its tag-selected active extent. This is separate from
/// FlatFormatV0: only scalar leaves are copied from the validated wire.
fn inline_decode(program: &Program, root: usize, input: &mut Input<'_>) -> Vec<u8> {
    let mut out = Vec::new();
    let mut work = vec![(root, 0usize)];
    let checked = |r: Result<u128, DecodeError>| r.unwrap_or_else(|_| fault("Packed codec reread"));
    while let Some((node, at)) = work.pop() {
        match &program.nodes[node] {
            Node::Scalar(scalar) => {
                let n = match scalar {
                    Scalar::Bool => 1,
                    Scalar::Integer(n) => *n,
                    Scalar::F32 | Scalar::Char => 4,
                    Scalar::F64 => 8,
                };
                out.resize(out.len().max(at + n), 0);
                out[at..at + n].copy_from_slice(&checked(input.number(n)).to_le_bytes()[..n]);
            }
            Node::Fields(fields) => work.extend(
                fields
                    .iter()
                    .rev()
                    .map(|&(offset, child)| (child, at + offset as usize)),
            ),
            Node::Checked { value, .. } => work.push((*value, at)),
            Node::Variant {
                niche,
                tag,
                arms,
                extents,
                ..
            } => {
                if *niche {
                    fault("a pointer niche in a pointer-free Packed element");
                }
                let ordinal = checked(input.number(4)) as u32;
                let extent = extents[ordinal as usize] as usize;
                out.resize(out.len().max(at + extent), 0);
                let tag = at + *tag as usize;
                out[tag..tag + 4].copy_from_slice(&ordinal.to_le_bytes());
                work.extend(
                    arms[ordinal as usize]
                        .iter()
                        .rev()
                        .map(|&(offset, child)| (child, at + offset as usize)),
                );
            }
            Node::Repeat {
                count,
                stride,
                element,
            } => work.extend(
                (0..*count)
                    .rev()
                    .map(|i| (*element, at + (i * stride) as usize)),
            ),
            Node::FlatFields(_)
            | Node::FlatVariant(_)
            | Node::Framed(_)
            | Node::FlatText { .. }
            | Node::FlatSequence { .. } => {
                let bytes = flat_decode(program, node, input);
                out.resize(out.len().max(at + bytes.len()), 0);
                out[at..at + bytes.len()].copy_from_slice(&bytes);
            }
            _ => fault("a reference-bearing node in a pointer-free Packed element"),
        }
    }
    out
}

/// Transcode validated wire data into private FlatFormatV0 framing. The work
/// stack, prefix patches and output are runtime owned, including for nested lists.
fn flat_decode(program: &Program, root: usize, input: &mut Input<'_>) -> Vec<u8> {
    enum Job {
        Visit(usize),
        Elements(usize, u64),
        Prefix(usize),
    }
    let mut work = vec![Job::Visit(root)];
    let mut out = Vec::new();
    let checked = |r: Result<u128, DecodeError>| r.unwrap_or_else(|_| fault("flat codec reread"));
    while let Some(job) = work.pop() {
        let node = match job {
            Job::Visit(node) => node,
            Job::Prefix(at) => {
                let length = (out.len() - at - 8) as u64;
                out[at..at + 8].copy_from_slice(&length.to_le_bytes());
                continue;
            }
            Job::Elements(node, count) => {
                if count > 0 {
                    work.push(Job::Elements(node, count - 1));
                    work.push(Job::Visit(node));
                }
                continue;
            }
        };
        match &program.nodes[node] {
            Node::Scalar(scalar) => {
                let n = match scalar {
                    Scalar::Bool => 1,
                    Scalar::Integer(n) => *n,
                    Scalar::F32 | Scalar::Char => 4,
                    Scalar::F64 => 8,
                };
                out.extend_from_slice(&checked(input.number(n)).to_le_bytes()[..n]);
            }
            Node::FlatText { .. } => {
                let n = checked(input.number(8)) as u64;
                out.extend_from_slice(&n.to_le_bytes());
                out.extend_from_slice(input.take(n).unwrap_or_else(|_| fault("flat text reread")));
            }
            Node::Framed(child) => {
                work.push(Job::Prefix(out.len()));
                out.extend_from_slice(&[0; 8]);
                work.push(Job::Visit(*child));
            }
            Node::FlatFields(fields) => work.extend(fields.iter().rev().map(|&f| Job::Visit(f))),
            // Zero-size elements keep their ordinary layout identity: their
            // fields have no packing offsets or bytes to reconstruct.
            Node::Fields(fields) => work.extend(fields.iter().rev().map(|&(_, f)| Job::Visit(f))),
            Node::Checked { value, .. } => work.push(Job::Visit(*value)),
            Node::FlatVariant(arms) => {
                let ordinal = checked(input.number(4)) as u32;
                out.extend_from_slice(&ordinal.to_le_bytes());
                let arm = arms
                    .get(ordinal as usize)
                    .unwrap_or_else(|| fault("flat variant reread"));
                work.extend(arm.iter().rev().map(|&f| Job::Visit(f)));
            }
            Node::FlatSequence {
                offset,
                stride,
                element,
            } => {
                let count = checked(input.number(8)) as u64;
                work.push(Job::Prefix(out.len()));
                out.extend_from_slice(&[0; 8]);
                if *offset == 16 {
                    out.extend_from_slice(&count.to_le_bytes());
                }
                work.push(Job::Elements(*element, count));
            }
            Node::Repeat { count, element, .. } => work.push(Job::Elements(*element, *count)),
            _ => fault("a non-flat codec node in flat reconstruction"),
        }
    }
    out
}

// ----- Encoding --------------------------------------------------------------------------

enum Put {
    Visit(usize, *const u8),
    /// The next `count` elements of a sequence, `stride` apart from `at`.
    Elements {
        node: usize,
        at: *const u8,
        stride: u64,
        count: u64,
        set: bool,
    },
    Sequential {
        node: usize,
        at: *const u8,
        count: u64,
    },
    Rows {
        columns: Vec<(*const u8, u64, usize)>,
        row: u64,
        count: u64,
    },
    /// The next `count` map entries from `at`.
    Entries {
        container: Container,
        at: *const u8,
        count: u64,
    },
    /// The start of one element of the innermost set.
    Mark,
    /// The end of the innermost set: sort its elements' encodings.
    SetEnd,
    /// A decode's check pass: call check `check` on the checked value at `at`, after
    /// the value's own inner occurrences.
    Check(usize, *const u8),
}

/// The SER-2 payload of the value at `value`. With `checks`, a decode's check pass over
/// the value it reconstructed: it also calls each checked node's check, inner occurrences
/// first and otherwise in encoding order, and is None at the first rejection (SER-1).
unsafe fn encode_payload(
    ctx: *mut TaskCtx,
    program: &Program,
    value: *const u8,
    checks: Option<&[CheckThunk]>,
) -> Stopped<Option<Vec<u8>>> {
    let mut out = Vec::new();
    let mut work = vec![Put::Visit(program.root, value)];
    // Open sets: where the elements begin, and each element's start.
    let mut sets: Vec<(usize, Vec<usize>)> = Vec::new();
    let mut values = 0u64;
    while let Some(job) = work.pop() {
        if out.len() as u64 > LIMIT_BYTES {
            return Err(budget());
        }
        match job {
            Put::Check(check, at) => {
                let thunk = checks
                    .and_then(|checks| checks.get(check))
                    .unwrap_or_else(|| fault("a codec check outside the codec's checks"));
                let mut verdict = 0u8;
                // SAFETY: the check borrows the reconstructed value at `at`, of its layout.
                if unsafe { crate::depth::enter_callback(ctx) } != crate::STATUS_OK {
                    return Err(Stop::Failed);
                }
                // SAFETY: successful admission retains the borrowed occurrence;
                // the thunk returns only after its ordinary cleanup.
                let status = unsafe { thunk(ctx, at, &mut verdict) };
                // SAFETY: this check's successful entry retires exactly once.
                unsafe {
                    crate::depth::hirsel_rt_v2_depth_leave(ctx);
                }
                match (status, verdict) {
                    (crate::STATUS_OK, 1) => (),
                    (crate::STATUS_OK, 0) => return Ok(None),
                    (crate::STATUS_OK, _) => fault("a codec check verdict that is not a bool"),
                    // FAIL-3: the codec cannot unwind, so the check's failure ends the run.
                    _ => return Err(Stop::Failed),
                }
            }
            Put::Mark => sets
                .last_mut()
                .unwrap_or_else(|| fault("a codec set mark outside a set"))
                .1
                .push(out.len()),
            Put::SetEnd => {
                let (start, mut starts) = sets
                    .pop()
                    .unwrap_or_else(|| fault("a codec set end outside a set"));
                starts.push(out.len());
                let mut elements: Vec<Vec<u8>> = starts
                    .windows(2)
                    .map(|w| out[w[0]..w[1]].to_vec())
                    .collect();
                elements.sort_unstable();
                out.truncate(start);
                elements.iter().for_each(|e| out.extend_from_slice(e));
            }
            Put::Elements {
                node,
                at,
                stride,
                count,
                set,
            } => {
                if count == 0 {
                    continue;
                }
                work.push(Put::Elements {
                    node,
                    at: at.wrapping_add(stride as usize),
                    stride,
                    count: count - 1,
                    set,
                });
                work.push(Put::Visit(node, at));
                if set {
                    work.push(Put::Mark);
                }
            }
            Put::Sequential { node, at, count } => {
                if count > 0 {
                    let extent = unsafe { flat_extent(program, node, at) };
                    work.push(Put::Sequential {
                        node,
                        at: at.wrapping_add(extent as usize),
                        count: count - 1,
                    });
                    work.push(Put::Visit(node, at));
                }
            }
            Put::Rows {
                columns,
                row,
                count,
            } => {
                if row < count {
                    values += 1;
                    if values > LIMIT_VALUES {
                        return Err(budget());
                    }
                    let fields: Vec<_> = columns
                        .iter()
                        .map(|&(at, stride, child)| {
                            Put::Visit(child, at.wrapping_add((row * stride) as usize))
                        })
                        .collect();
                    work.push(Put::Rows {
                        columns,
                        row: row + 1,
                        count,
                    });
                    work.extend(fields.into_iter().rev());
                }
            }
            Put::Entries {
                container,
                at,
                count,
            } => {
                if count == 0 {
                    continue;
                }
                work.push(Put::Entries {
                    container,
                    at: at.wrapping_add(container.entries.stride as usize),
                    count: count - 1,
                });
                if !unsafe { container.map.expect("map storage").live(at) } {
                    continue;
                }
                if let Some(value) = container.value {
                    work.push(Put::Visit(
                        value,
                        at.wrapping_add(container.value_at as usize),
                    ));
                }
                work.push(Put::Visit(
                    container.key,
                    at.wrapping_add(container.key_at as usize),
                ));
                if container.set {
                    work.push(Put::Mark);
                }
            }
            Put::Visit(node, at) => {
                values += 1;
                if values > LIMIT_VALUES {
                    return Err(budget());
                }
                unsafe {
                    encode_node(
                        ctx,
                        program,
                        node,
                        at,
                        &mut out,
                        &mut work,
                        &mut sets,
                        checks.is_some(),
                    )
                };
            }
        }
    }
    if out.len() as u64 > LIMIT_BYTES {
        return Err(budget());
    }
    Ok(Some(out))
}

unsafe fn encode_node(
    ctx: *mut TaskCtx,
    program: &Program,
    node: usize,
    at: *const u8,
    out: &mut Vec<u8>,
    work: &mut Vec<Put>,
    sets: &mut Vec<(usize, Vec<usize>)>,
    checking: bool,
) {
    unsafe {
        match &program.nodes[node] {
            Node::Checked { check, value } => {
                if checking {
                    work.push(Put::Check(*check, at));
                }
                work.push(Put::Visit(*value, at));
            }
            Node::Scalar(Scalar::Bool) => out.push(u8::from(*at != 0)),
            Node::Scalar(Scalar::Integer(width)) => out.extend_from_slice(read(at, *width)),
            Node::Scalar(Scalar::Char) => out.extend_from_slice(read(at, 4)),
            Node::Scalar(Scalar::F32) => {
                let bits = at.cast::<u32>().read_unaligned();
                let bits = if nan32(bits) { 0x7fc0_0000 } else { bits };
                out.extend_from_slice(&bits.to_le_bytes());
            }
            Node::Scalar(Scalar::F64) => {
                let bits = at.cast::<u64>().read_unaligned();
                let bits = if nan64(bits) {
                    0x7ff8_0000_0000_0000
                } else {
                    bits
                };
                out.extend_from_slice(&bits.to_le_bytes());
            }
            Node::Text(text) => {
                let bytes = text_bytes(text, at);
                out.extend_from_slice(&(bytes.len() as u64).to_le_bytes());
                out.extend_from_slice(bytes);
            }
            Node::Path => {
                let text = crate::paths::names(ctx).path_text(word(at));
                out.extend_from_slice(&(text.len() as u64).to_le_bytes());
                out.extend_from_slice(text.as_bytes());
            }
            Node::Fields(fields) => {
                for &(offset, child) in fields.iter().rev() {
                    work.push(Put::Visit(child, at.add(offset as usize)));
                }
            }
            Node::Variant {
                niche,
                tag,
                nullary,
                arms,
                ..
            } => {
                let ordinal = if *niche {
                    if word(at.add(*tag as usize)) == 0 {
                        *nullary
                    } else {
                        1 - *nullary
                    }
                } else {
                    at.add(*tag as usize).cast::<u32>().read_unaligned()
                };
                let arm = arms
                    .get(ordinal as usize)
                    .unwrap_or_else(|| fault("a variant value outside its arms"));
                out.extend_from_slice(&ordinal.to_le_bytes());
                for &(offset, child) in arm.iter().rev() {
                    work.push(Put::Visit(child, at.add(offset as usize)));
                }
            }
            Node::Box { payload: p, .. } => {
                work.push(Put::Visit(*p, payload(pointer(at))));
            }
            Node::Sequence { elements, element } => {
                let block = payload(pointer(at));
                let count = word(block);
                out.extend_from_slice(&count.to_le_bytes());
                work.push(Put::Elements {
                    node: *element,
                    at: block.add(elements.offset as usize),
                    stride: elements.stride,
                    count,
                    set: false,
                });
            }
            Node::Repeat {
                count,
                stride,
                element,
            } => work.push(Put::Elements {
                node: *element,
                at,
                stride: *stride,
                count: *count,
                set: false,
            }),
            Node::FlatText { .. } => {
                let length = word(at);
                out.extend_from_slice(&length.to_le_bytes());
                out.extend_from_slice(read(at.add(8), length as usize));
            }
            Node::Framed(child) => work.push(Put::Visit(*child, at.add(8))),
            Node::FlatFields(fields) => flat_fields(program, fields, at, work),
            Node::FlatVariant(arms) => {
                let ordinal = at.cast::<u32>().read_unaligned();
                out.extend_from_slice(&ordinal.to_le_bytes());
                let fields = arms
                    .get(ordinal as usize)
                    .unwrap_or_else(|| fault("flat variant ordinal"));
                flat_fields(program, fields, at.add(4), work);
            }
            Node::FlatSequence {
                offset,
                stride,
                element,
            } => {
                let count = if *offset == 16 {
                    word(at.add(8))
                } else {
                    if *stride == 0 {
                        fault("flat sequence without a count");
                    }
                    word(at) / *stride
                };
                out.extend_from_slice(&count.to_le_bytes());
                work.push(Put::Sequential {
                    node: *element,
                    at: at.add(*offset as usize),
                    count,
                });
            }
            Node::Sequential {
                offset, element, ..
            } => {
                let p = payload(pointer(at));
                let count = word(p);
                out.extend_from_slice(&count.to_le_bytes());
                work.push(Put::Sequential {
                    node: *element,
                    at: p.add(*offset as usize),
                    count,
                });
            }
            Node::Columns { data, columns, .. } => {
                let p = payload(pointer(at));
                let (count, capacity) = (word(p), word(p.add(8)));
                out.extend_from_slice(&count.to_le_bytes());
                let columns = columns
                    .iter()
                    .map(|&(prefix, stride, child)| {
                        (
                            p.add((*data + capacity * prefix) as usize).cast_const(),
                            stride,
                            child,
                        )
                    })
                    .collect();
                work.push(Put::Rows {
                    columns,
                    row: 0,
                    count,
                });
            }
            Node::Container(container) => {
                let container_at = payload(pointer(at));
                let entries = payload(pointer(container_at.add(container.entries_at as usize)));
                let count = word(entries);
                let live = container
                    .map
                    .map_or(count, |map| word(container_at.add(map.live_at as usize)));
                out.extend_from_slice(&live.to_le_bytes());
                let first = entries.add(container.entries.offset as usize);
                if container.set {
                    sets.push((out.len(), Vec::new()));
                    work.push(Put::SetEnd);
                }
                work.push(Put::Entries {
                    container: *container,
                    at: first,
                    count,
                });
            }
        }
    }
}

/// `codec.encode` (LIR schema revision 29): write a fresh Bytes slice of the SER-2 envelope of
/// the value at `value` at `out`. It cannot fail; past the reference's operational budgets the
/// run stops environmentally.
///
/// # Safety
/// `ctx` is a live run context, `codec` a compiler-emitted codec, `value` a live value of its
/// root node's layout and `out` writable storage of its Bytes layout.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_codec_encode(
    ctx: *mut TaskCtx,
    codec: *const Codec,
    value: *const c_void,
    out: *mut c_void,
) {
    // SAFETY: caller contract; the encoding's own storage is dropped before a stop.
    unsafe { crate::stop::check(ctx, encode(ctx, codec, value, out)) }
}

unsafe fn encode(
    ctx: *mut TaskCtx,
    codec: *const Codec,
    value: *const c_void,
    out: *mut c_void,
) -> Stopped<()> {
    unsafe {
        let (program, descriptor, _) = program(codec);
        let payload = encode_payload(ctx, &program, value.cast(), None)?
            .unwrap_or_else(|| fault("an encoding rejected by a check"));
        let mut envelope = Vec::with_capacity(24 + descriptor.len() + payload.len());
        envelope.extend_from_slice(MAGIC);
        envelope.extend_from_slice(&WIRE_VERSION.to_le_bytes());
        envelope.extend_from_slice(&(descriptor.len() as u64).to_le_bytes());
        envelope.extend_from_slice(descriptor);
        envelope.extend_from_slice(&(payload.len() as u64).to_le_bytes());
        envelope.extend_from_slice(&payload);
        if envelope.len() as u64 > LIMIT_BYTES {
            return Err(budget());
        }
        let Node::Text(text) = &program.nodes[program.bytes] else {
            unreachable!("checked at parse")
        };
        std::ptr::write_bytes(out.cast::<u8>(), 0, program.out_size as usize);
        write_text(ctx, text, out.cast(), &envelope);
    }
    Ok(())
}

// ----- Decoding --------------------------------------------------------------------------

struct Input<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Input<'a> {
    fn take(&mut self, n: u64) -> Result<&'a [u8], DecodeError> {
        let end = (self.at as u64).checked_add(n).ok_or(DecodeError::Length)?;
        if end > self.bytes.len() as u64 {
            return Err(DecodeError::Truncated);
        }
        let bytes = &self.bytes[self.at..end as usize];
        self.at = end as usize;
        Ok(bytes)
    }
    fn number(&mut self, n: usize) -> Result<u128, DecodeError> {
        let mut bytes = [0; 16];
        bytes[..n].copy_from_slice(self.take(n as u64)?);
        Ok(u128::from_le_bytes(bytes))
    }
    fn length(&mut self) -> Result<u64, DecodeError> {
        Ok(self.number(8)? as u64)
    }
}

/// The payload of a complete envelope whose descriptor is `descriptor`.
fn envelope<'a>(bytes: &'a [u8], descriptor: &[u8]) -> Result<&'a [u8], DecodeError> {
    let mut input = Input { bytes, at: 0 };
    if input.take(4)? != MAGIC || input.number(4)? != u128::from(WIRE_VERSION) {
        return Err(DecodeError::Version);
    }
    let length = input.length()?;
    if input.take(length)? != descriptor {
        return Err(DecodeError::SchemaMismatch);
    }
    let length = input.length()?;
    let payload = input.take(length)?;
    if input.at != bytes.len() {
        return Err(DecodeError::Trailing);
    }
    Ok(payload)
}

enum Check {
    Visit(usize),
    /// The next `count` elements of a sequence or set (`key`: each one a set key).
    Elements {
        node: usize,
        count: u64,
        key: bool,
    },
    /// The next `count` map entries.
    Entries {
        key: usize,
        value: usize,
        count: u64,
    },
    KeyEnd {
        at: usize,
        set: bool,
    },
    CollectionStart,
    CollectionEnd,
}

#[derive(Default)]
struct Collection<'a> {
    keys: BTreeSet<&'a [u8]>,
    previous: Option<&'a [u8]>,
}

/// Validate a whole payload against the program, in the reference's reading order.
fn validate(program: &Program, bytes: &[u8]) -> Result<(), Invalid> {
    let mut input = Input { bytes, at: 0 };
    let mut work = vec![Check::Visit(program.root)];
    let mut collections: Vec<Collection<'_>> = Vec::new();
    let mut values = 0u64;
    let reserve = |values: u64, n: u64| {
        if n > LIMIT_VALUES.saturating_sub(values) {
            Err(Invalid::Budget)
        } else {
            Ok(())
        }
    };
    while let Some(job) = work.pop() {
        match job {
            Check::CollectionStart => collections.push(Collection::default()),
            Check::CollectionEnd => {
                collections.pop();
            }
            Check::KeyEnd { at, set } => {
                let key = &input.bytes[at..input.at];
                let collection = collections
                    .last_mut()
                    .unwrap_or_else(|| fault("a codec key outside a collection"));
                if set {
                    if let Some(previous) = collection.previous {
                        if previous == key {
                            return Err(DecodeError::Duplicate.into());
                        }
                        if previous > key {
                            return Err(DecodeError::Order.into());
                        }
                    }
                    collection.previous = Some(key);
                } else if !collection.keys.insert(key) {
                    return Err(DecodeError::Duplicate.into());
                }
            }
            Check::Elements { node, count, key } => {
                if count == 0 {
                    continue;
                }
                work.push(Check::Elements {
                    node,
                    count: count - 1,
                    key,
                });
                if key {
                    work.push(Check::KeyEnd {
                        at: input.at,
                        set: true,
                    });
                }
                work.push(Check::Visit(node));
            }
            Check::Entries { key, value, count } => {
                if count == 0 {
                    continue;
                }
                work.push(Check::Entries {
                    key,
                    value,
                    count: count - 1,
                });
                work.push(Check::Visit(value));
                work.push(Check::KeyEnd {
                    at: input.at,
                    set: false,
                });
                work.push(Check::Visit(key));
            }
            Check::Visit(node) => {
                values += 1;
                if values > LIMIT_VALUES {
                    return Err(Invalid::Budget);
                }
                match &program.nodes[node] {
                    Node::Scalar(Scalar::Bool) => {
                        if input.number(1)? > 1 {
                            return Err(DecodeError::Scalar.into());
                        }
                    }
                    Node::Scalar(Scalar::Integer(width)) => {
                        input.take(*width as u64)?;
                    }
                    Node::Scalar(Scalar::F32) => {
                        let bits = input.number(4)? as u32;
                        if nan32(bits) && bits != 0x7fc0_0000 {
                            return Err(DecodeError::Scalar.into());
                        }
                    }
                    Node::Scalar(Scalar::Char) => {
                        if char::from_u32(input.number(4)? as u32).is_none() {
                            return Err(DecodeError::Scalar.into());
                        }
                    }
                    Node::Scalar(Scalar::F64) => {
                        let bits = input.number(8)? as u64;
                        if nan64(bits) && bits != 0x7ff8_0000_0000_0000 {
                            return Err(DecodeError::Scalar.into());
                        }
                    }
                    Node::Text(Text { utf8, .. }) | Node::FlatText { utf8 } => {
                        let n = input.length()?;
                        let bytes = input.take(n)?;
                        if *utf8 && std::str::from_utf8(bytes).is_err() {
                            return Err(DecodeError::Utf8.into());
                        }
                    }
                    // `path.of` admits any text.
                    Node::Path => {
                        let n = input.length()?;
                        if std::str::from_utf8(input.take(n)?).is_err() {
                            return Err(DecodeError::Utf8.into());
                        }
                    }
                    Node::Fields(fields) => {
                        work.extend(fields.iter().rev().map(|&(_, f)| Check::Visit(f)));
                    }
                    Node::Variant { arms, .. } => {
                        let tag = input.number(4)? as u32;
                        let arm = arms.get(tag as usize).ok_or(DecodeError::Tag)?;
                        work.extend(arm.iter().rev().map(|&(_, f)| Check::Visit(f)));
                    }
                    Node::Framed(child) | Node::Checked { value: child, .. } => {
                        work.push(Check::Visit(*child))
                    }
                    Node::FlatFields(fields) => {
                        work.extend(fields.iter().rev().map(|&f| Check::Visit(f)))
                    }
                    Node::FlatVariant(arms) => {
                        let tag = input.number(4)? as u32;
                        let arm = arms.get(tag as usize).ok_or(DecodeError::Tag)?;
                        work.extend(arm.iter().rev().map(|&f| Check::Visit(f)));
                    }
                    Node::Box { payload, .. } => work.push(Check::Visit(*payload)),
                    Node::Sequence { element, .. }
                    | Node::FlatSequence { element, .. }
                    | Node::Sequential { element, .. }
                    | Node::Columns { element, .. } => {
                        let count = input.length()?;
                        reserve(values, count)?;
                        work.push(Check::Elements {
                            node: *element,
                            count,
                            key: false,
                        });
                    }
                    Node::Repeat { count, element, .. } => {
                        reserve(values, *count)?;
                        work.push(Check::Elements {
                            node: *element,
                            count: *count,
                            key: false,
                        });
                    }
                    Node::Container(container) => {
                        let count = input.length()?;
                        reserve(
                            values,
                            count.saturating_mul(if container.set { 1 } else { 2 }),
                        )?;
                        work.push(Check::CollectionEnd);
                        work.push(match container.value {
                            None => Check::Elements {
                                node: container.key,
                                count,
                                key: true,
                            },
                            Some(value) => Check::Entries {
                                key: container.key,
                                value,
                                count,
                            },
                        });
                        work.push(Check::CollectionStart);
                    }
                }
            }
        }
    }
    if input.at != bytes.len() {
        return Err(DecodeError::Trailing.into());
    }
    Ok(())
}

enum Build {
    Visit(usize, *mut u8),
    Elements {
        node: usize,
        at: *mut u8,
        stride: u64,
        count: u64,
    },
    Entries {
        container: Container,
        at: *mut u8,
        count: u64,
    },
}

/// A fresh zeroed heap block for `size` payload bytes.
unsafe fn block(ctx: *mut TaskCtx, index: u32, size: u64, align: u64) -> *mut Header {
    unsafe {
        let block = hirsel_rt_v2_alloc(ctx, index, size, align.max(8), DOMAIN_HEAP, 0);
        std::ptr::write_bytes(payload(block), 0, size as usize);
        block
    }
}

/// A fresh sequence block of `count` zeroed elements.
unsafe fn sequence(ctx: *mut TaskCtx, e: &Elements, count: u64) -> *mut Header {
    unsafe {
        let block = allocate_sequence(ctx, e.index, count, e.stride, e.align);
        std::ptr::write_bytes(
            payload(block).add(e.offset as usize),
            0,
            (count * e.stride) as usize,
        );
        block
    }
}

/// Build the validated payload's value at `out` (zeroed storage of the root node's layout).
/// Validation has succeeded, so every read succeeds.
unsafe fn materialize(ctx: *mut TaskCtx, program: &Program, bytes: &[u8], out: *mut u8) {
    let mut input = Input { bytes, at: 0 };
    let checked = |r: Result<u128, DecodeError>| r.unwrap_or_else(|_| fault("codec reread"));
    let mut work = vec![Build::Visit(program.root, out)];
    while let Some(job) = work.pop() {
        let (node, at) = match job {
            Build::Visit(node, at) => (node, at),
            Build::Elements {
                node,
                at,
                stride,
                count,
            } => {
                if count > 0 {
                    work.push(Build::Elements {
                        node,
                        at: at.wrapping_add(stride as usize),
                        stride,
                        count: count - 1,
                    });
                    work.push(Build::Visit(node, at));
                }
                continue;
            }
            Build::Entries {
                container,
                at,
                count,
            } => {
                if count > 0 {
                    unsafe {
                        container.map.expect("map storage").publish(at);
                    }
                    work.push(Build::Entries {
                        container,
                        at: at.wrapping_add(container.entries.stride as usize),
                        count: count - 1,
                    });
                    if let Some(value) = container.value {
                        work.push(Build::Visit(
                            value,
                            at.wrapping_add(container.value_at as usize),
                        ));
                    }
                    work.push(Build::Visit(
                        container.key,
                        at.wrapping_add(container.key_at as usize),
                    ));
                }
                continue;
            }
        };
        unsafe {
            match &program.nodes[node] {
                Node::Scalar(Scalar::Bool) => *at = checked(input.number(1)) as u8,
                Node::Scalar(Scalar::Integer(width)) => {
                    write(at, &checked(input.number(*width)).to_le_bytes()[..*width])
                }
                Node::Scalar(Scalar::F32 | Scalar::Char) => {
                    write(at, &checked(input.number(4)).to_le_bytes()[..4])
                }
                Node::Scalar(Scalar::F64) => {
                    write(at, &checked(input.number(8)).to_le_bytes()[..8])
                }
                Node::Text(text) => {
                    let n = checked(input.number(8)) as u64;
                    let bytes = input.take(n).unwrap_or_else(|_| fault("codec reread"));
                    write_text(ctx, text, at, bytes);
                }
                Node::Path => {
                    let n = checked(input.number(8)) as u64;
                    let bytes = input.take(n).unwrap_or_else(|_| fault("codec reread"));
                    let text = std::str::from_utf8(bytes).unwrap_or_else(|_| fault("codec reread"));
                    let id = crate::paths::names(ctx).paths.intern(text);
                    write(at, &id.to_le_bytes());
                }
                Node::Fields(fields) => {
                    for &(offset, child) in fields.iter().rev() {
                        work.push(Build::Visit(child, at.add(offset as usize)));
                    }
                }
                Node::Checked { value, .. } => work.push(Build::Visit(*value, at)),
                Node::Variant { .. } => {
                    let ordinal = checked(input.number(4)) as u32;
                    for &(offset, child) in program.write_arm(node, at, ordinal).iter().rev() {
                        work.push(Build::Visit(child, at.add(offset as usize)));
                    }
                }
                Node::Box {
                    index,
                    size,
                    align,
                    payload: p,
                } => {
                    if *size == NO_NODE {
                        let content = flat_decode(program, *p, &mut input);
                        let b = block(ctx, *index, content.len() as u64, *align);
                        write(payload(b), &content);
                        set_pointer(at, b);
                    } else {
                        let b = block(ctx, *index, *size, *align);
                        set_pointer(at, b);
                        work.push(Build::Visit(*p, payload(b)));
                    }
                }
                Node::Sequence { elements, element } => {
                    let count = checked(input.number(8)) as u64;
                    let b = sequence(ctx, elements, count);
                    set_pointer(at, b);
                    work.push(Build::Elements {
                        node: *element,
                        at: payload(b).add(elements.offset as usize),
                        stride: elements.stride,
                        count,
                    });
                }
                Node::Repeat {
                    count,
                    stride,
                    element,
                } => work.push(Build::Elements {
                    node: *element,
                    at,
                    stride: *stride,
                    count: *count,
                }),
                Node::FlatText { .. }
                | Node::Framed(_)
                | Node::FlatFields(_)
                | Node::FlatVariant(_)
                | Node::FlatSequence { .. } => {
                    let content = flat_decode(program, node, &mut input);
                    write(at, &content);
                }
                Node::Sequential {
                    index,
                    align,
                    offset,
                    element,
                } => {
                    let count = checked(input.number(8)) as u64;
                    let mut content = Vec::new();
                    for _ in 0..count {
                        content.extend(inline_decode(program, *element, &mut input));
                    }
                    let used = content.len() as u64;
                    let b = block(
                        ctx,
                        *index,
                        offset
                            .checked_add(used)
                            .unwrap_or_else(|| fault("a validated codec extent overflows")),
                        *align,
                    );
                    write(payload(b), &count.to_le_bytes());
                    write(payload(b).add(8), &used.to_le_bytes());
                    write(payload(b).add(*offset as usize), &content);
                    set_pointer(at, b);
                }
                Node::Columns {
                    index,
                    align,
                    data,
                    quantum,
                    columns,
                    ..
                } => {
                    let count = checked(input.number(8)) as u64;
                    let capacity = count
                        .checked_add(quantum - 1)
                        .unwrap_or_else(|| fault("a validated codec extent overflows"))
                        & !(quantum - 1);
                    let width: u64 = columns.iter().map(|&(_, stride, _)| stride).sum();
                    let size = capacity
                        .checked_mul(width)
                        .and_then(|n| data.checked_add(n))
                        .unwrap_or_else(|| fault("a validated codec extent overflows"));
                    let b = block(ctx, *index, size, *align);
                    write(payload(b), &count.to_le_bytes());
                    write(payload(b).add(8), &capacity.to_le_bytes());
                    set_pointer(at, b);
                    for row in (0..count).rev() {
                        for &(prefix, stride, child) in columns.iter().rev() {
                            work.push(Build::Visit(
                                child,
                                payload(b).add((*data + capacity * prefix + row * stride) as usize),
                            ));
                        }
                    }
                }
                Node::Container(container) => {
                    let count = checked(input.number(8)) as u64;
                    let c = block(ctx, container.index, container.size, container.align);
                    let entries = sequence(ctx, &container.entries, count);
                    // An empty index is a valid unindexed container.
                    let index = allocate_sequence(
                        ctx,
                        container.index_index,
                        0,
                        container.index_stride,
                        container.index_align,
                    );
                    set_pointer(payload(c).add(container.entries_at as usize), entries);
                    set_pointer(payload(c).add(container.index_at as usize), index);
                    if let Some(map) = container.map {
                        let hashes = allocate_sequence(
                            ctx,
                            container.index_index,
                            0,
                            container.index_stride,
                            container.index_align,
                        );
                        set_pointer(payload(c).add(map.hashes_at as usize), hashes);
                        write(payload(c).add(map.live_at as usize), &count.to_le_bytes());
                    }
                    set_pointer(at, c);
                    let first = payload(entries).add(container.entries.offset as usize);
                    work.push(Build::Entries {
                        container: *container,
                        at: first,
                        count,
                    });
                }
            }
        }
    }
    if input.at != bytes.len() {
        fault("a codec rebuild that did not read its whole payload");
    }
}

/// `codec.decode` (LIR schema revision 29): validate the Bytes slice at `bytes` completely,
/// then write `Ok` of fresh owned data or `Error` of its first `DecodeError` at `out`. It cannot
/// fail; past the reference's operational budgets the run stops environmentally.
///
/// # Safety
/// `ctx` is a live run context, `codec` a compiler-emitted codec, `bytes` a live Bytes value of
/// its Bytes node's layout and `out` writable storage of its result layout.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_codec_decode(
    ctx: *mut TaskCtx,
    codec: *const Codec,
    bytes: *const c_void,
    out: *mut c_void,
) {
    // SAFETY: this semantic entry owns its task until it suspends or returns.
    let _role = unsafe { crate::tasks::role::Guard::new(ctx) };
    // SAFETY: caller contract; the decoder's own storage is dropped before a stop.
    unsafe { crate::stop::check(ctx, decode(ctx, codec, bytes, out)) }
}

unsafe fn decode(
    ctx: *mut TaskCtx,
    codec: *const Codec,
    bytes: *const c_void,
    out: *mut c_void,
) -> Stopped<()> {
    unsafe {
        let (program, descriptor, checks) = program(codec);
        let Node::Text(text) = &program.nodes[program.bytes] else {
            unreachable!("checked at parse")
        };
        let input = text_bytes(text, bytes.cast());
        if input.len() as u64 > LIMIT_BYTES {
            return Err(budget());
        }
        let out = out.cast::<u8>();
        std::ptr::write_bytes(out, 0, program.out_size as usize);
        let checked = match envelope(input, descriptor).map_err(Invalid::Decode) {
            Ok(payload) => validate(&program, payload).map(|()| payload),
            Err(invalid) => Err(invalid),
        };
        let checked = match checked {
            Ok(payload) => {
                let &[(offset, _)] = program.write_arm(program.out, out, 0) else {
                    fault("a codec result whose Ok arm is not one field");
                };
                let value = out.add(offset as usize);
                materialize(ctx, &program, payload, value);
                match program.root_descriptor {
                    None => Ok(()),
                    // SER-1: every check admits its occurrence, or the value is released.
                    Some(index) => match encode_payload(ctx, &program, value, Some(checks))? {
                        Some(_) => Ok(()),
                        None => {
                            let mut cleanup =
                                std::mem::MaybeUninit::<crate::finalize::Cleanup>::uninit();
                            let status = crate::finalize::hirsel_rt_v2_finalize(
                                ctx,
                                index,
                                value,
                                std::ptr::null(),
                                cleanup.as_mut_ptr(),
                                0,
                                0,
                            );
                            if status != crate::STATUS_OK {
                                fault("a decoded value's release failed");
                            }
                            std::ptr::write_bytes(out, 0, program.out_size as usize);
                            Err(Invalid::Decode(DecodeError::Invalid))
                        }
                    },
                }
            }
            Err(invalid) => Err(invalid),
        };
        match checked {
            Ok(()) => (),
            Err(Invalid::Budget) => return Err(budget()),
            Err(Invalid::Decode(error)) => {
                let &[(offset, error_node)] = program.write_arm(program.out, out, 1) else {
                    fault("a codec result whose Error arm is not one field");
                };
                let none = program.write_arm(error_node, out.add(offset as usize), error as u32);
                if !none.is_empty() {
                    fault("a DecodeError arm with fields");
                }
            }
        }
    }
    Ok(())
}

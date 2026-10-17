//! The canonical SER-2 codec over native layouts (runtime ABI 9; LIR schema revision 29).
//!
//! `hirsel_rt_v2_codec_encode` and `_decode` walk a compiler-emitted codec program
//! (`hirsel_rt_v2.h`, "The canonical SER-2 codec") that pairs each schema node with the layout
//! of its values. Both walks are iterative over runtime-owned work stacks, so no native stack
//! depth follows the data. Decoding validates the envelope and the whole payload before it
//! allocates anything, then builds fresh blocks in reading order; nothing partially decoded
//! exists. Error classes and their order are the reference codec's (`hirsel_types::wire`).
//! The reference's operational budgets (bytes, visited values) terminate environmentally.
use crate::alloc::{
    allocate_sequence, hirsel_rt_v2_alloc, payload, sequence_elements_offset, sequence_slice_start,
};
use crate::{DOMAIN_HEAP, Header, RunCtx, environmental, fault};
use std::collections::BTreeSet;
use std::ffi::c_void;

/// `hirsel_rt_v2_codec`.
#[repr(C)]
pub struct Codec {
    pub descriptor: *const u8,
    pub descriptor_length: u64,
    pub program: *const u64,
    pub program_length: u64,
}

pub const CODEC_VERSION: u64 = 1;
pub const NODE_SCALAR: u64 = 0;
pub const NODE_TEXT: u64 = 1;
pub const NODE_FIELDS: u64 = 2;
pub const NODE_VARIANT: u64 = 3;
pub const NODE_BOX: u64 = 4;
pub const NODE_SEQUENCE: u64 = 5;
pub const NODE_REPEAT: u64 = 6;
pub const NODE_MAP: u64 = 7;
pub const NODE_SET: u64 = 8;
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
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Scalar {
    Bool,
    Integer(usize),
    F32,
    F64,
}

struct Program {
    nodes: Vec<Node>,
    out_size: u64,
    root: usize,
    out: usize,
    bytes: usize,
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
        let mut w = Words { words, at: 0 };
        if w.next() != CODEC_VERSION {
            fault("a codec program of another version");
        }
        let count = usize::try_from(w.next()).unwrap_or_else(|_| fault("codec node count"));
        let out_size = w.next();
        let (root, out, bytes) = (w.node(count), w.node(count), w.node(count));
        let starts: Vec<u64> = (0..count).map(|_| w.next()).collect();
        let mut nodes = Vec::with_capacity(count);
        for start in starts {
            w.at = usize::try_from(start).unwrap_or_else(|_| fault("codec node offset"));
            let node = match w.next() {
                NODE_SCALAR => Node::Scalar(match w.next() {
                    0 => Scalar::Bool,
                    1 | 6 => Scalar::Integer(1),
                    2 | 7 => Scalar::Integer(2),
                    3 | 8 => Scalar::Integer(4),
                    4 | 9 => Scalar::Integer(8),
                    5 | 10 => Scalar::Integer(16),
                    11 => Scalar::F32,
                    12 => Scalar::F64,
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
                    if niche && (arms.len() != 2 || nullary > 1) {
                        fault("a codec niche that is not two arms");
                    }
                    Node::Variant {
                        niche,
                        tag,
                        nullary,
                        arms,
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
                    Node::Container(Container {
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
                _ => fault("an unknown codec node kind"),
            };
            nodes.push(node);
        }
        let program = Program {
            nodes,
            out_size,
            root,
            out,
            bytes,
        };
        if !matches!(
            program.nodes[program.bytes],
            Node::Text(Text { utf8: false, .. })
        ) {
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

unsafe fn program<'a>(codec: *const Codec) -> (Program, &'a [u8]) {
    unsafe {
        let codec = &*codec;
        let words = std::slice::from_raw_parts(codec.program, codec.program_length as usize);
        let descriptor =
            std::slice::from_raw_parts(codec.descriptor, codec.descriptor_length as usize);
        (Program::parse(words), descriptor)
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
unsafe fn write_text(ctx: *mut RunCtx, text: &Text, at: *mut u8, bytes: &[u8]) {
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

fn budget() -> ! {
    environmental("codec operational budget exhausted")
}

fn nan32(bits: u32) -> bool {
    bits & 0x7fff_ffff > 0x7f80_0000
}
fn nan64(bits: u64) -> bool {
    bits & 0x7fff_ffff_ffff_ffff > 0x7ff0_0000_0000_0000
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
}

/// The SER-2 payload of the value at `value`.
unsafe fn encode_payload(program: &Program, value: *const u8) -> Vec<u8> {
    let mut out = Vec::new();
    let mut work = vec![Put::Visit(program.root, value)];
    // Open sets: where the elements begin, and each element's start.
    let mut sets: Vec<(usize, Vec<usize>)> = Vec::new();
    let mut values = 0u64;
    while let Some(job) = work.pop() {
        if out.len() as u64 > LIMIT_BYTES {
            budget();
        }
        match job {
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
                let value = container.value.expect("a map entry has a value");
                work.push(Put::Visit(
                    value,
                    at.wrapping_add(container.value_at as usize),
                ));
                work.push(Put::Visit(
                    container.key,
                    at.wrapping_add(container.key_at as usize),
                ));
            }
            Put::Visit(node, at) => {
                values += 1;
                if values > LIMIT_VALUES {
                    budget();
                }
                unsafe { encode_node(program, node, at, &mut out, &mut work, &mut sets) };
            }
        }
    }
    if out.len() as u64 > LIMIT_BYTES {
        budget();
    }
    out
}

unsafe fn encode_node(
    program: &Program,
    node: usize,
    at: *const u8,
    out: &mut Vec<u8>,
    work: &mut Vec<Put>,
    sets: &mut Vec<(usize, Vec<usize>)>,
) {
    unsafe {
        match &program.nodes[node] {
            Node::Scalar(Scalar::Bool) => out.push(u8::from(*at != 0)),
            Node::Scalar(Scalar::Integer(width)) => out.extend_from_slice(read(at, *width)),
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
            Node::Container(container) => {
                let entries = payload(pointer(
                    payload(pointer(at)).add(container.entries_at as usize),
                ));
                let count = word(entries);
                out.extend_from_slice(&count.to_le_bytes());
                let first = entries.add(container.entries.offset as usize);
                if container.set {
                    sets.push((out.len(), Vec::new()));
                    work.push(Put::SetEnd);
                    work.push(Put::Elements {
                        node: container.key,
                        at: first.add(container.key_at as usize),
                        stride: container.entries.stride,
                        count,
                        set: true,
                    });
                } else {
                    work.push(Put::Entries {
                        container: *container,
                        at: first,
                        count,
                    });
                }
            }
        }
    }
}

/// `codec.encode` (LIR schema revision 29): write a fresh Bytes slice of the SER-2 envelope of
/// the value at `value` at `out`. It cannot fail; the reference's operational budgets terminate
/// environmentally.
///
/// # Safety
/// `ctx` is a live run context, `codec` a compiler-emitted codec, `value` a live value of its
/// root node's layout and `out` writable storage of its Bytes layout.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_codec_encode(
    ctx: *mut RunCtx,
    codec: *const Codec,
    value: *const c_void,
    out: *mut c_void,
) {
    unsafe {
        let (program, descriptor) = program(codec);
        let payload = encode_payload(&program, value.cast());
        let mut envelope = Vec::with_capacity(24 + descriptor.len() + payload.len());
        envelope.extend_from_slice(MAGIC);
        envelope.extend_from_slice(&WIRE_VERSION.to_le_bytes());
        envelope.extend_from_slice(&(descriptor.len() as u64).to_le_bytes());
        envelope.extend_from_slice(descriptor);
        envelope.extend_from_slice(&(payload.len() as u64).to_le_bytes());
        envelope.extend_from_slice(&payload);
        if envelope.len() as u64 > LIMIT_BYTES {
            budget();
        }
        let Node::Text(text) = &program.nodes[program.bytes] else {
            unreachable!("checked at parse")
        };
        std::ptr::write_bytes(out.cast::<u8>(), 0, program.out_size as usize);
        write_text(ctx, text, out.cast(), &envelope);
    }
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
fn validate(program: &Program, bytes: &[u8]) -> Result<(), DecodeError> {
    let mut input = Input { bytes, at: 0 };
    let mut work = vec![Check::Visit(program.root)];
    let mut collections: Vec<Collection<'_>> = Vec::new();
    let mut values = 0u64;
    let reserve = |values: u64, n: u64| {
        if n > LIMIT_VALUES.saturating_sub(values) {
            budget();
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
                            return Err(DecodeError::Duplicate);
                        }
                        if previous > key {
                            return Err(DecodeError::Order);
                        }
                    }
                    collection.previous = Some(key);
                } else if !collection.keys.insert(key) {
                    return Err(DecodeError::Duplicate);
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
                    budget();
                }
                match &program.nodes[node] {
                    Node::Scalar(Scalar::Bool) => {
                        if input.number(1)? > 1 {
                            return Err(DecodeError::Scalar);
                        }
                    }
                    Node::Scalar(Scalar::Integer(width)) => {
                        input.take(*width as u64)?;
                    }
                    Node::Scalar(Scalar::F32) => {
                        let bits = input.number(4)? as u32;
                        if nan32(bits) && bits != 0x7fc0_0000 {
                            return Err(DecodeError::Scalar);
                        }
                    }
                    Node::Scalar(Scalar::F64) => {
                        let bits = input.number(8)? as u64;
                        if nan64(bits) && bits != 0x7ff8_0000_0000_0000 {
                            return Err(DecodeError::Scalar);
                        }
                    }
                    Node::Text(text) => {
                        let n = input.length()?;
                        let bytes = input.take(n)?;
                        if text.utf8 && std::str::from_utf8(bytes).is_err() {
                            return Err(DecodeError::Utf8);
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
                    Node::Box { payload, .. } => work.push(Check::Visit(*payload)),
                    Node::Sequence { element, .. } => {
                        let count = input.length()?;
                        reserve(values, count);
                        work.push(Check::Elements {
                            node: *element,
                            count,
                            key: false,
                        });
                    }
                    Node::Repeat { count, element, .. } => {
                        reserve(values, *count);
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
                        );
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
        return Err(DecodeError::Trailing);
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
unsafe fn block(ctx: *mut RunCtx, index: u32, size: u64, align: u64) -> *mut Header {
    unsafe {
        let block = hirsel_rt_v2_alloc(ctx, index, size, align.max(8), DOMAIN_HEAP, 0);
        std::ptr::write_bytes(payload(block), 0, size as usize);
        block
    }
}

/// A fresh sequence block of `count` zeroed elements.
unsafe fn sequence(ctx: *mut RunCtx, e: &Elements, count: u64) -> *mut Header {
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
unsafe fn materialize(ctx: *mut RunCtx, program: &Program, bytes: &[u8], out: *mut u8) {
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
                    work.push(Build::Entries {
                        container,
                        at: at.wrapping_add(container.entries.stride as usize),
                        count: count - 1,
                    });
                    let value = container.value.expect("a map entry has a value");
                    work.push(Build::Visit(
                        value,
                        at.wrapping_add(container.value_at as usize),
                    ));
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
                Node::Scalar(Scalar::F32) => {
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
                Node::Fields(fields) => {
                    for &(offset, child) in fields.iter().rev() {
                        work.push(Build::Visit(child, at.add(offset as usize)));
                    }
                }
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
                    let b = block(ctx, *index, *size, *align);
                    set_pointer(at, b);
                    work.push(Build::Visit(*p, payload(b)));
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
                    set_pointer(at, c);
                    let first = payload(entries).add(container.entries.offset as usize);
                    work.push(match container.value {
                        None => Build::Elements {
                            node: container.key,
                            at: first.add(container.key_at as usize),
                            stride: container.entries.stride,
                            count,
                        },
                        Some(_) => Build::Entries {
                            container: *container,
                            at: first,
                            count,
                        },
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
/// fail; the reference's operational budgets terminate environmentally.
///
/// # Safety
/// `ctx` is a live run context, `codec` a compiler-emitted codec, `bytes` a live Bytes value of
/// its Bytes node's layout and `out` writable storage of its result layout.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_codec_decode(
    ctx: *mut RunCtx,
    codec: *const Codec,
    bytes: *const c_void,
    out: *mut c_void,
) {
    unsafe {
        let (program, descriptor) = program(codec);
        let Node::Text(text) = &program.nodes[program.bytes] else {
            unreachable!("checked at parse")
        };
        let input = text_bytes(text, bytes.cast());
        if input.len() as u64 > LIMIT_BYTES {
            budget();
        }
        let out = out.cast::<u8>();
        std::ptr::write_bytes(out, 0, program.out_size as usize);
        let checked = envelope(input, descriptor)
            .and_then(|payload| validate(&program, payload).map(|()| payload));
        match checked {
            Ok(payload) => {
                let &[(offset, _)] = program.write_arm(program.out, out, 0) else {
                    fault("a codec result whose Ok arm is not one field");
                };
                materialize(ctx, &program, payload, out.add(offset as usize));
            }
            Err(error) => {
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
}

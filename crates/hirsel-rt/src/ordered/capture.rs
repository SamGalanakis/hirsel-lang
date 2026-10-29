//! Read-only capture scans. This follows LayoutRecord.scan, never its drop
//! program: non-resource handles still constrain possible ordered operations.
//! Unknown or bounded-out scans cannot certify early-commit independence.

use std::collections::BTreeSet;

use hirsel_core_contract::handle_scan::*;

use crate::{Header, Layout, TaskCtx};

/// Raw handles must be resolved by the native scheduler's typed handle map.
/// In particular an untagged task word must never be guessed to be an atomic.
#[derive(Debug, Default, Eq, PartialEq)]
pub struct Captures {
    pub handles: BTreeSet<u64>,
    pub typed: BTreeSet<Handle>,
    pub unknown: bool,
    pub roots: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct Handle {
    pub word: u64,
    pub kind: u64,
    pub ty: u32,
    pub obtains: Option<u32>,
}

/// Static type edges follow every variant, not only the current value's.
/// Missing/opaque metadata has no definition, so Carries cannot certify it.
#[derive(Clone, Default)]
pub struct Types {
    pub definitions: std::sync::Arc<super::type_graph::TypeGraph>,
    pub families: std::sync::Arc<std::collections::BTreeMap<u64, BTreeSet<u32>>>,
}

/// Read the admitted program's immutable type graph, once before dispatch.
/// # Safety
/// `ctx` is live and its registry contains matching native scan programs.
pub unsafe fn types(ctx: *mut TaskCtx) -> Types {
    // SAFETY: caller contract; descriptor resolution reads immutable data.
    unsafe { types_with((*ctx).heap_ref().layout_count(), |id| (*ctx).descriptor(id)) }
}

unsafe fn types_with(count: u32, descriptor: impl Fn(u32) -> *const Layout) -> Types {
    let mut definitions = super::type_graph::TypeGraph::with_capacity(count as usize);
    let mut families = std::collections::BTreeMap::<_, BTreeSet<_>>::new();
    // SAFETY: caller contract, immutable registry lives for the run.
    unsafe {
        for id in 0..count {
            let layout = &*descriptor(id);
            if layout.scan_words == u64::MAX || layout.scan_words % WORDS as u64 != 0 {
                continue;
            }
            if layout.scan_words == 0 {
                definitions.define(id, Vec::new());
                continue;
            }
            if layout.scan.is_null() || layout.scan_words > (WORK_MAX * WORDS) as u64 {
                continue;
            }
            let mut edges = Vec::new();
            let mut known = true;
            for op in std::slice::from_raw_parts(layout.scan, layout.scan_words as usize)
                .chunks_exact(WORDS)
            {
                match op[0] {
                    HANDLE | UNKNOWN | TYPE_UNKNOWN => known = false,
                    TYPED_HANDLE => {
                        if op[2] == 0 || op[3] == 0 {
                            known = false;
                            continue;
                        }
                        let ty = op[3] as u32;
                        families.entry(op[2]).or_default().insert(ty);
                        edges.push(ty);
                        if op[4] != 0 {
                            edges.push(op[4] as u32);
                        }
                    }
                    INLINE | TYPE_EDGE => {
                        edges.push(op[2] as u32);
                    }
                    SEQUENCE | REPEAT => {
                        edges.push(op[4] as u32);
                    }
                    COLUMN => {
                        edges.push(op[6] as u32);
                    }
                    BLOCK | ROOT | TAG | NONNULL | LIVE_BIT | VALUE | VALUE_FIELD
                    | VALUE_COLUMN => {}
                    _ => known = false,
                }
            }
            if known {
                definitions.define(id, edges);
            }
        }
    }
    Types {
        definitions: definitions.into(),
        families: families.into(),
    }
}

const WORK_MAX: usize = 65_536;

/// Scan a live value while the scheduler role excludes object transitions.
/// This owns its result and retains no pointer or borrow into the value.
///
/// # Safety
/// `ctx`, the linked descriptors, `value`, and every active holder reachable
/// from it are live for this call, with no concurrent writes to those values.
/// Descriptor programs and offsets were produced by the matching compiler.
pub unsafe fn scan(ctx: *mut TaskCtx, value: *const u8, layout: u32) -> Captures {
    // SAFETY: caller contract; descriptor resolution only reads immutable data.
    unsafe { scan_with(value, layout, |id| (*ctx).descriptor(id)) }
}

unsafe fn scan_with(
    value: *const u8,
    layout: u32,
    descriptor: impl Fn(u32) -> *const Layout,
) -> Captures {
    let mut out = Captures::default();
    let mut pending = vec![(value, layout)];
    let mut blocks = BTreeSet::new();
    let mut work = 0usize;
    // SAFETY: the caller guarantees matching descriptors and live value graphs.
    unsafe {
        while let Some((value, layout)) = pending.pop() {
            let layout = &*descriptor(layout);
            if layout.scan_words == u64::MAX
                || layout.scan_words % WORDS as u64 != 0
                || (layout.scan_words != 0 && layout.scan.is_null())
            {
                out.unknown = true;
                continue;
            }
            if layout.scan_words == 0 {
                continue;
            }
            if layout.scan_words > (WORK_MAX * WORDS) as u64 {
                out.unknown = true;
                continue;
            }
            let words = std::slice::from_raw_parts(layout.scan, layout.scan_words as usize);
            let mut at = 0;
            while at < words.len() {
                work += 1;
                if work > WORK_MAX || pending.len() > WORK_MAX {
                    out.unknown = true;
                    return out;
                }
                let op = &words[at..at + WORDS];
                at += WORDS;
                let address = value.wrapping_add(op[1] as usize);
                match op[0] {
                    TYPED_HANDLE => {
                        if op[2] == 0 || op[3] == 0 {
                            out.unknown = true;
                        }
                        out.typed.insert(Handle {
                            word: address.cast::<u64>().read_unaligned(),
                            kind: op[2],
                            ty: op[3] as u32,
                            obtains: (op[4] != 0).then_some(op[4] as u32),
                        });
                    }
                    HANDLE => {
                        out.handles.insert(address.cast::<u64>().read_unaligned());
                    }
                    BLOCK | ROOT => {
                        out.roots |= op[0] == ROOT;
                        let block = address.cast::<*const Header>().read_unaligned();
                        if block.is_null() || !blocks.insert(block.addr()) {
                            continue;
                        }
                        let id = (*block).index();
                        // Shared values are inert. A mutex's exact-root wrapper
                        // is the exception: it contains the run-context handle.
                        if op[0] != ROOT || (*descriptor(id)).flags & crate::LAYOUT_MUTEX_ROOT != 0
                        {
                            pending.push((block.add(1).cast(), id));
                        }
                    }
                    INLINE => pending.push((address, op[2] as u32)),
                    TAG | NONNULL | LIVE_BIT => {
                        let active = match op[0] {
                            TAG => u64::from(address.cast::<u32>().read_unaligned()) == op[2],
                            NONNULL => !address.cast::<*const u8>().read_unaligned().is_null(),
                            LIVE_BIT if op[4] < 64 => {
                                (address.cast::<u64>().read_unaligned() >> op[4]) & 1 == op[2]
                            }
                            _ => {
                                out.unknown = true;
                                false
                            }
                        };
                        let span = op[3] as usize;
                        if span % WORDS != 0 || span > words.len() - at {
                            out.unknown = true;
                            return out;
                        }
                        if !active {
                            at += span;
                        }
                    }
                    SEQUENCE | REPEAT | COLUMN => {
                        let (count, stride, id, start) = match op[0] {
                            SEQUENCE => (
                                value.add(op[2] as usize).cast::<u64>().read_unaligned(),
                                op[3],
                                op[4] as u32,
                                address,
                            ),
                            REPEAT => (op[2], op[3], op[4] as u32, address),
                            _ => {
                                let count =
                                    value.add(op[2] as usize).cast::<u64>().read_unaligned();
                                let capacity =
                                    value.add(op[3] as usize).cast::<u64>().read_unaligned();
                                let Some(prefix) = capacity.checked_mul(op[5]) else {
                                    out.unknown = true;
                                    return out;
                                };
                                (count, op[4], op[6] as u32, address.add(prefix as usize))
                            }
                        };
                        if count > WORK_MAX.saturating_sub(pending.len()) as u64 {
                            out.unknown = true;
                            return out;
                        }
                        for i in 0..count {
                            let Some(offset) = i.checked_mul(stride) else {
                                out.unknown = true;
                                return out;
                            };
                            pending.push((start.add(offset as usize), id));
                        }
                    }
                    UNKNOWN => out.unknown = true,
                    TYPE_EDGE | TYPE_UNKNOWN | VALUE | VALUE_FIELD | VALUE_COLUMN => {}
                    _ => {
                        out.unknown = true;
                        return out;
                    }
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn value_metadata_is_not_a_holder_read_or_a_type_closure_edge() {
        let words = [
            VALUE,
            VALUE_BITS,
            8,
            0,
            0,
            0,
            0,
            0,
            VALUE_FIELD,
            0,
            0,
            u64::MAX,
            0,
            0,
            0,
            0,
            VALUE_COLUMN,
            u64::MAX,
            u64::MAX,
            u64::MAX,
            0,
            0,
            0,
            0,
        ];
        let descriptor = layout(&words);
        // There is deliberately no value at address zero. These instructions
        // describe decoding metadata and may never read a holder themselves.
        let captures = unsafe { scan_with(std::ptr::null(), 0, |_| &descriptor) };
        assert_eq!(captures, Captures::default());
        let types = unsafe { types_with(1, |_| &descriptor) };
        assert_eq!(types.definitions.get(&0), Some([].as_slice()));
    }

    fn layout(program: &[u64]) -> Layout {
        Layout {
            magic: crate::LAYOUT_MAGIC,
            version: crate::LAYOUT_VERSION,
            size: 32,
            align: 8,
            finalizer: None,
            steps: std::ptr::null(),
            step_count: 0,
            field_count: 0,
            flags: 0,
            logical: 0,
            logical_count: std::ptr::null(),
            scan: program.as_ptr(),
            scan_words: program.len() as u64,
        }
    }

    #[test]
    fn inactive_variant_and_dead_capture_never_read_their_storage() {
        let words = [
            TAG,
            0,
            1,
            WORDS as u64,
            0,
            0,
            0,
            0,
            HANDLE,
            8,
            0,
            0,
            0,
            0,
            0,
            0,
            LIVE_BIT,
            16,
            1,
            WORDS as u64,
            2,
            0,
            0,
            0,
            HANDLE,
            24,
            0,
            0,
            0,
            0,
            0,
            0,
        ];
        let layout = layout(&words);
        let mut value = [0u64, 19, 0, 37];
        let captures = unsafe { scan_with(value.as_ptr().cast(), 0, |_| &layout) };
        assert_eq!(captures, Captures::default());
        value[0] = 1;
        value[2] = 4;
        let captures = unsafe { scan_with(value.as_ptr().cast(), 0, |_| &layout) };
        assert_eq!(captures.handles, BTreeSet::from([19, 37]));
        assert!(!captures.unknown);
    }

    #[test]
    fn missing_metadata_cannot_certify_empty_captures() {
        let mut layout = layout(&[]);
        layout.scan_words = u64::MAX;
        assert!(unsafe { scan_with(std::ptr::null(), 0, |_| &layout) }.unknown);
        layout.scan_words = 0;
        assert!(!unsafe { scan_with(std::ptr::null(), 0, |_| &layout) }.unknown);
    }

    #[test]
    fn repeat_finds_nondestructible_handles_and_unknown_is_conservative() {
        let leaf = layout(&[HANDLE, 0, 0, 0, 0, 0, 0, 0]);
        let program = [REPEAT, 0, 3, 8, 1, 0, 0, 0, UNKNOWN, 0, 0, 0, 0, 0, 0, 0];
        let parent = layout(&program);
        let value = [11u64, 22, 33];
        let captures = unsafe {
            scan_with(value.as_ptr().cast(), 0, |id| {
                if id == 0 { &parent } else { &leaf }
            })
        };
        assert_eq!(captures.handles, BTreeSet::from([11, 22, 33]));
        assert!(captures.unknown);
    }

    #[test]
    fn counted_aliases_are_scanned_once_and_shared_mutex_roots_are_not_inert() {
        #[repr(C)]
        struct Block {
            header: Header,
            handle: u64,
        }
        let program = [HANDLE, 0, 0, 0, 0, 0, 0, 0];
        let mut leaf = layout(&program);
        let block = Block {
            header: Header {
                count: 1,
                descriptor: 1,
            },
            handle: 91,
        };
        let root_program = [ROOT, 0, 0, 0, 0, 0, 0, 0];
        let root = layout(&root_program);
        let value = &block.header as *const Header;
        let captures = unsafe {
            scan_with((&raw const value).cast(), 0, |id| {
                if id == 0 { &root } else { &leaf }
            })
        };
        assert!(captures.handles.is_empty());
        leaf.flags = crate::LAYOUT_MUTEX_ROOT;
        let captures = unsafe {
            scan_with((&raw const value).cast(), 0, |id| {
                if id == 0 { &root } else { &leaf }
            })
        };
        assert_eq!(captures.handles, BTreeSet::from([91]));
        let aliases = [value, value];
        let pair_program = [BLOCK, 0, 0, 0, 0, 0, 0, 0, BLOCK, 8, 0, 0, 0, 0, 0, 0];
        let pair = layout(&pair_program);
        let captures = unsafe {
            scan_with(aliases.as_ptr().cast(), 0, |id| {
                if id == 0 { &pair } else { &leaf }
            })
        };
        assert_eq!(captures.handles, BTreeSet::from([91]));
        assert!(!captures.unknown);
    }

    #[test]
    fn structure_of_arrays_uses_capacity_and_declared_column_prefix() {
        let program = [HANDLE, 0, 0, 0, 0, 0, 0, 0];
        let leaf = layout(&program);
        let program = [COLUMN, 16, 0, 8, 8, 8, 1, 0];
        let parent = layout(&program);
        // len 2, capacity 3; an unrelated 8-byte column comes first.
        let values = [2u64, 3, 100, 101, 102, 11, 22, 99];
        let captures = unsafe {
            scan_with(values.as_ptr().cast(), 0, |id| {
                if id == 0 { &parent } else { &leaf }
            })
        };
        assert_eq!(captures.handles, BTreeSet::from([11, 22]));
        assert!(!captures.unknown);
    }

    #[test]
    fn typed_receiver_closure_reads_inactive_types_without_reading_a_value() {
        use super::super::footprint::ObjectId;
        use super::super::objects::{Kind, Registry};
        use std::collections::BTreeMap;
        let empty = layout(&[]);
        let receiver_words = [TYPED_HANDLE, 0, 4, 2, 3, 0, 0, 0];
        let receiver = layout(&receiver_words);
        let message_words = [
            TAG,
            0,
            1,
            WORDS as u64,
            0,
            0,
            0,
            0,
            TYPED_HANDLE,
            u64::MAX,
            1,
            4,
            0,
            0,
            0,
            0,
        ];
        let message = layout(&message_words);
        let atomic_words = [TYPED_HANDLE, 0, 1, 4, 0, 0, 0, 0];
        let atomic = layout(&atomic_words);
        let layouts = [&empty, &empty, &receiver, &message, &atomic];
        let types = unsafe { types_with(layouts.len() as u32, |id| layouts[id as usize]) };
        assert!(types.definitions[&3].contains(&4));
        let value = 19u64;
        let captures =
            unsafe { scan_with((&raw const value).cast(), 2, |id| layouts[id as usize]) };
        assert_eq!(
            captures.typed,
            BTreeSet::from([Handle {
                word: 19,
                kind: 4,
                ty: 2,
                obtains: Some(3)
            }])
        );
        let mut registry = Registry::default();
        registry.install_types(types);
        registry.bind(
            Kind::Receiver,
            19,
            ObjectId {
                creator: vec![],
                ordinal: 0,
            },
            2,
            BTreeSet::new(),
        );
        let held = registry.captured(&captures);
        assert!(!held.unknown);
        assert_eq!(held.obtains, BTreeSet::from([3]));
        registry.bind(
            Kind::Atomic,
            29,
            ObjectId {
                creator: vec![],
                ordinal: 1,
            },
            4,
            BTreeSet::new(),
        );
        let operation = registry.footprint(Kind::Atomic, 29, true).unwrap();
        assert!(
            !registry
                .carries
                .disjoint(1, &BTreeMap::from([(1, held)]), &operation)
        );
    }
}

//! Read-only capture scans. This follows LayoutRecord.scan, never its drop
//! program: non-resource handles still constrain possible ordered operations.
//! Unknown or bounded-out scans cannot certify early-commit independence.

use std::collections::BTreeSet;

use hirsel_kernels::handle_scan::*;

use crate::{Header, Layout, TaskCtx};

/// Raw handles must be resolved by the native scheduler's typed handle map.
/// In particular an untagged task word must never be guessed to be an atomic.
#[derive(Debug, Default, Eq, PartialEq)]
pub struct Captures {
    pub handles: BTreeSet<u64>,
    pub unknown: bool,
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
                let address = value.add(op[1] as usize);
                match op[0] {
                    HANDLE => {
                        out.handles.insert(address.cast::<u64>().read_unaligned());
                    }
                    BLOCK | ROOT => {
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
}

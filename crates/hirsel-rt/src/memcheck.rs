//! Valgrind client requests. The memory requests are made only by the `memcheck` archive the
//! native memcheck campaign links; the stack requests describing task fibers are made by every
//! archive. Memcheck sees only malloc and free; these requests tell it about the blocks the
//! runtime carves out of its own backing allocations:
//!
//! - each slab is a mempool whose chunks are its live class blocks. Its block area starts
//!   inaccessible, so a free slot, the redzone after every slot ([`SLAB_REDZONE`]) and a reset
//!   token's payload are unaddressable, and a slab returned with blocks still live (values
//!   reachable at task exit) drops them with its pool instead of reporting them as leaks;
//! - each region segment lent to a region is a mempool whose chunks are its reservations, with
//!   a redzone after each ([`REGION_REDZONE`]); the unreserved tail is unaddressable, and region
//!   or arena exit destroys the pool, so storage used after the reset is reported.
//!
//! The archive also keeps every allocation, free and arena bump in the runtime (no inline fast
//! path: [`crate::alloc`]'s `Compiled` pages stay null and its inline-free flag zero, and a
//! region's `end` stays null), so emitted code needs no requests of its own.
//!
//! The request codes and the amd64 `VALGRIND_DO_CLIENT_REQUEST_EXPR` sequence are transcribed
//! from Valgrind 3.22.0's `valgrind.h` and `memcheck.h`, vendored in `third-party/valgrind`
//! (BSD-style licence, third-party/NOTICES.md). Outside Valgrind a request is six instructions
//! and returns its default; on other targets it compiles to nothing.
#![cfg_attr(not(feature = "memcheck"), allow(dead_code))]

/// Bytes left unaddressable after every slab slot and every region reservation, so a read or
/// write just past a block lands in no other block.
pub(crate) const SLAB_REDZONE: usize = if cfg!(feature = "memcheck") { 16 } else { 0 };
pub(crate) const REGION_REDZONE: u64 = if cfg!(feature = "memcheck") { 16 } else { 0 };

/// `VG_USERREQ_TOOL_BASE('M','C')`, memcheck's first request.
const MAKE_MEM_NOACCESS: u64 = (b'M' as u64) << 24 | (b'C' as u64) << 16;
const MAKE_MEM_UNDEFINED: u64 = MAKE_MEM_NOACCESS + 1;
const MAKE_MEM_DEFINED: u64 = MAKE_MEM_NOACCESS + 2;
const RUNNING_ON_VALGRIND: u64 = 0x1001;
const CREATE_MEMPOOL: u64 = 0x1303;
const DESTROY_MEMPOOL: u64 = 0x1304;
const MEMPOOL_ALLOC: u64 = 0x1305;
const MEMPOOL_FREE: u64 = 0x1306;
const MEMPOOL_CHANGE: u64 = 0x1309;
const STACK_REGISTER: u64 = 0x1501;
const STACK_DEREGISTER: u64 = 0x1502;

/// One client request: `VALGRIND_DO_CLIENT_REQUEST_EXPR(default, code, a1, a2, a3, a4, 0)`.
#[inline(always)]
fn request(default: u64, code: u64, [a1, a2, a3, a4]: [usize; 4]) -> u64 {
    #[cfg(target_arch = "x86_64")]
    {
        let args: [u64; 6] = [code, a1 as u64, a2 as u64, a3 as u64, a4 as u64, 0];
        let mut result = default;
        // SAFETY: the rotations total 128 bits, so they leave rdi as it was, and
        // `xchg rbx, rbx` changes nothing: natively the sequence only reads `args`. Valgrind
        // recognises it and answers the request in rdx.
        unsafe {
            std::arch::asm!(
                "rol rdi, 3",
                "rol rdi, 13",
                "rol rdi, 61",
                "rol rdi, 51",
                "xchg rbx, rbx",
                inout("rdx") result,
                in("rax") args.as_ptr(),
                options(nostack),
            );
        }
        result
    }
    #[cfg(not(target_arch = "x86_64"))]
    {
        let _ = (code, a1, a2, a3, a4);
        default
    }
}

/// Nonzero under Valgrind.
pub(crate) fn running_on_valgrind() -> u64 {
    request(0, RUNNING_ON_VALGRIND, [0; 4])
}

pub(crate) fn noaccess(address: *const u8, bytes: usize) {
    request(0, MAKE_MEM_NOACCESS, [address.addr(), bytes, 0, 0]);
}

pub(crate) fn undefined(address: *const u8, bytes: usize) {
    request(0, MAKE_MEM_UNDEFINED, [address.addr(), bytes, 0, 0]);
}

pub(crate) fn defined(address: *const u8, bytes: usize) {
    request(0, MAKE_MEM_DEFINED, [address.addr(), bytes, 0, 0]);
}

/// A pool anchored at `pool`, with no redzones of memcheck's own and undefined fresh chunks.
pub(crate) fn create_pool(pool: *const u8) {
    request(0, CREATE_MEMPOOL, [pool.addr(), 0, 0, 0]);
}

/// Forget the pool and every chunk still in it, making the chunks unaddressable.
pub(crate) fn destroy_pool(pool: *const u8) {
    request(0, DESTROY_MEMPOOL, [pool.addr(), 0, 0, 0]);
}

/// `bytes` at `address` become an addressable, undefined chunk of `pool`.
pub(crate) fn pool_alloc(pool: *const u8, address: *const u8, bytes: usize) {
    request(0, MEMPOOL_ALLOC, [pool.addr(), address.addr(), bytes, 0]);
}

/// The chunk at `address` leaves `pool` and becomes unaddressable.
pub(crate) fn pool_free(pool: *const u8, address: *const u8) {
    request(0, MEMPOOL_FREE, [pool.addr(), address.addr(), 0, 0]);
}

/// The chunk at `address` of `pool` now spans `bytes`; addressability is the caller's.
pub(crate) fn pool_resize(pool: *const u8, address: *const u8, bytes: usize) {
    let address = address.addr();
    request(0, MEMPOOL_CHANGE, [pool.addr(), address, address, bytes]);
}

/// Tell Valgrind that `start .. end` is a stack (a task fiber's), so switching to it is not a
/// frame of that size. Returns the stack's ID. A core request: every Valgrind tool answers it,
/// so every archive makes it.
pub(crate) fn stack_register(start: *const u8, end: *const u8) -> u64 {
    request(0, STACK_REGISTER, [start.addr(), end.addr(), 0, 0])
}

/// Forget a stack [`stack_register`] described.
pub(crate) fn stack_deregister(id: u64) {
    request(0, STACK_DEREGISTER, [id as usize, 0, 0, 0]);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn natively_every_request_returns_its_default_and_changes_nothing() {
        assert_eq!(running_on_valgrind(), 0);
        assert_eq!(request(7, CREATE_MEMPOOL, [0; 4]), 7);
        let mut bytes = [0x5au8; 64];
        let base = bytes.as_mut_ptr();
        create_pool(base);
        pool_alloc(base, base, 32);
        pool_resize(base, base, 48);
        pool_free(base, base);
        destroy_pool(base);
        assert_eq!(stack_register(base, base.wrapping_add(64)), 0);
        stack_deregister(0);
        noaccess(base, 64);
        undefined(base, 64);
        defined(base, 64);
        bytes[63] = 0xa5;
        assert!(bytes[..63].iter().all(|&b| b == 0x5a) && bytes[63] == 0xa5);
    }
}

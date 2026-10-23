// Adapted from hashbrown f7c44c608517ec79e0134d5747458aeb0aa8b86e,
// src/control/group/{sse2.rs:73-88,generic.rs:95-112}.
// Copyright (c) 2016 The Rust Project Developers.
// Licensed under either the Apache License, Version 2.0 or the MIT license.
// Unlike hashbrown controls, all 256 values here are occupied fingerprints.

#[inline]
pub(crate) fn matches(bytes: &[u8; 16], tag: u8) -> u16 {
    #[cfg(target_arch = "x86_64")]
    {
        use std::arch::x86_64::*;
        // SAFETY: SSE2 is baseline on x86-64; the borrow contains 16 bytes.
        unsafe {
            let group = _mm_loadu_si128(bytes.as_ptr().cast());
            _mm_movemask_epi8(_mm_cmpeq_epi8(group, _mm_set1_epi8(tag as i8))) as u16
        }
    }
    #[cfg(not(target_arch = "x86_64"))]
    generic(bytes, tag)
}

#[cfg_attr(target_arch = "x86_64", cfg(test))]
pub(crate) fn generic(bytes: &[u8; 16], tag: u8) -> u16 {
    const W: usize = size_of::<usize>();
    let repeat = |b| usize::from_ne_bytes([b; W]);
    let mut result = 0;
    for start in (0..16).step_by(W) {
        // SAFETY: W divides 16 on supported targets; the load is unaligned.
        let word = unsafe { bytes.as_ptr().add(start).cast::<usize>().read_unaligned() };
        let cmp = word ^ repeat(tag);
        let candidates = (cmp.wrapping_sub(repeat(1)) & !cmp & repeat(0x80)).to_le();
        for lane in 0..W {
            // SWAR subtraction can borrow into adjacent bytes. Refine every
            // candidate, and normalize byte order to logical 16-byte lanes.
            if candidates & (0x80usize << (lane * 8)) != 0 && bytes[start + lane] == tag {
                result |= 1 << (start + lane);
            }
        }
    }
    result
}

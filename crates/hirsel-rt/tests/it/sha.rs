//! SHA exports borrow exact extents, preserve owners and allocate no digest storage.
use crate::support::Ctx;
use hirsel_rt::{sha::*, text::allocate_bytes};
use std::ptr;
type Hash =
    unsafe extern "C" fn(*mut hirsel_rt::TaskCtx, *mut u8, *mut hirsel_rt::Header, u64, u64) -> u8;
fn unhex(s: &str) -> Vec<u8> {
    s.as_bytes()
        .chunks_exact(2)
        .map(|b| u8::from_str_radix(std::str::from_utf8(b).unwrap(), 16).unwrap())
        .collect()
}
#[test]
fn cavp_nonzero_extents_no_allocation_and_unchanged_owner() {
    for ctx in [Ctx::production(), Ctx::diagnostic()] {
        for row in include_str!("../../../../native-tests/library/vectors/sha-cavp.tsv").lines() {
            let f: Vec<_> = row.split('\t').collect();
            let hash: Hash = match f[0] {
                "sha1" => hirsel_rt_v2_bytes_sha1,
                "sha224" => hirsel_rt_v2_bytes_sha224,
                "sha256" => hirsel_rt_v2_bytes_sha256,
                "sha384" => hirsel_rt_v2_bytes_sha384,
                "sha512" => hirsel_rt_v2_bytes_sha512,
                "sha512_256" => hirsel_rt_v2_bytes_sha512_256,
                _ => unreachable!(),
            };
            let input = unhex(f[2]);
            let expected = unhex(f[3]);
            let source = [b"prefix".as_slice(), &input, b"suffix"].concat();
            unsafe {
                let value = allocate_bytes(ctx.0, &source);
                let before = ctx.stats();
                let mut output = [0xa5; 66];
                assert_eq!(
                    hash(
                        ctx.0,
                        output.as_mut_ptr().add(1),
                        value.owner,
                        value.start + 6,
                        input.len() as u64
                    ),
                    0
                );
                assert_eq!(&output[1..1 + expected.len()], expected);
                assert_eq!(output[0], 0xa5);
                assert_eq!(output[1 + expected.len()], 0xa5);
                assert_eq!((*value.owner).count, 1);
                assert_eq!(ctx.stats().backing_allocations, before.backing_allocations);
                assert_eq!(ctx.stats().live_blocks, before.live_blocks);
                assert_eq!(
                    hirsel_rt::release::hirsel_rt_v2_release(
                        ctx.0,
                        value.owner,
                        (*value.owner).index(),
                        ptr::null(),
                        0,
                        0
                    ),
                    0
                );
            }
            assert_eq!(ctx.stats().live_blocks, 0);
        }
    }
}

#[test]
fn excessive_64_bit_length_is_rejected_before_reading_or_publishing() {
    let ctx = Ctx::diagnostic();
    for hash in [
        hirsel_rt_v2_bytes_sha1 as Hash,
        hirsel_rt_v2_bytes_sha224,
        hirsel_rt_v2_bytes_sha256,
    ] {
        let mut output = [0xa5; 64];
        assert_eq!(
            unsafe { hash(ctx.0, output.as_mut_ptr(), ptr::null_mut(), 0, 1u64 << 61) },
            hirsel_rt::STATUS_FAILURE
        );
        assert_eq!(output, [0xa5; 64]);
    }
    assert_eq!(ctx.stats().live_blocks, 0);
}

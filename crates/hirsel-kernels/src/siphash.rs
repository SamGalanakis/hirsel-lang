//! SipHash-1-3 over bytes with explicitly supplied keys.
fn round(v: &mut [u64; 4]) {
    v[0] = v[0].wrapping_add(v[1]);
    v[1] = v[1].rotate_left(13);
    v[1] ^= v[0];
    v[0] = v[0].rotate_left(32);
    v[2] = v[2].wrapping_add(v[3]);
    v[3] = v[3].rotate_left(16);
    v[3] ^= v[2];
    v[0] = v[0].wrapping_add(v[3]);
    v[3] = v[3].rotate_left(21);
    v[3] ^= v[0];
    v[2] = v[2].wrapping_add(v[1]);
    v[1] = v[1].rotate_left(17);
    v[1] ^= v[2];
    v[2] = v[2].rotate_left(32);
}
pub fn siphash13(bytes: &[u8], k0: u64, k1: u64) -> u64 {
    let mut v = [
        0x736f6d6570736575 ^ k0,
        0x646f72616e646f6d ^ k1,
        0x6c7967656e657261 ^ k0,
        0x7465646279746573 ^ k1,
    ];
    let mut chunks = bytes.chunks_exact(8);
    for c in &mut chunks {
        let m = u64::from_le_bytes(c.try_into().unwrap());
        v[3] ^= m;
        round(&mut v);
        v[0] ^= m;
    }
    let mut last = (bytes.len() as u64) << 56;
    for (i, b) in chunks.remainder().iter().enumerate() {
        last |= (*b as u64) << (8 * i);
    }
    v[3] ^= last;
    round(&mut v);
    v[0] ^= last;
    v[2] ^= 0xff;
    for _ in 0..3 {
        round(&mut v);
    }
    v[0] ^ v[1] ^ v[2] ^ v[3]
}

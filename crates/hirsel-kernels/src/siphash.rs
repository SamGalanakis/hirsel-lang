//! SipHash-1-3 over bytes with explicitly supplied keys.
#[inline(always)]
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
/// One SipHash-1-3 stream. Chunk boundaries do not contribute to the hash.
/// Its six words are also the caller-owned native hashing scratch storage.
#[repr(C)]
#[derive(Clone)]
pub struct SipHasher13 {
    state: [u64; 4],
    tail: u64,
    length: u64,
}
impl SipHasher13 {
    pub fn new(k0: u64, k1: u64) -> Self {
        Self {
            state: [
                0x736f6d6570736575 ^ k0,
                0x646f72616e646f6d ^ k1,
                0x6c7967656e657261 ^ k0,
                0x7465646279746573 ^ k1,
            ],
            tail: 0,
            length: 0,
        }
    }
    #[inline(always)]
    fn compress(&mut self, word: u64) {
        self.state[3] ^= word;
        round(&mut self.state);
        self.state[0] ^= word;
    }
    // One-shot callers know the initial tail is empty; keep their state in registers.
    #[inline(always)]
    pub fn write(&mut self, mut bytes: &[u8]) {
        let used = (self.length & 7) as usize;
        self.length = self.length.wrapping_add(bytes.len() as u64);
        if used != 0 {
            let take = (8 - used).min(bytes.len());
            for (i, &byte) in bytes[..take].iter().enumerate() {
                self.tail |= u64::from(byte) << ((used + i) * 8);
            }
            bytes = &bytes[take..];
            if used + take != 8 {
                return;
            }
            self.compress(self.tail);
            self.tail = 0;
        }
        let mut chunks = bytes.chunks_exact(8);
        for chunk in &mut chunks {
            self.compress(u64::from_le_bytes(chunk.try_into().unwrap()));
        }
        for (i, &byte) in chunks.remainder().iter().enumerate() {
            self.tail |= u64::from(byte) << (i * 8);
        }
    }
    #[inline(always)]
    pub fn finish(&self) -> u64 {
        let mut final_state = self.clone();
        final_state.compress(self.tail | (self.length << 56));
        final_state.state[2] ^= 0xff;
        for _ in 0..3 {
            round(&mut final_state.state);
        }
        final_state.state.into_iter().fold(0, |a, b| a ^ b)
    }
}
pub fn siphash13(bytes: &[u8], k0: u64, k1: u64) -> u64 {
    let mut state = SipHasher13::new(k0, k1);
    state.write(bytes);
    state.finish()
}

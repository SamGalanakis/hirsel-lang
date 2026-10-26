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
// Rust core hash/sip.rs:113-146 loads a short tail in 4/2/1-byte pieces.
// Keep the reads bounded to this slice, including an unaligned final byte.
#[inline(always)]
fn tail_word(mut bytes: &[u8]) -> u64 {
    if bytes.len() == 8 {
        return u64::from_le_bytes(bytes.try_into().unwrap());
    }
    let mut word = 0u64;
    let mut shift = 0;
    if bytes.len() >= 4 {
        word = u64::from(u32::from_le_bytes(bytes[..4].try_into().unwrap()));
        bytes = &bytes[4..];
        shift = 32;
    }
    if bytes.len() >= 2 {
        word |= u64::from(u16::from_le_bytes(bytes[..2].try_into().unwrap())) << shift;
        bytes = &bytes[2..];
        shift += 16;
    }
    if let Some(&byte) = bytes.first() {
        word |= u64::from(byte) << shift;
    }
    word
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
            self.tail |= tail_word(&bytes[..take]) << (used * 8);
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
        self.tail = tail_word(chunks.remainder());
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

//! Shared binary frame envelope. The borrowed reader follows SQLite's WAL
//! header/payload/checksum validation and rr's length-delimited event stream.
use std::ops::Range;

pub const VERSION: u32 = 8;
pub const MAGIC: &[u8] = b"hirsel-io-tape 8\0";
pub const FRAME: usize = 17;

/// Append one frame in an existing buffer. Hash the payload in place; the
/// checksum never needs a separately encoded or copied payload.
pub fn frame(out: &mut Vec<u8>, seq: u64, kind: u8, payload: &[u8]) -> Result<(), String> {
    let start = begin(out);
    out.extend_from_slice(payload);
    finish(out, start, seq, kind)
}
pub fn begin(out: &mut Vec<u8>) -> usize {
    let start = out.len();
    out.resize(start + FRAME, 0);
    start
}
pub fn finish(out: &mut [u8], start: usize, seq: u64, kind: u8) -> Result<(), String> {
    let len = u32::try_from(out.len() - start - FRAME).map_err(|_| "tape frame too large")?;
    let rest = &mut out[start..];
    rest[..4].copy_from_slice(&len.to_le_bytes());
    rest[8..16].copy_from_slice(&seq.to_le_bytes());
    rest[16] = kind;
    let crc = crc32c_append(crc32c_append(0, &rest[..4]), &rest[8..]);
    rest[4..8].copy_from_slice(&crc.to_le_bytes());
    Ok(())
}
/// A validated frame borrows the tape buffer, including its raw payload bytes.
#[derive(Debug)]
pub struct Frame<'a> {
    pub kind: u8,
    pub payload: &'a [u8],
    pub range: Range<usize>,
}
pub fn read(bytes: &[u8], at: usize, sequence: u64) -> Result<Frame<'_>, String> {
    let rest = bytes.get(at..).ok_or("bad frame offset")?;
    if rest.len() < FRAME {
        return Err("torn frame header".into());
    }
    let len = u32::from_le_bytes(rest[..4].try_into().unwrap()) as usize;
    let end = FRAME
        .checked_add(len)
        .filter(|n| *n <= rest.len())
        .ok_or("torn frame payload")?;
    let rest = &rest[..end];
    let crc = u32::from_le_bytes(rest[4..8].try_into().unwrap());
    if crc != crc32c_append(crc32c_append(0, &rest[..4]), &rest[8..]) {
        return Err("checksum mismatch".into());
    }
    if u64::from_le_bytes(rest[8..16].try_into().unwrap()) != sequence {
        return Err("sequence gap".into());
    }
    Ok(Frame {
        kind: rest[16],
        payload: &rest[FRAME..],
        range: at..at + end,
    })
}

// Port of crc32c (zowens/crc32c), src/lib.rs:49-70 and
// src/sw.rs:42-70: runtime hardware dispatch and slicing-by-8 fallback.
// Use unaligned little-endian loads; no alignment prefix or parallel-lane tables.
const TABLE: [[u32; 256]; 8] = {
    let mut t = [[0; 256]; 8];
    let mut i = 0;
    while i < 256 {
        let mut c = i as u32;
        let mut bit = 0;
        while bit < 8 {
            c = (c >> 1) ^ (0x82f6_3b78u32.wrapping_mul(c & 1));
            bit += 1;
        }
        t[0][i] = c;
        i += 1;
    }
    let mut n = 1;
    while n < 8 {
        let mut j = 0;
        while j < 256 {
            let c = t[n - 1][j];
            t[n][j] = t[0][(c & 255) as usize] ^ (c >> 8);
            j += 1;
        }
        n += 1;
    }
    t
};
fn software(crc: u32, bytes: &[u8]) -> u32 {
    let mut c = !crc;
    let mut chunks = bytes.chunks_exact(8);
    for chunk in &mut chunks {
        let word = u64::from_le_bytes(chunk.try_into().unwrap()) ^ u64::from(c);
        c = TABLE[7][word as u8 as usize]
            ^ TABLE[6][(word >> 8) as u8 as usize]
            ^ TABLE[5][(word >> 16) as u8 as usize]
            ^ TABLE[4][(word >> 24) as u8 as usize]
            ^ TABLE[3][(word >> 32) as u8 as usize]
            ^ TABLE[2][(word >> 40) as u8 as usize]
            ^ TABLE[1][(word >> 48) as u8 as usize]
            ^ TABLE[0][(word >> 56) as u8 as usize];
    }
    for &b in chunks.remainder() {
        c = TABLE[0][((c as u8) ^ b) as usize] ^ (c >> 8);
    }
    !c
}
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "sse4.2")]
unsafe fn hardware(crc: u32, bytes: &[u8]) -> u32 {
    use std::arch::x86_64::{_mm_crc32_u8, _mm_crc32_u64};
    let mut c = u64::from(!crc);
    let mut chunks = bytes.chunks_exact(8);
    for chunk in &mut chunks {
        c = _mm_crc32_u64(c, u64::from_le_bytes(chunk.try_into().unwrap()));
    }
    for &b in chunks.remainder() {
        c = u64::from(_mm_crc32_u8(c as u32, b));
    }
    !(c as u32)
}
pub fn crc32c_append(crc: u32, bytes: &[u8]) -> u32 {
    #[cfg(target_arch = "x86_64")]
    if std::arch::is_x86_feature_detected!("sse4.2") {
        // SAFETY: the CPU feature was detected above; all loads stay in bounds.
        return unsafe { hardware(crc, bytes) };
    }
    software(crc, bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn castagnoli_vectors_and_streaming() {
        // RFC 3720 B.4 and the crc32c crate's public example.
        for (bytes, expected) in [
            (&b"123456789"[..], 0xe3069283),
            (&b"Hello world!"[..], 0x7b98e751),
            (&[0u8; 32][..], 0x8a9136aa),
            (&[255u8; 32][..], 0x62a8ab43),
        ] {
            assert_eq!(software(0, bytes), expected);
            assert_eq!(crc32c_append(0, bytes), expected);
            for n in 0..=bytes.len() {
                assert_eq!(
                    crc32c_append(crc32c_append(0, &bytes[..n]), &bytes[n..]),
                    expected
                );
            }
        }
        // crc32c/tests/simple.rs:31-65, including incremental and large vectors.
        for (bytes,expected) in [(&b"012345678910"[..],0x8412e281),(&b"1"[..],0x90f599e3),(&b"This is a very long string which is used to test the CRC-32-Castagnoli function."[..],0x20cb1e59)] {
            assert_eq!(software(0,bytes),expected);
            assert_eq!(crc32c_append(0,bytes),expected);
        }
        let big = "Hello!.\n".repeat(8192 * 4);
        assert_eq!(software(0, big.as_bytes()), 0x12bd9191);
        assert_eq!(crc32c_append(0, big.as_bytes()), 0x12bd9191);
        let bytes: Vec<_> = (0..8192).map(|n| n as u8).collect();
        for n in 0..128 {
            assert_eq!(software(0, &bytes[n..]), crc32c_append(0, &bytes[n..]));
        }
    }
    #[test]
    fn envelope_borrows_and_detects_damage() {
        let mut bytes = vec![];
        frame(&mut bytes, 0, 3, b"\0\xffpayload").unwrap();
        let parsed = read(&bytes, 0, 0).unwrap();
        assert_eq!(parsed.payload.as_ptr(), bytes[FRAME..].as_ptr());
        for i in 0..bytes.len() {
            let mut bad = bytes.clone();
            bad[i] ^= 1;
            assert!(read(&bad, 0, 0).is_err());
            assert!(read(&bytes[..i], 0, 0).is_err());
        }
        assert!(read(&bytes, 0, 1).is_err());
    }
}

//! RustCrypto SHA adapters. Compression, buffering, padding and CPU dispatch are upstream.
//! hashes 82c36a4 sha2/src/lib.rs:74-109, sha2/src/core_api.rs:15-175;
//! sha1 0.10.6 src/lib.rs:56-132. Only Hirsel's deterministic work checkpoints are added.
use sha2::Digest;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Algorithm {
    Sha1,
    Sha224,
    Sha256,
    Sha384,
    Sha512,
    Sha512_256,
}

impl Algorithm {
    pub fn from_operation(name: &str) -> Option<Self> {
        Some(match name {
            "sha1" => Self::Sha1,
            "sha224" => Self::Sha224,
            "sha256" => Self::Sha256,
            "sha384" => Self::Sha384,
            "sha512" => Self::Sha512,
            "sha512_256" => Self::Sha512_256,
            _ => return None,
        })
    }
    pub const fn block_size(self) -> u64 {
        match self {
            Self::Sha1 | Self::Sha224 | Self::Sha256 => 64,
            _ => 128,
        }
    }
    pub const fn output_size(self) -> usize {
        match self {
            Self::Sha1 => 20,
            Self::Sha224 => 28,
            Self::Sha256 | Self::Sha512_256 => 32,
            Self::Sha384 => 48,
            Self::Sha512 => 64,
        }
    }
    /// Compression blocks, including the mandatory one or two padding blocks.
    /// Quotient/remainder avoids overflowing `length + padding` at u64::MAX.
    pub fn blocks(self, length: u64) -> u64 {
        let block = self.block_size();
        let length_bytes = if block == 64 { 8 } else { 16 };
        (length / block)
            .checked_add(1 + u64::from(length % block >= block - length_bytes))
            .expect("SHA block count fits u64")
    }
    /// One call unit, plus one unit for each compression block, including padding.
    pub fn charge(self, length: u64) -> u64 {
        self.blocks(length)
            .checked_add(1)
            .expect("SHA work charge fits u64")
    }
}

/// At most 64 compression blocks between checkpoints. This is independent of SIMD width.
pub const CHECKPOINT_BLOCKS: usize = 64;

/// Each callback precedes its work. No output is published until every charge succeeds.
/// The unused suffix of the fixed output buffer is zero; no heap storage is needed.
pub fn digest<E>(
    algorithm: Algorithm,
    source: &[u8],
    mut checkpoint: impl FnMut(u64) -> Result<(), E>,
) -> Result<[u8; 64], E> {
    fn hash<D: Digest, E>(
        algorithm: Algorithm,
        source: &[u8],
        checkpoint: &mut impl FnMut(u64) -> Result<(), E>,
    ) -> Result<[u8; 64], E> {
        checkpoint(1)?;
        let mut state = D::new();
        let block = algorithm.block_size() as usize;
        let full = source.len() / block * block;
        for chunk in source[..full].chunks(block * CHECKPOINT_BLOCKS) {
            checkpoint((chunk.len() / block) as u64)?;
            state.update(chunk);
        }
        // The tail and padding are charged together, before finalize compresses them.
        checkpoint(algorithm.blocks(source.len() as u64) - (full / block) as u64)?;
        state.update(&source[full..]);
        let mut out = [0; 64];
        out[..algorithm.output_size()].copy_from_slice(&state.finalize());
        Ok(out)
    }
    match algorithm {
        Algorithm::Sha1 => hash::<sha1::Sha1, E>(algorithm, source, &mut checkpoint),
        Algorithm::Sha224 => hash::<sha2::Sha224, E>(algorithm, source, &mut checkpoint),
        Algorithm::Sha256 => hash::<sha2::Sha256, E>(algorithm, source, &mut checkpoint),
        Algorithm::Sha384 => hash::<sha2::Sha384, E>(algorithm, source, &mut checkpoint),
        Algorithm::Sha512 => hash::<sha2::Sha512, E>(algorithm, source, &mut checkpoint),
        Algorithm::Sha512_256 => hash::<sha2::Sha512_256, E>(algorithm, source, &mut checkpoint),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn unhex(s: &str) -> Vec<u8> {
        s.as_bytes()
            .chunks_exact(2)
            .map(|b| u8::from_str_radix(std::str::from_utf8(b).unwrap(), 16).unwrap())
            .collect()
    }
    #[test]
    fn cavp_and_boundary_vectors_and_exact_checkpoint_charges() {
        for row in include_str!("../../../native-tests/library/vectors/sha-cavp.tsv").lines() {
            let fields: Vec<_> = row.split('\t').collect();
            let algorithm = Algorithm::from_operation(fields[0]).unwrap();
            let message = unhex(fields[2]);
            assert_eq!(message.len(), fields[1].parse::<usize>().unwrap());
            let mut charges = vec![];
            let output = digest(algorithm, &message, |cost| {
                charges.push(cost);
                Ok::<_, ()>(())
            })
            .unwrap();
            assert_eq!(
                &output[..algorithm.output_size()],
                unhex(fields[3]),
                "{} {}",
                fields[0],
                fields[1]
            );
            assert_eq!(
                charges.iter().sum::<u64>(),
                algorithm.charge(message.len() as u64)
            );
            assert!(
                charges
                    .iter()
                    .all(|&n| n > 0 && n <= CHECKPOINT_BLOCKS as u64)
            );
        }
    }
    #[test]
    fn padding_counts_extreme_lengths_and_checkpoint_rejection() {
        for algorithm in [
            Algorithm::Sha1,
            Algorithm::Sha224,
            Algorithm::Sha256,
            Algorithm::Sha384,
            Algorithm::Sha512,
            Algorithm::Sha512_256,
        ] {
            let b = algorithm.block_size();
            let threshold = if b == 64 { 56 } else { 112 };
            for (length, blocks) in [
                (0, 1),
                (threshold - 1, 1),
                (threshold, 2),
                (b - 1, 2),
                (b, 2),
                (b + threshold - 1, 2),
                (b + threshold, 3),
            ] {
                assert_eq!(algorithm.blocks(length), blocks, "{algorithm:?} {length}");
            }
            assert_eq!(algorithm.blocks(u64::MAX), u64::MAX / b + 2);
            let source = vec![42; b as usize * (CHECKPOINT_BLOCKS * 3 + 1) + 1];
            let mut full = vec![];
            digest(algorithm, &source, |cost| {
                full.push(cost);
                Ok::<_, ()>(())
            })
            .unwrap();
            assert_eq!(
                full.iter().sum::<u64>(),
                algorithm.charge(source.len() as u64)
            );
            assert_eq!(full, [1, 64, 64, 64, 1, 1]);
            for reject in 0..full.len() {
                let mut seen = 0;
                assert_eq!(
                    digest(algorithm, &source, |_| {
                        let at = seen;
                        seen += 1;
                        if at == reject { Err(reject) } else { Ok(()) }
                    }),
                    Err(reject)
                );
                assert_eq!(seen, reject + 1, "no work/checkpoint after rejection");
            }
        }
    }
}

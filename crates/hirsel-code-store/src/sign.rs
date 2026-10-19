//! The signature seam: which signatures admit a code object to a store.
use crate::object::hex;
use hirsel_syntax::ContentHash;
use ring::signature::{ED25519, Ed25519KeyPair, KeyPair, UnparsedPublicKey};

const SIGNATURE_DOMAIN: &[u8] = b"hirsel.code.signature.v1\0";

/// An Ed25519 public key.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Hash)]
pub struct PublicKey(pub [u8; 32]);

impl PublicKey {
    /// A key from its 64 lowercase hex digits.
    pub fn parse(text: &str) -> Option<Self> {
        let digit = |b: u8| match b {
            b'0'..=b'9' => Some(b - b'0'),
            b'a'..=b'f' => Some(b - b'a' + 10),
            _ => None,
        };
        if text.len() != 64 {
            return None;
        }
        let mut key = [0u8; 32];
        for (byte, pair) in key.iter_mut().zip(text.as_bytes().chunks(2)) {
            *byte = digit(pair[0])? << 4 | digit(pair[1])?;
        }
        Some(Self(key))
    }
    /// The 64 lowercase hex digits, the key's spelling in configuration
    /// and object keys.
    pub fn hex(&self) -> String {
        hex(&ContentHash(self.0))
    }
}

/// The message a code signature signs: a domain and the definition hash,
/// which binds the object's whole content.
fn message(hash: &ContentHash) -> Vec<u8> {
    [SIGNATURE_DOMAIN, hash.0.as_slice()].concat()
}

/// A signing key for code objects.
pub struct Signer {
    pair: Ed25519KeyPair,
}

impl Signer {
    /// The Ed25519 key with this 32-byte seed.
    pub fn from_seed(seed: &[u8; 32]) -> Self {
        Self {
            pair: Ed25519KeyPair::from_seed_unchecked(seed).expect("any 32 bytes are a seed"),
        }
    }
    /// A key from its PKCS#8 v1 or v2 document.
    pub fn from_pkcs8(document: &[u8]) -> Result<Self, String> {
        Ed25519KeyPair::from_pkcs8_maybe_unchecked(document)
            .map(|pair| Self { pair })
            .map_err(|e| format!("not an Ed25519 PKCS#8 key: {e}"))
    }
    pub fn public_key(&self) -> PublicKey {
        PublicKey(
            self.pair
                .public_key()
                .as_ref()
                .try_into()
                .expect("an Ed25519 public key is 32 bytes"),
        )
    }
    /// The signature of the code object with `hash`.
    pub fn sign(&self, hash: &ContentHash) -> Vec<u8> {
        self.pair.sign(&message(hash)).as_ref().to_vec()
    }
}

/// Whether `signature` is `key`'s signature of the code object `hash`.
pub fn verify(key: &PublicKey, hash: &ContentHash, signature: &[u8]) -> bool {
    UnparsedPublicKey::new(&ED25519, key.0)
        .verify(&message(hash), signature)
        .is_ok()
}

/// A store's signature policy, chosen by its configuration's `signatures`
/// member. It changes which code a store admits, never what admitted code
/// means.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub enum Signatures {
    /// `sign.none@1` (default): no signature is required or checked; the
    /// content hash alone identifies code.
    #[default]
    None,
    /// `sign.ed25519@1`: every object must carry an Ed25519 signature by
    /// one of `keys` (sorted, unique, at least one), checked when it is
    /// added and again whenever it is read.
    Ed25519 { keys: Vec<PublicKey> },
}

impl Signatures {
    pub fn provider(&self) -> &'static str {
        match self {
            Self::None => "sign.none@1",
            Self::Ed25519 { .. } => "sign.ed25519@1",
        }
    }
    /// The keys whose signature admits an object, or `None` when objects
    /// need no signature.
    pub fn trusted(&self) -> Option<&[PublicKey]> {
        match self {
            Self::None => None,
            Self::Ed25519 { keys } => Some(keys),
        }
    }
}

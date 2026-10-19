//! The code store: `add(bundle) -> Ok | Missing(hashes)` and fetch by hash
//! over an object storage seam.
use crate::object::{CodeObject, ObjectError, hex};
use crate::sign::{PublicKey, Signatures, Signer, verify};
use hirsel_object_storage::{ObjectStorage, Put, StorageError};
use hirsel_proof_export::DefinitionGroup;
use hirsel_syntax::ContentHash;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

/// Code objects offered to a store, each under the hash it claims, with
/// signatures of them.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Bundle {
    objects: BTreeMap<ContentHash, Vec<u8>>,
    signatures: BTreeMap<ContentHash, BTreeMap<PublicKey, Vec<u8>>>,
}

impl Bundle {
    pub fn new() -> Self {
        Self::default()
    }
    /// The code objects of every member of `groups`.
    pub fn of_groups<'a>(groups: impl IntoIterator<Item = &'a DefinitionGroup>) -> Self {
        let mut bundle = Self::new();
        for group in groups {
            for object in CodeObject::of_group(group) {
                bundle.insert(object.hash(), object.bytes().to_vec());
            }
        }
        bundle
    }
    /// Offer `bytes` as the code object of `hash`.
    pub fn insert(&mut self, hash: ContentHash, bytes: Vec<u8>) {
        self.objects.insert(hash, bytes);
    }
    /// Remove the object claimed for `hash` and its signatures.
    pub fn remove(&mut self, hash: &ContentHash) -> Option<Vec<u8>> {
        self.signatures.remove(hash);
        self.objects.remove(hash)
    }
    /// Offer `key`'s `signature` of the object `hash`.
    pub fn insert_signature(&mut self, hash: ContentHash, key: PublicKey, signature: Vec<u8>) {
        self.signatures
            .entry(hash)
            .or_default()
            .insert(key, signature);
    }
    /// Sign every object with `signer`.
    pub fn sign(&mut self, signer: &Signer) {
        let key = signer.public_key();
        for hash in self.objects.keys().copied().collect::<Vec<_>>() {
            self.insert_signature(hash, key, signer.sign(&hash));
        }
    }
    /// Each offered signature of `hash` that verifies under its own key.
    fn valid_signatures<'a>(
        &'a self,
        hash: &'a ContentHash,
    ) -> impl Iterator<Item = (&'a PublicKey, &'a Vec<u8>)> + 'a {
        self.signatures
            .get(hash)
            .into_iter()
            .flatten()
            .filter(move |(key, signature)| verify(key, hash, signature))
    }
    /// Each offered object with the hash it claims.
    pub fn objects(&self) -> impl Iterator<Item = (&ContentHash, &[u8])> {
        self.objects.iter().map(|(h, b)| (h, b.as_slice()))
    }
    pub fn len(&self) -> usize {
        self.objects.len()
    }
    pub fn is_empty(&self) -> bool {
        self.objects.is_empty()
    }
}

/// The outcome of [`CodeStore::add`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Added {
    /// Every object of the bundle is in the store with its closure.
    Ok,
    /// Nothing was added: these dependencies are neither in the bundle nor
    /// in the store.
    Missing(BTreeSet<ContentHash>),
}

/// The outcome of [`CodeStore::closure`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Closure {
    /// The object and everything it reaches, verified.
    Complete(BTreeMap<ContentHash, CodeObject>),
    /// The closure reaches objects the store does not hold.
    Missing(BTreeSet<ContentHash>),
}

/// Why a store operation failed. A failed add may have created some
/// objects, each only after its dependencies, so the store never holds an
/// object without its closure.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum StoreError {
    /// `code.none@1` holds no code to add to.
    NoStore,
    /// A bundle object is not the code of the hash it claims; nothing was
    /// added.
    Refused {
        hash: ContentHash,
        reason: ObjectError,
    },
    /// The store's signature policy admits no signature of this object.
    Unsigned(ContentHash),
    /// A stored object is not the code of its key.
    Corrupt {
        hash: ContentHash,
        reason: ObjectError,
    },
    /// The object storage failed.
    Storage(StorageError),
}

impl std::fmt::Display for StoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoStore => write!(f, "code.none@1 stores no code"),
            Self::Refused { hash, reason } => write!(f, "refused {}: {reason}", hex(hash)),
            Self::Unsigned(hash) => write!(f, "no trusted signature of {}", hex(hash)),
            Self::Corrupt { hash, reason } => {
                write!(f, "stored {} is corrupt: {reason}", hex(hash))
            }
            Self::Storage(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for StoreError {}

impl From<StorageError> for StoreError {
    fn from(error: StorageError) -> Self {
        Self::Storage(error)
    }
}

fn code_key(hash: &ContentHash) -> String {
    format!("code/{}", hex(hash))
}

fn signature_key(hash: &ContentHash, key: &PublicKey) -> String {
    format!("sig/{}/{}", hex(hash), key.hex())
}

/// A code store: immutable code objects keyed by definition hash
/// (`code/<hash>`) and signatures (`sig/<hash>/<public key>`), all written
/// create-if-absent. Every object read is re-hashed and, under a signing
/// policy, its signature checked again.
#[derive(Clone)]
pub struct CodeStore {
    provider: &'static str,
    objects: Option<Arc<dyn ObjectStorage>>,
    signatures: Signatures,
}

impl std::fmt::Debug for CodeStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "CodeStore({}, {})",
            self.provider,
            self.signatures.provider()
        )
    }
}

impl CodeStore {
    /// `code.none@1`: holds no code; a place runs only its own revision.
    pub fn none() -> Self {
        Self {
            provider: "code.none@1",
            objects: None,
            signatures: Signatures::None,
        }
    }
    /// The store `provider` over `objects` under `signatures`.
    pub fn over(
        provider: &'static str,
        objects: Arc<dyn ObjectStorage>,
        signatures: Signatures,
    ) -> Self {
        Self {
            provider,
            objects: Some(objects),
            signatures,
        }
    }
    pub fn provider(&self) -> &'static str {
        self.provider
    }
    pub fn signatures(&self) -> &Signatures {
        &self.signatures
    }

    /// The stored object `hash`, verified, without its signatures.
    fn load(&self, hash: &ContentHash) -> Result<Option<CodeObject>, StoreError> {
        let Some(objects) = &self.objects else {
            return Ok(None);
        };
        let Some(bytes) = objects.get(&code_key(hash))? else {
            return Ok(None);
        };
        CodeObject::decode(*hash, bytes)
            .map(Some)
            .map_err(|reason| StoreError::Corrupt {
                hash: *hash,
                reason,
            })
    }

    /// Whether the store holds a trusted signature of `hash`.
    fn stored_signature(&self, hash: &ContentHash, keys: &[PublicKey]) -> Result<bool, StoreError> {
        let objects = self.objects.as_ref().expect("a signing store has objects");
        for key in keys {
            if let Some(signature) = objects.get(&signature_key(hash, key))?
                && verify(key, hash, &signature)
            {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// The code object `hash`, verified against its hash and, under a
    /// signing policy, a trusted signature.
    pub fn get(&self, hash: &ContentHash) -> Result<Option<CodeObject>, StoreError> {
        let Some(object) = self.load(hash)? else {
            return Ok(None);
        };
        if let Some(keys) = self.signatures.trusted()
            && !self.stored_signature(hash, keys)?
        {
            return Err(StoreError::Unsigned(*hash));
        }
        Ok(Some(object))
    }

    /// Add `bundle` all or nothing: every object is verified against the
    /// hash it claims (and, under a signing policy, needs a trusted
    /// signature in the bundle or the store), and the bundle is added only
    /// when every dependency of every object is in the bundle or the store.
    /// An object already stored is success, so re-adding is idempotent.
    pub fn add(&self, bundle: &Bundle) -> Result<Added, StoreError> {
        let Some(objects) = &self.objects else {
            return Err(StoreError::NoStore);
        };
        // An object holds its whole recursive group, so it offers every
        // member: a store holds a group whole.
        let mut offered = BTreeMap::new();
        for (hash, bytes) in bundle.objects() {
            let object = CodeObject::decode(*hash, bytes.to_vec()).map_err(|reason| {
                StoreError::Refused {
                    hash: *hash,
                    reason,
                }
            })?;
            for member in object.group() {
                offered.entry(member.hash()).or_insert(member);
            }
        }
        let mut new = BTreeMap::new();
        for (&hash, object) in &offered {
            if let Some(keys) = self.signatures.trusted()
                && !bundle
                    .valid_signatures(&hash)
                    .any(|(key, _)| keys.contains(key))
                && !self.stored_signature(&hash, keys)?
            {
                return Err(StoreError::Unsigned(hash));
            }
            if self.load(&hash)?.is_none() {
                new.insert(hash, object);
            }
        }
        let mut missing = BTreeSet::new();
        for object in new.values() {
            for dependency in object.dependencies() {
                if !offered.contains_key(dependency) && self.get(dependency)?.is_none() {
                    missing.insert(*dependency);
                }
            }
        }
        if !missing.is_empty() {
            return Ok(Added::Missing(missing));
        }
        for hash in offered.keys() {
            for (key, signature) in bundle.valid_signatures(hash) {
                objects.put_new(&signature_key(hash, key), signature)?;
            }
        }
        // Dependencies first, so every stored object's closure is stored.
        let mut written = BTreeSet::new();
        for root in new.keys() {
            let mut stack = vec![(*root, false)];
            while let Some((hash, expanded)) = stack.pop() {
                if written.contains(&hash) {
                    continue;
                }
                let Some(object) = new.get(&hash) else {
                    continue;
                };
                if !expanded {
                    stack.push((hash, true));
                    stack.extend(object.dependencies().iter().map(|d| (*d, false)));
                    continue;
                }
                if objects.put_new(&code_key(&hash), object.bytes())? == Put::Exists {
                    // A concurrent add stored it first; it must still verify.
                    self.load(&hash)?;
                }
                written.insert(hash);
            }
        }
        Ok(Added::Ok)
    }

    /// The object `hash` and every object it reaches, each verified as
    /// [`get`](Self::get) does.
    pub fn closure(&self, hash: &ContentHash) -> Result<Closure, StoreError> {
        let mut found = BTreeMap::new();
        let mut missing = BTreeSet::new();
        let mut pending = vec![*hash];
        while let Some(hash) = pending.pop() {
            if found.contains_key(&hash) || missing.contains(&hash) {
                continue;
            }
            match self.get(&hash)? {
                Some(object) => {
                    pending.extend(object.dependencies().iter().copied());
                    for member in object.group() {
                        found.insert(member.hash(), member);
                    }
                }
                None => {
                    missing.insert(hash);
                }
            }
        }
        Ok(if missing.is_empty() {
            Closure::Complete(found)
        } else {
            Closure::Missing(missing)
        })
    }
}

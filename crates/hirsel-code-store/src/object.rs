//! Code objects: one definition's hash preimage, verified by recomputation.
use hirsel_codec::{DecodeError, Reader, Writer};
use hirsel_proof_export::{ContentReference, DefinitionGroup, content_references, group_hashes};
use hirsel_syntax::ContentHash;
use std::collections::BTreeSet;

const OBJECT_VERSION: &str = "hirsel.code.object.v1";

/// A definition's code object: the LANG-6 hash preimage of its recursive
/// group (each member's content text and group references) and the
/// member it is. Decoding recomputes the hash, so an object is never
/// trusted for what it claims.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CodeObject {
    hash: ContentHash,
    bytes: Vec<u8>,
    dependencies: BTreeSet<ContentHash>,
    /// Every member's hash, this one's included, in member order.
    group: Vec<ContentHash>,
}

/// Why bytes are not the code object of a hash.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ObjectError {
    /// Not a canonical code object.
    Malformed(String),
    /// A well-formed object whose recomputed hash is `actual`.
    Tampered { actual: ContentHash },
}

impl std::fmt::Display for ObjectError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Malformed(message) => write!(f, "not a code object: {message}"),
            Self::Tampered { actual } => write!(f, "content hashes to {}", hex(actual)),
        }
    }
}

/// Lowercase hex of a hash, its spelling in object keys and references.
pub fn hex(hash: &ContentHash) -> String {
    hash.0.iter().map(|b| format!("{b:02x}")).collect()
}

fn encode(member: usize, texts: &[String], uses: &[Vec<usize>]) -> Vec<u8> {
    let mut w = Writer::<()>::default();
    w.open("CodeObject");
    w.text(OBJECT_VERSION);
    w.uint(member as u128);
    w.open("list");
    for (text, used) in texts.iter().zip(uses) {
        w.open("Member");
        w.text(text);
        w.open("list");
        for &j in used {
            w.uint(j as u128);
        }
        w.close();
        w.close();
    }
    w.close();
    w.close();
    w.finish().0.into_bytes()
}

fn decode(text: &str) -> Result<(usize, Vec<String>, Vec<Vec<usize>>), DecodeError> {
    let mut r = Reader::new(text);
    r.open("CodeObject")?;
    if r.text()? != OBJECT_VERSION {
        return Err(r.error(format!("not {OBJECT_VERSION}")));
    }
    let index = |r: &mut Reader| {
        let n = r.uint()?;
        usize::try_from(n).map_err(|_| r.error("index out of range"))
    };
    let member = index(&mut r)?;
    let (mut texts, mut uses) = (vec![], vec![]);
    r.open("list")?;
    while !r.at_close() {
        r.open("Member")?;
        texts.push(r.text()?);
        r.open("list")?;
        let mut used = vec![];
        while !r.at_close() {
            used.push(index(&mut r)?);
        }
        r.close()?;
        r.close()?;
        uses.push(used);
    }
    r.close()?;
    r.close()?;
    r.finish()?;
    Ok((member, texts, uses))
}

impl CodeObject {
    /// The code object of each member of `group`, in member order.
    pub fn of_group(group: &DefinitionGroup) -> Vec<CodeObject> {
        Self::members(&group.texts, &group.uses, &group.hashes)
    }
    fn members(texts: &[String], uses: &[Vec<usize>], hashes: &[ContentHash]) -> Vec<CodeObject> {
        (0..texts.len())
            .map(|member| {
                Self::decode(hashes[member], encode(member, texts, uses))
                    .expect("a definition group encodes as its members' code objects")
            })
            .collect()
    }
    /// The code object of every member of this object's recursive group,
    /// this one included, in member order. Each holds the same group.
    pub fn group(&self) -> Vec<CodeObject> {
        if self.group.len() == 1 {
            return vec![self.clone()];
        }
        let text = std::str::from_utf8(&self.bytes).expect("a decoded object is text");
        let (_, texts, uses) = decode(text).expect("a decoded object decodes");
        Self::members(&texts, &uses, &self.group)
    }
    /// The object `bytes` claim to be for `hash`: canonical, with each
    /// member's group references matching its text, and recomputing to
    /// `hash`.
    pub fn decode(hash: ContentHash, bytes: Vec<u8>) -> Result<Self, ObjectError> {
        let malformed = |m: String| ObjectError::Malformed(m);
        let text = std::str::from_utf8(&bytes).map_err(|e| malformed(e.to_string()))?;
        let (member, texts, uses) =
            decode(text).map_err(|e| malformed(format!("{} at byte {}", e.message, e.offset)))?;
        if member >= texts.len() {
            return Err(malformed(format!("member {member} of {}", texts.len())));
        }
        if encode(member, &texts, &uses) != bytes {
            return Err(malformed("not in canonical form".into()));
        }
        let mut dependencies = BTreeSet::new();
        for (i, (text, used)) in texts.iter().zip(&uses).enumerate() {
            let references = content_references(text).map_err(|e| {
                malformed(format!("member {i}: {} at byte {}", e.message, e.offset))
            })?;
            let mut markers = 0;
            for reference in references {
                match reference {
                    ContentReference::Definition(hash) => {
                        dependencies.insert(hash);
                    }
                    ContentReference::Group => markers += 1,
                }
            }
            if markers != used.len() {
                return Err(malformed(format!(
                    "member {i} has {markers} group references and {} targets",
                    used.len()
                )));
            }
        }
        let hashes = group_hashes(&texts, &uses).map_err(malformed)?;
        if hashes[member] != hash {
            return Err(ObjectError::Tampered {
                actual: hashes[member],
            });
        }
        Ok(Self {
            hash,
            bytes,
            dependencies,
            group: hashes,
        })
    }
    /// The definition hash this object is the code of.
    pub fn hash(&self) -> ContentHash {
        self.hash
    }
    /// The canonical bytes.
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }
    /// The definition hashes of its recursive group, in member order.
    pub fn group_hashes(&self) -> &[ContentHash] {
        &self.group
    }
    /// The definitions outside its group that it names.
    pub fn dependencies(&self) -> &BTreeSet<ContentHash> {
        &self.dependencies
    }
}

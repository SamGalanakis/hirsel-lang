//! Static callable storage behavior, separate from borrowed lifetimes.
//!
//! Profiles contain semantic selectors and a canonical rooted source type identity. Their
//! query/body/validation observations belong to the proof that publishes or
//! consumes them, never to this output's hash. Nothing here is a runtime grant.
use crate::{OwnedTypePart, ParameterRef, SharedPath, SharedSlice};
use hirsel_syntax::ContentHash;

pub mod graph;
mod paths;
pub use paths::{StorageTypePaths, storage_type_paths};

/// A real definition instance plus its existing lexical callable binder.
/// Generated instances may share the latter; neither key enters profile hashes.
#[derive(
    serde::Serialize, serde::Deserialize, Clone, Copy, Debug, Eq, PartialEq, Hash, Ord, PartialOrd,
)]
pub struct StorageCallable {
    pub definition: hirsel_syntax::DefId,
    pub scope: hirsel_syntax::SiteRef,
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq)]
pub struct CallableStorageProfile {
    pub callable: StorageCallable,
    pub profile: StorageProfile,
}

/// Parameters retain their ordinary binder semantics. Captures index the
/// callable's existing capture table, not an invented parameter binder.
#[derive(
    serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq, Hash, Ord, PartialOrd,
)]
pub enum StorageInput {
    Parameter(ParameterRef),
    Capture(u32),
}

#[derive(
    serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq, Hash, Ord, PartialOrd,
)]
pub struct StorageSelector {
    pub input: StorageInput,
    pub parts: SharedPath<OwnedTypePart>,
}

/// One owned result part whose placement comes from an input part. This is
/// not an access loan: a deep copy can retain placement without sharing bytes.
#[derive(
    serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq, Hash, Ord, PartialOrd,
)]
pub struct StorageResult {
    pub parts: SharedPath<OwnedTypePart>,
    pub source: StorageSelector,
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq, Hash)]
pub enum StorageBehavior {
    Known {
        allocations: SharedSlice<StorageSelector>,
        results: SharedSlice<StorageResult>,
    },
    /// A demanded proof cannot use an unresolved, erased or opaque producer.
    Unknown,
}

impl StorageBehavior {
    pub fn empty() -> Self {
        Self::Known {
            allocations: SharedSlice::new(),
            results: SharedSlice::new(),
        }
    }

    /// Monotone union for a finite selector lattice. Unknown evidence cannot
    /// become a known-empty promise through a cycle or a continuing branch.
    pub fn join(&mut self, other: &Self) -> bool {
        let next = match (&*self, other) {
            (Self::Unknown, _) | (_, Self::Unknown) => Self::Unknown,
            (
                Self::Known {
                    allocations: left_allocations,
                    results: left_results,
                },
                Self::Known {
                    allocations: right_allocations,
                    results: right_results,
                },
            ) => Self::Known {
                allocations: left_allocations
                    .iter()
                    .chain(right_allocations.iter())
                    .cloned()
                    .collect(),
                results: left_results
                    .iter()
                    .chain(right_results.iter())
                    .cloned()
                    .collect(),
            }
            .canonical(),
        };
        let changed = *self != next;
        *self = next;
        changed
    }

    fn canonical(&self) -> Self {
        match self {
            Self::Known {
                allocations,
                results,
            } => {
                let mut allocations = allocations.to_vec();
                allocations.sort();
                allocations.dedup();
                let mut results = results.to_vec();
                results.sort();
                results.dedup();
                Self::Known {
                    allocations: allocations.into(),
                    results: results.into(),
                }
            }
            Self::Unknown => Self::Unknown,
        }
    }
}

/// Canonical rooted source TYPE identity plus canonical inferred behavior. Callable
/// association (SiteRef/BodyId), read stamps and V digests are carried outside.
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq, Hash)]
pub struct StorageProfile {
    pub source_type: ContentHash,
    pub behavior: StorageBehavior,
    pub digest: ContentHash,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum StorageProfileError {
    UnabstractedParameter(ParameterRef),
    StaleDigest,
    UnknownProducer,
    SourceTypeMismatch,
    MissingAllocation(StorageSelector),
    MissingResult(StorageResult),
    BehaviorMismatch,
}

impl StorageProfile {
    pub fn new(
        source_type: ContentHash,
        behavior: StorageBehavior,
    ) -> Result<Self, StorageProfileError> {
        let behavior = behavior.canonical();
        let digest = semantic_digest(source_type, &behavior)?;
        Ok(Self {
            source_type,
            behavior,
            digest,
        })
    }

    /// `derived` is reconstructed by the consumer from executable operands,
    /// not another producer claim. Matching hashes alone cannot establish it.
    pub fn check_against(&self, derived: &Self) -> Result<(), StorageProfileError> {
        if self.digest != semantic_digest(self.source_type, &self.behavior)?
            || derived.digest != semantic_digest(derived.source_type, &derived.behavior)?
        {
            return Err(StorageProfileError::StaleDigest);
        }
        if matches!(self.behavior, StorageBehavior::Unknown)
            || matches!(derived.behavior, StorageBehavior::Unknown)
        {
            return Err(StorageProfileError::UnknownProducer);
        }
        if self.source_type != derived.source_type {
            return Err(StorageProfileError::SourceTypeMismatch);
        }
        let actual = self.behavior.canonical();
        let expected = derived.behavior.canonical();
        if let (
            StorageBehavior::Known {
                allocations,
                results,
            },
            StorageBehavior::Known {
                allocations: required_allocations,
                results: required_results,
            },
        ) = (&actual, &expected)
        {
            if let Some(missing) = required_allocations
                .iter()
                .find(|required| allocations.binary_search(required).is_err())
            {
                return Err(StorageProfileError::MissingAllocation(missing.clone()));
            }
            if let Some(missing) = required_results
                .iter()
                .find(|required| results.binary_search(required).is_err())
            {
                return Err(StorageProfileError::MissingResult(missing.clone()));
            }
        }
        if actual != expected {
            return Err(StorageProfileError::BehaviorMismatch);
        }
        Ok(())
    }
}

/// Semantic output only. Body-relative references must first be abstracted
/// into the callable's declared binder; interner/body/revision IDs never enter.
fn semantic_digest(
    source_type: ContentHash,
    behavior: &StorageBehavior,
) -> Result<ContentHash, StorageProfileError> {
    fn integer(bytes: &mut Vec<u8>, value: u32) {
        bytes.extend(value.to_le_bytes());
    }
    fn count(bytes: &mut Vec<u8>, value: usize) {
        bytes.extend((value as u64).to_le_bytes());
    }
    fn path(bytes: &mut Vec<u8>, parts: &SharedPath<OwnedTypePart>) {
        count(bytes, parts.len());
        for part in parts.iter() {
            match part {
                OwnedTypePart::Field(field) => {
                    bytes.push(0);
                    integer(bytes, *field);
                }
                OwnedTypePart::VariantField { variant, field } => {
                    bytes.push(1);
                    integer(bytes, *variant);
                    integer(bytes, *field);
                }
                OwnedTypePart::Elements => bytes.push(2),
            }
        }
    }
    fn selector(
        bytes: &mut Vec<u8>,
        selector: &StorageSelector,
    ) -> Result<(), StorageProfileError> {
        match selector.input {
            StorageInput::Parameter(ParameterRef::Bound { depth, parameter }) => {
                bytes.push(0);
                integer(bytes, depth);
                integer(bytes, parameter.0);
            }
            StorageInput::Parameter(parameter @ ParameterRef::Body { .. }) => {
                return Err(StorageProfileError::UnabstractedParameter(parameter));
            }
            StorageInput::Capture(capture) => {
                bytes.push(1);
                integer(bytes, capture);
            }
        }
        path(bytes, &selector.parts);
        Ok(())
    }
    let mut bytes = Vec::new();
    bytes.extend(source_type.0);
    match behavior.canonical() {
        StorageBehavior::Known {
            allocations,
            results,
        } => {
            bytes.push(0);
            count(&mut bytes, allocations.len());
            for allocation in &allocations {
                selector(&mut bytes, allocation)?;
            }
            count(&mut bytes, results.len());
            for result in &results {
                path(&mut bytes, &result.parts);
                selector(&mut bytes, &result.source)?;
            }
        }
        StorageBehavior::Unknown => bytes.push(1),
    }
    Ok(ContentHash::of_parts(
        "hirsel.storage.behavior.v1",
        [bytes.as_slice()],
    ))
}

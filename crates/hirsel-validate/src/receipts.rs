//! Local verifier authority, separate from producer query associations. Imported
//! content can share the physical store but cannot publish through this API.
use crate::{SignatureLookup, VRead, Validation, read_holds};
use bincode::Options;
use hirsel_artifact_store::{Action, Address, Domain, Store};
use hirsel_span::Anchor;
use hirsel_syntax::{ContentHash, DefPath, PackageId};
use hirsel_tir::Definition;
use hirsel_types::TypeInterner;
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{Arc, Mutex},
};

const DOMAIN: Domain = Domain {
    kind: 0x5600_0001,
    schema: 1,
};

/// An exact artifact identity computed here from every field and reachable type
/// row. Its definition and interner stay bound to that identity for validation.
pub struct Artifact<'a> {
    definition: &'a Definition,
    types: Arc<TypeInterner>,
    identity: Option<ContentHash>,
}
impl<'a> Artifact<'a> {
    pub fn new(
        definition: &'a Definition,
        types: Arc<TypeInterner>,
        owner: impl FnMut(Anchor) -> Option<(DefPath, u32)>,
    ) -> Self {
        let identity = hirsel_product_codec::canonical_bytes(types.clone(), definition, owner)
            .map(|bytes| ContentHash::of_parts("hirsel.validate.artifact.v1", [bytes.as_slice()]));
        Self {
            definition,
            types,
            identity,
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
struct Receipt {
    artifact: ContentHash,
    policy: ContentHash,
    reads: Vec<VRead>,
}
enum Backend {
    Memory(Mutex<HashMap<ContentHash, hirsel_artifact_store::Bytes>>),
    Directory(Store),
}
/// Only `validate` can issue a receipt. There is no producer/import/seed method.
/// Directory associations are trusted local state; exchanging output bytes
/// never exchanges these associations. Arbitrary filesystem writers are outside
/// this local-cache authority, as with the compiler's existing trusted cache.
pub struct VerifierCache(Backend);
impl std::fmt::Debug for VerifierCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("VerifierCache")
    }
}
impl Default for VerifierCache {
    fn default() -> Self {
        Self(Backend::Memory(Mutex::default()))
    }
}
impl VerifierCache {
    /// Use the same immutable output containers as ordinary query products.
    /// The verifier's domain and address associations remain separately owned.
    pub fn directory(root: PathBuf) -> Self {
        Self(Backend::Directory(Store::new(root)))
    }

    fn load<T: serde::de::DeserializeOwned>(&self, key: ContentHash, domain: Domain) -> Option<T> {
        let bytes = match &self.0 {
            Backend::Memory(rows) => rows
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .get(&key)
                .cloned()?,
            Backend::Directory(store) => {
                let association = store.lookup(Address(key))?;
                if association.action != Action(key) || association.reference.domain != domain {
                    return None;
                }
                let bytes = store.get(association.reference)?;
                store.touch(Address(key), association.reference);
                bytes
            }
        };
        bincode::DefaultOptions::new()
            .with_fixint_encoding()
            .with_limit(bytes.len() as u64)
            .reject_trailing_bytes()
            .deserialize(&bytes)
            .ok()
    }
    fn publish<T: Serialize>(&self, key: ContentHash, domain: Domain, receipt: &T) {
        let Ok(bytes) = bincode::serialize(receipt) else {
            return;
        };
        if bytes.len() > hirsel_artifact_store::MAX_RECORD {
            return;
        }
        match &self.0 {
            Backend::Memory(rows) => {
                rows.lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .insert(key, bytes.into());
            }
            Backend::Directory(store) => {
                if let Ok(reference) = store.put(domain, &bytes) {
                    let _ = store.associate(Address(key), Action(key), reference);
                }
            }
        }
    }
    /// Bind an ordinary evaluation output to evidence minted by V. The opaque
    /// receipt cannot be constructed or decoded by an ordinary producer.
    pub fn publish_execution(&self, subject: ContentHash, receipt: &ExecutionReceipt) {
        self.publish(
            execution_key(subject),
            EXECUTION_DOMAIN,
            &ExecutionRecord {
                subject,
                contexts: receipt.contexts.to_vec(),
                artifacts: receipt
                    .artifacts
                    .iter()
                    .map(|a| (a.artifact, a.policy))
                    .collect(),
            },
        );
    }
    /// Read only the verifier's local association and independently observe
    /// every base universe used by the completed execution. No producer record
    /// can seed this association by carrying a successful-looking read list.
    pub fn execution(
        &self,
        subject: ContentHash,
        mut current: impl FnMut(&PackageId) -> ContentHash,
    ) -> Option<ExecutionReceipt> {
        let record: ExecutionRecord = self.load(execution_key(subject), EXECUTION_DOMAIN)?;
        if record.subject != subject
            || record.artifacts.is_empty()
            || record.contexts.is_empty()
            || !record.contexts.windows(2).all(|w| w[0].0 < w[1].0)
            || !record
                .contexts
                .iter()
                .all(|(package, digest)| current(package) == *digest)
        {
            return None;
        }
        Some(ExecutionReceipt {
            contexts: record.contexts.into(),
            artifacts: record
                .artifacts
                .into_iter()
                .map(|(artifact, policy)| AcceptedArtifact { artifact, policy })
                .collect::<Vec<_>>()
                .into(),
        })
    }
}

const EXECUTION_DOMAIN: Domain = Domain {
    kind: 0x5600_0002,
    schema: 1,
};
fn execution_key(subject: ContentHash) -> ContentHash {
    ContentHash::of_parts("hirsel.validate.execution.v1", [subject.0.as_slice()])
}
/// A terminal successful V decision. There is intentionally no Deserialize or
/// producer constructor: only validate_with_receipt can mint this capability.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AcceptedArtifact {
    artifact: ContentHash,
    policy: ContentHash,
}
/// Composed execution evidence contains small artifact identities, not bodies.
/// Its complete independent base contexts guard private overlays and callees.
#[derive(Clone, Debug)]
pub struct ExecutionReceipt {
    contexts: Arc<[(PackageId, ContentHash)]>,
    artifacts: Arc<[AcceptedArtifact]>,
}
impl ExecutionReceipt {
    pub fn compose(
        contexts: &[(PackageId, ContentHash)],
        artifacts: &[AcceptedArtifact],
    ) -> Option<Self> {
        if artifacts.is_empty()
            || contexts.is_empty()
            || !contexts.windows(2).all(|w| w[0].0 < w[1].0)
        {
            return None;
        }
        Some(Self {
            contexts: contexts.to_vec().into(),
            artifacts: artifacts.to_vec().into(),
        })
    }
    pub fn contexts(&self) -> &[(PackageId, ContentHash)] {
        &self.contexts
    }
    pub fn artifacts(&self) -> &[AcceptedArtifact] {
        &self.artifacts
    }
}
#[derive(Serialize, Deserialize)]
struct ExecutionRecord {
    subject: ContentHash,
    contexts: Vec<(PackageId, ContentHash)>,
    artifacts: Vec<(ContentHash, ContentHash)>,
}

/// Reuse only this verifier's successful terminal record, after independently
/// observing its complete view/private/absence/universe/FirstHeap inputs. The
/// returned boolean says whether full V executed. An unencodable artifact still
/// receives full V; it simply cannot acquire a portable receipt.
pub fn validate_cached(
    artifact: &Artifact<'_>,
    signatures: &dyn SignatureLookup,
    policy: ContentHash,
    cache: Option<&VerifierCache>,
    reads: &mut Vec<VRead>,
) -> (Validation, bool) {
    let (report, ran, _) = validate_with_receipt(artifact, signatures, policy, cache, reads);
    (report, ran)
}
/// The same validation path, retaining the opaque evidence for composition
/// with dependent private/source artifacts after successful execution.
pub fn validate_with_receipt(
    artifact: &Artifact<'_>,
    signatures: &dyn SignatureLookup,
    policy: ContentHash,
    cache: Option<&VerifierCache>,
    reads: &mut Vec<VRead>,
) -> (Validation, bool, Option<AcceptedArtifact>) {
    let key = artifact.identity.map(|identity| {
        ContentHash::of_parts(
            "hirsel.validate.receipt.v1",
            [identity.0.as_slice(), policy.0.as_slice()],
        )
    });
    if let (Some(cache), Some(key), Some(identity)) = (cache, key, artifact.identity)
        && let Some(receipt) = cache.load::<Receipt>(key, DOMAIN)
        && receipt.artifact == identity
        && receipt.policy == policy
        && receipt
            .reads
            .iter()
            .all(|read| read_holds(read, signatures))
    {
        reads.extend(receipt.reads);
        return (
            Validation::default(),
            false,
            Some(AcceptedArtifact {
                artifact: identity,
                policy,
            }),
        );
    }
    let mut own = vec![];
    let report = crate::validate(artifact.definition, &artifact.types, signatures, &mut own);
    if report.violations.is_empty()
        && let (Some(cache), Some(key), Some(identity)) = (cache, key, artifact.identity)
    {
        cache.publish(
            key,
            DOMAIN,
            &Receipt {
                artifact: identity,
                policy,
                reads: own.clone(),
            },
        );
    }
    let accepted = report
        .violations
        .is_empty()
        .then(|| {
            artifact
                .identity
                .map(|artifact| AcceptedArtifact { artifact, policy })
        })
        .flatten();
    reads.extend(own);
    (report, true, accepted)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::support::{Fixture, owner_sig, path, session};
    use hirsel_tir::{BodyId, Constant, Rvalue};
    use std::sync::atomic::{AtomicU64, Ordering};

    struct Directory(PathBuf);
    impl Directory {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let root = std::env::temp_dir().join(format!(
                "hirsel-verdict-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir_all(&root).unwrap();
            Self(root)
        }
    }
    impl Drop for Directory {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    fn artifact(f: &Fixture) -> Artifact<'_> {
        Artifact::new(&f.definition, session(), |_| Some((path("main"), 0)))
    }
    fn check(f: &Fixture, cache: &VerifierCache, policy: ContentHash) -> (Validation, bool) {
        validate_cached(
            &artifact(f),
            &f.signatures,
            policy,
            Some(cache),
            &mut vec![],
        )
    }
    fn key(artifact: &Artifact<'_>, policy: ContentHash) -> ContentHash {
        ContentHash::of_parts(
            "hirsel.validate.receipt.v1",
            [artifact.identity.unwrap().0.as_slice(), policy.0.as_slice()],
        )
    }

    #[test]
    fn receipts_bind_all_artifact_bytes_policy_and_independent_views() {
        let mut f = Fixture::unit();
        owner_sig(&mut f, &[]);
        let cache = VerifierCache::default();
        let policy = ContentHash::default();
        let (report, ran) = check(&f, &cache, policy);
        assert!(ran && report.violations.is_empty(), "{report:?}");
        assert!(!check(&f, &cache, policy).1);
        assert!(check(&f, &cache, ContentHash::of_bytes(b"new V policy")).1);
        // The producer's public fingerprint stays unchanged; V observes every
        // view field independently, including metadata and private shape.
        f.signatures
            .get_mut(f.definition.path)
            .unwrap()
            .metadata
            .visibility = hirsel_syntax::Visibility::Public;
        assert!(check(&f, &cache, policy).1);
        f.body().rvalues[0] = Rvalue::Constant(Constant::Bool(true));
        for _ in 0..2 {
            let (report, ran) = check(&f, &cache, policy);
            assert!(ran && !report.violations.is_empty());
        }
    }

    #[test]
    fn directory_receipts_survive_reopen_and_corruption_requires_first_v() {
        let f = Fixture::unit();
        let directory = Directory::new();
        let policy = ContentHash::default();
        assert!(check(&f, &VerifierCache::directory(directory.0.clone()), policy).1);
        let reopened = VerifierCache::directory(directory.0.clone());
        assert!(!check(&f, &reopened, policy).1);
        let store = Store::new(directory.0.clone());
        let receipt = store.lookup(Address(key(&artifact(&f), policy))).unwrap();
        std::fs::write(store.output_path(receipt.reference), b"truncated").unwrap();
        let (report, ran) = check(&f, &reopened, policy);
        assert!(ran && report.violations.is_empty());
    }

    #[test]
    fn execution_receipts_require_v_capabilities_and_exact_current_contexts() {
        let f = Fixture::unit();
        let directory = Directory::new();
        let cache = VerifierCache::directory(directory.0.clone());
        let policy = ContentHash::default();
        let (report, _, accepted) = validate_with_receipt(
            &artifact(&f),
            &f.signatures,
            policy,
            Some(&cache),
            &mut vec![],
        );
        assert!(report.violations.is_empty());
        let contexts = vec![(
            path("main").0.package,
            ContentHash::of_bytes(b"private and public views"),
        )];
        let accepted = accepted.unwrap();
        assert!(ExecutionReceipt::compose(&contexts, &[]).is_none());
        assert!(ExecutionReceipt::compose(&[], &[accepted.clone()]).is_none());
        let receipt = ExecutionReceipt::compose(&contexts, &[accepted]).unwrap();
        let subject = ContentHash::of_bytes(b"complete evaluated value and action");
        cache.publish_execution(subject, &receipt);
        let reopened = VerifierCache::directory(directory.0.clone());
        assert!(reopened.execution(subject, |_| contexts[0].1).is_some());
        assert!(
            reopened
                .execution(subject, |_| ContentHash::default())
                .is_none()
        );
        assert!(
            reopened
                .execution(ContentHash::default(), |_| contexts[0].1)
                .is_none()
        );
        // Even exact output bytes do not transfer the issuer's associations.
        let imported = Directory::new();
        let source = Store::new(directory.0.clone());
        let association = source.lookup(Address(execution_key(subject))).unwrap();
        let bytes = source.get(association.reference).unwrap();
        Store::new(imported.0.clone())
            .put(EXECUTION_DOMAIN, &bytes)
            .unwrap();
        assert!(
            VerifierCache::directory(imported.0.clone())
                .execution(subject, |_| contexts[0].1)
                .is_none()
        );
    }

    #[test]
    fn imported_claimed_receipt_content_confers_no_authority() {
        let mut f = Fixture::unit();
        f.definition.entry = BodyId(u32::MAX);
        let directory = Directory::new();
        let policy = ContentHash::default();
        let artifact = artifact(&f);
        let forged = Receipt {
            artifact: artifact.identity.unwrap(),
            policy,
            reads: vec![],
        };
        // An importer can publish checked content, including bytes claiming the
        // verifier domain. It cannot issue a verifier association through the
        // ordinary product interface.
        Store::new(directory.0.clone())
            .put(DOMAIN, &bincode::serialize(&forged).unwrap())
            .unwrap();
        let cache = VerifierCache::directory(directory.0.clone());
        let (report, ran) = check(&f, &cache, policy);
        assert!(ran && !report.violations.is_empty());
        assert!(
            Store::new(directory.0.clone())
                .lookup(Address(key(&artifact, policy)))
                .is_none()
        );
    }
}

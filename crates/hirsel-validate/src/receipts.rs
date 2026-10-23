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

const VIEW_DOMAIN: Domain = Domain {
    kind: 0x5600_0003,
    schema: 1,
};
fn view_key(digest: crate::ValidationDigest, policy: ContentHash) -> ContentHash {
    ContentHash::of_parts(
        "hirsel.validate.signature-receipt.v1",
        [digest.0.0.as_slice(), policy.0.as_slice()],
    )
}

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
    owners: std::collections::BTreeMap<Anchor, (DefPath, u32)>,
    overlay: Option<hirsel_types::SignatureRef>,
}
impl<'a> Artifact<'a> {
    pub fn new(
        definition: &'a Definition,
        types: Arc<TypeInterner>,
        mut owner: impl FnMut(Anchor) -> Option<(DefPath, u32)>,
    ) -> Self {
        let mut owners = std::collections::BTreeMap::new();
        let identity = hirsel_product_codec::canonical_bytes(types.clone(), definition, |anchor| {
            let value = owner(anchor)?;
            owners.insert(anchor, value.clone());
            Some(value)
        })
        .map(|bytes| ContentHash::of_parts("hirsel.validate.artifact.v1", [bytes.as_slice()]));
        Self {
            definition,
            types,
            identity,
            owners,
            overlay: None,
        }
    }
    /// Bind private validation to the exact replacement signature too.
    pub fn with_overlay(mut self, signature: hirsel_types::SignatureRef) -> Self {
        self.overlay = Some(signature);
        self.identity = hirsel_product_codec::canonical_bytes(
            self.types.clone(),
            &(self.definition, &self.overlay),
            |anchor| self.owners.get(&anchor).cloned(),
        )
        .map(|bytes| {
            ContentHash::of_parts("hirsel.validate.private-artifact.v2", [bytes.as_slice()])
        });
        self
    }
}

#[derive(Clone, Serialize, Deserialize)]
struct Receipt {
    artifact: ContentHash,
    policy: ContentHash,
    reads: Vec<VRead>,
}
#[derive(Serialize, Deserialize)]
struct ViewReceipt {
    digest: crate::ValidationDigest,
    policy: ContentHash,
    reads: Vec<VRead>,
}
enum Backend {
    Memory(
        Mutex<HashMap<ContentHash, hirsel_artifact_store::Bytes>>,
        Arc<hirsel_artifact_store::Memory>,
    ),
    Directory(Store),
}
/// Only `validate` can issue a receipt. There is no producer/import/seed method.
/// Directory associations are trusted local state; exchanging output bytes
/// never exchanges these associations. Arbitrary filesystem writers are outside
/// this local-cache authority, as with the compiler's existing trusted cache.
pub struct VerifierCache(
    Backend,
    Mutex<Option<Arc<hirsel_product_codec::TypeRecords>>>,
);
impl std::fmt::Debug for VerifierCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("VerifierCache")
    }
}
impl Default for VerifierCache {
    fn default() -> Self {
        Self(
            Backend::Memory(Mutex::default(), Arc::default()),
            Mutex::default(),
        )
    }
}
impl VerifierCache {
    /// Use the same immutable output containers as ordinary query products.
    /// The verifier's domain and address associations remain separately owned.
    pub fn directory(root: PathBuf) -> Self {
        Self(Backend::Directory(Store::new(root)), Mutex::default())
    }

    fn load<T: serde::de::DeserializeOwned>(&self, key: ContentHash, domain: Domain) -> Option<T> {
        let bytes = match &self.0 {
            Backend::Memory(rows, _) => rows
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
            Backend::Memory(rows, _) => {
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
    /// Only V publishes terminal successful signature checks. A local hit
    /// independently observes the exact view digest and every contextual read.
    pub(crate) fn view_verdict(
        &self,
        types: &TypeInterner,
        definition: hirsel_syntax::DefId,
        signature: &hirsel_types::Signature,
        digest: crate::ValidationDigest,
        signatures: &dyn SignatureLookup,
        policy: ContentHash,
    ) -> Arc<crate::ViewVerdict> {
        if digest != crate::digest(types, signature) {
            return Arc::new(crate::check_view(types, definition, signature, signatures));
        }
        let key = view_key(digest, policy);
        if let Some(receipt) = self.load::<ViewReceipt>(key, VIEW_DOMAIN)
            && receipt.digest == digest
            && receipt.policy == policy
            && receipt
                .reads
                .iter()
                .all(|read| read_holds(read, signatures))
        {
            return Arc::new(crate::ViewVerdict {
                error: None,
                reads: receipt.reads,
            });
        }
        let verdict = crate::check_view(types, definition, signature, signatures);
        if verdict.error.is_none() {
            self.publish(
                key,
                VIEW_DOMAIN,
                &ViewReceipt {
                    digest,
                    policy,
                    reads: verdict.reads.clone(),
                },
            );
        }
        Arc::new(verdict)
    }
    fn contents(&self) -> Arc<dyn hirsel_artifact_store::Contents> {
        match &self.0 {
            Backend::Memory(_, contents) => contents.clone(),
            Backend::Directory(store) => Arc::new(Store::new(store.root().to_path_buf())),
        }
    }
    fn records(&self, types: Arc<TypeInterner>) -> Arc<hirsel_product_codec::TypeRecords> {
        let mut current = self.1.lock().unwrap_or_else(|error| error.into_inner());
        if let Some(records) = &*current
            && Arc::ptr_eq(&records.types, &types)
        {
            return records.clone();
        }
        let records = hirsel_product_codec::TypeRecords::new(types, self.contents());
        *current = Some(records.clone());
        records
    }
    fn retain_artifact(&self, artifact: &Artifact<'_>, policy: ContentHash) {
        let Some(identity) = artifact.identity else {
            return;
        };
        if self
            .load::<ReplayRecord>(replay_key(identity, policy), REPLAY_DOMAIN)
            .is_some_and(|record| record.artifact == identity && record.policy == policy)
        {
            return;
        }
        let records = self.records(artifact.types.clone());
        let Some(bytes) = records
            .encode_record(&(artifact.definition, &artifact.overlay), |anchor| {
                artifact.owners.get(&anchor).cloned()
            })
        else {
            return;
        };
        self.publish(
            replay_key(identity, policy),
            REPLAY_DOMAIN,
            &ReplayRecord {
                artifact: identity,
                policy,
                bytes,
            },
        );
    }
    /// Read only replay data associated by this verifier after terminal V.
    /// Recompute identity after portable type/anchor decoding before exposing it.
    fn replay_artifact(
        &self,
        accepted: &AcceptedArtifact,
        types: Arc<TypeInterner>,
        mut anchor: impl FnMut(&DefPath, u32) -> Option<Anchor>,
    ) -> Option<ReplayArtifact> {
        let record: ReplayRecord = self.load(
            replay_key(accepted.artifact, accepted.policy),
            REPLAY_DOMAIN,
        )?;
        if record.artifact != accepted.artifact || record.policy != accepted.policy {
            return None;
        }
        let records = self.records(types.clone());
        let mut owners = std::collections::BTreeMap::new();
        let (definition, overlay): (
            hirsel_tir::DefinitionRef,
            Option<hirsel_types::SignatureRef>,
        ) = records.decode_record(&record.bytes, |path, occurrence| {
            let site = anchor(path, occurrence)?;
            owners.insert(site, (path.clone(), occurrence));
            Some(site)
        })?;
        let mut artifact = Artifact::new(&definition, types, |anchor| owners.get(&anchor).cloned());
        if let Some(signature) = &overlay {
            if signature.definition != definition.path {
                return None;
            }
            artifact = artifact.with_overlay(signature.clone());
        }
        if artifact.identity != Some(accepted.artifact) {
            return None;
        }
        Some(ReplayArtifact {
            definition,
            overlay,
            policy: accepted.policy,
            owners,
        })
    }
    /// Refresh only a local association, running V here against independently
    /// supplied current views. A caller cannot refresh it with cloned evidence.
    pub fn execution_revalidated(
        &self,
        subject: ContentHash,
        types: Arc<TypeInterner>,
        mut anchor: impl FnMut(&DefPath, u32) -> Option<Anchor>,
        environment: &dyn ReplayEnvironment,
    ) -> Option<ExecutionReceipt> {
        let record: ExecutionRecord = self.load(execution_key(subject), EXECUTION_DOMAIN)?;
        if record.subject != subject
            || record.artifacts.is_empty()
            || record.contexts.is_empty()
            || !record.contexts.windows(2).all(|w| w[0].0 < w[1].0)
        {
            return None;
        }
        let contexts: Vec<_> = record
            .contexts
            .iter()
            .map(|(package, _)| Some((package.clone(), environment.base_digest(package)?)))
            .collect::<Option<_>>()?;
        let changed = contexts != record.contexts;
        let mut artifacts = Vec::new();
        let mut covered = std::collections::BTreeSet::new();
        for (artifact, policy) in record.artifacts {
            let accepted = AcceptedArtifact { artifact, policy };
            if changed {
                let replay = self.replay_artifact(&accepted, types.clone(), &mut anchor)?;
                let package = replay.definition.path.path().0.package.clone();
                if !contexts.iter().any(|(p, _)| *p == package) {
                    return None;
                }
                covered.insert(package.clone());
                let views = environment.views(&package, replay.overlay.clone())?;
                let verdict_key = ContentHash::of_parts(
                    "hirsel.validate.receipt.v1",
                    [accepted.artifact.0.as_slice(), accepted.policy.0.as_slice()],
                );
                let original: Receipt = self.load(verdict_key, DOMAIN)?;
                if original.artifact != accepted.artifact
                    || original.policy != accepted.policy
                    || !original.reads.iter().all(|read| {
                        if read_holds(read, &*views) {
                            return true;
                        }
                        // A new earlier declaration may carry the exact same
                        // nominal heap role. The original provider view is
                        // still guarded by its independent VRead::View. Full V
                        // below must check the new provider and current world.
                        match read {
                            VRead::FirstHeap(Some(previous)) => {
                                let old = views
                                    .view(*previous)
                                    .and_then(|(s, _)| s.metadata.roles.heap.as_ref());
                                let current = views.first_heap().map(|(_, role)| role);
                                old.is_some() && old == current.as_ref()
                            }
                            _ => false,
                        }
                    })
                {
                    return None;
                }

                let mut exact = Artifact::new(&replay.definition, types.clone(), |a| {
                    replay.owners.get(&a).cloned()
                });
                if let Some(overlay) = replay.overlay {
                    exact = exact.with_overlay(overlay);
                }
                if exact.identity != Some(accepted.artifact) {
                    return None;
                }
                let (report, _, refreshed) =
                    validate_with_receipt(&exact, &*views, replay.policy, Some(self), &mut vec![]);
                if !report.violations.is_empty() || refreshed.as_ref() != Some(&accepted) {
                    return None;
                }
            }
            artifacts.push(accepted);
        }
        if changed && contexts.iter().any(|(p, _)| !covered.contains(p)) {
            return None;
        }
        // The independently observed environment must remain the same through V.
        if contexts
            .iter()
            .any(|(p, d)| environment.base_digest(p) != Some(*d))
        {
            return None;
        }
        let receipt = ExecutionReceipt {
            contexts: contexts.into(),
            artifacts: artifacts.into(),
        };
        if changed {
            self.publish_execution(subject, &receipt);
        }
        Some(receipt)
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

/// Immutable independent signature input for replay. Implementations supply
/// views, never verdicts. Overlay bytes come only from the verifier's exact
/// locally retained artifact, and are independently checked again by V.
pub trait ReplayEnvironment {
    fn base_digest(&self, package: &PackageId) -> Option<ContentHash>;
    fn views(
        &self,
        package: &PackageId,
        overlay: Option<hirsel_types::SignatureRef>,
    ) -> Option<Box<dyn SignatureLookup + '_>>;
}

const REPLAY_DOMAIN: Domain = Domain {
    kind: 0x5600_0004,
    schema: 1,
};
fn replay_key(artifact: ContentHash, policy: ContentHash) -> ContentHash {
    ContentHash::of_parts(
        "hirsel.validate.replay.v1",
        [artifact.0.as_slice(), policy.0.as_slice()],
    )
}
#[derive(Serialize, Deserialize)]
struct ReplayRecord {
    artifact: ContentHash,
    policy: ContentHash,
    bytes: Vec<u8>,
}
/// Construction is verifier-private. These bytes alone are not acceptance.
struct ReplayArtifact {
    definition: hirsel_tir::DefinitionRef,
    overlay: Option<hirsel_types::SignatureRef>,
    policy: ContentHash,
    owners: std::collections::BTreeMap<Anchor, (DefPath, u32)>,
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
        cache.retain_artifact(artifact, policy);
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
        cache.retain_artifact(artifact, policy);
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

    fn view(f: &Fixture, cache: &VerifierCache, policy: ContentHash) -> Arc<crate::ViewVerdict> {
        let definition = f.definition.path;
        let signature = f.signatures.get(definition).unwrap();
        cache.view_verdict(
            &session(),
            definition,
            signature,
            crate::digest(&session(), signature),
            &f.signatures,
            policy,
        )
    }

    #[test]
    fn signature_receipts_reopen_bind_policy_and_refuse_corrupt_or_imported_content() {
        use hirsel_intern::work::{self, Counter};
        let mut f = Fixture::unit();
        owner_sig(&mut f, &[]);
        let directory = Directory::new();
        let policy = ContentHash::default();
        let before = work::read(Counter::ValidateSignatureRows);
        assert!(
            view(&f, &VerifierCache::directory(directory.0.clone()), policy)
                .error
                .is_none()
        );
        assert_eq!(work::read(Counter::ValidateSignatureRows), before + 1);
        let reopened = VerifierCache::directory(directory.0.clone());
        assert!(view(&f, &reopened, policy).error.is_none());
        assert_eq!(work::read(Counter::ValidateSignatureRows), before + 1);
        assert!(
            view(&f, &reopened, ContentHash::of_bytes(b"changed policy"))
                .error
                .is_none()
        );
        assert_eq!(work::read(Counter::ValidateSignatureRows), before + 2);
        let signature = f.signatures.get(f.definition.path).unwrap();
        let key = view_key(crate::digest(&session(), signature), policy);
        let store = Store::new(directory.0.clone());
        let association = store.lookup(Address(key)).unwrap();
        let bytes = store.get(association.reference).unwrap();
        let imported = Directory::new();
        Store::new(imported.0.clone())
            .put(VIEW_DOMAIN, &bytes)
            .unwrap();
        assert!(
            view(&f, &VerifierCache::directory(imported.0.clone()), policy)
                .error
                .is_none()
        );
        assert_eq!(work::read(Counter::ValidateSignatureRows), before + 3);
        std::fs::write(store.output_path(association.reference), b"truncated").unwrap();
        assert!(view(&f, &reopened, policy).error.is_none());
        assert_eq!(work::read(Counter::ValidateSignatureRows), before + 4);
    }

    #[test]
    fn signature_receipts_never_accept_producer_digests_or_publish_failure() {
        use hirsel_intern::work::{self, Counter};
        let mut f = Fixture::unit();
        owner_sig(&mut f, &[]);
        let directory = Directory::new();
        let cache = VerifierCache::directory(directory.0.clone());
        let policy = ContentHash::default();
        assert!(view(&f, &cache, policy).error.is_none());
        let signature = f.signatures.get(f.definition.path).unwrap().clone();
        let good_digest = crate::digest(&session(), &signature);
        let mut broken = signature;
        let hirsel_types::SignatureKind::Function(function) = &mut broken.kind else {
            unreachable!()
        };
        function.result =
            session().intern(hirsel_types::Type::with_kind(hirsel_types::TypeKind::Error));
        let before = work::read(Counter::ValidateSignatureRows);
        for _ in 0..2 {
            let verdict = cache.view_verdict(
                &session(),
                f.definition.path,
                &broken,
                good_digest,
                &f.signatures,
                policy,
            );
            assert!(verdict.error.is_some());
        }
        let bad_digest = crate::digest(&session(), &broken);
        assert_ne!(bad_digest, good_digest);
        for _ in 0..2 {
            let verdict = cache.view_verdict(
                &session(),
                f.definition.path,
                &broken,
                bad_digest,
                &f.signatures,
                policy,
            );
            assert!(verdict.error.is_some());
        }
        assert_eq!(work::read(Counter::ValidateSignatureRows), before + 4);
        assert!(
            Store::new(directory.0.clone())
                .lookup(Address(view_key(bad_digest, policy)))
                .is_none()
        );
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

    struct ReplayViews<'a> {
        signatures: Option<&'a dyn SignatureLookup>,
        package: PackageId,
        digest: ContentHash,
        calls: std::cell::Cell<usize>,
    }
    impl ReplayEnvironment for ReplayViews<'_> {
        fn base_digest(&self, package: &PackageId) -> Option<ContentHash> {
            (package == &self.package).then_some(self.digest)
        }
        fn views(
            &self,
            package: &PackageId,
            _: Option<hirsel_types::SignatureRef>,
        ) -> Option<Box<dyn SignatureLookup + '_>> {
            self.calls.set(self.calls.get() + 1);
            self.base_digest(package)?;
            Some(Box::new(self.signatures?))
        }
    }

    #[test]
    fn execution_replay_runs_v_and_refuses_missing_wrong_or_uncovered_contexts() {
        let mut f = Fixture::unit();
        owner_sig(&mut f, &[]);
        f.body().blocks[0].span.anchor = Anchor::fresh();
        let cache = VerifierCache::default();
        let policy = ContentHash::default();
        let (_, _, accepted) = validate_with_receipt(
            &artifact(&f),
            &f.signatures,
            policy,
            Some(&cache),
            &mut vec![],
        );
        let accepted = accepted.unwrap();
        let package = path("main").0.package;
        let old = ContentHash::of_bytes(b"old universe");
        let new = ContentHash::of_bytes(b"new universe");
        let receipt =
            ExecutionReceipt::compose(&[(package.clone(), old)], &[accepted.clone()]).unwrap();
        let subject = ContentHash::of_bytes(b"replay subject");
        cache.publish_execution(subject, &receipt);
        let mut environment = ReplayViews {
            signatures: None,
            package: package.clone(),
            digest: new,
            calls: std::cell::Cell::new(0),
        };
        let anchor = f.definition.bodies[0].blocks[0].span.anchor;
        let replay = |environment: &ReplayViews<'_>| {
            cache.execution_revalidated(
                subject,
                session(),
                |p, n| (*p == path("main") && n == 0).then_some(anchor),
                environment,
            )
        };
        // A retained clone cannot refresh context: V must receive current views.
        assert!(replay(&environment).is_none());
        assert!(cache.execution(subject, |_| new).is_none());
        environment.package.name = "wrong-package".into();
        assert!(replay(&environment).is_none());
        environment.package = package.clone();
        let empty = crate::SignatureMap::new(session());
        environment.signatures = Some(&empty);
        assert!(replay(&environment).is_none());
        assert!(cache.execution(subject, |_| new).is_none());
        environment.signatures = Some(&f.signatures);
        assert!(replay(&environment).is_some());
        assert!(environment.calls.get() >= 2);
        assert!(cache.execution(subject, |_| new).is_some());
        let mut other = package.clone();
        other.name = "uncovered".into();
        let mut contexts = vec![(package, old), (other, old)];
        contexts.sort();
        cache.publish_execution(
            subject,
            &ExecutionReceipt::compose(&contexts, &[accepted]).unwrap(),
        );
        assert!(replay(&environment).is_none());
    }

    #[test]
    fn execution_replay_refuses_a_supplied_but_uncovered_universe() {
        let mut f = Fixture::unit();
        owner_sig(&mut f, &[]);
        let cache = VerifierCache::default();
        let (_, _, accepted) = validate_with_receipt(
            &artifact(&f),
            &f.signatures,
            ContentHash::default(),
            Some(&cache),
            &mut vec![],
        );
        let old = ContentHash::of_bytes(b"old");
        let mut other = path("main").0.package;
        other.name = "other".into();
        let mut contexts = vec![(path("main").0.package, old), (other, old)];
        contexts.sort();
        let subject = ContentHash::of_bytes(b"uncovered subject");
        cache.publish_execution(
            subject,
            &ExecutionReceipt::compose(&contexts, &[accepted.unwrap()]).unwrap(),
        );
        struct Supplied<'a>(&'a dyn SignatureLookup);
        impl ReplayEnvironment for Supplied<'_> {
            fn base_digest(&self, _: &PackageId) -> Option<ContentHash> {
                Some(ContentHash::of_bytes(b"new"))
            }
            fn views(
                &self,
                _: &PackageId,
                _: Option<hirsel_types::SignatureRef>,
            ) -> Option<Box<dyn SignatureLookup + '_>> {
                Some(Box::new(self.0))
            }
        }
        assert!(
            cache
                .execution_revalidated(
                    subject,
                    session(),
                    |_, _| Some(Anchor::default()),
                    &Supplied(&f.signatures)
                )
                .is_none()
        );
        assert!(
            cache
                .execution(subject, |_| ContentHash::of_bytes(b"new"))
                .is_none()
        );
    }

    #[test]
    fn execution_replay_heap_provider_equivalence_requires_all_old_views_and_complete_role() {
        use crate::support::{arena_slot, function_signature};
        let mut f = Fixture::call();
        arena_slot(&mut f, hirsel_tir::Storage::Temporary);
        owner_sig(&mut f, &[]);
        let cache = VerifierCache::default();
        let policy = ContentHash::default();
        let mut reads = vec![];
        let (report, _, accepted) = validate_with_receipt(
            &artifact(&f),
            &f.signatures,
            policy,
            Some(&cache),
            &mut reads,
        );
        assert!(report.violations.is_empty(), "{report:?}");
        assert!(
            reads.contains(&VRead::FirstHeap(Some(hirsel_syntax::DefId::intern(
                &path("Arena")
            ))))
        );
        let accepted = accepted.unwrap();
        let package = path("main").0.package;
        let old = ContentHash::of_bytes(b"original heap universe");
        let new = ContentHash::of_bytes(b"earlier equal provider universe");
        let subject = ContentHash::of_bytes(b"heap replay subject");
        let receipt = ExecutionReceipt::compose(&[(package.clone(), old)], &[accepted]).unwrap();
        let mut current = f.signatures.clone();
        let mut earlier = function_signature(
            "0provider",
            vec![],
            hirsel_types::TypeKind::Tuple(vec![].into()),
        );
        earlier.metadata.roles = current.get(path("Arena")).unwrap().metadata.roles.clone();
        current.insert(earlier.definition, earlier);
        struct MissingHeap<'a>(&'a dyn SignatureLookup);
        impl SignatureLookup for MissingHeap<'_> {
            fn view(
                &self,
                d: hirsel_syntax::DefId,
            ) -> Option<(&hirsel_types::Signature, crate::ValidationDigest)> {
                self.0.view(d)
            }
            fn first_heap(&self) -> Option<(hirsel_syntax::DefId, hirsel_types::HeapRole)> {
                None
            }
            fn universe_errors(&self) -> (ContentHash, &[crate::Violation]) {
                self.0.universe_errors()
            }
        }
        let attempt = |signatures: &dyn SignatureLookup| {
            cache.publish_execution(subject, &receipt);
            cache.execution_revalidated(
                subject,
                session(),
                |_, _| Some(Anchor::default()),
                &ReplayViews {
                    signatures: Some(signatures),
                    package: package.clone(),
                    digest: new,
                    calls: std::cell::Cell::new(0),
                },
            )
        };
        // The ordinary read guard stays exact. Only verifier replay compares
        // the complete role and validates the retained artifact again.
        let old_heap = VRead::FirstHeap(Some(hirsel_syntax::DefId::intern(&path("Arena"))));
        assert!(!read_holds(&old_heap, &current));
        assert!(attempt(&MissingHeap(&current)).is_none());
        for field in ["arena", "operation"] {
            let mut changed = current.clone();
            let role = changed
                .get_mut(path("0provider"))
                .unwrap()
                .metadata
                .roles
                .heap
                .as_mut()
                .unwrap();
            if field == "arena" {
                role.arena = path("DifferentArena");
            } else {
                role.operation = path("different_operation");
            }
            assert!(attempt(&changed).is_none(), "changed {field}");
        }
        let mut missing = current.clone();
        missing.remove(path("Arena"));
        assert!(attempt(&missing).is_none());
        let mut stale = current.clone();
        stale.get_mut(path("Arena")).unwrap().metadata.visibility =
            hirsel_syntax::Visibility::Public;
        assert!(attempt(&stale).is_none());
        assert!(attempt(&current).is_some());
        assert!(cache.execution(subject, |_| new).is_some());
    }

    #[test]
    fn private_overlay_identity_and_replay_associations_are_verifier_owned() {
        let mut f = Fixture::unit();
        owner_sig(&mut f, &[]);
        f.body().blocks[0].span.anchor = Anchor::fresh();
        let directory = Directory::new();
        let cache = VerifierCache::directory(directory.0.clone());
        let signature = std::sync::Arc::new(f.signatures.get(f.definition.path).unwrap().clone());
        let exact = artifact(&f).with_overlay(signature.clone());
        let mut changed = (*signature).clone();
        changed.metadata.visibility = hirsel_syntax::Visibility::Public;
        assert_ne!(
            exact.identity,
            artifact(&f)
                .with_overlay(std::sync::Arc::new(changed))
                .identity
        );
        let policy = ContentHash::default();
        let (_, _, accepted) =
            validate_with_receipt(&exact, &f.signatures, policy, Some(&cache), &mut vec![]);
        let accepted = accepted.unwrap();
        let anchor = f.definition.bodies[0].blocks[0].span.anchor;
        assert!(
            cache
                .replay_artifact(&accepted, session(), |_, _| Some(anchor))
                .is_some()
        );
        assert!(
            cache
                .replay_artifact(&accepted, session(), |_, _| None)
                .is_none()
        );
        let source = Store::new(directory.0.clone());
        let association = source
            .lookup(Address(replay_key(accepted.artifact, accepted.policy)))
            .unwrap();
        let bytes = source.get(association.reference).unwrap();
        let imported = Directory::new();
        Store::new(imported.0.clone())
            .put(REPLAY_DOMAIN, &bytes)
            .unwrap();
        assert!(
            VerifierCache::directory(imported.0.clone())
                .replay_artifact(&accepted, session(), |_, _| Some(anchor))
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

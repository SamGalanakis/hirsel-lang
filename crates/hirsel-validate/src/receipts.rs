//! Local verifier authority, separate from producer query associations. Imported
//! content can share the physical store but cannot publish through this API.
use crate::{SignatureLookup, VRead, Validation, read_holds};
mod installation;
mod outputs;
use bincode::Options;
use hirsel_artifact_store::{Action, Address, Domain, Store};
use hirsel_span::Anchor;
use hirsel_syntax::{ContentHash, DefPath, PackageId};
use hirsel_tir::Definition;
use hirsel_types::TypeInterner;
pub use installation::Installation;
pub use outputs::{
    SourceInvocation, StoredDefinitionOutput, VerifiedDefinitionOutput, VerifiedInvocations,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{Arc, Mutex, Weak},
};

/// Local evidence for one immutable source Definition, minted only by V.
/// A weak binding preserves retention limits; copying reads cannot create it.
/// This capability is deliberately neither serializable nor deserializable.
#[derive(Debug)]
pub struct VerifiedDefinition {
    definition: Weak<Definition>,
    owner: hirsel_syntax::DefId,
    types: Weak<TypeInterner>,
    policy: ContentHash,
    accepted: Option<AcceptedArtifact>,
    reads: Vec<VRead>,
    environment: Mutex<Weak<()>>,
}
impl Clone for VerifiedDefinition {
    fn clone(&self) -> Self {
        Self {
            definition: self.definition.clone(),
            owner: self.owner,
            types: self.types.clone(),
            policy: self.policy,
            accepted: self.accepted.clone(),
            reads: self.reads.clone(),
            environment: Mutex::new(self.environment.lock().unwrap().clone()),
        }
    }
}
impl VerifiedDefinition {
    pub fn holds<'a>(
        &self,
        definition: &hirsel_tir::DefinitionRef,
        types: &Arc<TypeInterner>,
        policy: ContentHash,
        signatures: impl Into<crate::SignatureInputs<'a>>,
    ) -> bool {
        self.definition.ptr_eq(&Arc::downgrade(definition))
            && self.types.ptr_eq(&Arc::downgrade(types))
            && self.environment_holds(policy, signatures)
    }
    /// A clean stamp is scoped to V's exact immutable snapshot. Every new
    /// snapshot and every general lookup checks the complete observations.
    pub fn environment_holds<'a>(
        &self,
        policy: ContentHash,
        signatures: impl Into<crate::SignatureInputs<'a>>,
    ) -> bool {
        if self.policy != policy {
            return false;
        }
        let signatures = signatures.into();
        let Some(types) = self.types.upgrade() else {
            return false;
        };
        if let Some(stamp) = signatures.stamp() {
            if !Arc::ptr_eq(&types, &stamp.types) {
                return false;
            }
            let mut seen = self.environment.lock().unwrap();
            if seen.ptr_eq(&Arc::downgrade(&stamp.identity)) {
                return true;
            }
            if !self
                .reads
                .iter()
                .all(|r| crate::input::read_holds_for_types(&types, r, signatures.lookup()))
            {
                return false;
            }
            *seen = Arc::downgrade(&stamp.identity);
            true
        } else {
            self.reads
                .iter()
                .all(|r| crate::input::read_holds_for_types(&types, r, signatures.lookup()))
        }
    }
    pub fn owner(&self) -> hirsel_syntax::DefId {
        self.owner
    }
    pub fn reads(&self) -> &Vec<VRead> {
        &self.reads
    }
    /// The portable acceptance issued with this evidence. Its consumer must
    /// first check the exact immutable artifact and current environment.
    pub fn accepted_artifact(&self) -> Option<AcceptedArtifact> {
        self.accepted.clone()
    }
}

/// The ordinary receipt query, additionally binding local evidence to the
/// immutable input. Misses run the existing validator exactly once.
pub fn validate_definition<'a>(
    definition: &hirsel_tir::DefinitionRef,
    types: Arc<TypeInterner>,
    owner: impl FnMut(Anchor) -> Option<(DefPath, u32)>,
    signatures: impl Into<crate::SignatureInputs<'a>>,
    policy: ContentHash,
    cache: Option<&VerifierCache>,
) -> (
    Validation,
    bool,
    Option<AcceptedArtifact>,
    Option<VerifiedDefinition>,
) {
    let signatures = signatures.into();
    let mut reads = vec![];
    let artifact = Artifact::new(definition, types.clone(), owner);
    let (report, ran, accepted) =
        validate_with_receipt(&artifact, signatures.lookup(), policy, cache, &mut reads);
    let evidence = report.violations.is_empty().then(|| VerifiedDefinition {
        definition: Arc::downgrade(definition),
        owner: definition.path,
        types: Arc::downgrade(&types),
        policy,
        accepted: accepted.clone(),
        reads,
        environment: Mutex::new(
            signatures
                .stamp()
                .map_or_else(Weak::new, |stamp| Arc::downgrade(&stamp.identity)),
        ),
    });
    (report, ran, accepted, evidence)
}

const VIEW_DOMAIN: Domain = Domain {
    kind: 0x5600_0003,
    schema: 2,
};
fn view_key(digest: crate::ValidationDigest, policy: ContentHash) -> ContentHash {
    ContentHash::of_parts(
        "hirsel.validate.signature-receipt.v2",
        [digest.0.0.as_slice(), policy.0.as_slice()],
    )
}

const DOMAIN: Domain = Domain {
    kind: 0x5600_0001,
    schema: 2,
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
        owner: impl FnMut(Anchor) -> Option<(DefPath, u32)>,
    ) -> Self {
        Self::identify(definition, types, owner, None)
    }
    /// Identify a private artifact and its exact replacement signature in one
    /// pass. The ordinary body identity is not an intermediate product.
    pub fn private(
        definition: &'a Definition,
        types: Arc<TypeInterner>,
        signature: hirsel_types::SignatureRef,
        owner: impl FnMut(Anchor) -> Option<(DefPath, u32)>,
    ) -> Self {
        Self::identify(definition, types, owner, Some(signature))
    }
    fn identify(
        definition: &'a Definition,
        types: Arc<TypeInterner>,
        mut owner: impl FnMut(Anchor) -> Option<(DefPath, u32)>,
        overlay: Option<hirsel_types::SignatureRef>,
    ) -> Self {
        let mut owners = std::collections::BTreeMap::new();
        let mut bind = |anchor| {
            let value = owner(anchor)?;
            owners.insert(anchor, value.clone());
            Some(value)
        };
        let (domain, bytes) = if overlay.is_some() {
            (
                "hirsel.validate.private-artifact.v2",
                hirsel_product_codec::canonical_bytes(
                    types.clone(),
                    &(definition, &overlay),
                    &mut bind,
                ),
            )
        } else {
            (
                "hirsel.validate.artifact.v1",
                hirsel_product_codec::canonical_bytes(types.clone(), definition, &mut bind),
            )
        };
        let identity = bytes.map(|bytes| ContentHash::of_parts(domain, [bytes.as_slice()]));
        Self {
            definition,
            types,
            identity,
            owners,
            overlay,
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
struct Receipt {
    action: ContentHash,
    artifact: ContentHash,
    policy: ContentHash,
    reads: Vec<VRead>,
}
fn receipt_action(artifact: ContentHash, policy: ContentHash, reads: &[VRead]) -> ContentHash {
    // VRead's DefIds encode stable DefPaths; order is V's canonical read order.
    let encoded = bincode::serialize(reads).expect("verifier reads are encodable");
    ContentHash::of_parts(
        "hirsel.validate.action.v2",
        [
            artifact.0.as_slice(),
            policy.0.as_slice(),
            encoded.as_slice(),
        ],
    )
}
#[derive(Serialize, Deserialize)]
struct ViewReceipt {
    digest: crate::ValidationDigest,
    policy: ContentHash,
    reads: Vec<VRead>,
}
enum Backend {
    Memory(
        Mutex<
            HashMap<
                ContentHash,
                (
                    hirsel_artifact_store::Reference,
                    hirsel_artifact_store::Bytes,
                ),
            >,
        >,
        Arc<hirsel_artifact_store::Memory>,
    ),
    Directory(Store),
    Hosted(
        Arc<dyn VerifierRows>,
        Arc<dyn hirsel_artifact_store::Contents>,
    ),
    Registered(Arc<VerifierCache>, Arc<installation::Rows>),
    Transport(
        Arc<VerifierCache>,
        Arc<dyn VerifierRows>,
        Arc<TransportContents>,
    ),
}

struct TransportContents {
    selected: Arc<dyn hirsel_artifact_store::Contents>,
    request: Arc<dyn hirsel_artifact_store::Contents>,
}
impl hirsel_artifact_store::Contents for TransportContents {
    fn get_content(
        &self,
        reference: hirsel_artifact_store::Reference,
    ) -> Option<hirsel_artifact_store::Bytes> {
        if let Some(bytes) = self.request.get_content(reference) {
            return Some(bytes);
        }
        let bytes = self.selected.get_content(reference)?;
        (self.request.put_content(reference.domain, &bytes)? == reference).then_some(bytes)
    }
    fn put_content(
        &self,
        domain: Domain,
        bytes: &[u8],
    ) -> Option<hirsel_artifact_store::Reference> {
        let reference = domain.reference(bytes)?;
        if self.request.put_content(domain, bytes)? != reference {
            return None;
        }
        let _ = self.selected.put_content(domain, bytes);
        Some(reference)
    }
}
/// A host-owned, bounded home for verifier rows and their content. Only this
/// cache publishes rows, so product bytes in the same host cannot supply them.
/// Either may be dropped by the host's bound; a missing row is a miss.
pub trait VerifierRows: hirsel_artifact_store::Contents {
    fn publication_batch(&self) -> Option<hirsel_artifact_store::Batch> {
        None
    }
    fn publication_request(&self) -> Option<hirsel_artifact_store::Request> {
        None
    }
    fn row(&self, key: ContentHash) -> Option<hirsel_artifact_store::Reference>;
    fn set_row(&self, key: ContentHash, reference: hirsel_artifact_store::Reference);
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
    /// Keep the trusted namespace while giving an independent compiler
    /// operation its own physical epoch. A shared ordinary root joins the
    /// supplied operation handle; transported/hosted owners already have one.
    pub fn for_operation(self: &Arc<Self>, shared: Option<&Store>) -> Arc<Self> {
        match &self.0 {
            Backend::Directory(store) => Arc::new(Self::directory_store(
                shared
                    .filter(|shared| shared.root() == store.root())
                    .cloned()
                    .unwrap_or_else(|| store.for_operation()),
            )),
            // DirStore reselects a registration against the current installed
            // generation for each operation. Do not retain the previous pin.
            Backend::Registered(local, _) => local.for_operation(shared),
            _ => self.clone(),
        }
    }
    /// Use the same immutable output containers as ordinary query products.
    /// The verifier's domain and address associations remain separately owned.
    pub fn directory(root: PathBuf) -> Self {
        Self::directory_store(Store::rebuildable(root))
    }
    /// Share physical publication state with ordinary products in the selected
    /// trusted local directory. This does not import verifier associations.
    pub fn directory_store(store: Store) -> Self {
        Self(Backend::Directory(store), Mutex::default())
    }
    /// Keep rows, receipts and type records in the host's bounded storage.
    pub fn hosted<T: VerifierRows + 'static>(rows: Arc<T>) -> Self {
        Self(Backend::Hosted(rows.clone(), rows), Mutex::default())
    }

    /// Keep this request's verifier content through best-effort cache eviction.
    /// Association lookup and publication remain in the selected verifier;
    /// transport bytes grant no authority and use the caller's existing bound.
    pub fn with_transport<T: VerifierRows + 'static>(
        self: &Arc<Self>,
        request: Arc<T>,
    ) -> Arc<Self> {
        Arc::new(Self(
            Backend::Transport(
                self.clone(),
                request.clone(),
                Arc::new(TransportContents {
                    selected: self.contents(),
                    request,
                }),
            ),
            Mutex::default(),
        ))
    }

    pub(crate) fn publication_batch(&self) -> Option<hirsel_artifact_store::Batch> {
        match &self.0 {
            Backend::Directory(store) => Some(store.batch()),
            Backend::Transport(selected, _, _) | Backend::Registered(selected, _) => {
                selected.publication_batch()
            }
            Backend::Hosted(rows, _) => rows.publication_batch(),
            Backend::Memory(..) => None,
        }
    }
    /// Join an admitted compiler operation's bounded cache publication epoch.
    /// The guard supplies no verification authority. The caller joins its
    /// producers before finishing it; ordinary bytes cannot seed V's rows.
    pub fn publication_request(&self) -> Option<hirsel_artifact_store::Request> {
        match &self.0 {
            Backend::Directory(store) => Some(store.request()),
            Backend::Transport(selected, _, _) | Backend::Registered(selected, _) => {
                selected.publication_request()
            }
            Backend::Hosted(rows, _) => rows.publication_request(),
            _ => None,
        }
    }

    fn load<T: serde::de::DeserializeOwned>(&self, key: ContentHash, domain: Domain) -> Option<T> {
        self.load_record(key, domain).map(|(_, value)| value)
    }
    /// The reference comes from this verifier's own association and the same
    /// checked bytes that are decoded below, never from producer metadata.
    fn load_record<T: serde::de::DeserializeOwned>(
        &self,
        key: ContentHash,
        domain: Domain,
    ) -> Option<(hirsel_artifact_store::Reference, T)> {
        let (reference, bytes) = self.row_content(key, domain)?;
        let value = bincode::DefaultOptions::new()
            .with_fixint_encoding()
            .with_limit(bytes.len() as u64)
            .reject_trailing_bytes()
            .deserialize(&bytes)
            .ok()?;
        Some((reference, value))
    }
    fn row_content(
        &self,
        key: ContentHash,
        domain: Domain,
    ) -> Option<(
        hirsel_artifact_store::Reference,
        hirsel_artifact_store::Bytes,
    )> {
        let (reference, bytes) = match &self.0 {
            Backend::Memory(rows, _) => rows
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .get(&key)
                .cloned()?,
            Backend::Directory(store) => {
                let opened = store.lookup_open(Address(key))?;
                let association = opened.association;
                if association.action != Action(key) || association.reference.domain != domain {
                    return None;
                }
                let bytes = opened.get()?;
                store.touch(Address(key), association.reference);
                (association.reference, bytes)
            }
            Backend::Hosted(rows, contents) => {
                let reference = rows.row(key)?;
                (reference, contents.get_content(reference)?)
            }
            Backend::Registered(local, installed) => {
                if let Some(value) = local.row_content(key, domain) {
                    return Some(value);
                }
                let reference = installed.row(key)?;
                (
                    reference,
                    hirsel_artifact_store::Contents::get_content(&**installed, reference)?,
                )
            }
            Backend::Transport(selected, rows, contents) => {
                if let Some(reference) = rows.row(key) {
                    (reference, contents.request.get_content(reference)?)
                } else {
                    let (reference, bytes) = selected.row_content(key, domain)?;
                    if contents.request.put_content(domain, &bytes)? != reference {
                        return None;
                    }
                    rows.set_row(key, reference);
                    (reference, bytes)
                }
            }
        };
        if reference.domain != domain {
            return None;
        }
        Some((reference, bytes))
    }
    fn publish<T: Serialize>(&self, key: ContentHash, domain: Domain, receipt: &T) {
        let Ok(bytes) = bincode::serialize(receipt) else {
            return;
        };
        if bytes.len() > hirsel_artifact_store::MAX_RECORD {
            return;
        }
        self.publish_bytes(key, domain, &bytes);
    }
    fn publish_bytes(&self, key: ContentHash, domain: Domain, bytes: &[u8]) {
        match &self.0 {
            Backend::Memory(rows, contents) => {
                use hirsel_artifact_store::Contents;
                let Some(reference) = contents.put_content(domain, &bytes) else {
                    return;
                };
                let Some(bytes) = contents.get_content(reference) else {
                    return;
                };
                rows.lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .insert(key, (reference, bytes));
            }
            Backend::Directory(store) => {
                let _batch = store.batch();
                if let Ok(reference) = store.put(domain, &bytes) {
                    let _ = store.associate(Address(key), Action(key), reference);
                }
            }
            Backend::Hosted(rows, contents) => {
                if let Some(reference) = contents.put_content(domain, &bytes) {
                    rows.set_row(key, reference);
                }
            }
            Backend::Registered(local, _) => local.publish_bytes(key, domain, bytes),
            Backend::Transport(selected, rows, contents) => {
                let Some(reference) = domain.reference(bytes) else {
                    return;
                };
                if contents.request.put_content(domain, bytes) != Some(reference) {
                    return;
                }
                rows.set_row(key, reference);
                selected.publish_bytes(key, domain, bytes);
            }
        }
    }
    /// Only V publishes terminal successful signature checks. A local hit
    /// independently observes the exact view digest and every contextual read.
    pub(crate) fn view_verdict(
        &self,
        types: &TypeInterner,
        definition: hirsel_syntax::DefId,
        input: crate::ViewInput<'_>,
        signatures: &dyn SignatureLookup,
        policy: ContentHash,
    ) -> Arc<crate::ViewVerdict> {
        if !input.for_types(types) {
            return Arc::new(crate::ViewVerdict {
                error: Some(crate::Violation {
                    body: None,
                    block: None,
                    instruction: None,
                    kind: crate::ViolationKind::InvalidSignature(definition.path().clone()),
                }),
                reads: vec![],
            });
        }
        let (signature, digest) = input.into_parts();
        let key = view_key(digest, policy);
        if let Some(receipt) = self.load::<ViewReceipt>(key, VIEW_DOMAIN)
            && receipt.digest == digest
            && receipt.policy == policy
            && receipt
                .reads
                .iter()
                .all(|read| crate::input::read_holds_for_types(types, read, signatures))
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
            Backend::Directory(store) => Arc::new(store.clone()),
            Backend::Hosted(_, contents) => contents.clone(),
            Backend::Transport(_, _, contents) => contents.clone(),
            Backend::Registered(local, installed) => Arc::new(installation::Contents {
                local: local.contents(),
                installed: installed.clone(),
            }),
        }
    }
    fn records(&self, types: Arc<TypeInterner>) -> Arc<hirsel_product_codec::TypeRecords> {
        // Hosted storage bounds bytes only; a cached codec would keep its type
        // maps and interner alive beyond the host's bound and revision.
        if matches!(
            &self.0,
            Backend::Hosted(..) | Backend::Transport(..) | Backend::Registered(..)
        ) {
            return hirsel_product_codec::TypeRecords::new(types, self.contents());
        }
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
        let _batch = self.publication_batch();
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
        let Some(body) = records
            .encode_tir(artifact.definition, |anchor| {
                artifact.owners.get(&anchor).cloned()
            })
            .and_then(|bytes| {
                self.contents()
                    .put_content(hirsel_product_codec::CheckedTir::DOMAIN, &bytes)
            })
        else {
            return;
        };
        let Some(bytes) = records.encode_record(&artifact.overlay, |anchor| {
            artifact.owners.get(&anchor).cloned()
        }) else {
            return;
        };
        self.publish(
            replay_key(identity, policy),
            REPLAY_DOMAIN,
            &ReplayRecord {
                artifact: identity,
                policy,
                body,
                bytes,
            },
        );
    }
    fn replay_rows(
        &self,
        record: &ReplayRecord,
        types: Arc<TypeInterner>,
        mut anchor: impl FnMut(&DefPath, u32) -> Option<Anchor>,
        owner: Arc<dyn Send + Sync>,
    ) -> Option<ReplayRows> {
        let records = self.records(types);
        let mut owners = std::collections::BTreeMap::new();
        let mut bind = |path: &DefPath, occurrence| {
            let site = anchor(path, occurrence)?;
            owners.insert(site, (path.clone(), occurrence));
            Some(site)
        };
        let archive = hirsel_product_codec::CheckedTir::open(
            self.contents()
                .get_content(record.body)?
                .retain_owner(owner),
            record.body,
            records.clone(),
            &mut bind,
        )?;
        let overlay = records.decode_record(&record.bytes, &mut bind)?;
        Some(ReplayRows {
            archive,
            overlay,
            owners,
        })
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
        let ReplayRows {
            archive,
            overlay,
            owners,
        } = self.replay_rows(&record, types.clone(), &mut anchor, Arc::new(()))?;
        let definition = Arc::new(archive.materialize()?);
        if overlay
            .as_ref()
            .is_some_and(|s| s.definition != definition.path)
        {
            return None;
        }
        // Decode already bound every anchor and type row. Identify that exact
        // immutable closure once, including its overlay, before retaining it.
        let identity = match &overlay {
            Some(_) => hirsel_product_codec::canonical_bytes(
                types.clone(),
                &(&definition, &overlay),
                |anchor| owners.get(&anchor).cloned(),
            )
            .map(|bytes| {
                ContentHash::of_parts("hirsel.validate.private-artifact.v2", [bytes.as_slice()])
            }),
            None => hirsel_product_codec::canonical_bytes(types, &definition, |anchor| {
                owners.get(&anchor).cloned()
            })
            .map(|bytes| ContentHash::of_parts("hirsel.validate.artifact.v1", [bytes.as_slice()])),
        };
        if identity != Some(accepted.artifact) {
            return None;
        }
        Some(ReplayArtifact {
            definition,
            overlay,
            identity: accepted.artifact,
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
                    "hirsel.validate.receipt.v2",
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
                                    .and_then(|view| view.signature().metadata.roles.heap.as_ref());
                                let current = views.first_heap().map(|(_, role)| role);
                                old.is_some() && old == current.as_ref()
                            }
                            _ => false,
                        }
                    })
                {
                    return None;
                }

                // replay_artifact independently identified this decoded
                // closure. Its immutable fields have not changed since then.
                let exact = Artifact {
                    definition: &replay.definition,
                    types: types.clone(),
                    identity: Some(replay.identity),
                    owners: replay.owners,
                    overlay: replay.overlay,
                };
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
    schema: 3,
};
fn replay_key(artifact: ContentHash, policy: ContentHash) -> ContentHash {
    ContentHash::of_parts(
        "hirsel.validate.replay.v3",
        [artifact.0.as_slice(), policy.0.as_slice()],
    )
}
#[derive(Serialize, Deserialize)]
struct ReplayRecord {
    artifact: ContentHash,
    policy: ContentHash,
    body: hirsel_artifact_store::Reference,
    #[serde(
        serialize_with = "hirsel_product_codec::serialize_payload",
        deserialize_with = "hirsel_product_codec::deserialize_payload"
    )]
    bytes: Vec<u8>,
}
struct ReplayRows {
    archive: hirsel_product_codec::CheckedTir,
    overlay: Option<hirsel_types::SignatureRef>,
    owners: std::collections::BTreeMap<Anchor, (DefPath, u32)>,
}
/// Construction is verifier-private. These bytes alone are not acceptance.
struct ReplayArtifact {
    definition: hirsel_tir::DefinitionRef,
    overlay: Option<hirsel_types::SignatureRef>,
    identity: ContentHash,
    policy: ContentHash,
    owners: std::collections::BTreeMap<Anchor, (DefPath, u32)>,
}
const EXECUTION_DOMAIN: Domain = Domain {
    kind: 0x5600_0002,
    schema: 2,
};
fn execution_key(subject: ContentHash) -> ContentHash {
    ContentHash::of_parts("hirsel.validate.execution.v2", [subject.0.as_slice()])
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
    /// Actual compact context/artifact ownership, without source body bytes.
    pub fn retained_bytes(&self) -> Option<u64> {
        let slots = (std::mem::size_of::<Self>() + 4 * std::mem::size_of::<usize>()) as u64;
        slots
            .checked_add(
                (self.contexts.len() as u64)
                    .checked_mul(std::mem::size_of::<(PackageId, ContentHash)>() as u64)?,
            )?
            .checked_add(bincode::serialized_size(self.contexts.as_ref()).ok()?)?
            .checked_add(
                (self.artifacts.len() as u64)
                    .checked_mul(std::mem::size_of::<AcceptedArtifact>() as u64)?,
            )
    }
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
            "hirsel.validate.receipt.v2",
            [identity.0.as_slice(), policy.0.as_slice()],
        )
    });
    if let (Some(cache), Some(key), Some(identity)) = (cache, key, artifact.identity)
        && let Some(receipt) = cache.load::<Receipt>(key, DOMAIN)
        && receipt.artifact == identity
        && receipt.policy == policy
        && receipt.action == receipt_action(identity, policy, &receipt.reads)
        && receipt
            .reads
            .iter()
            .all(|read| crate::input::read_holds_for_types(&artifact.types, read, signatures))
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
                action: receipt_action(identity, policy, &own),
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

    #[test]
    fn replay_record_bulk_overlay_bytes_preserve_sequence_encoding_and_decode_limits() {
        #[derive(Serialize, Deserialize)]
        struct Original {
            artifact: ContentHash,
            policy: ContentHash,
            body: hirsel_artifact_store::Reference,
            bytes: Vec<u8>,
        }
        let fixture = Fixture::unit();
        let cache = VerifierCache::default();
        let body = cache
            .records(session())
            .encode_tir(&fixture.definition, |_| Some((path("main"), 0)))
            .unwrap();
        let body = cache
            .contents()
            .put_content(hirsel_product_codec::CheckedTir::DOMAIN, &body)
            .unwrap();
        let original = Original {
            artifact: ContentHash::of_bytes(b"artifact"),
            policy: ContentHash::of_bytes(b"policy"),
            body,
            bytes: (0..=255).cycle().take(4096).collect(),
        };
        let record = ReplayRecord {
            artifact: original.artifact,
            policy: original.policy,
            body: original.body,
            bytes: original.bytes.clone(),
        };
        let bytes = bincode::serialize(&original).unwrap();
        assert_eq!(bincode::serialize(&record).unwrap(), bytes);
        let limit = bytes.len() as u64;
        let options = || {
            bincode::DefaultOptions::new()
                .with_fixint_encoding()
                .with_limit(limit)
                .reject_trailing_bytes()
        };
        let decoded: ReplayRecord = options().deserialize(&bytes).unwrap();
        assert_eq!(decoded.bytes, original.bytes);
        assert_eq!(decoded.artifact, original.artifact);
        assert_eq!(decoded.policy, original.policy);
        assert!(
            options()
                .deserialize::<ReplayRecord>(&bytes[..bytes.len() - 1])
                .is_err()
        );
        let mut trailing = bytes.clone();
        trailing.push(0);
        assert!(options().deserialize::<ReplayRecord>(&trailing).is_err());
        // An impossible payload length must fail before an allocation/copy.
        let mut invalid = bytes;
        let count_at = bincode::serialized_size(&(record.artifact, record.policy, record.body))
            .unwrap() as usize;
        invalid[count_at..count_at + 8].copy_from_slice(&u64::MAX.to_le_bytes());
        assert!(options().deserialize::<ReplayRecord>(&invalid).is_err());
    }

    #[test]
    fn verifier_type_content_joins_its_own_publication_batch() {
        let dir = Directory::new();
        let cache = VerifierCache::directory(dir.0.clone());
        let batch = cache.publication_batch().unwrap();
        let contents = cache.contents();
        let reader = Store::new(dir.0.clone());
        let mut references = vec![];
        for n in 0u32..8 {
            let reference = contents.put_content(VIEW_DOMAIN, &n.to_le_bytes()).unwrap();
            assert!(contents.get_content(reference).is_some());
            assert!(
                reader.get(reference).is_none(),
                "type content escaped its batch"
            );
            references.push(reference);
        }
        batch.finish().unwrap();
        for (n, reference) in references.into_iter().enumerate() {
            assert_eq!(&*reader.get(reference).unwrap(), &(n as u32).to_le_bytes());
        }
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
            "hirsel.validate.receipt.v2",
            [artifact.identity.unwrap().0.as_slice(), policy.0.as_slice()],
        )
    }

    fn view(f: &Fixture, cache: &VerifierCache, policy: ContentHash) -> Arc<crate::ViewVerdict> {
        let definition = f.definition.path;
        let signature = f.signatures.get(definition).unwrap();
        cache.view_verdict(
            &session(),
            definition,
            crate::IdentifiedSignature::new(session(), Arc::new(signature.clone())).input(),
            &f.signatures,
            policy,
        )
    }

    #[test]
    fn cached_nested_body_and_private_type_mutations_require_full_v() {
        let mut f = crate::support::captured_generic_copy(true, false, false);
        let cache = VerifierCache::default();
        let policy = ContentHash::default();
        let (report, ran) = check(&f, &cache, policy);
        assert!(ran && report.violations.is_empty(), "{report:?}");
        assert!(!check(&f, &cache, policy).1);
        let original = f.definition.bodies[1].clone();
        let public = f.signatures.get(f.definition.path).unwrap().fingerprint;
        let hirsel_tir::Terminator::Return { value, .. } =
            &mut f.definition.bodies[1].blocks[0].terminator
        else {
            panic!()
        };
        value.place = hirsel_tir::PlaceId(u32::MAX);
        let (report, ran) = check(&f, &cache, policy);
        assert!(ran && !report.violations.is_empty());
        f.definition.bodies[1] = original;
        f.definition.bodies[1].result =
            session().intern(hirsel_types::Type::with_kind(hirsel_types::TypeKind::Error));
        assert_eq!(
            f.signatures.get(f.definition.path).unwrap().fingerprint,
            public
        );
        let (report, ran) = check(&f, &cache, policy);
        assert!(ran && !report.violations.is_empty());
    }

    #[test]
    fn hosted_verifier_retains_no_type_records_beyond_its_callers() {
        #[derive(Default)]
        struct Rows(
            hirsel_artifact_store::Memory,
            Mutex<HashMap<ContentHash, hirsel_artifact_store::Reference>>,
        );
        impl hirsel_artifact_store::Contents for Rows {
            fn get_content(
                &self,
                reference: hirsel_artifact_store::Reference,
            ) -> Option<hirsel_artifact_store::Bytes> {
                self.0.get_content(reference)
            }
            fn put_content(
                &self,
                domain: Domain,
                bytes: &[u8],
            ) -> Option<hirsel_artifact_store::Reference> {
                self.0.put_content(domain, bytes)
            }
        }
        impl VerifierRows for Rows {
            fn row(&self, key: ContentHash) -> Option<hirsel_artifact_store::Reference> {
                self.1.lock().unwrap().get(&key).copied()
            }
            fn set_row(&self, key: ContentHash, reference: hirsel_artifact_store::Reference) {
                self.1.lock().unwrap().insert(key, reference);
            }
        }
        let cache = VerifierCache::hosted(Arc::new(Rows::default()));
        let types = session();
        let baseline = Arc::strong_count(&types);
        let records = cache.records(types.clone());
        assert!(Arc::strong_count(&types) > baseline);
        drop(records);
        assert_eq!(Arc::strong_count(&types), baseline, "no hosted codec cache");
        let directory = Directory::new();
        let cached = VerifierCache::directory(directory.0.clone());
        drop(cached.records(types.clone()));
        assert!(
            Arc::strong_count(&types) > baseline,
            "directory caches keep their codec"
        );
    }

    #[test]
    fn equal_digest_from_a_foreign_type_universe_cannot_reuse_a_local_signature_verdict() {
        use crate::SignatureLookup;
        let mut f = Fixture::unit();
        owner_sig(&mut f, &[]);
        let types = session();
        let base = hirsel_types::TypeBase::build(types.len(), |_| {
            types.ids().map(|id| types.row(id).clone()).collect()
        });
        let foreign = crate::IdentifiedSignature::new(
            TypeInterner::new(Some(base)),
            Arc::new(f.signatures.get(f.definition.path).unwrap().clone()),
        );
        let original = f.signatures.view(f.definition.path).unwrap();
        assert_eq!(original.digest(), foreign.digest());
        struct Views<'a> {
            foreign: crate::IdentifiedSignature,
            original: &'a crate::SignatureMap,
        }
        impl SignatureLookup for Views<'_> {
            fn view(&self, d: hirsel_syntax::DefId) -> Option<crate::ViewInput<'_>> {
                if d == self.foreign.signature().definition {
                    Some(self.foreign.input())
                } else {
                    self.original.view(d)
                }
            }
            fn first_heap(&self) -> Option<(hirsel_syntax::DefId, hirsel_types::HeapRole)> {
                self.original.first_heap()
            }
            fn universe_errors(&self) -> (ContentHash, &[crate::Violation]) {
                self.original.universe_errors()
            }
        }
        let mut verdicts = crate::ValidatedSignatures::default();
        assert!(
            verdicts
                .verdict(&types, f.definition.path, &f.signatures)
                .error
                .is_none()
        );
        let views = Views {
            foreign,
            original: &f.signatures,
        };
        let (report, _, _, evidence) = validate_definition(
            &Arc::new(f.definition.clone()),
            types.clone(),
            |_| Some((path("main"), 0)),
            &views,
            ContentHash::default(),
            None,
        );
        assert!(
            !report.violations.is_empty(),
            "foreign owner must not be treated as optional absence"
        );
        assert!(evidence.is_none());
        assert!(
            verdicts
                .verdict(&types, f.definition.path, &views)
                .error
                .is_some()
        );
        assert!(
            VerifierCache::default()
                .view_verdict(
                    &types,
                    f.definition.path,
                    views.view(f.definition.path).unwrap(),
                    &views,
                    ContentHash::default()
                )
                .error
                .is_some()
        );
    }

    #[test]
    fn local_evidence_binds_immutable_body_types_policy_and_current_views() {
        let mut f = Fixture::unit();
        owner_sig(&mut f, &[]);
        let types = session();
        let definition = Arc::new(f.definition.clone());
        let policy = ContentHash::of_bytes(b"source policy");
        let (report, ran, accepted, evidence) = validate_definition(
            &definition,
            types.clone(),
            |_| Some((path("main"), 0)),
            &f.signatures,
            policy,
            None,
        );
        assert!(ran && report.violations.is_empty());
        assert!(
            accepted.is_some(),
            "uncached V must issue execution acceptance"
        );
        let evidence = evidence.unwrap();
        assert_eq!(evidence.accepted_artifact(), accepted);
        assert!(evidence.holds(&definition, &types, policy, &f.signatures));
        assert!(!evidence.holds(
            &Arc::new((*definition).clone()),
            &types,
            policy,
            &f.signatures
        ));
        assert!(!evidence.holds(&definition, &types, ContentHash::default(), &f.signatures));
        let mut changed = definition.clone();
        Arc::make_mut(&mut changed).entry = BodyId(u32::MAX);
        assert!(!evidence.holds(&changed, &types, policy, &f.signatures));
        let (rejected, _, accepted, rejected_evidence) = validate_definition(
            &changed,
            types.clone(),
            |_| Some((path("main"), 0)),
            &f.signatures,
            policy,
            None,
        );
        assert!(!rejected.violations.is_empty());
        assert!(accepted.is_none());
        assert!(rejected_evidence.is_none());
        assert_eq!(
            Arc::strong_count(&definition),
            1,
            "evidence must not pin bodies"
        );
        f.signatures
            .get_mut(f.definition.path)
            .unwrap()
            .metadata
            .visibility = hirsel_syntax::Visibility::Public;
        assert!(!evidence.holds(&definition, &types, policy, &f.signatures));
    }

    #[test]
    fn identified_view_never_pairs_changed_bytes_with_a_foreign_digest() {
        let mut f = Fixture::unit();
        owner_sig(&mut f, &[]);
        let original = f.signatures.get(f.definition.path).unwrap().clone();
        let old = crate::IdentifiedSignature::new(session(), Arc::new(original.clone()));
        let mut changed = original;
        changed.metadata.visibility = hirsel_syntax::Visibility::Public;
        let current = crate::IdentifiedSignature::new(session(), Arc::new(changed));
        assert_ne!(old.input().digest(), current.input().digest());
        assert_eq!(
            old.input().signature().metadata.visibility,
            hirsel_syntax::Visibility::Private
        );
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
                crate::IdentifiedSignature::new(session(), Arc::new(broken.clone())).input(),
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
                crate::IdentifiedSignature::new(session(), Arc::new(broken.clone())).input(),
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
    fn local_receipts_bind_exact_bytes_and_validator_build_before_reuse() {
        let mut f = Fixture::unit();
        let directory = Directory::new();
        let options = ContentHash::of_bytes(b"validation options");
        let policy = |build: &[u8]| {
            ContentHash::of_parts("local V build and options", [build, options.0.as_slice()])
        };
        let original = policy(b"V build 1");
        let cache = VerifierCache::directory(directory.0.clone());
        let (report, ran) = check(&f, &cache, original);
        assert!(ran && report.violations.is_empty());
        drop(cache);
        let reopened = VerifierCache::directory(directory.0.clone());
        assert!(!check(&f, &reopened, original).1);
        let (report, ran) = check(&f, &reopened, policy(b"V build 2"));
        assert!(
            ran && report.violations.is_empty(),
            "a new V build must run V"
        );
        f.definition.entry = hirsel_tir::BodyId(u32::MAX);
        for _ in 0..2 {
            let (report, ran) = check(&f, &reopened, original);
            assert!(
                ran && !report.violations.is_empty(),
                "changed bytes must run and fail V"
            );
        }
        assert!(
            Store::new(directory.0.clone())
                .lookup(Address(key(&artifact(&f), original)))
                .is_none(),
            "V must not publish a rejection as acceptance"
        );
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
            fn view(&self, d: hirsel_syntax::DefId) -> Option<crate::ViewInput<'_>> {
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
            let role = std::sync::Arc::make_mut(
                &mut changed.get_mut(path("0provider")).unwrap().metadata.roles,
            )
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
        let exact = Artifact::private(&f.definition, session(), signature.clone(), |_| {
            Some((path("main"), 0))
        });
        let expected = hirsel_product_codec::canonical_bytes(
            session(),
            &(&f.definition, &Some(signature.clone())),
            |_| Some((path("main"), 0)),
        )
        .unwrap();
        assert_eq!(
            exact.identity,
            Some(ContentHash::of_parts(
                "hirsel.validate.private-artifact.v2",
                [expected.as_slice()]
            ))
        );
        let mut changed = (*signature).clone();
        changed.metadata.visibility = hirsel_syntax::Visibility::Public;
        assert_ne!(
            exact.identity,
            Artifact::private(
                &f.definition,
                session(),
                std::sync::Arc::new(changed),
                |_| Some((path("main"), 0))
            )
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
            action: receipt_action(artifact.identity.unwrap(), policy, &[]),
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
    fn installation_products(root: &std::path::Path, bytes: &[u8]) {
        let store = Store::new(root.to_path_buf());
        let reference = store
            .put(
                Domain {
                    kind: 31,
                    schema: 1,
                },
                bytes,
            )
            .unwrap();
        store
            .associate(
                Address(ContentHash::of_bytes(b"ordinary")),
                Action(ContentHash::default()),
                reference,
            )
            .unwrap();
    }
    fn registered(
        f: &Fixture,
        root: &std::path::Path,
        state: &std::path::Path,
        build: ContentHash,
    ) -> Arc<VerifierCache> {
        let install = Installation::begin(root, state, build).unwrap();
        let cache = install.verifier();
        assert!(check(f, &cache, build).0.violations.is_empty());
        install.finish().unwrap();
        Installation::registered(root, state, build, Arc::default()).unwrap()
    }
    #[test]
    fn registered_installation_refuses_producer_forgery_with_and_without_registration() {
        let root = Directory::new();
        let state = Directory::new();
        let state = state.0.join("private");
        let build = ContentHash::of_bytes(b"local verifier");
        let valid = Fixture::unit();
        let mut malformed = Fixture::unit();
        malformed.definition.entry = BodyId(u32::MAX);
        let exact = artifact(&malformed);
        let identity = exact.identity.unwrap();
        let receipt = Receipt {
            action: receipt_action(identity, build, &[]),
            artifact: identity,
            policy: build,
            reads: vec![],
        };
        let producer = Store::new(root.0.clone());
        let reference = producer
            .put(DOMAIN, &bincode::serialize(&receipt).unwrap())
            .unwrap();
        let address = key(&exact, build);
        producer
            .associate(Address(address), Action(address), reference)
            .unwrap();
        // The producer has correct identities, checksums and association, as in
        // 211. Selecting product bytes still supplies no verifier capability.
        assert!(Installation::registered(&root.0, &state, build, Arc::default()).is_none());
        let (fresh, ran) = check(&malformed, &VerifierCache::default(), build);
        assert!(ran && !fresh.violations.is_empty());
        let installed = registered(&valid, &root.0, &state, build);
        assert!(!check(&valid, &installed, build).1);
        let (refused, ran) = check(&malformed, &installed, build);
        assert!(ran);
        assert_eq!(
            format!("{:?}", refused.violations),
            format!("{:?}", fresh.violations)
        );
        assert!(
            Installation::registered(
                &root.0,
                &state,
                ContentHash::of_bytes(b"another verifier"),
                Arc::default()
            )
            .is_none()
        );
    }
    #[test]
    fn registration_is_bound_to_root_state_and_exact_generation() {
        let root = Directory::new();
        let other = Directory::new();
        let state = Directory::new();
        let state = state.0.join("private");
        let fresh_state = Directory::new();
        let fresh_state = fresh_state.0.join("private");
        let build = ContentHash::of_bytes(b"local verifier");
        let f = Fixture::unit();
        installation_products(&root.0, b"generation N");
        installation_products(&other.0, b"generation N");
        let pinned = registered(&f, &root.0, &state, build);
        assert!(!check(&f, &pinned, build).1);
        let copy = |source: &std::path::Path, target: &std::path::Path| {
            std::fs::create_dir_all(target).unwrap();
            for entry in std::fs::read_dir(source).unwrap() {
                let path = entry.unwrap().path();
                if path.is_file() {
                    std::fs::copy(&path, target.join(path.file_name().unwrap())).unwrap();
                }
            }
        };
        // Copy registration bytes under the new root's expected name too.
        for path in std::fs::read_dir(&state)
            .unwrap()
            .filter_map(Result::ok)
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|x| x == "registration"))
        {
            std::fs::copy(
                path,
                installation::registration_path(&state, &other.0.canonicalize().unwrap()),
            )
            .unwrap();
        }
        assert!(Installation::registered(&other.0, &state, build, Arc::default()).is_none());
        installation::private_directory(&fresh_state).unwrap();
        copy(&state, &fresh_state);
        assert!(Installation::registered(&root.0, &fresh_state, build, Arc::default()).is_none());
        installation_products(&root.0, b"modified generation N+1");
        assert!(Installation::registered(&root.0, &state, build, Arc::default()).is_none());
        // A selected request retains only N's issued associations.
        assert!(!check(&f, &pinned, build).1);
        let mut modified = Fixture::unit();
        modified.definition.entry = BodyId(u32::MAX);
        assert!(!check(&modified, &pinned, build).0.violations.is_empty());
    }
    #[test]
    fn deleting_or_corrupting_private_registration_falls_back_to_identical_v() {
        let root = Directory::new();
        let state = Directory::new();
        let state = state.0.join("private");
        let build = ContentHash::of_bytes(b"local verifier");
        let f = Fixture::unit();
        installation_products(&root.0, b"products");
        let pinned = registered(&f, &root.0, &state, build);
        let (accepted, ran) = check(&f, &pinned, build);
        assert!(!ran && accepted.violations.is_empty());
        let path = installation::registration_path(
            &state.canonicalize().unwrap(),
            &root.0.canonicalize().unwrap(),
        );
        let mut bytes = std::fs::read(&path).unwrap();
        bytes[0] ^= 1;
        std::fs::write(&path, bytes).unwrap();
        assert!(Installation::registered(&root.0, &state, build, Arc::default()).is_none());
        std::fs::remove_dir_all(&state).unwrap();
        assert!(Installation::registered(&root.0, &state, build, Arc::default()).is_none());
        let (fresh, ran) = check(&f, &VerifierCache::default(), build);
        assert!(ran);
        assert_eq!(
            format!("{:?}", accepted.violations),
            format!("{:?}", fresh.violations)
        );
    }
}

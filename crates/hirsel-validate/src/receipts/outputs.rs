//! Body references come from V's own encoding of its checked Definition.
use super::*;
use hirsel_artifact_store::Reference;
use hirsel_syntax::DefId;

const BODY: Domain = REPLAY_DOMAIN;
const RECEIPT: Domain = Domain {
    kind: 0x5600_0006,
    schema: 4,
};
fn key(subject: ContentHash, policy: ContentHash) -> ContentHash {
    ContentHash::of_parts(
        "hirsel.validate.definition-output.v4",
        [subject.0.as_slice(), policy.0.as_slice()],
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::support::{Fixture, owner_sig, path, session};
    use std::sync::atomic::{AtomicU64, Ordering};

    #[test]
    fn checked_replay_rows_preserve_identity_and_refuse_changed_or_missing_children() {
        let fixture = Fixture::unit();
        let definition = Arc::new(fixture.definition.clone());
        let cache = VerifierCache::default();
        let policy = ContentHash::of_bytes(b"checked replay rows");
        let (report, _, _, evidence) = validate_definition(
            &definition,
            session(),
            |_| Some((path("main"), 0)),
            &fixture.signatures,
            policy,
            Some(&cache),
        );
        assert!(report.violations.is_empty());
        let evidence = evidence.unwrap();
        let output = cache
            .definition_output_from_evidence(
                &definition,
                session(),
                policy,
                &fixture.signatures,
                &evidence,
            )
            .unwrap();
        let anchor = Anchor::fresh();
        let archive = cache
            .read_definition_archive(
                &output,
                session(),
                |_, _| Some(anchor),
                policy,
                &fixture.signatures,
                Arc::new(()),
            )
            .unwrap();
        assert_eq!(archive.header().unwrap().path, definition.path);
        assert_eq!(archive.body_count() as usize, definition.bodies.len());
        assert_eq!(
            archive.body(0).unwrap().local_count() as usize,
            definition.bodies[0].locals.len()
        );
        let accepted = evidence.accepted.as_ref().unwrap();
        let (_, mut record): (_, ReplayRecord) = cache
            .load_record(replay_key(accepted.artifact, policy), REPLAY_DOMAIN)
            .unwrap();
        let original = record.body;
        let mut changed = (*definition).clone();
        changed.entry = hirsel_tir::BodyId(u32::MAX);
        let changed = cache
            .records(session())
            .encode_tir(&changed, |_| Some((path("main"), 0)))
            .unwrap();
        let changed = cache
            .contents()
            .put_content(hirsel_product_codec::CheckedTir::DOMAIN, &changed)
            .unwrap();
        let mut missing = original;
        missing.len += 1;
        for child in [missing, changed] {
            record.body = child;
            cache.publish(
                replay_key(accepted.artifact, policy),
                REPLAY_DOMAIN,
                &record,
            );
            let poisoned = cache
                .definition_output_from_evidence(
                    &definition,
                    session(),
                    policy,
                    &fixture.signatures,
                    &evidence,
                )
                .unwrap();
            assert!(
                cache
                    .read_definition_archive(
                        &poisoned,
                        session(),
                        |_, _| Some(anchor),
                        policy,
                        &fixture.signatures,
                        Arc::new(()),
                    )
                    .is_none()
            );
            assert!(
                cache
                    .read_definition(
                        &poisoned,
                        session(),
                        |_, _| Some(anchor),
                        policy,
                        &fixture.signatures
                    )
                    .is_none()
            );
        }
        // The already checked immutable owner retains the original child.
        assert_eq!(archive.header().unwrap().entry, definition.entry);
    }

    struct Directory(PathBuf);
    impl Directory {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let root = std::env::temp_dir().join(format!(
                "hirsel-v-output-{}-{}",
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

    #[test]
    fn transported_outputs_survive_cache_eviction_and_recheck_the_current_environment() {
        #[derive(Default)]
        struct Rows(
            Mutex<hirsel_artifact_store::Memory>,
            Mutex<HashMap<ContentHash, Reference>>,
        );
        impl hirsel_artifact_store::Contents for Rows {
            fn get_content(&self, reference: Reference) -> Option<hirsel_artifact_store::Bytes> {
                self.0.lock().unwrap().get_content(reference)
            }
            fn put_content(&self, domain: Domain, bytes: &[u8]) -> Option<Reference> {
                self.0.lock().unwrap().put_content(domain, bytes)
            }
        }
        impl VerifierRows for Rows {
            fn row(&self, key: ContentHash) -> Option<Reference> {
                self.1.lock().unwrap().get(&key).copied()
            }
            fn set_row(&self, key: ContentHash, reference: Reference) {
                self.1.lock().unwrap().insert(key, reference);
            }
        }
        let rows = Arc::new(Rows::default());
        let selected = Arc::new(VerifierCache::hosted(rows.clone()));
        let request = selected.with_transport(Arc::new(Rows::default()));
        let mut f = Fixture::unit();
        owner_sig(&mut f, &[]);
        let definition = Arc::new(f.definition.clone());
        let policy = ContentHash::of_bytes(b"transported private output");
        let (report, _, _, evidence) = validate_definition(
            &definition,
            session(),
            |_| Some((path("main"), 0)),
            &f.signatures,
            policy,
            Some(&request),
        );
        assert!(report.violations.is_empty(), "{report:?}");
        let output = request
            .definition_output_from_evidence(
                &definition,
                session(),
                policy,
                &f.signatures,
                &evidence.unwrap(),
            )
            .unwrap();
        let (stored, bytes) = request.compact_definition_output(&output).unwrap();
        *rows.0.lock().unwrap() = Default::default();
        rows.1.lock().unwrap().clear();
        assert!(
            stored
                .load(
                    &selected,
                    &session(),
                    &f.signatures,
                    &bytes,
                    stored.decode_bytes()
                )
                .is_none()
        );
        let restored = stored
            .load(
                &request,
                &session(),
                &f.signatures,
                &bytes,
                stored.decode_bytes(),
            )
            .unwrap();
        assert_eq!(restored, output);
        assert_eq!(
            request
                .read_definition(
                    &restored,
                    session(),
                    |_, _| Some(Anchor::default()),
                    policy,
                    &f.signatures
                )
                .unwrap()
                .path,
            definition.path
        );
        f.signatures.clear();
        assert!(
            stored
                .load(
                    &request,
                    &session(),
                    &f.signatures,
                    &bytes,
                    stored.decode_bytes()
                )
                .is_none()
        );
    }

    #[test]
    fn stored_output_rechecks_current_inputs_and_refuses_before_decode() {
        let mut f = Fixture::unit();
        owner_sig(&mut f, &[]);
        let definition = Arc::new(f.definition.clone());
        let cache = VerifierCache::default();
        let policy = ContentHash::of_bytes(b"stored source validation");
        let (report, _, _, evidence) = validate_definition(
            &definition,
            session(),
            |_| Some((path("main"), 0)),
            &f.signatures,
            policy,
            Some(&cache),
        );
        assert!(report.violations.is_empty(), "{report:?}");
        let output = cache
            .definition_output_from_evidence(
                &definition,
                session(),
                policy,
                &f.signatures,
                &evidence.unwrap(),
            )
            .unwrap();
        let (stored, bytes) = cache.compact_definition_output(&output).unwrap();
        assert!(
            stored
                .load(
                    &cache,
                    &session(),
                    &f.signatures,
                    &bytes,
                    stored.decode_bytes() - 1
                )
                .is_none()
        );
        for damaged in [bytes[..bytes.len() - 1].to_vec(), {
            let mut damaged = bytes.clone();
            damaged[0] ^= 1;
            damaged
        }] {
            assert!(
                stored
                    .load(
                        &cache,
                        &session(),
                        &f.signatures,
                        &damaged,
                        stored.decode_bytes()
                    )
                    .is_none()
            );
        }
        let restored = stored
            .load(
                &cache,
                &session(),
                &f.signatures,
                &bytes,
                stored.decode_bytes(),
            )
            .unwrap();
        assert_eq!(restored, output);
        assert!(
            stored
                .load(
                    &VerifierCache::default(),
                    &session(),
                    &f.signatures,
                    &bytes,
                    stored.decode_bytes()
                )
                .is_none()
        );
        let mut wrong = stored.clone();
        wrong.artifact = ContentHash::of_bytes(b"different body");
        assert!(
            wrong
                .load(
                    &cache,
                    &session(),
                    &f.signatures,
                    &bytes,
                    stored.decode_bytes()
                )
                .is_none()
        );
        let mut wrong = stored.clone();
        wrong.policy = ContentHash::of_bytes(b"different policy");
        assert!(
            wrong
                .load(
                    &cache,
                    &session(),
                    &f.signatures,
                    &bytes,
                    stored.decode_bytes()
                )
                .is_none()
        );
        f.signatures.clear();
        assert!(
            stored
                .load(
                    &cache,
                    &session(),
                    &f.signatures,
                    &bytes,
                    stored.decode_bytes()
                )
                .is_none()
        );
    }

    #[test]
    fn completed_definition_returns_its_existing_verifier_replay_without_publication() {
        for directory_backed in [false, true] {
            let mut f = Fixture::unit();
            owner_sig(&mut f, &[]);
            let definition = Arc::new(f.definition.clone());
            let directory = Directory::new();
            let physical = Store::rebuildable(directory.0.clone());
            let statistics = physical.enable_statistics();
            let cache = if directory_backed {
                VerifierCache::directory_store(physical.clone())
            } else {
                VerifierCache::default()
            };
            let policy = ContentHash::of_bytes(b"completed source validation");
            let (report, _, _, evidence) = validate_definition(
                &definition,
                session(),
                |_| Some((path("main"), 0)),
                &f.signatures,
                policy,
                Some(&cache),
            );
            assert!(report.violations.is_empty(), "{report:?}");
            let evidence = evidence.unwrap();
            let accepted = evidence.accepted.as_ref().unwrap();
            let (expected, record): (_, ReplayRecord) = cache
                .load_record(replay_key(accepted.artifact, policy), REPLAY_DOMAIN)
                .unwrap();
            assert_eq!(record.artifact, accepted.artifact);
            let before = statistics.snapshot();
            for _ in 0..2 {
                let output = cache
                    .definition_output_from_evidence(
                        &definition,
                        session(),
                        policy,
                        &f.signatures,
                        &evidence,
                    )
                    .unwrap();
                assert_eq!(output.reference(), expected);
                assert!(cache.contents().get_content(expected).is_some());
                assert!(
                    cache
                        .read_definition(
                            &output,
                            session(),
                            |_, _| Some(Anchor::default()),
                            policy,
                            &f.signatures,
                        )
                        .is_some()
                );
            }
            assert!(
                cache
                    .definition_output_from_evidence(
                        &definition,
                        session(),
                        ContentHash::default(),
                        &f.signatures,
                        &evidence,
                    )
                    .is_none()
            );
            let after = statistics.snapshot();
            assert_eq!(
                after.counters["PublicationAttempts"],
                before.counters["PublicationAttempts"]
            );
            assert_eq!(
                after.counters["IndexPublications"],
                before.counters["IndexPublications"]
            );
        }
    }

    #[test]
    fn output_receipt_reopens_without_body_decode_and_rejects_foreign_bytes_and_contexts() {
        let mut f = Fixture::unit();
        owner_sig(&mut f, &[]);
        let definition = Arc::new(f.definition.clone());
        let directory = Directory::new();
        let cache = VerifierCache::directory(directory.0.clone());
        let policy = ContentHash::of_bytes(b"source V policy");
        let subject = ContentHash::of_bytes(b"complete lower output and action");
        let (report, _, _, evidence) = validate_definition(
            &definition,
            session(),
            |_| Some((path("main"), 0)),
            &f.signatures,
            policy,
            Some(&cache),
        );
        assert!(report.violations.is_empty(), "{report:?}");
        let evidence = evidence.unwrap();
        let output = cache
            .publish_definition(
                subject,
                &definition,
                session(),
                policy,
                &f.signatures,
                &evidence,
                b"query metadata",
            )
            .unwrap();
        let reopened = VerifierCache::directory(directory.0.clone());
        let get = |cache: &VerifierCache, subject, owner, policy| {
            cache.definition_output(subject, owner, &session(), policy, &f.signatures)
        };
        assert_eq!(
            get(&reopened, subject, definition.path, policy),
            Some(output.clone())
        );
        assert_eq!(output.metadata(), b"query metadata");
        assert!(
            reopened
                .read_definition(
                    &output,
                    session(),
                    |_, _| Some(Anchor::default()),
                    policy,
                    &f.signatures
                )
                .is_some()
        );
        assert!(get(&reopened, ContentHash::default(), definition.path, policy).is_none());
        assert!(
            get(
                &reopened,
                subject,
                hirsel_syntax::DefId::intern(&path("other")),
                policy
            )
            .is_none()
        );
        assert!(get(&reopened, subject, definition.path, ContentHash::default()).is_none());
        assert!(
            reopened
                .definition_output(
                    subject,
                    definition.path,
                    &TypeInterner::new(None),
                    policy,
                    &f.signatures
                )
                .is_none()
        );
        let mut changed = definition.clone();
        Arc::make_mut(&mut changed).entry = hirsel_tir::BodyId(u32::MAX);
        assert!(
            cache
                .publish_definition(
                    subject,
                    &changed,
                    session(),
                    policy,
                    &f.signatures,
                    &evidence,
                    b""
                )
                .is_none()
        );
        // Even all exact envelope and receipt bytes imported into contents
        // cannot transfer the local verifier association that issued them.
        let imported = Directory::new();
        let source = Store::new(directory.0.clone());
        let target = Store::new(imported.0.clone());
        target
            .put(BODY, &source.get(output.reference()).unwrap())
            .unwrap();
        let association = source.lookup(Address(key(subject, policy))).unwrap();
        target
            .put(RECEIPT, &source.get(association.reference).unwrap())
            .unwrap();
        assert!(
            get(
                &VerifierCache::directory(imported.0.clone()),
                subject,
                definition.path,
                policy
            )
            .is_none()
        );
        std::fs::write(source.output_path(output.reference()), b"truncated").unwrap();
        assert!(get(&reopened, subject, definition.path, policy).is_none());
    }

    #[test]
    fn output_receipt_rejects_changed_private_views_with_equal_producer_fingerprints() {
        let mut f = Fixture::unit();
        owner_sig(&mut f, &[]);
        let definition = Arc::new(f.definition.clone());
        let cache = VerifierCache::default();
        let policy = ContentHash::default();
        let (_, _, _, evidence) = validate_definition(
            &definition,
            session(),
            |_| Some((path("main"), 0)),
            &f.signatures,
            policy,
            Some(&cache),
        );
        let output = cache
            .publish_definition(
                policy,
                &definition,
                session(),
                policy,
                &f.signatures,
                &evidence.unwrap(),
                b"",
            )
            .unwrap();
        assert!(output.matches_definition(
            &definition,
            session(),
            |_| Some((path("main"), 0)),
            policy,
            &f.signatures,
        ));
        let signature = f.signatures.get_mut(definition.path).unwrap();
        let producer = signature.fingerprint;
        signature.metadata.visibility = hirsel_syntax::Visibility::Public;
        assert_eq!(signature.fingerprint, producer);
        assert!(!output.matches_definition(
            &definition,
            session(),
            |_| Some((path("main"), 0)),
            policy,
            &f.signatures,
        ));
        assert!(
            cache
                .definition_output(policy, definition.path, &session(), policy, &f.signatures)
                .is_none()
        );
        assert!(
            cache
                .read_definition(
                    &output,
                    session(),
                    |_, _| Some(Anchor::default()),
                    policy,
                    &f.signatures
                )
                .is_none()
        );
        // Revalidate the identical body against the new independent private
        // view. Body content backdates; the complete observation must change.
        let (report, _, _, evidence) = validate_definition(
            &definition,
            session(),
            |_| Some((path("main"), 0)),
            &f.signatures,
            policy,
            Some(&cache),
        );
        assert!(report.violations.is_empty(), "{report:?}");
        let current = cache
            .publish_definition(
                policy,
                &definition,
                session(),
                policy,
                &f.signatures,
                &evidence.unwrap(),
                b"new query metadata",
            )
            .unwrap();
        assert_eq!(current.reference(), output.reference());
        assert_ne!(
            current.observation_identity(),
            output.observation_identity()
        );
        assert_eq!(
            cache
                .definition_output(policy, definition.path, &session(), policy, &f.signatures)
                .unwrap()
                .observation_identity(),
            current.observation_identity()
        );
    }
}
#[derive(Serialize, Deserialize)]
struct Record {
    subject: ContentHash,
    owner: DefId,
    policy: ContentHash,
    artifact: ContentHash,
    reference: Reference,
    reads: Vec<VRead>,
    metadata: Vec<u8>,
    invocations: SourceInvocations,
}
/// A call observed by source V, before native instance lowering. These are
/// read-only data; only an opaque source output associates them with authority.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SourceInvocation {
    pub body: u32,
    pub block: u32,
    pub statement: u32,
    pub restart: bool,
    pub callee: Option<DefPath>,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
struct SourceInvocations {
    entry: u32,
    bodies: u32,
    calls: Vec<SourceInvocation>,
}
impl SourceInvocations {
    fn observe(
        definition: &hirsel_tir::Definition,
        types: &TypeInterner,
        signatures: &dyn SignatureLookup,
    ) -> Option<Self> {
        let coverage = crate::invocation_coverage(types, definition, signatures).ok()?;
        let calls = coverage
            .into_iter()
            .map(|(body, block, statement, restart)| {
                let callee =
                    match &definition.bodies[body as usize].blocks[block as usize].terminator {
                        hirsel_tir::Terminator::Call {
                            callee: hirsel_tir::Callee::Definition { path, .. },
                            ..
                        } => Some(path.clone()),
                        _ => None,
                    };
                SourceInvocation {
                    body,
                    block,
                    statement,
                    restart,
                    callee,
                }
            })
            .collect();
        Some(Self {
            entry: definition.entry.0,
            bodies: u32::try_from(definition.bodies.len()).ok()?,
            calls,
        })
    }
}

/// Opaque original-source observations from V's own checked output. There is
/// no public constructor or decoder; ordinary query metadata cannot mint it.
#[derive(Clone, Debug)]
pub struct VerifiedInvocations {
    owner: DefId,
    source: ContentHash,
    facts: Arc<SourceInvocations>,
}
impl VerifiedInvocations {
    pub fn owner(&self) -> DefId {
        self.owner
    }
    pub fn source_identity(&self) -> ContentHash {
        self.source
    }
    /// The original Lower entry, independent of derived native instances.
    pub fn entry_body(&self) -> u32 {
        self.facts.entry
    }
    pub fn body_count(&self) -> u32 {
        self.facts.bodies
    }
    pub fn calls(&self) -> &[SourceInvocation] {
        &self.facts.calls
    }
}

/// The reference names only bytes V wrote from its own checked source body.
/// It cannot certify an unrelated producer record or native import/C/Ready IR.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VerifiedDefinitionOutput {
    owner: DefId,
    reference: Reference,
    artifact: ContentHash,
    policy: ContentHash,
    reads: Arc<[VRead]>,
    metadata: Arc<[u8]>,
    invocations: Arc<SourceInvocations>,
}
impl VerifiedDefinitionOutput {
    /// Strongly owned compact observations, excluding the independently stored
    /// body named by reference. Shared allocations are conservatively charged
    /// once for each admitted owner that keeps this token alive.
    pub fn retained_bytes(&self) -> Option<u64> {
        let mut bytes = (std::mem::size_of::<Self>()
            + std::mem::size_of::<SourceInvocations>()
            + 6 * std::mem::size_of::<usize>()) as u64;
        bytes = bytes.checked_add(
            (self.reads.len() as u64).checked_mul(std::mem::size_of::<VRead>() as u64)?,
        )?;
        bytes = bytes.checked_add(self.metadata.len() as u64)?;
        bytes = bytes.checked_add(
            (self.invocations.calls.capacity() as u64)
                .checked_mul(std::mem::size_of::<SourceInvocation>() as u64)?,
        )?;
        for call in &self.invocations.calls {
            if let Some(path) = &call.callee {
                bytes = bytes.checked_add(bincode::serialized_size(path).ok()?)?;
            }
        }
        Some(bytes)
    }
    pub fn reference(&self) -> Reference {
        self.reference
    }
    pub fn owner(&self) -> DefId {
        self.owner
    }
    /// Ordinary identity of the exact artifact, policy and independently
    /// observed environment. A body reference alone omits private-view reads.
    /// The query owner still validates current Lower dependencies and obtains
    /// this token from V; this hash grants no verification authority.
    pub fn observation_identity(&self) -> ContentHash {
        receipt_action(self.artifact, self.policy, &self.reads)
    }
    /// Original call identities and legal restarts observed by V, independent
    /// of native units and producer-supplied query metadata.
    pub fn invocations(&self) -> VerifiedInvocations {
        VerifiedInvocations {
            owner: self.owner,
            source: self.observation_identity(),
            facts: self.invocations.clone(),
        }
    }
    /// Rebind an already issued V output to an independently supplied original
    /// after its replay bytes were evicted. Exact canonical artifact identity,
    /// current private observations and policy are all checked here. Content
    /// alone cannot create this opaque output or replace its authority.
    pub fn matches_definition(
        &self,
        definition: &hirsel_tir::DefinitionRef,
        types: Arc<TypeInterner>,
        owner: impl FnMut(Anchor) -> Option<(DefPath, u32)>,
        policy: ContentHash,
        signatures: &dyn SignatureLookup,
    ) -> bool {
        self.owner == definition.path
            && self.policy == policy
            && self
                .reads
                .iter()
                .all(|read| crate::input::read_holds_for_types(&types, read, signatures))
            && Artifact::new(definition, types, owner).identity == Some(self.artifact)
    }
    /// Query metadata remains producer data; the query owner checks it.
    pub fn metadata(&self) -> &[u8] {
        &self.metadata
    }
}
/// Compact retained authority for a completed V output. Its metadata can live
/// in ordinary bounded storage; bytes alone cannot construct this capability.
#[derive(Clone)]
pub struct StoredDefinitionOutput {
    owner: DefId,
    policy: ContentHash,
    artifact: ContentHash,
    reference: Reference,
    metadata: ContentHash,
    decode_bytes: u64,
}
impl StoredDefinitionOutput {
    /// Peak metadata decode storage, checked before provider I/O by consumers.
    pub fn decode_bytes(&self) -> u64 {
        self.decode_bytes
    }
    /// Restore only the bytes bound by this verifier-minted capability, with
    /// current independent input validation. No body is decoded or accepted here.
    pub fn load(
        &self,
        cache: &VerifierCache,
        types: &TypeInterner,
        signatures: &dyn SignatureLookup,
        bytes: &[u8],
        allowance: u64,
    ) -> Option<VerifiedDefinitionOutput> {
        if self.decode_bytes > allowance
            || bytes.len() as u64 > allowance
            || ContentHash::of_bytes(bytes) != self.metadata
        {
            return None;
        }
        let record: Record = bincode::DefaultOptions::new()
            .with_fixint_encoding()
            .with_limit(bytes.len() as u64)
            .reject_trailing_bytes()
            .deserialize(bytes)
            .ok()?;
        if record.owner != self.owner
            || record.artifact != self.artifact
            || record.policy != self.policy
            || record.reference != self.reference
            || record.subject != receipt_action(record.artifact, record.policy, &record.reads)
            || !record
                .reads
                .iter()
                .all(|read| crate::input::read_holds_for_types(types, read, signatures))
        {
            return None;
        }
        cache.contents().get_content(self.reference)?;
        Some(VerifiedDefinitionOutput {
            owner: self.owner,
            reference: self.reference,
            artifact: self.artifact,
            policy: self.policy,
            reads: record.reads.into(),
            metadata: record.metadata.into(),
            invocations: Arc::new(record.invocations),
        })
    }
}
impl VerifierCache {
    /// Return compact authority and its ordinary transport bytes. This does
    /// not publish another verifier receipt or retain the metadata in V.
    pub fn compact_definition_output(
        &self,
        output: &VerifiedDefinitionOutput,
    ) -> Option<(StoredDefinitionOutput, Vec<u8>)> {
        self.contents().get_content(output.reference)?;
        let record = Record {
            subject: output.observation_identity(),
            owner: output.owner,
            policy: output.policy,
            artifact: output.artifact,
            reference: output.reference,
            reads: output.reads.to_vec(),
            metadata: output.metadata.to_vec(),
            invocations: (*output.invocations).clone(),
        };
        let bytes = bincode::serialize(&record).ok()?;
        let decode_bytes = output
            .retained_bytes()?
            .checked_mul(3)?
            .checked_add((bytes.len() as u64).checked_mul(4)?)?
            .checked_add(std::mem::size_of::<Record>() as u64)?;
        Some((
            StoredDefinitionOutput {
                owner: output.owner,
                policy: output.policy,
                artifact: output.artifact,
                reference: output.reference,
                metadata: ContentHash::of_bytes(&bytes),
                decode_bytes,
            },
            bytes,
        ))
    }
    /// V encodes the actual immutable body, rather than accepting a caller's
    /// claimed body reference. The subject is only its query lookup address.
    pub fn publish_definition<'a>(
        &self,
        subject: ContentHash,
        definition: &hirsel_tir::DefinitionRef,
        types: Arc<TypeInterner>,
        policy: ContentHash,
        signatures: impl Into<crate::SignatureInputs<'a>>,
        evidence: &VerifiedDefinition,
        metadata: &[u8],
    ) -> Option<VerifiedDefinitionOutput> {
        let output = self.definition_output_with_metadata(
            definition,
            types,
            policy,
            signatures.into(),
            evidence,
            metadata,
        )?;
        self.publish(
            key(subject, policy),
            RECEIPT,
            &Record {
                subject,
                owner: output.owner,
                policy,
                artifact: output.artifact,
                reference: output.reference,
                reads: output.reads.to_vec(),
                metadata: metadata.to_vec(),
                invocations: (*output.invocations).clone(),
            },
        );
        Some(output)
    }
    /// Return V's output for an actual completed validation, without looking up
    /// or publishing an ordinary query. The empty metadata is not a cutoff
    /// receipt: the caller must already have completed its real Lower demand.
    pub fn definition_output_from_evidence<'a>(
        &self,
        definition: &hirsel_tir::DefinitionRef,
        types: Arc<TypeInterner>,
        policy: ContentHash,
        signatures: impl Into<crate::SignatureInputs<'a>>,
        evidence: &VerifiedDefinition,
    ) -> Option<VerifiedDefinitionOutput> {
        self.definition_output_with_metadata(
            definition,
            types,
            policy,
            signatures.into(),
            evidence,
            &[],
        )
    }
    fn definition_output_with_metadata(
        &self,
        definition: &hirsel_tir::DefinitionRef,
        types: Arc<TypeInterner>,
        policy: ContentHash,
        signatures: crate::SignatureInputs<'_>,
        evidence: &VerifiedDefinition,
        metadata: &[u8],
    ) -> Option<VerifiedDefinitionOutput> {
        if !evidence.holds(definition, &types, policy, signatures) {
            return None;
        }
        let accepted = evidence.accepted.as_ref()?;
        if accepted.policy != policy {
            return None;
        }
        let identity = accepted.artifact;
        // Return V's own checked replay output, without serializing or writing
        // the envelope again for a native cutoff.
        let (reference, record): (_, ReplayRecord) =
            self.load_record(replay_key(identity, policy), REPLAY_DOMAIN)?;
        if record.artifact != identity || record.policy != policy {
            return None;
        }
        Some(VerifiedDefinitionOutput {
            owner: definition.path,
            reference,
            artifact: identity,
            policy,
            reads: evidence.reads.clone().into(),
            metadata: metadata.into(),
            invocations: Arc::new(SourceInvocations::observe(
                definition,
                &types,
                signatures.lookup(),
            )?),
        })
    }
    /// The query owner validates prior ordinary dependency outputs, depth,
    /// selections and private execution before asking for this V-owned body.
    /// Here V checks local authority, its independent current environment and
    /// raw content integrity without decoding the Definition.
    pub fn definition_output(
        &self,
        subject: ContentHash,
        owner: DefId,
        types: &TypeInterner,
        policy: ContentHash,
        signatures: &dyn SignatureLookup,
    ) -> Option<VerifiedDefinitionOutput> {
        let record: Record = self.load(key(subject, policy), RECEIPT)?;
        if record.subject != subject
            || record.owner != owner
            || record.policy != policy
            || record.reference.domain != BODY
            || !record
                .reads
                .iter()
                .all(|r| crate::input::read_holds_for_types(types, r, signatures))
        {
            return None;
        }
        self.contents().get_content(record.reference)?;
        Some(VerifiedDefinitionOutput {
            owner,
            reference: record.reference,
            artifact: record.artifact,
            policy,
            reads: record.reads.into(),
            metadata: record.metadata.into(),
            invocations: Arc::new(record.invocations),
        })
    }
    /// Read independently checked rows from a verifier-issued replay output.
    /// Complete artifact identity is recomputed through the established serde
    /// contract, without reconstructing a Definition or Body.
    pub fn read_definition_archive(
        &self,
        output: &VerifiedDefinitionOutput,
        types: Arc<TypeInterner>,
        anchor: impl FnMut(&DefPath, u32) -> Option<Anchor>,
        policy: ContentHash,
        signatures: &dyn SignatureLookup,
        owner: Arc<dyn Send + Sync>,
    ) -> Option<hirsel_product_codec::CheckedTir> {
        if output.reference.domain != BODY
            || output.policy != policy
            || !output
                .reads
                .iter()
                .all(|read| crate::input::read_holds_for_types(&types, read, signatures))
        {
            return None;
        }
        let bytes = self.contents().get_content(output.reference)?;
        let record: ReplayRecord = bincode::DefaultOptions::new()
            .with_fixint_encoding()
            .with_limit(bytes.len() as u64)
            .reject_trailing_bytes()
            .deserialize(&bytes)
            .ok()?;
        if record.artifact != output.artifact || record.policy != output.policy {
            return None;
        }
        let ReplayRows {
            archive,
            overlay,
            owners,
        } = self.replay_rows(&record, types.clone(), anchor, owner)?;
        if overlay.is_some() || archive.header()?.path != output.owner {
            return None;
        }
        let bytes = hirsel_product_codec::canonical_bytes(
            types,
            &hirsel_product_codec::TirView::Archived(&archive),
            |anchor| owners.get(&anchor).cloned(),
        )?;
        (ContentHash::of_parts("hirsel.validate.artifact.v1", [bytes.as_slice()])
            == output.artifact)
            .then_some(archive)
    }
    /// Explicit materialization for transforming consumers. Row readers use
    /// read_definition_archive and retain its immutable checked owner instead.
    pub fn read_definition(
        &self,
        output: &VerifiedDefinitionOutput,
        types: Arc<TypeInterner>,
        anchor: impl FnMut(&DefPath, u32) -> Option<Anchor>,
        policy: ContentHash,
        signatures: &dyn SignatureLookup,
    ) -> Option<hirsel_tir::DefinitionRef> {
        Some(Arc::new(
            self.read_definition_archive(output, types, anchor, policy, signatures, Arc::new(()))?
                .materialize()?,
        ))
    }
}

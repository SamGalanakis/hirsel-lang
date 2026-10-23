//! Body references come from V's own encoding of its checked Definition.
use super::*;
use hirsel_artifact_store::Reference;
use hirsel_syntax::DefId;

const BODY: Domain = REPLAY_DOMAIN;
const RECEIPT: Domain = Domain {
    kind: 0x5600_0006,
    schema: 2,
};
fn key(subject: ContentHash, policy: ContentHash) -> ContentHash {
    ContentHash::of_parts(
        "hirsel.validate.definition-output.v2",
        [subject.0.as_slice(), policy.0.as_slice()],
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::support::{Fixture, owner_sig, path, session};
    use std::sync::atomic::{AtomicU64, Ordering};

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
        let signature = f.signatures.get_mut(definition.path).unwrap();
        let producer = signature.fingerprint;
        signature.metadata.visibility = hirsel_syntax::Visibility::Public;
        assert_eq!(signature.fingerprint, producer);
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
}
impl VerifiedDefinitionOutput {
    pub fn reference(&self) -> Reference {
        self.reference
    }
    pub fn owner(&self) -> DefId {
        self.owner
    }
    /// Query metadata remains producer data; the query owner checks it.
    pub fn metadata(&self) -> &[u8] {
        &self.metadata
    }
}
impl VerifierCache {
    /// V encodes the actual immutable body, rather than accepting a caller's
    /// claimed body reference. The subject is only its query lookup address.
    pub fn publish_definition(
        &self,
        subject: ContentHash,
        definition: &hirsel_tir::DefinitionRef,
        types: Arc<TypeInterner>,
        policy: ContentHash,
        signatures: &dyn SignatureLookup,
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
        // Reuse the existing verifier replay envelope. No second body encoding
        // or provider is introduced for the native cutoff.
        let record: ReplayRecord = self.load(replay_key(identity, policy), REPLAY_DOMAIN)?;
        if record.artifact != identity || record.policy != policy {
            return None;
        }
        let bytes = bincode::serialize(&record).ok()?;
        let reference = self.contents().put_content(BODY, &bytes)?;
        if BODY.reference(&bytes)? != reference {
            return None;
        }
        self.publish(
            key(subject, policy),
            RECEIPT,
            &Record {
                subject,
                owner: definition.path,
                policy,
                artifact: identity,
                reference,
                reads: evidence.reads.clone(),
                metadata: metadata.to_vec(),
            },
        );
        Some(VerifiedDefinitionOutput {
            owner: definition.path,
            reference,
            artifact: identity,
            policy,
            reads: evidence.reads.clone().into(),
            metadata: metadata.into(),
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
        })
    }
    /// Materialize only a demanded body, rechecking identity after portable
    /// type/anchor rebinding. This performs no second lowering or validator.
    pub fn read_definition(
        &self,
        output: &VerifiedDefinitionOutput,
        types: Arc<TypeInterner>,
        mut anchor: impl FnMut(&DefPath, u32) -> Option<Anchor>,
        policy: ContentHash,
        signatures: &dyn SignatureLookup,
    ) -> Option<hirsel_tir::DefinitionRef> {
        if output.policy != policy
            || !output
                .reads
                .iter()
                .all(|r| crate::input::read_holds_for_types(&types, r, signatures))
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
        let mut owners = std::collections::BTreeMap::new();
        let (definition, overlay): (
            hirsel_tir::DefinitionRef,
            Option<hirsel_types::SignatureRef>,
        ) = self
            .records(types.clone())
            .decode_record(&record.bytes, |path, occurrence| {
                let site = anchor(path, occurrence)?;
                owners.insert(site, (path.clone(), occurrence));
                Some(site)
            })?;
        if overlay.is_some() {
            return None;
        }
        let exact = Artifact::new(&definition, types, |a| owners.get(&a).cloned());
        (definition.path == output.owner && exact.identity == Some(output.artifact))
            .then_some(definition)
    }
}

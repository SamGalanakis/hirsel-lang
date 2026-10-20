//! `FrozenProvider`: a body's signature provider over a frozen snapshot.
//!
//! P workers read signatures, lookups and private views from the snapshot
//! without locks, and every read is recorded into the task's `ReadsBuilder`
//! with the value it observed, so the memo the task produces can be verified
//! by comparing observed values alone (design §1(b), "Typed reads").
use super::SignatureSnapshot;
use crate::memo::{LookupKey, LookupSource, ReadKey, ReadsBuilder};
use hirsel_syntax::{ContentHash, DefId, FileContext, PackageId, Path};
use hirsel_types::{LookupOutput, QueryError, SignatureProvider, SignatureRef};
use hirsel_validate::SignatureLookup;

/// The value a failed signature read observes: the failure, hashed.
fn failed(error: &QueryError) -> ContentHash {
    ContentHash::of_parts(
        "hirsel.read.failed-signature.v1",
        [format!("{error:?}").as_bytes()],
    )
}
/// A signature provider for bodies of `owner`'s package, reading `snapshot`.
pub struct FrozenProvider<'a> {
    snapshot: &'a SignatureSnapshot,
    owner: PackageId,
    reads: &'a mut ReadsBuilder,
}
impl<'a> FrozenProvider<'a> {
    pub fn new(
        snapshot: &'a SignatureSnapshot,
        owner: PackageId,
        reads: &'a mut ReadsBuilder,
    ) -> Self {
        Self {
            snapshot,
            owner,
            reads,
        }
    }
    /// `d`'s view as the owner's bodies see it, recording `Signature(d)` (or
    /// `FailedSignature(d)`) and, for an opaque type seen privately,
    /// `Representation(d)` with the private view's digest.
    pub fn body_view(&mut self, d: DefId) -> Result<SignatureRef, QueryError> {
        let view = self.signature(d)?;
        if let Some(Ok(private)) = self.snapshot.body_view(&self.owner, d)
            && !std::sync::Arc::ptr_eq(private, &view)
        {
            let digest = self
                .snapshot
                .representation(d)
                .and_then(Result::ok)
                .map_or_else(ContentHash::default, |(_, digest)| digest.0);
            self.reads.record(ReadKey::Representation(d), digest);
            return Ok(private.clone());
        }
        Ok(view)
    }
    /// V's digest of `d`'s view in the owner's universe, recording
    /// `ValidationView(d)`.
    pub fn validation_view(&mut self, d: DefId) -> Option<hirsel_validate::ValidationDigest> {
        let digest = self
            .snapshot
            .validation(&self.owner)
            .view(d)
            .map(|(_, digest)| digest);
        self.reads.record(
            ReadKey::ValidationView(d),
            digest.map_or_else(ContentHash::default, |digest| digest.0),
        );
        digest
    }
    /// The owner universe's V errors, recording `UniverseErrors`.
    pub fn universe_errors(&mut self) -> ContentHash {
        let (digest, _) = self.snapshot.validation(&self.owner).universe_errors();
        self.reads.record(ReadKey::UniverseErrors, digest);
        digest
    }
    /// The owner universe's first heap role, recording `FirstHeap`.
    pub fn first_heap(&mut self) -> Option<DefId> {
        let heap = self
            .snapshot
            .validation(&self.owner)
            .first_heap()
            .map(|(d, _)| d);
        self.reads.record(
            ReadKey::FirstHeap,
            heap.map_or_else(ContentHash::default, |d| d.path().fingerprint()),
        );
        heap
    }
    /// `DeclarationCandidates(d)`, recorded.
    pub fn declaration_candidates(&mut self, d: DefId) -> Option<ContentHash> {
        let candidates = self.snapshot.declaration_candidates(d);
        self.reads.record(
            ReadKey::DeclarationCandidates(d),
            candidates.unwrap_or_default(),
        );
        candidates
    }
}
impl SignatureProvider for FrozenProvider<'_> {
    fn signature(&mut self, definition: DefId) -> Result<SignatureRef, QueryError> {
        match self.snapshot.signature(definition) {
            Some(Ok(signature)) => {
                self.reads
                    .record(ReadKey::Signature(definition), signature.fingerprint);
                Ok(signature.clone())
            }
            Some(Err(error)) => {
                self.reads
                    .record(ReadKey::FailedSignature(definition), failed(error));
                Err(error.clone())
            }
            None => {
                let error = QueryError::NotImplemented;
                self.reads
                    .record(ReadKey::FailedSignature(definition), failed(&error));
                Err(error)
            }
        }
    }
    fn lookup(&mut self, context: &FileContext, path: &Path) -> LookupOutput {
        let output = self.snapshot.lookup(context, path);
        let segments: Vec<String> = path
            .segments
            .iter()
            .map(|s| s.as_str().to_owned())
            .collect();
        self.reads.record(
            ReadKey::Lookup(LookupKey::of(
                LookupSource::Provider,
                context,
                path.package.as_ref(),
                &segments,
            )),
            crate::queries::memos::provider_value(&output.result),
        );
        output
    }
}

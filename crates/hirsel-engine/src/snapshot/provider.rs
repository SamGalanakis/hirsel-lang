//! `FrozenProvider`: a body's signature provider over a frozen snapshot.
//!
//! P workers read signatures, lookups and private views from the snapshot
//! without locks, and every read is recorded into the task's `ReadsBuilder`
//! with the value it observed, so the memo the task produces can be verified
//! by comparing observed values alone (design §1(b), "Typed reads").
use super::SignatureSnapshot;
use crate::memo::{LookupKey, LookupSource, PackageKey, ReadKey, ReadsBuilder};
use hirsel_syntax::{ContentHash, DefId, FileContext, PackageId, Path};
use hirsel_types::{LookupOutput, QueryError, SignatureProvider, SignatureRef};

/// A signature provider for bodies of `owner`'s package, reading `snapshot`.
pub struct FrozenProvider<'a> {
    snapshot: &'a SignatureSnapshot,
    owner: PackageId,
    reads: &'a mut ReadsBuilder,
    private_shapes: bool,
    definition: Option<DefId>,
    citations: Option<&'a dyn super::CitationSource>,
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
            private_shapes: false,
            definition: None,
            citations: None,
        }
    }
    /// A body query's provider, selecting its package's private views while
    /// recording the same public-signature and representation receipts as
    /// the live engine recorder.
    pub fn for_body(
        snapshot: &'a SignatureSnapshot,
        owner: PackageId,
        reads: &'a mut ReadsBuilder,
    ) -> Self {
        Self {
            snapshot,
            owner,
            reads,
            private_shapes: true,
            definition: None,
            citations: None,
        }
    }
    /// A body worker's complete signature/citation provider over immutable
    /// signature and current-source inputs. Each worker owns its read builder.
    pub fn for_definition(
        snapshot: &'a SignatureSnapshot,
        owner: DefId,
        citations: &'a dyn super::CitationSource,
        reads: &'a mut ReadsBuilder,
    ) -> Self {
        let mut provider = Self::for_body(snapshot, owner.path().0.package.clone(), reads);
        provider.definition = Some(owner);
        provider.citations = Some(citations);
        provider
    }
    /// `d`'s view as the owner's bodies see it, recording `Signature(d)` (or
    /// `FailedSignature(d)`) and, for an opaque type seen privately,
    /// `Representation(d)` with the private view's digest.
    pub fn body_view(&mut self, d: DefId) -> Result<SignatureRef, QueryError> {
        let public = self.public_signature(d)?;
        let view = self
            .snapshot
            .body_view(&self.owner, d)
            .and_then(Result::ok)
            .unwrap_or(&public);
        if public.metadata.visibility == hirsel_syntax::Visibility::PublicOpaque
            && public.definition.path().0.package == self.owner
            && (self.snapshot.private.contains_key(&d)
                || self.snapshot.lookup.candidates.contains_key(&d))
        {
            let digest = self
                .snapshot
                .representation(d)
                .and_then(Result::ok)
                .map_or_else(ContentHash::default, |(_, digest)| digest.0);
            self.reads.record(ReadKey::Representation(d), digest);
        }
        Ok(view.clone())
    }
    /// V's digest of `d`'s view in the owner's universe, recording
    /// `ValidationView(d)`.
    pub fn validation_view(&mut self, d: DefId) -> Option<hirsel_validate::ValidationDigest> {
        let digest = self
            .snapshot
            .validation(&self.owner)
            .view(d)
            .map(|input| input.digest());
        self.reads.record(
            ReadKey::ValidationView(d),
            digest.map_or_else(ContentHash::default, |digest| digest.0),
        );
        digest
    }
    /// Every view in the originating package's base V universe. This is a
    /// separate receipt from its errors: an unchanged error-free base can
    /// have a different verdict when an evaluation replaces its owner.
    pub fn validation_universe(&mut self) -> ContentHash {
        let digest = self.snapshot.validation_input_digest(&self.owner);
        self.reads.record(
            ReadKey::ValidationUniverse(PackageKey::of(&self.owner)),
            digest,
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
            candidates.unwrap_or_else(crate::queries::memos::no_candidates),
        );
        candidates
    }
}
impl FrozenProvider<'_> {
    fn public_signature(&mut self, definition: DefId) -> Result<SignatureRef, QueryError> {
        match self.snapshot.signature(definition) {
            Some(Ok(signature)) => {
                self.reads
                    .record(ReadKey::Signature(definition), signature.fingerprint);
                Ok(signature.clone())
            }
            Some(Err(error)) => {
                self.reads.record(
                    ReadKey::FailedSignature(definition),
                    self.snapshot.failed_signature_receipt(definition),
                );
                Err(error.clone())
            }
            None => {
                let error = QueryError::NotImplemented;
                self.reads.record(
                    ReadKey::FailedSignature(definition),
                    self.snapshot.failed_signature_receipt(definition),
                );
                Err(error)
            }
        }
    }
}
impl SignatureProvider for FrozenProvider<'_> {
    fn signature(&mut self, definition: DefId) -> Result<SignatureRef, QueryError> {
        if self.private_shapes {
            self.body_view(definition)
        } else {
            self.public_signature(definition)
        }
    }
    fn cite_unique_failure(
        &mut self,
        target: &hirsel_syntax::DefPath,
        claim: hirsel_types::UniqueClaim,
    ) -> Option<hirsel_types::UniqueFailure> {
        let source = self.citations?;
        let owner = self.definition?;
        let target = DefId::intern(target);
        self.reads
            .record(ReadKey::CitedText(target), source.cited_text(target));
        source.unique_failure(owner, target, claim).or_else(|| {
            source.cited_name(owner, target)?;
            self.snapshot
                .header_products()?
                .unique_failure(target, claim)
        })
    }
    fn cite_name(&mut self, target: &hirsel_syntax::DefPath) -> Option<hirsel_span::Span> {
        let source = self.citations?;
        let owner = self.definition?;
        let target = DefId::intern(target);
        self.reads
            .record(ReadKey::CitedText(target), source.cited_text(target));
        source.cited_name(owner, target)
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

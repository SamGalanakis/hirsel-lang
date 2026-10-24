//! One immutable source snapshot's selected V-owned Lower envelopes. Decoding
//! is miss-only and uses V's existing exact body/environment rechecks.
use crate::{
    NativeRecordContext, NativeRecordSource, QueryEngine, QueryError, SignatureSnapshot, Verify,
};
use hirsel_syntax::{ContentHash, DefPath};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

struct Selected {
    output: hirsel_validate::VerifiedDefinitionOutput,
    implementation_revision: ContentHash,
    signature: ContentHash,
}
/// Source query inputs for native workers. There are no DefinitionRefs here;
/// the signature snapshot is the existing immutable independently checked view.
/// A handle cannot be supplied from arbitrary persisted associations or tokens.
pub struct NativeLowerReader {
    anchors: NativeRecordContext,
    signatures: Arc<SignatureSnapshot>,
    cache: Arc<hirsel_validate::VerifierCache>,
    policy: ContentHash,
    selected: BTreeMap<DefPath, Selected>,
    instances: hirsel_tir::InstanceBodies,
}
/// One demanded original source body, carrying only the actual selected V
/// output and source observations. It grants no native Value/Ready authority.
pub struct NativeLowerBody {
    definition: hirsel_tir::DefinitionRef,
    output: hirsel_validate::VerifiedDefinitionOutput,
    implementation_revision: ContentHash,
}
impl NativeLowerBody {
    pub fn definition(&self) -> &hirsel_tir::DefinitionRef {
        &self.definition
    }
    pub fn output(&self) -> &hirsel_validate::VerifiedDefinitionOutput {
        &self.output
    }
    pub fn implementation_revision(&self) -> ContentHash {
        self.implementation_revision
    }
}
impl NativeLowerReader {
    /// Only a selected current source query can provide this body's envelope.
    /// V rechecks its exact artifact and private input observations after type
    /// and source rebinding. CT5 uses are checked by the same source validator
    /// as the live native source attachment. No native stage is inferred.
    pub fn read(&self, path: &DefPath) -> Option<NativeLowerBody> {
        let selected = self.selected.get(path)?;
        let definition = self.cache.read_definition(
            &selected.output,
            self.anchors.record_types().clone(),
            |owner, occurrence| self.anchors.record_source_anchor(owner, occurrence),
            self.policy,
            self.signatures.validation(&path.0.package),
        )?;
        if definition.path.path() != path
            || definition.signature_hash != selected.signature
            || !hirsel_validate::validate_instance_uses(
                self.anchors.record_types(),
                &self.instances,
                path,
                &definition,
            )
            .is_empty()
        {
            return None;
        }
        Some(NativeLowerBody {
            definition,
            output: selected.output.clone(),
            implementation_revision: selected.implementation_revision,
        })
    }
    /// Observations of a current source-query-selected envelope, without
    /// decoding its body. Query metadata alone is not a verification identity.
    pub fn input(
        &self,
        path: &DefPath,
    ) -> Option<(
        &hirsel_validate::VerifiedDefinitionOutput,
        ContentHash,
        ContentHash,
    )> {
        let selected = self.selected.get(path)?;
        Some((
            &selected.output,
            selected.implementation_revision,
            selected.signature,
        ))
    }
    /// An inventory lookup opens no source or native body.
    pub fn contains(&self, path: &DefPath) -> bool {
        self.selected.contains_key(path)
    }
}
impl QueryEngine {
    /// Capture source-query-selected envelopes without decoding source bodies.
    /// Availability is explicit: VerifyOff, recomputation policy, absent local
    /// verifier transport returns None. An unavailable individual envelope
    /// remains absent from the reader and cannot be materialized.
    /// The ordinary source producer must run for such a miss; no proof is
    /// reconstructed from ordinary product-store bytes.
    pub fn native_lower_reader(
        &mut self,
        paths: &[DefPath],
    ) -> Result<Option<NativeLowerReader>, QueryError> {
        if self.verify != Verify::On || !self.native_query_policy().reuse {
            return Ok(None);
        }
        let Some(cache) = self.product_store().and_then(|store| store.verifier()) else {
            return Ok(None);
        };
        let mut selected = BTreeMap::new();
        let paths: BTreeSet<_> = paths.iter().cloned().collect();
        for path in &paths {
            let source = self.native_definition_product(path)?;
            if !crate::queries::generation::executable(&source.source.syntax) {
                continue;
            }
            let Some(output) = source.lower else {
                continue;
            };
            selected.insert(
                path.clone(),
                Selected {
                    output,
                    implementation_revision: source.source.implementation_revision,
                    signature: source.signature.fingerprint,
                },
            );
        }
        Ok(Some(NativeLowerReader {
            anchors: NativeRecordContext::capture(self),
            signatures: self.signature_snapshot(),
            cache,
            policy: self.source_verification_policy(),
            selected,
            instances: self.instance_bodies(&paths),
        }))
    }
}

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
    cache: Option<Arc<hirsel_validate::VerifierCache>>,
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
        let definition = self.cache.as_ref()?.read_definition(
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
            cache: Some(cache),
            policy: self.source_verification_policy(),
            selected,
            instances: self.instance_bodies(&paths),
        }))
    }
}

/// A selected ordinary Lower archive is data, not V authority. The worker
/// decodes it only on a native miss and runs the same independent source V
/// against the frozen private view when Verify is on.
struct OrdinarySource {
    reference: hirsel_artifact_store::Reference,
    implementation_revision: ContentHash,
    signature: ContentHash,
}
pub const NATIVE_ORIGINAL_BODY: hirsel_artifact_store::Domain = hirsel_artifact_store::Domain {
    kind: 0x4e00_0024,
    schema: 1,
};
pub struct NativeOriginalReader {
    verified: NativeLowerReader,
    ordinary: BTreeMap<DefPath, OrdinarySource>,
    records: crate::NativeRecords,
    store: Arc<dyn crate::store::ResultStore>,
    verify: Verify,
}
pub struct NativeOriginalBody {
    definition: hirsel_tir::DefinitionRef,
    output: Option<hirsel_validate::VerifiedDefinitionOutput>,
    implementation_revision: ContentHash,
    verification_ran: bool,
}
impl NativeOriginalBody {
    pub fn definition(&self) -> &hirsel_tir::DefinitionRef {
        &self.definition
    }
    pub fn output(&self) -> Option<&hirsel_validate::VerifiedDefinitionOutput> {
        self.output.as_ref()
    }
    pub fn implementation_revision(&self) -> ContentHash {
        self.implementation_revision
    }
    pub fn verification_ran(&self) -> bool {
        self.verification_ran
    }
}
impl NativeOriginalReader {
    pub fn input(
        &self,
        path: &DefPath,
    ) -> Option<(
        Option<&hirsel_validate::VerifiedDefinitionOutput>,
        Option<hirsel_artifact_store::Reference>,
        ContentHash,
        ContentHash,
    )> {
        if let Some((output, revision, signature)) = self.verified.input(path) {
            return Some((Some(output), None, revision, signature));
        }
        let input = self.ordinary.get(path)?;
        Some((
            None,
            Some(input.reference),
            input.implementation_revision,
            input.signature,
        ))
    }
    pub fn has_ordinary(&self) -> bool {
        !self.ordinary.is_empty()
    }
    pub fn read(&self, path: &DefPath) -> Option<NativeOriginalBody> {
        if self.verified.contains(path) {
            let body = self.verified.read(path)?;
            return Some(NativeOriginalBody {
                definition: body.definition,
                output: Some(body.output),
                implementation_revision: body.implementation_revision,
                verification_ran: false,
            });
        }
        let source = self.ordinary.get(path)?;
        let bytes = self.store.get_content(source.reference)?;
        let definition: hirsel_tir::DefinitionRef =
            self.records.decode(&self.verified.anchors, &bytes)?;
        if definition.path.path() != path
            || definition.signature_hash != source.signature
            || !hirsel_validate::validate_instance_uses(
                self.verified.anchors.record_types(),
                &self.verified.instances,
                path,
                &definition,
            )
            .is_empty()
        {
            return None;
        }
        let verification_ran = if self.verify == Verify::On {
            let _time = hirsel_intern::timings::phase(hirsel_intern::timings::Phase::Verify);
            let (report, ran, _, _) = hirsel_validate::validate_definition(
                &definition,
                self.verified.anchors.record_types().clone(),
                |anchor| self.verified.anchors.record_owner(anchor),
                self.verified.signatures.validation(&path.0.package),
                self.verified.policy,
                self.verified.cache.as_deref(),
            );
            if ran {
                hirsel_intern::timings::count(hirsel_intern::timings::Count::VRuns);
            }
            if hirsel_validate::tooling_limit_error(&report).is_some()
                || !report.violations.is_empty()
            {
                return None;
            }
            ran
        } else {
            false
        };
        Some(NativeOriginalBody {
            definition,
            output: None,
            implementation_revision: source.implementation_revision,
            verification_ran,
        })
    }
}
impl QueryEngine {
    /// Complete ordinary source queries only where their current V envelope is
    /// unavailable, transporting one original body at a time through the sole
    /// type/anchor codec. No Definition collection survives this method.
    /// VerifyOff/recomputation/absent verifier never manufacture a V token.
    pub fn native_original_reader(
        &mut self,
        paths: &[DefPath],
        transport: Arc<dyn crate::store::ResultStore>,
    ) -> Result<NativeOriginalReader, QueryError> {
        let records = crate::NativeRecords::new(self.types.clone(), transport.clone());
        let cache = self.product_store().and_then(|store| store.verifier());
        let mut selected = BTreeMap::new();
        let mut ordinary = BTreeMap::new();
        let paths: BTreeSet<_> = paths.iter().cloned().collect();
        for path in &paths {
            let source = self.native_definition_product(path)?;
            if !crate::queries::generation::executable(&source.source.syntax) {
                continue;
            }
            let revision = source.source.implementation_revision;
            let signature = source.signature.fingerprint;
            if self.native_query_policy().reuse
                && cache.is_some()
                && let Some(output) = source.lower.clone()
            {
                selected.insert(
                    path.clone(),
                    Selected {
                        output,
                        implementation_revision: revision,
                        signature,
                    },
                );
                continue;
            }
            let (definition, output) = self.native_definition_body_with_output(&source)?;
            if let Some(output) = output.filter(|_| cache.is_some()) {
                selected.insert(
                    path.clone(),
                    Selected {
                        output,
                        implementation_revision: revision,
                        signature,
                    },
                );
                continue;
            }
            let bytes = records
                .encode(self, &definition)
                .ok_or_else(|| source_transport("ordinary Lower encoding failed"))?;
            let reference = transport
                .put_content(NATIVE_ORIGINAL_BODY, &bytes)
                .ok_or_else(|| source_transport("ordinary Lower transport failed"))?;
            if NATIVE_ORIGINAL_BODY.reference(&bytes) != Some(reference) {
                return Err(source_transport("ordinary Lower content identity differs"));
            }
            ordinary.insert(
                path.clone(),
                OrdinarySource {
                    reference,
                    implementation_revision: revision,
                    signature,
                },
            );
        }
        Ok(NativeOriginalReader {
            verified: NativeLowerReader {
                anchors: NativeRecordContext::capture(self),
                signatures: self.signature_snapshot(),
                cache,
                policy: self.source_verification_policy(),
                selected,
                instances: self.instance_bodies(&paths),
            },
            ordinary,
            records,
            store: transport,
            verify: self.verify,
        })
    }
}
fn source_transport(message: &str) -> QueryError {
    QueryError::Operational(hirsel_types::OperationalFailure {
        operation: "native original source transport".into(),
        message: message.into(),
    })
}

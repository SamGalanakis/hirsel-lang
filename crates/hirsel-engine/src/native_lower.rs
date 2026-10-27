//! One immutable source snapshot's selected V-owned Lower envelopes. Decoding
//! is miss-only and uses V's existing exact body/environment rechecks.
use crate::{
    DefinitionQueries, NativeRecordContext, NativeRecordSource, QueryEngine, QueryError,
    SignatureSnapshot, Verify,
};
use hirsel_syntax::{ContentHash, DefPath};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

/// A settled front Lower result. The ordinary payload is always available;
/// an optional opaque V output can avoid rechecking only in its own context.
/// These observations are data and grant no native C authority.
#[derive(Clone)]
pub struct NativeOriginalProduct {
    pub definition: DefPath,
    pub implementation_revision: ContentHash,
    pub signature: ContentHash,
    pub body: hirsel_artifact_store::Reference,
    pub verified: Option<hirsel_validate::VerifiedDefinitionOutput>,
}

/// Source bindings captured before Lower tasks run. This contains no source
/// Definition or Lower output; sparse readers consume only settled demands.
pub struct NativeOriginalContext {
    owner: Option<Arc<dyn Send + Sync>>,
    anchors: Arc<NativeRecordContext>,
    signatures: Arc<SignatureSnapshot>,
    cache: Option<Arc<hirsel_validate::VerifierCache>>,
    policy: ContentHash,
    instances: Arc<hirsel_tir::InstanceBodies>,
    expected: BTreeMap<DefPath, (ContentHash, ContentHash)>,
    verify: Verify,
    reuse: bool,
}
impl NativeOriginalContext {
    pub(crate) fn retain_owner(mut self, owner: Arc<dyn Send + Sync>) -> Self {
        self.owner = Some(owner);
        self
    }

    /// Bind exact front observations without opening an ordinary source body.
    /// Unknown/stale source identities and duplicate demands are refused.
    pub fn reader(
        &self,
        products: impl IntoIterator<Item = NativeOriginalProduct>,
        store: Arc<dyn crate::store::ResultStore>,
    ) -> Result<NativeOriginalReader, QueryError> {
        let mut selected = BTreeMap::new();
        let mut ordinary = BTreeMap::new();
        let mut seen = BTreeSet::new();
        for product in products {
            let path = product.definition;
            if !seen.insert(path.clone())
                || self.expected.get(&path)
                    != Some(&(product.implementation_revision, product.signature))
                || product.body.domain != NATIVE_ORIGINAL_BODY
                || product
                    .verified
                    .as_ref()
                    .is_some_and(|output| output.owner().path() != &path)
            {
                return Err(source_transport(
                    "settled Lower differs from its source snapshot",
                ));
            }
            // The request's ordinary transport outlives best-effort verifier
            // storage. Keep its descriptor even when a V output is available.
            ordinary.insert(
                path.clone(),
                OrdinarySource {
                    reference: product.body,
                    implementation_revision: product.implementation_revision,
                    signature: product.signature,
                },
            );
            if self.reuse
                && self.cache.is_some()
                && let Some(output) = product.verified
            {
                selected.insert(
                    path,
                    Selected {
                        output,
                        implementation_revision: product.implementation_revision,
                        signature: product.signature,
                    },
                );
            }
        }
        Ok(NativeOriginalReader {
            owner: self.owner.clone(),
            verified: NativeLowerReader {
                anchors: self.anchors.clone(),
                signatures: self.signatures.clone(),
                cache: self.cache.clone(),
                policy: self.policy,
                selected,
                instances: self.instances.clone(),
            },
            ordinary,
            records: crate::NativeRecords::new(self.anchors.record_types().clone(), store.clone()),
            store,
            verify: self.verify,
            reuse: self.reuse,
        })
    }
}
impl QueryEngine {
    /// Freeze current source/private observations without demanding Lower.
    pub fn native_original_context(
        &mut self,
        paths: &[DefPath],
    ) -> Result<NativeOriginalContext, QueryError> {
        let paths: BTreeSet<_> = paths.iter().cloned().collect();
        let mut expected = BTreeMap::new();
        for path in &paths {
            let syntax = self.parse_definition(path)?.value;
            let signature = self.item_signature(path)?;
            if crate::queries::has_errors(&signature.diagnostics) {
                return Err(QueryError::Diagnostics(signature.diagnostics));
            }
            if crate::queries::generation::executable(&syntax) {
                expected.insert(
                    path.clone(),
                    (syntax.text_hash, signature.value.fingerprint),
                );
            }
        }
        Ok(NativeOriginalContext {
            owner: None,
            anchors: Arc::new(NativeRecordContext::capture(self)),
            signatures: self.signature_snapshot(),
            cache: self.product_store().and_then(|store| store.verifier()),
            policy: self.source_verification_policy(),
            instances: Arc::new(self.instance_bodies(&paths)),
            expected,
            verify: self.verify,
            reuse: self.native_query_policy().reuse,
        })
    }
}

struct Selected {
    output: hirsel_validate::VerifiedDefinitionOutput,
    implementation_revision: ContentHash,
    signature: ContentHash,
}
/// Source query inputs for native workers. There are no DefinitionRefs here;
/// the signature snapshot is the existing immutable independently checked view.
/// A handle cannot be supplied from arbitrary persisted associations or tokens.
pub struct NativeLowerReader {
    anchors: Arc<NativeRecordContext>,
    signatures: Arc<SignatureSnapshot>,
    cache: Option<Arc<hirsel_validate::VerifierCache>>,
    policy: ContentHash,
    selected: BTreeMap<DefPath, Selected>,
    instances: Arc<hirsel_tir::InstanceBodies>,
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
            anchors: Arc::new(NativeRecordContext::capture(self)),
            signatures: self.signature_snapshot(),
            cache: Some(cache),
            policy: self.source_verification_policy(),
            selected,
            instances: Arc::new(self.instance_bodies(&paths)),
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
    owner: Option<Arc<dyn Send + Sync>>,
    verified: NativeLowerReader,
    ordinary: BTreeMap<DefPath, OrdinarySource>,
    records: crate::NativeRecords,
    store: Arc<dyn crate::store::ResultStore>,
    verify: Verify,
    reuse: bool,
}
pub struct NativeOriginalBody {
    _owner: Option<Arc<dyn Send + Sync>>,
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
    /// The owning immutable signature/private-view snapshot, for binding
    /// settled source metadata without decoding an original body.
    pub fn signatures(&self) -> &Arc<SignatureSnapshot> {
        &self.verified.signatures
    }
    /// Current CT-5 catalogue captured with this original source snapshot.
    /// It contains selected declaration/actual/path data, never native code or
    /// a substitute source/V witness for the selected monomorphic definition.
    pub fn instance_bodies(&self) -> &hirsel_tir::InstanceBodies {
        &self.verified.instances
    }

    /// Copied reader maps and path buffers only. Shared source/signature/V,
    /// interner, CT-5 and store payloads retain their original admission owners.
    pub fn retained_metadata_bytes(&self) -> u64 {
        let mut weight = crate::NativeMetadataWeight::default();
        weight.add(std::mem::size_of::<Self>());
        weight.map(&self.verified.selected, |weight, path, _| weight.path(path));
        weight.map(&self.ordinary, |weight, path, _| weight.path(path));
        weight.bytes()
    }

    pub fn record_context(&self) -> Arc<NativeRecordContext> {
        self.verified.anchors.clone()
    }
    pub fn verification(&self) -> Verify {
        self.verify
    }
    pub fn query_policy(&self) -> crate::NativeQueryPolicy {
        crate::NativeQueryPolicy {
            source: self.verified.policy,
            reuse: self.reuse && !self.has_ordinary(),
        }
    }

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
        self.ordinary
            .keys()
            .any(|path| !self.verified.contains(path))
    }
    pub fn read(&self, path: &DefPath) -> Option<NativeOriginalBody> {
        if let Some(body) = self.verified.read(path) {
            return Some(NativeOriginalBody {
                _owner: self.owner.clone(),
                definition: body.definition,
                output: Some(body.output),
                implementation_revision: body.implementation_revision,
                verification_ran: false,
            });
        }
        let source = self.ordinary.get(path)?;
        let bytes = self.store.get_content(source.reference)?;
        let definition: hirsel_tir::DefinitionRef = self
            .records
            .decode(self.verified.anchors.as_ref(), &bytes)?;
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
        let output = if let Some((output, _, _)) = self.verified.input(path) {
            if !output.matches_definition(
                &definition,
                self.verified.anchors.record_types().clone(),
                |anchor| self.verified.anchors.record_owner(anchor),
                self.verified.policy,
                self.verified.signatures.validation(&path.0.package),
            ) {
                return None;
            }
            Some(output.clone())
        } else {
            None
        };
        let verification_ran = if output.is_none() && self.verify == Verify::On {
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
            _owner: self.owner.clone(),
            definition,
            output,
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
            owner: None,
            verified: NativeLowerReader {
                anchors: Arc::new(NativeRecordContext::capture(self)),
                signatures: self.signature_snapshot(),
                cache,
                policy: self.source_verification_policy(),
                selected,
                instances: Arc::new(self.instance_bodies(&paths)),
            },
            ordinary,
            records,
            store: transport,
            verify: self.verify,
            reuse: self.native_query_policy().reuse,
        })
    }
}
fn source_transport(message: &str) -> QueryError {
    QueryError::Operational(hirsel_types::OperationalFailure {
        operation: "native original source transport".into(),
        message: message.into(),
    })
}

impl QueryEngine {
    /// Revalidate an admitted native snapshot into a reader with private V storage.
    /// This also supports callers whose ordinary compilation retains no products.
    pub(crate) fn native_snapshot_reader(
        &mut self,
        input: &crate::NativeInput,
    ) -> Result<NativeOriginalReader, QueryError> {
        if self.verify != Verify::On {
            return Err(source_transport("sealed source snapshots require V"));
        }
        let cache = Arc::new(hirsel_validate::VerifierCache::default());
        let anchors = Arc::new(NativeRecordContext::capture(self));
        let signatures = self.signature_snapshot();
        let policy = self.source_verification_policy();
        let mut selected = BTreeMap::new();
        for (path, definition) in &input.definitions {
            let view = signatures.validation(&path.0.package);
            let (report, _, _, evidence) = hirsel_validate::validate_definition(
                definition,
                input.types.clone(),
                |anchor| anchors.record_owner(anchor),
                view,
                policy,
                Some(cache.as_ref()),
            );
            if !report.violations.is_empty() {
                return Err(source_transport(&format!(
                    "sealed source snapshot validation: {:?}",
                    report.violations
                )));
            }
            let output = cache
                .definition_output_from_evidence(
                    definition,
                    input.types.clone(),
                    policy,
                    view,
                    &evidence.ok_or_else(|| {
                        source_transport("sealed source snapshot lacks V evidence")
                    })?,
                )
                .ok_or_else(|| source_transport("sealed source snapshot lacks V output"))?;
            selected.insert(
                path.clone(),
                Selected {
                    output,
                    implementation_revision: input.source_owners[path].implementation_revision,
                    signature: input.signatures[path].fingerprint,
                },
            );
        }
        let store: Arc<dyn crate::store::ResultStore> = Arc::new(crate::store::MemStore::default());
        Ok(NativeOriginalReader {
            owner: None,
            verified: NativeLowerReader {
                anchors,
                signatures,
                cache: Some(cache),
                policy,
                selected,
                instances: Arc::new(input.instance_bodies.clone()),
            },
            ordinary: BTreeMap::new(),
            records: crate::NativeRecords::new(input.types.clone(), store.clone()),
            store,
            verify: self.verify,
            reuse: true,
        })
    }
}

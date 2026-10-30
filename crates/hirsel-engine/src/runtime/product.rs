//! Admitted typed values. Producers retain their existing snapshot reservation;
//! this owner additionally admits every value allocation it keeps alive.
use super::MemoryReservation;
use crate::memo::{
    DepthEvidence,
    retained::{Weight, retained_arc_bytes},
};
use crate::{
    NativeRecordContext, NativeRecordSource, NativeRecords, PublishedSummary, Reads, SummaryView,
};
use hirsel_artifact_store::{Action, Address, Bytes, Domain, Reference};
use hirsel_span::Span;
use hirsel_tir::DefinitionRef;
use hirsel_types::{QueryOutput, TypeInterner};
use std::{
    io,
    sync::{Arc, Mutex},
};

pub const LIVE_SUMMARY: Domain = Domain {
    kind: 0x5150_0001,
    schema: 1,
};
pub const LIVE_LOWER: Domain = Domain {
    kind: 0x5150_0002,
    schema: 2,
};

pub enum ArchiveAdmission<'a> {
    Retained,
    Materialize {
        scratch: &'a mut MemoryReservation,
        working_bytes: u64,
    },
}

#[derive(Clone)]
pub(crate) struct ProductSource {
    context: NativeRecordContext,
    records: NativeRecords,
    _backing: Arc<dyn Send + Sync>,
}
impl ProductSource {
    pub(crate) fn new(
        context: NativeRecordContext,
        records: NativeRecords,
        backing: Arc<dyn Send + Sync>,
    ) -> Option<Self> {
        records.uses_types(context.record_types()).then_some(Self {
            context,
            records,
            _backing: backing,
        })
    }
}

struct Owner<T> {
    address: Address,
    action: Action,
    domain: Domain,
    value: T,
    source: ProductSource,
    archive: Mutex<Option<ProductArchive>>,
    _reservation: MemoryReservation,
}
impl<T> Owner<T> {
    #[cfg(test)]
    fn archive(
        &self,
        scratch: &mut MemoryReservation,
        required: u64,
        publish: impl FnOnce(&T, &ProductSource, &MemoryReservation) -> io::Result<Vec<u8>>,
    ) -> io::Result<ProductArchive> {
        self.archive_with(
            ArchiveAdmission::Materialize {
                scratch,
                working_bytes: required,
            },
            publish,
        )
    }
    fn archive_with(
        &self,
        admission: ArchiveAdmission<'_>,
        publish: impl FnOnce(&T, &ProductSource, &MemoryReservation) -> io::Result<Vec<u8>>,
    ) -> io::Result<ProductArchive> {
        let mut archive = self.archive.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(archive) = &*archive {
            return Ok(archive.clone());
        }
        let ArchiveAdmission::Materialize {
            scratch,
            working_bytes: required,
        } = admission
        else {
            return Err(io::Error::other(
                "live product archive needs working admission",
            ));
        };
        let mut grant = scratch
            .split(required)
            .ok_or_else(|| io::Error::other("live product archive admission exhausted"))?;
        // The owning codec declares its complete temporary working bound and
        // runs only under that partition. No encoding occurs during admission.
        let result = publish(&self.value, &self.source, &grant).and_then(|bytes| {
            let retained = archive_bytes(bytes.capacity())
                .ok_or_else(|| io::Error::other("live product archive layout overflow"))?;
            if retained > grant.bytes() {
                return Err(io::Error::other(
                    "live product archive backing admission exhausted",
                ));
            }
            let reference = self
                .domain
                .reference(&bytes)
                .ok_or_else(|| io::Error::other("live product archive exceeds the record limit"))?;
            let reservation = grant.split(retained).ok_or_else(|| {
                io::Error::other("live product archive backing admission exhausted")
            })?;
            Ok(ProductArchive(ArchiveBacking::Materialized(Arc::new(
                ArchiveOwner {
                    bytes,
                    reference,
                    _reservation: reservation,
                },
            ))))
        });
        if scratch.join(grant).is_err() {
            return Err(io::Error::other("live product archive partition changed"));
        }
        let materialized = result?;
        *archive = Some(materialized.clone());
        Ok(materialized)
    }
}

/// Canonical bytes and their reference share their full backing admission.
/// Publication to any store is separate; cache eviction cannot invalidate an
/// archive still held by a product or an external reader.
#[derive(Clone)]
pub struct ProductArchive(ArchiveBacking);
#[derive(Clone)]
enum ArchiveBacking {
    Materialized(Arc<ArchiveOwner>),
    Imported(Arc<ImportedArchive>),
}
struct ImportedArchive {
    reference: Reference,
    bytes: Bytes,
    admission: ImportedAdmission,
}
enum ImportedAdmission {
    Owned(MemoryReservation),
    Shared {
        bytes: u64,
        _owner: Arc<dyn Send + Sync>,
    },
}
impl ImportedAdmission {
    fn bytes(&self) -> u64 {
        match self {
            Self::Owned(reservation) => reservation.bytes(),
            Self::Shared { bytes, .. } => *bytes,
        }
    }
}
struct ArchiveOwner {
    bytes: Vec<u8>,
    reference: Reference,
    _reservation: MemoryReservation,
}
impl ProductArchive {
    pub fn bytes(&self) -> &[u8] {
        match &self.0 {
            ArchiveBacking::Materialized(owner) => &owner.bytes,
            ArchiveBacking::Imported(owner) => &owner.bytes,
        }
    }
    pub fn reference(&self) -> Reference {
        match &self.0 {
            ArchiveBacking::Materialized(owner) => owner.reference,
            ArchiveBacking::Imported(owner) => owner.reference,
        }
    }
    pub fn retained_bytes(&self) -> u64 {
        match &self.0 {
            ArchiveBacking::Materialized(owner) => owner._reservation.bytes(),
            ArchiveBacking::Imported(owner) => owner.admission.bytes(),
        }
    }
    /// Import checked bytes under a complete backing reservation. A small view
    /// admits the entire region it keeps alive, including across eviction.
    pub fn open(
        store: &dyn crate::store::ResultStore,
        reference: Reference,
        allowance: &mut MemoryReservation,
    ) -> io::Result<Self> {
        let bytes = store
            .get_content(reference)
            .ok_or_else(|| io::Error::other("product archive is unavailable"))?;
        let charge = Self::checked_import_charge(reference, &bytes)?;
        let reservation = allowance
            .split(charge)
            .ok_or_else(|| io::Error::other("product archive backing admission exhausted"))?;
        Ok(Self(ArchiveBacking::Imported(Arc::new(ImportedArchive {
            reference,
            bytes,
            admission: ImportedAdmission::Owned(reservation),
        }))))
    }
    pub(crate) fn checked_import_charge(reference: Reference, bytes: &Bytes) -> io::Result<u64> {
        if reference.domain.reference(bytes) != Some(reference) {
            return Err(io::Error::other(
                "product archive content differs from its reference",
            ));
        }
        let mut retained = Some(retained_arc_bytes(
            &std::mem::MaybeUninit::<ImportedArchive>::uninit(),
        ));
        bytes.visit_retained_allocations(|_, bytes| {
            retained = retained.and_then(|n| n.checked_add(bytes as u64));
        });
        retained.ok_or_else(|| io::Error::other("product archive backing layout overflow"))
    }
    pub(crate) fn from_admitted_content(
        reference: Reference,
        bytes: Bytes,
        charge: u64,
        owner: Arc<dyn Send + Sync>,
    ) -> Self {
        Self(ArchiveBacking::Imported(Arc::new(ImportedArchive {
            reference,
            bytes,
            admission: ImportedAdmission::Shared {
                bytes: charge,
                _owner: owner,
            },
        })))
    }
}
fn archive_bytes(capacity: usize) -> Option<u64> {
    retained_arc_bytes(&std::mem::MaybeUninit::<ArchiveOwner>::uninit())
        .checked_add(capacity.try_into().ok()?)
}

/// A Summary view retains the complete immutable producer, type/source owners
/// and its own value admission. There are no reusable numeric session slots.
#[derive(Clone)]
pub struct LiveSummaryProduct {
    current: Arc<Owner<Arc<PublishedSummary>>>,
    facet: SummaryFacet,
}

/// Only the settled semantic projection. Positions, reads and actions are
/// consumed through the current product and cannot be observed through this
/// facet. Keeping it retains its complete original admitted backing.
#[derive(Clone)]
pub struct SummaryFacet(Arc<Owner<Arc<PublishedSummary>>>);
impl SummaryFacet {
    pub fn same_owner(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
    pub fn equivalent(&self, other: &Self) -> bool {
        self.same_owner(other)
            || SummaryView::from(&*self.0.value).equivalent(
                self.0.source.context.record_types(),
                SummaryView::from(&*other.0.value),
                other.0.source.context.record_types(),
            )
    }
}
impl LiveSummaryProduct {
    pub(crate) fn value_bytes(value: &PublishedSummary) -> Option<u64> {
        // Storage equations have a separate admitted owner. Ordinary Summary
        // publication does not silently accept an unknown backing layout.
        if value.storage.is_some() {
            return None;
        }
        let mut weight = Weight::default();
        weight.add(retained_arc_bytes(value) as usize);
        weight.definition_summary(&value.summary);
        weight.parameter_growth(&value.growth);
        if let Some(checked) = &value.checked {
            weight.arc(checked, Weight::checked_summary);
        }
        weight.reads(&value.reads);
        Some(weight.bytes + owner_bytes::<Arc<PublishedSummary>>())
    }
    pub(crate) fn new(
        identity: (Address, Action),
        value: Arc<PublishedSummary>,
        source: ProductSource,
        allowance: &mut MemoryReservation,
    ) -> Option<Self> {
        let reservation = allowance.split(Self::value_bytes(&value)?)?;
        let current = Arc::new(Owner {
            address: identity.0,
            action: identity.1,
            domain: LIVE_SUMMARY,
            value,
            source,
            archive: Mutex::new(None),
            _reservation: reservation,
        });
        Some(Self {
            facet: SummaryFacet(current.clone()),
            current,
        })
    }
    pub fn value(&self) -> &PublishedSummary {
        &self.current.value
    }
    pub fn reads(&self) -> &Reads {
        &self.current.value.reads
    }
    pub fn address(&self) -> Address {
        self.current.address
    }
    pub fn action(&self) -> Action {
        self.current.action
    }
    pub fn domain(&self) -> Domain {
        self.current.domain
    }
    pub fn types(&self) -> &Arc<TypeInterner> {
        self.current.source.context.record_types()
    }
    pub fn retained_bytes(&self) -> u64 {
        self.current._reservation.bytes()
            + if Arc::ptr_eq(&self.current, &self.facet.0) {
                0
            } else {
                self.facet.0._reservation.bytes()
            }
    }
    pub fn facet(&self) -> &SummaryFacet {
        &self.facet
    }
    pub fn equivalent(&self, other: &Self) -> bool {
        self.facet.equivalent(&other.facet)
    }
    /// Reuse the old settled value while retaining this computation's current
    /// source product, complete reads and action. Intermediate products are
    /// not chained: only the original equal facet survives.
    pub fn backdated(&self, previous: &Self) -> Option<Self> {
        self.equivalent(previous).then(|| Self {
            current: self.current.clone(),
            facet: previous.facet.clone(),
        })
    }
    /// Materialize at most once successfully. Admission or encoding failure
    /// leaves the immutable live value retryable. Stores publish these bytes
    /// separately and bind their complete computation associations as before.
    pub fn archive(
        &self,
        scratch: &mut MemoryReservation,
        required: u64,
    ) -> io::Result<ProductArchive> {
        self.archive_with(ArchiveAdmission::Materialize {
            scratch,
            working_bytes: required,
        })
    }
    pub(super) fn archive_with(
        &self,
        admission: ArchiveAdmission<'_>,
    ) -> io::Result<ProductArchive> {
        self.current
            .archive_with(admission, |value, source, grant| {
                value
                    .reads
                    .encoding_working_bytes()
                    .and_then(|working| {
                        source.records.encode_with_admission(
                            &source.context,
                            &**value,
                            grant,
                            working,
                        )
                    })
                    .ok_or_else(|| io::Error::other("live Summary archive encoding refused"))
            })
    }
}

/// The ordinary Lower value and complete source observations. Acceptance
/// remains in its existing verifier-owned product, outside facet equality.
#[derive(Clone)]
pub struct LiveLowerProduct(Arc<Owner<LowerValue>>);
struct LowerValue {
    output: crate::FrontLowerOutput,
}
impl LiveLowerProduct {
    pub fn required_bytes(value: &crate::FrontLowerOutput) -> Option<u64> {
        let mut weight = Weight::default();
        for (_, bytes) in value.reads.allocations() {
            weight.add(bytes);
        }
        weight.add(size_of_val(&*value.depth.first));
        let mut materialized = crate::NativeMetadataWeight::default();
        materialized.add(
            value
                .materialized
                .capacity()
                .checked_mul(size_of::<(hirsel_syntax::ExprId, hirsel_tir::Materialized)>())?,
        );
        materialized.vec(&value.selections, |w, (_, arguments)| w.flat_vec(arguments));
        for (_, value) in &value.materialized {
            materialized.materialized(value);
        }
        owner_bytes::<LowerValue>()
            .checked_add(crate::memo::retained_tir_bytes(&value.output.value))?
            .checked_add(crate::memo::retained_lowering_output_bytes(
                &value.output.diagnostics,
                &value.output.dependencies,
            ))?
            .checked_add(weight.bytes)?
            .checked_add(materialized.bytes())?
            .checked_add(
                value
                    .execution
                    .as_ref()
                    .map_or(Some(0), |r| r.retained_bytes())?,
            )?
            .checked_add(
                value
                    .verified
                    .as_ref()
                    .map_or(Some(0), |v| v.retained_bytes())?,
            )
    }
    pub(crate) fn new(
        identity: (Address, Action),
        output: crate::FrontLowerOutput,
        source: ProductSource,
        allowance: &mut MemoryReservation,
    ) -> Option<Self> {
        let reservation = allowance.split(Self::required_bytes(&output)?)?;
        Some(Self(Arc::new(Owner {
            address: identity.0,
            action: identity.1,
            domain: LIVE_LOWER,
            value: LowerValue { output },
            source,
            archive: Mutex::new(None),
            _reservation: reservation,
        })))
    }
    pub fn output(&self) -> &QueryOutput<DefinitionRef> {
        &self.0.value.output.output
    }
    pub fn reads(&self) -> &Reads {
        &self.0.value.output.reads
    }
    pub fn depth(&self) -> &DepthEvidence<Span> {
        &self.0.value.output.depth
    }
    /// Complete observations and local verifier-owned evidence from the
    /// original Lower operation. Archive bytes do not grant this authority.
    pub fn observations(&self) -> &crate::FrontLowerOutput {
        &self.0.value.output
    }
    pub fn address(&self) -> Address {
        self.0.address
    }
    pub fn action(&self) -> Action {
        self.0.action
    }
    pub fn domain(&self) -> Domain {
        self.0.domain
    }
    pub fn types(&self) -> &Arc<TypeInterner> {
        self.0.source.context.record_types()
    }
    pub fn retained_bytes(&self) -> u64 {
        self.0._reservation.bytes()
    }
    pub fn archive(
        &self,
        scratch: &mut MemoryReservation,
        required: u64,
    ) -> io::Result<ProductArchive> {
        self.archive_with(ArchiveAdmission::Materialize {
            scratch,
            working_bytes: required,
        })
    }
    pub(super) fn archive_with(
        &self,
        admission: ArchiveAdmission<'_>,
    ) -> io::Result<ProductArchive> {
        self.0.archive_with(admission, |value, source, grant| {
            value
                .output
                .reads
                .encoding_working_bytes()
                .and_then(|working| {
                    source.records.encode_with_admission(
                        &source.context,
                        &(
                            &value.output.output,
                            &value.output.reads,
                            &value.output.depth,
                            &value.output.materialized,
                            value.output.implementation_revision,
                            value.output.signature,
                            value.output.executed,
                            &value.output.selections,
                        ),
                        grant,
                        working,
                    )
                })
                .ok_or_else(|| io::Error::other("live Lower archive encoding refused"))
        })
    }
}

pub const LIVE_ANALYZED: Domain = Domain {
    kind: 0x5150_0003,
    schema: 1,
};

/// A demanded post-Modes body retains its complete typed allocation and source
/// backing. It carries no verifier acceptance for a subsequently lowered body.
#[derive(Clone)]
pub struct LiveAnalyzedProduct(Arc<Owner<AnalyzedValue>>);
struct AnalyzedValue {
    output: Arc<QueryOutput<crate::AnalyzedBody>>,
    reads: Arc<Reads>,
    phase: Option<crate::QueryKind>,
    diagnostic_phases: Vec<(hirsel_diag::Diagnostic, String)>,
}
impl LiveAnalyzedProduct {
    pub fn required_bytes(outcome: &crate::ModeResult) -> Option<u64> {
        let output = outcome.result.as_ref().ok()?;
        let mut weight = Weight::default();
        weight.reads(&outcome.reads);
        weight.vec(&outcome.diagnostic_phases, |w, (diagnostic, phase)| {
            w.diagnostic(diagnostic);
            w.add(phase.capacity());
        });
        owner_bytes::<AnalyzedValue>()
            .checked_add(crate::memo::retained::retained_analyzed_bytes(output))?
            .checked_add(weight.bytes)
    }
    pub(crate) fn new(
        identity: (Address, Action),
        outcome: crate::ModeResult,
        source: ProductSource,
        allowance: &mut MemoryReservation,
    ) -> Option<Self> {
        let reservation = allowance.split(Self::required_bytes(&outcome)?)?;
        let output = outcome.result.ok()?;
        Some(Self(Arc::new(Owner {
            address: identity.0,
            action: identity.1,
            domain: LIVE_ANALYZED,
            value: AnalyzedValue {
                output,
                reads: outcome.reads,
                phase: outcome.phase,
                diagnostic_phases: outcome.diagnostic_phases,
            },
            source,
            archive: Mutex::new(None),
            _reservation: reservation,
        })))
    }
    pub fn output(&self) -> &Arc<QueryOutput<crate::AnalyzedBody>> {
        &self.0.value.output
    }
    pub fn reads(&self) -> &Arc<Reads> {
        &self.0.value.reads
    }
    pub fn phase(&self) -> Option<crate::QueryKind> {
        self.0.value.phase
    }
    pub fn diagnostic_phases(&self) -> &[(hirsel_diag::Diagnostic, String)] {
        &self.0.value.diagnostic_phases
    }
    pub fn address(&self) -> Address {
        self.0.address
    }
    pub fn action(&self) -> Action {
        self.0.action
    }
    pub fn domain(&self) -> Domain {
        self.0.domain
    }
    pub fn retained_bytes(&self) -> u64 {
        self.0._reservation.bytes()
    }
    pub fn types(&self) -> &Arc<TypeInterner> {
        self.0.source.context.record_types()
    }
    pub fn archive(
        &self,
        scratch: &mut MemoryReservation,
        required: u64,
    ) -> io::Result<ProductArchive> {
        self.0.archive_with(
            ArchiveAdmission::Materialize {
                scratch,
                working_bytes: required,
            },
            |value, source, grant| {
                crate::queries::encode_live_analyzed(
                    &source.records,
                    &source.context,
                    &value.output,
                    &value.reads,
                    value.phase,
                    &value.diagnostic_phases,
                    grant,
                )
                .ok_or_else(|| io::Error::other("live analyzed body archive encoding refused"))
            },
        )
    }
}

fn owner_bytes<T>() -> u64 {
    retained_arc_bytes(&std::mem::MaybeUninit::<Owner<T>>::uninit())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::ProcessBudget;
    use crate::{CheckedSummary, DefinitionSummary, ParameterGrowth, PhaseAdapters, QueryEngine};
    use hirsel_syntax::{ContentHash, DefId, DefPath};

    fn fixture() -> (Arc<PublishedSummary>, NativeRecordContext) {
        let engine = QueryEngine::new(crate::default_environment(), PhaseAdapters::default());
        let owner = DefId::intern(&DefPath::core("live-summary"));
        let checked = Arc::new(CheckedSummary {
            summary: DefinitionSummary {
                owner,
                frames: [].into(),
                calls: [].into(),
                staged: [].into(),
            },
            growth: ParameterGrowth::default(),
        });
        let value = Arc::new(PublishedSummary {
            definition: owner,
            summary: checked.summary.clone(),
            growth: checked.growth.clone(),
            storage: None,
            checked: Some(checked),
            reads: Arc::new(Reads::empty()),
            text: ContentHash::default(),
        });
        (value, engine.source_records())
    }
    fn descriptor() -> (Address, Action) {
        (
            Address(ContentHash::default()),
            Action(ContentHash::default()),
        )
    }
    fn product_source(
        context: NativeRecordContext,
        backing: Arc<dyn Send + Sync>,
    ) -> ProductSource {
        let records = NativeRecords::new(
            context.record_types().clone(),
            Arc::new(crate::store::MemStore::default()),
        );
        ProductSource::new(context, records, backing).unwrap()
    }
    #[test]
    fn live_summary_exact_admission_keeps_the_producer_until_eviction_and_last_view_drop() {
        let (value, source) = fixture();
        let bytes = LiveSummaryProduct::value_bytes(&value).unwrap();
        let budget = ProcessBudget::new(bytes * 2 + 17);
        let backing = Arc::new(budget.reserve(17).unwrap());
        let producer = Arc::downgrade(&backing);
        let mut short = budget.reserve(bytes - 1).unwrap();
        assert!(
            LiveSummaryProduct::new(
                descriptor(),
                value.clone(),
                product_source(source.clone(), backing.clone()),
                &mut short
            )
            .is_none()
        );
        assert_eq!(short.bytes(), bytes - 1);
        drop(short);
        let mut exact = budget.reserve(bytes).unwrap();
        let product = LiveSummaryProduct::new(
            descriptor(),
            value,
            product_source(source, backing.clone()),
            &mut exact,
        )
        .unwrap();
        assert_eq!(exact.bytes(), 0);
        assert_eq!(product.retained_bytes(), bytes);
        let view = product.clone();
        drop(backing);
        let mut retained = crate::memo::Holds::new(bytes);
        let key = crate::memo::MemoKey::new(crate::QueryKind::Summary, product.value().definition);
        retained.hold(key, product, bytes);
        assert_eq!(retained.set_budget(0), vec![key]);
        assert!(producer.upgrade().is_some());
        assert_eq!(budget.reserved(), bytes + 17);
        assert_eq!(view.value().definition, key.def);
        drop(view);
        assert!(producer.upgrade().is_none());
        assert_eq!(budget.reserved(), 0);
    }
    #[test]
    fn live_archive_refusal_and_materialization_failure_do_not_cache_success() {
        let (value, source) = fixture();
        let budget = ProcessBudget::new(32 << 20);
        let backing = Arc::new(budget.reserve(1).unwrap());
        let mut allowance = budget
            .reserve(LiveSummaryProduct::value_bytes(&value).unwrap())
            .unwrap();
        let product = LiveSummaryProduct::new(
            descriptor(),
            value,
            product_source(source, backing),
            &mut allowance,
        )
        .unwrap();
        let required = archive_bytes(8).unwrap();
        let mut short = budget.reserve(required - 1).unwrap();
        assert!(
            product
                .current
                .archive(&mut short, required, |_, _, _| panic!(
                    "short grant ran archive"
                ))
                .is_err()
        );
        assert_eq!(short.bytes(), required - 1);
        drop(short);
        let mut scratch = budget.reserve(required).unwrap();
        assert!(
            product
                .current
                .archive(&mut scratch, required, |_, _, _| Err(io::Error::other(
                    "codec refused"
                )))
                .is_err()
        );
        assert_eq!(
            scratch.bytes(),
            required,
            "failed archive returns its working partition"
        );
        assert!(
            product
                .current
                .archive(&mut scratch, required, |_, _, _| Ok(vec![0; 9]))
                .is_err()
        );
        assert_eq!(scratch.bytes(), required);
        let oversized = hirsel_artifact_store::MAX_RECORD + 1;
        let oversized_charge = archive_bytes(oversized).unwrap();
        let mut oversized_grant = budget.reserve(oversized_charge).unwrap();
        assert!(
            product
                .current
                .archive(&mut oversized_grant, oversized_charge, |_, _, _| Ok(
                    vec![0; oversized]
                ))
                .is_err()
        );
        assert_eq!(
            oversized_grant.bytes(),
            oversized_charge,
            "record-limit refusal returns the entire partition"
        );
        drop(oversized_grant);
        let archive = product
            .current
            .archive(&mut scratch, required, |_, _, grant| {
                assert_eq!(grant.bytes(), required);
                Ok(b"complete".to_vec())
            })
            .unwrap();
        assert_eq!(scratch.bytes(), 0);
        assert_eq!(
            archive.reference(),
            LIVE_SUMMARY.reference(b"complete").unwrap()
        );
        drop(scratch);
        let mut empty = budget.reserve(0).unwrap();
        assert_eq!(
            product
                .current
                .archive(&mut empty, u64::MAX, |_, _, _| panic!(
                    "archive encoded twice"
                ))
                .unwrap()
                .reference(),
            archive.reference()
        );
        assert_eq!(product.value().reads.len(), 0);
        drop(product);
        assert_eq!(budget.reserved(), archive.retained_bytes());
        assert_eq!(archive.bytes(), b"complete");
        drop(archive);
        assert_eq!(budget.reserved(), 0);
    }

    #[test]
    fn summary_archive_pins_its_codec_and_refuses_foreign_source_binding() {
        for (name, read_count, type_edges) in [
            ("archive-admission-".repeat(1024), 0usize, 0usize),
            ("archive-reads".into(), 1024, 0),
            ("archive-types".into(), 0, 1024),
        ] {
            let (mut value, source) = fixture();
            let owner = DefId::intern(&DefPath::core(&name));
            let changed = Arc::get_mut(&mut value).unwrap();
            changed.definition = owner;
            changed.summary.owner = owner;
            changed.reads = Arc::new(
                Reads::from_vec(
                    (0..read_count)
                        .map(|index| {
                            let owner =
                                DefId::intern(&DefPath::core(&format!("archive-read-{index}")));
                            let mut output = [0; 32];
                            output[..8].copy_from_slice(&(index as u64).to_le_bytes());
                            (crate::ReadKey::Signature(owner), ContentHash(output))
                        })
                        .collect(),
                )
                .unwrap(),
            );
            if type_edges != 0 {
                let types = source.record_types();
                let unit = types.intern(hirsel_types::Type::unit());
                let tuple = types.intern(hirsel_types::Type::with_kind(
                    hirsel_types::TypeKind::Tuple(vec![unit; type_edges].into()),
                ));
                changed.summary.frames = Box::new([crate::Frame {
                    site: hirsel_types::NameSite::Item,
                    parent: None,
                    kind: crate::FrameKind::Item,
                    generic: false,
                    size_bindings: Box::new([]),
                }]);
                changed.summary.calls = Box::new([crate::SummaryCall {
                    callee: owner,
                    binder: hirsel_syntax::LexicalOwner::first(owner.path())
                        .site(hirsel_types::NameSite::Item),
                    arguments: Box::new([hirsel_types::GenericArgument::Type(tuple)]),
                    owning_frame: 0,
                    target_frame: None,
                    class: crate::CallClass::StaticSelection,
                    site: Span::default(),
                }]);
            }
            Arc::get_mut(changed.checked.as_mut().unwrap())
                .unwrap()
                .summary = changed.summary.clone();
            let (_, foreign) = fixture();
            let budget = ProcessBudget::new(2 << 20);
            let backing = Arc::new(budget.reserve(17).unwrap());
            let mut allowance = budget
                .reserve(LiveSummaryProduct::value_bytes(&value).unwrap())
                .unwrap();
            let records = NativeRecords::new(
                source.record_types().clone(),
                Arc::new(crate::store::MemStore::default()),
            );
            let foreign = NativeRecords::new(
                foreign.record_types().clone(),
                Arc::new(crate::store::MemStore::default()),
            );
            assert!(ProductSource::new(source.clone(), foreign, backing.clone()).is_none());
            assert_eq!(
                allowance.bytes(),
                LiveSummaryProduct::value_bytes(&value).unwrap()
            );
            let product = LiveSummaryProduct::new(
                descriptor(),
                value.clone(),
                ProductSource::new(source.clone(), records.clone(), backing).unwrap(),
                &mut allowance,
            )
            .unwrap();
            let working = 4096;
            let mut short = budget.reserve(working).unwrap();
            let (refused, allocated) =
                crate::test_alloc::measured(|| product.archive(&mut short, working));
            assert!(refused.is_err());
            assert!(
                allocated.peak <= working as isize,
                "archive refusal allocated {} bytes under a {working}-byte working partition",
                allocated.peak
            );
            assert_eq!(short.bytes(), working);
            drop(short);
            let expected = records.encode(&source, &*value).unwrap();
            let mut scratch = budget.reserve(1 << 20).unwrap();
            assert_eq!(scratch.bytes(), 1 << 20);
            let archive = product.archive(&mut scratch, 1 << 20).unwrap();
            assert_eq!(archive.bytes(), expected);
            let decoded: PublishedSummary = records.decode(&source, archive.bytes()).unwrap();
            assert_eq!(decoded.definition, value.definition);
            assert_eq!(decoded.summary, value.summary);
            assert_eq!(decoded.growth, value.growth);
            assert_eq!(
                decoded.checked.as_ref().unwrap().summary,
                value.checked.as_ref().unwrap().summary
            );
            assert_eq!(
                decoded.checked.as_ref().unwrap().growth,
                value.checked.as_ref().unwrap().growth
            );
            assert_eq!(decoded.reads, value.reads);
            assert_eq!(decoded.text, value.text);
            let mut empty = budget.reserve(0).unwrap();
            assert_eq!(
                product.archive(&mut empty, u64::MAX).unwrap().reference(),
                archive.reference(),
                "cached materialization does not require another working grant",
            );
            drop(product);
            drop(scratch);
            assert_eq!(budget.reserved(), archive.retained_bytes());
            drop(archive);
            assert_eq!(budget.reserved(), 0);
        }
    }

    #[test]
    fn imported_archive_admits_complete_range_backing_and_refuses_substitution() {
        struct Store(Bytes);
        impl crate::store::ResultStore for Store {
            fn get(&self, _: &crate::store::EntryKey) -> Option<Bytes> {
                None
            }
            fn put(&self, _: &crate::store::EntryKey, _: &[u8]) {}
            fn name(&self) -> &'static str {
                "live-archive-test"
            }
            fn get_content(&self, _: Reference) -> Option<Bytes> {
                Some(self.0.clone())
            }
        }
        let mut backing = vec![0; 4096];
        backing[100..108].copy_from_slice(b"complete");
        let bytes = Bytes::from(backing).range(100, 8, 1).unwrap();
        let reference = LIVE_SUMMARY.reference(b"complete").unwrap();
        let mut charge = retained_arc_bytes(&std::mem::MaybeUninit::<ImportedArchive>::uninit());
        bytes.visit_retained_allocations(|_, bytes| charge += bytes as u64);
        assert!(charge > 4096);
        let budget = ProcessBudget::new(charge);
        let mut short = budget.reserve(charge - 1).unwrap();
        assert!(ProductArchive::open(&Store(bytes.clone()), reference, &mut short).is_err());
        assert_eq!(short.bytes(), charge - 1);
        drop(short);
        let mut exact = budget.reserve(charge).unwrap();
        assert!(
            ProductArchive::open(
                &Store(Bytes::from(b"replaced".as_slice())),
                reference,
                &mut exact
            )
            .is_err()
        );
        assert_eq!(
            exact.bytes(),
            charge,
            "substitution is refused before retaining bytes"
        );
        let store = Store(bytes);
        let archive = ProductArchive::open(&store, reference, &mut exact).unwrap();
        assert_eq!(exact.bytes(), 0);
        assert_eq!(archive.retained_bytes(), charge);
        drop(store);
        drop(exact);
        let reader = archive.clone();
        drop(archive);
        assert_eq!(budget.reserved(), charge);
        assert_eq!(reader.bytes(), b"complete");
        drop(reader);
        assert_eq!(budget.reserved(), 0);
    }

    #[test]
    fn equal_live_summary_keeps_the_old_facet_and_new_action_and_observations() {
        let (value, source) = fixture();
        let budget = ProcessBudget::new(1 << 20);
        let old_backing = Arc::new(budget.reserve(17).unwrap());
        let old_producer = Arc::downgrade(&old_backing);
        let new_backing = Arc::new(budget.reserve(19).unwrap());
        let new_producer = Arc::downgrade(&new_backing);
        let mut allowance = budget.reserve(16 << 10).unwrap();
        let old = LiveSummaryProduct::new(
            descriptor(),
            value.clone(),
            product_source(source.clone(), old_backing),
            &mut allowance,
        )
        .unwrap();
        let mut value = (*value).clone();
        let dependency = DefId::intern(&DefPath::core("new-read"));
        let mut reads = crate::ReadsBuilder::new();
        reads.record(
            crate::ReadKey::Signature(dependency),
            ContentHash::of_parts("test", [b"new-read".as_slice()]),
        );
        value.reads = Arc::new(reads.finish().unwrap());
        value.text = ContentHash::of_parts("test", [b"new-source".as_slice()]);
        let action = Action(ContentHash::of_parts("test", [b"new-action".as_slice()]));
        let fresh = LiveSummaryProduct::new(
            (old.address(), action),
            Arc::new(value),
            product_source(source, new_backing),
            &mut allowance,
        )
        .unwrap();
        assert!(!fresh.facet().same_owner(old.facet()));
        let reused = fresh.backdated(&old).unwrap();
        assert!(reused.facet().same_owner(old.facet()));
        assert_eq!(reused.action(), action);
        assert_eq!(reused.reads().len(), 1);
        assert_eq!(old.reads().len(), 0);
        assert_eq!(
            reused.value().text,
            ContentHash::of_parts("test", [b"new-source".as_slice()])
        );
        assert_eq!(
            reused.retained_bytes(),
            fresh.retained_bytes() + old.retained_bytes()
        );
        drop(old);
        drop(fresh);
        drop(allowance);
        assert!(old_producer.upgrade().is_some());
        assert!(new_producer.upgrade().is_some());
        drop(reused);
        assert!(old_producer.upgrade().is_none());
        assert!(new_producer.upgrade().is_none());
        assert_eq!(budget.reserved(), 0);
    }
}

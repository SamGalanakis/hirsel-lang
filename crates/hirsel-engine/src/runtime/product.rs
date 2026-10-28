//! Admitted typed values. Producers retain their existing snapshot reservation;
//! this owner additionally admits every value allocation it keeps alive.
use super::MemoryReservation;
use crate::memo::{
    DepthEvidence,
    retained::{Weight, retained_arc_bytes},
};
use crate::{NativeRecordContext, NativeRecordSource, PublishedSummary, Reads, SummaryView};
use hirsel_artifact_store::{Action, Address, Domain, Reference};
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
    schema: 1,
};

struct Owner<T> {
    address: Address,
    action: Action,
    domain: Domain,
    value: T,
    source: NativeRecordContext,
    _backing: Arc<dyn Send + Sync>,
    archive: Mutex<Option<ProductArchive>>,
    _reservation: MemoryReservation,
}
impl<T> Owner<T> {
    fn archive(
        &self,
        scratch: &mut MemoryReservation,
        required: u64,
        publish: impl FnOnce(&T, &NativeRecordContext, &MemoryReservation) -> io::Result<Vec<u8>>,
    ) -> io::Result<ProductArchive> {
        let mut archive = self.archive.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(archive) = &*archive {
            return Ok(archive.clone());
        }
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
            Ok(ProductArchive(Arc::new(ArchiveOwner {
                bytes,
                reference,
                _reservation: reservation,
            })))
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
pub struct ProductArchive(Arc<ArchiveOwner>);
struct ArchiveOwner {
    bytes: Vec<u8>,
    reference: Reference,
    _reservation: MemoryReservation,
}
impl ProductArchive {
    pub fn bytes(&self) -> &[u8] {
        &self.0.bytes
    }
    pub fn reference(&self) -> Reference {
        self.0.reference
    }
    pub fn retained_bytes(&self) -> u64 {
        self.0._reservation.bytes()
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
                self.0.source.record_types(),
                SummaryView::from(&*other.0.value),
                other.0.source.record_types(),
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
        fn summary(w: &mut Weight<'_>, value: &crate::DefinitionSummary) {
            w.add(size_of_val(&*value.frames));
            for frame in &value.frames {
                w.add(size_of_val(&*frame.size_bindings));
            }
            w.add(size_of_val(&*value.calls));
            for call in &value.calls {
                w.add(size_of_val(&*call.arguments));
            }
            w.add(size_of_val(&*value.staged));
            for (_, parameters) in &value.staged {
                w.add(size_of_val(&**parameters));
            }
        }
        let mut weight = Weight::default();
        weight.add(retained_arc_bytes(value) as usize);
        summary(&mut weight, &value.summary);
        weight.add(size_of_val(&*value.growth.edges));
        if let Some(checked) = &value.checked {
            weight.arc(checked, |w, c| {
                summary(w, &c.summary);
                w.add(size_of_val(&*c.growth.edges));
            });
        }
        weight.reads(&value.reads);
        Some(weight.bytes + owner_bytes::<Arc<PublishedSummary>>())
    }
    pub(crate) fn new(
        identity: (Address, Action),
        value: Arc<PublishedSummary>,
        source: NativeRecordContext,
        backing: Arc<dyn Send + Sync>,
        allowance: &mut MemoryReservation,
    ) -> Option<Self> {
        let reservation = allowance.split(Self::value_bytes(&value)?)?;
        let current = Arc::new(Owner {
            address: identity.0,
            action: identity.1,
            domain: LIVE_SUMMARY,
            value,
            source,
            _backing: backing,
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
        self.current.source.record_types()
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
        publish: impl FnOnce(
            &PublishedSummary,
            &NativeRecordContext,
            &MemoryReservation,
        ) -> io::Result<Vec<u8>>,
    ) -> io::Result<ProductArchive> {
        self.current
            .archive(scratch, required, |value, source, grant| {
                publish(value, source, grant)
            })
    }
}

/// The ordinary Lower value and complete source observations. Acceptance
/// remains in its existing verifier-owned product, outside facet equality.
#[derive(Clone)]
pub struct LiveLowerProduct(Arc<Owner<LowerValue>>);
struct LowerValue {
    output: QueryOutput<DefinitionRef>,
    reads: Arc<Reads>,
    depth: DepthEvidence<Span>,
}
impl LiveLowerProduct {
    pub(crate) fn new(
        identity: (Address, Action),
        output: QueryOutput<DefinitionRef>,
        reads: Arc<Reads>,
        depth: DepthEvidence<Span>,
        source: NativeRecordContext,
        backing: Arc<dyn Send + Sync>,
        allowance: &mut MemoryReservation,
    ) -> Option<Self> {
        let mut weight = Weight::default();
        weight.reads(&reads);
        weight.add(size_of_val(&*depth.first));
        let bytes = owner_bytes::<LowerValue>()
            .checked_add(crate::memo::retained_tir_bytes(&output.value))?
            .checked_add(crate::memo::retained_lowering_output_bytes(
                &output.diagnostics,
                &output.dependencies,
            ))?
            .checked_add(weight.bytes)?;
        let reservation = allowance.split(bytes)?;
        Some(Self(Arc::new(Owner {
            address: identity.0,
            action: identity.1,
            domain: LIVE_LOWER,
            value: LowerValue {
                output,
                reads,
                depth,
            },
            source,
            _backing: backing,
            archive: Mutex::new(None),
            _reservation: reservation,
        })))
    }
    pub fn output(&self) -> &QueryOutput<DefinitionRef> {
        &self.0.value.output
    }
    pub fn reads(&self) -> &Reads {
        &self.0.value.reads
    }
    pub fn depth(&self) -> &DepthEvidence<Span> {
        &self.0.value.depth
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
        self.0.source.record_types()
    }
    pub fn retained_bytes(&self) -> u64 {
        self.0._reservation.bytes()
    }
    pub fn archive(
        &self,
        scratch: &mut MemoryReservation,
        required: u64,
        publish: impl FnOnce(
            &QueryOutput<DefinitionRef>,
            &Reads,
            &DepthEvidence<Span>,
            &NativeRecordContext,
            &MemoryReservation,
        ) -> io::Result<Vec<u8>>,
    ) -> io::Result<ProductArchive> {
        self.0.archive(scratch, required, |value, source, grant| {
            publish(&value.output, &value.reads, &value.depth, source, grant)
        })
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
                frames: Box::new([]),
                calls: Box::new([]),
                staged: Box::new([]),
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
                source.clone(),
                backing.clone(),
                &mut short
            )
            .is_none()
        );
        assert_eq!(short.bytes(), bytes - 1);
        drop(short);
        let mut exact = budget.reserve(bytes).unwrap();
        let product =
            LiveSummaryProduct::new(descriptor(), value, source, backing.clone(), &mut exact)
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
        let product =
            LiveSummaryProduct::new(descriptor(), value, source, backing, &mut allowance).unwrap();
        let required = archive_bytes(8).unwrap();
        let mut short = budget.reserve(required - 1).unwrap();
        assert!(
            product
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
                .archive(&mut scratch, required, |_, _, _| Ok(vec![0; 9]))
                .is_err()
        );
        assert_eq!(scratch.bytes(), required);
        let oversized = hirsel_artifact_store::MAX_RECORD + 1;
        let oversized_charge = archive_bytes(oversized).unwrap();
        let mut oversized_grant = budget.reserve(oversized_charge).unwrap();
        assert!(
            product
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
            source.clone(),
            old_backing,
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
            source,
            new_backing,
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

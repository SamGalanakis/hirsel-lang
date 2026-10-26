//! Inactive source coverage for every finalizer a typed cleanup root can
//! reach. Runtime cursor custody and close protocols are deliberately separate.
use super::{FinalizerFrame, FinalizerSource};
use crate::continuation::{ContinuationPlans, PlanInputs};
use crate::layout_types::{LayoutId, LayoutKind, LayoutRecord, LayoutTable};
use crate::stage::{Ready, ValidatedUnit};
use hirsel_syntax::ContentHash;
use std::collections::{BTreeMap, BTreeSet, VecDeque};

#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct FinalizerCoverage {
    pub root: LayoutId,
    /// Exact descriptors include all variant/drop/obtain edges and their order.
    pub layouts: Vec<(LayoutId, LayoutRecord)>,
    pub finalizers: Vec<FinalizerFrame>,
    pub end: u64,
}

impl FinalizerCoverage {
    pub fn propose(
        root: LayoutId,
        sources: &BTreeMap<LayoutId, ContentHash>,
        units: &[&ValidatedUnit<Ready>],
        inputs: &PlanInputs<'_>,
        plans: &ContinuationPlans,
        after: u64,
    ) -> Result<Self, String> {
        crate::continuation::validate(units, inputs, plans).map_err(|e| e.to_string())?;
        Self::propose_extents(root, sources, units, inputs, plans, after)
    }

    pub(crate) fn propose_extents(
        root: LayoutId,
        sources: &BTreeMap<LayoutId, ContentHash>,
        units: &[&ValidatedUnit<Ready>],
        inputs: &PlanInputs<'_>,
        plans: &ContinuationPlans,
        after: u64,
    ) -> Result<Self, String> {
        let mut found = BTreeMap::new();
        let mut work = VecDeque::from([root]);
        while let Some(id) = work.pop_front() {
            if found.contains_key(&id) {
                continue;
            }
            let record = inputs.table.layout(id).ok_or("missing cleanup layout")?;
            work.extend(record.references());
            found.insert(id, record.clone());
        }
        let mut end = after;
        let mut finalizers = Vec::new();
        for (&layout, record) in &found {
            if record.finalizer.is_none() {
                continue;
            }
            let body = *sources
                .get(&layout)
                .ok_or("missing cleanup finalizer source")?;
            let source = FinalizerSource::derive(units, inputs.table, layout, body)?;
            let frame = FinalizerFrame::propose_extent(&source, units, inputs, plans, end)?;
            end = frame.end;
            finalizers.push(frame);
        }
        let proposal = Self {
            root,
            layouts: found.into_iter().collect(),
            finalizers,
            end,
        };
        proposal.check_extents(units, inputs.table, plans, after)?;
        Ok(proposal)
    }

    /// Traverse actual descriptor references, never a submitted edge list, and
    /// require exactly one checked source frame per reachable finalizer layout.
    pub fn check(
        &self,
        units: &[&ValidatedUnit<Ready>],
        inputs: &PlanInputs<'_>,
        plans: &ContinuationPlans,
        after: u64,
    ) -> Result<(), String> {
        crate::continuation::validate(units, inputs, plans).map_err(|e| e.to_string())?;
        self.check_extents(units, inputs.table, plans, after)
    }

    pub(crate) fn check_extents(
        &self,
        units: &[&ValidatedUnit<Ready>],
        table: &LayoutTable,
        plans: &ContinuationPlans,
        after: u64,
    ) -> Result<(), String> {
        if self.finalizers.len() > 64 {
            return Err("cleanup finalizer resolver capacity".into());
        }
        let mut reached = BTreeSet::new();
        let mut pending = vec![self.root];
        let mut expected = BTreeSet::new();
        while let Some(id) = pending.pop() {
            if !reached.insert(id) {
                continue;
            }
            let actual = table.layout(id).ok_or("unknown cleanup layout")?;
            let matching: Vec<_> = self.layouts.iter().filter(|(key, _)| *key == id).collect();
            if matching.len() != 1 || &matching[0].1 != actual {
                return Err("cleanup descriptor coverage".into());
            }
            if matches!(
                actual.kind,
                LayoutKind::Callable { environment: None }
                    | LayoutKind::Iterator { .. }
                    | LayoutKind::Opaque { .. }
            ) {
                return Err("unproved erased cleanup source".into());
            }
            if actual.finalizer.is_some() {
                expected.insert(id);
            }
            pending.extend(actual.references());
        }
        if self.layouts.iter().map(|(id, _)| *id).collect::<Vec<_>>()
            != reached.into_iter().collect::<Vec<_>>()
            || self
                .finalizers
                .iter()
                .map(|f| f.source.layout)
                .collect::<Vec<_>>()
                != expected.into_iter().collect::<Vec<_>>()
        {
            return Err("cleanup finalizer graph coverage".into());
        }
        let mut end = after;
        for finalizer in &self.finalizers {
            finalizer.check_extent(units, table, plans, end)?;
            end = finalizer.end;
        }
        if end != self.end {
            return Err("cleanup finalizer frame bound".into());
        }
        Ok(())
    }
}

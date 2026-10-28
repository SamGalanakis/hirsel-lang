use super::*;

impl Selection {
    /// Resource selection follows the complete checked layout graph, including
    /// private storage and all variant arms. Unknown dynamic custody stays in
    /// the plan so existing transport checks must still prove it.
    pub(super) fn tracks_layout(self, layouts: &LayoutTable, root: LayoutId) -> bool {
        if self == Self::Owned {
            return true;
        }
        let mut pending = vec![root];
        let mut seen = BTreeSet::new();
        while let Some(layout) = pending.pop() {
            if !seen.insert(layout) {
                continue;
            }
            let Some(record) = layouts.layout(layout) else {
                return true;
            };
            // An endpoint stores no inline Resource. Queue contents belong to
            // the runtime object, with separate committed transport checks.
            // This cannot authorize a Resource-bearing runtime argument.
            if super::runtime_calls::channel_handle(layouts, layout) {
                continue;
            }
            if record.finalizer.is_some()
                || matches!(
                    record.kind,
                    LayoutKind::Resource
                        | LayoutKind::SealedRegion { .. }
                        | LayoutKind::Callable { .. }
                        | LayoutKind::Iterator { .. }
                )
                || (matches!(record.kind, LayoutKind::Handle)
                    && !resource_free_handle(layouts, layout)
                    && !super::runtime_calls::plain_channel_handle(layouts, layout)
                    && !data_handle(record))
            {
                return true;
            }
            let mut stored = record.clone();
            remove_channel_obtains(layouts, &mut stored.scan);
            stored.for_each_reference_mut(&mut |id| pending.push(*id));
        }
        false
    }
}

// Aggregate scans flatten their fields' handle rows. Their `obtains` edge is
// runtime queue custody, while fields and drop children still name storage.
// Retain every other edge, including a Resource also present as an inline field.
fn remove_channel_obtains(layouts: &LayoutTable, steps: &mut [crate::layout_types::ScanStep]) {
    use crate::layout_types::{HandleKind, ScanStep};
    for step in steps {
        match step {
            ScanStep::Handle { kind: HandleKind::Receiver, ty: Some(schema), obtains, .. }
                if super::runtime_calls::channel_handle(layouts, *schema)
                    && layouts.layout(*schema).is_some_and(|record|
                        matches!(record.scan.as_slice(), [ScanStep::Handle { obtains: expected, .. }] if expected == obtains)) =>
            {
                *obtains = None;
            }
            ScanStep::ActiveVariant { variants, .. } => {
                for (_, steps) in variants {
                    remove_channel_obtains(layouts, steps);
                }
            }
            _ => (),
        }
    }
}

// These handles expose their entire possible payload in the checked layout
// graph. Following references below also checks private storage, every variant
// arm and recursive mutex wrappers. Channels still need committed FIFO custody.
fn data_handle(record: &crate::layout_types::LayoutRecord) -> bool {
    use crate::layout_types::{HandleKind, ScanStep};
    !record.scan.is_empty()
        && record.scan.iter().all(|step| {
            matches!(
                step,
                ScanStep::Handle {
                    kind: HandleKind::Mutex | HandleKind::Task,
                    obtains: Some(_),
                    ..
                } | ScanStep::Handle {
                    kind: HandleKind::Scope | HandleKind::Atomic,
                    obtains: None,
                    ..
                }
            )
        })
}

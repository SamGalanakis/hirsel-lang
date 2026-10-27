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
                    && !data_handle(record))
            {
                return true;
            }
            pending.extend(record.references());
        }
        false
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

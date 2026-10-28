//! Result bindings live in scheduler rows across parks and compact resumes.
//! No parked TaskCtx or language payload supplies a shadow identity.
use super::{Buffer, Row, TaskCtx, observer};
use std::sync::{Arc, Weak};

#[derive(Clone, Copy)]
pub(crate) struct Binding {
    pub task: u64,
    pub frame: u64,
    pub marker: u32,
}
pub(crate) struct Record {
    pub binding: Binding,
    rows: Weak<Buffer<Row>>,
}
pub(crate) struct Held {
    pub record: Record,
    pub layout: u32,
}
pub(crate) struct Join {
    pub record: Record,
    pub child: u64,
}
impl Record {
    pub(crate) fn write(&self, row: Row) {
        if let Some(rows) = self.rows.upgrade() {
            let _ = rows.push(row);
        }
    }
    pub(crate) fn lost(&self) {
        self.write(Row::ObservationLost);
    }
}
unsafe fn take(ctx: *mut TaskCtx) -> Option<Record> {
    // SAFETY: the current task exclusively owns its context, under the role.
    unsafe {
        let binding = (*ctx).ownership_result.take()?;
        let observer = observer(ctx)?;
        Some(Record {
            binding,
            rows: Arc::downgrade(&(*observer).rows),
        })
    }
}
/// # Safety
/// ctx is the current task under its existing runtime role. descriptor names
/// the actual Option image passed to this TaskFinish invocation.
pub(crate) unsafe fn held(ctx: *mut TaskCtx, descriptor: u32) -> Option<Held> {
    let record = unsafe { take(ctx) }?;
    let Binding {
        task,
        frame,
        marker,
    } = record.binding;
    record.write(Row::TaskResultHeld {
        task,
        frame,
        marker,
        layout: descriptor,
    });
    Some(Held {
        record,
        layout: descriptor,
    })
}
/// # Safety
/// ctx is current under the existing role. child is this TaskJoin's actual
/// task-table word. Ordinary scope/handle validation retains its authority.
pub(crate) unsafe fn join(ctx: *mut TaskCtx, child: u64) -> Option<Join> {
    let record = unsafe { take(ctx) }?;
    let Binding {
        task,
        frame,
        marker,
    } = record.binding;
    record.write(Row::TaskResultJoin {
        task,
        frame,
        marker,
        child,
    });
    Some(Join { record, child })
}

//! CPU-only authority for the built-in concurrent core. Journals, including
//! Send recording writers, keep the synchronous affine route.
use super::*;
use crate::stop::{Stop, Stopped};

pub(crate) struct CpuReceipts(Arc<ConcurrentIo>);

impl CpuReceipts {
    pub(crate) fn admit(io: &Arc<ConcurrentIo>) -> Option<Self> {
        io.core
            .lock()
            .unwrap()
            .journal
            .absent()
            .then(|| Self(io.clone()))
    }

    /// The concurrent core's journal is fixed at admission. No arbitrary adapter
    /// or user closure can enter this route, including a Send journal's flush.
    fn invoke<T>(
        &self,
        ctx: *mut crate::TaskCtx,
        receipt: impl FnOnce(&mut RealCore<SendStreams>) -> Result<T, AdapterError>,
    ) -> Stopped<T> {
        let mut receipt = Some(receipt);
        let mut result = None;
        // SAFETY: the task retains its run for this synchronous, non-suspending
        // loan. The production gate arbitrates claim/withdraw and checks a held
        // compute permit's stop boundary before returning the receipt result.
        unsafe {
            crate::pool::call_local_in((*ctx).run_ptr(), &mut || {
                let run = (*ctx).run_ref();
                let mut core = self.0.core.lock().unwrap();
                let hint = &run.timer_hint;
                hint.publish(None);
                let points = hint.take_points();
                if points != 0 {
                    core.advance_ready_points(points);
                }
                result = Some(receipt.take().unwrap()(&mut core));
                hint.publish(core.ready_deadline());
            })
        }?;
        result
            .expect("a claimed CPU receipt returned a result")
            .map_err(Stop::refusal)
    }

    pub(crate) fn summary(&self, ctx: *mut crate::TaskCtx, summary: &CpuSummary) -> Stopped<()> {
        self.invoke(ctx, |core| core.cpu_summary(summary))
    }

    pub(crate) fn control(
        &self,
        ctx: *mut crate::TaskCtx,
        width: u32,
        action: u32,
        count: u64,
        value: Option<u32>,
    ) -> Stopped<Option<u32>> {
        self.invoke(ctx, |core| core.cpu_control(width, action, count, value))
    }
}

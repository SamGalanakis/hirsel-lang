//! Immutable proofs belong to a loaded program, independently of its runs.
use crate::{TaskCtx, ordered::capture::Types};

#[derive(Default)]
pub(crate) struct Image(std::sync::OnceLock<Metadata>);

pub(super) struct Metadata {
    pub types: Types,
    pub plain: super::drops::PlainLayouts,
}

impl Image {
    /// # Safety
    /// All contexts using this image have the same immutable linked layouts.
    pub(super) unsafe fn get(&self, ctx: *mut TaskCtx) -> &Metadata {
        self.0.get_or_init(|| {
            // SAFETY: the loaded program retains the same descriptor table
            // for every run; the result contains owned data, not ctx pointers.
            let types = unsafe { crate::ordered::capture::types(ctx) };
            let plain = unsafe { super::drops::plain_layouts(ctx, &types) };
            Metadata { types, plain }
        })
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn cached_proofs_outlive_a_run_without_sharing_its_object_type_updates() {
        use crate::ordered::{footprint::ObjectId, objects::Registry};
        let image = super::Image::default();
        unsafe {
            let mut ctx = std::ptr::null_mut();
            assert_eq!(
                crate::context::hirsel_rt_v2_ctx_create(&crate::Config::new(), &mut ctx),
                0
            );
            let types = image.get(ctx).types.clone();
            crate::context::hirsel_rt_v2_ctx_destroy(ctx);
            assert_eq!(
                crate::context::hirsel_rt_v2_ctx_create(&crate::Config::new(), &mut ctx),
                0
            );
            let next = image.get(ctx);
            let mut first = Registry::default();
            first.install_types(types);
            first.carries.define(0, [99].into());
            first.carries.register(
                0,
                ObjectId {
                    creator: vec![1],
                    ordinal: 0,
                },
            );
            assert_eq!(next.types.definitions[&0], Default::default());
            let plain = super::super::drops::plain_layouts(ctx, &next.types);
            for id in 0..(*ctx).heap_ref().layout_count() {
                assert_eq!(next.plain.contains(&id), plain.contains(&id));
            }
            crate::context::hirsel_rt_v2_ctx_destroy(ctx);
        }
    }
}

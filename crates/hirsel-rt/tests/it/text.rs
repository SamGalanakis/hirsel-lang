//! Text ABI, native byte storage and ownership across fresh and shared results.
use crate::support;
use hirsel_rt::text::*;
use hirsel_rt::views::Slice;
use hirsel_rt::{HEADER_SIZE, Header};
use support::Ctx;

unsafe fn shown<'a>(s: Slice) -> &'a [u8] {
    unsafe {
        std::slice::from_raw_parts(
            s.owner.cast::<u8>().add(HEADER_SIZE + 8 + s.start as usize),
            s.length as usize,
        )
    }
}
unsafe fn release(ctx: &Ctx, owner: *mut Header) {
    unsafe {
        assert_eq!(
            hirsel_rt::release::hirsel_rt_v2_release(
                ctx.0,
                owner,
                (*owner).index(),
                std::ptr::null(),
                0,
                0
            ),
            0
        );
    }
}

#[test]
fn byte_descriptor_and_text_records_match_derivation_on_both_targets() {
    use hirsel_layout::layout::{LayoutBuilder, Owners};

    use hirsel_lir::layout_types::{LayoutKind, ScalarKind, SizeSpec, TargetSpec};
    use hirsel_types::Primitive;
    use std::mem::{align_of, offset_of, size_of};
    for target in [
        TargetSpec::x86_64_unknown_linux_gnu(),
        TargetSpec::aarch64_unknown_linux_gnu(),
    ] {
        let mut ty = support::schema::ty();
        let string = ty.p(Primitive::String);
        let bytes = ty.p(Primitive::Bytes);
        let result = ty.variant(&[&[string], &[bytes]]);
        let owners = Owners::default();
        let mut builder = LayoutBuilder::new(&target, &owners);
        let slice = builder.layout(&ty.store, string).unwrap();
        let result = builder.layout(&ty.store, result).unwrap();
        let table = builder.finish();
        hirsel_lir::layout_validate::validate_table(&table, &target).unwrap();
        let payload = table
            .layouts
            .iter()
            .enumerate()
            .find(|(_, r)| {
                matches!(r.kind, LayoutKind::Sequence { element }
                if table.layout(element).unwrap().kind == LayoutKind::Scalar(ScalarKind::U8))
            })
            .unwrap()
            .0;
        let derived = hirsel_lir::rt_program::descriptor(
            &table,
            hirsel_lir::layout_types::LayoutId(payload as u32),
        )
        .unwrap();
        let runtime = &hirsel_rt_v2_bytes_descriptor.0;
        assert_eq!(
            (runtime.magic, runtime.version),
            (hirsel_rt::LAYOUT_MAGIC, hirsel_rt::LAYOUT_VERSION)
        );
        assert_eq!(
            (
                runtime.size,
                runtime.align,
                runtime.field_count,
                runtime.flags
            ),
            (
                derived.size,
                derived.align,
                derived.field_count,
                derived.flags
            )
        );
        assert!(runtime.finalizer.is_none() && derived.finalizer.is_none());
        assert!(runtime.steps.is_null() && derived.steps.is_empty());
        assert_eq!(runtime.step_count, 0);
        let slice = table.layout(slice).unwrap();
        assert_eq!(slice.size, SizeSpec::Fixed(size_of::<Slice>() as u64));
        assert_eq!(slice.align, align_of::<Slice>() as u32);
        let r = table.layout(result).unwrap();
        assert_eq!(r.size, SizeSpec::Fixed(size_of::<StringResult>() as u64));
        assert_eq!(r.align, align_of::<StringResult>() as u32);
        for variant in &r.tag.as_ref().unwrap().variants {
            assert_eq!(
                variant.fields[0].offset,
                offset_of!(StringResult, value) as u64
            );
            assert_eq!(
                table.layout(variant.fields[0].layout).unwrap().size,
                slice.size
            );
        }
    }
}

#[test]
fn bytes_list_descriptor_matches_derivation_on_both_targets() {
    use hirsel_layout::layout::{LayoutBuilder, Owners};
    use hirsel_lir::layout_types::{LayoutKind, TargetSpec};
    use hirsel_types::Primitive;
    for target in [
        TargetSpec::x86_64_unknown_linux_gnu(),
        TargetSpec::aarch64_unknown_linux_gnu(),
    ] {
        let mut ty = support::schema::ty();
        let bytes = ty.p(Primitive::Bytes);
        let list = ty.list(bytes);
        let owners = Owners::default();
        let mut builder = LayoutBuilder::new(&target, &owners);
        let slice = builder.layout(&ty.store, bytes).unwrap();
        builder.layout(&ty.store, list).unwrap();
        let table = builder.finish();
        hirsel_lir::layout_validate::validate_table(&table, &target).unwrap();
        let payload = table
            .layouts
            .iter()
            .position(|r| matches!(r.kind, LayoutKind::Sequence { element } if element == slice))
            .unwrap();
        let derived = hirsel_lir::rt_program::descriptor(
            &table,
            hirsel_lir::layout_types::LayoutId(payload as u32),
        )
        .unwrap();
        let runtime = &BYTES_LIST_DESCRIPTOR.0;
        assert_eq!(
            (
                runtime.magic,
                runtime.version,
                runtime.size,
                runtime.align,
                runtime.field_count,
                runtime.flags,
                runtime.logical
            ),
            (
                hirsel_rt::LAYOUT_MAGIC,
                hirsel_rt::LAYOUT_VERSION,
                derived.size,
                derived.align,
                derived.field_count,
                derived.flags,
                derived.logical
            )
        );
        assert!(runtime.finalizer.is_none() && derived.finalizer.is_none());
        let steps =
            unsafe { std::slice::from_raw_parts(runtime.steps, runtime.step_count as usize) };
        let steps: Vec<_> = steps
            .iter()
            .map(|s| {
                assert!(s.layout.is_null());
                (
                    s.kind,
                    s.field,
                    s.variant,
                    s.span,
                    s.offset,
                    s.stride,
                    s.count,
                    s.count_offset,
                )
            })
            .collect();
        let expected: Vec<_> = derived
            .steps
            .iter()
            .map(|s| {
                assert!(s.layout.is_none());
                (
                    s.kind.code(),
                    s.field,
                    s.variant,
                    s.span,
                    s.offset,
                    s.stride,
                    s.count,
                    s.count_offset,
                )
            })
            .collect();
        assert_eq!(steps, expected);
    }
}

#[test]
fn a_runtime_bytes_list_holds_its_items_and_releases_every_owner() {
    for ctx in [Ctx::production(), Ctx::diagnostic()] {
        unsafe {
            let items = [b"one".to_vec(), vec![], b"three\xff".to_vec()];
            let list = allocate_bytes_list(ctx.0, &items);
            assert_eq!((*list).index(), bytes_list_index(ctx.0));
            let payload = list.cast::<u8>().add(HEADER_SIZE);
            assert_eq!(payload.cast::<u64>().read(), 3);
            assert!(payload.add(8).cast::<u64>().read() >= 3);
            for (index, item) in items.iter().enumerate() {
                let slice = payload.add(16 + index * 24).cast::<Slice>().read();
                assert_eq!(shown(slice), item.as_slice());
                assert_eq!((*slice.owner).count, 1);
            }
            assert_eq!(ctx.stats().live_blocks, 4);
            release(&ctx, list);
        }
        assert_eq!(ctx.stats().live_blocks, 0);
    }
}

#[test]
fn from_bytes_transfers_the_same_nonzero_slice_on_both_utf8_outcomes() {
    for ctx in [Ctx::production(), Ctx::diagnostic()] {
        let valid = [b"prefix".as_slice(), "🦀".repeat(33).as_bytes(), b"TAIL"].concat();
        let invalid = [b"prefix".as_slice(), &vec![0x80; 132], b"TAIL"].concat();
        for (bytes, start, length, tag) in [
            (b"prefix\xc3\xa9TAIL".as_slice(), 6, 2, 0),
            (b"prefix\xffTAIL".as_slice(), 6, 1, 1),
            (b"prefixTAIL".as_slice(), 6, 0, 0),
            (valid.as_slice(), 6, 132, 0),
            (invalid.as_slice(), 6, 132, 1),
        ] {
            unsafe {
                let source = allocate_bytes(ctx.0, bytes);
                let allocations = ctx.stats().backing_allocations;
                let mut out = std::mem::MaybeUninit::uninit();
                assert_eq!(
                    hirsel_rt_v2_string_from_bytes(
                        ctx.0,
                        std::ptr::null_mut(),
                        out.as_mut_ptr(),
                        source.owner,
                        source.start + start,
                        length
                    ),
                    0
                );
                let out = out.assume_init();
                assert_eq!(
                    (out.tag, out.value.owner, out.value.start, out.value.length),
                    (tag, source.owner, source.start + start, length)
                );
                assert_eq!((*source.owner).count, 1);
                assert_eq!(ctx.stats().backing_allocations, allocations);
                assert_eq!(ctx.stats().live_blocks, 1);
                release(&ctx, out.value.owner);
            }
            assert_eq!(ctx.stats().live_blocks, 0);
        }
    }
}

#[test]
fn slices_share_once_and_failed_slices_leave_output_and_count_untouched() {
    for ctx in [Ctx::production(), Ctx::diagnostic()] {
        unsafe {
            let source = allocate_bytes(ctx.0, "prefixé😀xTAIL".as_bytes());
            for (from, to, expected) in [(0, 2, "é"), (2, 6, "😀"), (7, 7, "")] {
                let mut out = std::mem::MaybeUninit::uninit();
                assert_eq!(
                    hirsel_rt_v2_string_slice(
                        ctx.0,
                        std::ptr::null_mut(),
                        out.as_mut_ptr(),
                        source.owner,
                        source.start + 6,
                        7,
                        &from,
                        &to
                    ),
                    0
                );
                let out = out.assume_init();
                assert_eq!(shown(out), expected.as_bytes());
                assert_eq!(out.start, source.start + 6 + from);
                assert_eq!(out.owner, source.owner);
                assert_eq!((*source.owner).count, 2);
                release(&ctx, out.owner);
                assert_eq!((*source.owner).count, 1);
            }
            for (from, to) in [(6, 2), (1, 2), (0, 8)] {
                let mut out = Slice {
                    owner: std::ptr::null_mut(),
                    start: 123,
                    length: 456,
                };
                assert_eq!(
                    hirsel_rt_v2_string_slice(
                        ctx.0,
                        std::ptr::null_mut(),
                        &mut out,
                        source.owner,
                        source.start + 6,
                        7,
                        &from,
                        &to
                    ),
                    1
                );
                assert!(out.owner.is_null());
                assert_eq!(
                    (out.start, out.length, (*source.owner).count),
                    (123, 456, 1)
                );
            }
            release(&ctx, source.owner);
        }
        assert_eq!(ctx.stats().live_blocks, 0);
    }
}

#[test]
fn byte_bridges_preserve_nested_intervals_and_publish_only_after_checks() {
    for ctx in [Ctx::production(), Ctx::diagnostic()] {
        unsafe {
            let source = allocate_bytes(ctx.0, "prefixé😀xTAIL".as_bytes());
            hirsel_rt::count::hirsel_rt_v2_dup(ctx.0, source.owner);
            let view = Slice {
                owner: source.owner,
                start: source.start + 6,
                length: 7,
            };
            let mut slice = std::mem::MaybeUninit::uninit();
            assert_eq!(
                hirsel_rt_v2_bytes_slice(
                    ctx.0,
                    std::ptr::null_mut(),
                    slice.as_mut_ptr(),
                    view.owner,
                    view.start,
                    view.length,
                    2,
                    6
                ),
                0
            );
            let slice = slice.assume_init();
            assert_eq!(
                (slice.owner, slice.start, slice.length),
                (source.owner, source.start + 8, 4)
            );
            assert_eq!(shown(slice), "😀".as_bytes());
            for (from, to) in [(7, 2), (0, 8), (u64::MAX, u64::MAX)] {
                let mut out = Slice {
                    owner: std::ptr::null_mut(),
                    start: 123,
                    length: 456,
                };
                assert_eq!(
                    hirsel_rt_v2_bytes_slice(
                        ctx.0,
                        std::ptr::null_mut(),
                        &mut out,
                        view.owner,
                        view.start,
                        view.length,
                        from,
                        to
                    ),
                    1
                );
                assert!(out.owner.is_null());
                assert_eq!(
                    (out.start, out.length, (*source.owner).count),
                    (123, 456, 3)
                );
            }
            // The retained child credit keeps exactly the same owner live.
            release(&ctx, source.owner);
            release(&ctx, view.owner);
            assert_eq!(shown(slice), "😀".as_bytes());
            assert_eq!((*slice.owner).count, 1);
            release(&ctx, slice.owner);
        }
    }
}

#[test]
fn position_checks_only_the_selected_utf8_interval_and_none_is_initialized() {
    for ctx in [Ctx::production(), Ctx::diagnostic()] {
        unsafe {
            let source = allocate_bytes(ctx.0, "prefixé😀xTAIL".as_bytes());
            for offset in [0, 2, 6, 7, 1, 3, 8, u64::MAX] {
                let mut out = PositionOption {
                    tag: u32::MAX,
                    padding: u32::MAX,
                    offset: u64::MAX,
                };
                assert_eq!(
                    hirsel_rt_v2_string_position(
                        ctx.0,
                        std::ptr::null_mut(),
                        &mut out,
                        source.owner,
                        source.start + 6,
                        7,
                        offset
                    ),
                    0
                );
                let valid = [0, 2, 6, 7].contains(&offset);
                assert_eq!(out.tag, u32::from(valid));
                assert_eq!(out.padding, 0);
                assert_eq!(out.offset, if valid { offset } else { 0 });
                if valid {
                    let mut back = u64::MAX;
                    assert_eq!(
                        hirsel_rt_v2_string_byte_offset(
                            ctx.0,
                            &mut back,
                            source.owner,
                            source.start + 6,
                            7,
                            &out.offset
                        ),
                        0
                    );
                    assert_eq!(back, offset);
                }
                assert_eq!((*source.owner).count, 1);
            }
            release(&ctx, source.owner);
        }
    }
}

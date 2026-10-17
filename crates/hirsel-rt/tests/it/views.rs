//! Unicode cursor ABI records, slice provenance and owner accounting.
use crate::support;
use hirsel_rt::views::*;
use hirsel_rt::{DOMAIN_HEAP, HEADER_SIZE, Header};
use support::Ctx;
use support::schema::Ty;

fn block(ctx: &Ctx, text: &str) -> *mut Header {
    unsafe {
        let owner = hirsel_rt::alloc::hirsel_rt_v2_alloc(
            ctx.0,
            0,
            8 + text.len() as u64,
            8,
            DOMAIN_HEAP,
            0,
        );
        let data = owner.cast::<u8>().add(HEADER_SIZE);
        data.cast::<u64>().write(text.len() as u64);
        std::ptr::copy_nonoverlapping(text.as_ptr(), data.add(8), text.len());
        owner
    }
}
fn release(ctx: &Ctx, owner: *mut Header) {
    unsafe {
        assert_eq!(
            hirsel_rt::release::hirsel_rt_v2_release(
                ctx.0,
                owner,
                (*owner).index(),
                std::ptr::null(),
                0
            ),
            0
        )
    }
}

#[test]
fn cursor_and_option_records_match_derived_layouts_on_both_targets() {
    use hirsel_layout::layout::{LayoutBuilder, OwnerRepresentation, Owners};

    use hirsel_lir::layout_types::{LayoutKind, ScanStep, SizeSpec, TargetSpec};
    use hirsel_types::{GenericArgument, Primitive, TypeKind};
    use std::mem::{align_of, offset_of, size_of};
    for target in [
        TargetSpec::x86_64_unknown_linux_gnu(),
        TargetSpec::aarch64_unknown_linux_gnu(),
    ] {
        let path = support::schema::path("ViewCursor");
        let mut ty = Ty::default();
        let word = ty.p(Primitive::U64);
        let scalar = ty.p(Primitive::U32);
        let string = ty.p(Primitive::String);
        let cursor = ty.add(TypeKind::Named {
            definition: path.clone(),
            arguments: vec![GenericArgument::Type(string)].into(),
        });
        // StrPos is the library owner's one-field plain u64 representation.
        let position = ty.record(&[word]);
        let variants = [
            ty.variant(&[&[], &[scalar]]),
            ty.variant(&[&[], &[position]]),
        ];
        let mut owners = Owners::default();
        owners.nominals.insert(
            path,
            OwnerRepresentation::ViewCursor {
                revision: Default::default(),
            },
        );
        let view_names = ["ScalarView", "PositionView"];
        let view_types: Vec<_> = view_names
            .iter()
            .map(|name| {
                let path = support::schema::path(name);
                owners
                    .nominals
                    .insert(path.clone(), OwnerRepresentation::StringView);
                ty.add(TypeKind::Named {
                    definition: path,
                    arguments: vec![].into(),
                })
            })
            .collect();
        let mut b = LayoutBuilder::new(&target, &owners);
        let string_layout = b.layout(&ty.store, string).unwrap();
        for view in view_types {
            assert_eq!(b.layout(&ty.store, view).unwrap(), string_layout);
        }
        let cursor = b.layout(&ty.store, cursor).unwrap();
        let ids: Vec<_> = variants
            .iter()
            .map(|&t| b.layout(&ty.store, t).unwrap())
            .collect();
        let table = b.finish();
        hirsel_lir::layout_validate::validate_table(&table, &target).unwrap();
        let r = table.layout(cursor).unwrap();
        assert_eq!(r.size, SizeSpec::Fixed(size_of::<ViewCursor>() as u64));
        assert_eq!(r.align, align_of::<ViewCursor>() as u32);
        assert_eq!(r.fields[1].offset, offset_of!(ViewCursor, position) as u64);
        assert_eq!(r.scan, vec![ScanStep::Uncounted { offset: 0 }]);
        assert!(r.drop.is_empty());
        assert!(matches!(
            table.layout(r.fields[0].layout).unwrap().kind,
            LayoutKind::Slice { .. }
        ));
        for (id, size, align, offset) in [
            (
                ids[0],
                size_of::<ScalarOption>(),
                align_of::<ScalarOption>(),
                offset_of!(ScalarOption, value),
            ),
            (
                ids[1],
                size_of::<PositionOption>(),
                align_of::<PositionOption>(),
                offset_of!(PositionOption, value),
            ),
        ] {
            let r = table.layout(id).unwrap();
            assert_eq!(r.size, SizeSpec::Fixed(size as u64));
            assert_eq!(r.align, align as u32);
            assert_eq!(
                r.tag.as_ref().unwrap().variants[1].fields[0].offset,
                offset as u64
            );
        }
    }
}

#[test]
fn sliced_scalars_and_positions_preserve_relative_offsets_and_empty_terminal() {
    let ctx = Ctx::diagnostic();
    let owner = block(&ctx, "prefixé😀xTAIL");
    let mut cursor = ViewCursor {
        view: Slice {
            owner,
            start: 99,
            length: 99,
        },
        position: 99,
    };
    unsafe {
        assert_eq!(
            hirsel_rt_v2_unicode_scalars_open(
                ctx.0,
                std::ptr::null_mut(),
                &mut cursor,
                owner,
                6,
                7
            ),
            0
        );
        let mut out = ScalarOption::default();
        for value in [0xe9, 0x1f600, 0x78] {
            assert_eq!(
                hirsel_rt_v2_unicode_scalars_next(
                    ctx.0,
                    std::ptr::null_mut(),
                    &mut out,
                    &mut cursor
                ),
                0
            );
            assert_eq!((out.tag, out.value), (1, value));
        }
        for _ in 0..2 {
            hirsel_rt_v2_unicode_scalars_next(ctx.0, std::ptr::null_mut(), &mut out, &mut cursor);
            assert_eq!(out.tag, 0);
        }
        let mut out = PositionOption::default();
        for length in [7, 0] {
            hirsel_rt_v2_scalar_positions_open(
                ctx.0,
                std::ptr::null_mut(),
                &mut cursor,
                owner,
                6,
                length,
            );
            let expected: &[u64] = if length == 0 { &[0] } else { &[0, 2, 6, 7] };
            for &value in expected {
                hirsel_rt_v2_scalar_positions_next(
                    ctx.0,
                    std::ptr::null_mut(),
                    &mut out,
                    &mut cursor,
                );
                assert_eq!((out.tag, out.value), (1, value));
            }
            for _ in 0..2 {
                hirsel_rt_v2_scalar_positions_next(
                    ctx.0,
                    std::ptr::null_mut(),
                    &mut out,
                    &mut cursor,
                );
                assert_eq!(out.tag, 0);
            }
        }
        assert_eq!((*owner).count, 1);
    }
    release(&ctx, owner);
    assert_eq!(ctx.stats().live_blocks, 0);
}

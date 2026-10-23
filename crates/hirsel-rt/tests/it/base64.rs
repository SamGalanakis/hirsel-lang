//! The Base64 ABI borrows exactly an extent and returns a fresh, exact extent.
use crate::support::Ctx;
use base64::Engine;
use hirsel_rt::text::{
    allocate_bytes, hirsel_rt_v2_bytes_base64_decode, hirsel_rt_v2_bytes_base64_encode,
};
use hirsel_rt::views::Slice;
use std::ptr;

unsafe fn shown<'a>(value: Slice) -> &'a [u8] {
    unsafe {
        std::slice::from_raw_parts(
            value
                .owner
                .cast::<u8>()
                .add(hirsel_rt::HEADER_SIZE + 8 + value.start as usize),
            value.length as usize,
        )
    }
}

unsafe fn release(ctx: &Ctx, value: Slice) {
    assert_eq!(
        unsafe {
            hirsel_rt::release::hirsel_rt_v2_release(
                ctx.0,
                value.owner,
                (*value.owner).index(),
                ptr::null(),
                0,
                0,
            )
        },
        0
    );
}

#[test]
fn borrowed_subextents_empty_success_and_rejection_preserve_owners() {
    for ctx in [Ctx::production(), Ctx::diagnostic()] {
        for (url, padded, scalar) in [
            (0, 1, base64::engine::general_purpose::STANDARD),
            (0, 0, base64::engine::general_purpose::STANDARD_NO_PAD),
            (1, 1, base64::engine::general_purpose::URL_SAFE),
            (1, 0, base64::engine::general_purpose::URL_SAFE_NO_PAD),
        ] {
            for len in [
                0, 1, 2, 3, 23, 24, 25, 31, 32, 33, 63, 64, 65, 127, 128, 129, 257,
            ] {
                let raw: Vec<u8> = (0..len).map(|i| (i * 137) as u8).collect();
                let expected = scalar.encode(&raw);
                unsafe {
                    let backing = [b"prefix".as_slice(), &raw, b"suffix"].concat();
                    let source = allocate_bytes(ctx.0, &backing);
                    let mut out = std::mem::MaybeUninit::uninit();
                    assert_eq!(
                        hirsel_rt_v2_bytes_base64_encode(
                            ctx.0,
                            ptr::null_mut(),
                            out.as_mut_ptr(),
                            source.owner,
                            source.start + 6,
                            raw.len() as u64,
                            &url,
                            &(1 - padded),
                        ),
                        0
                    );
                    let encoded = out.assume_init();
                    assert_ne!(encoded.owner, source.owner);
                    assert!(!encoded.owner.is_null());
                    assert_eq!(shown(encoded), expected.as_bytes());
                    assert_eq!((*source.owner).count, 1);
                    assert_eq!(shown(source), backing);
                    release(&ctx, encoded);
                    release(&ctx, source);

                    let backing = [b"!!!".as_slice(), expected.as_bytes(), b"%%%"].concat();
                    let source = allocate_bytes(ctx.0, &backing);
                    let mut out = std::mem::MaybeUninit::uninit();
                    assert_eq!(
                        hirsel_rt_v2_bytes_base64_decode(
                            ctx.0,
                            ptr::null_mut(),
                            out.as_mut_ptr(),
                            source.owner,
                            source.start + 3,
                            expected.len() as u64,
                            &url,
                            &(1 - padded),
                        ),
                        0
                    );
                    let decoded = out.assume_init();
                    assert!(
                        !decoded.owner.is_null(),
                        "Some(empty) must differ from None"
                    );
                    assert_ne!(decoded.owner, source.owner);
                    assert_eq!(shown(decoded), raw);
                    assert_eq!((*source.owner).count, 1);
                    assert_eq!(shown(source), backing);
                    release(&ctx, decoded);
                    release(&ctx, source);
                }
                assert_eq!(ctx.stats().live_blocks, 0);
            }
            for bad in [
                b"!".as_slice(),
                b"AAAA!AAA",
                b"AB==",
                b"AAB=",
                b"AAAA\n",
                b"=",
                b"A===",
                b"AA===",
            ] {
                unsafe {
                    let source = allocate_bytes(ctx.0, bad);
                    let mut out = std::mem::MaybeUninit::uninit();
                    assert_eq!(
                        hirsel_rt_v2_bytes_base64_decode(
                            ctx.0,
                            ptr::null_mut(),
                            out.as_mut_ptr(),
                            source.owner,
                            source.start,
                            source.length,
                            &url,
                            &(1 - padded),
                        ),
                        0
                    );
                    let rejected = out.assume_init();
                    assert!(rejected.owner.is_null());
                    assert_eq!((rejected.start, rejected.length), (0, 0));
                    assert_eq!((*source.owner).count, 1);
                    assert_eq!(shown(source), bad);
                    release(&ctx, source);
                }
                assert_eq!(ctx.stats().live_blocks, 0);
            }
        }
    }
}

#[test]
fn optional_bytes_and_both_exports_match_derived_layouts() {
    use hirsel_layout::layout::{LayoutBuilder, Owners};
    use hirsel_lir::layout_types::{SizeSpec, TagForm, TargetSpec};
    use hirsel_types::Primitive;
    for target in [
        TargetSpec::x86_64_unknown_linux_gnu(),
        TargetSpec::aarch64_unknown_linux_gnu(),
    ] {
        let mut ty = crate::support::schema::ty();
        let bytes = ty.p(Primitive::Bytes);
        let optional = ty.variant(&[&[], &[bytes]]);
        let owners = Owners::default();
        let mut builder = LayoutBuilder::new(&target, &owners);
        let ids = [bytes, optional].map(|tyid| builder.layout(&ty.store, tyid).unwrap());
        let table = builder.finish();
        hirsel_lir::layout_validate::validate_table(&table, &target).unwrap();
        for id in ids {
            let record = table.layout(id).unwrap();
            assert_eq!(record.size, SizeSpec::Fixed(24));
            assert_eq!(record.align, 8);
        }
        let tag = table.layout(ids[1]).unwrap().tag.as_ref().unwrap();
        assert_eq!(tag.offset, 0);
        assert_eq!(
            tag.form,
            TagForm::Niche {
                nullary: 0,
                dataful: 1
            }
        );
    }
    for symbol in [
        "hirsel_rt_v2_bytes_base64_encode",
        "hirsel_rt_v2_bytes_base64_decode",
    ] {
        let export = hirsel_rt_operations::HIRSEL_ABI_EXPORTS
            .iter()
            .find(|e| e.symbol == symbol)
            .unwrap();
        assert_eq!(export.result, Some((24, 8)));
    }
}

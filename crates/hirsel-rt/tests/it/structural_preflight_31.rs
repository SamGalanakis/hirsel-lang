//! Finite boundary controls for empty distinct payloads and transparent views.
unsafe extern "C" {
    fn hirsel_rt_v2_depth_preflight(
        ctx: *mut std::ffi::c_void,
        program: *const std::ffi::c_void,
        program_length: u64,
        edges: *const std::ffi::c_void,
        edge_count: u64,
        value: *const std::ffi::c_void,
    ) -> u8;
}

fn admits(distinct: usize, transparent: usize) -> bool {
    let count = distinct + transparent + 1;
    let mut words = vec![
        hirsel_lir::codec::PROGRAM_VERSION,
        count as u64,
        0,
        0,
        0,
        0,
        u64::MAX,
    ];
    words.resize(7 + count, 0);
    let mut edges = vec![0; count];
    for i in 0..count {
        words[7 + i] = words.len() as u64;
        if i + 1 == count {
            words.extend([hirsel_lir::codec::NODE_FIELDS, 0]);
        } else {
            words.extend([hirsel_lir::codec::NODE_CHECKED, 0, (i + 1) as u64]);
            edges[i] = u8::from(i < distinct);
        }
    }
    // SAFETY: the final unit occupies no bytes; every wrapper projects the
    // same address, and both static buffers remain borrowed through the call.
    unsafe {
        hirsel_rt_v2_depth_preflight(
            std::ptr::null_mut(),
            words.as_ptr().cast(),
            words.len() as u64,
            edges.as_ptr().cast(),
            edges.len() as u64,
            std::ptr::null(),
        ) == 0
    }
}

#[test]
fn empty_payload_and_transparent_physical_nodes_keep_the_logical_boundary() {
    assert_eq!(
        hirsel_rt::hirsel_rt_v2_abi_version(),
        hirsel_rt::ABI_VERSION
    );
    assert_eq!(
        hirsel_rt::hirsel_rt_v2_abi_version(),
        hirsel_rt::ABI_VERSION
    );
    assert!(admits(1087, 0));
    assert!(admits(1088, 0));
    assert!(!admits(1089, 0));
    assert!(admits(1088, 64));
    assert!(!admits(1089, 64));
}

unsafe extern "C" {
    fn hirsel_rt_v2_structural_order(
        ctx: *mut std::ffi::c_void,
        program: *const std::ffi::c_void,
        program_length: u64,
        left: *const std::ffi::c_void,
        right: *const std::ffi::c_void,
    ) -> u8;
    fn hirsel_rt_v2_structural_hash(
        ctx: *mut std::ffi::c_void,
        program: *const std::ffi::c_void,
        program_length: u64,
        value: *const std::ffi::c_void,
        seed: u64,
    ) -> u64;
}

#[test]
fn value_program_preserves_signed_order_and_ieee_unordered() {
    let _ = hirsel_rt::hirsel_rt_v2_abi_version();
    let program = |code| {
        vec![
            hirsel_lir::codec::PROGRAM_VERSION,
            1,
            0,
            0,
            0,
            0,
            u64::MAX,
            8,
            hirsel_lir::codec::NODE_SCALAR,
            code,
        ]
    };
    // Each signed width must order -1 below +1 even though its raw bits are larger.
    for code in 6..=10 {
        let words = program(code);
        let (negative, positive) = (u128::MAX, 1u128);
        // SAFETY: live scalar images match the program's width, no context reads.
        unsafe {
            let order = hirsel_rt_v2_structural_order(
                std::ptr::null_mut(),
                words.as_ptr().cast(),
                words.len() as u64,
                (&negative as *const u128).cast(),
                (&positive as *const u128).cast(),
            );
            assert_eq!(order, 1, "signed scalar code {code}");
        }
    }
    let words = program(12);
    let (negative_zero, positive_zero, nan) = (-0.0f64, 0.0f64, f64::NAN);
    // SAFETY: both operands are live f64 values matching the scalar program.
    unsafe {
        let a = (&negative_zero as *const f64).cast();
        let b = (&positive_zero as *const f64).cast();
        let n = (&nan as *const f64).cast();
        let p = words.as_ptr().cast();
        let len = words.len() as u64;
        assert_eq!(
            hirsel_rt_v2_structural_order(std::ptr::null_mut(), p, len, a, b),
            0
        );
        assert_eq!(
            hirsel_rt_v2_structural_order(std::ptr::null_mut(), p, len, n, n),
            3
        );
        assert_eq!(
            hirsel_rt_v2_structural_hash(std::ptr::null_mut(), p, len, a, 7),
            hirsel_rt_v2_structural_hash(std::ptr::null_mut(), p, len, b, 7)
        );
    }
}

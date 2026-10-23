//! Complete-buffer qualification of simdutf8 0.1.5's basic interface.
//! Upstream tests/tests.rs:35-51,113-157 tests safe dispatch and feature-gated
//! implementations separately. Keep Rust std as the independent validity oracle.

type Validator = unsafe fn(&[u8]) -> Result<(), simdutf8::basic::Utf8Error>;

fn implementations() -> Vec<(&'static str, Validator)> {
    let mut out: Vec<(&str, Validator)> = Vec::new();
    #[cfg(target_arch = "x86_64")]
    {
        if std::is_x86_feature_detected!("sse4.2") {
            out.push(("sse4.2", simdutf8::basic::imp::x86::sse42::validate_utf8));
        }
        if std::is_x86_feature_detected!("avx2") {
            out.push(("avx2", simdutf8::basic::imp::x86::avx2::validate_utf8));
        }
    }
    #[cfg(all(target_arch = "aarch64", target_feature = "neon"))]
    out.push(("neon", simdutf8::basic::imp::aarch64::neon::validate_utf8));
    out
}

fn check(input: &[u8]) {
    let expected = std::str::from_utf8(input).is_ok();
    assert_eq!(
        simdutf8::basic::from_utf8(input).is_ok(),
        expected,
        "{input:x?}"
    );
    for (name, validate) in implementations() {
        // SAFETY: implementations() admits only ISA features present on this CPU.
        assert_eq!(
            unsafe { validate(input) }.is_ok(),
            expected,
            "{name}: {input:x?}"
        );
    }
}

#[test]
fn complete_utf8_vectors_and_block_boundaries() {
    // RFC 3629 edge classes also exercised by upstream tests/tests.rs. Prefixes
    // and suffixes make the complete-buffer EOF and previous-block state visible.
    let vectors: &[&[u8]] = &[
        b"",
        b"a",
        b"\0",
        b"\x7f",
        b"\xc2\x80",
        b"\xdf\xbf",
        b"\xe0\xa0\x80",
        b"\xed\x9f\xbf",
        b"\xef\xbf\xbf",
        b"\xf0\x90\x80\x80",
        b"\xf4\x8f\xbf\xbf",
        b"\xc0\x80",
        b"\xc1\xbf",
        b"\xe0\x9f\xbf",
        b"\xf0\x8f\xbf\xbf",
        b"\xed\xa0\x80",
        b"\xed\xbf\xbf",
        b"\xf4\x90\x80\x80",
        b"\xf5\x80\x80\x80",
        b"\x80",
        b"\xbf",
        b"\xff",
        b"\xc2A",
        b"\xe1\x80A",
        b"\xf1\x80\x80A",
    ];
    let mut count = 0;
    for vector in vectors {
        for end in 0..=vector.len() {
            for block in [16usize, 32, 64, 128] {
                for offset in block - 4..=block + 4 {
                    let mut input = vec![b'a'; offset];
                    input.extend_from_slice(&vector[..end]);
                    check(&input);
                    input.extend_from_slice(b"Z");
                    check(&input);
                    count += 2;
                }
            }
        }
    }
    eprintln!(
        "utf8 vectors: {count} extents; forced implementations: {:?}",
        implementations()
            .iter()
            .map(|(name, _)| *name)
            .collect::<Vec<_>>()
    );
}

#[test]
fn complete_utf8_seeded_differential() {
    let mut count = 0;
    for a in 0..=255u8 {
        check(&[a]);
        for b in 0..=255u8 {
            check(&[a, b]);
            count += 1;
        }
    }
    let mut seed = 2026102329u64;
    for n in 0..4096 {
        let len = n % 257;
        let mut input = Vec::with_capacity(len);
        for _ in 0..len {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            input.push(seed as u8);
        }
        check(&input);
        count += 1;
        // A valid multibyte prefix plus a randomly corrupted interior or EOF.
        let mut input = "aé€🦀".repeat(n % 32).into_bytes();
        check(&input);
        if !input.is_empty() {
            let index = seed as usize % input.len();
            input[index] = (seed >> 8) as u8;
            check(&input);
        }
    }
    eprintln!(
        "utf8 seeded differential: seed=2026102329, {count} raw cases plus valid/mutated cases"
    );
}

#[cfg(unix)]
#[test]
fn complete_utf8_guard_pages() {
    struct Pages(*mut libc::c_void, usize);
    impl Drop for Pages {
        fn drop(&mut self) {
            // SAFETY: this guard owns the entire successful mmap.
            assert_eq!(unsafe { libc::munmap(self.0, self.1) }, 0);
        }
    }
    // SAFETY: sysconf has no pointer arguments.
    let page = usize::try_from(unsafe { libc::sysconf(libc::_SC_PAGESIZE) }).unwrap();
    // SAFETY: anonymous private mapping, checked before any access.
    let base = unsafe {
        libc::mmap(
            std::ptr::null_mut(),
            page * 3,
            libc::PROT_NONE,
            libc::MAP_PRIVATE | libc::MAP_ANONYMOUS,
            -1,
            0,
        )
    };
    assert_ne!(base, libc::MAP_FAILED);
    let _pages = Pages(base, page * 3);
    // SAFETY: middle page lies inside our mapping; outer pages stay unreadable.
    let middle = unsafe { base.cast::<u8>().add(page) };
    assert_eq!(
        unsafe { libc::mprotect(middle.cast(), page, libc::PROT_READ | libc::PROT_WRITE) },
        0
    );
    let mut count = 0;
    for len in 0..=260 {
        for start in [middle, unsafe { middle.add(page - len) }] {
            // SAFETY: exactly len initialized bytes in our writable page (or
            // an empty extent at the unreadable right guard when len == 0).
            let input = unsafe { std::slice::from_raw_parts_mut(start, len) };
            input.fill(b'a');
            check(input);
            if let Some(last) = input.last_mut() {
                *last = 0xf0;
            }
            check(input);
            if len >= 4 {
                input[len - 4..].copy_from_slice("🦀".as_bytes());
                check(input);
                input[0] = 0x80;
                check(input);
            }
            count += 1;
        }
    }
    eprintln!(
        "utf8 guard pages: {count} extents, safe dispatch and all supported forced SIMD; empty right guard reads nothing"
    );
}

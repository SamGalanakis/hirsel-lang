#[test]
fn debug_assertions_are_enabled() {
    assert!(std::panic::catch_unwind(|| debug_assert!(std::hint::black_box(false))).is_err());
}

#[test]
fn integer_overflow_is_checked() {
    assert!(
        std::panic::catch_unwind(|| { std::hint::black_box(u8::MAX) + std::hint::black_box(1) })
            .is_err()
    );
}

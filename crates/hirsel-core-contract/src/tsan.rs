//! Diagnostic x86-64 SysV callbacks whose arguments fit in integer registers.
//! compiler-rt's tsan_interface.h and tsan_interface_atomic.h define these ABIs.
//! The runtime bridge moves callbacks onto the suspended native idle stack.

/// Includes the incoming return address and one saved register. The callback
/// itself executes on idle and contributes no bytes to the private stack.
pub const PRIVATE_STACK_BYTES: u64 = 16;

/// The callback inventory and bridge ABI are part of diagnostic helper identity.
pub const CONTRACT: &[u8] = include_bytes!("tsan.rs");

/// One inventory for emission, independent certification and runtime thunks.
#[macro_export]
macro_rules! tsan_callbacks {
    ($visitor:ident) => {
        $visitor! {
            (hirsel_tsan_func_entry, __tsan_func_entry, __tsan_func_entry, ""),
            (hirsel_tsan_func_exit, __tsan_func_exit, __tsan_func_exit, ""),
            (hirsel_tsan_read_range, __tsan_read_range, __tsan_read_range_pc, "mov rdx, [rsp]"),
            (hirsel_tsan_write_range, __tsan_write_range, __tsan_write_range_pc, "mov rdx, [rsp]"),
            (hirsel_tsan_memcpy, __tsan_memcpy, __tsan_memcpy, ""),
            (hirsel_tsan_memmove, __tsan_memmove, __tsan_memmove, ""),
            (hirsel_tsan_memset, __tsan_memset, __tsan_memset, ""),
            (hirsel_tsan_atomic_thread_fence, __tsan_atomic_thread_fence, __tsan_atomic_thread_fence, ""),
            (hirsel_tsan_atomic_signal_fence, __tsan_atomic_signal_fence, __tsan_atomic_signal_fence, ""),
            (hirsel_tsan_read1, __tsan_read1, __tsan_read1_pc, "mov rsi, [rsp]"),
            (hirsel_tsan_write1, __tsan_write1, __tsan_write1_pc, "mov rsi, [rsp]"),
            (hirsel_tsan_read_write1, __tsan_read_write1, __tsan_read_write1, ""),
            (hirsel_tsan_unaligned_read1, __tsan_unaligned_read1, __tsan_read_range_pc, "mov esi, 1\nmov rdx, [rsp]"),
            (hirsel_tsan_unaligned_write1, __tsan_unaligned_write1, __tsan_write_range_pc, "mov esi, 1\nmov rdx, [rsp]"),
            (hirsel_tsan_unaligned_read_write1, __tsan_unaligned_read_write1, __tsan_unaligned_read_write1, ""),
            (hirsel_tsan_read2, __tsan_read2, __tsan_read2_pc, "mov rsi, [rsp]"),
            (hirsel_tsan_write2, __tsan_write2, __tsan_write2_pc, "mov rsi, [rsp]"),
            (hirsel_tsan_read_write2, __tsan_read_write2, __tsan_read_write2, ""),
            (hirsel_tsan_unaligned_read2, __tsan_unaligned_read2, __tsan_read_range_pc, "mov esi, 2\nmov rdx, [rsp]"),
            (hirsel_tsan_unaligned_write2, __tsan_unaligned_write2, __tsan_write_range_pc, "mov esi, 2\nmov rdx, [rsp]"),
            (hirsel_tsan_unaligned_read_write2, __tsan_unaligned_read_write2, __tsan_unaligned_read_write2, ""),
            (hirsel_tsan_read4, __tsan_read4, __tsan_read4_pc, "mov rsi, [rsp]"),
            (hirsel_tsan_write4, __tsan_write4, __tsan_write4_pc, "mov rsi, [rsp]"),
            (hirsel_tsan_read_write4, __tsan_read_write4, __tsan_read_write4, ""),
            (hirsel_tsan_unaligned_read4, __tsan_unaligned_read4, __tsan_read_range_pc, "mov esi, 4\nmov rdx, [rsp]"),
            (hirsel_tsan_unaligned_write4, __tsan_unaligned_write4, __tsan_write_range_pc, "mov esi, 4\nmov rdx, [rsp]"),
            (hirsel_tsan_unaligned_read_write4, __tsan_unaligned_read_write4, __tsan_unaligned_read_write4, ""),
            (hirsel_tsan_read8, __tsan_read8, __tsan_read8_pc, "mov rsi, [rsp]"),
            (hirsel_tsan_write8, __tsan_write8, __tsan_write8_pc, "mov rsi, [rsp]"),
            (hirsel_tsan_read_write8, __tsan_read_write8, __tsan_read_write8, ""),
            (hirsel_tsan_unaligned_read8, __tsan_unaligned_read8, __tsan_read_range_pc, "mov esi, 8\nmov rdx, [rsp]"),
            (hirsel_tsan_unaligned_write8, __tsan_unaligned_write8, __tsan_write_range_pc, "mov esi, 8\nmov rdx, [rsp]"),
            (hirsel_tsan_unaligned_read_write8, __tsan_unaligned_read_write8, __tsan_unaligned_read_write8, ""),
            (hirsel_tsan_read16, __tsan_read16, __tsan_read16_pc, "mov rsi, [rsp]"),
            (hirsel_tsan_write16, __tsan_write16, __tsan_write16_pc, "mov rsi, [rsp]"),
            (hirsel_tsan_read_write16, __tsan_read_write16, __tsan_read_write16, ""),
            (hirsel_tsan_unaligned_read16, __tsan_unaligned_read16, __tsan_read_range_pc, "mov esi, 16\nmov rdx, [rsp]"),
            (hirsel_tsan_unaligned_write16, __tsan_unaligned_write16, __tsan_write_range_pc, "mov esi, 16\nmov rdx, [rsp]"),
            (hirsel_tsan_unaligned_read_write16, __tsan_unaligned_read_write16, __tsan_unaligned_read_write16, ""),
            (hirsel_tsan_atomic8_load, __tsan_atomic8_load, __tsan_atomic8_load, ""),
            (hirsel_tsan_atomic8_store, __tsan_atomic8_store, __tsan_atomic8_store, ""),
            (hirsel_tsan_atomic8_exchange, __tsan_atomic8_exchange, __tsan_atomic8_exchange, ""),
            (hirsel_tsan_atomic8_fetch_add, __tsan_atomic8_fetch_add, __tsan_atomic8_fetch_add, ""),
            (hirsel_tsan_atomic8_fetch_sub, __tsan_atomic8_fetch_sub, __tsan_atomic8_fetch_sub, ""),
            (hirsel_tsan_atomic8_fetch_and, __tsan_atomic8_fetch_and, __tsan_atomic8_fetch_and, ""),
            (hirsel_tsan_atomic8_fetch_or, __tsan_atomic8_fetch_or, __tsan_atomic8_fetch_or, ""),
            (hirsel_tsan_atomic8_fetch_xor, __tsan_atomic8_fetch_xor, __tsan_atomic8_fetch_xor, ""),
            (hirsel_tsan_atomic8_fetch_nand, __tsan_atomic8_fetch_nand, __tsan_atomic8_fetch_nand, ""),
            (hirsel_tsan_atomic8_compare_exchange_val, __tsan_atomic8_compare_exchange_val, __tsan_atomic8_compare_exchange_val, ""),
            (hirsel_tsan_atomic16_load, __tsan_atomic16_load, __tsan_atomic16_load, ""),
            (hirsel_tsan_atomic16_store, __tsan_atomic16_store, __tsan_atomic16_store, ""),
            (hirsel_tsan_atomic16_exchange, __tsan_atomic16_exchange, __tsan_atomic16_exchange, ""),
            (hirsel_tsan_atomic16_fetch_add, __tsan_atomic16_fetch_add, __tsan_atomic16_fetch_add, ""),
            (hirsel_tsan_atomic16_fetch_sub, __tsan_atomic16_fetch_sub, __tsan_atomic16_fetch_sub, ""),
            (hirsel_tsan_atomic16_fetch_and, __tsan_atomic16_fetch_and, __tsan_atomic16_fetch_and, ""),
            (hirsel_tsan_atomic16_fetch_or, __tsan_atomic16_fetch_or, __tsan_atomic16_fetch_or, ""),
            (hirsel_tsan_atomic16_fetch_xor, __tsan_atomic16_fetch_xor, __tsan_atomic16_fetch_xor, ""),
            (hirsel_tsan_atomic16_fetch_nand, __tsan_atomic16_fetch_nand, __tsan_atomic16_fetch_nand, ""),
            (hirsel_tsan_atomic16_compare_exchange_val, __tsan_atomic16_compare_exchange_val, __tsan_atomic16_compare_exchange_val, ""),
            (hirsel_tsan_atomic32_load, __tsan_atomic32_load, __tsan_atomic32_load, ""),
            (hirsel_tsan_atomic32_store, __tsan_atomic32_store, __tsan_atomic32_store, ""),
            (hirsel_tsan_atomic32_exchange, __tsan_atomic32_exchange, __tsan_atomic32_exchange, ""),
            (hirsel_tsan_atomic32_fetch_add, __tsan_atomic32_fetch_add, __tsan_atomic32_fetch_add, ""),
            (hirsel_tsan_atomic32_fetch_sub, __tsan_atomic32_fetch_sub, __tsan_atomic32_fetch_sub, ""),
            (hirsel_tsan_atomic32_fetch_and, __tsan_atomic32_fetch_and, __tsan_atomic32_fetch_and, ""),
            (hirsel_tsan_atomic32_fetch_or, __tsan_atomic32_fetch_or, __tsan_atomic32_fetch_or, ""),
            (hirsel_tsan_atomic32_fetch_xor, __tsan_atomic32_fetch_xor, __tsan_atomic32_fetch_xor, ""),
            (hirsel_tsan_atomic32_fetch_nand, __tsan_atomic32_fetch_nand, __tsan_atomic32_fetch_nand, ""),
            (hirsel_tsan_atomic32_compare_exchange_val, __tsan_atomic32_compare_exchange_val, __tsan_atomic32_compare_exchange_val, ""),
            (hirsel_tsan_atomic64_load, __tsan_atomic64_load, __tsan_atomic64_load, ""),
            (hirsel_tsan_atomic64_store, __tsan_atomic64_store, __tsan_atomic64_store, ""),
            (hirsel_tsan_atomic64_exchange, __tsan_atomic64_exchange, __tsan_atomic64_exchange, ""),
            (hirsel_tsan_atomic64_fetch_add, __tsan_atomic64_fetch_add, __tsan_atomic64_fetch_add, ""),
            (hirsel_tsan_atomic64_fetch_sub, __tsan_atomic64_fetch_sub, __tsan_atomic64_fetch_sub, ""),
            (hirsel_tsan_atomic64_fetch_and, __tsan_atomic64_fetch_and, __tsan_atomic64_fetch_and, ""),
            (hirsel_tsan_atomic64_fetch_or, __tsan_atomic64_fetch_or, __tsan_atomic64_fetch_or, ""),
            (hirsel_tsan_atomic64_fetch_xor, __tsan_atomic64_fetch_xor, __tsan_atomic64_fetch_xor, ""),
            (hirsel_tsan_atomic64_fetch_nand, __tsan_atomic64_fetch_nand, __tsan_atomic64_fetch_nand, ""),
            (hirsel_tsan_atomic64_compare_exchange_val, __tsan_atomic64_compare_exchange_val, __tsan_atomic64_compare_exchange_val, ""),
        }
    };
}

macro_rules! inventory {
    ($(($bridge:ident, $callback:ident, $target:ident, $pc:literal),)*) => {
        /// Exact bridge names and the compiler-rt entries they invoke.
        pub const CALLBACKS: &[(&str, &str)] = &[$((stringify!($bridge), stringify!($callback))),*];
    };
}
tsan_callbacks!(inventory);

/// Unknown callbacks retain their original spelling and cannot receive a
/// private-stack certificate.
pub fn bridge(callback: &str) -> Option<&'static str> {
    CALLBACKS
        .iter()
        .find_map(|&(bridge, raw)| (callback == raw).then_some(bridge))
}

/// Only the fixed thunks have a checked private footprint. Raw compiler-rt
/// callbacks, including callbacks not in this inventory, have no such bound.
pub fn private_stack_bytes(bridge: &str) -> Option<u64> {
    CALLBACKS
        .iter()
        .any(|&(name, _)| name == bridge)
        .then_some(PRIVATE_STACK_BYTES)
}

#[cfg(test)]
mod tests {
    #[test]
    fn private_certification_rejects_raw_unknown_and_stack_argument_callbacks() {
        use super::*;
        assert_eq!(private_stack_bytes("hirsel_tsan_read8"), Some(16));
        for name in [
            "__tsan_read8",
            "hirsel_tsan_unknown",
            "hirsel_tsan_atomic128_compare_exchange_val",
        ] {
            assert_eq!(private_stack_bytes(name), None);
        }
        assert_eq!(bridge("__tsan_atomic128_compare_exchange_val"), None);
        assert_eq!(bridge("__tsan_read8"), Some("hirsel_tsan_read8"));
    }
}

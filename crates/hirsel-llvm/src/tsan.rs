#[cfg(test)]
#[path = "../../../tools/tests/fixtures/native_tsan/control.rs"]
mod control;

use hirsel_lir::{emission::Manifested, stage::ObjectSymbolKind};

// LLVM ThreadSanitizer.cpp initialize(): only these instrumentation imports
// are admitted. The ordinary object validator still rejects every other name.
pub(crate) fn manifest(manifested: &mut Manifested) {
    for name in [
        "__tsan_func_entry",
        "__tsan_func_exit",
        "__tsan_init",
        "__tsan_memcpy",
        "__tsan_memmove",
        "__tsan_memset",
        "__tsan_atomic_thread_fence",
        "__tsan_atomic_signal_fence",
    ] {
        manifested.import(name, ObjectSymbolKind::Runtime, None);
    }
    for bytes in [1, 2, 4, 8, 16] {
        for access in [
            "read",
            "write",
            "read_write",
            "unaligned_read",
            "unaligned_write",
            "unaligned_read_write",
        ] {
            manifested.import(
                &format!("__tsan_{access}{bytes}"),
                ObjectSymbolKind::Runtime,
                None,
            );
        }
        for operation in [
            "load",
            "store",
            "exchange",
            "fetch_add",
            "fetch_sub",
            "fetch_and",
            "fetch_or",
            "fetch_xor",
            "fetch_nand",
            "compare_exchange_val",
        ] {
            manifested.import(
                &format!("__tsan_atomic{}_{operation}", bytes * 8),
                ObjectSymbolKind::Runtime,
                None,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::control;
    use crate::context::{Context, Module, TargetMachine};
    use hirsel_lir::{emission::OptLevel, layout_types::TargetSpec};

    #[test]
    fn native_tsan_instruments_real_loads_stores_and_atomics() {
        let target = TargetSpec::x86_64_unknown_linux_gnu();
        for opt in [OptLevel::None, OptLevel::Speed] {
            let context = Context::new();
            let module = Module::parse(
                &context,
                "tsan",
                r#"define i64 @mapper(ptr %input, ptr %output, ptr %count) {
  %value = load i64, ptr %input, align 1
  store i64 %value, ptr %output, align 1
  %old = atomicrmw add ptr %count, i64 1 monotonic
  ret i64 %old
}
define void @tsan_mapper(ptr %cell) {
  %value = load i64, ptr %cell
  %next = add i64 %value, 1
  store i64 %next, ptr %cell
  ret void
}
"#,
            )
            .unwrap();
            let machine = TargetMachine::new(&target, opt != OptLevel::None).unwrap();
            module.set_target(&machine);
            module
                .run_passes(super::super::emit::pipeline(opt), &machine)
                .unwrap();
            module.instrument_thread(&machine).unwrap();
            module.verify().unwrap();
            let text = module.text();
            for name in [
                "@__tsan_unaligned_read8",
                "@__tsan_unaligned_write8",
                "@__tsan_atomic64_fetch_add",
                "@__tsan_func_entry",
                "@__tsan_func_exit",
            ] {
                assert!(text.contains(name), "missing {name}: {text}");
            }
            if opt == OptLevel::Speed {
                control::emitted_access_control(&module.emit_object(&machine).unwrap());
            }
        }
    }
}

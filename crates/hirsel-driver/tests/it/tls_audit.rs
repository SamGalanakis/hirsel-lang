//! Generated code reads no thread-local storage. A task's compiled frames receive its context,
//! which names its task and never its worker, so they stay valid when the task resumes on
//! another thread; a thread-local read in them would not. Every emitted object is refused if it
//! has a TLS section, symbol or relocation (`hirsel_lir::emission::artifact`); this audit
//! compiles programs that reach every suspending operation family through both emitters, for
//! both targets and both optimisation levels, and reads each object back.
use hirsel_driver::compiler_adapters;
use hirsel_driver::native::{
    Backend, OptLevel, Options, Partition, TargetSpec, build, emit_backend_objects,
};
use hirsel_engine::{
    QueryEngine, check_progress, default_context, default_environment, with_compiler_stack,
};
use hirsel_lir::object::ObjectFile;

/// Programs whose code suspends in tasks, joins, task sets, channels, mutexes, atomics, frozen
/// values, arenas, cancellation and finalizers.
const PROGRAMS: [&str; 10] = [
    "conformance/CLO/CLO-3-child-atomic-capture-run.hir",
    "conformance/CONC/CONC-2-task-take-capture-freeze-run.hir",
    "conformance/CONC/CONC-2-task-set-first-completed-run.hir",
    "conformance/CONC/CONC-5-owned-payload-mutexes-run.hir",
    "conformance/CONC/CONC-6-suspended-receiver-run.hir",
    "conformance/CONC/CONC-3-cancel-order-child-run.hir",
    "conformance/CONC/CONC-3-shielded-drop-send-run.hir",
    "conformance/FAIL/FAIL-4-owned-payload-dead-run.hir",
    "conformance/MEM/MEM-4-copy-destinations-run.hir",
    "conformance/MEM/MEM-11-nested-root-release-run.hir",
];

#[test]
fn emitted_objects_read_no_thread_local_storage() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    with_compiler_stack(move || {
        for path in PROGRAMS {
            let source = std::fs::read_to_string(root.join(path)).unwrap();
            let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
            let file = engine.sources.add(path, source).unwrap();
            let output = check_progress(&mut engine, file, &default_context()).unwrap();
            assert!(
                output.diagnostics.is_empty(),
                "{path}: {:?}",
                output.diagnostics
            );
            let input = engine.export_native_input().unwrap();
            let validated = engine.import_native_input(input).unwrap();
            for target in [
                TargetSpec::x86_64_unknown_linux_gnu(),
                TargetSpec::aarch64_unknown_linux_gnu(),
            ] {
                let ready = build(&validated, &Options::new(target.clone())).unwrap();
                for (backend, opt) in [
                    (Backend::Llvm, OptLevel::Speed),
                    (Backend::Llvm, OptLevel::None),
                    (Backend::Cranelift, OptLevel::None),
                ] {
                    let objects =
                        emit_backend_objects(&ready, backend, opt, Partition::Whole, false, None)
                            .unwrap_or_else(|e| panic!("{path} {backend:?} {opt:?}: {e:?}"));
                    for object in objects {
                        let file = ObjectFile::read(&target, object.bytes).unwrap();
                        assert!(
                            file.thread_locals.is_empty(),
                            "{path} {} {backend:?} {opt:?}: {:?}",
                            target.triple,
                            file.thread_locals
                        );
                    }
                }
            }
        }
    });
}

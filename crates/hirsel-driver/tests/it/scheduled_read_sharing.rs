use hirsel_driver::*;
use hirsel_engine::{runtime::*, store::MemStore, *};
use std::sync::Arc;

#[test]
fn front_complete_reads_share_equal_phase_allocations_without_merging_phase_keys() {
    with_compiler_stack(|| {
        let directory = tempfile::tempdir().unwrap();
        let manifest = directory.path().join("hirsel.json");
        std::fs::write(directory.path().join("main.hir"), "pub fn main()->u8{1u8}").unwrap();
        std::fs::write(&manifest, r#"{"schema":1,"package":{"name":"sharing","version":"1"},"sources":[{"path":"main.hir","module":[]}],"dependencies":[],"entry":{"module":[],"name":"main"}}"#).unwrap();
        let mut program = COMPILER
            .with_config(CompileConfig {
                store: Arc::new(MemStore::default()),
                ..Default::default()
            })
            .with_owned_engines()
            .load_program(&manifest)
            .unwrap();
        let definitions = program.engine.front_source_roots();
        let budget = ProcessBudget::new(64 << 20);
        let errors = crate::scheduled_front::errors();
        let front = program
            .engine
            .front_body_queries(
                &definitions,
                MemoryWeight {
                    retained: 1 << 20,
                    scratch: 8 << 20,
                    stack: 0,
                },
                budget.reserve(8 << 20).unwrap(),
                errors,
            )
            .unwrap();
        let mut runtime = AdmittedRuntime::with_reservation(
            hirsel_intern::jobs::Jobs::new(1),
            budget.reserve(32 << 20).unwrap(),
            errors,
            2 << 20,
        )
        .unwrap();
        runtime.register(FRONT_BODY_KIND, front.factory()).unwrap();
        let request = runtime.request(&[front.query(&program.entry).unwrap()]);
        let product = runtime.wait(request).unwrap()[0].clone();
        let output = product.summary_value().unwrap();
        let rows = front.memo_snapshot();
        let find = |phase| {
            rows.iter()
                .find(|(key, _)| key.kind == phase)
                .unwrap()
                .1
                .clone()
        };
        let checked = find(QueryKind::Check);
        let modes = find(QueryKind::Modes);
        assert_eq!(checked, modes, "this vector adds no Modes read");
        assert!(
            Arc::ptr_eq(&checked, &modes),
            "equal complete phase reads retain one allocation"
        );
        assert!(Arc::ptr_eq(&modes, &output.value().reads));
        let before = front.retained_bytes();
        let second = front.checked(Ok(product.clone())).unwrap();
        assert_eq!(front.retained_bytes(), before);
        assert_eq!(second.reads, output.value().reads);
        assert!(product.archive(ArchiveAdmission::Retained).is_err());
        assert_eq!(front.recompute_count(&program.entry, QueryKind::Resolve), 1);
        assert_eq!(front.recompute_count(&program.entry, QueryKind::Check), 1);
        assert_eq!(front.recompute_count(&program.entry, QueryKind::Modes), 1);
        runtime.shutdown();
    });
}

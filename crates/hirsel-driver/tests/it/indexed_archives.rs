//! Actual compiler products read through the same typed live/archive contract.
use hirsel_driver::COMPILER;
use hirsel_engine::{
    CheckedTir, CompileConfig, DefinitionQueries, NativeRecordContext, NativeRecords, TirView,
    store::{MemStore, ResultStore},
    with_compiler_stack,
};
use std::sync::Arc;

#[test]
fn actual_lower_products_have_checked_indexed_body_and_instruction_rows() {
    with_compiler_stack(|| {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("main.hir");
        std::fs::write(
            &path,
            "fn choose(x:bool)->u32{if x{7u32}else{9u32}}\npub fn main()->u32{choose(true)}\n",
        )
        .unwrap();
        let store = Arc::new(MemStore::default());
        let mut program = COMPILER
            .with_config(CompileConfig {
                store: store.clone(),
                ..Default::default()
            })
            .with_owned_engines()
            .load_program_source(&path, None)
            .unwrap();
        for name in ["main", "choose"] {
            let mut path = program.entry.clone();
            path.0.name = name.into();
            let lowered = program.engine.lower_to_tir(&path).unwrap();
            assert!(lowered.diagnostics.is_empty(), "{:?}", lowered.diagnostics);
            let definition = lowered.value;
            let context = Arc::new(NativeRecordContext::capture(&program.engine));
            let records = NativeRecords::new(program.engine.types().clone(), store.clone());
            let encoded = records.encode_tir(context.as_ref(), &definition).unwrap();
            let expected = store.put_content(CheckedTir::DOMAIN, &encoded).unwrap();
            let archived = records
                .open_tir(
                    context.clone(),
                    store.get_content(expected).unwrap(),
                    expected,
                    context.clone(),
                )
                .unwrap();
            let live = TirView::Live(&definition);
            let stored = TirView::Archived(&archived);
            assert_eq!(live.header(), stored.header());
            assert_eq!(live.body_count(), stored.body_count());
            let mut instructions = 0;
            for n in 0..live.body_count() {
                let left = live.body(n).unwrap();
                let right = stored.body(n).unwrap();
                assert_eq!(left.header(), right.header());
                assert_eq!(left.local_count(), right.local_count());
                for i in 0..left.local_count() {
                    assert_eq!(left.local(i), right.local(i));
                }
                assert_eq!(left.block_count(), right.block_count());
                for i in 0..left.block_count() {
                    let left = left.block(i).unwrap();
                    let right = right.block(i).unwrap();
                    assert_eq!(left.span(), right.span());
                    assert_eq!(left.terminator(), right.terminator());
                    assert_eq!(left.instruction_count(), right.instruction_count());
                    for k in 0..left.instruction_count() {
                        assert_eq!(left.instruction(k), right.instruction(k));
                        instructions += 1;
                    }
                }
            }
            assert!(instructions > 0);
            assert_eq!(archived.materialize().unwrap(), *definition);
        }
    });
}

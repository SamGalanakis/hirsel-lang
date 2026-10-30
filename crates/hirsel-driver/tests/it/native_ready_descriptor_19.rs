//! Original plan forgery vectors applied to actual root and Ready descriptors.
use hirsel_driver::{compiler_adapters, native};
use hirsel_engine::{
    runtime::*,
    store::{self, ResultStore, VerifyStore},
    *,
};
use native::queries::{self, NativeRoots};
use std::sync::Arc;

#[test]
fn actual_root_rejects_changed_owner_policy_and_c_bound_ready_summary() {
    with_compiler_stack(|| {
        let directory = tempfile::tempdir().unwrap();
        let selected: Arc<dyn ResultStore> =
            Arc::new(store::DirStore::new(directory.path().join("products")));
        let transport = ProcessBudget::new(24 << 20);
        let store: Arc<dyn ResultStore> = Arc::new(store::RequestStore::new(
            selected,
            transport.reserve(24 << 20).unwrap(),
        ));
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters())
            .with_result_store(store.clone());
        let file = engine
            .sources
            .add(
                "plan.hir",
                "fn helper(x:u64)->u64{x+1u64}\nfn main()->u64{helper(7u64)}".into(),
            )
            .unwrap();
        let checked = check_progress(&mut engine, file, &default_context()).unwrap();
        assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
        let input = engine.export_native_input().unwrap();
        let _validated = engine.import_native_input(input).unwrap();
        let main = entry_path(&default_context());
        crate::native_object_products_19::compile_front(
            crate::native_front_fixture::products(&mut engine, &[]).unwrap(),
            store.clone(),
            &[main.clone()],
            native::Backend::Cranelift,
            native::OptLevel::None,
            native::Options::new(native::TargetSpec::x86_64_unknown_linux_gnu()),
            false,
            hirsel_intern::jobs::Jobs::ONE,
            VerifyStore::Off,
            false,
            |view, root, readies| {
                let (summaries, table, _keys) = view
                    .with_link_context(root, readies, |program| {
                        assert!(program.units.is_empty());
                        Ok((
                            program.summaries.clone(),
                            program.table.clone(),
                            program.keys.clone(),
                        ))
                    })
                    .unwrap();
                let mut missing = (**root).clone();
                missing.instances.clear();
                assert!(
                    view.with_link_context(&Arc::new(missing), readies, |_| Ok(()))
                        .is_err()
                );
                let mut wrong_policy = (**root).clone();
                wrong_policy.instances[0].policy =
                    hirsel_syntax::ContentHash::of_bytes(b"forged policy");
                assert!(
                    view.with_link_context(&Arc::new(wrong_policy), readies, |_| Ok(()))
                        .is_err()
                );
                let original = summaries
                    .iter()
                    .find(|summary| summary.definition.0.name == "main")
                    .unwrap();
                assert!(!original.calls.is_empty(), "original written helper call");
                let selected = root
                    .instances
                    .iter()
                    .position(|owner| owner.instance.definition().0.name == "main")
                    .unwrap();
                let address = root.instances[selected].address(queries::READY_KIND);
                let selected = readies
                    .iter()
                    .position(|ready| ready.product.address == address)
                    .unwrap();
                // Preserve each original plan forgery independently. Coherent
                // summary bytes retain the real owner and product identity;
                // only current C can authorize their complete metadata.
                for mutation in 0..4 {
                    let mut summary = original.clone();
                    let mut table = table.clone();
                    match mutation {
                        0 => summary.recovers = !summary.recovers,
                        1 => {
                            summary
                                .glue
                                .callbacks
                                .insert((DefPath::core("unselected"), None));
                        }
                        2 => {
                            let mut abi = table.fn_abis[summary.abi.0 as usize].clone();
                            abi.result_arena = Some(0);
                            summary.abi = hirsel_lir::FnAbiId(table.fn_abis.len() as u32);
                            table.fn_abis.push(abi);
                        }
                        3 => summary.calls.clear(),
                        _ => unreachable!(),
                    }
                    let keys = hirsel_lir::LayoutKeys::new(&table);
                    let bytes = summary.encode(&table, &keys).unwrap();
                    let reference = store
                        .put_content(queries::ready::UNIT_SUMMARY_DOMAIN, &bytes)
                        .unwrap();
                    assert_ne!(reference, readies[selected].unit_summary);
                    let mut forged = readies.to_vec();
                    let mut candidate = (*forged[selected]).clone();
                    candidate.unit_summary = reference;
                    forged[selected] = Arc::new(candidate);
                    assert!(
                        view.with_link_context(root, &forged, |_| Ok(())).is_err(),
                        "ordinary summary mutation {mutation} acquired C authority"
                    );
                }
                let mut wrong_schema = (**root).clone();
                let archive = root.product.archive(ArchiveAdmission::Retained).unwrap();
                let mut domain = archive.reference().domain;
                domain.schema += 1;
                let reference = store.put_content(domain, archive.bytes()).unwrap();
                wrong_schema.product = crate::scheduled_front::imported_fixture(
                    &*store,
                    root.product.address,
                    root.product.action,
                    reference,
                );
                assert!(
                    view.with_link_context(&Arc::new(wrong_schema), readies, |_| Ok(()))
                        .is_err()
                );
            },
            |_, _| (),
        );
    });
}

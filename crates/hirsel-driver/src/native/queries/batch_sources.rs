use crate::native::receipt_source::{factory::NamedInput, provider::Front};
use hirsel_layout::layout::LayoutBuilder;
use hirsel_rc::instance::{InstanceRole, Instances, Program};

pub(in crate::native) fn capture(
    reader: Option<&hirsel_engine::NativeOriginalReader>,
    program: &Program,
    instances: &mut Instances,
    layouts: &mut LayoutBuilder<'_>,
) -> Result<hirsel_rc::import::SourceData, String> {
    let Some(reader) = reader else {
        return Ok(Default::default());
    };
    let provider = Front::new(reader);
    let roots = instances
        .ids()
        .filter(|id| instances.roles[id.index()] == InstanceRole::Function)
        .map(|id| NamedInput {
            base: instances.lexical_key(id).clone(),
            actuals: instances.actuals[id.index()].clone(),
        })
        .collect();
    let (graph, receipts) = super::receipt_requests::reconstruct(&provider, roots, program)?;
    for request in &graph.requests {
        let candidates = instances
            .ids()
            .filter(|id| {
                instances.lexical_key(*id).definition() == request.ordinary_key().definition()
                    && instances.lexical_key(*id).body() == request.ordinary_key().body()
            })
            .collect::<Vec<_>>();
        let mut selected = None;
        for id in candidates {
            if instances
                .receipt_lexical_matches(id, request, layouts)
                .map_err(|e| format!("batch lexical receipt: {e:?}"))?
                && selected.replace(id).is_some()
            {
                return Err("ambiguous batch source receipt".into());
            }
        }
        let id = selected.ok_or("batch source receipt has no lexical implementation")?;
        instances
            .bind_receipt_request(id, request, layouts)
            .map_err(|e| format!("batch source receipt: {e:?}"))?;
    }
    let expectations = super::seal_inventory::current(&provider, &graph, program, |_| true)?;
    Ok(hirsel_rc::import::SourceData {
        expectations,
        receipts,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn heap_witness_imports_each_original_and_library_body() {
        hirsel_engine::with_compiler_stack(|| {
            let source = include_str!("../../../../../conformance/MEM/MEM-4-sealed-heap-run.hir");
            let scratch = tempfile::tempdir().unwrap();
            let path = scratch.path().join("sealed-heap.hir");
            std::fs::write(&path, source).unwrap();
            let manifest = scratch.path().join("hirsel.json");
            std::fs::write(
                &manifest,
                serde_json::json!({
                    "schema": 1,
                    "package": {"name": "standalone", "version": "0"},
                    "sources": [{"path": "sealed-heap.hir", "module": []}],
                    "dependencies": [],
                    "entry": {"module": [], "name": "main"},
                })
                .to_string(),
            )
            .unwrap();
            let mut loaded = crate::load_program(&manifest).unwrap();
            loaded.check_for_execution().unwrap();
            let input = loaded.engine.export_native_input().unwrap();
            let validated = loaded.engine.import_native_input(input).unwrap();
            let program = crate::native::adapt::program(&validated);
            let target = hirsel_lir::TargetSpec::x86_64_unknown_linux_gnu();
            let owners = hirsel_rc::library::owners(&program.types, &program.signatures)
                .over(&program.owners);
            let mut layouts = LayoutBuilder::new(&target, &owners);
            layouts.prepare_native_controls();
            let mut instances = hirsel_rc::instance::expand(&program, &mut layouts).unwrap();
            let sources = capture(
                validated.original_reader(),
                &program,
                &mut instances,
                &mut layouts,
            )
            .unwrap();
            let mut index = 0;
            let mut failures = Vec::new();
            while index < instances.len() {
                let id = hirsel_rc::instance::InstanceId(index as u32);
                index += 1;
                let key = (
                    instances.view(id).key.definition().clone(),
                    instances.view(id).key.digest(),
                );
                if let Some(expected) = sources.expectations.get(&key) {
                    instances.bind_payload_sources(id, expected.payloads().to_vec());
                }
                match hirsel_rc::import::import_instance(
                    &program,
                    &mut instances,
                    id,
                    None,
                    &mut layouts,
                ) {
                    Ok(imported) => {
                        for request in imported.required {
                            hirsel_rc::library::instantiate(&mut layouts, &mut instances, request)
                                .unwrap();
                        }
                    }
                    Err(error) => failures.push(format!(
                        "body {} {:?}: {error}",
                        instances.view(id).key.body(),
                        key.0
                    )),
                }
            }
            assert!(index > 3, "original and library bodies executed");
            assert!(failures.is_empty(), "{}", failures.join("\n"));
            let ready =
                crate::native::build(&validated, &crate::native::Options::new(target)).unwrap();
            let run = crate::native::run_heap(
                &ready,
                &loaded.entry,
                &crate::native::Sites::new(validated.snapshot()),
            );
            let run = run.unwrap();
            assert_eq!(run.observation.exit_code, Some(0));
            assert!(run.observation.failures.is_empty(), "{:?}", run.observation);
        });
    }
}

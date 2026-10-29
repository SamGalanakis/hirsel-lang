//! Object-only requests compile helpers that have no executable launch ABI.
use hirsel_driver::{
    native::{self, Backend},
    requests::{
        CompileOperation, CompileRequest, CompileResult, CompilerRequests, NativeOutput,
        NativeRequest, NativeSourceText,
    },
};
use hirsel_engine::{
    requests::{OutputReadiness, RequestEngine, observe_inputs},
    runtime::QueryRuntime,
};
use std::sync::Arc;

#[test]
fn object_requests_emit_generic_helpers_without_link_or_launch() {
    hirsel_engine::with_compiler_stack(|| {
        let directory = tempfile::tempdir().unwrap();
        let manifest = directory.path().join("hirsel.json");
        std::fs::write(directory.path().join("sample.hir"), "pub fn keep[T](name take value:T)->T uses {value} {value}\npub fn empty[T]()->List[T] uses {} {[]}\n").unwrap();
        std::fs::write(&manifest, r#"{"schema":1,"package":{"name":"object_helpers","version":"1"},"sources":[{"path":"sample.hir","module":["sample"]}],"dependencies":[]}"#).unwrap();
        let compiler = hirsel_driver::COMPILER.with_owned_engines();
        let target = native::TargetSpec::x86_64_unknown_linux_gnu();
        let tools = Arc::new(
            native::Toolchain::in_tree(&directory.path().join("tools"), &[&target]).unwrap(),
        );
        let mut expected = None;
        for backend in [Backend::Cranelift, Backend::Llvm] {
            let mut selector =
                CompilerRequests::for_host(compiler.clone(), compiler.configuration())
                    .with_toolchain(tools.clone());
            let (loaded, inputs) = observe_inputs(
                selector.input_limits(),
                selector.capture_attempts(),
                |inputs| compiler.load_check_package_inputs(&manifest, inputs, true),
            )
            .unwrap();
            let loaded = loaded.unwrap();
            let context = loaded
                .files
                .iter()
                .find(|(_, context)| context.package == loaded.entry.0.package)
                .unwrap()
                .1
                .clone();
            let mut first = loaded.entry.clone();
            first.0.name = "native_coverage_keep".into();
            let mut second = first.clone();
            second.0.name = "native_coverage_empty".into();
            drop(loaded);
            let request = CompileRequest {
                path: manifest.clone(), inputs,
                operation: CompileOperation::NativeBuild(NativeRequest {
                    definition: Some(first.clone()),
                    test_entries: vec![],
                    supplemental_sources: vec![NativeSourceText {
                        name: "native-surface/sample.hir".into(), context,
                        text: "fn native_coverage_keep(name take value:u64)->u64 {keep(name: take value)}\nfn native_coverage_empty()->List[u64]{empty()}\n".into(),
                    }],
                    output: NativeOutput::Objects { roots: vec![first, second] },
                    optimization: Some(native::OptLevel::None),
                    backend, profile: native::BuildProfile::Default,
                    target: target.clone(), validation: native::Validation::EveryStage,
                    launch: Default::default(), run: None, fuel: false, evidence: None,
                }),
            };
            let mut selected = selector.select(&request).unwrap();
            drop(selector);
            let complete = {
                let mut runtime = selected.runtime.lock().unwrap();
                let request = runtime.request(&selected.roots);
                runtime.wait(request)
            }
            .unwrap_or_else(|reference| {
                panic!(
                    "object request: {:?}",
                    selected
                        .output
                        .failed(reference)
                        .map(|error| error.to_string())
                )
            });
            let output = selected.output.ready(&complete).unwrap();
            let CompileResult::NativeObjects(objects) = &output.result else {
                panic!("object-only request returned another result");
            };
            assert!(
                objects.instances >= 4,
                "both generic wrappers and their selected instances"
            );
            if let Some(expected) = expected {
                assert_eq!(objects.instances, expected);
            } else {
                expected = Some(objects.instances);
            }
            assert!(!objects.products.is_empty());
            for product in &objects.products {
                let fragment = native::object_queries::fragment_product(
                    objects.store.as_ref(),
                    product.clone(),
                )
                .unwrap();
                let object =
                    native::object_queries::load_object(objects.store.as_ref(), fragment.object)
                        .unwrap();
                assert!(object.bytes.starts_with(b"\x7fELF"));
                assert!(
                    native::object_queries::linked_product(objects.store.as_ref(), product.clone())
                        .is_err(),
                    "a Codegen output cannot supply an executable"
                );
            }
        }
    });
}

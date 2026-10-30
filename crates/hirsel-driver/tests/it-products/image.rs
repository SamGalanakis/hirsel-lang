use crate::image_fixture::*;
use hirsel_driver::{requests::*, *};
use hirsel_engine::*;
use std::sync::Arc;

#[test]
fn imaged_generic_comptime_materializes_selected_client_sizes() {
    with_compiler_stack(|| {
        let f = Fixture::new();
        let source = "pub fn value[n:size](xs:Inline[u8,n])->u64{let _=xs;comptime (n*n)}\n";
        let client = "pub fn main()->Result[(),()]{let a:Inline[u8,2]=[1,2];let b:Inline[u8,3]=[1,2,3];failure.check(lib.value(a)==4u64,Arithmetic());failure.check(lib.value(b)==9u64,Arithmetic());Ok(())}\n";
        f.write("lib.hir", source);
        let library = f.0.join("lib.json");
        std::fs::write(&library, serde_json::json!({"schema":1,"package":{"name":"lib","version":"1"},"sources":[{"path":"lib.hir","module":["lib"]}],"dependencies":[]}).to_string()).unwrap();
        let _imager = f.program("imager", client, &library, "lib");
        let image = f.0.join("lib.products");
        COMPILER.precompute_package(&library, &image).unwrap();
        let client = f.program("client", client, &library, "lib");
        let config = |store| CompileConfig {
            store,
            jobs: hirsel_intern::jobs::Jobs::new(1),
            ..CompileConfig::for_memory_cap(None)
        };
        let cached_config = config(Arc::new(
            store::DirStore::new(f.0.join("reader")).with_read_only_root(image),
        ) as Arc<dyn store::ResultStore>);
        let fresh_config = config(Arc::new(store::NoneStore));
        let mut cached =
            CompilerRequests::for_host(COMPILER.with_config(cached_config.clone()), cached_config);
        let mut fresh =
            CompilerRequests::for_host(COMPILER.with_config(fresh_config.clone()), fresh_config);
        let actual = cached
            .execute(&client, CompileOperation::NativeSources { roots: vec![] })
            .unwrap();
        let expected = fresh
            .execute(&client, CompileOperation::NativeSources { roots: vec![] })
            .unwrap();
        let (CompileResult::NativeSources(actual), CompileResult::NativeSources(expected)) =
            (&actual.result, &expected.result)
        else {
            panic!("expected native sources");
        };
        let selected = |input: &FrontNativeProducts| {
            input
                .statics
                .constants
                .iter()
                .filter(|(path, _)| path.0.name.starts_with("$constant:$comptime:value:"))
                .map(|(path, value)| (path.clone(), value.root.kind.clone()))
                .collect::<std::collections::BTreeMap<_, _>>()
        };
        let values = selected(actual);
        assert_eq!(values, selected(expected));
        assert_eq!(
            values.values().cloned().collect::<Vec<_>>(),
            vec![
                hirsel_tir::ValueKind::Constant(hirsel_tir::Constant::Integer(4)),
                hirsel_tir::ValueKind::Constant(hirsel_tir::Constant::Integer(9)),
            ]
        );
        let validated = cached
            .execute(
                &client,
                CompileOperation::NativeOwnership(OwnershipRequest {
                    selection: hirsel_lir::ownership_trace::Selection::Resources,
                    definition: None,
                    target: native::TargetSpec::x86_64_unknown_linux_gnu(),
                    validation: native::Validation::EveryStage,
                    launch: Default::default(),
                    fuel: false,
                }),
            )
            .unwrap();
        let CompileResult::Ownership(validated) = &validated.result else {
            panic!("expected validated ownership");
        };
        assert!(!validated.units().is_empty());
    });
}

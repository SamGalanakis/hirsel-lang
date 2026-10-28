use hirsel_driver::{COMPILER, requests::*};
use hirsel_engine::{CompileConfig, LoweringPolicy, QueryError, ToolError, with_compiler_stack};

#[test]
fn private_result_payload_inference_matches_scalar_preparation() {
    with_compiler_stack(|| {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(
            directory.path().join("main.hir"),
            r#"pub fn produced()->Result[unique Bytes,()] { Ok(bytes.from_list([1u8])) }
fn execute()->Result[Bytes,()] { produced() }
pub fn main()->Result[(),()] {
  let work = fn()->Result[unique Bytes,()] { execute() }
  let _ = work()?
  Ok(())
}
"#,
        )
        .unwrap();
        let manifest = directory.path().join("hirsel.json");
        std::fs::write(&manifest, r#"{"schema":1,"package":{"name":"private_result","version":"1"},"sources":[{"path":"main.hir","module":[]}],"dependencies":[],"entry":{"module":[],"name":"main"}}"#).unwrap();
        let config = CompileConfig::default();
        let compiler = COMPILER.with_config(config.clone());
        let mut scalar = compiler.load_program(&manifest).unwrap();
        let cold = scalar.check(LoweringPolicy::Reachable).unwrap();
        for diagnostic in &cold.diagnostics {
            eprintln!("SCALAR {}", diagnostic.render_human(&cold.sources));
        }
        let mut requests = CompilerRequests::for_host(compiler, config);
        let result = requests.execute(&manifest, CompileOperation::Tir { definition: None });
        if let Err(ToolError::Query {
            error: QueryError::Diagnostics(diagnostics),
            sources,
            ..
        }) = &result
        {
            for diagnostic in diagnostics {
                eprintln!("REQUEST {}", diagnostic.render_human(sources));
            }
        }
        assert!(cold.diagnostics.is_empty());
        assert!(result.is_ok(), "request preparation failed");
    });
}

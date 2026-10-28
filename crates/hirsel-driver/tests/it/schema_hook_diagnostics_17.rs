//! Field-attributed failures, following Serde check.rs:29–53 (MIT OR Apache-2.0).
//! Expected hook contracts below are written independently of generated text.
use hirsel_driver::{
    requests::{CompileOperation, CompileResult},
    session::Session,
};
use hirsel_engine::{LoweringPolicy, with_compiler_stack};
use std::{fs, path::PathBuf};

fn checked(session: &mut Session) -> hirsel_engine::CheckOutput {
    match session.request(CompileOperation::Check {
        lowering: LoweringPolicy::Comptime,
    }) {
        Ok(output) => {
            let CompileResult::Check(checked) = &output.result else {
                panic!("expected Check")
            };
            checked.clone()
        }
        // A generated declaration can reject during registration, before the
        // Check roots exist. Its source diagnostic is a completed refusal.
        Err(hirsel_engine::ToolError::Query {
            error: hirsel_engine::QueryError::Diagnostics(diagnostics),
            sources,
            ..
        }) => hirsel_engine::CheckOutput {
            diagnostics,
            sources,
            completion: hirsel_engine::CheckCompletion::Complete,
            unsafe_ffi: vec![],
        },
        Err(error) => panic!("{error}"),
    }
}

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap()
}

fn source(direction: &str, variant: bool) -> String {
    let outer = if variant {
        "type Outer = Item(service: List[Service])"
    } else {
        "type Outer { service: Service }"
    };
    let (input, output) = if direction == "decode" {
        ("json.Value", "Outer")
    } else {
        ("Outer", "json.Value")
    };
    format!(
        "// λ relocated application\ntype Service {{ name: String }}\n{outer}\nfn outer(wire: {input}, mut check: schema.CheckState) -> Result[unique {output}, schema.SchemaError] uses {{}} = comptime derive.schema_{direction}(shape(Outer), schema.strict_policy())\n"
    )
}

fn fixture(source: &str, dependency: Option<&str>) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("app.hir"), source).unwrap();
    let mut dependencies = vec![
        serde_json::json!({"manifest":root().join("stdlib/std@1/hirsel.json"),"package":{"name":"std","version":"1"},"expose":[]}),
    ];
    if let Some(helper) = dependency {
        fs::create_dir(dir.path().join("helper")).unwrap();
        fs::write(dir.path().join("helper/types.hir"), helper).unwrap();
        fs::write(dir.path().join("helper/hirsel.json"), serde_json::json!({"schema":1,"package":{"name":"helper","version":"0"},"sources":[{"path":"types.hir","module":["model"]}],"dependencies":[dependencies[0].clone()]}).to_string()).unwrap();
        dependencies.push(serde_json::json!({"manifest":"helper/hirsel.json","package":{"name":"helper","version":"0"},"expose":[]}));
    }
    fs::write(dir.path().join("hirsel.json"), serde_json::json!({"schema":1,"package":{"name":"hooks","version":"0"},"sources":[{"path":"app.hir","module":[]}],"dependencies":dependencies,"entry":{"module":[],"name":"main"}}).to_string()).unwrap();
    dir
}

fn reject(
    source: &str,
    dependency: Option<&str>,
    direction: &str,
    member: &str,
    rule: &str,
    callee: &str,
    nominal: &str,
) {
    with_compiler_stack(|| {
        let dir = fixture(source, dependency);
        let mut program = Session::open(&dir.path().join("hirsel.json"), hirsel_engine::Verify::On);
        let output = checked(&mut program);
        assert_eq!(output.completion, hirsel_engine::CheckCompletion::Complete);
        let errors: Vec<_> = output
            .diagnostics
            .iter()
            .filter(|d| d.severity == hirsel_diag::Severity::Error)
            .collect();
        assert_eq!(errors.len(), 1, "{errors:#?}");
        let d = errors[0];
        assert_eq!(d.rule.as_str(), rule, "{d:#?}");
        let primary = output.sources.slice(d.primary).unwrap();
        if rule == "LANG-4" {
            assert_eq!(primary, callee);
        }
        let at = output.sources.resolve(d.primary);
        let generated = output.sources.file(at.file).unwrap();
        assert_eq!(generated.path, PathBuf::from("<generated:hooks@0/outer>"));
        if rule == "LANG-4" {
            let expected =
                generated.text().find(&format!("match {callee}(")).unwrap() + "match ".len();
            assert_eq!(
                (at.start as usize, at.end as usize),
                (expected, expected + callee.len())
            );
        }

        assert_eq!(
            &generated.text()[at.start as usize..at.end as usize],
            primary
        );
        let (input, result) = if direction == "decode" {
            ("std@1/json.Value", nominal)
        } else {
            (nominal, "std@1/json.Value")
        };
        let contract = format!(
            "fn {callee}(value: {input}, mut check: std@1/schema.CheckState) -> core@1/Result[unique {result}, std@1/schema.SchemaError] uses {{}}"
        );
        let context = format!("schema field `hooks@0/Outer.{member}` requires `{contract}`");
        let field = d
            .secondary
            .iter()
            .find(|l| l.message == context)
            .unwrap_or_else(|| panic!("missing field/contract {context}: {d:#?}"));
        assert!(
            output
                .sources
                .slice(field.span)
                .unwrap()
                .contains("Service")
        );
        assert_eq!(
            output
                .sources
                .file(output.sources.resolve(field.span).file)
                .unwrap()
                .path,
            dir.path().join("app.hir")
        );
        let origin =
            format!("comptime derive.schema_{direction}(shape(Outer), schema.strict_policy())");
        assert!(
            d.secondary
                .iter()
                .any(|l| output.sources.slice(l.span) == Some(origin.as_str()))
        );
        let human = d.render_human(&output.sources);
        let json = d.to_json(&output.sources);
        assert!(human.contains(&context), "{human}");
        assert!(
            json["secondary"]
                .as_array()
                .unwrap()
                .iter()
                .any(|l| l["message"] == context)
        );
        assert!(d.fix.is_none());
        let repeated = checked(&mut program);
        assert_eq!(
            output
                .diagnostics
                .iter()
                .map(|d| d.to_json(&output.sources))
                .collect::<Vec<_>>(),
            repeated
                .diagnostics
                .iter()
                .map(|d| d.to_json(&repeated.sources))
                .collect::<Vec<_>>()
        );
    });
}

#[test]
fn schema_hook_diagnostics_17_missing_record_decode_encode() {
    for direction in ["decode", "encode"] {
        reject(
            &source(direction, false),
            None,
            direction,
            "service",
            "LANG-4",
            &format!("hooks@0/service_schema_{direction}"),
            "hooks@0/Service",
        );
    }
}

#[test]
fn schema_hook_diagnostics_17_variant_container_decode_encode() {
    for direction in ["decode", "encode"] {
        reject(
            &source(direction, true),
            None,
            direction,
            "Item.service",
            "LANG-4",
            &format!("hooks@0/service_schema_{direction}"),
            "hooks@0/Service",
        );
        reject(
            &source(direction, true).replace("Item(service: List[Service])", "Item(List[Service])"),
            None,
            direction,
            "Item.0",
            "LANG-4",
            &format!("hooks@0/service_schema_{direction}"),
            "hooks@0/Service",
        );
    }
}

#[test]
fn schema_hook_diagnostics_17_wrong_mode() {
    let s = format!(
        "{}\nfn service_schema_decode(wire: json.Value, take check: schema.CheckState) -> Result[unique Service, schema.SchemaError] uses {{}} {{ Ok(Service(name: \"x\")) }}\n",
        source("decode", false)
    );
    reject(
        &s,
        None,
        "decode",
        "service",
        "OWN-2",
        "hooks@0/service_schema_decode",
        "hooks@0/Service",
    );
}

#[test]
fn schema_hook_diagnostics_17_wrong_result() {
    let s = format!(
        "{}\nfn service_schema_encode(value: Service, mut check: schema.CheckState) -> Result[unique String, schema.SchemaError] uses {{}} {{ Ok(\"x\") }}\n",
        source("encode", false)
    );
    reject(
        &s,
        None,
        "encode",
        "service",
        "LANG-7",
        "hooks@0/service_schema_encode",
        "hooks@0/Service",
    );
}

#[test]
fn schema_hook_diagnostics_17_cross_package_module() {
    let s = source("decode", false)
        .replace("type Service { name: String }\n", "")
        .replace("service: Service", "service: helper@0/model.Service");
    reject(
        &s,
        Some("pub type Service { name: String }"),
        "decode",
        "service",
        "LANG-4",
        "helper@0/model.service_schema_decode",
        "helper@0/model.Service",
    );
}

#[test]
fn schema_hook_diagnostics_17_handwritten_missing_name_unaffected() {
    with_compiler_stack(|| {
        let dir = fixture("fn ordinary() { missing_hook() }", None);
        let mut program = Session::open(&dir.path().join("hirsel.json"), hirsel_engine::Verify::On);
        let output = checked(&mut program);
        assert_eq!(output.diagnostics.len(), 1, "{:?}", output.diagnostics);
        let d = &output.diagnostics[0];
        assert_eq!(d.rule.as_str(), "LANG-4");
        assert_eq!(output.sources.slice(d.primary), Some("missing_hook"));
        assert!(
            !d.secondary
                .iter()
                .any(|l| l.message.contains("schema field"))
        );
    });
}

#[test]
fn schema_hook_diagnostics_17_manual_repairs_execute_both_backends() {
    with_compiler_stack(|| {
        let source = r#"type Service { name: String }
type Outer { service: Service }
type Choice = Item(service: List[Service])
fn service_schema_decode(wire: json.Value, mut check: schema.CheckState) -> Result[unique Service, schema.SchemaError] uses {} {
  schema.charge_check(mut check)?
  match wire {
    Text(s) => Ok(Service(name: unique s)),
    _ => Error(schema.scalar_error(mut check, "String")),
  }
}
fn service_schema_encode(value: Service, mut check: schema.CheckState) -> Result[unique json.Value, schema.SchemaError] uses {} {
  schema.charge_check(mut check)?
  Ok(Text(unique value.name))
}
fn outer_schema_decode(wire: json.Value, mut check: schema.CheckState) -> Result[unique Outer, schema.SchemaError] uses {} = comptime derive.schema_decode(shape(Outer), schema.strict_policy())
fn outer_schema_encode(value: Outer, mut check: schema.CheckState) -> Result[unique json.Value, schema.SchemaError] uses {} = comptime derive.schema_encode(shape(Outer), schema.strict_policy())
fn choice_schema_decode(wire: json.Value, mut check: schema.CheckState) -> Result[unique Choice, schema.SchemaError] uses {} = comptime derive.schema_decode(shape(Choice), schema.strict_policy())
fn choice_schema_encode(value: Choice, mut check: schema.CheckState) -> Result[unique json.Value, schema.SchemaError] uses {} = comptime derive.schema_encode(shape(Choice), schema.strict_policy())
pub fn main() -> Result[(), String] uses {} {
  let mut check = match schema.check_state(schema.default_options()) { Ok(c) => take c, Error(_) => return Error("options") }
  let Ok(wire) = json.parse("{\"service\":\"雪\"}") else { return Error("JSON") }
  let Ok(outer) = outer_schema_decode(wire, mut check) else { return Error("decode record") }
  if outer.service.name != "雪" { return Error("record value") }
  let Ok(encoded) = outer_schema_encode(outer, mut check) else { return Error("encode record") }
  let Ok(text) = json.stringify(encoded) else { return Error("JSON") }
  if text != "{\"service\":\"雪\"}" { return Error("record golden") }
  let Ok(variant_wire) = json.parse("{\"Item\":[[\"雪\"]]}") else { return Error("JSON") }
  let Ok(choice) = choice_schema_decode(variant_wire, mut check) else { return Error("decode variant") }
  match choice { Item(service: values) => { if values[0].name != "雪" { return Error("variant value") } } }
  let Ok(variant_encoded) = choice_schema_encode(choice, mut check) else { return Error("encode variant") }
  let Ok(variant_text) = json.stringify(variant_encoded) else { return Error("JSON") }
  if variant_text != "{\"Item\":[[\"雪\"]]}" { return Error("variant golden") }
  Ok(())
}
"#;
        let dir = fixture(source, None);
        let path = dir.path().join("hirsel.json");
        let prepared = hirsel_driver::request_tir(&path, hirsel_engine::Verify::On, None)
            .unwrap_or_else(|e| match e {
                hirsel_engine::ToolError::Query {
                    error: hirsel_engine::QueryError::Diagnostics(ds),
                    sources,
                    ..
                } => panic!(
                    "{}",
                    ds.iter()
                        .map(|d| d.render_human(&sources))
                        .collect::<String>()
                ),
                e => panic!("{e}"),
            });
        let config = hirsel_engine::RunConfiguration {
            entry: prepared.entry.clone(),
            hash_seed: 0,
            inputs: vec![],
        };
        let execution = hirsel_driver::testing::observe_tir(
            &prepared,
            &config,
            hirsel_engine::SimulationConfig::default(),
            hirsel_interp::Limits::default(),
            true,
        )
        .unwrap();
        let observation = hirsel_driver::native::Observation::of_reference(
            execution.observation.as_ref().unwrap(),
        );
        assert_eq!(observation.exit_code, Some(0), "{observation:?}");
        assert!(observation.failures.is_empty(), "{observation:?}");
        for backend in [
            hirsel_driver::native::Backend::Llvm,
            hirsel_driver::native::Backend::Cranelift,
        ] {
            let exe = dir.path().join(format!("repair-{backend:?}"));
            let built = hirsel_driver::native::build_path_native_backend(
                &path,
                &exe,
                hirsel_driver::native::TargetSpec::x86_64_unknown_linux_gnu(),
                hirsel_driver::native::BuildProfile::Default,
                backend,
                None,
                false,
                hirsel_engine::Verify::On,
            )
            .unwrap();
            assert!(built.is_ok(), "{built:?}");
            let output = std::process::Command::new(&exe).output().unwrap();
            assert!(output.status.success(), "{backend:?}: {output:?}");
            assert!(
                output.stdout.is_empty() && output.stderr.is_empty(),
                "{output:?}"
            );
        }
    });
}

#[test]
fn schema_hook_diagnostics_17_generic_bound_keeps_gen_owner() {
    let s = format!(
        "{}\nfn service_schema_encode[T: plain](value: T, mut check: schema.CheckState) -> Result[unique json.Value, schema.SchemaError] uses {{}} {{ Ok(Text(\"x\")) }}\n",
        source("encode", false)
    );
    reject(
        &s,
        None,
        "encode",
        "service",
        "GEN-1",
        "hooks@0/service_schema_encode",
        "hooks@0/Service",
    );
}

#[test]
fn schema_hook_diagnostics_17_cross_package_manual_twins() {
    with_compiler_stack(|| {
        for direction in ["decode", "encode"] {
            let s = source(direction, false)
                .replace("type Service { name: String }\n", "")
                .replace("service: Service", "service: helper@0/model.Service");
            let helper = "pub type Service { name: String }\npub fn service_schema_decode(wire: json.Value, mut check: schema.CheckState) -> Result[unique Service, schema.SchemaError] uses {} { match wire { Text(s) => Ok(Service(name: unique s)), _ => Error(schema.scalar_error(mut check, \"String\")) } }\npub fn service_schema_encode(value: Service, mut check: schema.CheckState) -> Result[unique json.Value, schema.SchemaError] uses {} { Ok(Text(unique value.name)) }\n";
            let dir = fixture(&s, Some(helper));
            let mut program =
                Session::open(&dir.path().join("hirsel.json"), hirsel_engine::Verify::On);
            let output = checked(&mut program);
            assert_eq!(output.completion, hirsel_engine::CheckCompletion::Complete);
            assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
        }
    });
}

#[test]
fn schema_hook_diagnostics_17_nested_nominal_variant() {
    for direction in ["decode", "encode"] {
        let s = source(direction, false).replace(
            "type Service { name: String }",
            "type Service = Named(String)",
        );
        reject(
            &s,
            None,
            direction,
            "service",
            "LANG-4",
            &format!("hooks@0/service_schema_{direction}"),
            "hooks@0/Service",
        );
    }
}

//! Field attribution and incorrect custom hook vectors follow Serde's
//! internals/check.rs:29–53 and tests/ui/with/incorrect_type.rs (MIT OR Apache-2.0).
//! Contracts are handwritten independently of the generator.
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

fn fixture(source: &str, helper: Option<&str>) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("relocated.hir"), source).unwrap();
    let stdlib = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../stdlib/std@1/hirsel.json")
        .canonicalize()
        .unwrap();
    let mut sources = vec![serde_json::json!({"path":"relocated.hir","module":[]})];
    if let Some(helper) = helper {
        fs::write(dir.path().join("model.hir"), helper).unwrap();
        sources.push(serde_json::json!({"path":"model.hir","module":["model"]}));
    }
    fs::write(dir.path().join("hirsel.json"), serde_json::json!({"schema":1,"package":{"name":"hooks","version":"0"},"sources":sources,"dependencies":[{"manifest":stdlib,"package":{"name":"std","version":"1"},"expose":[]}],"entry":{"module":[],"name":"main"}}).to_string()).unwrap();
    dir
}

fn source(encode: bool, shape: &str) -> String {
    let declaration = if encode {
        "fn outer(value: Outer) -> json.Value uses {} = comptime derive.json_value(shape(Outer))"
    } else {
        "fn outer(value: json.Value) -> Result[Outer, String] uses {} = comptime derive.from_json(shape(Outer))"
    };
    format!(
        "// 雪 λ relocated source\ntype Service {{ name: String }}\ntype Outer {shape}\n{declaration}\n"
    )
}

fn reject(
    source: &str,
    helper: Option<&str>,
    encode: bool,
    member: &str,
    rule: &str,
    module: &str,
) {
    with_compiler_stack(|| {
        let dir = fixture(source, helper);
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
        let suffix = if encode { "to_json" } else { "from_json" };
        let callee = format!("hooks@0/{module}service_{suffix}");
        let nominal = format!("hooks@0/{module}Service");
        let contract = if encode {
            format!("fn {callee}(value: {nominal}) -> std@1/json.Value uses {{}}")
        } else {
            format!(
                "fn {callee}(value: std@1/json.Value) -> core@1/Result[{nominal}, String] uses {{}}"
            )
        };
        let context = format!("schema field `hooks@0/Outer.{member}` requires `{contract}`");
        let labels: Vec<_> = d
            .secondary
            .iter()
            .filter(|l| l.message.starts_with("schema field"))
            .collect();
        assert_eq!(labels.len(), 1, "{d:#?}");
        assert_eq!(labels[0].message, context);
        let field = labels[0];
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
            dir.path().join("relocated.hir")
        );
        let at = output.sources.resolve(d.primary);
        let generated = output.sources.file(at.file).unwrap();
        assert_eq!(generated.path, PathBuf::from("<generated:hooks@0/outer>"));
        assert_eq!(
            &generated.text()[at.start as usize..at.end as usize],
            output.sources.slice(d.primary).unwrap()
        );
        if rule == "LANG-4" {
            assert_eq!(output.sources.slice(d.primary), Some(callee.as_str()));
            let start = generated.text().find(&format!("= {callee}(")).unwrap() + 2;
            assert_eq!(
                (at.start as usize, at.end as usize),
                (start, start + callee.len())
            );
        }
        let origin = if encode {
            "comptime derive.json_value(shape(Outer))"
        } else {
            "comptime derive.from_json(shape(Outer))"
        };
        assert!(
            d.secondary
                .iter()
                .any(|l| l.message == "generated by this `= comptime` body"
                    && output.sources.slice(l.span) == Some(origin))
        );
        assert!(d.render_human(&output.sources).contains(&context));
        let json = d.to_json(&output.sources);
        assert!(
            json["secondary"]
                .as_array()
                .unwrap()
                .iter()
                .any(|l| l["message"] == context)
        );
        assert!(d.fix.is_none());
    });
}

#[test]
fn json_hook_guidance_24_record_nested_list_option() {
    for encode in [true, false] {
        for ty in [
            "Service",
            "Option[Service]",
            "List[Option[Service]]",
            "Option[List[Service]]",
            "Map[String, Service]",
            "(u64, Service)",
        ] {
            reject(
                &source(encode, &format!("{{ unrelated: u64, service: {ty} }}")),
                None,
                encode,
                "service",
                "LANG-4",
                "",
            );
        }
    }
}

#[test]
fn json_hook_guidance_24_labelled_and_positional_payloads() {
    for encode in [true, false] {
        reject(
            &source(encode, "{ service: Service }").replace(
                "type Service { name: String }",
                "type Service = Named(String)",
            ),
            None,
            encode,
            "service",
            "LANG-4",
            "",
        );
        for (shape, member) in [
            (
                "= Item(service: List[Option[Service]]) | Other(u64)",
                "Item.service",
            ),
            ("= Item(List[Service]) | Other(u64)", "Item.0"),
            ("= Item(u64, Option[Service]) | Other(u64)", "Item.1"),
        ] {
            reject(&source(encode, shape), None, encode, member, "LANG-4", "");
        }
    }
}

#[test]
fn json_hook_guidance_24_wrong_argument_result_and_mode() {
    for (encode, hook, rule) in [
        (
            true,
            "fn service_to_json(value: u64) -> json.Value uses {} { Null }",
            "LANG-7",
        ),
        (
            true,
            "fn service_to_json(value: Service) -> String uses {} { \"wrong\" }",
            "LANG-7",
        ),
        (
            true,
            "fn service_to_json(take value: Service) -> json.Value uses {} { Null }",
            "OWN-2",
        ),
        (
            false,
            "fn service_from_json(value: u64) -> Result[Service, String] uses {} { Ok(Service(name: \"x\")) }",
            "LANG-7",
        ),
        (
            false,
            "fn service_from_json(value: json.Value) -> Result[u64, String] uses {} { Ok(0u64) }",
            "LANG-7",
        ),
        (
            false,
            "fn service_from_json(take value: json.Value) -> Result[Service, String] uses {} { Ok(Service(name: \"x\")) }",
            "OWN-2",
        ),
    ] {
        reject(
            &format!("{}\n{hook}\n", source(encode, "{ service: List[Service] }")),
            None,
            encode,
            "service",
            rule,
            "",
        );
    }
}

#[test]
fn json_hook_guidance_24_cross_module_nominal() {
    for encode in [true, false] {
        let s = source(encode, "{ service: model.Service }")
            .replace("type Service { name: String }\n", "");
        reject(
            &s,
            Some("pub type Service { name: String }"),
            encode,
            "service",
            "LANG-4",
            "model.",
        );
        let helper = "pub type Service { name: String }\npub fn service_to_json(value: Service) -> json.Value uses {} = comptime derive.json_value(shape(Service))\npub fn service_from_json(value: json.Value) -> Result[Service, String] uses {} = comptime derive.from_json(shape(Service))\n";
        with_compiler_stack(|| {
            let dir = fixture(&s, Some(helper));
            let mut p = Session::open(&dir.path().join("hirsel.json"), hirsel_engine::Verify::On);
            let output = checked(&mut p);
            assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
        });
    }
}

#[test]
fn json_hook_guidance_24_recipe_roundtrip_interpreter_checked_h_and_native() {
    with_compiler_stack(|| {
        let docs = include_str!("../../../../docs/spec/std-library.md");
        let recipe = docs
            .split("### Nested JSON helpers")
            .nth(1)
            .unwrap()
            .split("```hirsel\n")
            .nth(1)
            .unwrap()
            .split("```")
            .next()
            .unwrap();
        let source = format!(
            "{}\n{}",
            recipe.replace("pub fn main()", "fn recipe_main()"),
            r#"
type Choice = Labelled(items: List[Option[Item]]) | Positional(Item) | Tuple(u64, Item)
fn choice_to_json(value: Choice) -> json.Value uses {} = comptime derive.json_value(shape(Choice))
fn choice_from_json(value: json.Value) -> Result[unique Choice, String] uses {} = comptime derive.from_json(shape(Choice))
type Refusal { name: String }
type ErrorOrder { first: Refusal, last: u64 }
fn refusal_from_json(value: json.Value) -> Result[unique Refusal, String] uses {} { Error("nested hook rejected") }
fn error_order_from_json(value: json.Value) -> Result[unique ErrorOrder, String] uses {} = comptime derive.from_json(shape(ErrorOrder))
pub fn main() -> Result[(), String] uses {} {
  recipe_main()?
  let values: List[String] = ["{\"Labelled\":{\"items\":[{\"name\":\"雪\"},null]}}", "{\"Positional\":{\"name\":\"λ\"}}", "{\"Tuple\":[7,{\"name\":\"雪\"}]}"]
  for expected in values {
    let Ok(wire) = json.parse(unique expected) else { return Error("parse variant") }
    let decoded = choice_from_json(wire)?
    let Ok(actual) = json.stringify(choice_to_json(decoded)) else { return Error("encode variant") }
    if actual != expected { return Error("variant form") }
  }
  let Ok(rejected) = json.parse("{\"first\":null}") else { return Error("parse error control") }
  match error_order_from_json(rejected) {
    Error(message) => { if message != "nested hook rejected" { return Error("hook error or evaluation order changed") } },
    Ok(_) => return Error("hook error lost"),
  }
  Ok(())
}
"#
        );
        let dir = fixture(&source, None);
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
        for checked in [false, true] {
            let execution = hirsel_driver::testing::observe_tir(
                &prepared,
                &config,
                hirsel_engine::SimulationConfig::default(),
                hirsel_interp::Limits::default(),
                checked,
            )
            .unwrap();
            let observation = hirsel_driver::native::Observation::of_reference(
                execution.observation.as_ref().unwrap(),
            );
            assert_eq!(observation.exit_code, Some(0), "{observation:?}");
            assert!(observation.failures.is_empty(), "{observation:?}");
        }
        let capped = dir.path().join("capped");
        fs::write(&capped, include_str!("../../../../tools/capped")).unwrap();
        for backend in [
            hirsel_driver::native::Backend::Llvm,
            hirsel_driver::native::Backend::Cranelift,
        ] {
            let exe = dir.path().join(format!("roundtrip-{backend:?}"));
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
            let output = std::process::Command::new("python3")
                .arg(&capped)
                .args(["--timeout", "60"])
                .arg(exe)
                .env("HIRSEL_JOBS", "1")
                .env("HIRSEL_MEMORY_LIMIT", "2G")
                .output()
                .unwrap();
            assert!(output.status.success(), "{backend:?}: {output:?}");
            assert!(output.stdout.is_empty(), "{output:?}");
        }
    });
}

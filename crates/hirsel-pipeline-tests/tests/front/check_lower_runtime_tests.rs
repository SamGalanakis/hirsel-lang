//! Real phase outputs execute against observable finalizer effects.
use crate::check_support::{Provider, path};
use hirsel_engine::{DefinitionQueries, QueryEngine};
use hirsel_interp::{Execution, Limits, Outcome, Value};
use hirsel_syntax::{FileContext, ItemKind};
use hirsel_tir::RunConfiguration;

struct Emit {
    fail: Option<String>,
}
impl hirsel_interp::Operations for Emit {
    fn call(
        &mut self,
        call: hirsel_interp::OperationCall<'_>,
        context: &mut hirsel_interp::OperationContext<'_>,
    ) -> std::result::Result<Option<hirsel_interp::OperationResult>, hirsel_interp::Error> {
        if call.path.0.name != "emit" {
            return Ok(None);
        }
        let Value::String(text) = &call.arguments[0] else {
            panic!("text argument")
        };
        context.stdout.extend_from_slice(text.as_bytes());
        if self.fail.as_ref() == Some(text) {
            return Ok(Some(hirsel_interp::OperationResult::Failed(
                hirsel_interp::Failure {
                    rule: const { hirsel_diag::RuleId::of("LANG-9") },
                    line: 6,
                    site: call.site,
                    kind: "check".into(),
                },
            )));
        }
        Ok(Some(hirsel_interp::OperationResult::returned(Value::Unit)))
    }
}

const RESOURCE: &str = r#"@noalloc fn emit(text: String)
resource type Token { text: String } @noalloc drop(self) { emit(self.text) }
"#;

fn run_source(program: &str, fail: Option<&str>) -> Execution {
    let mut engine = QueryEngine::new(
        hirsel_engine::default_environment(),
        hirsel_driver::compiler_adapters(),
    );
    let context = FileContext {
        package: path("main").0.package,
        modules: vec![],
    };
    let source = format!("{RESOURCE}{program}");
    let file = engine.sources.add("fixture.hir", source).unwrap();
    let parsed = engine.parse_file(file, &context).unwrap();
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let mut definitions = vec![];
    for id in &parsed.value.roots {
        let item = &parsed.value.tree.items[id.0 as usize];
        if matches!(
            &item.kind,
            ItemKind::Function { body: Some(_), .. } | ItemKind::Type { drop: Some(_), .. }
        ) {
            definitions.push((*engine.lower_to_tir(&item.definition).unwrap().value).clone());
        }
    }
    let runtime = engine.runtime_types().unwrap();
    hirsel_interp::run_with_runtime(
        &mut Provider(definitions),
        &engine.sources,
        &RunConfiguration {
            entry: path("main"),
            hash_seed: 0,
            inputs: vec![],
        },
        vec![],
        Limits::default(),
        &mut Emit {
            fail: fail.map(str::to_owned),
        },
        &runtime,
    )
    .unwrap()
}

#[test]
fn cleanup_continues_after_finalizer_failure_and_keeps_original_failure() {
    let execution = run_source(
        r#"fn main() {
    let last = Token(text: "last\n")
    let broken = Token(text: "broken\n")
    let _ = ()
    let _ = 255u8 + 1u8
}"#,
        Some("broken\n"),
    );
    assert!(
        matches!(execution.outcome, Outcome::Failed(ref f) if f.rule.as_str() == "SEM-1" && f.line == 7)
    );
    assert_eq!(execution.stdout, b"broken\nlast\n");
    assert_eq!(execution.failures.len(), 2);
}

#[test]
fn match_tables_share_guard_fallbacks_and_unguarded_arm_bodies() {
    for fields in [4, 8, 16] {
        for guarded in [false, true] {
            let types = vec!["bool"; fields].join(",");
            let mut arms = vec![];
            for selected in 0..fields {
                let patterns = (0..fields)
                    .map(|i| if i == selected { "true" } else { "_" })
                    .collect::<Vec<_>>()
                    .join(",");
                let guard = if guarded {
                    format!(" if reject({selected}u64)")
                } else {
                    String::new()
                };
                arms.push(format!("({patterns}){guard}=>{selected}u64"));
            }
            arms.push(format!("_=>{fields}u64"));
            let source = format!(
                "@noalloc fn reject(n:u64)->bool{{false}}\nfn main(p:({types}))->u64{{match p{{{}}}}}",
                arms.join(",")
            );
            let mut engine = QueryEngine::new(
                hirsel_engine::default_environment(),
                hirsel_driver::compiler_adapters(),
            );
            let context = FileContext {
                package: path("main").0.package,
                modules: vec![],
            };
            let file = engine.sources.add("fixture.hir", source).unwrap();
            let parsed = engine.parse_file(file, &context).unwrap();
            assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
            let definition = engine.lower_to_tir(&path("main")).unwrap().value;
            let instructions: usize = definition.bodies[0]
                .blocks
                .iter()
                .map(|b| b.instructions.len())
                .sum();
            let source_cells = (fields + 1) * fields;
            assert!(
                definition.bodies[0].blocks.len() + instructions <= 12 * source_cells,
                "{fields} fields, guarded={guarded}: {} blocks, {instructions} instructions",
                definition.bodies[0].blocks.len()
            );
        }
    }
}

#[test]
fn distinct_constructor_patterns_execute_from_the_table() {
    let execution = run_source(
        "distinct type Percent(v:u8)\nfn main()->u8 {let p=Percent(7);match p {Percent(n)=>n}}",
        None,
    );
    assert!(
        matches!(execution.outcome,Outcome::Returned {value:Value::Integer(n),..} if n.bits()==7)
    );
}

//! Read-through-owner controls follow Rust Arc's Deref and get_mut control shapes
//! (f6c1107f, alloc/src/rcs/arc.rs:2553 and alloctests/tests/sync.rs:79).
//! Hirsel instead uses a bounded callback and consuming freeze/thaw; no Weak.
use hirsel_driver::*;
use hirsel_engine::*;
use std::path::PathBuf;

fn rendered(output: &CheckOutput, expected: &[(&str, &str)]) -> Vec<serde_json::Value> {
    assert_eq!(output.completion, CheckCompletion::Complete);
    let errors: Vec<_> = output
        .diagnostics
        .iter()
        .filter(|d| d.severity == hirsel_diag::Severity::Error)
        .collect();
    assert_eq!(errors.len(), expected.len(), "{:?}", output.diagnostics);
    for (d, (rule, primary)) in errors.iter().zip(expected) {
        assert_eq!(d.rule.as_str(), *rule, "{d:?}");
        assert_eq!(output.sources.slice(d.primary), Some(*primary));
        assert!(d.fix.is_none());
        let json = d.to_json(&output.sources);
        let human = d.render_human(&output.sources);
        let at = output.sources.resolve(d.primary);
        assert_eq!(json["primary"]["start"], at.start);
        assert_eq!(json["primary"]["end"], at.end);
        for (label, encoded) in d
            .secondary
            .iter()
            .zip(json["secondary"].as_array().unwrap())
        {
            let at = output.sources.resolve(label.span);
            assert!(output.sources.slice(label.span).is_some());
            assert_eq!(encoded["span"]["start"], at.start);
            assert_eq!(encoded["span"]["end"], at.end);
            assert_eq!(encoded["message"], label.message);
            assert!(human.contains(&label.message));
        }
        for note in json["notes"].as_array().unwrap() {
            assert!(human.contains(note.as_str().unwrap()), "{human}");
        }
        eprintln!("{human}{json}");
    }
    errors.iter().map(|d| d.to_json(&output.sources)).collect()
}

fn check(source: &str, expected: &[(&str, &str)]) -> Vec<serde_json::Value> {
    with_compiler_stack(|| {
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
        let file = engine.sources.add("shared.hir", source.into()).unwrap();
        let output = check_progress(&mut engine, file, &default_context()).unwrap();
        let json = rendered(&output, expected);
        let warm = check_progress(&mut engine, file, &default_context()).unwrap();
        assert_eq!(json, rendered(&warm, expected));
        json
    })
}

fn guidance(json: &serde_json::Value, payload: &str) -> String {
    let notes = json["notes"].as_array().unwrap();
    assert_eq!(notes.len(), 3, "{json}");
    assert_eq!(json["secondary"].as_array().unwrap().len(), 1);
    assert_eq!(json["secondary"][0]["span"], json["primary"]);
    let help = notes[0].as_str().unwrap();
    assert!(help.contains("help: read inside shared.with"), "{json}");
    assert!(
        help.contains(&format!("fn(value: {payload}) -> ()")),
        "{json}"
    );
    assert!(
        notes[1]
            .as_str()
            .unwrap()
            .contains("plain value, a deep `unique` copy, or a copied nested Shared root")
    );
    assert!(
        notes[2]
            .as_str()
            .unwrap()
            .contains("shared.thaw(take root)")
    );
    assert!(notes[2].as_str().unwrap().contains("Error(root)"));
    help.split('`').nth(1).unwrap().to_owned()
}

const RECORD: &str = "type Record { value: u64 }\n";

#[test]
fn shared_read_diagnostics_19_existing_failures_and_checked_snippets() {
    for (body, rule, primary, repaired) in [
        ("read(s)", "LANG-7", "read(s)", "read(value)"),
        ("s.value", "LANG-4", "s.value", "value.value"),
        (
            "match s { Record(value: n) => n }",
            "LANG-13",
            "Record(value: n)",
            "match value { Record(value: n) => n }",
        ),
    ] {
        let source = format!(
            "{RECORD}fn read(r: Record) -> u64 {{ r.value }}\nfn probe(s: Shared[Record]) -> u64 {{ {body} }}"
        );
        let diagnostics = check(&source, &[(rule, primary)]);
        let snippet = guidance(&diagnostics[0], "Record");
        check(
            &format!("{RECORD}fn example(root: Shared[Record]) {{ {snippet} }}"),
            &[],
        );
        check(
            &source.replace(
                body,
                &format!("shared.with(s, fn(value: Record) -> u64 {{ {repaired} }})"),
            ),
            &[],
        );
    }
}

#[test]
fn shared_read_diagnostics_19_nested_fields_and_copied_roots() {
    let declarations = format!("{RECORD}type Holder {{ config: Shared[Record] }}\n");
    for (body, rule, primary) in [
        ("holder.config.value", "LANG-7", "holder.config.value"),
        ("let s = copy holder.config; s.value", "LANG-7", "s.value"),
        ("read(holder.config)", "LANG-7", "read(holder.config)"),
    ] {
        let json = check(
            &format!(
                "{declarations}fn read(r: Record) -> u64 {{ r.value }}\nfn probe(holder: Holder) -> u64 {{ {body} }}"
            ),
            &[(rule, primary)],
        );
        let snippet = guidance(&json[0], "Record");
        check(
            &format!("{declarations}fn example(root: Shared[Record]) {{ {snippet} }}"),
            &[],
        );
    }
    check(
        &format!(
            "{declarations}fn nested(s: Shared[Holder]) -> Shared[Record] {{ shared.with(s, fn(holder: Holder) -> Shared[Record] {{ copy holder.config }}) }}"
        ),
        &[],
    );
    check(
        &format!(
            "{declarations}fn nested(s: Shared[Holder]) -> u64 {{ let root = shared.with(s, fn(holder: Holder) -> Shared[Record] {{ copy holder.config }}); shared.with(root, fn(r: Record) -> u64 {{ r.value }}) }}"
        ),
        &[],
    );
}

#[test]
fn shared_read_diagnostics_19_generic_payload_snippets() {
    for argument in ["()", "((),)", "((), ())", "Record"] {
        let declarations = format!("{RECORD}type Wrap[T] {{ value: T }}\n");
        let payload = format!("Wrap[{argument}]");
        let json = check(
            &format!("{declarations}fn probe(s: Shared[{payload}]) {{ let _ = s.value }}"),
            &[("LANG-4", "s.value")],
        );
        let snippet = guidance(&json[0], &payload);
        check(
            &format!("{declarations}fn example(root: Shared[{payload}]) {{ {snippet} }}"),
            &[],
        );
    }
}

#[test]
fn shared_read_diagnostics_19_existing_loan_refusals_keep_their_rules() {
    // These are the existing corpus twins, reused to assert that access help
    // never replaces a loan-lifetime or write/take refusal.
    for (name, rule, primary, provided, required) in [
        (
            "escape",
            "MODE-1",
            "copy i.items",
            "provided: `i` is a borrowed parameter; it lives in the caller's region",
            "required: the result names no `in ..` source, so it is global (MODE-7)",
        ),
        (
            "mut",
            "OWN-1",
            "list.push(mut i.items, 3)",
            "provided: `i` is borrowed: its caller lends it read-only (OWN-1)",
            "required: writing needs a `mut` binding or a `mut` parameter",
        ),
        (
            "take",
            "OWN-1",
            "consume(take i)",
            "provided: `i` is a borrowed parameter; it lives in the caller's region",
            "required: taking moves the value out of its owner",
        ),
    ] {
        let source = std::fs::read_to_string(
            root().join(format!("conformance/MEM/MEM-11-with-{name}-bad.hir")),
        )
        .unwrap();
        let json = check(&source, &[(rule, primary)]);
        let primary_start = source.rfind(primary).unwrap();
        assert_eq!(json[0]["primary"]["start"], primary_start);
        assert_eq!(json[0]["primary"]["end"], primary_start + primary.len());
        let labels = json[0]["secondary"].as_array().unwrap();
        assert_eq!(labels.len(), 2);
        let provided_start = source.rfind("fn(i: Index)").unwrap() + "fn(i: ".len();
        assert_eq!(labels[0]["span"]["start"], provided_start);
        assert_eq!(labels[0]["span"]["end"], provided_start + "Index".len());
        assert_eq!(labels[0]["message"], provided);
        let (required_start, required_len) = if name == "escape" {
            (
                source.rfind("-> List[u64]").unwrap() + "-> ".len(),
                "List[u64]".len(),
            )
        } else {
            (primary_start, primary.len())
        };
        assert_eq!(labels[1]["span"]["start"], required_start);
        assert_eq!(labels[1]["span"]["end"], required_start + required_len);
        assert_eq!(labels[1]["message"], required);
        assert!(
            !json[0]
                .to_string()
                .contains("help: read inside shared.with")
        );
        let source = std::fs::read_to_string(
            root().join(format!("conformance/MEM/MEM-11-with-{name}-ok.hir")),
        )
        .unwrap();
        check(&source, &[]);
    }
}

#[test]
fn shared_read_diagnostics_19_unrelated_errors_keep_generic_guidance() {
    for (source, rule, primary) in [
        (
            format!(
                "{RECORD}type Other {{ value: u64 }}\nfn read(r: Record) {{}}\nfn probe(s: Shared[Other]) {{ read(s) }}"
            ),
            "LANG-7",
            "read(s)",
        ),
        (
            format!("{RECORD}fn probe(s: Shared[Record]) {{ let _ = s.missing }}"),
            "LANG-4",
            "s.missing",
        ),
        (
            "type Shared { value: u64 }\nfn probe(s: Shared) { let _ = s.missing }".into(),
            "LANG-4",
            "s.missing",
        ),
        (
            format!(
                "{RECORD}type Other {{ value: u64 }}\nfn probe(s: Shared[Record]) {{ match s {{ Other(value: n) => () }} }}"
            ),
            "LANG-13",
            "Other(value: n)",
        ),
    ] {
        let json = check(&source, &[(rule, primary)]);
        assert!(!json[0].to_string().contains("shared.with"));
    }
}

#[test]
fn shared_read_diagnostics_19_imported_names_and_opacity() {
    with_compiler_stack(|| {
        let f = crate::image_fixture::Fixture::new();
        f.write("lib.hir", "pub type Record { value: u64 }\npub opaque type Hidden: inert uses {} { secret: u64 }\n");
        let library = f.0.join("library.json");
        std::fs::write(&library, serde_json::json!({"schema":1,"package":{"name":"library","version":"1"},"sources":[{"path":"lib.hir","module":[]}],"dependencies":[]}).to_string()).unwrap();
        for (parameter, body, rule, primary, has_help) in [
            ("library@1/Record", "s.value", "LANG-4", "s.value", true),
            ("library@1/Record", "read(s)", "LANG-7", "read(s)", true),
            ("library@1/Hidden", "s.secret", "LANG-4", "s.secret", false),
            (
                "library@1/Hidden",
                "shared.with(s, fn(value: library@1/Hidden) -> u64 { value.secret })",
                "PROG-2",
                "value.secret",
                false,
            ),
        ] {
            let prefix = "fn read(r: library@1/Record) -> u64 { r.value }\n";
            let source = format!(
                "{prefix}fn probe(s: Shared[{parameter}]) -> u64 {{ {body} }}\npub fn main() {{}}"
            );
            let path = f.program("app", &source, &library, "library");
            let mut p = load_program(&path).unwrap();
            let output = p.check(LoweringPolicy::Comptime).unwrap();
            let json = rendered(&output, &[(rule, primary)]);
            if has_help {
                let snippet = guidance(&json[0], parameter);
                let source = format!(
                    "fn probe(root: Shared[{parameter}]) {{ {snippet} }}\npub fn main() {{}}"
                );
                f.program("app", &source, &library, "library");
                let mut p = load_program(&path).unwrap();
                rendered(&p.check(LoweringPolicy::Comptime).unwrap(), &[]);
            } else {
                assert!(!json[0].to_string().contains("shared.with"));
            }
        }
    });
}

#[test]
fn shared_read_diagnostics_19_documented_read_copy_and_thaw() {
    let docs = std::fs::read_to_string(root().join("docs/spec/memory.md")).unwrap();
    let block = docs
        .split("<!-- shared-read-diagnostics-19 -->")
        .nth(1)
        .unwrap();
    let source = block
        .split("```hirsel\n")
        .nth(1)
        .unwrap()
        .split("```")
        .next()
        .unwrap();
    check(source, &[]);
}

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

#[test]
fn shared_read_diagnostics_19_existing_frozen_config_h_llvm_cranelift() {
    with_compiler_stack(|| {
        // Reuse the existing frozen-config read rather than another runtime fixture.
        let mut program =
            load_program(&root().join("conformance/MEM/MEM-11-tour-11-shared-config-run.hir.json"))
                .unwrap();
        program.check_for_execution().unwrap();
        let config = RunConfiguration {
            entry: program.entry.clone(),
            hash_seed: 0,
            inputs: vec![],
        };
        let execution =
            run_observed_checked(&mut program.engine, &config, SimulationConfig::default())
                .unwrap();
        let reference = native::Observation::of_reference(execution.observation.as_ref().unwrap());
        assert_eq!(reference.exit_code, Some(0));
        assert!(reference.failures.is_empty());
        let input = program.engine.export_native_input().unwrap();
        let validated = program.engine.import_native_input(input).unwrap();
        let options = native::Options::new(native::TargetSpec::x86_64_unknown_linux_gnu());
        let configurations = vec![
            native::Configuration {
                options: options.clone(),
                engine: native::Engine::Heap { campaign: false },
            },
            native::Configuration {
                options: options.clone(),
                engine: native::Engine::Native {
                    opt: native::OptLevel::Speed,
                    partition: native::Partition::Whole,
                    runtime: native::Runtime::Diagnostic,
                },
            },
            native::Configuration {
                options,
                engine: native::Engine::Cranelift {
                    partition: native::Partition::Whole,
                    runtime: native::Runtime::Diagnostic,
                },
            },
        ];
        let scratch = tempfile::Builder::new()
            .prefix("shared-read-diagnostics-19-")
            .tempdir()
            .unwrap();
        let toolchain =
            native::Toolchain::for_matrix(&scratch.path().join("runtime"), &configurations)
                .unwrap();
        let outcomes = native::differential(
            &validated,
            &program.entry,
            &reference,
            &configurations,
            &toolchain,
            scratch.path(),
        );
        assert_eq!(outcomes.len(), 3);
        for (configuration, outcome) in outcomes {
            eprintln!("shared-read-diagnostics-19/frozen-config: {configuration}: {outcome:?}");
            assert!(
                matches!(outcome, native::Outcome::Agree),
                "{configuration}: {outcome:?}"
            );
        }
    });
}

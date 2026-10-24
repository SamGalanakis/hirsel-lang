//! Located negative/positive vectors adapt rust-analyzer's diagnostic handlers
//! unresolved_ident.rs:6-56 and expected_function.rs:8-36 (MIT OR Apache-2.0),
//! revision 3131751eaf1c3b5da6e618acdc2148ad7db65bc7. Hirsel retains LANG-4's
//! contextual bare variants; no Rust resolution rules or implementation is copied.
use hirsel_driver::*;
use hirsel_engine::*;
use serde_json::{Value, json};
use std::path::Path;

const MODEL: &str = "pub type Outcome = Timeout | Done\n\
    pub type Maybe[T] = Present(T) | Missing\n\
    pub type Other = Timeout | Finished\n\
    pub fn finish() -> u64 { 1u64 }\n";

fn fixture(dir: &Path, model: &str, source: &str, dependency: bool, ambiguous: bool) {
    let owner_sources = json!([{"path":"model.hir","module":["model"]}]);
    let mut sources = json!([{"path":"main.hir","module":[]}]);
    let mut dependencies = vec![];
    if dependency {
        dependencies.push(json!({"manifest":"dep.json","package":{"name":"dep","version":"1"},"expose":[{"head":"vendor","module":["model"]}]}));
        if ambiguous {
            dependencies.push(json!({"manifest":"other.json","package":{"name":"other","version":"1"},"expose":[{"head":"vendor","module":["model"]}]}));
        }
    } else {
        sources
            .as_array_mut()
            .unwrap()
            .push(owner_sources[0].clone());
    }
    let files = json!({
        "model.hir":model,
        "main.hir":source,
        "dep.json":json!({"schema":1,"package":{"name":"dep","version":"1"},"sources":owner_sources,"dependencies":[]}).to_string(),
        "other.json":json!({"schema":1,"package":{"name":"other","version":"1"},"sources":[{"path":"other.hir","module":["model"]}],"dependencies":[]}).to_string(),
        "other.hir":"pub fn unrelated() {}\n",
        "hirsel.json":json!({"schema":1,"package":{"name":"witness","version":"1"},"sources":sources,"dependencies":dependencies,"entry":{"module":[],"name":"f"}}).to_string(),
    });
    eprintln!("VARIANT_GUIDANCE_48_FIXTURE {files}");
    for (name, text) in files.as_object().unwrap() {
        std::fs::write(dir.join(name), text.as_str().unwrap()).unwrap();
    }
}

fn checked(program: &mut LoadedProgram) -> CheckOutput {
    let output = program.check(LoweringPolicy::Comptime).unwrap();
    assert_eq!(output.completion, CheckCompletion::Complete);
    for d in &output.diagnostics {
        eprintln!("VARIANT_GUIDANCE_48_JSON {}", d.to_json(&output.sources));
        eprintln!(
            "VARIANT_GUIDANCE_48_HUMAN\n{}",
            d.render_human(&output.sources)
        );
    }
    output
}

fn json_diagnostics(output: &CheckOutput) -> Vec<Value> {
    output
        .diagnostics
        .iter()
        .map(|d| d.to_json(&output.sources))
        .collect()
}

fn accepted(dir: &Path, source: &str) {
    std::fs::write(dir.join("main.hir"), source).unwrap();
    let mut program = load_program(&dir.join("hirsel.json")).unwrap();
    let output = checked(&mut program);
    assert!(
        output.diagnostics.is_empty(),
        "{source}\n{:?}",
        output.diagnostics
    );
    let text = program.engine.export_tir(&program.entry).unwrap();
    program.engine.import_tir(&text).unwrap();
    eprintln!("VARIANT_GUIDANCE_48_ACCEPTED_V {source:?}");
}

#[test]
fn expected_variants_explain_bare_spelling_and_repairs_export() {
    with_compiler_stack(|| {
        // Repairs are independently written, then compared to the located edit.
        let cases = [
            (
                "fn f() -> model.Outcome { model.Timeout }\n",
                "model.Timeout",
                "Outcome",
                "fn f() -> model.Outcome { Timeout }\n",
            ),
            (
                "fn f() -> model.Outcome { model.Timeout() }\n",
                "model.Timeout",
                "Outcome",
                "fn f() -> model.Outcome { Timeout() }\n",
            ),
            (
                "fn f() -> model.Maybe[u64] { model.Present(1u64) }\n",
                "model.Present",
                "Maybe",
                "fn f() -> model.Maybe[u64] { Present(1u64) }\n",
            ),
            (
                "fn f() -> model.Maybe[model.Maybe[u64]] { Present(model.Present(1u64)) }\n",
                "model.Present",
                "Maybe",
                "fn f() -> model.Maybe[model.Maybe[u64]] { Present(Present(1u64)) }\n",
            ),
            (
                "fn f() -> model.Other { model.Timeout }\n",
                "model.Timeout",
                "Other",
                "fn f() -> model.Other { Timeout }\n",
            ),
            (
                "// café 🐑\n\nfn f() -> model.Outcome { witness@1/model.Timeout() }\n",
                "witness@1/model.Timeout",
                "Outcome",
                "// café 🐑\n\nfn f() -> model.Outcome { Timeout() }\n",
            ),
        ];
        for (source, spelling, owner, twin) in cases {
            let dir = tempfile::tempdir().unwrap();
            fixture(dir.path(), MODEL, source, false, false);
            let mut program = load_program(&dir.path().join("hirsel.json")).unwrap();
            let output = checked(&mut program);
            let [d] = &output.diagnostics[..] else {
                panic!("{:?}", output.diagnostics)
            };
            assert_eq!(d.rule.as_str(), "LANG-4");
            assert!(d.message.starts_with("unresolved name `"));
            assert_eq!(output.sources.slice(d.primary), Some(spelling));
            let span = output.sources.resolve(d.primary);
            let start = source.find(spelling).unwrap();
            assert_eq!(
                (span.start as usize, span.end as usize),
                (start, start + spelling.len())
            );
            let variant = spelling.rsplit('.').next().unwrap();
            let help = format!(
                "help: variants are written bare and selected by the expected type; write `{variant}` without the qualifier for `witness@1/model/{owner}` (keep any payload arguments). When several types declare `{variant}`, supply an expected type, for example with a binding annotation or function return type"
            );
            assert!(d.notes.contains(&help), "{:?}", d.notes);
            assert!(
                d.fix.is_none(),
                "help alone must not promise a machine repair"
            );
            assert!(d.render_human(&output.sources).contains(&help));
            let json = d.to_json(&output.sources);
            assert!(json["notes"].as_array().unwrap().contains(&json!(help)));
            assert_eq!(json["primary"]["start"], start);
            assert_eq!(json["primary"]["end"], start + spelling.len());
            assert_eq!(json["fix"], Value::Null);
            assert_eq!(
                json_diagnostics(&output),
                json_diagnostics(&checked(&mut program))
            );
            let mut repaired = source.to_owned();
            repaired.replace_range(start..start + spelling.len(), variant);
            assert_eq!(repaired, twin);
            accepted(dir.path(), twin);
        }
    });
}

#[test]
fn unsupported_paths_keep_ordinary_diagnostics_and_function_typos() {
    with_compiler_stack(|| {
        for source in [
            "fn f() { let _ = model.Timeout }\n",
            "fn f() { let _ = model.Present(1u64) }\n",
            "fn f() -> model.Outcome { wrong.Timeout }\n",
            "fn f() -> model.Outcome { model.NoSuchVariant }\n",
            "fn f() -> model.Outcome { witness@2/model.Timeout }\n",
        ] {
            let dir = tempfile::tempdir().unwrap();
            fixture(dir.path(), MODEL, source, false, false);
            let output = checked(&mut load_program(&dir.path().join("hirsel.json")).unwrap());
            let [d] = &output.diagnostics[..] else {
                panic!("{:?}", output.diagnostics)
            };
            assert_eq!(d.rule.as_str(), "LANG-4");
            assert!(
                !d.notes
                    .iter()
                    .any(|n| n.contains("variants are written bare"))
            );
            assert!(d.fix.is_none());
        }
        let dir = tempfile::tempdir().unwrap();
        let source = "fn f() -> u64 { model.finsh() }\n";
        fixture(dir.path(), MODEL, source, false, false);
        let output = checked(&mut load_program(&dir.path().join("hirsel.json")).unwrap());
        let [d] = &output.diagnostics[..] else {
            panic!("{:?}", output.diagnostics)
        };
        assert_eq!(d.rule.as_str(), "LANG-4");
        assert!(
            !d.notes
                .iter()
                .any(|n| n.contains("variants are written bare"))
        );
        let fix = d.fix.as_ref().expect("ordinary function typo repair");
        assert_eq!(output.sources.slice(fix.span), Some("finsh"));
        let span = output.sources.resolve(fix.span);
        let mut repaired = source.to_owned();
        repaired.replace_range(span.start as usize..span.end as usize, &fix.replacement);
        assert_eq!(repaired, "fn f() -> u64 { model.finish() }\n");
        accepted(dir.path(), &repaired);
        // A qualified function is legal even when an expected enum has this arm.
        fixture(
            dir.path(),
            &format!("{MODEL}pub fn Timeout() -> Outcome {{ Done }}\n"),
            "fn f() -> model.Outcome { model.Timeout() }\n",
            false,
            false,
        );
        accepted(dir.path(), "fn f() -> model.Outcome { model.Timeout() }\n");
    });
}

#[test]
fn dependency_aliases_respect_visibility_and_module_ambiguity() {
    with_compiler_stack(|| {
        let dir = tempfile::tempdir().unwrap();
        let source = "fn f() -> vendor.Outcome { vendor.Timeout }\n";
        fixture(dir.path(), MODEL, source, true, false);
        let output = checked(&mut load_program(&dir.path().join("hirsel.json")).unwrap());
        let [d] = &output.diagnostics[..] else {
            panic!("{:?}", output.diagnostics)
        };
        assert_eq!(d.rule.as_str(), "LANG-4");
        assert_eq!(output.sources.slice(d.primary), Some("vendor.Timeout"));
        assert!(
            d.message
                .starts_with("unresolved module member `vendor.Timeout`")
        );
        assert!(d.notes.iter().any(|n| n.contains("`dep@1/model/Outcome`")));
        assert!(d.fix.is_none());
        accepted(dir.path(), "fn f() -> vendor.Outcome { Timeout }\n");
        for model in [
            "type Outcome = Timeout | Done\n",
            "pub opaque type Outcome: data uses {} = Timeout | Done\n",
        ] {
            fixture(dir.path(), model, source, true, false);
            let output = checked(&mut load_program(&dir.path().join("hirsel.json")).unwrap());
            assert!(!output.diagnostics.is_empty());
            for d in &output.diagnostics {
                assert!(
                    !d.notes
                        .iter()
                        .any(|n| n.contains("variants are written bare"))
                );
                assert!(d.fix.is_none());
            }
            assert!(
                output
                    .diagnostics
                    .iter()
                    .any(|d| d.rule.as_str() == "PROG-2")
                    || model.contains("opaque")
            );
        }
        let source = "fn f() -> dep@1/model.Outcome { vendor.Timeout }\n";
        fixture(dir.path(), MODEL, source, true, true);
        let output = checked(&mut load_program(&dir.path().join("hirsel.json")).unwrap());
        let [d] = &output.diagnostics[..] else {
            panic!("{:?}", output.diagnostics)
        };
        assert_eq!(d.rule.as_str(), "LANG-4");
        assert!(d.message.starts_with("ambiguous module head"));
        assert_eq!(output.sources.slice(d.primary), Some("vendor.Timeout"));
        assert!(
            !d.notes
                .iter()
                .any(|n| n.contains("variants are written bare"))
        );
        assert!(d.fix.is_none());
    });
}

#[test]
fn cached_help_tracks_header_changes_and_relocated_utf8_source() {
    with_compiler_stack(|| {
        let dir = tempfile::tempdir().unwrap();
        let source = "fn f() -> model.Outcome { model.Timeout }\n";
        fixture(dir.path(), MODEL, source, false, false);
        let mut program = load_program(&dir.path().join("hirsel.json")).unwrap();
        let first = checked(&mut program);
        assert!(
            first.diagnostics[0]
                .notes
                .iter()
                .any(|n| n.contains("variants are written bare"))
        );
        let main = dir.path().join("main.hir");
        let shifted = format!("// déplacé 🐑\n\n{source}");
        let file = program.engine.sources.add(&main, shifted.clone()).unwrap();
        let (known, context) = program
            .files
            .iter_mut()
            .find(|(_, context)| {
                context.package == program.entry.0.package && context.modules.is_empty()
            })
            .unwrap();
        *known = file;
        let context = context.clone();
        program.primary = file;
        eprintln!("VARIANT_GUIDANCE_48_EDIT {}", json!({"main.hir": shifted}));
        program.engine.parse_file(file, &context).unwrap();
        let moved = checked(&mut program);
        assert_eq!(first.diagnostics[0].notes, moved.diagnostics[0].notes);
        assert_eq!(
            moved.sources.slice(moved.diagnostics[0].primary),
            Some("model.Timeout")
        );
        assert_eq!(
            moved.sources.resolve(moved.diagnostics[0].primary).start as usize,
            shifted.find("model.Timeout").unwrap()
        );
        let model = dir.path().join("model.hir");
        let file = program
            .engine
            .sources
            .add(&model, MODEL.replace("Timeout", "Expired"))
            .unwrap();
        let (known, context) = program
            .files
            .iter_mut()
            .find(|(_, context)| {
                context.package == program.entry.0.package && context.modules == ["model"]
            })
            .unwrap();
        *known = file;
        let context = context.clone();
        eprintln!(
            "VARIANT_GUIDANCE_48_EDIT {}",
            json!({"model.hir": MODEL.replace("Timeout", "Expired")})
        );
        program.engine.parse_file(file, &context).unwrap();
        let changed = checked(&mut program);
        assert!(
            !changed.diagnostics[0]
                .notes
                .iter()
                .any(|n| n.contains("variants are written bare"))
        );
        assert!(changed.diagnostics[0].fix.is_none());
    });
}

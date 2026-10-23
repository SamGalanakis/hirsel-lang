//! Typed capability diagnostics use H's parameter association, never source order.
use hirsel_driver::*;
use hirsel_engine::*;

fn checked(source: &str) -> CheckOutput {
    let mut engine = crate::source_std::engine();
    let file = engine.sources.add("output.hir", source.into()).unwrap();
    let output = check_progress(&mut engine, file, &default_context()).unwrap();
    assert_eq!(output.completion, CheckCompletion::Complete);
    output
}

#[test]
fn output_capability_guidance_24_expected_supplied_and_omitted() {
    with_compiler_stack(|| {
        for (operation, expected, wrong) in [
            ("out.print", "Stdout", "Stderr"),
            ("err.print", "Stderr", "Stdout"),
        ] {
            for (argument, actual) in [("sink, ", Some(wrong)), ("1u64, ", Some("u64")), ("", None)]
            {
                let call = format!("{operation}({argument}\"x\")");
                let source = format!("fn emit(sink:{wrong}){{let _={call}}}\npub fn main(){{}}");
                let output = checked(&source);
                let [d] = &output.diagnostics[..] else {
                    panic!("{source}\n{:?}", output.diagnostics)
                };
                assert_eq!(d.rule.as_str(), "CAP-1");
                assert_eq!(output.sources.slice(d.primary), Some(call.as_str()));
                assert!(d.message.contains(expected), "{d:?}");
                assert!(d.message.contains("`s`"), "{d:?}");
                if let Some(actual) = actual {
                    assert!(d.message.contains(actual), "{d:?}");
                    let label = d
                        .secondary
                        .iter()
                        .find(|l| {
                            output.sources.slice(l.span) == Some(argument.trim_end_matches(", "))
                        })
                        .unwrap_or_else(|| panic!("{d:?}"));
                    assert!(label.message.contains(actual));
                    assert!(label.message.contains(expected));
                    assert!(label.message.contains(&format!("`s: {expected}`")));
                } else {
                    assert!(d.message.contains("omitted"), "{d:?}");
                    assert!(d.secondary.is_empty());
                }
                assert_eq!(
                    d.notes
                        .iter()
                        .any(|n| n.contains(if operation == "out.print" {
                            "err.print"
                        } else {
                            "out.print"
                        })),
                    actual == Some(wrong)
                );
                assert!(d.fix.is_none());
                let human = d.render_human(&output.sources);
                let json = d.to_json(&output.sources);
                assert_eq!(json["message"], d.message);
                for label in &d.secondary {
                    assert!(human.contains(&label.message));
                }
            }
        }
    });
}

#[test]
fn output_capability_guidance_24_labelled_user_calls_and_repairs_export() {
    with_compiler_stack(|| {
        // Capability is second in the declaration but first in the written call.
        let header =
            "fn emit(text message:String, via sink:Stdout){let _=out.print(sink,message)}\n";
        for (args, actual) in [
            ("via: wrong, text: \"x\"", Some("Stderr")),
            ("text: \"x\", via: 1u64", Some("u64")),
            ("text: \"x\"", None),
        ] {
            let source =
                format!("{header}fn probe(wrong:Stderr){{emit({args})}}\npub fn main(){{}}");
            let output = checked(&source);
            let [d] = &output.diagnostics[..] else {
                panic!("{source}\n{:?}", output.diagnostics)
            };
            assert_eq!(d.rule.as_str(), "CAP-1");
            assert!(
                d.message.contains("Stdout") && d.message.contains("`sink`"),
                "{d:?}"
            );
            if let Some(actual) = actual {
                assert!(
                    d.secondary.iter().any(|l| output.sources.slice(l.span)
                        == Some(if actual == "Stderr" { "wrong" } else { "1u64" })
                        && l.message.contains(actual)),
                    "{d:?}"
                );
            } else {
                assert!(d.message.contains("omitted"));
            }
            assert!(d.notes.iter().all(|n| !n.contains("err.print")), "{d:?}");
        }
        for body in [
            "let Ok(sink)=io.stdout(rt) else{return Error(())};let _=out.print(sink,\"x\")",
            "let Ok(sink)=io.stderr(rt) else{return Error(())};let _=err.print(sink,\"x\")",
            "let Ok(sink)=io.stdout(rt) else{return Error(())};emit(via:sink,text:\"x\")",
        ] {
            let source = format!("{header}pub fn main(rt:Io)->Result[(),()]{{{body};Ok(())}}");
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("main.hir");
            std::fs::write(&path, &source).unwrap();
            let mut program = load_program(&path).unwrap();
            program
                .check_for_execution()
                .unwrap_or_else(|e| panic!("{source}\n{e}"));
            let input = program.engine.export_native_input().unwrap();
            program.engine.import_native_input(input).unwrap();
        }
    });
}

#[test]
fn output_capability_guidance_24_identity_and_authority_controls() {
    with_compiler_stack(|| {
        for (source, rule) in [
            (
                "fn emit(s:Stdout) uses {} {let _=out.print(s,\"x\")}\npub fn main(){}",
                "CAP-1",
            ),
            (
                "fn emit(a:Dir,b:Dir,mut f:File of a){let mut buffer:List[u8]=[];let _=file.read(b,mut f,mut buffer,1u64)}\npub fn main(){}",
                "PROV-1",
            ),
            (
                "fn need(s:Stdout,n:u64){}\nfn emit() uses {Stdout}{need()}\npub fn main(){}",
                "LANG-12",
            ),
        ] {
            let output = checked(source);
            assert_eq!(
                output.diagnostics.first().unwrap().rule.as_str(),
                rule,
                "{source}\n{:?}",
                output.diagnostics
            );
        }
        // Identical local spelling cannot impersonate core's capability identity.
        let output = checked(
            "type Stdout {value:u64}\nfn emit(s:Stdout){let _=out.print(s,\"x\")}\npub fn main(){}",
        );
        let d = output.diagnostics.first().unwrap();
        assert_eq!(d.rule.as_str(), "CAP-1");
        assert!(d.message.contains("core@1/Stdout"), "{d:?}");
        assert!(d.notes.iter().all(|n| !n.contains("err.print")));
    });
}

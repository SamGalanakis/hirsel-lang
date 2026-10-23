//! MEM-5 cleanup reports source obligations rather than internal effect rows.
use hirsel_driver::*;
use hirsel_engine::*;

fn check(source: &str) -> (QueryEngine, CheckOutput) {
    let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
    let file = engine.sources.add("cleanup.hir", source.into()).unwrap();
    let output = check_progress(&mut engine, file, &default_context()).unwrap();
    assert_eq!(output.completion, CheckCompletion::Complete);
    (engine, output)
}

const RETAINED: &str = "@noalloc fn relay[R](read:@noalloc fn()->R uses {}, divisor:u64)->R uses {}{let output=read();let _=1u64/divisor;take output}\npub fn main(){}";

fn assert_source_diagnostic(output: &CheckOutput, parameter: &str) {
    let [d] = &output.diagnostics[..] else {
        panic!("{:?}", output.diagnostics)
    };
    assert_eq!(d.rule.as_str(), "MEM-5");
    assert_eq!(output.sources.slice(d.primary), Some("output"));
    assert!(
        d.fix.is_none(),
        "a bound cannot be added without caller proof"
    );
    let human = d.render_human(&output.sources);
    let json = d.to_json(&output.sources);
    eprintln!("{human}\n{json}");
    for internal in [
        "DependsOn",
        "TypeParamId",
        "SiteRef",
        "LexicalOwner",
        "MayAllocate",
    ] {
        assert!(!human.contains(internal), "{human}");
        assert!(!json.to_string().contains(internal), "{json}");
    }
    assert!(
        d.notes
            .iter()
            .any(|n| n.contains(&format!("`{parameter}: noalloc_drop`"))),
        "{d:?}"
    );
    assert!(
        d.notes.iter().any(|n| n.contains("`output` is owned here")),
        "{d:?}"
    );
    assert!(
        d.notes
            .iter()
            .any(|n| n.contains("every caller") && n.contains("opaque supplier")),
        "{d:?}"
    );
    assert!(
        d.secondary
            .iter()
            .any(|l| output.sources.slice(l.span) == Some("1u64/divisor")
                && l.message.contains("failure cleanup")),
        "{d:?}"
    );
    for note in &d.notes {
        assert!(human.contains(note));
        assert!(json["notes"].as_array().unwrap().iter().any(|n| n == note));
    }
    for label in &d.secondary {
        assert!(human.contains(&label.message));
        assert!(
            json["secondary"]
                .as_array()
                .unwrap()
                .iter()
                .any(|l| l["message"] == label.message)
        );
    }
}

#[test]
fn noalloc_cleanup_repairs_21_names_retained_result_and_missing_promise() {
    with_compiler_stack(|| {
        for source in [
            RETAINED.to_string(),
            RETAINED
                .replace("[R]", "[Payload]")
                .replace("->R", "->Payload"),
        ] {
            let (_, output) = check(&source);
            assert_source_diagnostic(
                &output,
                if source.contains("Payload") {
                    "Payload"
                } else {
                    "R"
                },
            );
        }
    });
}

#[test]
fn noalloc_cleanup_repairs_21_alias_and_nested_fields_keep_declaring_parameter() {
    with_compiler_stack(|| {
        for declaration in [
            "type Alias[T]=T\n",
            "type Wrapped[T] { payload:(T,) }\ntype Alias[T]=Wrapped[T]\n",
        ] {
            let source = format!("{declaration}{}", RETAINED.replace("->R", "->Alias[R]"));
            let (_, output) = check(&source);
            assert_source_diagnostic(&output, "R");
            // The transparent identity alias has the same accepted repair as R.
            // A nominal argument also checks exact cleanup-row equality; no
            // bound-only repair for that separate obligation is advertised.
            if !declaration.contains("Wrapped") {
                let caller = "@noalloc fn one()->Alias[u64] uses {} {7u64}\npub fn main() {let _=relay(one,1u64)}";
                accepted(
                    &source
                        .replace("relay[R]", "relay[R:noalloc_drop]")
                        .replace("pub fn main(){}", caller),
                );
            }
            assert!(
                !output.diagnostics[0]
                    .notes
                    .iter()
                    .any(|n| n.contains("`T: noalloc_drop`"))
            );
        }
        // A proof on the first parameter cannot discharge the second one.
        let source = RETAINED
            .replace("relay[R]", "relay[Proved:noalloc_drop,R]")
            .replace("divisor:u64", "other:Proved,divisor:u64");
        let (_, output) = check(&source);
        assert_source_diagnostic(&output, "R");
        assert!(
            !output.diagnostics[0]
                .notes
                .iter()
                .any(|n| n.contains("`Proved: noalloc_drop`"))
        );
    });
}

fn accepted(source: &str) {
    let (mut engine, output) = check(source);
    assert!(
        output.diagnostics.is_empty(),
        "{source}\n{:?}",
        output.diagnostics
    );
    let input = engine
        .export_native_input()
        .unwrap_or_else(|e| panic!("{source}\n{e:?}"));
    engine.import_native_input(input).unwrap();
    let mut relay = entry_path(&default_context());
    relay.0.name = "relay".into();
    let tir = engine.export_tir(&relay).unwrap();
    engine.import_tir(&tir).unwrap();
}

#[test]
fn noalloc_cleanup_repairs_21_complete_repairs_check_and_export_with_proved_callers() {
    with_compiler_stack(|| {
        let caller = "@noalloc fn one()->u64 uses {} {7u64}\npub fn main()->u64 {relay(one,1u64)}";
        // Strengthen only the helper header. The division and read order stay intact.
        accepted(
            &RETAINED
                .replace("[R]", "[R:noalloc_drop]")
                .replace("pub fn main(){}", caller),
        );
        accepted(
            &RETAINED
                .replace("[R]", "[R:plain]")
                .replace("pub fn main(){}", caller),
        );
        // Direct transfer is valid when no failure follows creation of the result.
        accepted(
            "@noalloc fn relay[R](read:@noalloc fn()->R uses {})->R uses {} {read()}\n@noalloc fn one()->u64 uses {} {7u64}\npub fn main()->u64 {relay(one)}",
        );
        // This keeps the division but changes its order relative to read(); it
        // is only an alternative when the caller permits that behavior change.
        accepted(
            &RETAINED
                .replace(
                    "let output=read();let _=1u64/divisor;take output",
                    "let _=1u64/divisor;read()",
                )
                .replace("pub fn main(){}", caller),
        );
    });
}

#[test]
fn noalloc_cleanup_repairs_21_allocating_nested_resource_and_unproved_supplier_reject() {
    with_compiler_stack(|| {
        let resource = "resource type Alloc { n:u64 } drop(x) {let _=box x.n}\ntype Inner { token:Alloc }\ntype Outer { inner:Inner }\n";
        let source = format!(
            "{resource}{}",
            RETAINED
                .replace("relay[R]", "relay")
                .replace("->R", "->Outer")
        );
        let (_, output) = check(&source);
        let [d] = &output.diagnostics[..] else {
            panic!("{:?}", output.diagnostics)
        };
        assert_eq!(d.rule.as_str(), "MEM-5");
        assert_eq!(output.sources.slice(d.primary), Some("output"));
        assert!(
            d.notes
                .iter()
                .any(|n| n == "destructor `Alloc` permits allocation"),
            "{d:?}"
        );
        assert!(!d.render_human(&output.sources).contains("MayAllocate"));
        assert!(
            d.secondary
                .iter()
                .any(|l| output.sources.slice(l.span) == Some("1u64/divisor"))
        );
        // A noalloc invocation promise does not prove the callback result's
        // cleanup. Adding a bound without strengthening this caller is invalid.
        let source = format!(
            "{}\nfn client[R](read:@noalloc fn()->R uses {{}})->R uses {{}} {{relay(read,1u64)}}",
            RETAINED.replace("[R]", "[R:noalloc_drop]")
        );
        let (_, output) = check(&source);
        assert!(
            output
                .diagnostics
                .iter()
                .any(|d| d.rule.as_str() == "GEN-1"),
            "{:?}",
            output.diagnostics
        );
    });
}

#[test]
fn noalloc_cleanup_repairs_21_borrow_only_client_needs_no_drop_bound() {
    with_compiler_stack(|| {
        accepted(
            "@noalloc fn inspect[R](value:R) uses {} {}\n@noalloc fn relay[R](value:R,divisor:u64)->u64 uses {} {inspect(value);1u64/divisor}\nresource type Alloc {n:u64} drop(x) {let _=box x.n}\npub fn main()->u64 {let value=Alloc(n:7);relay(value,1u64)}",
        );
    });
}

#[test]
fn noalloc_cleanup_repairs_21_opaque_supplier_must_export_the_bound() {
    with_compiler_stack(|| {
        for promised in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let bound = if promised { "noalloc_drop" } else { "()" };
            std::fs::write(dir.path().join("owner.hir"), format!("pub opaque type Secret: {bound} uses {{}} {{private_payload:u64}}\n@noalloc pub fn read()->Secret uses {{}} {{Secret(private_payload:7)}}")).unwrap();
            let client = RETAINED.replace("[R]", "[R:noalloc_drop]").replace(
                "pub fn main(){}",
                "pub fn main(){let _=relay(vendor.read,1u64)}",
            );
            std::fs::write(dir.path().join("client.hir"), client).unwrap();
            for (name, manifest) in [
                (
                    "owner.json",
                    serde_json::json!({"schema":1,"package":{"name":"owner","version":"1"},"sources":[{"path":"owner.hir","module":[]}],"dependencies":[]}),
                ),
                (
                    "client.json",
                    serde_json::json!({"schema":1,"package":{"name":"client","version":"1"},"entry":{"module":[],"name":"main"},"sources":[{"path":"client.hir","module":[]}],"dependencies":[{"manifest":"owner.json","package":{"name":"owner","version":"1"},"expose":[{"head":"vendor","module":[]}]}]}),
                ),
            ] {
                std::fs::write(dir.path().join(name), manifest.to_string()).unwrap();
            }
            let mut program = load_program(&dir.path().join("client.json")).unwrap();
            let output = program.check(LoweringPolicy::Reachable).unwrap();
            assert_eq!(output.completion, CheckCompletion::Complete);
            if promised {
                assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
                let input = program.engine.export_native_input().unwrap();
                program.engine.import_native_input(input).unwrap();
            } else {
                assert_eq!(output.diagnostics.len(), 1, "{:?}", output.diagnostics);
                assert_eq!(output.diagnostics[0].rule.as_str(), "GEN-1");
                assert!(output.diagnostics[0].fix.is_none());
                assert!(
                    !output.diagnostics[0]
                        .render_human(&output.sources)
                        .contains("private_payload")
                );
            }
        }
    });
}

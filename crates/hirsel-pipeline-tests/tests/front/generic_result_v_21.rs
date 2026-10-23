//! Callback results are owned, with binder-exact generic and mode receipts.
use hirsel_engine::{
    QueryEngine, check_snapshot, default_context, default_environment, entry_path,
};

fn checked(source: &str) -> QueryEngine {
    let mut engine = QueryEngine::new(default_environment(), hirsel_driver::compiler_adapters());
    let file = engine
        .sources
        .add("generic-result.hir", source.into())
        .unwrap();
    let diagnostics = check_snapshot(&mut engine, file, &default_context()).unwrap();
    assert!(diagnostics.is_empty(), "{source}\n{diagnostics:?}");
    engine
}

#[test]
fn generic_result_v_21_source_and_validation_agree() {
    let mut count = 0;
    for bound in [":plain", ":noalloc_drop", ""] {
        for (name, declaration, result, body) in [
            ("option", "", "Option[R]", "Some(read())"),
            (
                "variant",
                "type Packet[T]=Packet(T)\n",
                "Packet[R]",
                "Packet(read())",
            ),
            ("tuple", "", "(R,)", "(read(),)"),
            ("direct", "", "R", "read()"),
            (
                "retained",
                "",
                "R",
                "let output=read();let _=1u64/divisor;take output",
            ),
        ] {
            let noalloc =
                if name == "direct" || name == "retained" && !bound.is_empty() || bound == ":plain"
                {
                    "@noalloc "
                } else {
                    ""
                };
            let source = format!(
                "{declaration}{noalloc}fn wrap[R{bound}](read:@noalloc fn()->R uses {{}}, divisor:u64)->{result} uses {{}} {{{body}}}\npub fn main(){{}}"
            );
            let mut engine = checked(&source);
            let input = engine
                .export_native_input()
                .unwrap_or_else(|e| panic!("{source}\n{e:?}"));
            let mut signatures = hirsel_validate::SignatureMap::new(engine.types().clone());
            for signature in input.signatures.values() {
                signatures.insert(signature.definition, (**signature).clone());
            }
            let mut path = entry_path(&default_context());
            path.0.name = "wrap".into();
            let definition = input.definitions.get(&path).unwrap();
            hirsel_validate::assert_valid(definition, engine.types(), &signatures);
            count += 1;
        }
    }
    assert_eq!(count, 15);
    eprintln!("generic_result_v_21: {count} source/V transfers");
}

#[test]
fn generic_result_v_21_unbounded_retained_still_requires_cleanup_bound() {
    let source = "@noalloc fn wrap[R](read:@noalloc fn()->R uses {}, divisor:u64)->R uses {}{let output=read();let _=1u64/divisor;take output}";
    let mut engine = QueryEngine::new(default_environment(), hirsel_driver::compiler_adapters());
    let file = engine.sources.add("unbounded.hir", source.into()).unwrap();
    let ds = match check_snapshot(&mut engine, file, &default_context()) {
        Ok(ds) | Err(hirsel_types::QueryError::Diagnostics(ds)) => ds,
        Err(error) => panic!("{error:?}"),
    };
    assert_eq!(ds.len(), 1, "{ds:?}");
    assert_eq!(ds[0].rule.as_str(), "MEM-5");
    assert_eq!(engine.sources.slice(ds[0].primary).unwrap(), "output");
}

/// Rust core::option::map (option.rs:1150-1168) and the Some/None vectors
/// in coretests/tests/option.rs:103-121 at f6c1107f8be13e48465e12c0fd26e6a39fe18e4f.
/// MIT, The Rust Project Contributors. Hirsel callbacks return owned values;
/// neither Rust references nor FnOnce/Destruct trait rules are imported.
#[test]
fn generic_result_v_21_matches_reference_h_llvm_cranelift() {
    use hirsel_driver::native::*;
    use hirsel_engine::{RunConfiguration, run_observed_snapshot, with_compiler_stack};
    with_compiler_stack(|| {
        let mut options = Options::new(TargetSpec::x86_64_unknown_linux_gnu());
        options.validation = Validation::EveryStage;
        let matrix = vec![
            Configuration {
                options: options.clone(),
                engine: Engine::Heap { campaign: true },
            },
            Configuration {
                options: options.clone(),
                engine: Engine::Native {
                    opt: OptLevel::Speed,
                    partition: Partition::Whole,
                    runtime: Runtime::Diagnostic,
                },
            },
            Configuration {
                options,
                engine: Engine::Cranelift {
                    partition: Partition::Whole,
                    runtime: Runtime::Diagnostic,
                },
            },
        ];
        let directory =
            std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("generic-result-v-21");
        let toolchain = Toolchain::for_matrix(&directory.join("runtime"), &matrix).unwrap();
        let mut agreements = 0;
        for failure in [false, true] {
            let mut source = String::from("type Packet[T]=Packet(T)\n");
            for (suffix, bound) in [("plain", ":plain"), ("drop", ":noalloc_drop"), ("any", "")] {
                for (name, result, body) in [
                    ("option", "Option[R]", "Some(read())"),
                    ("variant", "Packet[R]", "Packet(read())"),
                    ("tuple", "(R,)", "(read(),)"),
                ] {
                    let noalloc = if bound == ":plain" { "@noalloc " } else { "" };
                    source.push_str(&format!("{noalloc}fn {name}_{suffix}[R{bound}](read:@noalloc fn()->R uses {{}})->{result} uses {{}} {{{body}}}\n"));
                }
            }
            source.push_str(
                r#"
@noalloc fn one()->u64 uses {} {1}
@noalloc fn relay[R:noalloc_drop](read:@noalloc fn()->R uses {}, divisor:u64)->R uses {} {
 let output=read()
 let _=1u64/divisor
 take output
}
resource type Token { n:u64 } @noalloc drop(x) {()}
resource type Other { n:u64 } drop(x) {()}
@noalloc fn token()->Token uses {} {Token(n:7)}
@noalloc fn other()->Other uses {} {Other(n:9)}
fn mapped[R](take value:Option[u64],read:@noalloc fn()->R uses {})->Option[R] uses {} {
 match take value {Some(_)=>Some(read()),None=>None}
}
pub fn main()->Result[(),u64] {
 if option_plain(one)!=Some(1u64) {return Error(1)}
 let Packet(p)=take variant_plain(one)
 let (t,)=take tuple_plain(one)
 if p!=1u64 || t!=1u64 {return Error(2)}
 if option_drop(one)!=Some(1u64) || option_any(one)!=Some(1u64) {return Error(3)}
 let Packet(d)=take variant_drop(one)
 let (e,)=take tuple_drop(one)
 let Packet(a)=take variant_any(one)
 let (b,)=take tuple_any(one)
 if d+e+a+b!=4u64 {return Error(4)}
 if mapped(Some(1u64),one)!=Some(1u64) || mapped(None,one)!=None {return Error(5)}
 let token_n=match take option_drop(token) {Some(v)=>v.n,None=>0u64}
 let Packet(token_d)=take variant_drop(token)
 let (token_e,)=take tuple_drop(token)
 if token_n+token_d.n+token_e.n!=21u64 {return Error(6)}
 let other_n=match take option_any(other) {Some(v)=>v.n,None=>0u64}
 let Packet(other_a)=take variant_any(other)
 let (other_b,)=take tuple_any(other)
 if other_n+other_a.n+other_b.n!=27u64 {return Error(7)}
"#,
            );
            // Unknown callback results retained across both success and failure.
            source.push_str(&format!(
                " let kept=relay(token,{}u64)\n if kept.n!=7u64 {{return Error(8)}}\n Ok(())\n}}",
                if failure { 0 } else { 1 }
            ));
            let mut engine = checked(&source);
            let entry = entry_path(&default_context());
            let run = run_observed_snapshot(
                &mut engine,
                &RunConfiguration {
                    entry: entry.clone(),
                    hash_seed: 0,
                    inputs: vec![],
                },
                Default::default(),
            )
            .unwrap();
            let reference = Observation::of_reference(run.observation.as_ref().unwrap());
            assert!(reference.output.is_empty());
            if failure {
                assert_eq!(reference.exit_code, None);
                assert_eq!(reference.failures.len(), 1);
                assert_eq!(reference.failures[0].rule, "SEM-1");
            } else {
                assert_eq!(reference.exit_code, Some(0));
                assert!(reference.failures.is_empty());
            }
            let input = engine.export_native_input().unwrap();
            let input = engine.import_native_input(input).unwrap();
            let name = if failure { "failure" } else { "return" };
            std::fs::create_dir_all(directory.join(name)).unwrap();
            let outcomes = differential(
                &input,
                &entry,
                &reference,
                &matrix,
                &toolchain,
                &directory.join(name),
            );
            assert_eq!(outcomes.len(), 3);
            for (configuration, outcome) in outcomes {
                assert!(
                    matches!(outcome, Outcome::Agree),
                    "{name} {configuration}: {outcome:?}"
                );
                agreements += 1;
            }
        }
        assert_eq!(agreements, 6);
        eprintln!(
            "generic_result_v_21: 2 reference outcomes; {agreements} H/LLVM/Cranelift agreements"
        );
    });
}

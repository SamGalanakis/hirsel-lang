//! LANG-8 compositions: nested success types, generic arm freezing, opacity,
//! and independent V rejection of forged propagation IR.
//! Vector structure: Rust f6c1107f8be13e48465e12c0fd26e6a39fe18e4f,
//! library/core/src/result.rs:2170-2200 and coretests/tests/result.rs:11-77
//! (MIT OR Apache-2.0). Preserve success/error payload observations; Hirsel's
//! nominal, unique, one-level arm rule replaces Rust's From conversion.
use hirsel_driver::*;
use hirsel_engine::*;
use hirsel_syntax::ContentHash;
use hirsel_tir::{Constructor, Projection, Rvalue};
use serde_json::json;

#[path = "../support/history.rs"]
#[allow(dead_code, reason = "shared runner also supports reduction histories")]
mod history;

const TYPES: &str = "type FileProblem = Missing(u64)\ntype OtherProblem = Other(u64)\n";

fn receipt(id: &str, phase: &str, source: &str, expected: &str) {
    eprintln!(
        "RESULT_INJECTION_TYPING_26 {}",
        json!({
            "id":id,"phase":phase,"source":source,"expected":expected,
            "source_hash":format!("{:?}",ContentHash::of_bytes(source.as_bytes())),
            "expected_hash":format!("{:?}",ContentHash::of_bytes(expected.as_bytes())),
            "seed":"20261026-result-injection-typing-26"
        })
    );
}

fn checked(source: &str) -> QueryEngine {
    let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
    let file = engine.sources.add("main.hir", source.into()).unwrap();
    let output = check_progress(&mut engine, file, &default_context()).unwrap();
    assert_eq!(output.completion, CheckCompletion::Complete);
    let errors: Vec<_> = output
        .diagnostics
        .iter()
        .filter(|d| d.severity == hirsel_diag::Severity::Error)
        .collect();
    assert!(errors.is_empty(), "{source}\n{errors:?}");
    engine
}
fn path(name: &str) -> hirsel_syntax::DefPath {
    let mut p = entry_path(&default_context());
    p.0.name = name.into();
    p
}

// Each pair adds a nested Result success context to the retained primitive,
// ambiguous, identity and one-level controls. Explicit arms are actual twins,
// including zero/two-field mappers that remain invalid when named.
fn local_case(index: usize, explicit: bool) {
    let (extra, error, target, arm, plain_ok, named_ok) = match index {
        0 => ("", "FileProblem", "FileProblem", "", true, true),
        1 => ("", "String", "String", "", true, true),
        2 => (
            "type App = Input(FileProblem)",
            "FileProblem",
            "App",
            "Input",
            true,
            true,
        ),
        3 => (
            "type App = Input(OtherProblem)",
            "FileProblem",
            "App",
            "Input",
            false,
            false,
        ),
        4 => (
            "type App = Input(FileProblem) | Backup(FileProblem)",
            "FileProblem",
            "App",
            "Backup",
            false,
            true,
        ),
        5 => (
            "type App = Input(FileProblem, u64)",
            "FileProblem",
            "App",
            "Input",
            false,
            false,
        ),
        6 => (
            "type Mid = Middle(FileProblem)\ntype App = Input(Mid)",
            "FileProblem",
            "App",
            "Input",
            false,
            false,
        ),
        7 => (
            "type Mid = Middle(FileProblem)\ntype Outer = Outside(Mid)\ntype App = Input(Outer)",
            "FileProblem",
            "App",
            "Input",
            false,
            false,
        ),
        8 => ("type App = Input(u64)", "u64", "App", "Input", false, true),
        9 => (
            "type App = Input(String)",
            "String",
            "App",
            "Input",
            false,
            true,
        ),
        10 => (
            "type App = Input(Bytes)",
            "Bytes",
            "App",
            "Input",
            false,
            true,
        ),
        11 => (
            "type App = Input((u64, u64))",
            "(u64, u64)",
            "App",
            "Input",
            false,
            true,
        ),
        12 => (
            "type App = Input(fn() -> u64 uses {})",
            "fn() -> u64 uses {}",
            "App",
            "Input",
            false,
            true,
        ),
        13 => (
            "type App = Input(Never)",
            "Never",
            "App",
            "Input",
            false,
            true,
        ),
        14 => (
            "type Alias = FileProblem\ntype App = Input(Alias)",
            "FileProblem",
            "App",
            "Input",
            true,
            true,
        ),
        15 => (
            "type Distinct = Missing(u64)\ntype App = Input(Distinct)",
            "FileProblem",
            "App",
            "Input",
            false,
            false,
        ),
        _ => unreachable!(),
    };
    let mapper = if explicit {
        if arm.is_empty() {
            " keep".into()
        } else {
            format!(" {arm}")
        }
    } else {
        String::new()
    };
    let keep = if explicit && arm.is_empty() {
        format!("fn keep(take e: {error}) -> {error} {{ take e }}\n")
    } else {
        String::new()
    };
    let owned = matches!(index, 1 | 9 | 10 | 12);
    let passing = if owned { "take " } else { "" };
    let value = if owned { "(take v)" } else { "v" };
    let source = format!(
        "{TYPES}{extra}\n{keep}fn probe({passing}v: Result[Result[u64, OtherProblem], {error}]) -> Result[Result[u64, OtherProblem], {target}] {{ let n: Result[u64, OtherProblem] = {value}?{mapper}\nOk(n) }}\npub fn main() {{}}\n"
    );
    let id = format!("S{:02}", index * 2 + usize::from(explicit));
    let ok = if explicit { named_ok } else { plain_ok };
    receipt(
        &id,
        "started-check",
        &source,
        if ok { "accept+V" } else { "LANG-8" },
    );
    let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
    let file = engine.sources.add("main.hir", source.clone()).unwrap();
    let output = check_progress(&mut engine, file, &default_context()).unwrap();
    assert_eq!(output.completion, CheckCompletion::Complete);
    let errors: Vec<_> = output
        .diagnostics
        .iter()
        .filter(|d| d.severity == hirsel_diag::Severity::Error)
        .collect();
    if ok {
        assert!(errors.is_empty(), "{id}: {source}\n{errors:?}");
        receipt(&id, "pass-check", &source, "accept");
        engine
            .lower_to_tir(&path("probe"))
            .unwrap_or_else(|e| panic!("{id}: V: {e:?}"));
        receipt(&id, "pass-V", &source, "accept");
    } else {
        assert_eq!(errors.len(), 1, "{id}: {errors:?}");
        assert_eq!(errors[0].rule.as_str(), "LANG-8", "{id}: {errors:?}");
        receipt(&id, "pass-check", &source, "LANG-8");
    }
}

#[test]
fn static_local_00() {
    with_compiler_stack(|| {
        for named in [false, true] {
            local_case(0, named);
        }
    });
}
#[test]
fn static_local_01() {
    with_compiler_stack(|| {
        for named in [false, true] {
            local_case(1, named);
        }
    });
}
#[test]
fn static_local_02() {
    with_compiler_stack(|| {
        for named in [false, true] {
            local_case(2, named);
        }
    });
}
#[test]
fn static_local_03() {
    with_compiler_stack(|| {
        for named in [false, true] {
            local_case(3, named);
        }
    });
}
#[test]
fn static_local_04() {
    with_compiler_stack(|| {
        for named in [false, true] {
            local_case(4, named);
        }
    });
}
#[test]
fn static_local_05() {
    with_compiler_stack(|| {
        for named in [false, true] {
            local_case(5, named);
        }
    });
}
#[test]
fn static_local_06() {
    with_compiler_stack(|| {
        for named in [false, true] {
            local_case(6, named);
        }
    });
}
#[test]
fn static_local_07() {
    with_compiler_stack(|| {
        for named in [false, true] {
            local_case(7, named);
        }
    });
}
#[test]
fn static_local_08() {
    with_compiler_stack(|| {
        for named in [false, true] {
            local_case(8, named);
        }
    });
}
#[test]
fn static_local_09() {
    with_compiler_stack(|| {
        for named in [false, true] {
            local_case(9, named);
        }
    });
}
#[test]
fn static_local_10() {
    with_compiler_stack(|| {
        for named in [false, true] {
            local_case(10, named);
        }
    });
}
#[test]
fn static_local_11() {
    with_compiler_stack(|| {
        for named in [false, true] {
            local_case(11, named);
        }
    });
}
#[test]
fn static_local_12() {
    with_compiler_stack(|| {
        for named in [false, true] {
            local_case(12, named);
        }
    });
}
#[test]
fn static_local_13() {
    with_compiler_stack(|| {
        for named in [false, true] {
            local_case(13, named);
        }
    });
}
#[test]
fn static_local_14() {
    with_compiler_stack(|| {
        for named in [false, true] {
            local_case(14, named);
        }
    });
}
#[test]
fn static_local_15() {
    with_compiler_stack(|| {
        for named in [false, true] {
            local_case(15, named);
        }
    });
}

fn generic(reverse: bool, last: bool, explicit: bool, main: &str) -> String {
    let arms = if last {
        "Files(FileProblem) | Source(E)"
    } else {
        "Source(E) | Files(FileProblem)"
    };
    let mapper = if explicit { " Source" } else { "" };
    let calls = if reverse {
        "let vb: Result[Result[u64, OtherProblem], OtherProblem] = Error(Other(23))\nlet b = probe(take vb)\nlet va: Result[Result[u64, OtherProblem], FileProblem] = Error(Missing(17))\nlet a = probe(take va)"
    } else {
        "let va: Result[Result[u64, OtherProblem], FileProblem] = Error(Missing(17))\nlet a = probe(take va)\nlet vb: Result[Result[u64, OtherProblem], OtherProblem] = Error(Other(23))\nlet b = probe(take vb)"
    };
    format!(
        "{TYPES}type Mixed[E] = {arms}\nfn probe[E](take v: Result[Result[u64, OtherProblem], E]) -> Result[Result[u64, OtherProblem], Mixed[E]] {{ let n: Result[u64, OtherProblem] = (take v)?{mapper}\nOk(n) }}\nfn nested[E](take v: Result[Result[u64, OtherProblem], E]) -> Result[Result[u64, OtherProblem], Mixed[E]] {{ Ok(probe(take v)?) }}\npub fn main() -> Result[(), ()] {{ {calls}\n{main}\nOk(()) }}\n"
    )
}

#[test]
fn static_generic_definition_freezes_both_arm_positions_and_orders() {
    with_compiler_stack(|| {
        for reverse in [false, true] {
            for last in [false, true] {
                for explicit in [false, true] {
                    let id = format!(
                        "S{}",
                        32 + usize::from(reverse) * 4
                            + usize::from(last) * 2
                            + usize::from(explicit)
                    );
                    let source = generic(reverse, last, explicit, "");
                    receipt(&id, "started-check", &source, "accept+V");
                    let mut engine = checked(&source);
                    receipt(&id, "pass-check", &source, "accept");
                    engine.export_native_input().unwrap();
                    receipt(&id, "pass-V", &source, "accept");
                }
            }
        }
    });
}

#[test]
fn static_visibility_is_package_nominal_and_not_structural() {
    with_compiler_stack(|| {
        for same in [false, true] {
            for opaque in [false, true] {
                for identity in [false, true] {
                    let id = format!(
                        "S{}",
                        40 + usize::from(same) * 4
                            + usize::from(opaque) * 2
                            + usize::from(identity)
                    );
                    let owner = format!(
                        "pub type FileProblem = Missing(u64)\npub {}type App: data uses {{}} = Input(FileProblem)\n",
                        if opaque { "opaque " } else { "" }
                    );
                    let target = if identity {
                        "api.FileProblem"
                    } else {
                        "api.App"
                    };
                    let source = format!(
                        "fn probe(v: Result[Result[u64, ()], api.FileProblem]) -> Result[Result[u64, ()], {target}] {{ let n: Result[u64, ()] = v?\nOk(n) }}\npub fn main() {{}}\n"
                    );
                    let dir = tempfile::tempdir().unwrap();
                    let owner_manifest = json!({"schema":1,"package":{"name":"owner","version":"1"},"sources":[{"path":"owner.hir","module":["api"]}],"dependencies":[]});
                    let sources = if same {
                        json!([{"path":"client.hir","module":[]},{"path":"owner.hir","module":["api"]}])
                    } else {
                        json!([{"path":"client.hir","module":[]}])
                    };
                    let deps = if same {
                        json!([])
                    } else {
                        json!([{"manifest":"owner.json","package":{"name":"owner","version":"1"},"expose":[{"head":"api","module":["api"]}]}])
                    };
                    let client_manifest = json!({"schema":1,"package":{"name":if same {"owner"} else {"client"},"version":"1"},"entry":{"module":[],"name":"main"},"sources":sources,"dependencies":deps});
                    for (name, text) in [
                        ("owner.hir", owner.clone()),
                        ("client.hir", source.clone()),
                        ("owner.json", owner_manifest.to_string()),
                        ("client.json", client_manifest.to_string()),
                    ] {
                        std::fs::write(dir.path().join(name), text).unwrap();
                    }
                    let ok = same || !opaque || identity;
                    let fixture = json!({"owner":owner,"client":source,"owner_manifest":owner_manifest,"client_manifest":client_manifest}).to_string();
                    receipt(
                        &id,
                        "started-check",
                        &fixture,
                        if ok { "accept+V" } else { "LANG-8" },
                    );
                    let mut program = load_program(&dir.path().join("client.json")).unwrap();
                    let output = program.check(LoweringPolicy::Comptime).unwrap();
                    assert_eq!(output.completion, CheckCompletion::Complete);
                    let errors: Vec<_> = output
                        .diagnostics
                        .iter()
                        .filter(|d| d.severity == hirsel_diag::Severity::Error)
                        .collect();
                    if ok {
                        assert!(errors.is_empty(), "{id}: {errors:?}");
                        receipt(&id, "pass-check", &fixture, "accept");
                        let mut p = path("probe");
                        p.0.package.name = if same { "owner" } else { "client" }.into();
                        p.0.package.version = "1".into();
                        program.engine.lower_to_tir(&p).unwrap();
                        receipt(&id, "pass-V", &fixture, "accept");
                    } else {
                        assert_eq!(errors.len(), 1, "{id}: {errors:?}");
                        assert_eq!(errors[0].rule.as_str(), "LANG-8");
                        receipt(&id, "pass-check", &fixture, "LANG-8");
                    }
                }
            }
        }
    });
}

fn hash(source: &str) -> String {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("main.hir");
    std::fs::write(&p, source).unwrap();
    hash_path(&p).unwrap()
}
#[test]
fn six_definition_hash_twins() {
    with_compiler_stack(|| {
        for reverse in [false, true] {
            for last in [false, true] {
                let plain = generic(reverse, last, false, "");
                let named = generic(reverse, last, true, "");
                assert_eq!(hash(&plain), hash(&named));
                receipt(
                    &format!("H{}", usize::from(reverse) * 2 + usize::from(last)),
                    "pass-hash",
                    &plain,
                    &named,
                );
            }
        }
        for prefix in [
            "type Alias = FileProblem\ntype App = Input(Alias)",
            "type App = Noise(OtherProblem) | Input(FileProblem)",
        ] {
            let named = format!(
                "{TYPES}{prefix}\nfn probe(v: Result[Result[u64, OtherProblem], FileProblem]) -> Result[Result[u64, OtherProblem], App] {{ Ok(v? Input) }}\npub fn main() {{}}\n"
            );
            let plain = named.replace("? Input", "?");
            assert_eq!(hash(&plain), hash(&named));
            receipt(
                if prefix.contains("Alias") { "H4" } else { "H5" },
                "pass-hash",
                &plain,
                &named,
            );
        }
    });
}

fn runtime_case(id: usize) {
    let reverse = id & 1 != 0;
    let last = id & 2 != 0;
    let nested = id & 4 != 0;
    let call = if nested { "nested" } else { "probe" };
    let body = format!(
        r#"
if !(match a {{ Error(Source(Missing(n))) => n == 17, _ => false }}) {{ return Error(()) }}
if !(match b {{ Error(Source(Other(n))) => n == 23, _ => false }}) {{ return Error(()) }}
let good: Result[Result[u64, OtherProblem], FileProblem] = Ok(Ok(667))
if !(match {call}(take good) {{ Ok(Ok(n)) => n == 667, _ => false }}) {{ return Error(()) }}
let inner: Result[Result[u64, OtherProblem], FileProblem] = Ok(Error(Other(666)))
if !(match {call}(take inner) {{ Ok(Error(Other(n))) => n == 666, _ => false }}) {{ return Error(()) }}
let bad: Result[Result[u64, OtherProblem], FileProblem] = Error(Missing(31))
if !(match {call}(take bad) {{ Error(Source(Missing(n))) => n == 31, _ => false }}) {{ return Error(()) }}
"#
    );
    let source = generic(reverse, last, false, &body);
    let name = format!("R{id}");
    receipt(
        &name,
        "started-runtime",
        &source,
        "Ok; arms Source/Ok; payloads 17,23,667,666,31",
    );
    let scratch = std::path::Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("result-injection-typing-26-{name}"));
    let runner = history::Runner::new(&scratch).unwrap();
    let report = runner.run(&name, &source);
    // Each engine compares independently with this exact exit/payload oracle.
    assert_eq!(report.cells.len(), 4, "{report:?}");
    for cell in &report.cells {
        assert_eq!(
            cell.category,
            history::Category::Observation,
            "{name}: {cell:?}"
        );
        let o = cell.observation.as_ref().unwrap();
        assert_eq!(o.exit_code, Some(0), "{name}: {cell:?}");
        assert!(
            o.output.is_empty()
                && o.failures.is_empty()
                && !o.blocked
                && o.budget.is_none()
                && o.signaled.is_none(),
            "{name}: {cell:?}"
        );
        receipt(
            &name,
            &format!("pass-runtime-{}", cell.engine),
            &source,
            "Ok; arms Source/Ok; payloads 17,23,667,666,31",
        );
    }
}

#[test]
fn runtime_0() {
    runtime_case(0);
}

#[test]
fn runtime_1() {
    runtime_case(1);
}

#[test]
fn runtime_2() {
    runtime_case(2);
}

#[test]
fn runtime_3() {
    runtime_case(3);
}

#[test]
fn runtime_4() {
    runtime_case(4);
}

#[test]
fn runtime_5() {
    runtime_case(5);
}

#[test]
fn runtime_6() {
    runtime_case(6);
}

#[test]
fn runtime_7() {
    runtime_case(7);
}

// V sees only the edited TIR and declarations, never the original `?` spelling.
// Four single-field mutations on each of three distinct propagation sites.
fn mutation_case(shape: usize) {
    with_compiler_stack(|| {
        let (extra, error, arm) = match shape {
            0 => (
                "type App = Input(FileProblem) | Spare(OtherProblem)",
                "FileProblem",
                "App",
            ),
            1 => (
                "type App = Input(FileProblem) | Spare(OtherProblem)",
                "App",
                "App",
            ),
            _ => (
                "type Mid = Middle(FileProblem)\ntype App = Input(Mid) | Spare(OtherProblem)",
                "Mid",
                "App",
            ),
        };
        let source = format!(
            "{TYPES}{extra}\nfn probe(v: Result[Result[u64, OtherProblem], {error}]) -> Result[Result[u64, OtherProblem], {arm}] {{ Ok(v?) }}\npub fn main() {{}}\n"
        );
        let mut engine = checked(&source);
        let base = (*engine.lower_to_tir(&path("probe")).unwrap().value).clone();
        let input = engine.export_native_input().unwrap();
        let mut signatures = hirsel_validate::SignatureMap::new(input.types.clone());
        signatures.extend_signatures(input.signatures.values().map(|s| s.as_ref().clone()));
        let verdict = |d: &hirsel_tir::Definition| {
            hirsel_validate::validate(d, engine.types(), &signatures, &mut vec![])
        };
        assert!(verdict(&base).violations.is_empty());
        for field in 0..4 {
            let id = format!("V{}", shape * 4 + field);
            receipt(
                &id,
                "started-V",
                &source,
                "reject one-field forged propagation",
            );
            let mut edited = base.clone();
            let body = &mut edited.bodies[base.entry.0 as usize];
            if field < 2 {
                let place = body
                    .places
                    .iter_mut()
                    .find(|p| {
                        p.projection
                            .iter()
                            .any(|p| matches!(p, Projection::VariantField { .. }))
                    })
                    .unwrap();
                let index = place
                    .projection
                    .iter()
                    .position(|p| matches!(p, Projection::VariantField { .. }))
                    .unwrap();
                let projection = &mut place.projection[index];
                let Projection::VariantField {
                    variant,
                    field: payload,
                } = projection
                else {
                    unreachable!()
                };
                if field == 0 {
                    *variant = 999;
                } else {
                    *payload = 999;
                }
            } else {
                // The propagated Error wrapper is core.Result variant 1.
                let constructor = body
                    .rvalues
                    .iter_mut()
                    .find_map(|r| match r {
                        Rvalue::Construct {
                            constructor:
                                Constructor::Variant {
                                    definition,
                                    variant,
                                },
                            fields,
                            ..
                        } if definition.0.package.name_is("core")
                            && definition.0.name == "Result"
                            && *variant == 1 =>
                        {
                            Some((definition, variant, fields))
                        }
                        _ => None,
                    })
                    .expect("propagated Result error constructor");
                if field == 2 {
                    *constructor.1 = 0;
                } else {
                    constructor.2[0].field = 999;
                }
            }
            // No canonical digest/import guard can stand in for V's type check.
            let result = verdict(&edited);
            assert!(
                !result.violations.is_empty(),
                "{id}: forged propagation accepted"
            );
            eprintln!(
                "RESULT_INJECTION_TYPING_26_MUTATION {}",
                json!({"id":id,"before":format!("{base:?}"),"after":format!("{edited:?}"),"violations":format!("{:?}",result.violations)})
            );
            receipt(
                &id,
                "pass-V",
                &source,
                "reject one-field forged propagation",
            );
        }
    });
}
#[test]
fn validation_injected_error_plan() {
    mutation_case(0);
}
#[test]
fn validation_identity_error_plan() {
    mutation_case(1);
}
#[test]
fn validation_nested_nominal_error_plan() {
    mutation_case(2);
}

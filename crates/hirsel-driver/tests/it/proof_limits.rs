use hirsel_driver::*;
use hirsel_engine::*;

fn checked(source: String, name: &str) {
    let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
    let context = default_context();
    let file = engine.sources.add("proof-limits.hir", source).unwrap();
    engine.parse_file(file, &context).unwrap();
    let mut path = entry_path(&context);
    path.0.name = name.into();
    engine.lower_to_tir(&path).unwrap();
}

#[test]
fn a_record_with_129_distinct_nominal_fields_proves_plain() {
    with_compiler_stack(|| {
        for width in [128, 129] {
            let fields = (0..width)
                .map(|n| format!("f{n}: Leaf[Inline[u64, {}]],\n", n + 1))
                .collect::<String>();
            checked(
                format!(
                    "type Leaf[T] {{ value: T }}\ntype Wide {{\n{fields}}}\nfn require[T:plain](x: T) {{}}\nfn probe(x: Wide) {{ require(x) }}\n"
                ),
                "probe",
            );
        }
    });
}

#[test]
#[ignore = "full gate"]
fn full_gate_finite_200_level_nominal_graph_reaches_validated_ir() {
    with_compiler_stack(|| {
        let mut source = "type D0[T] { value: T }\n".to_string();
        for n in 1..200 {
            source.push_str(&format!("type D{n}[T] {{ value: D{}[T] }}\n", n - 1));
        }
        source.push_str("fn require[T:plain](x:T) {}\nfn probe(x:D199[u64]) { require(x) }\n");
        checked(source, "probe");
    });
}

#[test]
fn expanding_polymorphic_recursion_has_one_operational_outcome() {
    with_compiler_stack(|| {
        let source = "type Grow[T] { next: box Grow[List[T]] }\nfn require[T:copyable](x:T) {}\nfn probe(x:Grow[u32]) { require(x) }\n";
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
        let context = default_context();
        let file = engine.sources.add("nonregular.hir", source.into()).unwrap();
        engine.parse_file(file, &context).unwrap();
        let mut path = entry_path(&context);
        path.0.name = "probe".into();
        let actual = engine.lower_to_tir(&path).unwrap_err();
        assert_eq!(actual, hirsel_types::type_proof_limit());
        let report = hirsel_validate::Validation {
            violations: vec![hirsel_validate::Violation {
                body: None,
                block: None,
                instruction: None,
                kind: hirsel_validate::ViolationKind::TypeProofToolingLimit {
                    limit: hirsel_types::MAX_POLYMORPHIC_EXPANSIONS,
                },
            }],
        };
        assert_eq!(hirsel_validate::tooling_limit_error(&report), Some(actual));
    });
}

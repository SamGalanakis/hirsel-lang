//! Missing-arm rendering vectors adapted from rust-analyzer (MIT/Apache-2.0),
//! 3131751e, missing_match_arms.rs:80-157 and expr.rs:725-768. Reuse the
//! independent finite model; Hirsel guards never establish complete coverage.
#[allow(dead_code)]
#[path = "../../../hirsel-validate/tests/it/match_coverage_22/model.rs"]
mod model;
use hirsel_driver::*;
use hirsel_engine::*;

const PRODUCT: &str =
    "fn probe(x:(bool,bool))->u32 {match x {(true,_)=>1, (false,true)=>2}}\npub fn main() {}\n";
const GUARD: &str = "fn probe(x:bool)->u32 {match x {true if x=>1,false=>2}}\npub fn main() {}\n";

fn engine() -> QueryEngine {
    QueryEngine::new(default_environment(), compiler_adapters())
}
fn checked(source: &str) -> CheckOutput {
    let mut engine = engine();
    let file = engine.sources.add("missing.hir", source.into()).unwrap();
    let output = check_progress(&mut engine, file, &default_context()).unwrap();
    assert_eq!(output.completion, CheckCompletion::Complete);
    output
}
fn rejected(source: &str, expected: &[&str], more: usize) {
    let output = checked(source);
    let [d] = &output.diagnostics[..] else {
        panic!("{source}\n{:?}", output.diagnostics)
    };
    assert_eq!(d.rule.as_str(), "LANG-7");
    assert_eq!(d.message, "match does not cover every value");
    let start = source.find("match ").unwrap();
    let end = source[start..].find('}').unwrap() + start + 1;
    assert_eq!(output.sources.slice(d.primary), Some(&source[start..end]));
    assert!(d.secondary.is_empty());
    assert!(d.fix.is_none());
    let human = d.render_human(&output.sources);
    let json = d.to_json(&output.sources);
    assert_eq!(json["message"], d.message);
    assert_eq!(json["rule"], "LANG-7");
    let notes = json["notes"].as_array().unwrap();
    for note in notes {
        assert!(human.contains(note.as_str().unwrap()));
    }
    if expected.is_empty() {
        assert!(d.notes.is_empty(), "{source}\n{human}");
        assert!(human.contains("add arms for the missing values"));
    } else {
        let list = expected
            .iter()
            .map(|p| format!("`{p}`"))
            .collect::<Vec<_>>()
            .join(", ");
        assert_eq!(
            d.notes[0],
            format!(
                "help: Add unguarded arms for missing patterns: {list}. Guarded arms do not establish complete coverage."
            ),
            "{source}"
        );
        assert_eq!(d.notes.len(), 1 + usize::from(more > 0));
        if more > 0 {
            assert_eq!(
                d.notes[1],
                format!("{more} more missing patterns not shown.")
            );
        }
    }
    eprintln!(
        "MISSING_CASE_22 {}",
        serde_json::json!({"source":source,"diagnostic":json})
    );
}
fn accepted(source: &str) {
    let mut engine = engine();
    let file = engine.sources.add("repair.hir", source.into()).unwrap();
    let output = check_progress(&mut engine, file, &default_context()).unwrap();
    assert_eq!(output.completion, CheckCompletion::Complete);
    assert!(
        output.diagnostics.is_empty(),
        "{source}\n{:?}",
        output.diagnostics
    );
    // Every repair has reachable calls in main; validate its actual helper body.
    engine
        .lower_to_tir(&entry_path(&default_context()))
        .unwrap();
    let input = engine.export_native_input().unwrap();
    engine.import_native_input(input).unwrap();
    eprintln!(
        "MISSING_CASE_22_REPAIR {}",
        serde_json::json!({"source":source,"decision":"source+V"})
    );
}

#[test]
fn match_missing_case_22_retained_product_and_guard_human_json() {
    with_compiler_stack(|| {
        rejected(PRODUCT, &["(false, false)"], 0);
        rejected(GUARD, &["true"], 0);
        // Removing the guard entirely repairs coverage. Merely changing its
        // condition to true does not, nor does adding a guarded missing arm.
        rejected(&GUARD.replace("if x", "if true"), &["true"], 0);
        rejected(
            &PRODUCT.replace("=>2}", "=>2, (false,false) if true=>3}"),
            &["(false, false)"],
            0,
        );
        accepted(&PRODUCT.replace("=>2}", "=>2, (false,false)=>3}").replace("pub fn main() {}", "pub fn main() {probe((false,false));probe((false,true));probe((true,false));probe((true,true));}"));
        accepted(&GUARD.replace("false=>2}", "false=>2,true=>3}").replace(
            "pub fn main() {}",
            "pub fn main() {probe(false);probe(true);}",
        ));
    });
}

#[test]
fn match_missing_case_22_finite_model_proves_every_displayed_value_uncovered() {
    with_compiler_stack(|| {
        let mut cases = 0;
        let mut examples = 0;
        // New rendering assertions over 24 negative matrices and eight empty ones;
        // this does not repeat its unchanged accepted coverage/V campaign.
        let originals = model::cases();
        let negatives = originals.iter().filter(|c| !c.truth().exhaustive).cloned();
        let empty = originals
            .iter()
            .filter(|c| c.name.ends_with("matrix-0"))
            .cloned()
            .map(|mut c| {
                c.name.push_str("-empty");
                c.arms.clear();
                c
            });
        for case in negatives.chain(empty) {
            let output = checked(&case.source());
            let [d] = &output.diagnostics[..] else {
                panic!("{}: {:?}", case.name, output.diagnostics)
            };
            let examples_note = d.notes.first().expect("finite witness");
            let displayed: Vec<_> = examples_note.split('`').skip(1).step_by(2).collect();
            assert!(!displayed.is_empty() && displayed.len() <= 3);
            let uncovered: Vec<_> = case
                .domain
                .values()
                .into_iter()
                .filter(|value| {
                    let mut query = case.clone();
                    query.arms.push(model::Arm {
                        pattern: model::Pattern::Exact(value.clone()),
                        guard: None,
                    });
                    *query.truth().reachable_arms.last().unwrap()
                })
                .map(|value| value.source())
                .collect();
            assert_eq!(
                displayed
                    .iter()
                    .map(|p| p.replace(' ', ""))
                    .collect::<Vec<_>>(),
                uncovered.iter().take(3).cloned().collect::<Vec<_>>(),
                "{}",
                case.name
            );
            if uncovered.len() > 3 {
                assert_eq!(
                    d.notes[1],
                    format!("{} more missing patterns not shown.", uncovered.len() - 3)
                );
            } else {
                assert_eq!(d.notes.len(), 1);
            }
            for spelling in displayed {
                let value = case
                    .domain
                    .values()
                    .into_iter()
                    .find(|v| v.source() == spelling.replace(' ', ""))
                    .expect("concrete domain member");
                let mut query = case.clone();
                query.arms.push(model::Arm {
                    pattern: model::Pattern::Exact(value),
                    guard: None,
                });
                assert_eq!(
                    query.truth().reachable_arms.last(),
                    Some(&true),
                    "{}: {spelling}",
                    case.name
                );
                examples += 1;
            }
            cases += 1;
            eprintln!(
                "MISSING_CASE_22_MODEL {}",
                serde_json::json!({"id":case.name,"source":case.source(),"diagnostic":d.to_json(&output.sources)})
            );
        }
        assert_eq!((cases, examples), (32, 45));
    });
}

#[test]
fn match_missing_case_22_constructor_order_labels_and_three_example_cap() {
    with_compiler_stack(|| {
        rejected("fn probe(x:())->u32 {match x {}}", &["()"], 0);
        rejected(
            "fn probe(x:(bool,((),bool)))->u32 {match x {(true,_)=>1}}",
            &["(false, ((), false))", "(false, ((), true))"],
            0,
        );
        rejected(
            "type Choice = Z(bool) | A | M(left: bool, right: bool)\nfn probe(x:Choice)->u32 {match x {Z(true)=>1}}",
            &["Z(false)", "A", "M(left: false, right: false)"],
            3,
        );
        rejected(
            "type Row { a: bool, b: bool }\nfn probe(x:Row)->u32 {match x {Row(b: true, a: _)=>1}}",
            &["Row(a: false, b: false)", "Row(a: true, b: false)"],
            0,
        );
        rejected(
            "type Choice = A(bool) | B(bool)\nfn probe(x:Choice)->u32 {match x {A(false | true)=>1}}",
            &["B(false)", "B(true)"],
            0,
        );
        rejected(
            "fn probe(x:(bool,bool,bool))->u32 {match x {}}",
            &[
                "(false, false, false)",
                "(false, false, true)",
                "(false, true, false)",
            ],
            5,
        );
        accepted(
            "type Choice = Z(bool) | A | M(left: bool, right: bool)\nfn probe(x:Choice)->u32 {match x {Z(true)=>1,Z(false)=>2,A=>3,M(left:false,right:false)=>4,M(left:false,right:true)=>5,M(left:true,right:false)=>6,M(left:true,right:true)=>7}}\npub fn main(){probe(Z(false));probe(Z(true));probe(A);probe(M(left:false,right:false));probe(M(left:false,right:true));probe(M(left:true,right:false));probe(M(left:true,right:true));}",
        );
        accepted(
            "type Row { a: bool, b: bool }\nfn probe(x:Row)->u32 {match x {Row(b:true,a:_)=>1,Row(a:false,b:false)=>2,Row(a:true,b:false)=>3}}\npub fn main(){probe(Row(a:false,b:false));probe(Row(a:true,b:false));probe(Row(a:false,b:true));probe(Row(a:true,b:true));}",
        );
    });
}

#[test]
fn match_missing_case_22_uncertain_domains_keep_generic_advice() {
    with_compiler_stack(|| {
        rejected("fn probe(x:u32)->u32 {match x {1=>1}}", &[], 0);
        rejected("fn probe[T](x:T)->u32 {match x {}}", &[], 0);
        rejected(
            "type Chain = End | Link(box Chain)\nfn probe(x:Chain)->u32 {match x {End=>1}}",
            &[],
            0,
        );
        rejected(
            "fn probe(x:(bool,bool,bool,bool,bool,bool,bool,bool))->u32 {match x {}}",
            &[],
            0,
        );
        rejected("fn probe(x:box bool)->u32 {match x {true=>1}}", &[], 0);
    });
}

#[test]
fn match_missing_case_22_foreign_opaque_representation_stays_hidden() {
    with_compiler_stack(|| {
        let dir = tempfile::tempdir().unwrap();
        for (path, text) in [
            ("owner.hir", "pub opaque type Hidden: data uses {} = Secret(bool) | Other".to_owned()),
            ("client.hir", "fn probe(x:vendor.Hidden)->u32 {match x {}}\npub fn main(){}".to_owned()),
            ("owner.json", serde_json::json!({"schema":1,"package":{"name":"owner","version":"1"},"sources":[{"path":"owner.hir","module":[]}],"dependencies":[]}).to_string()),
            ("client.json", serde_json::json!({"schema":1,"package":{"name":"client","version":"1"},"sources":[{"path":"client.hir","module":[]}],"dependencies":[{"manifest":"owner.json","package":{"name":"owner","version":"1"},"expose":[{"head":"vendor","module":[]}]}],"entry":{"module":[],"name":"main"}}).to_string()),
        ] { std::fs::write(dir.path().join(path), text).unwrap(); }
        let mut program = load_program(&dir.path().join("client.json")).unwrap();
        let output = program.check(LoweringPolicy::Reachable).unwrap();
        assert_eq!(output.completion, CheckCompletion::Complete);
        let [d] = &output.diagnostics[..] else {
            panic!("{:?}", output.diagnostics)
        };
        assert_eq!(d.message, "match does not cover every value");
        assert!(d.notes.is_empty());
        let human = d.render_human(&output.sources);
        assert!(
            !human.contains("Secret") && !human.contains("Other"),
            "{human}"
        );
        eprintln!("MISSING_CASE_22_OPAQUE {}", d.to_json(&output.sources));
    });
}

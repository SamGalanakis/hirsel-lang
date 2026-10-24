use super::*;
use serde_json::{Value, json};
use std::path::PathBuf;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap()
}
fn card(manifest: &str, arguments: &[&str]) -> std::process::Output {
    let output = Command::new(env!("CARGO_BIN_EXE_hirsel"))
        .args(["deps", "card"])
        .arg(root().join(manifest))
        .args(arguments)
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    assert!(output.stderr.is_empty(), "{output:?}");
    output
}

#[test]
fn transitive_authority_card_matches_goldens_and_does_not_run_code() {
    let manifest = "packages/dep_card_fixture/app/hirsel.json";
    let text = card(manifest, &[]);
    assert_eq!(
        String::from_utf8(text.stdout).unwrap(),
        include_str!("dep_card/transitive.txt")
    );
    let report: Value = serde_json::from_slice(&card(manifest, &["--json"]).stdout).unwrap();
    let golden: Value = serde_json::from_str(include_str!("dep_card/transitive.json")).unwrap();
    assert_eq!(report, golden);
    let leaf = &report["cards"][0];
    let middle = &report["cards"][1];
    assert_eq!(leaf["package"], "leaf@1");
    assert_eq!(
        middle["weight"]["transitive_dependencies"],
        json!(["leaf@1"])
    );
    assert_eq!(
        middle["authority"]["reached_by_you"]["operations"],
        json!(["core@1/out/print"])
    );
    assert!(
        middle["authority"]["whole_public_api"]["operations"]
            .as_array()
            .unwrap()
            .contains(&json!("core@1/net/connect"))
    );
    assert_eq!(
        leaf["exposure"],
        json!([{"type":"leaf@1/leaf/Token","signatures":["app@1/expose"]}])
    );
    assert_eq!(leaf["health"]["tests"], 1);
    assert!(leaf["health"]["last_gate"].is_null());
    assert!(leaf["health"]["provenance"].is_null());
}

#[test]
fn std_json_dependency_card_matches_goldens() {
    // d2b8596c changed the JSON declaration graph; keep the current scan leaves
    // and ordinary product facts pinned alongside the unchanged authority oracle.
    let manifest = "packages/dep_card_fixture/json/hirsel.json";
    let text = card(manifest, &["--package", "std@1/json"]);
    assert_eq!(
        String::from_utf8(text.stdout).unwrap(),
        include_str!("dep_card/std-json.txt")
    );
    let report: Value =
        serde_json::from_slice(&card(manifest, &["--json", "--package", "std@1/json"]).stdout)
            .unwrap();
    let golden: Value = serde_json::from_str(include_str!("dep_card/std-json.json")).unwrap();
    assert_eq!(report, golden);
    assert_eq!(
        report["cards"][0]["authority"]["reached_by_you"]["capabilities"],
        json!([])
    );
    assert_eq!(report["cards"][0]["exposure"], json!([]));
}

#[test]
fn dependency_card_rejects_unknown_selectors_and_malformed_options() {
    for args in [
        vec!["deps"],
        vec!["deps", "tree"],
        vec!["deps", "card"],
        vec!["deps", "card", "--json", "--json", "missing.json"],
        vec!["deps", "card", "--package"],
        vec!["deps", "card", "--code-store"],
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_hirsel"))
            .args(args)
            .output()
            .unwrap();
        assert!(!output.status.success(), "{output:?}");
    }
    for selector in ["missing@1", "leaf", "leaf@2", "leaf@1/missing", "leaf@1/"] {
        let output = Command::new(env!("CARGO_BIN_EXE_hirsel"))
            .args(["deps", "card", "--package", selector, "--json"])
            .arg(root().join("packages/dep_card_fixture/app/hirsel.json"))
            .output()
            .unwrap();
        assert!(!output.status.success(), "{output:?}");
        assert!(!output.stdout.is_empty() || !output.stderr.is_empty());
    }
}

#[test]
fn library_roots_empty_dependencies_and_explicit_store_are_reported() {
    let report: Value = serde_json::from_slice(
        &card("packages/dep_card_fixture/leaf/hirsel.json", &["--json"]).stdout,
    )
    .unwrap();
    assert_eq!(report["reach_basis"], "public_api");
    assert_eq!(report["cards"], json!([]));
    let config = file(r#"{"provider":"code.none@1"}"#);
    let output = card(
        "packages/dep_card_fixture/app/hirsel.json",
        &[
            "--json",
            "--package",
            "leaf@1",
            "--code-store",
            config.to_str().unwrap(),
        ],
    );
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    let p = &report["cards"][0]["health"]["provenance"];
    assert_eq!(p["provider"], "code.none@1");
    assert!(!p["objects"].as_array().unwrap().is_empty());
    for object in p["objects"].as_array().unwrap() {
        assert_eq!(object["present"], false);
        assert!(object["signature_verified"].is_null());
        assert_eq!(object["hash"].as_str().unwrap().len(), 64);
    }
    std::fs::remove_file(config).unwrap();
}

// Validate live cards against the shipped schema, including exact members. The
// schema uses this small closed subset of JSON Schema; no runtime dependency is
// needed just to enforce the CLI's machine-readable contract.
fn conforms(schema: &Value, value: &Value, root: &Value) -> bool {
    if let Some(reference) = schema["$ref"].as_str() {
        return conforms(
            root.pointer(reference.strip_prefix('#').unwrap()).unwrap(),
            value,
            root,
        );
    }
    if let Some(options) = schema["anyOf"].as_array() {
        return options.iter().any(|s| conforms(s, value, root));
    }
    if let Some(options) = schema["enum"].as_array() {
        if !options.contains(value) {
            return false;
        }
    }
    if let Some(expected) = schema.get("const") {
        if expected != value {
            return false;
        }
    }
    match schema["type"].as_str() {
        Some("object") => {
            let Some(object) = value.as_object() else {
                return false;
            };
            if schema["required"]
                .as_array()
                .unwrap()
                .iter()
                .any(|k| !object.contains_key(k.as_str().unwrap()))
            {
                return false;
            }
            object.iter().all(|(k, v)| {
                schema["properties"]
                    .get(k)
                    .is_some_and(|s| conforms(s, v, root))
            })
        }
        Some("array") => value
            .as_array()
            .is_some_and(|a| a.iter().all(|v| conforms(&schema["items"], v, root))),
        Some("integer") => value
            .as_u64()
            .is_some_and(|n| n >= schema["minimum"].as_u64().unwrap_or(0)),
        Some("string") => value.as_str().is_some_and(|s| {
            schema.get("pattern").is_none()
                || (s.len() == 64
                    && s.bytes()
                        .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c)))
        }),
        Some("boolean") => value.is_boolean(),
        Some("null") => value.is_null(),
        None => true,
        other => panic!("unsupported schema type {other:?}"),
    }
}

#[test]
fn live_dependency_cards_conform_to_the_versioned_json_schema() {
    let schema: Value =
        serde_json::from_str(include_str!("../../../../docs/dependency-card.schema.json")).unwrap();
    for arguments in [vec!["--json"], vec!["--json", "--package", "middle@1"]] {
        let report: Value = serde_json::from_slice(
            &card("packages/dep_card_fixture/app/hirsel.json", &arguments).stdout,
        )
        .unwrap();
        assert!(conforms(&schema, &report, &schema), "{report}");
        let mut invalid = report;
        invalid["cards"][0]["health"]["last_gate"] = json!("passed");
        assert!(!conforms(&schema, &invalid, &schema));
    }
}

#[test]
fn opaque_representation_and_private_body_types_do_not_leak_into_exposure() {
    let source = file(
        "pub opaque type Hidden: inert uses {} { value: leaf.Token }\nfn private(value: leaf.Token) -> leaf.Token uses {} { value }\npub fn main() -> Result[(), ()] uses {} { Ok(()) }\n",
    );
    let manifest = source.with_extension("hir.json");
    std::fs::write(&manifest, json!({"schema":1,"package":{"name":"opaque_card","version":"1"},
        "sources":[{"path":source,"module":[]}],"entry":{"module":[],"name":"main"},
        "dependencies":[{"manifest":root().join("packages/dep_card_fixture/leaf/hirsel.json"),"package":{"name":"leaf","version":"1"},"expose":[]}]}).to_string()).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_hirsel"))
        .args(["deps", "card", "--json"])
        .arg(&manifest)
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["cards"][0]["exposure"], json!([]));
    assert_eq!(report["cards"][0]["weight"]["definitions"]["reached"], 0);
    assert_eq!(
        report["cards"][0]["authority"]["reached_by_you"]["capabilities"],
        json!([])
    );
    std::fs::remove_file(source).unwrap();
    std::fs::remove_file(manifest).unwrap();
}

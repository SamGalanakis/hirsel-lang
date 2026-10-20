//! Optional code-store evidence for the engine's computed dependency cards.
use hirsel_engine::{DefinitionQueries, ToolError};
use serde_json::{Value, json};
use std::{collections::BTreeSet, path::Path};

/// Inspect source/build facts, and optionally verify the public API closure's
/// objects in an explicitly selected code store. Never executes package tests.
pub fn cards(
    manifest: &Path,
    package: Option<&str>,
    store: Option<&Path>,
) -> Result<Value, ToolError> {
    let mut program = crate::COMPILER.load_test_package(manifest)?;
    let mut report = program.dependency_cards(manifest, package)?;
    let Some(config) = store else {
        return Ok(report);
    };
    let config: Value = serde_json::from_slice(&std::fs::read(config)?)
        .map_err(|e| ToolError::Runtime(format!("code store configuration: {e}")))?;
    let store = hirsel_code_store::config::open(&config).map_err(ToolError::Runtime)?;
    let wanted: BTreeSet<_> = report["cards"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|c| c["public_functions"].as_array().unwrap())
        .filter_map(Value::as_str)
        .collect();
    let mut roots = Vec::new();
    for (file, context) in program.files.clone() {
        let parsed = program
            .engine
            .parse_file(file, &context)
            .map_err(|e| program.error(e))?
            .value;
        for id in &parsed.roots {
            let path = &parsed.tree.items[id.0 as usize].definition;
            if wanted.contains(path.to_string().as_str()) {
                roots.push(path.clone());
            }
        }
    }
    let hashes = program
        .engine
        .definition_hashes(roots)
        .map_err(|e| program.error(e))?;
    for card in report["cards"].as_array_mut().unwrap() {
        let prefix = format!("{}/", card["package"].as_str().unwrap());
        let mut objects = Vec::new();
        for (path, hash) in &hashes.hashes {
            if !path.to_string().starts_with(&prefix) {
                continue;
            }
            let present = store
                .get(hash)
                .map_err(|e| ToolError::Runtime(format!("code store: {e:?}")))?
                .is_some();
            objects.push(json!({"definition":path.to_string(),"hash":hirsel_code_store::hex(hash),
                "present":present, "signature_verified": if present && store.signatures().trusted().is_some() { Some(true) } else { None }}));
        }
        card["health"]["provenance"] = json!({"provider":store.provider(), "signature_policy":store.signatures().provider(),
            "trusted_keys":store.signatures().trusted().unwrap_or_default().iter().map(|k| k.hex()).collect::<Vec<_>>(), "objects":objects});
        card["health"]["unavailable"]
            .as_object_mut()
            .unwrap()
            .remove("provenance");
    }
    Ok(report)
}

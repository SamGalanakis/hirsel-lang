//! Dependency allowlist: implementation crates
//! never depend on one another; only the driver wires them.
//!
//! Reads `cargo metadata`, classifies every workspace crate, and checks each
//! declared workspace edge by kind. Production and build edges must respect the
//! layering; dev edges (test-only) are reported and allowed.
use serde_json::Value;
use std::process::Command;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Class {
    /// Shared data crates: schemas, IR, protocols. They depend only on data crates.
    Data,
    /// Independent implementation crates (M1a phases and the M1b native crates).
    Implementation,
    /// Dependency-free utilities any layer may consume.
    LeafUtility,
    /// Shared pure leaves with no runtime representation or ambient inputs.
    LeafKernel,
    /// Shared persistent bytes, independent of compiler phase implementations.
    Storage,
    /// The driver layer: the engine and the driver, which wires implementation
    /// crates through adapters and alone depends on the engine.
    Wiring,
    /// The CLI and the conformance runner: never a compiler dependency.
    Tool,
}

const CRATES: &[(&str, Class)] = &[
    // Data rather than a leaf utility: its content keys are ContentHash values.
    ("hirsel-graph", Class::Data),
    ("hirsel-guard", Class::LeafUtility),
    // Shared protocol data and operations; debug sections use hirsel-guard.
    ("hirsel-free", Class::Data),
    ("hirsel-span", Class::Data),
    ("hirsel-intern", Class::Data),
    ("hirsel-diag", Class::Data),
    ("hirsel-syntax", Class::Data),
    ("hirsel-codec", Class::Data),
    ("hirsel-types", Class::Data),
    ("hirsel-artifact-store", Class::Storage),
    ("hirsel-product-codec", Class::Data),
    ("hirsel-type-analysis", Class::Data),
    ("hirsel-tir", Class::Data),
    ("hirsel-validate", Class::Data),
    ("hirsel-proof-export", Class::Data),
    ("hirsel-modes", Class::Data),
    ("hirsel-lir", Class::Data),
    ("hirsel-layout", Class::Data),
    ("hirsel-core-contract", Class::Data),
    ("hirsel-rt-operations", Class::Data),
    ("hirsel-run-config", Class::Data),
    ("hirsel-run-schema", Class::Data),
    ("hirsel-object-storage", Class::Data),
    ("hirsel-lexer", Class::Implementation),
    ("hirsel-parser", Class::Implementation),
    ("hirsel-fmt", Class::Implementation),
    ("hirsel-resolve", Class::Implementation),
    ("hirsel-check-protocol", Class::Data),
    ("hirsel-signature", Class::Implementation),
    ("hirsel-check", Class::Implementation),
    ("hirsel-lower", Class::Implementation),
    ("hirsel-authority", Class::Implementation),
    ("hirsel-ownership", Class::Implementation),
    ("hirsel-interp", Class::Implementation),
    ("hirsel-rc", Class::Implementation),
    ("hirsel-llvm", Class::Implementation),
    ("hirsel-cranelift", Class::Implementation),
    ("hirsel-rt", Class::Implementation),
    ("hirsel-heapcheck", Class::Implementation),
    ("hirsel-code-store", Class::Implementation),
    ("hirsel-remote", Class::Implementation),
    ("hirsel-kernels", Class::LeafKernel),
    ("hirsel-engine", Class::Wiring),
    ("hirsel-driver", Class::Wiring),
    ("hirsel-analysis-canary", Class::Tool),
    ("hirsel-cli", Class::Tool),
    ("hirsel-cli-args", Class::Tool),
    ("hirsel-conformance", Class::Tool),
    ("hirsel-native-build", Class::Tool),
    ("hirsel-pipeline-tests", Class::Tool),
    ("hirsel-free-loom", Class::Tool),
    ("hirsel-heaps-loom", Class::Tool),
];

/// The implementation crates the engine may use directly; it reaches
/// signature, check, modes and lower only through its `Phases` trait object.
const ENGINE_IMPLEMENTATIONS: &[&str] = &[
    "hirsel-interp",
    "hirsel-resolve",
    "hirsel-parser",
    "hirsel-lexer",
    "hirsel-fmt",
];

// Storage access is explicit; pure graph helpers and compiler phases cannot
// acquire ambient persistence through an unrestricted data-crate edge.
const STORAGE_CONSUMERS: &[&str] = &[
    "hirsel-driver",
    "hirsel-engine",
    "hirsel-lir",
    "hirsel-product-codec",
    "hirsel-validate",
];

/// Shared utilities and native data crates take no external production dependency.
const NO_EXTERNAL_PRODUCTION: &[&str] = &[
    "hirsel-graph",
    "hirsel-lir",
    "hirsel-layout",
    "hirsel-core-contract",
    "hirsel-codec",
];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Kind {
    Production,
    Dev,
    Build,
}

#[derive(Clone, Debug)]
struct EdgeRow {
    from: String,
    to: String,
    kind: Kind,
    workspace: bool,
}

fn class(name: &str) -> Option<Class> {
    CRATES.iter().find(|(n, _)| *n == name).map(|(_, c)| *c)
}

/// The rule for one edge between workspace crates; `Err` names the violation.
fn check_edge(from: &str, to: &str, kind: Kind) -> Result<(), String> {
    let (Some(source), Some(target)) = (class(from), class(to)) else {
        return Err(format!("unclassified crate in {from} -> {to}"));
    };
    if kind == Kind::Dev {
        return Ok(());
    }
    let allowed = match (source, target) {
        _ if matches!(from, "hirsel-lir" | "hirsel-codec")
            && matches!(to, "hirsel-types" | "hirsel-tir" | "hirsel-layout") =>
        {
            false
        }
        (Class::LeafUtility, _) => false,
        (_, Class::LeafUtility) => true,
        (Class::LeafKernel, Class::Data) => to == "hirsel-core-contract",
        (_, Class::LeafKernel) => {
            matches!(from, "hirsel-rt" | "hirsel-interp" | "hirsel-heapcheck")
        }
        (Class::LeafKernel, _) => false,
        (_, Class::Storage) => STORAGE_CONSUMERS.contains(&from),
        (_, Class::Data) => true,
        (Class::Wiring, Class::Implementation) => {
            from != "hirsel-engine" || ENGINE_IMPLEMENTATIONS.contains(&to)
        }
        (Class::Tool, Class::Wiring | Class::Implementation) => true,
        (Class::Data | Class::Storage, _) => false,
        (Class::Implementation, _) => false,
        (Class::Tool, Class::Tool) => to == "hirsel-cli-args",
        (_, Class::Tool) => false,
        (Class::Wiring, Class::Wiring) => from == "hirsel-driver" && to == "hirsel-engine",
    };
    if allowed {
        Ok(())
    } else {
        Err(format!(
            "{kind:?} edge {from} ({source:?}) -> {to} ({target:?})"
        ))
    }
}

fn metadata() -> Value {
    let root = concat!(env!("CARGO_MANIFEST_DIR"), "/../..");
    let prepared = std::path::Path::new(root).join("tools/buck2/workspace-metadata.json");
    if prepared.is_file() {
        return serde_json::from_slice(&std::fs::read(prepared).unwrap())
            .expect("prepared Cargo metadata JSON");
    }
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let output = Command::new(cargo)
        .args([
            "metadata",
            "--format-version",
            "1",
            "--no-deps",
            "--offline",
        ])
        .current_dir(root)
        .output()
        .expect("run cargo metadata");
    assert!(
        output.status.success(),
        "cargo metadata failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("cargo metadata JSON")
}

fn edges(metadata: &Value) -> (Vec<String>, Vec<EdgeRow>) {
    let mut members = Vec::new();
    let mut edges = Vec::new();
    for package in metadata["packages"].as_array().expect("packages") {
        let from = package["name"].as_str().expect("package name").to_string();
        for dependency in package["dependencies"].as_array().expect("dependencies") {
            let kind = match dependency["kind"].as_str() {
                None => Kind::Production,
                Some("dev") => Kind::Dev,
                Some("build") => Kind::Build,
                Some(other) => panic!("unknown dependency kind {other}"),
            };
            edges.push(EdgeRow {
                from: from.clone(),
                to: dependency["name"].as_str().expect("dependency name").into(),
                kind,
                workspace: dependency.get("path").is_some_and(|p| !p.is_null()),
            });
        }
        members.push(from);
    }
    (members, edges)
}

#[test]
fn workspace_edges_respect_the_crate_layering() {
    let (members, edges) = edges(&metadata());
    let unclassified: Vec<_> = members.iter().filter(|m| class(m).is_none()).collect();
    assert!(
        unclassified.is_empty(),
        "classify every workspace crate in tests/it/dependencies.rs: {unclassified:?}"
    );
    let mut violations = Vec::new();
    let mut counts = [0usize; 3];
    for edge in edges.iter().filter(|e| e.workspace) {
        counts[edge.kind as usize] += 1;
        if let Err(violation) = check_edge(&edge.from, &edge.to, edge.kind) {
            violations.push(violation);
        }
    }
    for edge in &edges {
        if !edge.workspace
            && edge.kind == Kind::Production
            && NO_EXTERNAL_PRODUCTION.contains(&edge.from.as_str())
            // PS-18 findings are JSON sidecars; this codec does not wire compiler phases.
            && !(edge.from == "hirsel-proof-export" && edge.to == "serde_json")
        {
            violations.push(format!(
                "external production dependency {} -> {}",
                edge.from, edge.to
            ));
        }
    }
    for package in metadata()["packages"].as_array().unwrap() {
        if package["name"] == "hirsel-kernels" {
            for dep in package["dependencies"].as_array().unwrap() {
                if dep["kind"].is_null() && dep["path"].is_null() {
                    assert!(matches!(
                        dep["name"].as_str(),
                        Some("sha1" | "sha2" | "itoa")
                    ));
                    assert!(dep["req"].as_str().unwrap().starts_with('='));
                    assert_eq!(dep["uses_default_features"], false);
                }
            }
        }
    }
    println!(
        "{} workspace crates; workspace edges: {} production, {} dev, {} build",
        members.len(),
        counts[0],
        counts[1],
        counts[2]
    );
    for edge in edges.iter().filter(|e| e.workspace && e.kind == Kind::Dev) {
        println!("allowed test-only edge {} -> {}", edge.from, edge.to);
    }
    assert!(
        violations.is_empty(),
        "dependency violations: {violations:#?}"
    );
    assert!(
        counts[0] > 0 && counts[1] > 0,
        "metadata parse saw no edges"
    );
}

#[test]
fn layering_controls() {
    // The graph helpers serve every compiler layer and read only data crates
    // and leaf utilities.
    for &(consumer, kind) in CRATES {
        if consumer != "hirsel-graph" {
            let leaf = matches!(kind, Class::LeafUtility | Class::LeafKernel);
            assert_eq!(
                check_edge(consumer, "hirsel-graph", Kind::Production).is_ok(),
                !leaf,
                "{consumer}"
            );
            assert_eq!(
                check_edge("hirsel-graph", consumer, Kind::Production).is_ok(),
                matches!(kind, Class::Data | Class::LeafUtility),
                "{consumer}"
            );
        }
    }
    for consumer in STORAGE_CONSUMERS {
        assert!(check_edge(consumer, "hirsel-artifact-store", Kind::Production).is_ok());
    }
    assert!(check_edge("hirsel-graph", "hirsel-artifact-store", Kind::Production).is_err());
    assert!(check_edge("hirsel-kernels", "hirsel-artifact-store", Kind::Production).is_err());
    for consumer in ["hirsel-rc", "hirsel-cli", "hirsel-conformance"] {
        for kind in [Kind::Production, Kind::Build] {
            assert!(check_edge(consumer, "hirsel-artifact-store", kind).is_err());
        }
    }
    for &(phase, class) in CRATES {
        if matches!(class, Class::Implementation | Class::Wiring | Class::Tool) {
            assert!(check_edge("hirsel-artifact-store", phase, Kind::Production).is_err());
            assert!(check_edge("hirsel-artifact-store", phase, Kind::Build).is_err());
        }
    }
    // Leaf utilities depend on no workspace crate.
    assert!(check_edge("hirsel-guard", "hirsel-graph", Kind::Production).is_err());
    assert!(check_edge("hirsel-free", "hirsel-guard", Kind::Production).is_ok());
    assert!(check_edge("hirsel-free", "hirsel-rt", Kind::Production).is_err());
    assert!(check_edge("hirsel-free", "hirsel-interp", Kind::Production).is_err());
    for consumer in ["hirsel-rt", "hirsel-interp", "hirsel-heapcheck"] {
        assert!(check_edge(consumer, "hirsel-kernels", Kind::Production).is_ok());
    }
    assert!(check_edge("hirsel-rc", "hirsel-kernels", Kind::Production).is_err());
    assert!(check_edge("hirsel-kernels", "hirsel-types", Kind::Production).is_err());
    assert!(check_edge("hirsel-kernels", "hirsel-core-contract", Kind::Production).is_ok());
    // Implementation crates may use shared data crates.
    assert!(check_edge("hirsel-rc", "hirsel-lir", Kind::Production).is_ok());
    assert!(check_edge("hirsel-heapcheck", "hirsel-core-contract", Kind::Production).is_ok());
    for from in ["hirsel-lir", "hirsel-codec"] {
        for to in ["hirsel-types", "hirsel-tir", "hirsel-layout"] {
            assert!(check_edge(from, to, Kind::Production).is_err());
            assert!(check_edge(from, to, Kind::Build).is_err());
            assert!(check_edge(from, to, Kind::Dev).is_ok());
        }
    }
    assert!(check_edge("hirsel-types", "hirsel-codec", Kind::Production).is_ok());
    assert!(check_edge("hirsel-layout", "hirsel-types", Kind::Production).is_ok());
    assert!(check_edge("hirsel-layout", "hirsel-lir", Kind::Production).is_ok());
    // Only the driver wires implementations; data crates stay data.
    assert!(check_edge("hirsel-driver", "hirsel-rc", Kind::Production).is_ok());
    assert!(check_edge("hirsel-lir", "hirsel-rc", Kind::Production).is_err());
    assert!(check_edge("hirsel-lir", "hirsel-driver", Kind::Production).is_err());
    assert!(check_edge("hirsel-rc", "hirsel-driver", Kind::Production).is_err());
    assert!(check_edge("hirsel-driver", "hirsel-cli", Kind::Production).is_err());
    assert!(check_edge("hirsel-lir", "hirsel-driver", Kind::Dev).is_ok());
    // The engine uses only its listed phases; only the driver depends on it.
    assert!(check_edge("hirsel-engine", "hirsel-interp", Kind::Production).is_ok());
    assert!(check_edge("hirsel-engine", "hirsel-check", Kind::Production).is_err());
    assert!(check_edge("hirsel-engine", "hirsel-rc", Kind::Production).is_err());
    assert!(check_edge("hirsel-driver", "hirsel-engine", Kind::Production).is_ok());
    assert!(check_edge("hirsel-engine", "hirsel-driver", Kind::Production).is_err());
    assert!(check_edge("hirsel-rc", "hirsel-engine", Kind::Production).is_err());
    // A crate nobody classified fails.
    assert!(check_edge("hirsel-new", "hirsel-lir", Kind::Production).is_err());
}

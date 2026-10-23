//! PROG-2 composition vectors adapted from rust-analyzer (MIT OR Apache-2.0),
//! 3131751eaf1c3b5da6e618acdc2148ad7db65bc7: hir-def/src/visibility.rs:43-119,
//! hir-def/src/nameres/path_resolution.rs:1-82 and tests/mod_resolution.rs:1-96;
//! ide-diagnostics/src/handlers/private_field.rs:1-138 (named/tuple fields).
//! Hirsel uses package opacity and internal ancestry, not Rust field visibility.
//! Reuse package_head_22's explicit source/dependency/exposure fixture shape and
//! the public manifest loader; expected answers below use only PROG-2's table.
use hirsel_driver::*;
use hirsel_engine::*;
use hirsel_syntax::{DefPath, ItemPath, PackageId};
use serde_json::json;

#[derive(Clone, Copy, Debug)]
enum Client {
    Owner,
    OtherName,
    OtherVersion,
}
impl Client {
    fn package(self) -> PackageId {
        let (name, version) = match self {
            Self::Owner => ("dep", "1"),
            Self::OtherName => ("client", "1"),
            Self::OtherVersion => ("dep", "2"),
        };
        PackageId {
            name: name.into(),
            version: version.into(),
        }
    }
    fn owner(self) -> bool {
        matches!(self, Self::Owner)
    }
    fn short(self, module: &[&str]) -> String {
        if self.owner() {
            module.join(".")
        } else {
            "vendor".into()
        }
    }
}

/// Each call emits its complete immutable input before loading and its result
/// after checking. Setup/incomplete/V failures panic separately from PROG-2.
fn case(
    id: &str,
    client: Client,
    module: &[&str],
    target_module: &[&str],
    owner: &str,
    source: &str,
    rejected_operation: Option<&str>,
) {
    let dir = tempfile::tempdir().unwrap();
    let owner_source = json!({"path":"owner.hir", "module":target_module});
    let mut sources = vec![json!({"path":"client.hir", "module":module})];
    let mut dependencies = vec![];
    if client.owner() {
        sources.push(owner_source.clone());
    } else {
        dependencies.push(json!({"manifest":"owner.json", "package":{"name":"dep","version":"1"}, "expose":[{"head":"vendor", "module":target_module}]}));
    }
    let files = json!({
        "owner.hir":owner,
        "client.hir":source,
        "owner.json":json!({"schema":1,"package":{"name":"dep","version":"1"},"sources":[owner_source],"dependencies":[]}).to_string(),
        "client.json":json!({"schema":1,"package":client.package(),"sources":sources,"dependencies":dependencies,"entry":{"module":module,"name":"main"}}).to_string(),
    });
    eprintln!(
        "VISIBILITY_BOUNDARY_22_FIXTURE {}",
        json!({"id":id,"files":files,"rejected_operation":rejected_operation})
    );
    for (name, value) in files.as_object().unwrap() {
        std::fs::write(dir.path().join(name), value.as_str().unwrap()).unwrap();
    }
    let mut program = load_program(&dir.path().join("client.json")).unwrap();
    let output = program.check(LoweringPolicy::Reachable).unwrap();
    assert_eq!(output.completion, CheckCompletion::Complete, "{id}");
    if let Some(operation) = rejected_operation {
        // Each private type occurrence in a public signature is diagnosed.
        // Hidden positional variants also lack a public value-namespace name;
        // retain that LANG-4 alongside the representation denial at that site.
        let sites: Vec<_> = source.match_indices(operation).map(|(i, _)| i).collect();
        assert!(!sites.is_empty());
        let expected = if operation == "Full(7)" {
            vec!["LANG-4", "PROG-2"]
        } else {
            vec!["PROG-2"; sites.len()]
        };
        assert_eq!(
            output
                .diagnostics
                .iter()
                .map(|d| d.rule.as_str())
                .collect::<Vec<_>>(),
            expected,
            "{id}: {:?}",
            output.diagnostics
        );
        for d in &output.diagnostics {
            let primary = output.sources.resolve(d.primary);
            assert!(
                sites.iter().any(|&start| primary.start as usize >= start
                    && primary.end as usize <= start + operation.len()
                    && primary.start < primary.end),
                "{id}: expected site in {operation:?}, got {:?}: {d:?}",
                output.sources.slice(d.primary)
            );
            assert!(
                output
                    .sources
                    .file(primary.file)
                    .unwrap()
                    .path
                    .ends_with("client.hir")
            );
            assert!(d.fix.is_none(), "{id}: inaccessible repair: {d:?}");
            assert!(
                d.notes.is_empty(),
                "{id}: unexpected repair suggestion: {d:?}"
            );
        }
    } else {
        assert!(
            output.diagnostics.is_empty(),
            "{id}: {:?}",
            output.diagnostics
        );
        // Explicitly lower the parameterized probe: empty main does not prove
        // that the helper's body passes V. No runtime execution is claimed.
        program
            .engine
            .lower_to_tir(&DefPath(ItemPath {
                package: client.package(),
                modules: module.iter().map(|s| (*s).into()).collect(),
                name: "probe".into(),
            }))
            .unwrap_or_else(|e| panic!("{id}: V: {e:?}"));
    }
    eprintln!(
        "VISIBILITY_BOUNDARY_22_RESULT {}",
        json!({"id":id,"decision":if rejected_operation.is_some() {"PROG-2"} else {"accept+V"}})
    );
}

#[test]
fn visibility_boundary_22_definitions_obey_package_and_internal_ancestry() {
    with_compiler_stack(|| {
        // 24 alias/full-path pairs: two namespaces, three exact client
        // identities, descendant/sibling, private/public. Every file differs.
        for client in [Client::Owner, Client::OtherName, Client::OtherVersion] {
            for (module, beneath_parent) in [(&["a", "child"][..], true), (&["b"][..], false)] {
                for public in [false, true] {
                    for is_type in [false, true] {
                        let target = &["a", "internal", "detail"];
                        let owner = format!(
                            "{}{}",
                            if public { "pub " } else { "" },
                            if is_type {
                                "type Secret { payload: u64 }"
                            } else {
                                "fn secret() -> u64 { 7 }"
                            }
                        );
                        // Private definitions are package-wide, but internal
                        // ancestry still restricts even public definitions.
                        let allowed = client.owner() && beneath_parent;
                        for full in [false, true] {
                            let prefix = if full {
                                "dep@1/a.internal.detail".into()
                            } else {
                                client.short(target)
                            };
                            let name =
                                format!("{prefix}.{}", if is_type { "Secret" } else { "secret" });
                            let source = if is_type {
                                format!("fn probe(v: {name}) {{}}\npub fn main() {{}}\n")
                            } else {
                                format!("fn probe() -> u64 {{ {name}() }}\npub fn main() {{}}\n")
                            };
                            case(
                                &format!(
                                    "definition-{client:?}-{beneath_parent}-{public}-{is_type}-{full}"
                                ),
                                client,
                                module,
                                target,
                                &owner,
                                &source,
                                (!allowed).then_some(name.as_str()),
                            );
                        }
                    }
                }
            }
        }
    });
}

const OPAQUE: &str = r#"
pub opaque type Hidden: data uses {} { payload: u64 }
pub opaque type Choice: data uses {} = Empty() | Full(u64)
pub fn make(n: u64) -> Hidden { Hidden(payload: n) }
pub fn read(v: Hidden) -> u64 { v.payload }
pub fn choose(n: u64) -> Choice { Full(n) }
"#;

#[test]
fn visibility_boundary_22_opaque_operations_and_callable_alternatives() {
    with_compiler_stack(|| {
        // 12 alias/full-path pairs: three exact identities and four operations.
        // Type spelling remains public, including an owner-provided constructor
        // function and reader; record/positional construction and fields do not.
        for client in [Client::Owner, Client::OtherName, Client::OtherVersion] {
            for operation in ["field", "record", "variant", "type-and-api"] {
                for full in [false, true] {
                    let prefix = if full {
                        "dep@1/api".into()
                    } else {
                        client.short(&["api"])
                    };
                    let (probe, site) = match operation {
                        "field" => (
                            format!("fn probe(v: {prefix}.Hidden) -> u64 {{ v.payload }}"),
                            "v.payload".into(),
                        ),
                        "record" => {
                            let site = format!("{prefix}.Hidden(payload: 7)");
                            (format!("fn probe() -> {prefix}.Hidden {{ {site} }}"), site)
                        }
                        "variant" => (
                            format!("fn probe() -> {prefix}.Choice {{ Full(7) }}"),
                            "Full(7)".into(),
                        ),
                        _ => (
                            format!(
                                "pub fn probe() -> u64 {{ let v: {prefix}.Hidden = {prefix}.make(7); let _: {prefix}.Choice = {prefix}.choose(9); {prefix}.read(v) }}"
                            ),
                            String::new(),
                        ),
                    };
                    let source = format!("{probe}\npub fn main() {{}}\n");
                    case(
                        &format!("opaque-{client:?}-{operation}-{full}"),
                        client,
                        &["sibling"],
                        &["api"],
                        OPAQUE,
                        &source,
                        (!client.owner() && operation != "type-and-api").then_some(site.as_str()),
                    );
                }
            }
        }
    });
}

#[test]
fn visibility_boundary_22_public_signatures_cannot_launder_private_types() {
    with_compiler_stack(|| {
        // Four private/public source pairs: two spellings, function argument +
        // result and transparent alias. No new authority ceiling is involved.
        for full in [false, true] {
            for alias in [false, true] {
                for public_type in [false, true] {
                    let name = if full {
                        "dep@1/api.Secret"
                    } else {
                        "api.Secret"
                    };
                    let owner = format!(
                        "{}type Secret {{ payload: u64 }}",
                        if public_type { "pub " } else { "" }
                    );
                    let header = if alias {
                        format!("pub type Leaked = {name}")
                    } else {
                        format!("pub fn probe(v: {name}) -> {name} {{ v }}")
                    };
                    let source = format!(
                        "{header}\n{}pub fn main() {{}}\n",
                        if alias { "fn probe() {}\n" } else { "" }
                    );
                    case(
                        &format!("signature-{full}-{alias}-{public_type}"),
                        Client::Owner,
                        &["sibling"],
                        &["api"],
                        &owner,
                        &source,
                        (!public_type).then_some(name),
                    );
                }
            }
        }
    });
}

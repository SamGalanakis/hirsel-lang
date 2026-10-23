//! Finite empty/singleton permission controls and cross-package private-field
//! vectors (Rust BTreeSet subset tests / rust-analyzer private_field). Only the
//! fixture shapes are adapted: CAP-5 and exact package/version own the verdicts.
use hirsel_driver::*;
use hirsel_engine::*;
use hirsel_types::{AuthoritySummary, AuthorityTerm, SignatureKind};
use hirsel_validate::check_view;
use std::path::{Path, PathBuf};

const OWNER: &str = r#"
@capability(global)
pub opaque type Global: plain uses {} { payload: u64 }
@capability(local)
pub opaque type Local: () uses {} { payload: u64 }
pub fn make_global(n: u64) -> Global { Global(payload: n) }
pub fn make_local(n: u64) -> u64 { let facet = Local(payload: n); facet.payload }
pub fn read_global(v: Global) -> u64 { v.payload }
pub fn read_local(v: Local) -> u64 { v.payload }
pub type Record[T] { value: T }
pub type Choice[T] = Empty() | Full(T)
pub type Alias[T] = Record[T]
pub type Wrapper[T] { value: T, scalar: u64 }
"#;

fn path(package: &str, name: &str) -> DefPath {
    DefPath(hirsel_syntax::ItemPath {
        package: hirsel_syntax::PackageId {
            name: package.into(),
            version: "1".into(),
        },
        modules: vec![],
        name: name.into(),
    })
}

fn fixture(owner: &str, client: &str) -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("owner.hir"), owner).unwrap();
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
    let manifest = dir.path().join("client.json");
    (dir, manifest)
}

fn check(manifest: &Path, expected: Option<&str>) -> LoadedProgram {
    let mut program = load_program(manifest).unwrap();
    let output = program.check(LoweringPolicy::Reachable).unwrap();
    assert_eq!(
        output.completion,
        CheckCompletion::Complete,
        "{manifest:?}: {:?}",
        output.completion
    );
    let rules: Vec<_> = output.diagnostics.iter().map(|d| d.rule.as_str()).collect();
    assert_eq!(
        rules,
        expected.into_iter().collect::<Vec<_>>(),
        "{:?}",
        output.diagnostics
    );
    program
}

#[test]
fn nominal_authority_22_compositions_keep_the_seal_permission() {
    with_compiler_stack(|| {
        // Ten pairs: two seal kinds times five nonrecursive compositions.
        for seal in ["Global", "Local"] {
            for shape in [
                format!("(vendor.{seal}, u64)"),
                format!("vendor.Record[vendor.{seal}]"),
                format!("vendor.Choice[vendor.{seal}]"),
                format!("vendor.Alias[vendor.{seal}]"),
                format!("vendor.Wrapper[vendor.{seal}]"),
            ] {
                for allowed in [true, false] {
                    let permission = if allowed {
                        format!("vendor.{seal}")
                    } else {
                        String::new()
                    };
                    let client = format!(
                        "fn charge(v: {shape}) uses {{v}} {{}}\nfn probe(v: {shape}) uses {{{permission}}} {{ charge(v) }}\npub fn main() {{ let g = vendor.make_global(7); let _ = vendor.read_global(g) }}"
                    );
                    let (_dir, manifest) = fixture(OWNER, &client);
                    let mut program = check(&manifest, if allowed { None } else { Some("CAP-1") });
                    if allowed {
                        // Explicitly lower this parameterized body, even though main
                        // does not call it. This proves V acceptance, not execution.
                        program
                            .engine
                            .lower_to_tir(&path("client", "probe"))
                            .unwrap_or_else(|error| panic!("{seal}: {shape}: {error:?}"));
                    }
                }
            }
        }
    });
}

#[test]
fn nominal_authority_22_owner_views_and_client_access() {
    with_compiler_stack(|| {
        for seal in ["Global", "Local"] {
            let client = format!(
                "fn probe(v: vendor.{seal}) -> u64 {{ vendor.read_{}(v) }}\npub fn main() {{}}",
                seal.to_lowercase()
            );
            let (_dir, manifest) = fixture(OWNER, &client);
            let mut program = check(&manifest, None);
            for name in ["make_global", "make_local", "read_global", "read_local"] {
                program.engine.lower_to_tir(&path("owner", name)).unwrap();
            }
            let public = program
                .engine
                .item_signature(&path("owner", seal))
                .unwrap()
                .value;
            let SignatureKind::Type(d) = &public.kind else {
                panic!("type")
            };
            assert!(d.representation.is_none() && d.implementation_authority.is_none());
            assert!(
                hirsel_types::encoding::signature_types(program.engine.types(), &public).is_empty()
            );
            assert_eq!(
                d.authority,
                AuthoritySummary::Known(
                    vec![AuthorityTerm::Capability(path("owner", seal))].into()
                )
            );
            let owner = program
                .engine
                .owner_representation(&path("owner", seal), &path("owner", seal))
                .unwrap();
            assert!(program.engine.types().contains(owner.private_shape));
            // Inspect the actual two package universes, then ask V to prove
            // each declaration independently of source normalization.
            let snapshot = program.engine.signature_snapshot();
            for package in ["owner", "client"] {
                let views = snapshot.validation(&path(package, "probe").0.package);
                let view = views.view(public.definition).unwrap().signature();
                assert!(
                    check_view(program.engine.types(), public.definition, view, views)
                        .error
                        .is_none()
                );
                let SignatureKind::Type(d) = &view.kind else {
                    panic!("type")
                };
                assert_eq!(
                    d.authority,
                    AuthoritySummary::Known(
                        vec![AuthorityTerm::Capability(path("owner", seal))].into()
                    )
                );
                if package == "owner" {
                    assert_eq!(d.representation, Some(owner.private_shape));
                    assert_eq!(d.implementation_authority, Some(AuthoritySummary::inert()));
                } else {
                    assert!(d.representation.is_none() && d.implementation_authority.is_none());
                }
            }
            assert!(
                program
                    .engine
                    .owner_representation(&path("client", "probe"), &path("owner", seal))
                    .is_err()
            );
            // Paired access: same scalar field accepted through the owner API,
            // rejected when projected directly by the client.
            let client =
                format!("fn probe(v: vendor.{seal}) -> u64 {{ v.payload }}\npub fn main() {{}}");
            let (_dir, manifest) = fixture(OWNER, &client);
            check(&manifest, Some("PROG-2"));
        }
    });
}

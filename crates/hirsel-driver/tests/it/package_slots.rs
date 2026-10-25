//! PROG-1 implementation slots: schema-2 slot declarations, fills, whole-map
//! overrides and forwarding; one effective map per package/version, with both
//! selecting paths named when two differ; and configuration outside identity.
//!
//! Vectors follow dune's virtual-library tests (double-implementation.t: two
//! implementations through two requirers; vlib-default-impl/default-impl.t: a
//! default-implementation cycle) and Cargo's duplicate `links` refusal naming
//! both dependency paths (src/compiler/links.rs:44).
use hirsel_driver::*;
use hirsel_engine::*;
use hirsel_syntax::{DefPath, PackageId};
use hirsel_types::{GenericDeclarations, SignatureKind, serialization::serializable_schema};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

const PROVIDER: &str = "pub signature Digest {
  fn bits() -> u64 uses {}
}

pub signature Clock {
  fn now() -> u64 uses {}
}

@noalloc
pub fn fast_bits() -> u64 uses {} {
  1
}

@noalloc
pub fn slow_bits() -> u64 uses {} {
  2
}

fn hidden_bits() -> u64 uses {} {
  3
}

pub implementation fast: Digest {
  bits = fast_bits
}

pub implementation slow: Digest {
  bits = slow_bits
}

implementation hidden: Digest {
  bits = hidden_bits
}

pub implementation tick: Clock {
  now = fast_bits
}
";
const MAC: &str = "@serializable
pub type Tag {
  value: u64,
}

@noalloc
pub fn tag() -> Tag uses {} {
  Tag(value: 7)
}
";
const MAIN: &str = "pub fn main() -> Result[(), ()] {
  let t = mac.tag()
  Ok(())
}
";

fn implementation(dependency: &str, name: &str) -> Value {
    json!({"implementation": {"dependency": dependency, "module": ["api"], "name": name}})
}
fn forward(dependency: &str, name: &str) -> Value {
    json!({"parameter": {"dependency": dependency, "name": name}})
}
fn descriptor(manifest: &str, name: &str) -> Value {
    json!({"manifest": manifest, "package": {"name": name, "version": "1"}})
}
fn uses(id: &str, bindings: Value) -> Value {
    json!({"use": id, "bindings": bindings, "expose": []})
}

/// `prov@1` (schema 1), `mac@1` with slot `H` defaulting to `prov.fast`, and
/// `fwd@1`, whose slot `R` forwards mac's effective `H` by default.
struct Fixture(tempfile::TempDir);
impl Fixture {
    fn new() -> Self {
        let dir = tempfile::Builder::new()
            .prefix("hirsel-package-slots-")
            .tempdir()
            .unwrap();
        let fixture = Self(dir);
        fixture.write("prov/prov.hir", PROVIDER);
        fixture.json(
            "prov/hirsel.json",
            json!({"schema":1,"package":{"name":"prov","version":"1"},
                "sources":[{"path":"prov.hir","module":["api"]}],"dependencies":[]}),
        );
        fixture.write("mac/mac.hir", MAC);
        fixture.mac(implementation("prov", "fast"));
        fixture.write("fwd/fwd.hir", "pub fn unused() {}\n");
        fixture.fwd(forward("mac", "H"), json!({}));
        fixture.write("other/other.hir", "pub fn unused() {}\n");
        fixture.other(json!({}));
        fixture.write("main.hir", MAIN);
        fixture
    }
    fn write(&self, path: &str, text: &str) {
        let path = self.0.path().join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }
    fn json(&self, path: &str, value: Value) {
        self.write(path, &value.to_string());
    }
    fn mac(&self, default: Value) {
        self.json(
            "mac/hirsel.json",
            json!({"schema":2,"package":{"name":"mac","version":"1"},
                "sources":[{"path":"mac.hir","module":["mac"]}],
                "parameters":{"H":{
                    "signature":{"dependency":"prov","module":["api"],"name":"Digest"},
                    "default":default}},
                "dependency_catalog":{"prov":descriptor("../prov/hirsel.json","prov"),
                    "gone":descriptor("../absent/hirsel.json","gone")},
                "dependencies":[]}),
        );
    }
    fn fwd(&self, default: Value, mac_bindings: Value) {
        self.json(
            "fwd/hirsel.json",
            json!({"schema":2,"package":{"name":"fwd","version":"1"},
                "sources":[{"path":"fwd.hir","module":["fwd"]}],
                "parameters":{"R":{
                    "signature":{"dependency":"prov","module":["api"],"name":"Digest"},
                    "default":default}},
                "dependency_catalog":{"prov":descriptor("../prov/hirsel.json","prov"),
                    "mac":descriptor("../mac/hirsel.json","mac")},
                "dependencies":[uses("mac", mac_bindings)]}),
        );
    }
    fn other(&self, mac_bindings: Value) {
        self.json(
            "other/hirsel.json",
            json!({"schema":2,"package":{"name":"other","version":"1"},
                "sources":[{"path":"other.hir","module":["other"]}],
                "dependency_catalog":{"prov":descriptor("../prov/hirsel.json","prov"),
                    "mac":descriptor("../mac/hirsel.json","mac")},
                "dependencies":[uses("mac", mac_bindings)]}),
        );
    }
    /// The root `app@1`, whose catalog also names `prov` as `alias`.
    fn app(&self, dependencies: Value) -> PathBuf {
        self.json(
            "hirsel.json",
            json!({"schema":2,"package":{"name":"app","version":"1"},
                "entry":{"module":[],"name":"main"},
                "sources":[{"path":"main.hir","module":[]}],
                "dependency_catalog":{
                    "mac":descriptor("mac/hirsel.json","mac"),
                    "mac2":descriptor("mac/hirsel.json","mac"),
                    "fwd":descriptor("fwd/hirsel.json","fwd"),
                    "other":descriptor("other/hirsel.json","other"),
                    "prov":descriptor("prov/hirsel.json","prov"),
                    "alias":descriptor("prov/hirsel.json","prov")},
                "dependencies":dependencies}),
        );
        self.0.path().join("hirsel.json")
    }
}

fn mac() -> PackageId {
    package("mac")
}
fn package(name: &str) -> PackageId {
    PackageId {
        name: name.into(),
        version: "1".into(),
    }
}
fn prov(name: &str) -> String {
    format!("prov@1/api/{name}")
}
fn slot(program: &LoadedProgram, package: &PackageId, slot: &str) -> String {
    program.slots[package][slot].implementation.to_string()
}
fn refusal(path: &Path) -> String {
    match load_program(path) {
        Err(ToolError::Runtime(message)) => message,
        Err(other) => panic!("expected a tool-input error, got {other}"),
        Ok(_) => panic!("expected a tool-input error"),
    }
}

#[test]
fn an_omitted_slot_takes_its_default_and_selects_its_provider() {
    with_compiler_stack(|| {
        let fixture = Fixture::new();
        let path = fixture.app(json!([uses("mac", json!({}))]));
        let program = load_program(&path).unwrap();
        assert_eq!(slot(&program, &mac(), "H"), prov("fast"));
        assert_eq!(
            program.slots[&mac()]["H"].signature.to_string(),
            prov("Digest")
        );
        assert!(program.slots.get(&package("app")).is_none());
        // The default's catalog target is selected without being exposed.
        assert!(
            program
                .source_paths()
                .iter()
                .any(|p| p.ends_with("prov/prov.hir"))
        );
        let output = check_path(&path).unwrap();
        assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    });
}

#[test]
fn a_dependent_overrides_a_slot_and_its_default_provider_is_never_opened() {
    with_compiler_stack(|| {
        let fixture = Fixture::new();
        let path = fixture.app(json!([uses(
            "mac",
            json!({"H": implementation("prov", "slow")})
        )]));
        assert_eq!(
            slot(&load_program(&path).unwrap(), &mac(), "H"),
            prov("slow")
        );
        // A default naming an absent catalog target loads while overridden...
        fixture.mac(implementation("gone", "fast"));
        let program = load_program(&path).unwrap();
        assert_eq!(slot(&program, &mac(), "H"), prov("slow"));
        // ...and fails as a missing input when it is the effective actual.
        let path = fixture.app(json!([uses("mac", json!({}))]));
        assert!(load_program(&path).is_err());
    });
}

#[test]
fn a_forwarded_slot_reads_the_dependency_s_effective_slot() {
    with_compiler_stack(|| {
        let fixture = Fixture::new();
        // fwd's R forwards mac's H, which fwd's own edge fills: R follows it.
        fixture.fwd(
            forward("mac", "H"),
            json!({"H": implementation("prov", "slow")}),
        );
        let path = fixture.app(json!([uses("fwd", json!({}))]));
        let program = load_program(&path).unwrap();
        assert_eq!(slot(&program, &package("fwd"), "R"), prov("slow"));
        assert_eq!(slot(&program, &mac(), "H"), prov("slow"));
        // An omitted binding is the default: a second, default selection of
        // mac is a different map, and no path takes precedence.
        fixture.fwd(forward("mac", "H"), json!({}));
        let path = fixture.app(json!([
            uses("mac", json!({"H": implementation("prov", "slow")})),
            uses("fwd", json!({})),
        ]));
        assert!(refusal(&path).contains("app@1 -> fwd@1 -> mac@1 selects {H = prov@1/api/fast}"));
        // An explicit actual on fwd's edge suppresses its forwarding default.
        let path = fixture.app(json!([
            uses("mac", json!({})),
            uses("fwd", json!({"R": implementation("prov", "slow")})),
        ]));
        let program = load_program(&path).unwrap();
        assert_eq!(slot(&program, &package("fwd"), "R"), prov("slow"));
        assert_eq!(slot(&program, &mac(), "H"), prov("fast"));
        // A frame forwards its own slot to a dependency's edge.
        fixture.fwd(
            implementation("prov", "slow"),
            json!({"H": forward("self", "R")}),
        );
        let path = fixture.app(json!([uses("fwd", json!({}))]));
        let program = load_program(&path).unwrap();
        assert_eq!(slot(&program, &mac(), "H"), prov("slow"));
    });
}

#[test]
fn a_forwarding_cycle_bad() {
    with_compiler_stack(|| {
        let fixture = Fixture::new();
        // mac.H <- fwd.R (fwd's default) <- mac.H (app's actual for mac).
        let path = fixture.app(json!([
            uses("mac", json!({"H": forward("fwd", "R")})),
            uses("fwd", json!({})),
        ]));
        let message = refusal(&path);
        assert!(
            message.contains("implementation slot forwarding cycle: ")
                && message.contains("fwd@1.R")
                && message.contains("mac@1.H"),
            "{message}"
        );
        // Two slots of one package forwarding to each other.
        fixture.json(
            "mac/hirsel.json",
            json!({"schema":2,"package":{"name":"mac","version":"1"},
                "sources":[{"path":"mac.hir","module":["mac"]}],
                "parameters":{
                    "H":{"signature":{"dependency":"self","module":["mac"],"name":"Tag"},
                        "default":forward("self","K")},
                    "K":{"signature":{"dependency":"self","module":["mac"],"name":"Tag"},
                        "default":forward("self","H")}},
                "dependency_catalog":{},"dependencies":[]}),
        );
        let path = fixture.app(json!([uses("mac", json!({}))]));
        assert_eq!(
            refusal(&path),
            "package manifest: implementation slot forwarding cycle: \
             mac@1.H -> mac@1.K -> mac@1.H"
        );
    });
}

#[test]
fn two_unequal_maps_for_one_package_bad() {
    with_compiler_stack(|| {
        let fixture = Fixture::new();
        // app selects mac with H = slow; other selects mac with its default.
        let edges = [
            uses("mac", json!({"H": implementation("prov", "slow")})),
            uses("other", json!({})),
        ];
        let message = refusal(&fixture.app(json!(edges)));
        assert_eq!(
            message,
            "package manifest: mac@1 is selected with two unequal implementation slot \
             maps: app@1 -> mac@1 selects {H = prov@1/api/slow}, and app@1 -> other@1 -> \
             mac@1 selects {H = prov@1/api/fast}"
        );
        // Edge order and discovery order never choose the reported paths.
        let [a, b] = edges;
        assert_eq!(refusal(&fixture.app(json!([b, a]))), message);
        // Two catalog IDs naming one package are two selections of it.
        let message = refusal(&fixture.app(json!([
            uses("mac", json!({"H": implementation("prov", "slow")})),
            uses("mac2", json!({})),
        ])));
        assert!(
            message.contains("app@1 -> mac@1 selects {H = prov@1/api/slow}")
                && message.contains("app@1 -> mac@1 (as mac2) selects {H = prov@1/api/fast}"),
            "{message}"
        );
    });
}

#[test]
fn maps_equal_after_normalization_are_one_selection() {
    with_compiler_stack(|| {
        let fixture = Fixture::new();
        // An explicit default through a catalog alias, the omitted default and
        // a forwarded default all normalize to prov@1/api/fast.
        fixture.other(json!({"H": implementation("prov", "fast")}));
        let path = fixture.app(json!([
            uses("mac", json!({"H": implementation("alias", "fast")})),
            uses("other", json!({})),
            uses("fwd", json!({})),
        ]));
        let program = load_program(&path).unwrap();
        assert_eq!(slot(&program, &mac(), "H"), prov("fast"));
        assert_eq!(slot(&program, &package("fwd"), "R"), prov("fast"));
        let output = check_path(&path).unwrap();
        assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
        // Separately named equal-body implementations stay distinct.
        fixture.other(json!({"H": implementation("prov", "slow")}));
        assert!(refusal(&path).contains("two unequal implementation slot maps"));
    });
}

#[test]
fn slot_manifests_refuse_structural_errors() {
    with_compiler_stack(|| {
        let fixture = Fixture::new();
        let cases = [
            (
                json!([uses("mac", json!({"Q": implementation("prov", "fast")}))]),
                "app@1 -> mac@1 binds Q, which mac@1 does not declare as a slot",
            ),
            (
                json!([uses("mac", json!({"H": implementation("nowhere", "fast")}))]),
                "app@1 names \"nowhere\", which is not in its dependency_catalog",
            ),
            (
                json!([uses("mac", json!({"H": forward("other", "H")}))]),
                "app@1 forwards a slot of \"other\", which it does not select",
            ),
            (
                json!([uses(
                    "mac",
                    json!({"H": {"implementation": {"dependency":"prov",
                    "module":["api"],"name":"fast"}, "parameter": {}}})
                )]),
                "a binding is exactly one of",
            ),
            (
                json!([uses("mac", json!({})), uses("mac", json!({}))]),
                "two dependencies use \"mac\"",
            ),
            (
                json!([uses("missing", json!({}))]),
                "dependency uses \"missing\", which is not in the dependency_catalog",
            ),
            (
                json!([uses("mac", json!({"H": implementation("prov", "absent")}))]),
                "mac@1's slot H names the implementation prov@1/api/absent, which is not a \
                 declaration",
            ),
            (
                json!([uses("mac", json!({"H": implementation("prov", "hidden")}))]),
                "mac@1's slot H names the implementation prov@1/api/hidden, which mac@1 \
                 cannot see",
            ),
        ];
        for (dependencies, expected) in cases {
            let message = refusal(&fixture.app(dependencies));
            assert!(
                message.contains(expected),
                "{message}\nexpected: {expected}"
            );
        }
        // A dangling catalog ID fails even inside a suppressed default.
        fixture.mac(implementation("nowhere", "fast"));
        let path = fixture.app(json!([uses(
            "mac",
            json!({"H": implementation("prov", "slow")})
        )]));
        assert!(refusal(&path).contains("mac@1 names \"nowhere\""));
        // A required slot without an actual.
        fixture.json(
            "mac/hirsel.json",
            json!({"schema":2,"package":{"name":"mac","version":"1"},
                "sources":[{"path":"mac.hir","module":["mac"]}],
                "parameters":{"H":{"signature":{"dependency":"self","module":["mac"],"name":"Tag"}}},
                "dependency_catalog":{},"dependencies":[]}),
        );
        let path = fixture.app(json!([uses("mac", json!({}))]));
        assert_eq!(
            refusal(&path),
            "package manifest: mac@1's slot H is required, and app@1 -> mac@1 does not fill it"
        );
        // Schema 1 declares no slots.
        fixture.json(
            "mac/hirsel.json",
            json!({"schema":1,"package":{"name":"mac","version":"1"},
                "sources":[{"path":"mac.hir","module":["mac"]}],"parameters":{},
                "dependencies":[]}),
        );
        assert!(refusal(&path).contains("unknown field parameters"));
    });
}

#[test]
fn an_incompatible_fill_bad() {
    with_compiler_stack(|| {
        let fixture = Fixture::new();
        let fill = |name: &str| {
            fixture.app(json!([uses(
                "mac",
                json!({"H": implementation("prov", name)})
            )]))
        };
        // An implementation of another signature.
        assert_eq!(
            refusal(&fill("tick")),
            "package manifest: mac@1's slot H needs an implementation of prov@1/api/Digest; \
             prov@1/api/tick implements prov@1/api/Clock"
        );
        // A declaration that is no implementation.
        assert_eq!(
            refusal(&fill("fast_bits")),
            "package manifest: mac@1's slot H names prov@1/api/fast_bits, which is not an \
             implementation"
        );
        // An implementation of the slot's signature that the matcher refuses,
        // naming the member and its rule.
        fixture.write(
            "prov/prov.hir",
            &format!(
                "{PROVIDER}\npub fn taking(take x: u64) -> u64 uses {{}} {{\n  x\n}}\n\n\
                 pub implementation taken: Digest {{\n  bits = taking\n}}\n"
            ),
        );
        let message = refusal(&fill("taken"));
        assert!(
            message.starts_with(
                "package manifest: mac@1's slot H: prov@1/api/taken does not match \
                 prov@1/api/Digest at member `bits`: [GEN-6]"
            ),
            "{message}"
        );
        // The default fill conforms.
        fixture.write("prov/prov.hir", PROVIDER);
        assert!(load_program(&fixture.app(json!([uses("mac", json!({}))]))).is_ok());
    });
}

/// What codecs, definition identity, replay and grant admission observe of a
/// program whose `mac@1` fills `H` with `actual`.
fn observed(fixture: &Fixture, actual: &str) -> (String, String, Vec<u8>, String, String) {
    let path = fixture.app(json!([uses(
        "mac",
        json!({"H": implementation("prov", actual)})
    )]));
    let mut program = load_program(&path).unwrap();
    assert_eq!(slot(&program, &mac(), "H"), prov(actual));
    let output = program.check(LoweringPolicy::Comptime).unwrap();
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let hashes = program.engine.package_definition_hashes(&mac()).unwrap();
    let hashes = format!("{:?}", hashes.hashes);
    let tag = DefPath(hirsel_syntax::ItemPath {
        package: mac(),
        modules: vec!["mac".into()],
        name: "tag".into(),
    });
    let signature = program.engine.item_signature(&tag).unwrap().value;
    let SignatureKind::Function(function) = &signature.kind else {
        unreachable!()
    };
    let engine = &mut program.engine;
    let schema = serializable_schema(
        &engine.types().clone(),
        function.result,
        &GenericDeclarations::default(),
        &mut |d| engine.item_signature(d.path()).map(|s| Some(s.value)),
    )
    .unwrap()
    .unwrap();
    let run = export_run_path(&path).unwrap();
    let grants = entry_grants_hint(&path).unwrap();
    let mut stdout = Vec::new();
    let outcome = run_path(&path, &mut stdout).unwrap();
    (
        hashes,
        format!("{schema:?}"),
        run,
        grants,
        format!("{outcome:?} {}", String::from_utf8(stdout).unwrap()),
    )
}

#[test]
fn configuration_is_invisible_to_identity_codecs_replay_and_grants() {
    with_compiler_stack(|| {
        let fixture = Fixture::new();
        let fast = observed(&fixture, "fast");
        let slow = observed(&fixture, "slow");
        assert_eq!(fast.0, slow.0, "LANG-6 definition hashes of mac@1");
        assert_eq!(fast.1, slow.1, "codec schema of mac@1/mac/Tag");
        assert_eq!(fast.2, slow.2, "run configuration and semantic revision");
        assert_eq!(fast.3, slow.3, "selecting a provider grants nothing");
        assert_eq!(fast.4, slow.4, "observable run");
    });
}

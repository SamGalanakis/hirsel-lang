//! The shared source-test fixture must publish before discarding its program.
use crate::source_std;
use hirsel_engine::*;
use hirsel_syntax::{ItemPath, PackageId};

#[test]
#[ignore = "full gate"]
fn full_gate_shared_std_fixture_reuses_bodies_modes_and_lowering() {
    with_compiler_stack(|| {
        let paths = [("u64", "to_string"), ("unicode", "graphemes_open")].map(|(module, name)| {
            DefPath(ItemPath {
                package: PackageId {
                    name: "std".into(),
                    version: "1".into(),
                },
                modules: vec![module.into()],
                name: name.into(),
            })
        });
        let mut first = source_std::engine();
        for path in &paths {
            first.lower_to_tir(path).unwrap();
        }
        drop(first);
        let mut second = source_std::engine();
        let file = second
            .sources
            .add("client.hir", "pub fn main() {}\n".into())
            .unwrap();
        let output = check_progress(&mut second, file, &default_context()).unwrap();
        assert_eq!(output.completion, CheckCompletion::Complete);
        assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
        for path in &paths {
            second.lower_to_tir(path).unwrap();
            for kind in [
                QueryKind::Signature,
                QueryKind::Resolve,
                QueryKind::Check,
                QueryKind::Modes,
                QueryKind::Lower,
            ] {
                assert_eq!(second.recompute_count(path, kind), 0, "{path:?}: {kind:?}");
            }
        }
    });
}

#[test]
fn shared_fixture_keys_limits_lowering_and_source_revision() {
    with_compiler_stack(|| {
        let dir = tempfile::tempdir().unwrap();
        let main = dir.path().join("main.hir");
        std::fs::write(&main, "").unwrap();
        std::fs::write(dir.path().join("main.hir.json"), r#"{
            "schema":1,"package":{"name":"standalone","version":"0"},
            "entry":{"module":[],"name":"main"},
            "sources":[{"path":"main.hir","module":[]}],
            "dependencies":[{"manifest":"lib.json","package":{"name":"fixture","version":"1"},"expose":[]}]
        }"#).unwrap();
        std::fs::write(
            dir.path().join("lib.json"),
            r#"{
            "schema":1,"package":{"name":"fixture","version":"1"},
            "sources":[{"path":"lib.hir","module":[]}],"dependencies":[]
        }"#,
        )
        .unwrap();
        let lib = dir.path().join("lib.hir");
        std::fs::write(
            &lib,
            "pub fn five()->u64 { 1 + 2 }\npub const VALUE:u64 = comptime five()\n",
        )
        .unwrap();
        let defaults = ComptimeLimits::default();
        for lowering in [LoweringPolicy::Comptime, LoweringPolicy::Reachable] {
            let mut first = source_std::engine_for(&main, lowering, defaults)
                .unwrap_or_else(|o| panic!("{:?}", o.diagnostics));
            let private = first
                .sources
                .add("private.hir", "fn private() {}".into())
                .unwrap();
            let second = source_std::engine_for(&main, lowering, defaults)
                .unwrap_or_else(|o| panic!("{:?}", o.diagnostics));
            assert!(second.sources.file(private).is_none());
            let low = ComptimeLimits {
                budget: 0,
                ..defaults
            };
            let rejected = match source_std::engine_for(&main, lowering, low) {
                Ok(_) => panic!("zero-budget configuration reused the checked fixture"),
                Err(output) => output,
            };
            assert!(
                rejected
                    .diagnostics
                    .iter()
                    .any(|d| d.rule.as_str() == "CT-1"),
                "{:?}",
                rejected.diagnostics
            );
        }
        // An edited dependency cannot inherit the earlier clean setup.
        std::fs::write(&lib, "pub const VALUE:u64 = missing\n").unwrap();
        assert!(source_std::engine_for(&main, LoweringPolicy::Reachable, defaults).is_err());
        std::fs::write(
            &lib,
            "pub fn five()->u64 { 1 + 2 }\npub const VALUE:u64 = comptime five()\n",
        )
        .unwrap();
        assert!(source_std::engine_for(&main, LoweringPolicy::Reachable, defaults).is_ok());
    });
}

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

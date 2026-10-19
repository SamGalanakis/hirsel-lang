//! LANG-6's precheck agreement: every std definition hashes the same whether std
//! is adopted from its precheck or checked from source. It checks std twice, so
//! it links as its own binary with its own measured request.

hirsel_guard::install!();

#[path = "../support/precheck_fixture.rs"]
mod precheck_fixture;

use hirsel_driver::*;
use hirsel_engine::*;
use precheck_fixture::*;
use std::{path::Path, sync::Arc};

/// LANG-6: a definition hash is the same whether std is adopted from its precheck
/// or checked from source, for every std definition and the program's closure.
/// The two engines are independent, so each side loads and hashes on its own
/// thread.
#[test]
fn definition_hashes_are_the_same_with_and_without_the_std_precheck() {
    let f = Fixture::new();
    let path = std::env::var_os("HIRSEL_PRECHECK")
        .expect("set HIRSEL_PRECHECK to the toolchain's std precheck");
    let precheck = Arc::new(Precheck::read(Path::new(&path)).unwrap());
    let client = f.program("aaa", STD_PROGRAM, &std_manifest(), "std");
    let hashes = |cache: PackageCache, prechecked: bool| {
        with_compiler_stack(|| {
            let mut p = load(&client, &cache);
            assert_eq!(
                p.engine.prechecked(&package("std")),
                prechecked,
                "{:?}",
                p.precheck_notes
            );
            let entry = p.entry.clone();
            let mut all = p.engine.package_definition_hashes(&package("std")).unwrap();
            all.hashes
                .extend(p.engine.definition_hashes([entry]).unwrap().hashes);
            all.hashes
        })
    };
    let (actual, expected) = std::thread::scope(|scope| {
        let actual = scope.spawn(|| hashes(PackageCache::prechecked(vec![precheck]), true));
        let expected = hashes(PackageCache::disabled(), false);
        (actual.join().unwrap(), expected)
    });
    assert!(expected.len() > 1000, "{}", expected.len());
    let differing: Vec<_> = expected
        .iter()
        .filter(|(path, hash)| actual.get(*path) != Some(*hash))
        .map(|(path, _)| path.to_string())
        .collect();
    assert!(differing.is_empty(), "{differing:?}");
    assert_eq!(actual.len(), expected.len());
}

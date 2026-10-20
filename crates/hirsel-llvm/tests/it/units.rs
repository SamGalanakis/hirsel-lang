//! Unit objects are relocatable (design §1e): a unit emitted in two programs that number
//! its layouts differently has one `ready_digest` and the same object bytes, because every
//! number the program assigns lives in the program's glue object (`hirsel_lir::glue`).

use crate::support;

use hirsel_lir::emission::{EmitInputs, OptLevel};
use hirsel_lir::layout_types::TargetSpec;
use hirsel_lir::objects::ready_digest;
use hirsel_lir::stage::InputIdentity;
use hirsel_llvm::emit_object;
use support::programs::*;
use support::*;

fn inputs<'a>(program: &'a Program, input: &'a InputIdentity, opt: OptLevel) -> EmitInputs<'a> {
    static NONE: std::collections::BTreeSet<hirsel_syntax::ContentHash> =
        std::collections::BTreeSet::new();
    EmitInputs {
        input,
        target: &program.table.target,
        layouts: &program.table.table,
        keys: &program.table.keys,
        manifest: &program.manifest,
        statics: &program.statics,
        inline_instances: &NONE,
        available_externally: &NONE,
        callbacks: false,
        tracking: false,
        opt,
        measurement: false,
        fuel: false,
    }
}

/// Every fixture program, by name, for `target`.
fn programs(target: &TargetSpec) -> Vec<(&'static str, Program)> {
    vec![
        ("scalar", scalar_program(target)),
        ("abi", abi_program(target)),
        ("heap", heap_program(target)),
        ("ready", support::ready::ready_program(target)),
        ("glue", support::glue::corpus(target).program),
    ]
}

#[test]
fn a_unit_has_one_digest_and_one_object_across_programs_that_number_its_layouts_apart() {
    let input = input();
    for target in TargetSpec::executed() {
        let plain = programs(&target);
        let shifted = with_decoys(|| programs(&target));
        for ((name, a), (_, b)) in plain.iter().zip(&shifted) {
            assert_ne!(
                a.table.table.digest(),
                b.table.table.digest(),
                "{name}: the decoys change the table"
            );
            assert_eq!(a.units.len(), b.units.len());
            for opt in [OptLevel::None, OptLevel::Speed] {
                let mut keys = std::collections::BTreeSet::new();
                for (ua, ub) in a.units.iter().zip(&b.units) {
                    let unit = &ua.unit().header.definition.0.name;
                    let (ia, ib) = (inputs(a, &input, opt), inputs(b, &input, opt));
                    let ka = ready_digest(ua.unit(), &ia, "llvm", false, &[]).unwrap();
                    let kb = ready_digest(ub.unit(), &ib, "llvm", false, &[]).unwrap();
                    assert_eq!(ka, kb, "{name} {unit} {opt:?}: the digest");
                    // Another program's front-end input: the digest binds no program.
                    let mut elsewhere = ub.unit().clone();
                    elsewhere.header.input.digest = hash("another program");
                    let kc = ready_digest(&elsewhere, &ib, "llvm", false, &[]).unwrap();
                    assert_eq!(ka, kc, "{name} {unit}: the digest of another input");
                    assert!(keys.insert(ka), "{name} {unit}: two units share a digest");
                    let oa = emit_object(&[ua], &ia).unwrap();
                    let ob = emit_object(&[ub], &ib).unwrap();
                    assert!(
                        oa.bytes == ob.bytes,
                        "{name} {unit} {opt:?} on {}: the objects differ",
                        target.triple
                    );
                    assert_eq!(oa.imports, ob.imports, "{name} {unit}");
                    assert_eq!(oa.exports, ob.exports, "{name} {unit}");
                }
            }
        }
    }
}

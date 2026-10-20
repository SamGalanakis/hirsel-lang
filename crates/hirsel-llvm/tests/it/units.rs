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

/// Debug intervals (design §1e): with intervals recorded, a unit's object still carries no
/// DWARF, every unit function has well-formed intervals naming its own definition first,
/// and the object and its intervals are the same in programs that number its layouts apart,
/// so a unit's line table survives every edit that leaves the unit alone.
#[test]
fn debug_intervals_leave_units_without_dwarf_and_bind_no_program() {
    use hirsel_lir::debug::Profile;
    use hirsel_llvm::emit_object_profile;
    let input = input();
    let profile = Profile {
        debug: true,
        sources: None,
    };
    for target in TargetSpec::executed() {
        let plain = programs(&target);
        let shifted = with_decoys(|| programs(&target));
        for ((name, a), (_, b)) in plain.iter().zip(&shifted) {
            for opt in [OptLevel::None, OptLevel::Speed] {
                let mut mapped = false;
                for (ua, ub) in a.units.iter().zip(&b.units) {
                    let unit = &ua.unit().header.definition;
                    let (ia, ib) = (inputs(a, &input, opt), inputs(b, &input, opt));
                    let oa = emit_object_profile(&[ua], &ia, profile).unwrap();
                    let ob = emit_object_profile(&[ub], &ib, profile).unwrap();
                    let what = format!("{name} {unit} {opt:?} on {}", target.triple);
                    assert!(oa.bytes == ob.bytes, "{what}: the objects differ");
                    assert_eq!(oa.debug, ob.debug, "{what}: the intervals differ");
                    let sections = section_names(&oa.bytes);
                    assert!(
                        sections.iter().all(|s| !s.starts_with(".debug")),
                        "{what}: {sections:?}"
                    );
                    let symbol =
                        hirsel_lir::emission::symbol_name(unit, Some(&ua.unit().header.instance));
                    // An optimized library unit no call in its object keeps is gone.
                    if !defined_symbols(&oa.bytes)
                        .iter()
                        .any(|s| s.starts_with(&symbol))
                    {
                        assert_ne!(opt, OptLevel::None, "{what}: no code");
                        continue;
                    }
                    // A library unit's intervals are its body's or its root adapter's.
                    let function = oa
                        .debug
                        .iter()
                        .find(|f| f.items.first() == Some(unit))
                        .unwrap_or_else(|| panic!("{what}: no intervals"));
                    assert!(function.symbol.starts_with(&symbol), "{what}");
                    assert!(function.well_formed(), "{what}: {function:?}");
                    // Optimization may merge a small function's locations into none.
                    mapped |= !function.ranges.is_empty();
                    assert!(
                        opt != OptLevel::None || !function.ranges.is_empty(),
                        "{what}: no mapped code"
                    );
                    // Without intervals, the object is the one emit_object makes.
                    let plain = emit_object(&[ua], &ia).unwrap();
                    assert!(plain.debug.is_empty(), "{what}");
                }
                assert!(mapped, "{name} {opt:?}: no unit has mapped code");
            }
        }
    }
}

fn defined_symbols(bytes: &[u8]) -> Vec<String> {
    use object::read::{Object as _, ObjectSymbol as _};
    let file = object::File::parse(bytes).expect("an object");
    file.symbols()
        .filter(|s| s.is_definition())
        .filter_map(|s| s.name().ok().map(str::to_owned))
        .collect()
}

fn section_names(bytes: &[u8]) -> Vec<String> {
    use object::read::{Object as _, ObjectSection as _};
    let file = object::File::parse(bytes).expect("an object");
    file.sections()
        .filter_map(|s| s.name().ok().map(str::to_owned))
        .collect()
}

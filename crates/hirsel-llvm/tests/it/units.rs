//! Unit objects are relocatable (design §1e): a unit emitted in two programs that number
//! its layouts differently has one `ready_digest` and the same object bytes in both
//! backends, because every number the program assigns lives in the program's glue object
//! (`hirsel_lir::glue`).

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

/// Both backends' `emit_object_profile`, by name.
type Emit = fn(
    &[&hirsel_lir::stage::ValidatedUnit<hirsel_lir::stage::Ready>],
    &EmitInputs<'_>,
    hirsel_lir::debug::Profile<'_>,
) -> Result<hirsel_lir::stage::ObjectArtifact, String>;
const BACKENDS: [(&str, Emit); 2] = [
    ("llvm", |u, i, p| {
        hirsel_llvm::emit_object_profile(u, i, p).map_err(|e| e.to_string())
    }),
    ("cranelift", |u, i, p| {
        hirsel_cranelift::emit_object_profile(u, i, p).map_err(|e| e.to_string())
    }),
];

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
                    for (backend, emit) in BACKENDS {
                        let oa = emit(&[ua], &ia, Default::default()).unwrap();
                        let ob = emit(&[ub], &ib, Default::default()).unwrap();
                        let what = format!("{backend} {name} {unit} {opt:?} on {}", target.triple);
                        assert!(oa.bytes == ob.bytes, "{what}: the objects differ");
                        assert_eq!(oa.imports, ob.imports, "{what}");
                        assert_eq!(oa.exports, ob.exports, "{what}");
                    }
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
    let input = input();
    let profile = Profile {
        debug: true,
        sources: None,
    };
    for target in TargetSpec::executed() {
        let plain = programs(&target);
        let shifted = with_decoys(|| programs(&target));
        for ((name, a), (_, b)) in plain.iter().zip(&shifted) {
            for (backend, emit) in BACKENDS {
                for opt in [OptLevel::None, OptLevel::Speed] {
                    let mut mapped = false;
                    for (ua, ub) in a.units.iter().zip(&b.units) {
                        let unit = &ua.unit().header.definition;
                        let (ia, ib) = (inputs(a, &input, opt), inputs(b, &input, opt));
                        let oa = emit(&[ua], &ia, profile).unwrap();
                        let ob = emit(&[ub], &ib, profile).unwrap();
                        let what = format!("{backend} {name} {unit} {opt:?} on {}", target.triple);
                        assert!(oa.bytes == ob.bytes, "{what}: the objects differ");
                        assert_eq!(oa.debug, ob.debug, "{what}: the intervals differ");
                        let sections = section_names(&oa.bytes);
                        assert!(
                            sections.iter().all(|s| !s.starts_with(".debug")),
                            "{what}: {sections:?}"
                        );
                        let symbol = hirsel_lir::emission::symbol_name(
                            unit,
                            Some(&ua.unit().header.instance),
                        );
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
                        // Without intervals, the backend records none.
                        let plain = emit(&[ua], &ia, Profile::default()).unwrap();
                        assert!(plain.debug.is_empty(), "{what}");
                    }
                    assert!(mapped, "{backend} {name} {opt:?}: no unit has mapped code");
                }
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

/// A unit's fragment (`hirsel_lir::fragment`) encoded in one program decodes, bound to a
/// program that numbers its layouts apart, to the fragment that program makes itself.
#[test]
fn a_fragment_binds_into_a_program_that_numbers_its_layouts_apart() {
    use hirsel_lir::fragment::{UnitFragment, UnitSummary};
    use std::collections::HashMap;
    let input = input();
    for target in TargetSpec::executed() {
        let plain = programs(&target);
        let shifted = with_decoys(|| programs(&target));
        for ((name, a), (_, b)) in plain.iter().zip(&shifted) {
            let (ia, ib) = (
                inputs(a, &input, OptLevel::None),
                inputs(b, &input, OptLevel::None),
            );
            let layouts: HashMap<_, _> = ib
                .keys
                .all()
                .iter()
                .enumerate()
                .map(|(i, k)| (*k, hirsel_lir::layout_types::LayoutId(i as u32)))
                .collect();
            let abis: HashMap<_, _> = ib
                .layouts
                .fn_abis
                .iter()
                .enumerate()
                .map(|(i, abi)| {
                    (
                        ib.keys.fn_abi(abi),
                        hirsel_lir::abi_types::FnAbiId(i as u32),
                    )
                })
                .collect();
            for (ua, ub) in a.units.iter().zip(&b.units) {
                let what = format!(
                    "{name} {} on {}",
                    ua.unit().header.definition,
                    target.triple
                );
                let fragment =
                    |unit: &hirsel_lir::stage::ValidatedUnit<_>, i: &EmitInputs<'_>| UnitFragment {
                        summary: UnitSummary::of(unit.unit(), i.layouts, i.keys, i.manifest)
                            .unwrap(),
                        object: emit_object(&[unit], i).unwrap(),
                    };
                let (fa, fb) = (fragment(ua, &ia), fragment(ub, &ib));
                let bytes = fa.encode(ia.layouts, ia.keys).unwrap();
                assert_eq!(bytes, fb.encode(ib.layouts, ib.keys).unwrap(), "{what}");
                let decoded = UnitFragment::decode(&bytes, &|k| layouts.get(k).copied(), &|k| {
                    abis.get(k).copied()
                })
                .unwrap_or_else(|e| panic!("{what}: {e}"));
                assert_eq!(decoded, fb, "{what}");
            }
        }
    }
}

/// A resource drop body can be reached only by a descriptor in the separately
/// emitted glue object. Its template must survive even with no call in this object.
#[test]
fn an_optimized_library_finalizer_remains_defined_for_separate_glue() {
    use hirsel_lir::emission::symbol_name;
    use hirsel_lir::ir::UnitOrigin;
    let input = input();
    for target in TargetSpec::executed() {
        let program = support::ready::ready_program(&target);
        let mut raw = program
            .units
            .iter()
            .find(|u| u.unit().header.definition.0.name == "Res_drop")
            .unwrap()
            .unit()
            .clone();
        raw.header.origin = UnitOrigin::Library;
        let name = symbol_name(&raw.header.definition, Some(&raw.header.instance));
        let unit = support::seal(&program.table, &program.manifest, raw);
        for opt in [OptLevel::None, OptLevel::Speed] {
            let object = emit_object(&[&unit], &inputs(&program, &input, opt)).unwrap();
            let symbols =
                hirsel_lir::object::ObjectFile::read(&target, object.bytes.clone()).unwrap();
            assert!(
                symbols.exports.contains(&name),
                "{} {opt:?}: missing {name}",
                target.triple
            );
        }
    }
}

//! Flat constructors and constants through import, placement, RC and independent
//! heap execution. Every allocation/failure site runs its cleanup audit too.
use crate::rc_support as support;
use hirsel_heapcheck::{Config, NoOperations, Observed};
use hirsel_lir::validate::Inputs;
use hirsel_rc::instance::InstanceId;
use support::*;

const TYPES: &str = "flat type Item { text: String, n: u64 }\nflat type Items { items: List[Item] }\nfn checksum(xs: Items) -> u64 { let mut sum: u64 = 0\n for x in xs.items { sum += x.n + string.byte_len(x.text) }\n sum }\n";

pub(super) fn audited(source: &str, expected: u128) {
    let input = compile(source);
    let lowered = hirsel_rc::import::lower(&input, &target()).unwrap();
    let operations = lowered.operation_registry(&input);
    let problems = violations(&input, &lowered);
    assert!(
        problems.is_empty(),
        "{problems:#?}\n{}",
        lowered
            .units
            .iter()
            .map(dump)
            .collect::<Vec<_>>()
            .join("\n")
    );
    for regions in [false, true] {
        let mut units = Vec::new();
        for (i, unit) in lowered.units.iter().enumerate() {
            let id = InstanceId(i as u32);
            let placed = hirsel_rc::placement::place(
                unit.clone(),
                &lowered.table,
                &hirsel_rc::placement::Options::for_instance(regions, &lowered.instances, id),
            )
            .unwrap();
            let rc = hirsel_rc::insert::insert(
                placed,
                &lowered.table,
                &hirsel_rc::insert::Proofs::for_instance(&lowered.instances, id),
            )
            .unwrap();
            let rc = hirsel_rc::elide::elide(rc, &lowered.table).unwrap();
            let problems = hirsel_lir::validate::check(
                &rc,
                &Inputs {
                    memo: Default::default(),
                    discharge: None,
                    operations: &operations,
                    target: &target(),
                    layouts: &lowered.table,
                    manifest: &manifest(&input),
                },
            );
            assert!(problems.is_empty(), "{problems:?}\n{}", dump(&rc));
            units.push(rc);
        }
        let program = hirsel_heapcheck::Program {
            invocation_bodies: operations.invocation_bodies.clone(),
            invocations: operations.invocations.clone(),
            pretransfer_adapters: operations.pretransfer_adapters.clone(),
            target: target(),
            layouts: lowered.table.clone(),
            units,
            statics: lowered.statics.clone(),
        };
        let campaign = hirsel_heapcheck::fault_campaign(
            &program,
            &path("main"),
            &[],
            &mut || Box::new(NoOperations),
            &Config::default(),
        );
        assert!(
            campaign.first_violation().is_none(),
            "{source}\n{:?}",
            campaign.first_violation()
        );
        assert_eq!(
            campaign.clean.result.unwrap().observation.returned,
            Some(Observed::Integer {
                scalar: hirsel_lir::layout_types::ScalarKind::U64,
                bits: expected
            })
        );
    }
}

#[test]
fn constructing_a_flat_sequence_reads_and_releases_its_source() {
    audited(
        &format!(
            "{TYPES}fn main() -> u64 {{\n let source: List[Item] = [Item(text: \"\", n: 3), Item(text: \"abc\", n: 7)]\n let xs = Items(items: copy source)\n checksum(xs) + list.len(source)\n }}"
        ),
        15,
    );
}

#[test]
fn static_variable_flat_data_and_its_explicit_copy_walk() {
    audited(
        &format!(
            "{TYPES}const DATA: Items = comptime Items(items: [Item(text: \"\", n: 3), Item(text: \"abc\", n: 7)])\nfn main() -> u64 {{ let xs = DATA\n let ys = copy xs\n checksum(xs) + checksum(ys) }}"
        ),
        26,
    );
}

#[test]
fn empty_flat_sequences_keep_their_count_and_extent() {
    audited(
        &format!(
            "{TYPES}fn main() -> u64 {{ let xs = Items(items: [])\n checksum(xs) + list.len(xs.items) }}"
        ),
        0,
    );
}

#[test]
fn flat_sequences_encode_byte_slices_and_nested_lists() {
    audited(
        "flat type Texts { words: List[String], nested: List[List[u64]] }\nfn total(xs: Texts) -> u64 {\n let mut sum: u64 = 0\n for word in xs.words { sum += string.byte_len(word) }\n for values in xs.nested { for n in values { sum += n } }\n sum\n}\nfn main() -> u64 { let xs = Texts(words: [\"\", \"abc\", \"d\"], nested: [[2u64, 3u64], [], [7u64]])\n total(xs) }",
        16,
    );
}

#[test]
fn the_native_flat_conformance_programs_pass_import_validation() {
    for source in [
        include_str!("../../../../conformance/MEM/MEM-12-flat-native-parameter.hir"),
        include_str!("../../../../conformance/MEM/MEM-12-flat-native-walk.hir"),
        include_str!("../../../../conformance/MEM/MEM-12-flat-native-sequences.hir"),
    ] {
        let program = compile(source);
        let lowered = hirsel_rc::import::lower(&program, &target()).unwrap();
        assert!(
            violations(&program, &lowered).is_empty(),
            "{:?}\n{}",
            violations(&program, &lowered),
            lowered
                .units
                .iter()
                .map(dump)
                .collect::<Vec<_>>()
                .join("\n")
        );
    }
}

#[test]
fn stable_owned_flat_sequence_iteration_does_not_materialize_elements() {
    use hirsel_lir::ir::Inst;
    let source = format!(
        "{TYPES}fn main() -> u64 {{ let xs = Items(items: [Item(text: \"abc\", n: 7)])\n let mut sum: u64 = 0\n for x in xs.items {{ sum += x.n + string.byte_len(x.text) }}\n sum }}"
    );
    let input = compile(&source);
    let lowered = hirsel_rc::import::lower(&input, &target()).unwrap();
    assert!(violations(&input, &lowered).is_empty());
    let main = unit_named(&lowered, "main");
    let copies = main
        .instructions
        .iter()
        .filter(|i| matches!(i, Inst::FlatCopy { .. }))
        .count();
    assert_eq!(
        copies,
        0,
        "read-only iteration decoded an owned element\n{}",
        dump(main)
    );
    assert_eq!(
        main.instructions
            .iter()
            .filter(|i| matches!(i, Inst::FlatNext { .. }))
            .count(),
        1
    );
    audited(&source, 10);
}

#[test]
fn owned_flat_sequence_snapshots_keep_materialization_at_ordinary_boundaries() {
    use hirsel_lir::ir::Inst;
    let prefix = format!(
        "{TYPES}fn total(items: List[Item]) -> u64 {{ let mut sum = 0u64\n for x in items {{ sum += x.n }}\n sum }}\n"
    );
    for (body, expected) in [
        (
            "let mut xs = Items(items: [Item(text: \"a\", n: 7)])\n let saved = copy xs.items\n xs = Items(items: [Item(text: \"b\", n: 31)])\n let mut sum = 0u64\n for x in saved { sum += x.n }\n sum",
            7,
        ),
        (
            "let xs = Items(items: [Item(text: \"a\", n: 7)])\n let saved = copy xs.items\n total(saved)",
            7,
        ),
        (
            "let saved = { let xs = Items(items: [Item(text: \"a\", n: 7)])\n copy xs.items }\n let mut sum = 0u64\n for x in saved { sum += x.n }\n sum",
            7,
        ),
        (
            "let xs = Items(items: [Item(text: \"a\", n: 7)])\n let mut saved = copy xs.items\n list.reserve(mut saved, 0)\n total(saved)",
            7,
        ),
        (
            "let xs: List[Items] = [Items(items: [Item(text: \"a\", n: 7)]), Items(items: [Item(text: \"b\", n: 31)])]\n let mut i = 0u64\n let saved = copy xs[i].items\n i = 1\n let mut sum = 0u64\n for x in saved { sum += x.n }\n sum + i",
            8,
        ),
    ] {
        let source = format!("{prefix}fn main() -> u64 {{ {body} }}");
        let input = compile(&source);
        let lowered = hirsel_rc::import::lower(&input, &target()).unwrap();
        assert!(violations(&input, &lowered).is_empty());
        let main = unit_named(&lowered, "main");
        assert!(
            main.instructions
                .iter()
                .any(|i| matches!(i, Inst::FlatCopy { .. })),
            "snapshot lost its ordinary storage\n{}",
            dump(main)
        );
        audited(&source, expected);
    }
}

#[test]
fn an_owned_flat_source_initialized_in_a_loop_is_not_a_frame_constant() {
    use hirsel_lir::ir::SlotKind;
    use hirsel_lir::layout_types::LayoutKind;
    let source = format!(
        "{TYPES}fn main() -> u64 {{ let mut sum = 0u64\n for i in 0u64..3u64 {{ let xs = Items(items: [Item(text: \"a\", n: i)])\n for x in xs.items {{ sum += x.n }} }}\n sum }}"
    );
    let input = compile(&source);
    let lowered = hirsel_rc::import::lower(&input, &target()).unwrap();
    assert!(violations(&input, &lowered).is_empty());
    let main = unit_named(&lowered, "main");
    let constants = hirsel_lir::statics::constant_slots(main, &lowered.statics);
    let owned_flat: Vec<_> = main.slots.iter().enumerate().filter(|(_, slot)| {
        slot.kind == SlotKind::Local
            && matches!(lowered.table.layouts[main.types[slot.ty.index()].layout.0 as usize].kind,
                LayoutKind::Block { payload } if lowered.table.layouts[payload.0 as usize].kind == LayoutKind::Flat)
    }).collect();
    assert!(!owned_flat.is_empty());
    assert!(
        owned_flat
            .iter()
            .all(|(slot, _)| constants[*slot].is_none())
    );
    audited(&source, 3);
    super::rc_library_views::four_engines(
        format!(
            "{}\nfn output_error[E](take e: E) -> () {{}}\nfn grant_error(take e: GrantError) -> () {{}}\npub fn main(rt: Io, args: Args) -> Result[(), ()] {{ out.print(io.stdout(rt) ? grant_error, if value() == 3 {{ \"ok\\n\" }} else {{ \"bad\\n\" }}) ? output_error\n Ok(()) }}",
            source.replace("fn main() -> u64", "fn value() -> u64")
        ),
        "flat-loop-source-54",
    );
}

#[test]
fn stable_owned_flat_walks_keep_their_version_through_conditional_reads() {
    let source = format!(
        "{TYPES}fn main() -> u64 {{ let xs = Items(items: [Item(text: \"abc\", n: 7)])\n let mut sum = 0u64\n for x in xs.items {{ sum += x.n + string.byte_len(x.text)\n if x.n == 7 {{ let mut changed = copy x\n changed.n += 3\n sum += changed.n + string.byte_len(changed.text) }} }}\n sum }}"
    );
    audited(&source, 23);
}

/// Materializing `unique` from a flat encoding owns independent storage:
/// the borrowed source is still readable, then dies before the result is used.
#[test]
fn unique_flat_field_materialization_survives_its_carrier() {
    for (field, ty, value, measure, expected) in [
        ("title", "String", "\"abc\"", "string.byte_len(owned)", 3),
        ("data", "Bytes", "b\"abcd\"", "bytes.len(owned)", 4),
        (
            "words",
            "List[String]",
            "[\"abc\", \"d\"]",
            "string.byte_len(owned[0]) + string.byte_len(owned[1])",
            4,
        ),
        (
            "nested",
            "List[List[u64]]",
            "[[2u64, 3u64], [], [7u64]]",
            "owned[0][0] + owned[0][1] + owned[2][0]",
            12,
        ),
    ] {
        for copy in ["", "copy "] {
            audited(
                &format!(
                    "flat type Item {{ text: String, n: u64 }}\nflat type Fields {{ {field}: {ty} }}\ntype Row {{ fields: Fields }}\nfn detach(row: Row) -> unique {ty} {{ let owned = unique {copy}row.fields.{field}\n let _ = row.fields.{field}\n take owned }}\nfn main() -> u64 {{ let owned = {{ let row = Row(fields: Fields({field}: {value}))\n detach(row) }}\n {measure} }}"
                ),
                expected,
            );
        }
    }
    audited(
        "flat type Item { text: String, n: u64 }\nflat type Fields { part: Item }\ntype Row { fields: Fields }\nfn detach(row: Row) -> unique Item { unique row.fields.part }\nfn main() -> u64 { let owned = { let row = Row(fields: Fields(part: Item(text: \"abc\", n: 7)))\n detach(row) }\n string.byte_len(owned.text) + owned.n }",
        10,
    );
}

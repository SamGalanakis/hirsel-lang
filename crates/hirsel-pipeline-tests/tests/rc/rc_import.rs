//! SSA import of real driver-derived programs, each output checked by the
//! hirsel-lir stage validators.
use crate::rc_support as support;

use support::*;

#[test]
fn borrowed_box_key_reads_do_not_acquire_holders() {
    let source = r#"
type Word { key: Bytes, count: u64, next: u64 }
fn equal(nodes: List[box Word], index: u64, token: Bytes) -> bool {
  nodes[index].key == token
}
fn next(nodes: List[box Word], index: u64) -> u64 { nodes[index].next }
fn main() -> bool { equal([box Word(key: b"key", count: 1, next: 0)], 0, b"key") }
"#;
    let (program, lowered) = lower_valid(source);
    for name in ["equal", "next"] {
        let unit = unit_named(&lowered, name);
        assert!(
            unit.instructions
                .iter()
                .all(|i| !matches!(i, hirsel_lir::ir::Inst::Copy { .. })),
            "{}",
            dump(unit)
        );
    }
    // Insertion and elision must keep those views borrowed too. A Value-stage
    // read alone does not prove that the executable acquires no holder.
    for regions in [false, true] {
        let mut options = hirsel_driver::native::Options::new(target());
        options.regions = regions;
        let built = hirsel_driver::native::build_program(&program, manifest(&program), &options)
            .expect("build borrowed reads");
        let mut checked = 0;
        for unit in &built.units {
            let unit = unit.unit();
            if ["equal", "next"].contains(&unit.header.definition.0.name.as_str()) {
                checked += 1;
                assert!(
                    unit.instructions.iter().all(|i| !matches!(
                        i,
                        hirsel_lir::ir::Inst::Dup { .. } | hirsel_lir::ir::Inst::Release { .. }
                    )),
                    "{}",
                    dump(unit)
                );
            }
        }
        assert_eq!(checked, 2);
    }
}

#[test]
fn decimal_rescale_reuses_disposal_storage_from_an_earlier_block() {
    lower_valid(
        "fn main() -> Result[(), ()] { let v = std@1/decimal.new(std@1/bigint.zero(), 1); let _ = std@1/decimal.rescale(v, 0, HalfUp()); Ok(()) }",
    );
}

#[test]
fn markdown_render_reuses_disposal_storage_from_an_earlier_block() {
    lower_valid(
        "fn main() -> Result[(), ()] { let rendered = std@1/markdown.to_html(\"# Hello\", std@1/markdown.default_options()); failure.check(string.contains(rendered, \"<h1>Hello</h1>\"), TextInvariant()); Ok(()) }",
    );
}

#[test]
fn a_taken_indexed_callee_releases_before_later_borrowed_arguments() {
    lower_valid(include_str!(
        "../../../../conformance/CLO/CLO-2-nested-uses-annotation-run.hir"
    ));
}

/// A later pass starts the loop head from the previous pass's states, which
/// already name the deep copy's synthetic loan: the copy's failure edge ends
/// that loan, not one appended past it.
#[test]
fn a_deep_copy_in_a_loop_with_an_early_return_ends_its_loan_on_failure() {
    for source in [
        include_str!(
            "../../../../conformance/MEM/MEM-5-unique-extraction-loop-conditional-return-run.hir"
        ),
        include_str!(
            "../../../../conformance/MEM/MEM-5-unique-extraction-loop-early-return-run.hir"
        ),
        include_str!("../../../../conformance/MEM/MEM-5-unique-extraction-loop-field-call-run.hir"),
    ] {
        lower_valid(source);
    }
}

/// A `for` loop's element, matched on, is a view of `xs[i]` that its counter
/// does not move while the view lives: its reads go to the element in place
/// rather than to storage holding a borrow without credit.
#[test]
fn a_loop_element_matched_on_is_read_in_place() {
    lower_valid(include_str!(
        "../../../../conformance/LANG/LANG-13-loop-element-match-run.hir"
    ));
}

/// `channel.send` prepares its carrier's slots at entry, where the taken
/// message is still held: each preparation's failure releases it.
#[test]
fn a_library_slot_prepared_at_entry_releases_a_taken_parameter() {
    lower_valid(include_str!(
        "../../../../conformance/CONC/CONC-6-owned-record-payload-run.hir"
    ));
}

#[test]
fn failure_preparing_the_second_taken_argument_disposes_the_first() {
    let (_, lowered) = lower_valid(
        "fn pair(take a: String, take b: String) -> u64 { string.byte_len(a) + string.byte_len(b) }\nfn main() -> u64 { let xs: List[String] = [std@1/u64.to_string(1u64)]; let s = std@1/u64.to_string(2u64); pair(copy s, copy xs[1u64]) }",
    );
    let main = unit_named(&lowered, "main");
    // The element's bounds check can fail while the first argument's copy is
    // owned; the validator's exact credit replay proves that failure path
    // releases it.
    assert!(
        failure_edges(main)
            .iter()
            .any(|(rule, _)| rule.as_deref() == Some("MEM-14")),
        "{}",
        dump(main)
    );
}

#[test]
fn return_extracts_its_value_before_cleanup() {
    let (_, lowered) = lower_valid(
        "resource type R {n:u64} drop(x) {}\nfn keep(take r: R, n: u64) -> String { std@1/u64.to_string(n + r.n) }\nfn main() -> u64 { string.byte_len(keep(R(n: 1u64), 2u64)) }",
    );
    let keep = unit_named(&lowered, "keep");
    // The resource is dropped in a Cleanup block after the result was read,
    // and a failing finalizer disposes of the extracted result.
    assert!(
        keep.blocks
            .iter()
            .any(|b| b.kind == hirsel_lir::ir::BlockKind::Cleanup),
        "{}",
        dump(keep)
    );
    assert!(
        failure_edges(keep).iter().any(|(_, carried)| *carried == 1),
        "{}",
        dump(keep)
    );
}

#[test]
fn replace_installs_before_the_displaced_drop() {
    let (_, lowered) = lower_valid(
        "fn swap() -> u64 { let mut s = std@1/u64.to_string(1u64); s = std@1/u64.to_string(22u64); string.byte_len(s) }\nfn main() -> u64 { swap() }",
    );
    let text = dump(unit_named(&lowered, "swap"));
    let replace = text.find("Replace").expect("a Replace");
    let drop = text[replace..]
        .find("drop")
        .expect("a displaced drop after it");
    assert!(drop > 0, "{text}");
}

#[test]
fn short_circuit_and_branch_local_ownership() {
    lower_valid(
        "fn pick(a: bool, b: bool) -> u64 { let mut n = 0u64; if a && b { let s = std@1/u64.to_string(3u64); n = string.byte_len(s) } else if a || b { n = 2u64 }; n }\nfn main() -> u64 { pick(true, false) }",
    );
}

#[test]
fn question_mark_propagates_through_cleanup() {
    lower_valid(
        "fn half(n: u64) -> Result[u64, u64] { if n == 0u64 { Error(1u64) } else { Ok(n) } }\nfn twice(n: u64) -> Result[u64, u64] { let s = std@1/u64.to_string(n); let h = half(n)?; Ok(h + string.byte_len(s)) }\nfn main() -> u64 { match twice(2u64) { Ok(v) => v, Error(e) => e } }",
    );
}

#[test]
fn arbitrary_loops_join_versions_and_ownership() {
    let (_, lowered) = lower_valid(
        "fn count(n: u64) -> u64 { let mut i = 0u64; let mut t = 0u64; let mut s = std@1/u64.to_string(0u64); while i < n { if i == 3u64 { i = i + 1u64; continue }; s = std@1/u64.to_string(i); t = t + string.byte_len(s); if t > 100u64 { break }; i = i + 1u64 }; t }\nfn main() -> u64 { count(10u64) }",
    );
    let count = unit_named(&lowered, "count");
    // Merge blocks name each slot's reaching version through parameters.
    assert!(
        count.blocks.iter().any(|b| b.parameters.len >= 2),
        "{}",
        dump(count)
    );
}

#[test]
fn match_tables_become_ordered_switches_with_guard_continuations() {
    let (_, lowered) = lower_valid(
        "type Fig = Circle(u64) | Rect(u64, u64)\nfn area(s: Fig) -> u64 { match s { Circle(r) if r > 2u64 => r * r, Circle(r) => r, Rect(w, h) => w * h } }\nfn main() -> u64 { area(Rect(2u64, 3u64)) + area(Circle(3u64)) }",
    );
    let area = unit_named(&lowered, "area");
    let switches = area
        .terminators
        .iter()
        .filter(|t| matches!(t, hirsel_lir::ir::Terminator::Switch { .. }))
        .count();
    // Arms 0 and 1 are tested from the start. After arm 0's guard fails,
    // its tag is still proved: arm 1 enters without another switch or fallback.
    assert_eq!(switches, 2, "{}", dump(area));
}

/// A match arm's fields are prepared once, in the block that binds them, where the
/// arm's tag test proved the variant (here by elimination: the last arm is untested).
/// Each read of a field reuses that address, so no tag check follows the first call.
#[test]
fn a_proven_arm_prepares_its_fields_where_it_binds_them() {
    use hirsel_lir::ir::{Inst, ProjectionStep};
    let (_, lowered) = lower_valid(
        "type Tree = Leaf | Branch(left: box Tree, right: box Tree)\nfn check(t: box Tree) -> u64 { match t { Leaf() => 1u64, Branch(left: l, right: r) => 1u64 + check(l) + check(r) } }\nfn main() -> u64 { check(box Branch(left: box Leaf(), right: box Leaf())) }",
    );
    let check = unit_named(&lowered, "check");
    let variant_field = |address: hirsel_lir::ir::AddressId| {
        check.addresses[address.index()]
            .projection
            .get(&check.pools.projections)
            .unwrap()
            .iter()
            .any(|s| matches!(s, ProjectionStep::VariantField { .. }))
    };
    let mut binds = Vec::new();
    let mut prepared = Vec::new();
    let mut read = Vec::new();
    for (b, block) in check.blocks.iter().enumerate() {
        for inst in block.instructions.get(&check.instructions).unwrap() {
            match *inst {
                Inst::BindFields { .. } => binds.push(b),
                Inst::Address { address, .. } if variant_field(address) => {
                    prepared.push((b, address))
                }
                Inst::Read { address, .. } if variant_field(address) => read.push(address),
                _ => {}
            }
        }
    }
    assert_eq!(binds.len(), 1, "{}", dump(check));
    assert_eq!(prepared.len(), 2, "{}", dump(check));
    assert!(
        prepared.iter().all(|&(b, _)| b == binds[0]),
        "{}",
        dump(check)
    );
    let addresses: Vec<_> = prepared.iter().map(|&(_, a)| a).collect();
    assert_eq!(read, addresses, "{}", dump(check));
}

#[test]
fn a_counted_scrutinee_is_tested_through_a_view() {
    lower_valid(
        "fn size(o: Option[String]) -> u64 { match o { Some(s) => string.byte_len(s), None => 0u64 } }\nfn main() -> u64 { size(Some(std@1/u64.to_string(5u64))) }",
    );
}

#[test]
fn liveness_starts_at_definitions_and_covers_failure_only_uses() {
    use hirsel_lir::ir::{Inst, Terminator};
    use hirsel_rc::liveness::{liveness, terminator_defs};
    let (_, lowered) = lower_valid(
        "resource type R {n:u64} drop(x) {}\nfn keep(take r: R, n: u64) -> String { std@1/u64.to_string(n + r.n) }\nfn count(n: u64) -> u64 { let mut i = 0u64; let mut t = 0u64; while i < n { t = t + i; i = i + 1u64 }; t }\nfn main() -> u64 { string.byte_len(keep(R(n: 1u64), 2u64)) + count(3u64) }",
    );
    for unit in &lowered.units {
        let l = liveness(unit);
        // Only what the unit receives is live at its entry.
        let received: Vec<_> = unit
            .parameters
            .iter()
            .chain(&unit.captures)
            .copied()
            .collect();
        for v in l.live_in[unit.entry.index()].iter() {
            assert!(
                received.contains(&v),
                "v{} live before its definition\n{}",
                v.0,
                dump(unit)
            );
        }
        // Every value defined somewhere is defined once (SSA) and every live-in
        // value is defined: by an instruction, a terminator or a parameter.
        let mut defined = received.clone();
        for (b, block) in unit.blocks.iter().enumerate() {
            defined.extend(block.parameters.get(&unit.pools.values).unwrap_or(&[]));
            for inst in block.instructions.get(&unit.instructions).unwrap_or(&[]) {
                defined.extend(Inst::defs(inst));
            }
            defined.extend(terminator_defs(unit, &unit.terminators[b]));
        }
        for set in &l.live_in {
            for v in set.iter() {
                assert!(defined.contains(&v), "v{} live but never defined", v.0);
            }
        }
    }
    // keep's result is read before the resource's cleanup drop; only that
    // drop's failure edge (disposing the result) still uses it after the read.
    let keep = unit_named(&lowered, "keep");
    let l = liveness(keep);
    let (b, ret) = keep
        .terminators
        .iter()
        .enumerate()
        .find_map(|(b, t)| match t {
            Terminator::Return(Some(v)) => Some((b, *v)),
            _ => None,
        })
        .unwrap();
    let insts = keep.blocks[b].instructions.get(&keep.instructions).unwrap();
    let drop = insts
        .iter()
        .position(|i| matches!(i, hirsel_lir::ir::Inst::Drop { .. }))
        .expect("the cleanup drop before the return");
    let hirsel_lir::ir::Inst::Drop { failure, .. } = insts[drop] else {
        unreachable!()
    };
    let edge = &keep.tables.edges[keep.tables.failures[failure.index()].cleanup.index()];
    assert!(
        edge.arguments
            .get(&keep.pools.values)
            .unwrap()
            .contains(&ret),
        "{}",
        dump(keep)
    );
    let block = hirsel_lir::ir::BlockId(b as u32);
    let before = if drop == 0 {
        l.live_in[b].contains(ret)
    } else {
        l.live_after(keep, block, drop - 1, ret)
    };
    assert!(before, "{}", dump(keep));
}

/// Compare with the committed dump; `HIRSEL_BLESS=1` rewrites it.
fn golden(name: &str, text: &str) {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/rc/fixtures")
        .join(format!("{name}.lir.txt"));
    if std::env::var_os("HIRSEL_BLESS").is_some() {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, text).unwrap();
        return;
    }
    let expected = std::fs::read_to_string(&path)
        .unwrap_or_else(|_| panic!("missing {}; run with HIRSEL_BLESS=1", path.display()));
    assert_eq!(
        expected,
        text,
        "{} changed; rerun with HIRSEL_BLESS=1 if intended",
        path.display()
    );
}

const FIXTURES: &[(&str, &str)] = &[
    (
        "self-tail-sum",
        "fn sum(n: u64, total: u64) -> u64 { if n == 0u64 { total } else { sum(n - 1u64, total + n) } }\nfn main() -> u64 { sum(10u64, 0u64) }",
    ),
    (
        "borrowed-temporary-loop",
        "fn descend(n:u64,text:String)->u64 { if n == 0u64 { string.byte_len(text) } else { descend(n-1u64,std@1/u64.to_string(n)) } }\nfn main()->u64 {descend(3u64,std@1/u64.to_string(7u64))}",
    ),
    (
        "match-guard",
        "type Fig = Circle(u64) | Rect(u64, u64)\nfn area(s: Fig) -> u64 { match s { Circle(r) if r > 2u64 => r * r, Circle(r) => r, Rect(w, h) => w * h } }\nfn main() -> u64 { area(Rect(2u64, 3u64)) }",
    ),
    (
        "return-before-cleanup",
        "resource type R {n:u64} drop(x) {}\nfn keep(take r: R, n: u64) -> String { std@1/u64.to_string(n + r.n) }\nfn main() -> u64 { string.byte_len(keep(R(n: 1u64), 2u64)) }",
    ),
    (
        "question-mark",
        "fn half(n: u64) -> Result[u64, u64] { if n == 0u64 { Error(1u64) } else { Ok(n) } }\nfn twice(n: u64) -> Result[u64, u64] { let s = std@1/u64.to_string(n); let h = half(n)?; Ok(h + string.byte_len(s)) }\nfn main() -> u64 { match twice(2u64) { Ok(v) => v, Error(e) => e } }",
    ),
];

#[test]
fn dumps_match_their_fixtures() {
    for (name, source) in FIXTURES {
        let (_, lowered) = lower_valid(source);
        // The golden describes this fixture; lower_valid checks dependency units too.
        let package = path("main").0.package;
        let text: Vec<String> = lowered
            .units
            .iter()
            .filter(|unit| unit.header.definition.0.package == package)
            .map(dump)
            .collect();
        golden(name, &format!("{source}\n\n{}", text.join("\n")));
    }
}

/// One validated program, mutated in ways each obligation must catch.
#[test]
fn a_mutated_output_is_rejected() {
    use hirsel_lir::ir::{Inst, Terminator};
    let source = FIXTURES[3].1;
    let (program, lowered) = lower_valid(source);
    let id = instances_of(&lowered, "keep")[0];
    type Mutation = fn(&mut hirsel_lir::ir::LirUnit<hirsel_lir::stage::Value>) -> bool;
    let mutations: &[(&str, Mutation)] = &[
        ("drop the first cleanup drop", |u| {
            match u
                .instructions
                .iter()
                .position(|i| matches!(i, Inst::Drop { .. }))
            {
                Some(i) => {
                    let Inst::Drop { address, .. } = u.instructions[i] else {
                        unreachable!()
                    };
                    // A read of nothing: keep the row count, lose the release.
                    u.instructions[i] = Inst::EndOrigin { place: address };
                    true
                }
                None => false,
            }
        }),
        ("read a stale version", |u| {
            let reads: Vec<usize> = u
                .instructions
                .iter()
                .enumerate()
                .filter(|(_, i)| matches!(i, Inst::Read { .. }))
                .map(|(k, _)| k)
                .collect();
            let [a, b, ..] = reads[..] else { return false };
            let Inst::Read { reaching, .. } = u.instructions[a] else {
                unreachable!()
            };
            if let Inst::Read { reaching: r, .. } = &mut u.instructions[b] {
                if *r == reaching {
                    return false;
                }
                *r = reaching;
            }
            true
        }),
        ("return from a failure cleanup", |u| {
            let Some(b) = u
                .blocks
                .iter()
                .position(|b| b.kind == hirsel_lir::ir::BlockKind::FailureCleanup)
            else {
                return false;
            };
            u.terminators[b] = Terminator::Return(None);
            true
        }),
        ("forget a failure edge's disposal", |u| {
            let Some(f) = u
                .tables
                .failures
                .iter()
                .position(|f| u.tables.edges[f.cleanup.index()].arguments.len > 0)
            else {
                return false;
            };
            let e = u.tables.failures[f].cleanup.index();
            u.tables.edges[e].arguments.len -= 1;
            true
        }),
    ];
    let manifest = manifest(&program);
    let target = target();
    for (what, mutate) in mutations {
        let mut unit = lowered.units[id.index()].clone();
        assert!(
            mutate(&mut unit),
            "{what}: nothing to mutate\n{}",
            dump(&unit)
        );
        let inputs = hirsel_lir::validate::Inputs {
            memo: Default::default(),
            discharge: None,
            operations: &program.operation_registry(),
            target: &target,
            layouts: &lowered.table,
            manifest: &manifest,
        };
        let v = hirsel_lir::validate::check(&unit, &inputs);
        assert!(!v.is_empty(), "{what} was accepted\n{}", dump(&unit));
    }
}

fn instructions(u: &hirsel_lir::LirUnit<hirsel_lir::stage::Value>) -> Vec<hirsel_lir::ir::Inst> {
    u.instructions.clone()
}

#[test]
fn a_structural_comparison_lowers_to_compare_and_a_scalar_one_stays_binary() {
    use hirsel_lir::ir::{CompareOp, Inst};
    let (_, lowered) = lower_valid(
        "type P { a: u64, b: String }\nfn same(x: P, y: P) -> bool { x == y }\nfn less(x: String, y: String) -> bool { x < y }\nfn scalar(x: u64, y: u64) -> bool { x == y }",
    );
    for (name, expected) in [("same", CompareOp::Eq), ("less", CompareOp::Lt)] {
        let unit = unit_named(&lowered, name);
        let compares: Vec<_> = instructions(unit)
            .into_iter()
            .filter_map(|i| match i {
                Inst::Compare { op, failure, .. } => Some((op, failure)),
                _ => None,
            })
            .collect();
        assert_eq!(compares.len(), 1, "{}", dump(unit));
        assert_eq!(compares[0].0, expected);
        // Depth is the comparison's one failure.
        let rule = &unit.tables.failures[compares[0].1.index()].rule;
        assert_eq!(rule.as_deref(), Some("MEM-3"));
    }
    let scalar = unit_named(&lowered, "scalar");
    assert!(
        instructions(scalar)
            .iter()
            .all(|i| !matches!(i, Inst::Compare { .. })),
        "{}",
        dump(scalar)
    );
}

#[test]
fn compared_call_results_are_viewed_then_released() {
    use hirsel_lir::ir::Inst;
    // `view(b) == view(c)` takes two call results: the comparison reads them
    // through views and releases them after, as the interpreter discards them.
    let (_, lowered) = lower_valid(
        "fn view(x: Bytes) -> Bytes in x { copy x }\nfn same(b: Bytes, c: Bytes) -> bool { view(b) == view(c) }",
    );
    let same = unit_named(&lowered, "same");
    let insts = instructions(same);
    let at = insts
        .iter()
        .position(|i| matches!(i, Inst::Compare { .. }))
        .unwrap_or_else(|| panic!("{}", dump(same)));
    let after = &insts[at + 1..];
    assert!(
        after
            .iter()
            .filter(|i| matches!(i, Inst::Drop { .. }))
            .count()
            >= 2,
        "{}",
        dump(same)
    );
}

#[test]
fn a_many_closure_stored_as_a_once_function_weakens() {
    use hirsel_lir::ir::Inst;
    let (_, lowered) = lower_valid(
        "fn twice() -> u64 {\n  let mut g: once fn() -> u64 = fn() -> u64 { 1 }\n  let first = g()\n  g = fn() -> u64 { 2 }\n  first * 10 + g()\n}",
    );
    let twice = unit_named(&lowered, "twice");
    assert_eq!(
        instructions(twice)
            .iter()
            .filter(|i| matches!(i, Inst::Weaken { .. }))
            .count(),
        2,
        "{}",
        dump(twice)
    );
}

#[test]
fn a_view_assigned_on_two_paths_joins_without_a_copy() {
    use hirsel_lir::ir::{Holding, Inst};
    let (_, lowered) = lower_valid(
        "type P { f: String }\nfn pick(a: P, b: P, c: bool) -> u64 {\n  string.byte_len(if c { a.f } else { b.f })\n}",
    );
    let pick = unit_named(&lowered, "pick");
    // The two views meet as one borrowed block parameter under the lesser
    // loan; nothing is copied to join them.
    let joined = pick
        .blocks
        .iter()
        .flat_map(|b| b.parameters.get(&pick.pools.values).unwrap_or(&[]))
        .filter(|p| matches!(pick.values[p.index()].holding, Holding::Borrowed(_)))
        .count();
    assert_eq!(joined, 1, "{}", dump(pick));
    assert!(
        pick.instructions
            .iter()
            .all(|i| !matches!(i, Inst::Copy { .. })),
        "{}",
        dump(pick)
    );
}

#[test]
fn a_fixed_field_of_a_flat_value_is_a_place_at_its_cursor() {
    use hirsel_lir::ir::{Inst, ProjectionStep};
    let (_, lowered) = lower_valid(
        "flat type Item { text: String, n: u64 }\nfn edit(mut x: Item) -> () { x.n = 1 }\nfn read(x: Item) -> u64 { x.n }",
    );
    for name in ["edit", "read"] {
        let unit = unit_named(&lowered, name);
        // The cursor follows `text`'s prefix, so the address is prepared where
        // it is used, not in the prologue.
        let at = unit
            .addresses
            .iter()
            .position(|a| {
                a.projection.get(&unit.pools.projections) == Some(&[ProjectionStep::Field(1)][..])
            })
            .unwrap_or_else(|| panic!("{}", dump(unit)));
        let prologue = &unit.blocks[unit.entry.index()];
        let prepared_first = unit.instructions[prologue.instructions.start as usize..]
            [..prologue.instructions.len as usize]
            .iter()
            .any(|i| matches!(i, Inst::Address { address, .. } if address.index() == at));
        assert!(!prepared_first, "{}", dump(unit));
    }
}

#[test]
fn a_flat_record_is_constructed_from_ordinary_values() {
    use hirsel_lir::ir::Inst;
    use hirsel_lir::layout_types::LayoutKind;
    let (_, lowered) = lower_valid(
        "flat type Item { text: String, n: u64 }\nfn main() -> u64 {\n  let a = Item(text: \"flat\", n: 1)\n  a.n\n}",
    );
    let main = unit_named(&lowered, "main");
    // The construction encodes the literal slice itself (LIR schema
    // revision 4): its site is the flat record, its text field the slice.
    let flat = main
        .tables
        .constructs
        .iter()
        .any(|c| lowered.table.layout(c.layout).map(|r| &r.kind) == Some(&LayoutKind::Flat));
    assert!(flat, "{}", dump(main));
    assert!(
        main.instructions
            .iter()
            .any(|i| matches!(i, Inst::Construct { .. })),
        "{}",
        dump(main)
    );
}

#[test]
fn a_string_part_of_a_flat_value_is_viewed_at_its_source_type() {
    use hirsel_lir::ir::Inst;
    let (_, lowered) = lower_valid(
        "flat type Item { text: String, n: u64 }\nfn len(x: Item) -> u64 { string.byte_len(x.text) }",
    );
    let len = unit_named(&lowered, "len");
    assert!(
        len.instructions
            .iter()
            .any(|i| matches!(i, Inst::FlatView { .. })),
        "{}",
        dump(len)
    );
}

#[test]
fn a_flat_list_is_counted_and_walked_in_place() {
    use hirsel_lir::ir::{Inst, ProjectionStep, Terminator};
    let (_, lowered) = lower_valid(
        "flat type Item { text: String, n: u64 }\nflat type Items { items: List[Item] }\nfn sum(xs: Items) -> u64 {\n  let mut total: u64 = 0\n  for x in xs.items {\n    total += x.n\n  }\n  total\n}",
    );
    let sum = unit_named(&lowered, "sum");
    // `list.len` reads the encoding's count; no List is copied out of the
    // flat value and no call lends one.
    assert!(
        sum.instructions
            .iter()
            .any(|i| matches!(i, Inst::FlatLength { .. })),
        "{}",
        dump(sum)
    );
    assert!(
        sum.instructions
            .iter()
            .all(|i| !matches!(i, Inst::Copy { .. })),
        "{}",
        dump(sum)
    );
    assert!(
        sum.terminators
            .iter()
            .all(|t| !matches!(t, Terminator::Call { .. })),
        "{}",
        dump(sum)
    );
    // Each element is a place at its cursor in the list.
    assert!(
        sum.addresses.iter().any(|a| a
            .projection
            .get(&sum.pools.projections)
            .is_some_and(|p| p.iter().any(|s| matches!(s, ProjectionStep::FlatAt { .. })))),
        "{}",
        dump(sum)
    );
}

#[test]
fn a_copied_variable_part_of_a_flat_value_has_an_owned_source_value() {
    lower_valid(
        "flat type Item { text: String, n: u64 }\nfn text(take x: Item) -> String { copy x.text }",
    );
}

#[test]
fn a_taken_variable_part_of_a_flat_value_has_an_owned_source_value() {
    lower_valid(
        "flat type Item { text: String, n: u64 }\nfn text(take x: Item) -> String { let Item(text: s, n: _) = take x; take s }",
    );
}

#[test]
fn a_variable_flat_record_part_is_copied_in_a_loop_and_returned() {
    for source in [
        include_str!("../../../../conformance/MEM/MEM-12-flat-part-copy-loop.hir"),
        include_str!("../../../../conformance/MEM/MEM-12-flat-part-copy-return.hir"),
    ] {
        let (_, lowered) = lower_valid(source);
        assert!(lowered.units.iter().any(|unit| {
            unit.instructions
                .iter()
                .any(|inst| matches!(inst, hirsel_lir::ir::Inst::FlatCopy { .. }))
        }));
    }
}

#[test]
fn written_sendable_positions_keep_their_physical_callable_layout() {
    lower_valid(include_str!(
        "../../../../conformance/MODE/MODE-4-written-sendable-positions-run.hir"
    ));
}

#[test]
fn a_many_reference_prepared_for_once_replacement_keeps_staging_layout() {
    lower_valid(
        "fn seven() -> u64 { 7 }\nfn main() -> u64 { let mut single: sendable once fn() -> u64 = fn() { 1 }; single = seven; single() }",
    );
}

fn capture_limit_program(count: u32) -> String {
    let mut source = String::from(
        "resource type Token { n: u64 }\nfn drive(take g: once fn() -> ()) { g() }\npub fn main(rt: Io, args: Args) -> Result[(), ()] {\n",
    );
    for i in 0..count {
        source.push_str(&format!("let c{i} = Token(n: {i}u64)\n"));
    }
    source.push_str("drive(fn() {\n");
    for i in 0..count {
        source.push_str(&format!("let _ = take c{i}\n"));
    }
    source.push_str("})\nOk(())\n}\n");
    source
}

#[test]
fn a_65_capture_once_closure_reports_the_named_live_word_limit() {
    lower_valid(&capture_limit_program(64));
    let program = compile(&capture_limit_program(65));
    let error = hirsel_rc::import::lower(&program, &target())
        .err()
        .expect("capture limit");
    let hirsel_rc::import::LowerError::Instance(hirsel_rc::instance::InstanceError::Layout(
        _,
        hirsel_lir::layout::LayoutError::Unavailable { issue: None, what },
    )) = error
    else {
        panic!("{error:?}");
    };
    assert_eq!(
        what,
        "closure main body 1 with 65 movable captures requires 65 bits; native live-mask word implementation limit is 64 bits"
    );
}

#[test]
fn box_payload_projection_imports_a_scalar_without_selecting_a_field() {
    lower_valid("fn inspect(x: box bool) -> bool { match x { true => true, false => false } }");
}

#[test]
fn box_payload_projection_imports_nested_boxes_and_writable_fields() {
    lower_valid(include_str!(
        "../../../../conformance/MEM/MEM-7-box-read-nested-run.hir"
    ));
}

#[test]
fn box_payload_projection_imports_a_whole_scalar_payload_replacement() {
    use hirsel_tir::{InstructionKind, Projection};
    use hirsel_types::{SharedPath, TypeKind, UseKind};
    let mut program = compile("fn replace(mut x: box u64) -> () { x = box 2u64 }");
    let types = program.types.clone();
    let definition = program
        .definitions
        .values_mut()
        .find(|d| d.path.path().0.name == "replace")
        .unwrap();
    let definition = std::sync::Arc::make_mut(definition);
    let body = &mut definition.bodies[definition.entry.0 as usize];
    let (destination, value, displaced) = body
        .blocks
        .iter()
        .flat_map(|b| &b.instructions)
        .find_map(|i| {
            if let InstructionKind::Replace {
                destination,
                value,
                displaced,
                ..
            } = &i.kind
            {
                Some((*destination, value.place, *displaced))
            } else {
                None
            }
        })
        .unwrap();
    let TypeKind::Box(payload) = types
        .get(body.places[destination.0 as usize].ty)
        .unwrap()
        .kind
    else {
        panic!("box destination")
    };
    let projected = [destination, value].map(|id| {
        let mut place = body.places[id.0 as usize].clone();
        place.projection = SharedPath::from(vec![Projection::Field(0)]);
        place.ty = payload;
        let id = hirsel_tir::PlaceId(body.places.len() as u32);
        body.places.push(place);
        id
    });
    let displaced_place = &mut body.places[displaced.0 as usize];
    displaced_place.ty = payload;
    body.locals[displaced_place.root.0 as usize].ty = payload;
    for i in body.blocks.iter_mut().flat_map(|b| &mut b.instructions) {
        if let InstructionKind::Replace {
            destination, value, ..
        } = &mut i.kind
        {
            *destination = projected[0];
            value.place = projected[1];
            value.usage = UseKind::Copy;
        }
    }
    for block in &mut body.blocks {
        if let Some(index) = block
            .instructions
            .iter()
            .position(|i| matches!(i.kind, InstructionKind::Replace { .. }))
        {
            let mut drop = block.instructions[index].clone();
            drop.kind = InstructionKind::Drop {
                place: value,
                if_live: false,
            };
            block.instructions.insert(index + 1, drop);
        }
    }
    let lowered = hirsel_rc::import::lower(&program, &target()).unwrap();
    assert!(lowered.declined.is_empty(), "{:?}", lowered.declined);
    assert!(
        violations(&program, &lowered).is_empty(),
        "{:?}",
        violations(&program, &lowered)
    );
    assert!(lowered.units.iter().any(|u| {
        u.pools
            .projections
            .contains(&hirsel_lir::ir::ProjectionStep::Payload)
    }));
}

#[test]
fn kept_call_sources_release_after_later_temporaries() {
    lower_valid(include_str!(
        "../../../../conformance/LANG/LANG-3-kept-call-source-comparison-run.hir"
    ));
}

#[test]
fn mutable_callable_multiplicity_is_invariant_at_source() {
    let Err(error) = try_compile(include_str!(
        "../../../../conformance/MODE/MODE-3-mut-callable-multiplicity-bad.hir"
    )) else {
        panic!("a many slot cannot receive a once write-back");
    };
    assert!(error.contains("MODE-3"), "{error}");
    lower_valid(include_str!(
        "../../../../conformance/MODE/MODE-3-mut-callable-multiplicity-run.hir"
    ));
}

/// The importer reruns a body until its entry states converge. A tag fact
/// left from an earlier pass names this pass's versions (value ids restart),
/// so it must not elide the match's tag switch and orphan the Error arm.
#[test]
fn a_match_tag_is_tested_again_on_every_import_pass() {
    lower_valid(include_str!(
        "../../../../conformance/FACT/FACT-3-match-tag-retested-each-pass-run.hir"
    ));
}

/// `box C()` of a nullary variant reads the one static block of that variant at its payload
/// layout instead of allocating: the ordinal at an ordinal tag, a null word at a niche. A
/// variant with fields still allocates, and every read of one variant names one static.
#[test]
fn a_boxed_nullary_variant_reads_one_static_block() {
    use hirsel_lir::ir::{ConstructorKind, Inst};
    use hirsel_lir::layout_types::TagForm;
    let (_, lowered) = lower_valid(
        "type Tree = Leaf | Branch(left: box Tree, right: box Tree)\ntype Shade = Red | Green | Blue(n: u64)\nfn leaf() -> box Tree { box Leaf() }\nfn branch() -> box Tree { box Branch(left: leaf(), right: box Leaf()) }\nfn green() -> box Shade { let g: Shade = Green(); box g }\nfn blue() -> box Shade { box Blue(n: 3u64) }\nfn main() -> u64 { match branch() { Leaf() => 1u64, Branch(left: l, right: r) => match green() { Green() => 2u64, _ => match blue() { Blue(n: n) => n, _ => 0u64 } } } }",
    );
    let boxes = |name: &str| {
        let unit = unit_named(&lowered, name);
        let reads = unit
            .instructions
            .iter()
            .filter(|i| matches!(i, Inst::ReadStatic { .. }))
            .count();
        let allocations = unit
            .tables
            .constructs
            .iter()
            .filter(|c| c.constructor == ConstructorKind::Box)
            .count();
        (reads, allocations)
    };
    assert_eq!(boxes("leaf"), (1, 0));
    assert_eq!(boxes("branch"), (1, 1));
    assert_eq!(boxes("green"), (1, 0));
    assert_eq!(boxes("blue"), (0, 1));
    let nullary: Vec<_> = lowered
        .statics
        .iter()
        .filter(|s| s.symbol.definition == hirsel_rc::import::nullary_variant())
        .collect();
    assert_eq!(nullary.len(), 2, "one static per boxed nullary variant");
    for data in nullary {
        let [block] = data.blocks.as_slice() else {
            panic!("{data:?}")
        };
        let record = &lowered.table.layouts[block.layout.0 as usize];
        let tag = record.tag.as_ref().unwrap();
        let mut want = vec![0u8; block.bytes.len()];
        if tag.form == TagForm::Ordinal {
            let at = tag.offset as usize;
            want[at..at + 4].copy_from_slice(&1u32.to_le_bytes());
        } else {
            assert!(matches!(tag.form, TagForm::Niche { .. }));
        }
        assert_eq!(block.bytes, want);
        assert!(block.references.is_empty());
    }
}

/// A slot holding a constant inline table that is only read is a constant slot: native code
/// indexes the table's read-only data in place. The local a program writes after copying
/// the table into it stays an ordinary slot; the table it copies from is still constant.
#[test]
fn a_constant_table_read_in_place_is_a_constant_slot() {
    use hirsel_lir::ir::SlotKind;
    let table = (1..=16)
        .map(|n| format!("{n}u8"))
        .collect::<Vec<_>>()
        .join(", ");
    let (_, lowered) = lower_valid(&format!(
        "const T: Inline[u8, 16] = [{table}]\nfn get(i: u64) -> u8 {{ T[i] }}\nfn set(i: u64) -> u8 {{\n  let mut t = T\n  t[0] = 9u8\n  t[i]\n}}\nfn main() -> u64 {{ int.widen(get(1u64)) + int.widen(set(0u64)) }}",
    ));
    let constant = |name: &str| {
        let unit = unit_named(&lowered, name);
        let slots = hirsel_lir::statics::constant_slots(unit, &lowered.statics);
        let bytes: Vec<Vec<u8>> = slots
            .iter()
            .flatten()
            .map(|slot| slot.bytes.to_vec())
            .collect();
        let locals: Vec<bool> = unit
            .slots
            .iter()
            .zip(&slots)
            .filter(|(slot, _)| slot.kind == SlotKind::Local)
            .map(|(_, constant)| constant.is_some())
            .collect();
        (bytes, locals)
    };
    let table: Vec<u8> = (1..=16).collect();
    assert_eq!(constant("get"), (vec![table.clone()], vec![]));
    assert_eq!(constant("set"), (vec![table], vec![false]));
}

#[test]
fn a_joined_string_view_can_be_copied_through_a_match_alias() {
    lower_valid(
        "fn pick(a: String, b: String, select: bool) -> String in {a, b} { match (if select { a } else { b }) { value => value } }",
    );
}

#[test]
fn a_joined_string_view_can_feed_a_scalars_iterator() {
    lower_valid(
        "fn count(a: String, b: String, select: bool) -> u64 { let mut n = 0u64; for _ in string.scalars(if select { a } else { b }) { n = n + 1; }; n }",
    );
}

/// `unique` must decode a flat field into a fresh ordinary value, rather
/// than lend the field's uncounted encoding to the ordinary deep copier.
#[test]
fn unique_flat_field_projections_are_owned() {
    for source in [
        include_str!("../../../../conformance/MODE/MODE-2-unique-flat-field-projection-run.hir"),
        include_str!(
            "../../../../conformance/MODE/MODE-2-unique-flat-field-copy-projection-run.hir"
        ),
    ] {
        let (_, lowered) = lower_valid(source);
        let detached = unit_named(&lowered, "detached");
        assert!(
            detached
                .instructions
                .iter()
                .any(|i| matches!(i, hirsel_lir::ir::Inst::FlatCopy { .. })),
            "{}",
            dump(detached)
        );
    }
}

/// A view of a flat field still belongs to its carrier: labelling that
/// view owned cannot manufacture the independent storage `unique` needs.
#[test]
fn a_flat_field_view_cannot_claim_ownership() {
    use hirsel_lir::ir::{Holding, Inst};
    use hirsel_lir::validate::ViolationKind;
    let (program, mut lowered) = lower_valid(
        "flat type TextFields { title: String }\ntype Row { text: TextFields }\nfn title(row: Row) -> u64 { string.byte_len(row.text.title) }\nfn main() -> u64 { title(Row(text: TextFields(title: \"abc\"))) }",
    );
    let unit = lowered
        .units
        .iter_mut()
        .find(|u| u.header.definition.0.name.as_str() == "title")
        .unwrap();
    let result = unit
        .instructions
        .iter()
        .find_map(|i| match i {
            Inst::FlatView { result, .. } => Some(*result),
            _ => None,
        })
        .expect("a borrowed flat-field view");
    assert!(matches!(
        unit.values[result.index()].holding,
        Holding::Borrowed(_)
    ));
    unit.values[result.index()].holding = Holding::Owned;
    assert!(
        violations(&program, &lowered)
            .iter()
            .flat_map(|(_, v)| v)
            .any(|v| v.kind == ViolationKind::Holding(result))
    );
}

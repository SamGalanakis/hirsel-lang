//! LANG-10 constructor-tail destinations at LIR (schema revision 59): a
//! qualifying site lowers to a loop over `HoleOpen`/`HoleFill`, with
//! `HoleAbandon` on every failure edge, and C refuses each forged breach of the
//! hole discipline on its own.
use crate::rc_support as support;

use hirsel_lir::ir::{BlockKind, Inst, LirUnit, PoolRange, ValueRepr};
use hirsel_lir::validate::{Violation, ViolationKind};
use support::*;

const CHAIN: &str = "type Chain = End | Link(u64, box Chain)\nfn chain(n:u64)->Chain { if n == 0u64 { End } else { Link(n, box chain(n-1u64)) } }\nfn main()->u64 { match chain(3u64) { End => 0u64, Link(n, _) => n } }";

/// The instructions of every block, with each one's block and index.
fn sites<S>(u: &LirUnit<S>) -> Vec<(usize, usize, Inst)> {
    let mut out = Vec::new();
    for (b, block) in u.blocks.iter().enumerate() {
        for (i, inst) in block
            .instructions
            .get(&u.instructions)
            .unwrap()
            .iter()
            .enumerate()
        {
            out.push((b, i, inst.clone()));
        }
    }
    out
}

/// Rewrite block `b`'s instructions.
fn edit<S>(u: &mut LirUnit<S>, b: usize, f: impl FnOnce(&mut Vec<Inst>)) {
    let old = std::mem::take(&mut u.instructions);
    let mut f = Some(f);
    for (i, block) in u.blocks.iter_mut().enumerate() {
        let mut code = block.instructions.get(&old).unwrap().to_vec();
        if i == b {
            (f.take().unwrap())(&mut code);
        }
        block.instructions = PoolRange::new(u.instructions.len() as u32, code.len() as u32);
        u.instructions.extend(code);
    }
}

/// The violations of the chain's unit after `forge`.
fn forged(forge: impl FnOnce(&mut LirUnit<hirsel_lir::stage::Value>)) -> Vec<Violation> {
    let (program, mut lowered) = lower(CHAIN);
    let id = instances_of(&lowered, "chain")[0];
    forge(&mut lowered.units[id.index()]);
    unit_violations(&program, &lowered, id)
}

fn refused_as(v: &[Violation], what: &'static str) {
    assert!(
        v.iter().any(|v| v.kind
            == ViolationKind::Type(match what {
                "observed" => "open constructor-tail value observed",
                "exit" => "hole live at exit",
                "fill" => "hole fill",
                other => other,
            }))
            || what == "any" && !v.is_empty(),
        "{what}: {v:#?}"
    );
}

/// The fill at the site, in a body block that loops back.
fn site_fill<S>(u: &LirUnit<S>) -> (usize, usize, Inst) {
    sites(u)
        .into_iter()
        .find(|(b, _, i)| {
            matches!(i, Inst::HoleFill { .. })
                && u.blocks[*b].kind == BlockKind::Body
                && !sites(u)
                    .iter()
                    .any(|(b2, _, c)| b2 == b && matches!(c, Inst::Copy { .. }))
        })
        .expect("a site fill")
}

#[test]
fn a_constructor_tail_site_lowers_to_a_validated_loop() {
    let (program, lowered) = lower(CHAIN);
    let id = instances_of(&lowered, "chain")[0];
    let unit = &lowered.units[id.index()];
    assert!(
        lowered.plans[id.index()].is_some(),
        "{:?}",
        lowered.declined
    );
    assert_eq!(self_calls(unit), 0, "{}", dump(unit));
    assert!(has_cycle(unit));
    let all = sites(unit);
    let count = |p: fn(&Inst) -> bool| all.iter().filter(|(_, _, i)| p(i)).count();
    // The root opens at entry and the site opens each level's box.
    assert_eq!(
        count(|i| matches!(i, Inst::HoleOpen { .. })),
        2,
        "{}",
        dump(unit)
    );
    assert!(count(|i| matches!(i, Inst::HoleFill { .. })) >= 2);
    // Every abandonment is on a failure path and the outer one releases the root.
    for (b, _, inst) in &all {
        if let Inst::HoleAbandon { .. } = inst {
            assert_eq!(unit.blocks[*b].kind, BlockKind::FailureCleanup);
        }
    }
    assert!(
        all.iter()
            .any(|(_, _, i)| matches!(i, Inst::HoleAbandon { root: Some(_), .. }))
    );
    let v = unit_violations(&program, &lowered, id);
    assert!(v.is_empty(), "{v:#?}\n{}", dump(unit));
}

/// Owned earlier fields: a string evaluated before the call, and a box
/// holding an ordinary recursive call (OCaml's right-leaning tree).
#[test]
fn owned_earlier_fields_lower_to_validated_loops() {
    for (source, name) in [
        (
            "type Names = Nil | Name(String, box Names)\nfn names(n:u64)->Names { if n == 0u64 { Nil } else { Name(std@1/u64.to_string(n), box names(n-1u64)) } }\nfn main()->u64 { match names(3u64) { Nil => 0u64, Name(_, _) => 1u64 } }",
            "names",
        ),
        (
            "type Tree = Leaf | Node(box Tree, u64, box Tree)\nfn build(lo:u64,hi:u64)->Tree { if lo >= hi { Leaf } else { Node(box build(lo, lo + (hi - lo) / 4u64), lo + (hi - lo) / 4u64, box build(lo + (hi - lo) / 4u64 + 1u64, hi)) } }\nfn main()->u64 { match build(0u64, 9u64) { Leaf => 0u64, Node(_, v, _) => v } }",
            "build",
        ),
    ] {
        let (program, lowered) = lower(source);
        let id = instances_of(&lowered, name)[0];
        let unit = &lowered.units[id.index()];
        assert!(
            lowered.plans[id.index()].is_some(),
            "{source}\n{:?}",
            lowered.declined
        );
        assert!(
            sites(unit)
                .iter()
                .any(|(_, _, i)| matches!(i, Inst::HoleFill { .. })),
            "{}",
            dump(unit)
        );
        let v = unit_violations(&program, &lowered, id);
        assert!(v.is_empty(), "{source}\n{v:#?}\n{}", dump(unit));
    }
}

#[test]
fn c_refuses_a_double_fill() {
    let v = forged(|u| {
        let (b, i, fill) = site_fill(u);
        edit(u, b, |code| code.insert(i + 1, fill));
    });
    refused_as(&v, "any");
}

#[test]
fn c_refuses_a_fill_after_abandonment() {
    let v = forged(|u| {
        let (b, i, fill) = site_fill(u);
        let Inst::HoleFill { hole, .. } = fill else {
            unreachable!()
        };
        edit(u, b, |code| {
            code.insert(i, Inst::HoleAbandon { hole, root: None })
        });
    });
    refused_as(&v, "any");
}

/// The return's fill moves after the copy of the root's payload: the copy
/// reads the root while its last hole is open.
#[test]
fn c_refuses_a_read_of_an_open_root() {
    let v = forged(|u| {
        let all = sites(u);
        let (b, i, _) = all
            .iter()
            .find(|(b, i, inst)| {
                matches!(inst, Inst::HoleFill { .. })
                    && all
                        .iter()
                        .any(|(b2, j, c)| b2 == b && j > i && matches!(c, Inst::Copy { .. }))
            })
            .cloned()
            .expect("a return fill before the root's copy");
        edit(u, b, |code| {
            let fill = code.remove(i);
            code.push(fill);
        });
    });
    refused_as(&v, "observed");
}

/// A hole shared as if it were a block escapes the loop's discipline; C's
/// count rules already refuse the share of an uncounted value.
#[test]
fn c_refuses_a_hole_that_escapes() {
    let v = forged(|u| {
        let (b, i, open) = sites(u)
            .into_iter()
            .find(|(_, _, i)| matches!(i, Inst::HoleOpen { .. }))
            .unwrap();
        let Inst::HoleOpen { hole, .. } = open else {
            unreachable!()
        };
        edit(u, b, |code| code.insert(i + 1, Inst::Dup { value: hole }));
    });
    refused_as(&v, "any");
}

/// Without its abandonment a failure edge leaves its hole live at the exit.
#[test]
fn c_refuses_a_failure_path_that_keeps_its_hole() {
    let v = forged(|u| {
        let (b, i, _) = sites(u)
            .into_iter()
            .find(|(_, _, i)| matches!(i, Inst::HoleAbandon { root: Some(_), .. }))
            .unwrap();
        edit(u, b, |code| {
            code.remove(i);
        });
    });
    refused_as(&v, "any");
}

/// A fill whose value is the box itself, not its payload.
#[test]
fn c_refuses_a_mistyped_fill() {
    let v = forged(|u| {
        let (b, i, fill) = site_fill(u);
        let Inst::HoleFill { hole, .. } = fill else {
            unreachable!()
        };
        let block = sites(u)
            .into_iter()
            .find_map(|(_, _, i)| match i {
                Inst::HoleOpen { result, .. } => Some(result),
                _ => None,
            })
            .unwrap();
        assert_eq!(
            u.types[u.values[block.index()].ty.index()].repr,
            ValueRepr::Block
        );
        edit(u, b, |code| code[i] = Inst::HoleFill { hole, value: block });
    });
    refused_as(&v, "fill");
}

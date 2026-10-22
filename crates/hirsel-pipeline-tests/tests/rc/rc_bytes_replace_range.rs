//! The checked range wrapper is shared by direct calls and function values.
use crate::rc_support as support;
use hirsel_lir::ir::{Inst, Terminator};
use hirsel_rc::{insert::Proofs, placement::Options};

#[test]
fn direct_and_indirect_wrappers_detach_only_after_checks_and_empty_return() {
    for call in [
        "bytes.replace_range(mut b, 1, src)",
        "let replace = bytes.replace_range\nreplace(mut b, 1, src)",
    ] {
        let source = format!(
            "fn main() -> u64 {{ let mut b: List[u8] = [0u8,1u8,2u8,3u8,4u8]\nlet src = bytes.from_list(copy b)\n{call}\nlist.len(b) }}"
        );
        let (program, lowered) = support::lower_valid(&source);
        let ids = support::instances_of(&lowered, "replace_range");
        assert_eq!(ids.len(), 1, "{}", support::dump(&lowered.units[0]));
        let id = ids[0];
        let unit = &lowered.units[id.index()];
        let placed = hirsel_rc::placement::place(
            unit.clone(),
            &lowered.table,
            &Options::for_instance(true, &lowered.instances, id),
        )
        .unwrap();
        let rc = hirsel_rc::insert::insert(
            placed,
            &lowered.table,
            &Proofs::for_instance(&lowered.instances, id),
        )
        .unwrap();
        // The builder has a preparation entry and a separate body block.
        // Inspect actual normal paths rather than assuming checks sit at entry.
        let mut pending = vec![(rc.entry, 0usize, 0usize, false)];
        let mut seen = std::collections::BTreeSet::new();
        let mut empty_return = false;
        while let Some((block, mut checks, branches, mut prepared)) = pending.pop() {
            if !seen.insert((block, checks, branches, prepared)) {
                continue;
            }
            for inst in rc.blocks[block.index()]
                .instructions
                .get(&rc.instructions)
                .unwrap()
            {
                if matches!(inst, Inst::Check { .. }) {
                    checks += 1;
                }
                if matches!(inst, Inst::IsWritable { .. } | Inst::UniqueCopy { .. }) {
                    assert!(checks >= 2 && branches >= 1, "{}", support::dump(&rc));
                    prepared = true;
                }
            }
            let term = &rc.terminators[block.index()];
            if matches!(term, Terminator::Return(_)) && !prepared {
                assert!(checks >= 2);
                empty_return = true;
            }
            let branches = branches + usize::from(matches!(term, Terminator::Branch { .. }));
            for edge in term.normal_edges(&rc.pools) {
                pending.push((
                    rc.tables.edges[edge.index()].target,
                    checks,
                    branches,
                    prepared,
                ));
            }
        }
        assert!(empty_return, "{}", support::dump(&rc));
        assert!(
            rc.instructions
                .iter()
                .any(|i| matches!(i, Inst::IsWritable { .. }))
        );
        assert!(
            rc.instructions
                .iter()
                .any(|i| matches!(i, Inst::UniqueCopy { .. }))
        );
        assert!(!rc.instructions.iter().any(|i| matches!(
            i,
            Inst::ArenaPush { .. } | Inst::SeqSetLen { .. } | Inst::SeqReserve { .. }
        )));
        assert!(
            rc.terminators
                .iter()
                .any(|t| matches!(t, Terminator::Return(_)))
        );
        let _ = program;
    }
}

#[test]
fn replacement_is_mayallocate_and_does_not_retain_source_provenance() {
    let valid =
        "fn replace(src: Bytes, mut dst: List[u8]) { bytes.replace_range(mut dst, 0, src) }";
    support::try_compile(valid).unwrap();
    assert!(support::try_compile(&format!("@noalloc\n{valid}")).is_err());
    let indirect = "@noalloc\nfn replace(src: Bytes, mut dst: List[u8]) { let f = bytes.replace_range\nf(mut dst, 0, src) }";
    assert!(support::try_compile(indirect).is_err());
}

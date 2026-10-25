//! Redundant count elision: Rc LIR to Rc LIR.
//!
//! "Elide adjacent or dataflow-proved redundant dup/release only with the
//! same holder and lifetime." A semantic `Copy` is the Dup of a non-final
//! owning consumer; when insertion releases the copied slot right after it
//! (the copy was the content's last use), the pair cancels: the copy becomes
//! a move and the release disappears. For a whole slot the release goes
//! entirely; for a part, the move leaves that part moved out and the rest of
//! the content is dropped where the release was. A part inside a block's
//! payload (an element of a list) keeps its copy and the release, as its
//! block's drop has no hole to skip.
//!
//! Insertion never releases an anchored slot early (S-14), so a copy of
//! content that reaches a `Shared` root never cancels: its holder ends at its
//! cleanup anchor, and the stage validator's RC-ROOT rule refuses the move.
//!
//! The pair must be adjacent, so no check, effect or failure edge lies
//! between the Dup and the release it cancels. Only counted copies (`Dup`
//! and `Fields` recipes) cancel; a flat byte copy into distinct storage is
//! source Copy semantics and stays. Elision never adds an allocation, so a
//! noalloc promise is preserved.
use crate::PassError;
use crate::insert::{Editor, Facts};
use hirsel_lir::ir::{Inst, LirUnit, ReadMode};
use hirsel_lir::layout_types::{CopyRecipe, LayoutTable};
use hirsel_lir::stage::Rc;

pub fn elide(unit: LirUnit<Rc>, table: &LayoutTable) -> Result<LirUnit<Rc>, PassError> {
    let facts = Facts::new(&unit, table);
    let mut ed = Editor::new(unit);
    match rewrite(&mut ed, &facts, table, |_, _, _| {
        Ok::<_, std::convert::Infallible>(())
    }) {
        Ok(()) => Ok(ed.finish()),
        Err(never) => match never {},
    }
}

fn rewrite<E>(
    ed: &mut Editor<Rc>,
    facts: &Facts,
    table: &LayoutTable,
    mut record: impl FnMut(usize, usize, usize) -> Result<(), E>,
) -> Result<(), E> {
    for b in 0..ed.code.len() {
        let mut i = 0;
        while i + 2 < ed.code[b].len() {
            if let Some(replacement) = cancel(ed, facts, table, &ed.code[b][i..i + 3]) {
                record(b, i, replacement.len())?;
                ed.code[b].splice(i..i + 3, replacement);
            }
            i += 1;
        }
    }
    Ok(())
}

/// Run the same elisions with explicit instruction origins and the editor's
/// actual compaction maps. This staged trace adapter refuses an elision that
/// touches an observed transition; it never invents a replacement observation.
/// Value-identity rewrites still require a future checked transport rule.
pub fn elide_observed(
    unit: LirUnit<Rc>,
    table: &LayoutTable,
    input: &hirsel_lir::ownership_trace::Plan,
) -> Result<
    (
        LirUnit<Rc>,
        hirsel_lir::ownership_trace::Plan,
        hirsel_lir::ownership_trace::Recipe,
    ),
    String,
> {
    use hirsel_lir::ir::BlockId;
    use hirsel_lir::ownership_trace::{self as trace, Point};
    trace::check(&unit, input).map_err(|error| error.to_string())?;
    let before = unit.clone();
    let facts = Facts::new(&unit, table);
    let mut ed = Editor::new(unit);
    let mut origins: Vec<Vec<Option<u32>>> = ed
        .code
        .iter()
        .map(|code| (0..code.len()).map(|i| Some(i as u32)).collect())
        .collect();
    rewrite(&mut ed, &facts, table, |b, i, replacement_len| {
        let origins = &mut origins[b];
        for observation in &input.observations {
            if let Point::Instruction { block, index, .. } = observation.point
                && block == BlockId(b as u32)
                && origins[i..i + 3].contains(&Some(index))
            {
                return Err(format!(
                    "ownership trace: elision of observed marker {:?} needs an explicit semantic transport",
                    observation.marker.id
                ));
            }
        }
        origins.splice(i..i + 3, std::iter::repeat_n(None, replacement_len));
        Ok(())
    })?;
    crate::observation::finish(&before, input, ed, &origins)
}

/// `copy` of (a part of) a slot, then insertion's release of that slot.
fn cancel(
    ed: &Editor<Rc>,
    facts: &Facts,
    table: &LayoutTable,
    window: &[Inst],
) -> Option<Vec<Inst>> {
    let [
        Inst::Copy {
            result,
            address,
            reaching,
            ..
        },
        Inst::Read {
            result: taken,
            address: whole,
            reaching: version,
            mode: ReadMode::Take,
        },
        Inst::Release {
            value,
            anchor,
            failure,
            ..
        },
    ] = *window
    else {
        return None;
    };
    let (slot, path) = facts.slot_of(address)?;
    let (released, rest) = facts.slot_of(whole)?;
    if slot != released || !rest.is_empty() || value != taken || version != reaching {
        return None;
    }
    // A part inside a block's payload stays copied: moving it out would leave
    // a hole in the block that its drop cannot skip.
    if facts.places[address.index()]
        .as_ref()
        .is_some_and(|p| !p.crossings.is_empty())
    {
        return None;
    }
    let layout = ed.unit.types[ed.unit.values[result.index()].ty.index()].layout;
    if !matches!(
        table.layout(layout)?.copy,
        CopyRecipe::Dup | CopyRecipe::Fields
    ) {
        return None;
    }
    let mut out = vec![Inst::Read {
        result,
        address,
        reaching,
        mode: ReadMode::Take,
    }];
    if !path.is_empty() {
        // The rest of the content still ends here.
        out.push(Inst::Drop {
            address: whole,
            if_live: false,
            anchor,
            failure,
        });
    }
    Some(out)
}

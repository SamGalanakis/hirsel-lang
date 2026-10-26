//! Instruction coordinates and owned operands for entered cleanup. This table
//! is separate from call states: there is no CallId for a release instruction.
use super::*;

pub(super) fn sites(u:&LirUnit<Ready>,table:&LayoutTable)->Vec<(BlockId,usize)> {
    u.blocks.iter().enumerate().flat_map(|(b,block)| {
        block.instructions.get(&u.instructions).unwrap_or(&[]).iter().enumerate().filter_map(move |(i,inst)| {
            match *inst {
                Inst::Release{layout,live:MaskRef::All,..}
                    if matches!(table.layout(layout).map(|l|&l.kind),Some(LayoutKind::Shared{..})) =>Some((BlockId(b as u32),i)),
                _=>None,
            }
        })
    }).collect()
}

pub(super) fn root_states(
    u:&LirUnit<Ready>, table:&LayoutTable, storage:&[LiveStorage],
    base:u64, first:u32,
)->Result<Vec<CleanupResumeState>,String> {
    let live=crate::liveness::liveness(u);
    let mut out=Vec::new();
    for (b, block) in u.blocks.iter().enumerate() {
        for (i, inst) in block.instructions.get(&u.instructions).unwrap_or(&[]).iter().enumerate() {
            let Inst::Release {value,layout,live:MaskRef::All,anchor,failure}=*inst else {continue};
            if !matches!(table.layout(layout).map(|l|&l.kind),Some(LayoutKind::Shared{..})) {continue;}
            if u.values[value.index()].holding!=Holding::Owned {return Err("cleanup root has no owning credit".into());}
            let block=BlockId(b as u32);
            if successors_of(u,block).into_iter().any(|next|reaches(u,next,block)) {
                return Err("cleanup boundary on a cycle".into());
            }
            let (slots,loans)=storage_before(u,block,i);
            if !loans.is_empty() {return Err("cleanup with a live unproved loan".into());}
            let retained:Vec<_>=storage.iter().filter(|s|slots.contains(&s.slot)).cloned().collect();
            if slots.iter().any(|s|!parameter_slot(u,*s)&&!retained.iter().any(|r|r.slot==*s)) {
                return Err("cleanup storage outside the retained frame".into());
            }
            let mut values:BTreeSet<ValueId>=(0..u.values.len()).map(|v|ValueId(v as u32))
                .filter(|v|live.live_after(u,block,i,*v)).collect();
            values.insert(value);
            // Current failure disposal belongs to this instruction as well as
            // everything the later suffix needs; a failure must not lose it.
            let f=&u.tables.failures[failure.index()];
            values.extend(f.dispose.get(&u.pools.values).unwrap_or(&[]));
            let target=u.tables.edges[f.cleanup.index()].target;
            values.extend(live.live_in[target.index()].iter().filter(|v|!u.blocks[target.index()].parameters.get(&u.pools.values).unwrap_or(&[]).contains(v)));
            values.extend(u.tables.edges[f.cleanup.index()].arguments.get(&u.pools.values).unwrap_or(&[]));
            if values.iter().any(|v|matches!(u.values[v.index()].holding,Holding::Borrowed(_))
                || matches!(u.types[u.values[v.index()].ty.index()].repr,ValueRepr::Hole|ValueRepr::Token|ValueRepr::Uncounted)) {
                return Err("cleanup with an unproved interior address or reuse owner".into());
            }
            let values:Vec<_>=values.into_iter().collect();
            let layouts:Vec<_>=values.iter().map(|v|u.types[u.values[v.index()].ty.index()].layout).collect();
            if layouts.iter().any(|l|fixed(table,*l).is_none_or(|(_,align)|align>16)) {
                return Err("cleanup with dynamic or over-aligned live bytes".into());
            }
            let (offsets,bytes)=place(table,base,&layouts).ok_or("dynamic cleanup image")?;
            if bytes>MAX_FRAME_BYTES {return Err("oversize cleanup image".into());}
            out.push(CleanupResumeState {
                action:ResumeAction::ResumeCleanup{block,instruction:i as u32,anchor},
                block,instruction:i as u32,value,layout,anchor,failure,state:first+out.len() as u32,
                live:values.into_iter().zip(offsets).map(|(value,offset)| {
                    let ty=&u.types[u.values[value.index()].ty.index()];
                    LiveOwnedSlot{value,layout:ty.layout,repr:ty.repr,holding:u.values[value.index()].holding,offset}
                }).collect(),storage:retained,bytes,
            });
        }
    }
    Ok(out)
}

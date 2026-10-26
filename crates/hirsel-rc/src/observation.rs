//! Explicit marker relocation over a pass's instruction origins and the
//! common editor's actual block/edge compaction. This grants no ordinary LIR
//! authority and does not synthesize markers from rewritten instructions.
use crate::insert::Editor;
use hirsel_lir::ir::{BlockId, EdgeId, LirUnit};
use hirsel_lir::ownership_trace::{self as trace, Plan, Point, Recipe, Relocation};
use hirsel_lir::stage::Stage;

pub(crate) fn finish<A: Stage, B: Stage>(
    before: &LirUnit<A>,
    input: &Plan,
    ed: Editor<B>,
    origins: &[Vec<Option<u32>>],
) -> Result<(LirUnit<B>, Plan, Recipe), String> {
    let (after, (blocks, edges)) =
        ed.finish_reporting(|blocks, edges| (blocks.to_vec(), edges.to_vec()));
    let mut output = input.clone();
    let mut relocations = vec![];
    for observation in &mut output.observations {
        let remap_block = |block: BlockId| {
            blocks
                .get(block.index())
                .copied()
                .flatten()
                .ok_or_else(|| format!("ownership trace: removed observed block {block:?}"))
        };
        observation.point =
            match observation.point {
                Point::Instruction {
                    block,
                    index,
                    moment,
                } => {
                    let relocated = origins[block.index()]
                        .iter()
                        .position(|origin| *origin == Some(index))
                        .ok_or_else(|| {
                            format!(
                                "ownership trace: lost instruction for marker {:?}",
                                observation.marker.id
                            )
                        })?;
                    Point::Instruction {
                        block: remap_block(block)?,
                        index: relocated as u32,
                        moment,
                    }
                }
                Point::ReleaseDrop { block, index } => {
                    let relocated = origins[block.index()]
                        .iter()
                        .position(|origin| *origin == Some(index))
                        .ok_or_else(|| {
                            format!(
                                "ownership trace: lost release for marker {:?}",
                                observation.marker.id
                            )
                        })?;
                    Point::ReleaseDrop {
                        block: remap_block(block)?,
                        index: relocated as u32,
                    }
                }
                Point::Entry => Point::Entry,
                Point::Edge(edge) => Point::Edge(EdgeId(
                    edges.get(edge.index()).copied().flatten().ok_or_else(|| {
                        format!("ownership trace: removed observed edge {edge:?}")
                    })?,
                )),
                Point::CallEntered(block) => Point::CallEntered(remap_block(block)?),
                Point::CallReturned(block) => Point::CallReturned(remap_block(block)?),
                Point::Return(block) => Point::Return(remap_block(block)?),
                Point::CompoundDrop {
                    block,
                    normal,
                    index,
                    failure,
                } => Point::CompoundDrop {
                    block: remap_block(block)?,
                    normal: remap_block(normal)?,
                    index: if index == before.blocks[normal.index()].instructions.len {
                        origins[normal.index()].len() as u32
                    } else {
                        origins[normal.index()]
                            .iter()
                            .position(|origin| *origin == Some(index))
                            .ok_or("ownership trace: lost compound cleanup cut")?
                            as u32
                    },
                    failure: remap_block(failure)?,
                },
                Point::FinalizerDrop { block } => Point::FinalizerDrop {
                    block: remap_block(block)?,
                },
            };
        relocations.push(Relocation {
            id: observation.marker.id,
            point: observation.point,
            token: observation.marker.token,
            owner: observation.marker.owner,
        });
    }
    output.unit = after.digest();
    let recipe = Recipe {
        before: input.unit,
        after: output.unit,
        relocations,
    };
    trace::check_transform(before, input, &after, &output, Some(&recipe))
        .map_err(|error| error.to_string())?;
    Ok((after, output, recipe))
}

/// A pass explicitly retaining marker coordinates. Local correspondence is
/// checked again against its output, so this cannot guess a moved identity.
pub(crate) fn identity<A: Stage, B: Stage>(
    before: &LirUnit<A>,
    input: &Plan,
    after: LirUnit<B>,
) -> Result<(LirUnit<B>, Plan, Recipe), String> {
    let mut output = input.clone();
    output.unit = after.digest();
    let recipe = Recipe {
        before: input.unit,
        after: output.unit,
        relocations: input
            .observations
            .iter()
            .map(|observation| Relocation {
                id: observation.marker.id,
                point: observation.point,
                token: observation.marker.token,
                owner: observation.marker.owner,
            })
            .collect(),
    };
    trace::check_transform(before, input, &after, &output, Some(&recipe))
        .map_err(|error| error.to_string())?;
    Ok((after, output, recipe))
}

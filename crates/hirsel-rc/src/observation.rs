//! Explicit marker relocation over a pass's instruction origins and the
//! common editor's actual block/edge compaction. This grants no ordinary LIR
//! authority and does not synthesize markers from rewritten instructions.
use crate::insert::Editor;
use hirsel_lir::ir::{BlockId, EdgeId, LirUnit};
use hirsel_lir::ownership_trace::{
    self as trace, CleanupObservation, CleanupRelocation, Plan, Point, Recipe, Relocation,
};
use hirsel_lir::stage::Stage;

pub(crate) fn finish<A: Stage, B: Stage>(
    before: &LirUnit<A>,
    input: &Plan,
    ed: Editor<B>,
    origins: &[Vec<Option<u32>>],
) -> Result<(LirUnit<B>, Plan, Recipe), String> {
    finish_generated(before, input, ed, origins, Vec::new())
}

pub(crate) fn finish_generated<A: Stage, B: Stage>(
    before: &LirUnit<A>,
    input: &Plan,
    ed: Editor<B>,
    origins: &[Vec<Option<u32>>],
    generated: Vec<CleanupObservation>,
) -> Result<(LirUnit<B>, Plan, Recipe), String> {
    let (after, (blocks, edges)) =
        ed.finish_reporting(|blocks, edges| (blocks.to_vec(), edges.to_vec()));
    let after_digest = after.digest();
    let mut output = input.clone();
    let mut relocations = vec![];
    let remap_block = |block: BlockId| {
        blocks
            .get(block.index())
            .copied()
            .flatten()
            .ok_or_else(|| format!("ownership trace: removed observed block {block:?}"))
    };
    let remap =
        |point: Point, token: trace::Location, id: trace::MarkerId| -> Result<Point, String> {
            Ok(match point {
                Point::Instruction {
                    block,
                    index,
                    moment,
                } => {
                    let relocated = origins[block.index()]
                        .iter()
                        .position(|origin| *origin == Some(index))
                        .ok_or_else(|| {
                            format!("ownership trace: lost instruction for marker {:?}", id)
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
                            format!("ownership trace: lost release for marker {:?}", id)
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
                Point::Return(block) => {
                    let block = remap_block(block)?;
                    if let hirsel_lir::ir::Terminator::Return(Some(result)) =
                        after.terminators[block.index()]
                        && token != trace::Location::Value(result)
                    {
                        let original = token;
                        let address = after
                            .instructions
                            .iter()
                            .find_map(|inst| match *inst {
                                hirsel_lir::ir::Inst::Store { address, value, .. }
                                    if original == trace::Location::Value(value) =>
                                {
                                    Some(address)
                                }
                                _ => None,
                            })
                            .ok_or("ownership trace: staged return lost source store")?;
                        Point::StagedReturn { block, address }
                    } else {
                        Point::Return(block)
                    }
                }
                Point::StagedReturn { block, address } => Point::StagedReturn {
                    block: remap_block(block)?,
                    address,
                },
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
            })
        };
    for observation in &mut output.observations {
        observation.point = remap(
            observation.point,
            observation.marker.token,
            observation.marker.id,
        )?;
        relocations.push(Relocation {
            id: observation.marker.id,
            point: observation.point,
            token: observation.marker.token,
            owner: observation.marker.owner,
        });
    }
    let mut cleanups = Vec::new();
    for (old, extra) in input.cleanups.iter().zip(&mut output.cleanups) {
        extra.point = remap(extra.point, extra.token, extra.marker)?;
        cleanups.push(CleanupRelocation {
            before: Some(old.clone()),
            after: extra.clone(),
        });
    }
    for mut extra in generated {
        let placed = |point: Point| -> Result<Point, String> {
            let Point::Instruction {
                block,
                index,
                moment,
            } = point
            else {
                return Err("ownership generated cleanup has no authored instruction".into());
            };
            Ok(Point::Instruction {
                block: remap_block(block)?,
                index,
                moment,
            })
        };
        extra.point = placed(extra.point)?;
        extra.trigger.point = placed(extra.trigger.point)?;
        extra.trigger.unit = after_digest;
        cleanups.push(CleanupRelocation {
            before: None,
            after: extra.clone(),
        });
        output.cleanups.push(extra);
    }
    output.unit = after_digest;
    let recipe = Recipe {
        cleanups,
        before: input.unit,
        after: output.unit,
        relocations,
    };
    trace::check_transform(before, input, &after, &output, Some(&recipe)).map_err(|error| {
        if let trace::Error::InvalidCleanup(id) = error {
            let rows = output
                .cleanups
                .iter()
                .filter(|row| row.marker == id)
                .take(4)
                .map(|row| {
                    let code = |point| match point {
                        Point::Instruction { block, index, .. } => after
                            .blocks
                            .get(block.index())
                            .and_then(|info| info.instructions.get(&after.instructions))
                            .and_then(|code| {
                                code.get(index.saturating_sub(1) as usize..=index as usize)
                            }),
                        _ => None,
                    };
                    let action = code(row.point)
                        .and_then(|code| code.last())
                        .and_then(|inst| match inst {
                            hirsel_lir::ir::Inst::Drop { anchor, .. } => {
                                after.tables.cleanup_anchors.get(anchor.index())
                            }
                            _ => None,
                        });
                    (
                        row,
                        code(row.trigger.point),
                        action,
                        action.is_some_and(|a| before.tables.cleanup_anchors.contains(a)),
                    )
                })
                .collect::<Vec<_>>();
            format!(
                "{error}; local {}; old values {}; original {:?}; cleanups {rows:?}",
                trace::check(&after, &output).is_ok(),
                before.values.len(),
                output.observations.iter().find(|row| row.marker.id == id)
            )
        } else {
            error.to_string()
        }
    })?;
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
        cleanups: input
            .cleanups
            .iter()
            .cloned()
            .map(|row| CleanupRelocation {
                before: Some(row.clone()),
                after: row,
            })
            .collect(),
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

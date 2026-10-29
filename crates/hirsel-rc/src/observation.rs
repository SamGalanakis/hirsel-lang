//! Explicit marker relocation over a pass's instruction origins and the
//! common editor's actual block/edge compaction. This grants no ordinary LIR
//! authority and does not synthesize markers from rewritten instructions.
use crate::insert::Editor;
use hirsel_lir::ir::{BlockId, EdgeId, LirUnit};
use hirsel_lir::ownership_trace::{
    self as trace, CleanupObservation, CleanupRelocation, Plan, Point, Recipe, Relocation,
};
use hirsel_lir::stage::Stage;

#[derive(Default)]
pub(crate) struct Moves {
    pub sources: Vec<BlockId>,
    pub ends: Vec<BlockId>,
    pub empty: std::collections::BTreeMap<(BlockId, u32), Point>,
}

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
    finish_moved(before, input, ed, origins, generated, None)
}

pub(crate) fn finish_moved<A: Stage, B: Stage>(
    before: &LirUnit<A>,
    input: &Plan,
    ed: Editor<B>,
    origins: &[Vec<Option<u32>>],
    generated: Vec<CleanupObservation>,
    moves: Option<&Moves>,
) -> Result<(LirUnit<B>, Plan, Recipe), String> {
    let collect = |blocks: &[Option<BlockId>],
                   edges: &[Option<u32>],
                   anchors: &[Option<hirsel_lir::ir::CleanupAnchorId>]| {
        (blocks.to_vec(), edges.to_vec(), anchors.to_vec())
    };
    let (after, (blocks, edges, anchors)) = ed.finish_reporting(collect);
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
    let instruction = |block: BlockId, index: u32| -> Result<(BlockId, u32), String> {
        if moves.is_none() {
            let position = origins[block.index()]
                .iter()
                .position(|origin| *origin == Some(index))
                .ok_or_else(|| format!("ownership trace: lost instruction at {block:?}:{index}"))?;
            return Ok((remap_block(block)?, position as u32));
        }
        let candidates = origins
            .iter()
            .enumerate()
            .filter(|(at, _)| {
                moves.map_or(BlockId(*at as u32), |moves| moves.sources[*at]) == block
            })
            .flat_map(|(at, rows)| {
                rows.iter()
                    .enumerate()
                    .filter_map(move |(position, origin)| {
                        (*origin == Some(index)).then_some((BlockId(at as u32), position as u32))
                    })
            })
            .collect::<Vec<_>>();
        let [(block, index)] = candidates.as_slice() else {
            return Err(format!(
                "ownership trace: lost or cloned instruction at {block:?}:{index}"
            ));
        };
        Ok((remap_block(*block)?, *index))
    };
    let end = |block: BlockId| remap_block(moves.map_or(block, |moves| moves.ends[block.index()]));
    let remap =
        |point: Point, token: trace::Location, id: trace::MarkerId| -> Result<Point, String> {
            let removed = match point {
                Point::Entry | Point::Unreachable { .. } => false,
                Point::Edge(edge) => edges.get(edge.index()).copied().flatten().is_none(),
                Point::Instruction { block, .. }
                | Point::ReleaseDrop { block, .. }
                | Point::FinalizerDrop { block }
                | Point::EmptyDrop { block, .. }
                | Point::CompoundDrop { block, .. }
                | Point::StagedReturn { block, .. }
                | Point::CallEntered(block)
                | Point::CallReturned(block)
                | Point::Return(block) => blocks.get(block.index()).copied().flatten().is_none(),
            };
            if removed {
                let site = input
                    .observations
                    .iter()
                    .find(|row| row.marker.id == id)
                    .ok_or("removed ownership point has no original marker")?
                    .marker
                    .site;
                // The checker below independently proves that this exact input
                // point is unreachable. A missing map entry alone grants nothing.
                return Ok(Point::Unreachable {
                    origin: before.digest(),
                    marker: id,
                    site,
                });
            }
            Ok(match point {
                Point::Unreachable { .. } => point,
                Point::Instruction {
                    block,
                    index,
                    moment,
                } => {
                    if let Some(point) = moves.and_then(|moves| moves.empty.get(&(block, index))) {
                        let Point::EmptyDrop { anchor, block } = *point else {
                            return Err("ownership erased drop has no authored empty block".into());
                        };
                        return Ok(Point::EmptyDrop {
                            anchor,
                            block: remap_block(block)?,
                        });
                    }
                    let (target, relocated) = instruction(block, index)?;
                    let inst = after.blocks[target.index()]
                        .instructions
                        .get(&after.instructions)
                        .and_then(|code| code.get(relocated as usize));
                    if matches!(inst, Some(hirsel_lir::ir::Inst::Release { .. }))
                        && matches!(token, trace::Location::Address(_))
                    {
                        Point::ReleaseDrop {
                            block: target,
                            index: relocated as u32,
                        }
                    } else {
                        Point::Instruction {
                            block: target,
                            index: relocated as u32,
                            moment: if matches!(
                                inst,
                                Some(hirsel_lir::ir::Inst::Drop { if_live: true, .. })
                            ) {
                                trace::Moment::IfLive
                            } else {
                                moment
                            },
                        }
                    }
                }
                Point::ReleaseDrop { block, index } => {
                    let (block, index) = instruction(block, index)?;
                    Point::ReleaseDrop { block, index }
                }
                Point::Entry => Point::Entry,
                Point::Edge(edge) => Point::Edge(EdgeId(
                    edges.get(edge.index()).copied().flatten().ok_or_else(|| {
                        format!("ownership trace: removed observed edge {edge:?}")
                    })?,
                )),
                Point::CallEntered(block) => Point::CallEntered(end(block)?),
                Point::CallReturned(block) => Point::CallReturned(end(block)?),
                Point::Return(block) => {
                    let block = end(block)?;
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
                    anchor,
                    staged,
                    block,
                    normal,
                    index,
                    failure,
                } => Point::CompoundDrop {
                    anchor: if anchors.is_empty() {
                        anchor
                    } else {
                        anchors
                            .get(anchor.index())
                            .copied()
                            .flatten()
                            .ok_or("ownership trace: removed compound cleanup anchor")?
                    },
                    staged: staged
                        .map(|(at, storage)| {
                            origins[block.index()]
                                .iter()
                                .position(|origin| *origin == Some(at))
                                .map(|at| (at as u32, storage))
                                .ok_or("ownership trace: lost staged cleanup take")
                        })
                        .transpose()?,
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
                Point::EmptyDrop { anchor, block } => Point::EmptyDrop {
                    anchor,
                    block: remap_block(block)?,
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
        )
        .map_err(|error| format!("{error}: {observation:?}"))?;
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
            match point {
                Point::Instruction {
                    block,
                    index,
                    moment,
                } => Ok(Point::Instruction {
                    block: remap_block(block)?,
                    index,
                    moment,
                }),
                Point::ReleaseDrop { block, index } => Ok(Point::ReleaseDrop {
                    block: remap_block(block)?,
                    index,
                }),
                _ => Err("ownership generated cleanup has no authored instruction".into()),
            }
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

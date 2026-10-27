//! Pass-authored cleanup splices; ordinary lowering instantiates only the
//! zero-sized reporter. No instruction matching or marker reconstruction.
use super::*;
use hirsel_lir::ownership_trace::{self as trace, Plan, Point};

pub(super) trait Reporter {
    type Output;
    fn edge(&mut self, _old: EdgeId, _new: EdgeId) -> Result<(), PassError> {
        Ok(())
    }
    fn finalizer_edge(&mut self, _old: EdgeId, _block: BlockId) -> Result<(), PassError> {
        Ok(())
    }
    fn inserted(&mut self, _insertions: &[(BlockId, usize)]) -> Result<(), PassError> {
        Ok(())
    }
    fn prelude(&mut self, _ed: &Editor<Ready>) -> Result<(), PassError> {
        Ok(())
    }
    fn versions(&mut self, _ed: &Editor<Ready>, _before: bool) -> Result<(), PassError> {
        Ok(())
    }
    fn splice(&mut self, _splice: Splice<'_>) -> Result<(), PassError> {
        Ok(())
    }
    fn finish(self, ed: Editor<Ready>) -> Result<(LirUnit<Ready>, Self::Output), PassError>;
}

/// Positions authored by the lowering pass, with insertion order preserved.
pub(super) struct Splice<'a> {
    pub block: BlockId,
    pub index: usize,
    pub tail: BlockId,
    pub start: usize,
    pub replacement: Option<Point>,
    pub insertions: &'a [(BlockId, usize)],
}

pub(super) struct NoObservations;
impl Reporter for NoObservations {
    type Output = ();
    fn finish(self, ed: Editor<Ready>) -> Result<(LirUnit<Ready>, ()), PassError> {
        Ok((ed.finish(), ()))
    }
}

pub(super) struct Observations {
    plan: Plan,
    versions: Option<(LirUnit<Ready>, Plan)>,
    edges: BTreeMap<EdgeId, EdgeId>,
}
impl Observations {
    pub(super) fn new(plan: &Plan) -> Self {
        Self {
            plan: plan.clone(),
            versions: None,
            edges: BTreeMap::new(),
        }
    }
}
fn snapshot(ed: &Editor<Ready>) -> LirUnit<Ready> {
    let mut unit = ed.unit.clone();
    unit.instructions.clear();
    for (block, code) in unit.blocks.iter_mut().zip(&ed.code) {
        block.instructions = PoolRange::push(&mut unit.instructions, code.iter().cloned());
    }
    unit
}
fn refused(message: impl std::fmt::Display) -> PassError {
    PassError::Unsupported(format!("ownership trace cleanup: {message}"))
}
impl Reporter for Observations {
    type Output = Plan;
    fn edge(&mut self, old: EdgeId, new: EdgeId) -> Result<(), PassError> {
        if let Some(previous) = self.edges.get(&old) {
            if *previous != new {
                return Err(refused(
                    "shared observed edge has several flag replacements",
                ));
            }
            return Ok(());
        }
        let mut observed = false;
        for row in &mut self.plan.observations {
            if row.point == Point::Edge(old) {
                row.point = Point::Edge(new);
                observed = true;
            }
        }
        if observed {
            self.edges.insert(old, new);
        }
        Ok(())
    }
    fn finalizer_edge(&mut self, old: EdgeId, block: BlockId) -> Result<(), PassError> {
        for row in &mut self.plan.observations {
            if row.point == Point::Edge(old) {
                row.point = Point::FinalizerDrop { block };
            }
        }
        Ok(())
    }
    fn inserted(&mut self, insertions: &[(BlockId, usize)]) -> Result<(), PassError> {
        let points = self
            .plan
            .observations
            .iter_mut()
            .map(|row| &mut row.point)
            .chain(
                self.plan
                    .cleanups
                    .iter_mut()
                    .flat_map(|row| [&mut row.point]),
            );
        for point in points {
            let at = match point {
                Point::Instruction { block, index, .. }
                | Point::ReleaseDrop { block, index }
                | Point::CompoundDrop {
                    normal: block,
                    index,
                    ..
                } => Some((block, index)),
                _ => None,
            };
            if let Some((block, index)) = at {
                for &(at_block, at_index) in insertions {
                    if *block == at_block && *index as usize >= at_index {
                        *index = index
                            .checked_add(1)
                            .ok_or_else(|| refused("flag insertion index overflow"))?;
                    }
                }
            }
        }
        Ok(())
    }
    fn prelude(&mut self, ed: &Editor<Ready>) -> Result<(), PassError> {
        let unit = snapshot(ed);
        self.plan.unit = unit.digest();
        trace::check(&unit, &self.plan).map_err(refused)?;
        Ok(())
    }
    fn versions(&mut self, ed: &Editor<Ready>, before: bool) -> Result<(), PassError> {
        let unit = snapshot(ed);
        if before {
            let mut plan =
                Plan::selected(&unit, self.plan.selection, self.plan.observations.clone());
            plan.cleanups = self.plan.cleanups.clone();
            trace::check(&unit, &plan).map_err(refused)?;
            self.versions = Some((unit, plan));
        } else {
            let (input, plan) = self
                .versions
                .take()
                .ok_or_else(|| refused("missing version input"))?;
            // join_read_versions changes reaching-version names and appends
            // Plain phi parameters/arguments. Instruction and existing owned
            // parameter coordinates stay fixed. Author an explicit identity
            // recipe and check every original marker against both boundaries.
            let (_, output, _) =
                crate::observation::identity(&input, &plan, unit).map_err(refused)?;
            self.plan = output;
        }
        Ok(())
    }
    fn splice(&mut self, splice: Splice<'_>) -> Result<(), PassError> {
        let Splice {
            block,
            index,
            tail,
            start,
            replacement,
            insertions,
        } = splice;
        // The suffix is detached before auxiliary instructions are inserted.
        // Its final start and the replacement already include those insertions;
        // only points still in the editor need the chronological index shifts.
        let inserted = |point: Point| -> Result<Point, PassError> {
            let shift = |block, mut index: u32| -> Result<u32, PassError> {
                for &(b, at) in insertions {
                    if b == block && index as usize >= at {
                        index = index
                            .checked_add(1)
                            .ok_or_else(|| refused("instruction index overflow"))?;
                    }
                }
                Ok(index)
            };
            Ok(match point {
                Point::Instruction {
                    block,
                    index,
                    moment,
                } => Point::Instruction {
                    block,
                    index: shift(block, index)?,
                    moment,
                },
                Point::ReleaseDrop { block, index } => Point::ReleaseDrop {
                    block,
                    index: shift(block, index)?,
                },
                Point::CompoundDrop {
                    block,
                    normal,
                    index,
                    failure,
                } => Point::CompoundDrop {
                    block,
                    normal,
                    index: shift(normal, index)?,
                    failure,
                },
                _ => point,
            })
        };
        let mut transport = |point: &mut Point, marker: trace::MarkerId| -> Result<(), PassError> {
            let relocate = |old: u32| start as u32 + old - index as u32 - 1;
            *point = match *point {
                Point::Instruction {
                    block: b,
                    index: i,
                    moment,
                } if b == block => {
                    if i as usize == index {
                        replacement.ok_or_else(|| {
                            refused(format!(
                                "compound drop {:?} needs a transport recipe",
                                marker
                            ))
                        })?
                    } else if i as usize > index {
                        Point::Instruction {
                            block: tail,
                            index: relocate(i),
                            moment,
                        }
                    } else {
                        inserted(*point)?
                    }
                }
                Point::ReleaseDrop { block: b, index: i } if b == block && i as usize > index => {
                    Point::ReleaseDrop {
                        block: tail,
                        index: relocate(i),
                    }
                }
                Point::Return(b) if b == block => Point::Return(tail),
                Point::StagedReturn { block: b, address } if b == block => Point::StagedReturn {
                    block: tail,
                    address,
                },
                Point::CallEntered(b) if b == block => Point::CallEntered(tail),
                Point::CallReturned(b) if b == block => Point::CallReturned(tail),
                Point::CompoundDrop {
                    block: entry,
                    normal,
                    index: cut,
                    failure,
                } => {
                    let shifted = inserted(*point)?;
                    let Point::CompoundDrop {
                        index: shifted_cut, ..
                    } = shifted
                    else {
                        unreachable!()
                    };
                    Point::CompoundDrop {
                        block: if entry == block { tail } else { entry },
                        normal: if normal == block && cut as usize > index {
                            tail
                        } else {
                            normal
                        },
                        index: if normal == block && cut as usize > index {
                            relocate(cut)
                        } else {
                            shifted_cut
                        },
                        // This is an external successor's entry boundary,
                        // not a position in the detached instruction suffix.
                        // Its incoming edges still enter the original block
                        // and must stop before that independent cleanup.
                        failure,
                    }
                }
                Point::FinalizerDrop { block: b } if b == block => {
                    Point::FinalizerDrop { block: tail }
                }
                _ => inserted(*point)?,
            };
            Ok(())
        };
        for observation in &mut self.plan.observations {
            transport(&mut observation.point, observation.marker.id)?;
        }
        for extra in &mut self.plan.cleanups {
            transport(&mut extra.point, extra.marker)?;
        }
        Ok(())
    }
    fn finish(self, ed: Editor<Ready>) -> Result<(LirUnit<Ready>, Plan), PassError> {
        let before = snapshot(&ed);
        let mut plan = Plan::selected(&before, self.plan.selection, self.plan.observations);
        plan.cleanups = self.plan.cleanups;
        let origins: Vec<Vec<Option<u32>>> = ed
            .code
            .iter()
            .map(|code| (0..code.len()).map(|index| Some(index as u32)).collect())
            .collect();
        let (after, plan, _) =
            crate::observation::finish(&before, &plan, ed, &origins).map_err(refused)?;
        Ok((after, plan))
    }
}

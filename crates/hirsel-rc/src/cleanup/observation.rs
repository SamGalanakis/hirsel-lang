//! Pass-authored cleanup splices; ordinary lowering instantiates only the
//! zero-sized reporter. No instruction matching or marker reconstruction.
use super::*;
use hirsel_lir::ownership_trace::{self as trace, Plan, Point};

pub(super) trait Reporter {
    type Output;
    fn prelude(&mut self, _ed: &Editor<Ready>) -> Result<(), PassError> {
        Ok(())
    }
    fn inserted(&mut self, _block: BlockId, _index: usize) -> Result<(), PassError> {
        Ok(())
    }
    fn versions(&mut self, _ed: &Editor<Ready>, _before: bool) -> Result<(), PassError> {
        Ok(())
    }
    fn splice(
        &mut self,
        _block: BlockId,
        _index: usize,
        _tail: BlockId,
        _start: usize,
        _replacement: Option<Point>,
    ) -> Result<(), PassError> {
        Ok(())
    }
    fn finish(self, ed: Editor<Ready>) -> Result<(LirUnit<Ready>, Self::Output), PassError>;
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
    versions: Option<ContentHash>,
}
impl Observations {
    pub(super) fn new(plan: &Plan) -> Self {
        Self {
            plan: plan.clone(),
            versions: None,
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
    fn prelude(&mut self, ed: &Editor<Ready>) -> Result<(), PassError> {
        let unit = snapshot(ed);
        self.plan.unit = unit.digest();
        trace::check(&unit, &self.plan).map_err(refused)?;
        Ok(())
    }
    fn inserted(&mut self, _block: BlockId, _index: usize) -> Result<(), PassError> {
        // Auxiliary address insertion needs an explicit interleaving map.
        Err(refused("auxiliary insertion needs a transport recipe"))
    }
    fn versions(&mut self, ed: &Editor<Ready>, before: bool) -> Result<(), PassError> {
        let digest = snapshot(ed).digest();
        if before {
            self.versions = Some(digest);
        } else if self.versions != Some(digest) {
            return Err(refused("version repair needs a transport recipe"));
        }
        Ok(())
    }
    fn splice(
        &mut self,
        block: BlockId,
        index: usize,
        tail: BlockId,
        start: usize,
        replacement: Option<Point>,
    ) -> Result<(), PassError> {
        for observation in &mut self.plan.observations {
            let relocate = |old: u32| start as u32 + old - index as u32 - 1;
            observation.point = match observation.point {
                Point::Instruction {
                    block: b,
                    index: i,
                    moment,
                } if b == block => {
                    if i as usize == index {
                        replacement.ok_or_else(|| {
                            refused(format!(
                                "compound drop {:?} needs a transport recipe",
                                observation.marker.id
                            ))
                        })?
                    } else if i as usize > index {
                        Point::Instruction {
                            block: tail,
                            index: relocate(i),
                            moment,
                        }
                    } else {
                        observation.point
                    }
                }
                Point::ReleaseDrop { block: b, index: i } if b == block && i as usize > index => {
                    Point::ReleaseDrop {
                        block: tail,
                        index: relocate(i),
                    }
                }
                Point::Return(b) if b == block => Point::Return(tail),
                Point::CallEntered(b) if b == block => Point::CallEntered(tail),
                Point::CallReturned(b) if b == block => Point::CallReturned(tail),
                Point::FinalizerDrop { block: b } if b == block => {
                    Point::FinalizerDrop { block: tail }
                }
                _ => observation.point,
            };
        }
        Ok(())
    }
    fn finish(self, ed: Editor<Ready>) -> Result<(LirUnit<Ready>, Plan), PassError> {
        let before = snapshot(&ed);
        let plan = Plan::new(&before, self.plan.observations);
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

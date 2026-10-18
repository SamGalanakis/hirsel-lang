//! Mutexes and atomics (CONC-4, CONC-5, FAIL-4): the runtime's cells
//! (`hirsel-rt/src/sync.rs`), each holding its pointer-free payload as the
//! `Some` image of the compiled bodies' `Option`, which a create, put or store
//! copies in and a take or load copies out. A dead mutex keeps the failure
//! chain of the body that killed it.
use super::*;

#[derive(Clone)]
struct Cell {
    /// The payload's `Some` image, absent while a mutex body holds it.
    image: Option<Extent>,
    dead: Option<Vec<(ObservedFailure, Vec<u32>)>>,
}

#[derive(Clone, Default)]
pub(super) struct Synchronizers {
    cells: Vec<Cell>,
}

impl Synchronizers {
    fn cell(&mut self, word: u64) -> Checked<&mut Cell> {
        self.cells
            .get_mut(word as usize)
            .ok_or_else(|| err(V::Malformed, "a Mutex or Atomic handle of another run"))
    }
}

impl<S: Stage> Machine<'_, S> {
    /// The image a `Some` argument passes by value.
    fn sync_image(
        &mut self,
        f: &Frame,
        arguments: &[CallArgument],
        ordinal: u32,
    ) -> Checked<Extent> {
        let ArgumentValue::Value(value) = Self::task_argument(arguments, ordinal)?.value else {
            return fail(V::Malformed, "a payload image passed by address");
        };
        match self.read_use(f, value)? {
            Val::Image(image) => Ok(image),
            _ => fail(V::TypeConfusion, "a payload image that is not an aggregate"),
        }
    }

    /// Copy `image` into the `None` slot argument `ordinal`.
    fn sync_fill(
        &mut self,
        f: &Frame,
        arguments: &[CallArgument],
        ordinal: u32,
        image: Extent,
    ) -> Checked<()> {
        let r = self.task_slot(f, &Self::task_argument(arguments, ordinal)?)?;
        let size = self.size(r.layout)?;
        if image.len() != size {
            return fail(V::Malformed, "a Mutex or Atomic image of another size");
        }
        let owned = self.resources_of(r.layout)?;
        // The slot holds `None`: plain bits with nothing to release.
        self.take_part(r.ptr, size, &owned);
        self.write_value(
            r.ptr,
            r.layout,
            Moved {
                val: Val::Image(image),
                credited: false,
                borrow: None,
            },
        )
    }

    pub(super) fn sync_operation(
        &mut self,
        f: &mut Frame,
        site: &CallSite,
        name: &str,
        arguments: &[CallArgument],
        normal: Option<EdgeId>,
    ) -> Checked<Control> {
        match name {
            "$intrinsic.MutexCreate" | "$intrinsic.AtomicCreate" => {
                let image = self.sync_image(f, arguments, 0)?;
                let s = self.scheduler();
                s.sync.cells.push(Cell {
                    image: Some(image),
                    dead: None,
                });
                let word = (s.sync.cells.len() - 1) as u64;
                self.task_yield()?;
                let result = site
                    .result
                    .ok_or_else(|| err(V::Malformed, "a cell creation without a result"))?;
                self.task_return(f, arguments, Some((result, Val::Handle(word))), normal)
            }
            "$intrinsic.MutexTake" => {
                let word = self.task_word(f, &Self::task_argument(arguments, 0)?)?;
                let cell = self.scheduler().sync.cell(word)?;
                let status = if cell.dead.is_some() {
                    1
                } else {
                    let image = cell.image.take().ok_or_else(|| {
                        err(V::Malformed, "a mutex acquired while a body holds it")
                    })?;
                    self.sync_fill(f, arguments, 1, image)?;
                    0
                };
                let result = site
                    .result
                    .ok_or_else(|| err(V::Malformed, "a mutex acquisition without a status"))?;
                let val = Val::Bits {
                    scalar: ScalarKind::U32,
                    bits: status,
                };
                self.task_return(f, arguments, Some((result, val)), normal)
            }
            "$intrinsic.MutexPut" | "$intrinsic.AtomicStore" => {
                let word = self.task_word(f, &Self::task_argument(arguments, 0)?)?;
                let image = self.sync_image(f, arguments, 1)?;
                self.scheduler().sync.cell(word)?.image = Some(image);
                self.task_yield()?;
                self.task_return(f, arguments, None, normal)
            }
            "$intrinsic.MutexKill" => {
                let word = self.task_word(f, &Self::task_argument(arguments, 0)?)?;
                let s = self.scheduler();
                let current = s.current;
                let path = s.task(current)?.path.clone();
                let chain = self.chain(&path);
                self.scheduler().sync.cell(word)?.dead = Some(chain);
                self.task_yield()?;
                self.task_return(f, arguments, None, normal)
            }
            "$intrinsic.MutexCause" => {
                let word = self.task_word(f, &Self::task_argument(arguments, 0)?)?;
                let chain = self
                    .scheduler()
                    .sync
                    .cell(word)?
                    .dead
                    .clone()
                    .ok_or_else(|| err(V::Malformed, "the cause of a mutex that is not dead"))?;
                self.append_chain(chain);
                self.task_return(f, arguments, None, normal)
            }
            "$intrinsic.AtomicLoad" => {
                let word = self.task_word(f, &Self::task_argument(arguments, 0)?)?;
                let image = self
                    .scheduler()
                    .sync
                    .cell(word)?
                    .image
                    .clone()
                    .ok_or_else(|| err(V::Malformed, "an atomic without its value"))?;
                self.sync_fill(f, arguments, 1, image)?;
                self.task_yield()?;
                self.task_return(f, arguments, None, normal)
            }
            _ => unreachable!("a synchronization operation"),
        }
    }
}

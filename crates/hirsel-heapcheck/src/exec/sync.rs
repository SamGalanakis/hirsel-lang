//! Mutexes and atomics (CONC-4, CONC-5, FAIL-4): the runtime's cells
//! (`hirsel-rt/src/sync.rs`). An atomic's cell holds its scalar payload as the
//! `Some` image of the compiled bodies' `Option`, which a create or store copies
//! in and a load copies out. A mutex's payload crosses in an `Option[List[T]]`
//! carrier, as a channel message does: a create or put moves the carrier's
//! `Some` into a harness root, leaving `None`, and a take moves it back into a
//! `None` carrier. The payloads the mutexes still hold are dropped when the run
//! finishes, as the runtime releases them at the run's end. A dead mutex keeps
//! the failure chain of the body that killed it.
use super::*;

/// The harness root of mutex cell `index`'s payload carrier.
const MUTEX_ROOT: u64 = 1 << 60;
/// Synchronizer words occupy their own namespace, like channel endpoints.
/// Bare cell indices can alias closed handles returned by host operations.
const SYNCHRONIZER: u64 = 1 << 61;

#[derive(Clone)]
struct Cell {
    /// An atomic's `Some` image.
    image: Option<Extent>,
    /// A mutex's carrier layout; its payload is the root `MUTEX_ROOT | index`
    /// unless a mutex body holds it.
    carrier: Option<LayoutId>,
    dead: Option<Vec<(ObservedFailure, Vec<u32>)>>,
}

#[derive(Clone, Default)]
pub(super) struct Synchronizers {
    cells: Vec<Cell>,
}

impl Synchronizers {
    fn cell(&mut self, word: u64) -> Checked<&mut Cell> {
        if word & SYNCHRONIZER == 0 {
            return fail(V::Malformed, "a Mutex or Atomic handle of another run");
        }
        self.cells
            .get_mut((word & !SYNCHRONIZER) as usize)
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

    /// Move the `Some` carrier argument `ordinal` into mutex cell `index`'s root,
    /// leaving `None`; the carrier's layout.
    fn carry_in(
        &mut self,
        f: &Frame,
        arguments: &[CallArgument],
        ordinal: u32,
        index: u64,
    ) -> Checked<LayoutId> {
        let r = self.task_slot(f, &Self::task_argument(arguments, ordinal)?)?;
        let size = self.size(r.layout)?;
        let owned = self.resources_of(r.layout)?;
        let image = self.take_part(r.ptr, size, &owned);
        let (val, _, credited) = self.val_from(image, ValueRepr::Aggregate, r.layout)?;
        let mut none = Extent::new(size);
        none.write_bytes(0, &vec![0; size as usize]);
        self.write_value(
            r.ptr,
            r.layout,
            Moved {
                val: Val::Image(none),
                credited: false,
                borrow: None,
            },
        )?;
        self.set_root(
            MUTEX_ROOT | index,
            Some(Held {
                val,
                credits: u32::from(credited),
                borrow: None,
            }),
        );
        Ok(r.layout)
    }

    /// Move mutex cell `index`'s payload into the `None` carrier argument `ordinal`.
    fn carry_out(
        &mut self,
        f: &Frame,
        arguments: &[CallArgument],
        ordinal: u32,
        index: u64,
    ) -> Checked<()> {
        let held = self
            .roots
            .get(&(MUTEX_ROOT | index))
            .cloned()
            .ok_or_else(|| err(V::Malformed, "a mutex acquired while a body holds it"))?;
        self.set_root(MUTEX_ROOT | index, None);
        let r = self.task_slot(f, &Self::task_argument(arguments, ordinal)?)?;
        let size = self.size(r.layout)?;
        let owned = self.resources_of(r.layout)?;
        // The carrier holds `None`: plain bits with nothing to release.
        self.take_part(r.ptr, size, &owned);
        self.write_value(
            r.ptr,
            r.layout,
            Moved {
                val: held.val,
                credited: held.credits > 0,
                borrow: None,
            },
        )
    }

    /// The run finished: drop work for every payload the mutexes still hold.
    pub(super) fn end_mutexes(&mut self) -> Checked<Vec<Work>> {
        let Some(s) = self.tasks.as_deref_mut() else {
            return Ok(Vec::new());
        };
        let cells: Vec<_> = std::mem::take(&mut s.sync.cells)
            .into_iter()
            .enumerate()
            .filter_map(|(index, cell)| cell.carrier.map(|layout| (index as u64, layout)))
            .collect();
        let mut work = Vec::new();
        for (index, layout) in cells {
            let Some(held) = self.roots.get(&(MUTEX_ROOT | index)).cloned() else {
                continue;
            };
            self.set_root(MUTEX_ROOT | index, None);
            let temp = self.temporary(layout)?;
            self.write_value(
                temp,
                layout,
                Moved {
                    val: held.val,
                    credited: held.credits > 0,
                    borrow: None,
                },
            )?;
            work.push(Work::Death {
                alloc: temp.alloc,
                raw: None,
            });
            work.push(Work::DropAt {
                at: temp,
                layout,
                mask: None,
            });
        }
        Ok(work)
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
            "$intrinsic.MutexCreate" => {
                let word = self.scheduler().sync.cells.len() as u64;
                let carrier = self.carry_in(f, arguments, 0, word)?;
                self.scheduler().sync.cells.push(Cell {
                    image: None,
                    carrier: Some(carrier),
                    dead: None,
                });
                self.task_yield()?;
                let result = site
                    .result
                    .ok_or_else(|| err(V::Malformed, "a cell creation without a result"))?;
                self.task_return(
                    f,
                    arguments,
                    Some((result, Val::Handle(SYNCHRONIZER | word))),
                    normal,
                )
            }
            "$intrinsic.AtomicCreate" => {
                let image = self.sync_image(f, arguments, 0)?;
                let s = self.scheduler();
                s.sync.cells.push(Cell {
                    image: Some(image),
                    carrier: None,
                    dead: None,
                });
                let word = (s.sync.cells.len() - 1) as u64;
                self.task_yield()?;
                let result = site
                    .result
                    .ok_or_else(|| err(V::Malformed, "a cell creation without a result"))?;
                self.task_return(
                    f,
                    arguments,
                    Some((result, Val::Handle(SYNCHRONIZER | word))),
                    normal,
                )
            }
            "$intrinsic.MutexTake" => {
                let word = self.task_word(f, &Self::task_argument(arguments, 0)?)?;
                let dead = self.scheduler().sync.cell(word)?.dead.is_some();
                let status = if dead {
                    1
                } else {
                    self.carry_out(f, arguments, 1, word & !SYNCHRONIZER)?;
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
            "$intrinsic.MutexPut" => {
                let word = self.task_word(f, &Self::task_argument(arguments, 0)?)?;
                self.scheduler().sync.cell(word)?;
                if self
                    .roots
                    .contains_key(&(MUTEX_ROOT | (word & !SYNCHRONIZER)))
                {
                    return fail(V::Malformed, "a mutex payload put back twice");
                }
                self.carry_in(f, arguments, 1, word & !SYNCHRONIZER)?;
                self.task_yield()?;
                self.task_return(f, arguments, None, normal)
            }
            "$intrinsic.AtomicStore" => {
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

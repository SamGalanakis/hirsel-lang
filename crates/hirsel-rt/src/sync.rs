//! Mutexes and atomics of one run (CONC-4, CONC-5, FAIL-4).
//!
//! The scheduler is cooperative and a mutex body may not yield, so no task ever observes a
//! mutex held by another; sequentially consistent atomics are plain loads and stores. Each
//! Mutex or Atomic is a run-local handle word naming a cell. An atomic's cell holds its scalar
//! payload as the `Some` image of the compiled bodies' `Option`, which the runtime copies. A
//! mutex's cell holds its payload as the block of a one-element list, as a channel message
//! crosses, so the runtime moves block references and never reads the payload's layout. A cell
//! lives until the run ends, as a copyable Mutex may (the layout of `Mutex[T]` requires a
//! copyable payload, whose destruction is unobservable), and the run's end releases the blocks
//! its mutexes still hold through their own descriptors.
//!
//! A mutex body that fails kills its mutex: the cell keeps the failure chain, which a later
//! `mutex.with` continues as the cause of its FAIL-4 failure.

use std::ptr::NonNull;

use crate::Header;
use crate::failure::Chained;
use crate::fault;

enum Payload {
    /// An atomic's `Some` image.
    Image(Vec<u8>),
    /// A mutex's one-element list block.
    Block(NonNull<Header>),
}

struct Cell {
    generation: u32,
    /// Absent while a mutex body holds the payload.
    payload: Option<Payload>,
    /// A dead mutex's failure chain.
    dead: Option<Vec<Chained>>,
}

/// The run's mutex and atomic cells.
#[derive(Default)]
pub(crate) struct Synchronizers {
    cells: Vec<Cell>,
    generation: u32,
}

impl Synchronizers {
    fn create(&mut self, payload: Payload) -> u64 {
        self.generation = self.generation.wrapping_add(1).max(1);
        self.cells.push(Cell {
            generation: self.generation,
            payload: Some(payload),
            dead: None,
        });
        u64::from(self.generation) << 32 | (self.cells.len() - 1) as u64
    }

    /// A fresh atomic cell holding `size` bytes at `image`.
    ///
    /// # Safety
    /// `image` is readable for `size` bytes.
    pub(crate) unsafe fn create_image(&mut self, image: *const u8, size: u64) -> u64 {
        // SAFETY: caller contract.
        self.create(Payload::Image(unsafe { bytes(image, size) }.to_vec()))
    }

    /// A fresh mutex cell owning the payload block `block`.
    pub(crate) fn create_block(&mut self, block: NonNull<Header>) -> u64 {
        self.create(Payload::Block(block))
    }

    fn cell(&mut self, word: u64) -> &mut Cell {
        match self.cells.get_mut(word as u32 as usize) {
            Some(cell) if cell.generation == (word >> 32) as u32 => cell,
            _ => fault("a Mutex or Atomic handle of another run"),
        }
    }

    /// Copy the atomic's image into `slot`.
    ///
    /// # Safety
    /// `slot` is writable for `size` bytes.
    pub(crate) unsafe fn read(&mut self, word: u64, slot: *mut u8, size: u64) {
        let Some(Payload::Image(image)) = &self.cell(word).payload else {
            fault("an atomic load of a cell without an image");
        };
        if image.len() as u64 != size {
            fault("an Atomic image of another size");
        }
        // SAFETY: caller contract.
        unsafe { std::ptr::copy_nonoverlapping(image.as_ptr(), slot, image.len()) };
    }

    /// Replace the atomic's image with `size` bytes at `image`.
    ///
    /// # Safety
    /// `image` is readable for `size` bytes.
    pub(crate) unsafe fn write(&mut self, word: u64, image: *const u8, size: u64) {
        let cell = self.cell(word);
        let Some(Payload::Image(old)) = &mut cell.payload else {
            fault("an atomic store to a cell without an image");
        };
        if old.len() as u64 != size {
            fault("an Atomic image of another size");
        }
        // SAFETY: caller contract.
        old.copy_from_slice(unsafe { bytes(image, size) });
    }

    /// Move the mutex's payload block out; the cell is empty until a put.
    pub(crate) fn take(&mut self, word: u64) -> NonNull<Header> {
        match self.cell(word).payload.take() {
            Some(Payload::Block(block)) => block,
            Some(Payload::Image(_)) => fault("a mutex acquisition of an atomic"),
            None => fault("a mutex acquired while a body holds it"),
        }
    }

    /// Give the mutex its payload block back.
    pub(crate) fn put(&mut self, word: u64, block: NonNull<Header>) {
        let cell = self.cell(word);
        if cell.payload.is_some() {
            fault("a mutex payload put back twice");
        }
        cell.payload = Some(Payload::Block(block));
    }

    /// The failure chain a dead mutex keeps, if it is dead.
    pub(crate) fn dead(&mut self, word: u64) -> Option<&Vec<Chained>> {
        self.cell(word).dead.as_ref()
    }

    /// Kill the mutex with the failure chain of its body.
    pub(crate) fn kill(&mut self, word: u64, chain: Vec<Chained>) {
        self.cell(word).dead = Some(chain);
    }

    /// The run ended: every payload block the mutexes still hold, for the caller to release.
    pub(crate) fn end(&mut self) -> Vec<NonNull<Header>> {
        self.cells
            .drain(..)
            .filter_map(|cell| match cell.payload {
                Some(Payload::Block(block)) => Some(block),
                _ => None,
            })
            .collect()
    }
}

/// # Safety
/// `image` is readable for `size` bytes.
unsafe fn bytes<'a>(image: *const u8, size: u64) -> &'a [u8] {
    let size = usize::try_from(size).unwrap_or_else(|_| fault("a payload size overflows"));
    // SAFETY: caller contract.
    unsafe { std::slice::from_raw_parts(image, size) }
}

//! Mutexes and atomics of one run (CONC-4, CONC-5, FAIL-4).
//!
//! The scheduler is cooperative and a mutex body may not yield, so no task ever observes a
//! mutex held by another; sequentially consistent atomics are plain loads and stores. Each
//! Mutex or Atomic is a run-local handle word naming a cell that holds its payload as the `Some`
//! image of the compiled bodies' `Option`: the payload is pointer-free with no drop (the layout
//! of `Mutex[T]` requires it, and atomics are scalars), so the runtime copies images and never
//! reads a payload's layout. A cell lives until the run ends, as a copyable Mutex may.
//!
//! A mutex body that fails kills its mutex: the cell keeps the failure chain, which a later
//! `mutex.with` continues as the cause of its FAIL-4 failure.

use crate::failure::Chained;
use crate::fault;

struct Cell {
    generation: u32,
    /// The payload's `Some` image, absent while a body holds it.
    image: Option<Vec<u8>>,
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
    /// A fresh cell holding `size` bytes at `image`.
    ///
    /// # Safety
    /// `image` is readable for `size` bytes.
    pub(crate) unsafe fn create(&mut self, image: *const u8, size: u64) -> u64 {
        self.generation = self.generation.wrapping_add(1).max(1);
        // SAFETY: caller contract.
        let image = unsafe { bytes(image, size) }.to_vec();
        self.cells.push(Cell {
            generation: self.generation,
            image: Some(image),
            dead: None,
        });
        u64::from(self.generation) << 32 | (self.cells.len() - 1) as u64
    }

    fn cell(&mut self, word: u64) -> &mut Cell {
        match self.cells.get_mut(word as u32 as usize) {
            Some(cell) if cell.generation == (word >> 32) as u32 => cell,
            _ => fault("a Mutex or Atomic handle of another run"),
        }
    }

    /// Copy the cell's image into `slot`; with `take`, the cell is empty until a put.
    ///
    /// # Safety
    /// `slot` is writable for `size` bytes.
    pub(crate) unsafe fn read(&mut self, word: u64, slot: *mut u8, size: u64, take: bool) {
        let cell = self.cell(word);
        let image = if take {
            cell.image.take()
        } else {
            cell.image.clone()
        }
        .unwrap_or_else(|| fault("a mutex acquired while a body holds it"));
        if image.len() as u64 != size {
            fault("a Mutex or Atomic image of another size");
        }
        // SAFETY: caller contract.
        unsafe { std::ptr::copy_nonoverlapping(image.as_ptr(), slot, image.len()) };
    }

    /// Replace the cell's image with `size` bytes at `image`.
    ///
    /// # Safety
    /// `image` is readable for `size` bytes.
    pub(crate) unsafe fn write(&mut self, word: u64, image: *const u8, size: u64) {
        // SAFETY: caller contract.
        let image = unsafe { bytes(image, size) }.to_vec();
        self.cell(word).image = Some(image);
    }

    /// The failure chain a dead mutex keeps, if it is dead.
    pub(crate) fn dead(&mut self, word: u64) -> Option<&Vec<Chained>> {
        self.cell(word).dead.as_ref()
    }

    /// Kill the mutex with the failure chain of its body.
    pub(crate) fn kill(&mut self, word: u64, chain: Vec<Chained>) {
        self.cell(word).dead = Some(chain);
    }
}

/// # Safety
/// `image` is readable for `size` bytes.
unsafe fn bytes<'a>(image: *const u8, size: u64) -> &'a [u8] {
    let size = usize::try_from(size).unwrap_or_else(|_| fault("a payload size overflows"));
    // SAFETY: caller contract.
    unsafe { std::slice::from_raw_parts(image, size) }
}

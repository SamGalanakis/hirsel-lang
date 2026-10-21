//! Mutexes and atomics of one run (CONC-4, CONC-5, FAIL-4).
//!
//! Mutexes reuse the channel image machinery: a single `Option[T]` image stays
//! in its cell's buffer, whose storage survives take/put. Its counted handle
//! wrapper closes the cell on last release, detaching any present image for
//! descriptor-driven destruction. A failed body kills its empty cell.

use crate::channels::{Element, Ring};
use crate::failure::Chained;
use crate::fault;
use crate::release::Layout;

/// Internal handle tag, disjoint from channel endpoints and task sets.
pub(crate) const MUTEX: u64 = 1 << 61;

enum Payload {
    Reserved,
    Image(Vec<u8>),
    Mutex { element: Element, image: Ring },
    Closed,
}

struct Cell {
    generation: u32,
    payload: Payload,
    dead: Option<Vec<Chained>>,
}

/// A row allocation with no observable object yet. Only its owning command
/// can initialize it, once. Abandoned suffix reservations die with the run.
#[must_use]
pub(crate) struct Reserved {
    word: u64,
}
impl Reserved {
    pub(crate) fn word(&self) -> u64 {
        self.word
    }
}

#[derive(Default)]
pub(crate) struct Synchronizers {
    cells: Vec<Cell>,
    generation: u32,
}

impl Synchronizers {
    fn create(&mut self, payload: Payload) -> u64 {
        self.generation = (self.generation + 1) & 0x1fff_ffff;
        self.cells.push(Cell {
            generation: self.generation,
            payload,
            dead: None,
        });
        u64::from(self.generation) << 32 | (self.cells.len() - 1) as u64
    }

    pub(crate) fn reserve_image(&mut self) -> Reserved {
        Reserved {
            word: self.create(Payload::Reserved),
        }
    }

    pub(crate) fn reserve_mutex(&mut self) -> Reserved {
        Reserved {
            word: MUTEX | self.create(Payload::Reserved),
        }
    }

    /// Whether a provisional word has reached its creation's commit.
    pub(crate) fn initialized(&mut self, word: u64) -> bool {
        !matches!(self.cell(word).payload, Payload::Reserved)
    }

    fn initialize(&mut self, reservation: Reserved, payload: Payload) {
        let cell = self.cell(reservation.word);
        if !matches!(cell.payload, Payload::Reserved) {
            fault("a reserved synchronizer initialized twice");
        }
        cell.payload = payload;
    }

    /// # Safety
    /// `image` is readable for `size` bytes; the reservation belongs to this run.
    pub(crate) unsafe fn initialize_image(
        &mut self,
        reservation: Reserved,
        image: *const u8,
        size: u64,
    ) {
        if reservation.word & MUTEX != 0 {
            fault("a mutex reservation initialized as an atomic");
        }
        self.initialize(
            reservation,
            Payload::Image(unsafe { bytes(image, size) }.to_vec()),
        );
    }

    /// # Safety
    /// `slot` holds a `Some` image of `element`, and `layout` describes it.
    /// The mutex reservation belongs to this run.
    pub(crate) unsafe fn initialize_mutex(
        &mut self,
        reservation: Reserved,
        element: Element,
        layout: &Layout,
        slot: *mut u8,
    ) {
        let word = reservation.word;
        if word & MUTEX == 0 {
            fault("an atomic reservation initialized as a mutex");
        }
        let image = Ring::new(element.size, layout.align as usize);
        self.initialize(reservation, Payload::Mutex { element, image });
        unsafe { self.put(word, slot) };
    }

    /// # Safety
    /// `image` is readable for `size` bytes.
    pub(crate) unsafe fn create_image(&mut self, image: *const u8, size: u64) -> u64 {
        let reservation = self.reserve_image();
        let word = reservation.word();
        unsafe { self.initialize_image(reservation, image, size) };
        word
    }

    /// # Safety
    /// `slot` holds a `Some` image of `element`, and `layout` describes it.
    pub(crate) unsafe fn create_mutex(
        &mut self,
        element: Element,
        layout: &Layout,
        slot: *mut u8,
    ) -> u64 {
        let reservation = self.reserve_mutex();
        let word = reservation.word();
        unsafe { self.initialize_mutex(reservation, element, layout, slot) };
        word
    }

    fn cell(&mut self, word: u64) -> &mut Cell {
        match self.cells.get_mut(word as u32 as usize) {
            Some(cell) if cell.generation == ((word & !MUTEX) >> 32) as u32 => cell,
            _ => fault("a Mutex or Atomic handle of another run"),
        }
    }

    /// # Safety
    /// `slot` is writable for `size` bytes.
    pub(crate) unsafe fn read(&mut self, word: u64, slot: *mut u8, size: u64) {
        let Payload::Image(image) = &self.cell(word).payload else {
            fault("an atomic load of a cell without an image");
        };
        if image.len() as u64 != size {
            fault("an Atomic image of another size");
        }
        unsafe { std::ptr::copy_nonoverlapping(image.as_ptr(), slot, image.len()) };
    }

    /// # Safety
    /// `image` is readable for `size` bytes.
    pub(crate) unsafe fn write(&mut self, word: u64, image: *const u8, size: u64) {
        let Payload::Image(old) = &mut self.cell(word).payload else {
            fault("an atomic store to a cell without an image");
        };
        if old.len() as u64 != size {
            fault("an Atomic image of another size");
        }
        old.copy_from_slice(unsafe { bytes(image, size) });
    }

    /// # Safety
    /// `slot` is a writable `None` image of the cell's element.
    pub(crate) unsafe fn take(&mut self, word: u64, slot: *mut u8) {
        let Payload::Mutex { element, image } = &mut self.cell(word).payload else {
            fault("a mutex acquisition of another kind of cell");
        };
        if !image.pop(slot, element.size) {
            fault("a mutex acquired while a body holds it");
        }
    }

    pub(crate) fn available(&mut self, word: u64) -> bool {
        let cell = self.cell(word);
        cell.dead.is_some()
            || matches!(&cell.payload, Payload::Mutex { image, .. } if !image.is_empty())
    }

    /// # Safety
    /// `slot` is a `Some` image of the cell's element.
    pub(crate) unsafe fn put(&mut self, word: u64, slot: *mut u8) {
        let cell = self.cell(word);
        let Payload::Mutex { element, image } = &mut cell.payload else {
            fault("a mutex put to another kind of cell");
        };
        if !image.is_empty() || cell.dead.is_some() {
            fault("a mutex payload put back twice or after death");
        }
        image.push(slot, element.size);
        unsafe { std::ptr::copy_nonoverlapping(element.none.as_ptr(), slot, element.size) };
    }

    pub(crate) fn dead(&mut self, word: u64) -> Option<&Vec<Chained>> {
        self.cell(word).dead.as_ref()
    }
    pub(crate) fn kill(&mut self, word: u64, chain: Vec<Chained>) {
        self.cell(word).dead = Some(chain);
    }

    /// Detach before running user destruction, which may create or close cells.
    pub(crate) fn close(&mut self, word: u64) -> Option<(Ring, u32)> {
        let cell = self.cell(word);
        let Payload::Mutex { element, image } =
            std::mem::replace(&mut cell.payload, Payload::Closed)
        else {
            fault("a mutex closed twice or an atomic closed as a mutex");
        };
        cell.dead = None;
        (!image.is_empty() && !element.inert).then_some((image, element.descriptor))
    }

    pub(crate) fn end(&mut self) {
        // Semantic destruction belongs to the dropping task, before teardown.
        // At a replay cut, unexecuted cleanup may leave retained cell storage.
        self.cells.clear();
    }
}

/// # Safety
/// `image` is readable for `size` bytes.
unsafe fn bytes<'a>(image: *const u8, size: u64) -> &'a [u8] {
    let size = usize::try_from(size).unwrap_or_else(|_| fault("a payload size overflows"));
    unsafe { std::slice::from_raw_parts(image, size) }
}

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
pub(crate) use hirsel_free::atomic::Image as AtomicImage;
use hirsel_free::mutex::{Acquired, Permit};
use std::sync::{Arc, Mutex, MutexGuard};

/// Internal handle tag, disjoint from channel endpoints and task sets.
pub(crate) const MUTEX: u64 = 1 << 61;

enum Payload {
    Reserved,
    Image(Arc<AtomicImage>),
    Mutex(Arc<MutexCell>),
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
    stripes: Option<Arc<hirsel_free::atomic::Stripes>>,
}

pub(crate) struct MutexCell {
    pub(crate) element: Element,
    pub(crate) object: std::sync::OnceLock<hirsel_free::record::ObjectId>,
    state: Mutex<MutexImage>,
}

pub(crate) struct MutexImage {
    image: Ring,
    pub(crate) permit: Permit,
    closed: bool,
}

/// Owned storage moved out by a bounded close action. Metadata and an inert
/// image are destroyed after publication; user cleanup receives the live image.
pub(crate) struct ClosedMutex {
    image: Ring,
    descriptor: u32,
    inert: bool,
    _cell: Arc<MutexCell>,
    _dead: Option<Vec<Chained>>,
}

impl ClosedMutex {
    pub(crate) fn into_left(self) -> Option<(Ring, u32)> {
        (!self.image.is_empty() && !self.inert).then_some((self.image, self.descriptor))
    }
}

impl MutexCell {
    pub(crate) fn lock(&self) -> MutexGuard<'_, MutexImage> {
        self.state.lock().unwrap()
    }

    /// # Safety
    /// The slot holds a Some image of this cell's element.
    pub(crate) unsafe fn put(&self, state: &mut MutexImage, slot: *mut u8) -> Option<u32> {
        assert!(
            !state.closed && state.image.is_empty(),
            "a live empty mutex"
        );
        state.image.push(slot, self.element.size);
        unsafe {
            std::ptr::copy_nonoverlapping(self.element.none.as_ptr(), slot, self.element.size)
        };
        state.permit.release()
    }

    /// # Safety
    /// The slot is writable for this cell's element, and the caller owns its
    /// permit. The state lock and record stay held through publication.
    pub(crate) unsafe fn take(&self, state: &mut MutexImage, slot: *mut u8) {
        assert!(
            !state.closed && state.image.pop(slot, self.element.size),
            "a held mutex payload"
        );
    }
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
        payload_offset: u64,
        payload_size: u64,
    ) {
        if reservation.word & MUTEX != 0 {
            fault("a mutex reservation initialized as an atomic");
        }
        let image = AtomicImage::new(
            unsafe { bytes(image, size) },
            payload_offset as usize,
            payload_size as usize,
            || self.stripes.get_or_insert_with(Default::default).clone(),
        );
        self.initialize(reservation, Payload::Image(Arc::new(image)));
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
        let cell = Self::prepare_mutex(element, layout);
        unsafe { self.initialize_mutex_cell(reservation, cell, slot) };
    }

    pub(crate) fn prepare_mutex(element: Element, layout: &Layout) -> Arc<MutexCell> {
        let mut image = Ring::new(element.size, layout.align as usize);
        image.reserve_one();
        Arc::new(MutexCell {
            element,
            object: Default::default(),
            state: Mutex::new(MutexImage {
                image,
                permit: Permit::new(),
                closed: false,
            }),
        })
    }

    /// # Safety
    /// The prepared cell's slot is a readable Some carrier of its element.
    pub(crate) unsafe fn initialize_mutex_cell(
        &mut self,
        reservation: Reserved,
        cell: Arc<MutexCell>,
        slot: *mut u8,
    ) {
        let word = reservation.word;
        if word & MUTEX == 0 {
            fault("an atomic reservation initialized as a mutex");
        }
        self.initialize(reservation, Payload::Mutex(cell));
        unsafe { self.put(word, slot) };
    }

    /// # Safety
    /// `image` is readable for `size` bytes.
    pub(crate) unsafe fn create_image(
        &mut self,
        image: *const u8,
        size: u64,
        payload_offset: u64,
        payload_size: u64,
    ) -> u64 {
        let reservation = self.reserve_image();
        let word = reservation.word();
        unsafe { self.initialize_image(reservation, image, size, payload_offset, payload_size) };
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
        let image = self.image(word, size);
        image
            .access()
            .load(unsafe { std::slice::from_raw_parts_mut(slot, image.size()) });
    }

    /// # Safety
    /// `image` is readable for `size` bytes.
    pub(crate) unsafe fn write(&mut self, word: u64, image: *const u8, size: u64) {
        let old = self.image(word, size);
        old.access().store(unsafe { bytes(image, size) });
    }

    /// Resolve under the role, then retain independently while another task
    /// grows or retires the table. Access never borrows the task registry.
    pub(crate) fn image(&mut self, word: u64, size: u64) -> Arc<AtomicImage> {
        let Payload::Image(image) = &self.cell(word).payload else {
            fault("an atomic access of a cell without an image");
        };
        if image.size() as u64 != size {
            fault("an Atomic image of another size");
        }
        image.clone()
    }

    /// # Safety
    /// `slot` is a writable `None` image of the cell's element.
    pub(crate) unsafe fn take(&mut self, word: u64, slot: *mut u8) {
        let cell = self.mutex(word);
        let mut state = cell.lock();
        let mut queued = false;
        if state.permit.acquire(u32::MAX, &mut queued) != Acquired::Payload {
            fault("a mutex acquired while a body holds it");
        }
        unsafe { cell.take(&mut state, slot) };
    }

    pub(crate) fn available(&mut self, word: u64) -> bool {
        self.mutex(word).lock().permit.available()
    }

    /// # Safety
    /// `slot` is a `Some` image of the cell's element.
    pub(crate) unsafe fn put(&mut self, word: u64, slot: *mut u8) {
        let cell = self.mutex(word);
        let mut state = cell.lock();
        unsafe { cell.put(&mut state, slot) };
    }

    pub(crate) fn dead(&mut self, word: u64) -> Option<&Vec<Chained>> {
        self.cell(word).dead.as_ref()
    }
    pub(crate) fn kill(&mut self, word: u64, chain: Vec<Chained>) {
        self.kill_waiters(word, chain);
    }

    pub(crate) fn kill_waiters(&mut self, word: u64, chain: Vec<Chained>) -> Vec<u32> {
        let waiters = self.mutex(word).lock().permit.kill();
        self.set_dead(word, chain);
        waiters
    }

    pub(crate) fn set_dead(&mut self, word: u64, chain: Vec<Chained>) {
        self.cell(word).dead = Some(chain);
    }

    pub(crate) fn mutex(&mut self, word: u64) -> Arc<MutexCell> {
        let Payload::Mutex(cell) = &self.cell(word).payload else {
            fault("a mutex access of another kind of cell");
        };
        cell.clone()
    }

    /// Detach using an already-held object lock; no allocation, lock wait,
    /// backing-storage destruction or user code occurs in this action.
    pub(crate) fn close_locked(
        &mut self,
        word: u64,
        mutex: &Arc<MutexCell>,
        state: &mut MutexImage,
        empty: Ring,
    ) -> ClosedMutex {
        let cell = self.cell(word);
        let Payload::Mutex(owner) = std::mem::replace(&mut cell.payload, Payload::Closed) else {
            fault("a mutex closed twice or an atomic closed as a mutex");
        };
        assert!(
            Arc::ptr_eq(&owner, mutex),
            "the locked cell matches its live row"
        );
        state.closed = true;
        ClosedMutex {
            image: std::mem::replace(&mut state.image, empty),
            descriptor: mutex.element.descriptor,
            inert: mutex.element.inert,
            _cell: owner,
            _dead: cell.dead.take(),
        }
    }

    /// Detach before running user destruction, which may create or close cells.
    pub(crate) fn close(&mut self, word: u64) -> Option<(Ring, u32)> {
        self.close_inner(word, false)
    }

    /// Retain inert leftovers too, for initialized-value retirement evidence.
    pub(crate) fn close_images(&mut self, word: u64) -> Option<(Ring, u32)> {
        self.close_inner(word, true)
    }

    fn close_inner(&mut self, word: u64, retain_inert: bool) -> Option<(Ring, u32)> {
        let mutex = self.mutex(word);
        let mut state = mutex.lock();
        let closed = self.close_locked(word, &mutex, &mut state, Ring::new(mutex.element.size, 1));
        drop(state);
        drop(mutex);
        if retain_inert {
            (!closed.image.is_empty()).then_some((closed.image, closed.descriptor))
        } else {
            closed.into_left()
        }
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

#[cfg(test)]
mod free_atomic_tests {
    use super::*;

    #[test]
    fn resolved_images_remain_live_after_registry_growth_and_retirement() {
        let mut cells = Synchronizers::default();
        let initial = [0x13u8; 32];
        let word = unsafe {
            cells.create_image(
                initial.as_ptr(),
                initial.len() as u64,
                0,
                initial.len() as u64,
            )
        };
        let image = cells.image(word, initial.len() as u64);
        for _ in 0..128 {
            unsafe {
                cells.create_image(
                    initial.as_ptr(),
                    initial.len() as u64,
                    0,
                    initial.len() as u64,
                )
            };
        }
        cells.end();
        std::thread::scope(|scope| {
            scope
                .spawn(|| image.access().store(&[0x57; 32]))
                .join()
                .unwrap();
        });
        let mut loaded = [0; 32];
        image.access().load(&mut loaded);
        assert_eq!(loaded, [0x57; 32]);
    }
}

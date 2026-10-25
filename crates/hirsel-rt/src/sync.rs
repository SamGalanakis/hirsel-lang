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
    /// The failure chain that killed this mutex, for each later cause.
    dead: Option<Arc<Vec<Chained>>>,
    /// The wake token of each registered waiter, until it acquires, is killed
    /// or a put reserves the permit for it and takes its token.
    tokens: Vec<(u32, crate::pool::free::Token)>,
}

impl MutexImage {
    /// Room for one more waiter's token, before the section that keeps it.
    pub(crate) fn reserve_token(&mut self) {
        self.tokens.reserve(1);
    }

    /// Keep `task`'s wake token while it waits in the permit's queue.
    pub(crate) fn keep_token(&mut self, task: u32, token: crate::pool::free::Token) {
        assert!(
            self.tokens.len() < self.tokens.capacity(),
            "token room is reserved"
        );
        self.tokens.push((task, token));
    }

    /// The registered waiter `task`'s token, removed from this image.
    pub(crate) fn take_token(&mut self, task: u32) -> Option<crate::pool::free::Token> {
        let index = self.tokens.iter().position(|&(waiter, _)| waiter == task)?;
        Some(self.tokens.swap_remove(index).1)
    }

    /// The permit's waiters were killed: every token moves out whole, for a
    /// kill's wakes. The caller drops what it does not use after its section.
    pub(crate) fn take_tokens(&mut self) -> Vec<(u32, crate::pool::free::Token)> {
        std::mem::take(&mut self.tokens)
    }

    /// Kill the held permit with a chain the caller prepared: every waiter
    /// leaves the queue in FIFO order, and each later cause reads the chain.
    pub(crate) fn kill(&mut self, chain: Arc<Vec<Chained>>) -> std::collections::VecDeque<u32> {
        let waiters = self.permit.kill_detach();
        self.dead = Some(chain);
        waiters
    }

    /// The chain a dead mutex's cause continues.
    pub(crate) fn dead(&self) -> Option<&Arc<Vec<Chained>>> {
        self.dead.as_ref()
    }
}

/// Owned storage moved out by a bounded close action. Metadata and an inert
/// image are destroyed after publication; user cleanup receives the live image.
pub(crate) struct ClosedMutex {
    image: Ring,
    descriptor: u32,
    inert: bool,
    _dead: Option<Arc<Vec<Chained>>>,
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

    /// Detach under the held lock, exactly once. The transition is cell-local:
    /// no registry row, allocation, lock wait, storage destruction or user
    /// code. The registry row retires separately ([`Synchronizers::retire`]).
    pub(crate) fn close_locked(&self, state: &mut MutexImage, empty: Ring) -> ClosedMutex {
        assert!(!state.closed, "a mutex closed twice");
        state.closed = true;
        ClosedMutex {
            image: std::mem::replace(&mut state.image, empty),
            descriptor: self.element.descriptor,
            inert: self.element.inert,
            _dead: state.dead.take(),
        }
    }
}

impl Synchronizers {
    /// Rows are append-only, so no index names two cells and a word is never
    /// reissued: the generation, kept below [`MUTEX`], only rejects foreign
    /// words, and its wrap aliases nothing. A closed row stays `Closed`. The
    /// index must fit its 32 bits, or it would spill into the generation.
    fn create(&mut self, payload: Payload) -> u64 {
        let index = row_index(self.cells.len())
            .unwrap_or_else(|| fault("a run's synchronizer table is full"));
        self.generation = (self.generation + 1) & 0x1fff_ffff;
        self.cells.push(Cell {
            generation: self.generation,
            payload,
        });
        u64::from(self.generation) << 32 | u64::from(index)
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
        let image = unsafe { self.prepare_image(image, size, payload_offset, payload_size) };
        self.initialize_image_cell(reservation, image);
    }

    /// Build an atomic's image before the section that publishes its creation.
    /// # Safety
    /// `image` is readable for `size` bytes.
    pub(crate) unsafe fn prepare_image(
        &mut self,
        image: *const u8,
        size: u64,
        payload_offset: u64,
        payload_size: u64,
    ) -> Arc<AtomicImage> {
        Arc::new(AtomicImage::new(
            unsafe { bytes(image, size) },
            payload_offset as usize,
            payload_size as usize,
            || self.stripes.get_or_insert_with(Default::default).clone(),
        ))
    }

    /// Commit a reserved word to its prepared image: moves only owned pointers.
    pub(crate) fn initialize_image_cell(&mut self, reservation: Reserved, image: Arc<AtomicImage>) {
        if reservation.word & MUTEX != 0 {
            fault("a mutex reservation initialized as an atomic");
        }
        self.initialize(reservation, Payload::Image(image));
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
                dead: None,
                tokens: Vec::new(),
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

    /// A dead mutex's chain, for its cause.
    pub(crate) fn dead(&mut self, word: u64) -> Option<Vec<Chained>> {
        let chain = self.mutex(word).lock().dead.clone();
        chain.map(|chain| chain.as_ref().clone())
    }

    pub(crate) fn is_dead(&mut self, word: u64) -> bool {
        self.mutex(word).lock().dead.is_some()
    }

    pub(crate) fn kill(&mut self, word: u64, chain: Vec<Chained>) {
        self.kill_waiters(word, chain);
    }

    pub(crate) fn kill_waiters(&mut self, word: u64, chain: Vec<Chained>) -> Vec<u32> {
        let chain = Arc::new(chain);
        self.mutex(word).lock().kill(chain).into()
    }

    pub(crate) fn mutex(&mut self, word: u64) -> Arc<MutexCell> {
        let Payload::Mutex(cell) = &self.cell(word).payload else {
            fault("a mutex access of another kind of cell");
        };
        cell.clone()
    }

    /// Detach using an already-held object lock; no allocation, lock wait,
    /// backing-storage destruction or user code occurs in this action. The
    /// caller's retained cell outlives the row's reference dropped here.
    pub(crate) fn close_locked(
        &mut self,
        word: u64,
        mutex: &Arc<MutexCell>,
        state: &mut MutexImage,
        empty: Ring,
    ) -> ClosedMutex {
        let owner = self.retire(word);
        assert!(
            Arc::ptr_eq(&owner, mutex),
            "the locked cell matches its live row"
        );
        mutex.close_locked(state, empty)
    }

    /// Lifecycle administration of a closed cell: its row stops resolving.
    /// Its word is never reissued, so a role-free close retires it later.
    pub(crate) fn retire(&mut self, word: u64) -> Arc<MutexCell> {
        let cell = self.cell(word);
        let Payload::Mutex(owner) = std::mem::replace(&mut cell.payload, Payload::Closed) else {
            fault("a mutex closed twice or an atomic closed as a mutex");
        };
        owner
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

/// The next appended row's index, while it fits a word's low 32 bits.
fn row_index(len: usize) -> Option<u32> {
    u32::try_from(len).ok()
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

    /// Append-only rows make a generation wrap harmless: the wrapped words name distinct
    /// indices and still resolve to their own rows. Neither field spills into the other or
    /// into the tag.
    #[test]
    fn wrapped_generations_name_distinct_rows_and_indices_stay_in_32_bits() {
        assert_eq!(row_index(u32::MAX as usize), Some(u32::MAX));
        assert_eq!(row_index(u32::MAX as usize + 1), None);
        let mut cells = Synchronizers::default();
        cells.generation = 0x1fff_fffe;
        let last = cells.reserve_mutex().word();
        let wrapped = cells.reserve_mutex().word();
        let image = cells.reserve_image().word();
        assert_eq!(last, MUTEX | 0x1fff_ffff << 32);
        assert_eq!(wrapped, MUTEX | 1);
        assert_eq!(image, 1 << 32 | 2);
        for word in [last, wrapped, image] {
            assert!(
                !cells.initialized(word),
                "{word:#x} resolves to its reserved row"
            );
        }
    }

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

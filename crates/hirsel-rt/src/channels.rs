//! Unbounded FIFO channels between the tasks of one run (CONC-6).
//!
//! A message is carried inline: the `Some` image of the `Option[T]` the compiled `channel.send`
//! body passes, which the runtime copies into the channel's queue and replaces with the
//! channel's `None` image. A receive copies the oldest image into the compiled
//! `channel.receive` body's `None` slot. The compiled `channel.open` body gives the channel
//! `Option[T]`'s `None` image and its layout descriptor ([`Element`]), so the runtime moves
//! messages by their size and, when the last receiver closes, drops the ones nobody received
//! in place through the descriptor's drop program, oldest first. A send allocates nothing
//! but the queue's growth: the queue is one ring of images ([`Ring`]) that doubles when full.
//!
//! Sender and Receiver are run-local handle words with [`ENDPOINT`] set, an endpoint index and
//! a generation; dropping one closes it through [`close`], reached from the handle close of
//! `finalize.rs` instead of the Io adapter.

use hirsel_free::sync::{Arc, AtomicU64, Mutex, MutexGuard, Ordering};
use std::alloc::{Layout as Allocation, alloc, dealloc, handle_alloc_error};
use std::collections::{BTreeSet, VecDeque};
use std::ptr::NonNull;

use crate::fault;
use crate::release::{LAYOUT_DYNAMIC, Layout};

/// The tag of a channel endpoint's handle word.
pub(crate) const ENDPOINT: u64 = 1 << 63;

/// A send or receive status: done, closed, or canceled (`CANCELED + cause`).
pub(crate) const DONE: u32 = 0;
pub(crate) const CLOSED: u32 = 1;
pub(crate) const CANCELED: u32 = 2;

/// A channel's element: the `Option[T]` images it carries.
pub(crate) struct Element {
    /// `Option[T]`'s descriptor index, which drops an unreceived message.
    pub(crate) descriptor: u32,
    /// The image size in bytes.
    pub(crate) size: usize,
    /// The descriptor runs nothing: an unreceived message needs no drop.
    pub(crate) inert: bool,
    /// The `None` image a send leaves in the sender's slot.
    pub(crate) none: Box<[u8]>,
}

impl Element {
    /// The element of `layout`, `index`'s descriptor, whose `None` image is at `none`.
    ///
    /// # Safety
    /// `layout` is a live descriptor and `none` readable for its size.
    pub(crate) unsafe fn new(index: u32, layout: *const Layout, none: *const u8) -> Element {
        // SAFETY: caller contract.
        let layout = unsafe { &*layout };
        if layout.flags & LAYOUT_DYNAMIC != 0 {
            fault("a channel element of dynamic size");
        }
        let size = usize::try_from(layout.size)
            .ok()
            .filter(|&size| size > 0)
            .unwrap_or_else(|| fault("a channel element without an Option image"));
        Element {
            descriptor: index,
            size,
            inert: layout.finalizer.is_none() && layout.step_count == 0,
            // SAFETY: caller contract.
            none: unsafe { std::slice::from_raw_parts(none, size) }.into(),
        }
    }
}

/// Queued message images, oldest first: `len` images of `stride` bytes from slot `head` of a
/// buffer of `capacity` slots (a power of two, or zero before the first message), wrapping.
pub(crate) struct Ring {
    buffer: NonNull<u8>,
    stride: usize,
    align: usize,
    capacity: usize,
    head: usize,
    len: usize,
}

// SAFETY: a Ring owns raw image storage, with no Rust references into it.
// Runtime objects move it only under their exclusive owner or object lock;
// Hirsel's MODE-4 rules ensure that transferred payloads are sendable.
unsafe impl Send for Ring {}

impl Ring {
    pub(crate) fn reserve_one(&mut self) {
        if self.capacity == 0 {
            self.grow();
        }
    }
    pub(crate) fn new(size: usize, align: usize) -> Ring {
        let align = align.max(1).next_power_of_two();
        Ring {
            buffer: NonNull::dangling(),
            stride: size.next_multiple_of(align),
            align,
            capacity: 0,
            head: 0,
            len: 0,
        }
    }
    pub(crate) fn len(&self) -> usize {
        self.len
    }
    pub(crate) fn is_empty(&self) -> bool {
        self.len == 0
    }
    fn allocation(&self, capacity: usize) -> Allocation {
        capacity
            .checked_mul(self.stride)
            .and_then(|bytes| Allocation::from_size_align(bytes, self.align).ok())
            .unwrap_or_else(|| fault("a channel queue beyond the address space"))
    }
    /// The `i`th oldest image.
    pub(crate) fn at(&self, i: usize) -> *mut u8 {
        let slot = (self.head + i) & (self.capacity - 1);
        // SAFETY: `slot` is inside the buffer of `capacity` slots.
        unsafe { self.buffer.as_ptr().add(slot * self.stride) }
    }
    /// Append a copy of the `size`-byte image at `image`.
    pub(crate) fn push(&mut self, image: *const u8, size: usize) {
        if self.len == self.capacity {
            self.grow();
        }
        let at = self.at(self.len);
        // SAFETY: a free slot of at least `size` bytes; the image is another frame's.
        unsafe { std::ptr::copy_nonoverlapping(image, at, size) };
        self.len += 1;
    }
    /// Move the oldest image into `out`, if any.
    pub(crate) fn pop(&mut self, out: *mut u8, size: usize) -> bool {
        if self.len == 0 {
            return false;
        }
        // SAFETY: the oldest slot holds an image; `out` is another frame's.
        unsafe { std::ptr::copy_nonoverlapping(self.at(0), out, size) };
        self.head = (self.head + 1) & (self.capacity - 1);
        self.len -= 1;
        true
    }
    /// Double the buffer, keeping the images in order from slot 0.
    fn grow(&mut self) {
        let capacity = (self.capacity * 2).max(4);
        let allocation = self.allocation(capacity);
        // SAFETY: a nonzero size (the stride is nonzero).
        let buffer = NonNull::new(unsafe { alloc(allocation) })
            .unwrap_or_else(|| handle_alloc_error(allocation));
        let first = (self.capacity - self.head).min(self.len);
        // SAFETY: both runs are inside the old buffer, the new one is fresh and large enough.
        unsafe {
            if first > 0 {
                std::ptr::copy_nonoverlapping(self.at(0), buffer.as_ptr(), first * self.stride);
            }
            if self.len > first {
                std::ptr::copy_nonoverlapping(
                    self.buffer.as_ptr(),
                    buffer.as_ptr().add(first * self.stride),
                    (self.len - first) * self.stride,
                );
            }
        }
        self.release();
        self.buffer = buffer;
        self.capacity = capacity;
        self.head = 0;
    }
    fn release(&mut self) {
        if self.capacity > 0 {
            // SAFETY: the buffer was allocated with this allocation.
            unsafe { dealloc(self.buffer.as_ptr(), self.allocation(self.capacity)) };
        }
    }
}

impl Drop for Ring {
    /// Frees the storage only: images still queued were dropped or moved out first.
    fn drop(&mut self) {
        self.release();
    }
}

struct Channel {
    payload: Arc<ChannelCell>,
    /// The tasks parked receiving and their places in park order, ascending: a park appends.
    waiting: VecDeque<(u64, u32)>,
    /// A send or a sender's close since the last scheduling point.
    touched: bool,
}

pub(crate) struct ChannelCell {
    pub(crate) element: Element,
    pub(crate) object: std::sync::OnceLock<hirsel_free::record::ObjectId>,
    sequence: AtomicU64,
    /// Mirrors the role-owned park list's length. A receiver registers before
    /// its first poll takes the state lock, so a sender that reads zero under
    /// that lock leaves every receiver to see its message.
    parked: std::sync::atomic::AtomicU32,
    state: Mutex<ChannelState>,
}

pub(crate) struct ChannelState {
    queue: Ring,
    senders: u32,
    receivers: u32,
    /// Early sends are invisible to polls until their sender's turn retires.
    pending: BTreeSet<u64>,
    sent: u64,
    received: u64,
}

impl ChannelCell {
    pub(crate) fn lock(&self) -> MutexGuard<'_, ChannelState> {
        self.state.lock().unwrap()
    }

    /// Called under the object lock: one endpoint-event position is also its
    /// recording version. Tokio mpsc/list.rs:74-85,100-108 at ce685ee6916549ed
    /// uses one fetch_add for data and close positions; Hirsel includes dup
    /// and receiver close because they are replay-visible endpoint events.
    pub(crate) fn next_event(&self) -> u64 {
        self.sequence
            .fetch_add(1, Ordering::Relaxed)
            .checked_add(1)
            .expect("channel version overflow")
    }

    /// Whether a parked receiver may need a scheduling point to see a send.
    pub(crate) fn parked(&self) -> bool {
        self.parked.load(std::sync::atomic::Ordering::Relaxed) != 0
    }

    pub(crate) fn dup(&self, state: &mut ChannelState) {
        state.senders = state
            .senders
            .checked_add(1)
            .expect("channel sender count overflow");
    }

    /// Reserve queue growth before opening a phase; the lock keeps capacity
    /// available until its action and publication finish.
    pub(crate) fn prepare_send(&self, state: &mut ChannelState) {
        if state.receivers != 0 && state.queue.len == state.queue.capacity {
            state.queue.grow();
        }
    }

    /// # Safety
    /// slot is a Some carrier of this cell's element. Its lock is held.
    pub(crate) unsafe fn send(
        &self,
        state: &mut ChannelState,
        slot: *mut u8,
        early: bool,
    ) -> (u32, Option<u64>) {
        if state.receivers == 0 {
            return (CLOSED, None);
        }
        state.queue.push(slot, self.element.size);
        unsafe {
            std::ptr::copy_nonoverlapping(self.element.none.as_ptr(), slot, self.element.size)
        };
        let sequence = state.sent;
        state.sent = state
            .sent
            .checked_add(1)
            .unwrap_or_else(|| fault("a channel send sequence overflowed"));
        if early {
            state.pending.insert(sequence);
        }
        (DONE, early.then_some(sequence))
    }

    pub(crate) fn ready(&self, state: &ChannelState) -> bool {
        state.visible() > 0 || (state.queue.is_empty() && state.senders == 0)
    }

    /// # Safety
    /// slot is a writable None carrier; the object lock is held.
    pub(crate) unsafe fn receive(
        &self,
        state: &mut ChannelState,
        slot: *mut u8,
    ) -> Result<bool, u32> {
        if state.pending.contains(&state.received) {
            return Ok(false);
        }
        if state.queue.pop(slot, self.element.size) {
            state.received += 1;
            Ok(true)
        } else if state.senders == 0 {
            Err(CLOSED)
        } else {
            Ok(false)
        }
    }
}

/// Storage detached in a close section, destroyed only after publication.
pub(crate) struct ClosedChannel {
    left: Option<(Ring, u32)>,
    _row: Option<Channel>,
    _pending: BTreeSet<u64>,
}

impl ClosedChannel {
    pub(crate) fn into_left(self) -> Option<(Ring, u32)> {
        self.left
    }
}

/// A successful early send, published once by its turn's retirement.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct PendingSend {
    channel: u32,
    sequence: u64,
}

#[derive(Clone, Copy)]
struct Endpoint {
    generation: u32,
    channel: u32,
    sender: bool,
    initialized: bool,
}

/// Owned provisional endpoint; no lookup or close may observe it until init.
#[must_use]
pub(crate) struct ReservedEndpoint {
    word: u64,
}
impl ReservedEndpoint {
    pub(crate) fn word(&self) -> u64 {
        self.word
    }
}

#[must_use]
pub(crate) struct ReservedChannel {
    channel: u32,
    sender: ReservedEndpoint,
    receiver: ReservedEndpoint,
}
impl ReservedChannel {
    pub(crate) fn channel(&self) -> u32 {
        self.channel
    }

    pub(crate) fn words(&self) -> (u64, u64) {
        (self.sender.word(), self.receiver.word())
    }
}

/// The run's channels and their open endpoints.
#[derive(Default)]
pub(crate) struct Channels {
    channels: Vec<Option<Channel>>,
    vacant_channels: Vec<u32>,
    endpoints: Vec<Option<Endpoint>>,
    vacant_endpoints: Vec<u32>,
    generation: u32,
    /// The channels whose `touched` is set.
    touched: Vec<u32>,
}

impl Channels {
    /// Reserve identities only; channel counts, buffers and endpoint lookup
    /// remain absent until the owning creation command commits.
    pub(crate) fn reserve_open(&mut self) -> ReservedChannel {
        let channel = self.vacant_channels.pop().unwrap_or_else(|| {
            self.channels.push(None);
            (self.channels.len() - 1) as u32
        });
        ReservedChannel {
            channel,
            sender: self.reserve_endpoint(true),
            receiver: self.reserve_endpoint(false),
        }
    }

    pub(crate) fn initialize_open(
        &mut self,
        reservation: ReservedChannel,
        element: Element,
        layout: &Layout,
    ) {
        self.initialize_open_cell(reservation, Self::prepare_open(element, layout));
    }

    /// All storage allocation precedes a native recording section.
    pub(crate) fn prepare_open(element: Element, layout: &Layout) -> Arc<ChannelCell> {
        Arc::new(ChannelCell {
            state: Mutex::new(ChannelState {
                queue: Ring::new(element.size, layout.align as usize),
                senders: 1,
                receivers: 1,
                pending: BTreeSet::new(),
                sent: 0,
                received: 0,
            }),
            element,
            object: Default::default(),
            sequence: AtomicU64::new(1),
            parked: Default::default(),
        })
    }

    pub(crate) fn initialize_open_cell(
        &mut self,
        reservation: ReservedChannel,
        payload: Arc<ChannelCell>,
    ) {
        let channel = reservation.channel;
        if self.channels[channel as usize].is_some() {
            fault("a reserved channel initialized twice");
        }
        self.channels[channel as usize] = Some(Channel {
            payload,
            waiting: VecDeque::new(),
            touched: false,
        });
        self.initialize_endpoint(reservation.sender, channel);
        self.initialize_endpoint(reservation.receiver, channel);
    }

    /// A fresh channel of `element`s, `layout` its descriptor: its Sender and Receiver words.
    pub(crate) fn open(&mut self, element: Element, layout: &Layout) -> (u64, u64) {
        let reservation = self.reserve_open();
        let words = reservation.words();
        self.initialize_open(reservation, element, layout);
        words
    }

    pub(crate) fn reserve_dup(&mut self) -> ReservedEndpoint {
        self.reserve_endpoint(true)
    }

    /// A dup reserves no sender count until it commits. Its source may itself
    /// be provisional while logging, provided its creation commits first.
    pub(crate) fn initialize_dup(&mut self, reservation: ReservedEndpoint, sender: u64) {
        let channel = self.channel(sender, true);
        let cell = self.payload(channel);
        {
            let mut state = cell.lock();
            cell.dup(&mut state);
            cell.next_event();
        }
        self.initialize_dup_endpoint(reservation, channel);
    }

    /// The caller already committed the count under the retained cell lock.
    pub(crate) fn initialize_dup_endpoint(&mut self, reservation: ReservedEndpoint, channel: u32) {
        self.initialize_endpoint(reservation, channel);
    }

    /// Another Sender of the channel `sender` names.
    pub(crate) fn dup(&mut self, sender: u64) -> u64 {
        let reservation = self.reserve_dup();
        let word = reservation.word();
        self.initialize_dup(reservation, sender);
        word
    }

    fn reserve_endpoint(&mut self, sender: bool) -> ReservedEndpoint {
        // Below `tasks::TASK_SET`, which marks a task set's endpoint word.
        self.generation = (self.generation + 1) & 0x3fff_ffff;
        let row = Endpoint {
            generation: self.generation,
            channel: u32::MAX,
            sender,
            initialized: false,
        };
        let index = match self.vacant_endpoints.pop() {
            Some(index) => {
                self.endpoints[index as usize] = Some(row);
                index
            }
            None => {
                self.endpoints.push(Some(row));
                (self.endpoints.len() - 1) as u32
            }
        };
        ReservedEndpoint {
            word: ENDPOINT | u64::from(self.generation) << 32 | u64::from(index),
        }
    }

    fn initialize_endpoint(&mut self, reservation: ReservedEndpoint, channel: u32) {
        let word = reservation.word;
        let row = self.endpoints[word as u32 as usize]
            .as_mut()
            .unwrap_or_else(|| fault("a missing reserved endpoint"));
        if row.initialized || row.generation != ((word & !ENDPOINT) >> 32) as u32 {
            fault("a reserved endpoint initialized twice or of another run");
        }
        row.channel = channel;
        row.initialized = true;
    }

    pub(crate) fn initialized(&self, word: u64, sender: bool) -> bool {
        self.lookup(word)
            .is_some_and(|(_, endpoint)| endpoint.sender == sender)
    }

    /// The channel an open endpoint word of the given kind names.
    pub(crate) fn channel(&self, word: u64, sender: bool) -> u32 {
        match self.lookup(word) {
            Some((_, endpoint)) if endpoint.sender == sender => endpoint.channel,
            _ => fault("a channel endpoint that is closed, of another run or of the other kind"),
        }
    }

    /// The channel an open endpoint word names, and whether it is a Sender.
    pub(crate) fn endpoint_of(&self, word: u64) -> (u32, bool) {
        match self.lookup(word) {
            Some((_, endpoint)) => (endpoint.channel, endpoint.sender),
            None => fault("a channel endpoint closed twice or of another run"),
        }
    }

    /// Whether a receive on `channel` can act: a message is queued or no sender is left.
    pub(crate) fn ready(&self, channel: u32) -> bool {
        self.channels[channel as usize].as_ref().is_some_and(|row| {
            let state = row.payload.lock();
            row.payload.ready(&state)
        })
    }

    fn lookup(&self, word: u64) -> Option<(u32, Endpoint)> {
        if word & ENDPOINT == 0 {
            return None;
        }
        let index = word as u32;
        let endpoint = (*self.endpoints.get(index as usize)?)?;
        (endpoint.initialized && endpoint.generation == ((word & !ENDPOINT) >> 32) as u32)
            .then_some((index, endpoint))
    }

    fn row(&mut self, channel: u32) -> &mut Channel {
        self.channels[channel as usize]
            .as_mut()
            .unwrap_or_else(|| fault("a vacant channel row"))
    }

    pub(crate) fn payload(&self, channel: u32) -> Arc<ChannelCell> {
        self.channels[channel as usize]
            .as_ref()
            .unwrap_or_else(|| fault("a vacant channel row"))
            .payload
            .clone()
    }

    /// Queue the `Some` image in `slot` and leave `None` there, unless the channel has no
    /// receiver left (`CLOSED`).
    ///
    /// # Safety
    /// `slot` is a `Some` image of the channel's element in another frame.
    pub(crate) unsafe fn send(&mut self, channel: u32, slot: *mut u8) -> u32 {
        // SAFETY: caller contract.
        unsafe { self.send_inner(channel, slot, false) }.0
    }

    /// An E send's internal commit. It does not touch the channel: an older
    /// scheduling point would consume the touch before the message is visible.
    ///
    /// # Safety
    /// As for `send`; the executor has proved rules 1–3.
    pub(crate) unsafe fn send_pending(
        &mut self,
        channel: u32,
        slot: *mut u8,
    ) -> (u32, Option<PendingSend>) {
        // SAFETY: caller contract.
        unsafe { self.send_inner(channel, slot, true) }
    }

    unsafe fn send_inner(
        &mut self,
        channel: u32,
        slot: *mut u8,
        early: bool,
    ) -> (u32, Option<PendingSend>) {
        let cell = self.payload(channel);
        let mut state = cell.lock();
        let (status, pending) = unsafe { cell.send(&mut state, slot, early) };
        cell.next_event();
        drop(state);
        if !early && status == DONE {
            self.touch(channel);
        }
        (
            status,
            pending.map(|sequence| PendingSend { channel, sequence }),
        )
    }

    /// Retirement step 3: clear pending AND touch the channel immediately
    /// before polling. Consuming the ticket twice is an executor fault.
    pub(crate) fn publish(&mut self, send: PendingSend) {
        if !self
            .payload(send.channel)
            .lock()
            .pending
            .remove(&send.sequence)
        {
            fault("an early send was published twice");
        }
        self.touch(send.channel);
    }

    /// A parked receiver may be served: the next scheduling point wakes it.
    pub(crate) fn touch(&mut self, channel: u32) {
        let row = self.row(channel);
        if !row.waiting.is_empty() && !row.touched {
            row.touched = true;
            self.touched.push(channel);
        }
    }

    /// Move the channels touched since the last scheduling point into the empty `into`, whose
    /// storage the channels keep for the next point.
    pub(crate) fn take_touched(&mut self, into: &mut Vec<u32>) {
        std::mem::swap(&mut self.touched, into);
        for &channel in into.iter() {
            if let Some(Some(row)) = self.channels.get_mut(channel as usize) {
                row.touched = false;
            }
        }
    }

    /// `task` parked receiving on `channel` at park place `at`, the latest place so far.
    pub(crate) fn wait(&mut self, channel: u32, at: u64, task: u32) {
        let row = self.row(channel);
        row.waiting.push_back((at, task));
        row.payload.parked.store(
            row.waiting.len() as u32,
            std::sync::atomic::Ordering::Relaxed,
        );
    }
    /// The receiver parked at `at` is no longer waiting.
    pub(crate) fn unwait(&mut self, channel: u32, at: u64) {
        if let Some(Some(row)) = self.channels.get_mut(channel as usize)
            && let Some(i) = row.waiting.iter().position(|&(place, _)| place == at)
        {
            row.waiting.remove(i);
            row.payload.parked.store(
                row.waiting.len() as u32,
                std::sync::atomic::Ordering::Relaxed,
            );
        }
    }
    /// A live channel's queued message count, whether no sender is left, and its parked
    /// receivers in park order.
    pub(crate) fn waiting(&self, channel: u32) -> Option<(usize, bool, &VecDeque<(u64, u32)>)> {
        let row = self.channels.get(channel as usize)?.as_ref()?;
        let state = row.payload.lock();
        Some((
            state.visible(),
            state.queue.is_empty() && state.senders == 0,
            &row.waiting,
        ))
    }

    /// Move the oldest queued message into the `None` slot `slot`: `Ok(true)`, `Err(CLOSED)`
    /// once the queue is empty and no sender is left, or `Ok(false)` while a sender may still
    /// send.
    ///
    /// # Safety
    /// `slot` is a `None` image of the channel's element in another frame.
    pub(crate) unsafe fn receive(&mut self, channel: u32, slot: *mut u8) -> Result<bool, u32> {
        let cell = self.payload(channel);
        unsafe { cell.receive(&mut cell.lock(), slot) }
    }

    /// Reserve recycling lists before a native recording phase.
    pub(crate) fn prepare_close(&mut self) {
        self.vacant_endpoints.reserve(1);
        self.vacant_channels.reserve(1);
        self.touched.reserve(1);
    }

    /// Close the endpoint `word`. The messages a channel's last receiver leaves are returned
    /// with their descriptor index, for the caller to drop oldest first; none when the element
    /// needs no drop.
    pub(crate) fn close(&mut self, word: u64) -> Option<(Ring, u32)> {
        self.close_inner(word, false)
    }

    /// Retain inert leftovers too, for initialized-value retirement evidence.
    pub(crate) fn close_images(&mut self, word: u64) -> Option<(Ring, u32)> {
        self.close_inner(word, true)
    }

    fn close_inner(&mut self, word: u64, retain_inert: bool) -> Option<(Ring, u32)> {
        self.prepare_close();
        let (channel, _) = self.endpoint_of(word);
        let cell = self.payload(channel);
        let mut state = cell.lock();
        let closed = self.close_locked(word, &cell, &mut state);
        cell.next_event();
        drop(state);
        let left = closed.into_left();
        if cell.element.inert && !retain_inert {
            None
        } else {
            left
        }
    }

    /// Detach without freeing image storage in the phase. The caller retains
    /// the cell, holds its lock, and preallocated both recycling lists.
    pub(crate) fn close_locked(
        &mut self,
        word: u64,
        cell: &ChannelCell,
        row: &mut ChannelState,
    ) -> ClosedChannel {
        let Some((index, endpoint)) = self.lookup(word) else {
            fault("a channel endpoint closed twice or of another run");
        };
        self.endpoints[index as usize] = None;
        self.vacant_endpoints.push(index);
        let mut detached = ClosedChannel {
            left: None,
            _row: None,
            _pending: BTreeSet::new(),
        };
        if endpoint.sender {
            row.senders -= 1;
        } else {
            row.receivers -= 1;
            if row.receivers == 0 && row.queue.len() > 0 {
                let empty = Ring::new(cell.element.size, row.queue.align);
                let queue = std::mem::replace(&mut row.queue, empty);
                detached._pending = std::mem::take(&mut row.pending);
                row.received = row.sent;
                detached.left = Some((queue, cell.element.descriptor));
            }
        }
        if row.senders == 0 && row.receivers == 0 {
            detached._row = self.channels[endpoint.channel as usize].take();
            self.vacant_channels.push(endpoint.channel);
        } else if row.senders == 0 && endpoint.sender {
            // The parked receivers now see Closed.
            self.touch(endpoint.channel);
        }
        detached
    }
}

impl ChannelState {
    /// Visible FIFO prefix; a younger published message cannot pass a pending
    /// older send, even if retirement is temporarily inside a holding call.
    fn visible(&self) -> usize {
        self.pending.first().map_or(self.queue.len(), |&first| {
            usize::try_from(first - self.received)
                .unwrap_or_else(|_| fault("a channel queue beyond the address space"))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stable_cells_keep_fifo_images_across_concurrent_senders_and_registry_growth() {
        let (mut channels, _, _, index) = channel();
        let cell = channels.payload(index);
        let mut threads = Vec::new();
        for task in 0..8u64 {
            let cell = cell.clone();
            threads.push(std::thread::spawn(move || {
                let mut sent = Vec::new();
                for op in 0..32u64 {
                    let value = task * 1000 + op;
                    let mut image = [1, value];
                    let mut state = cell.lock();
                    cell.prepare_send(&mut state);
                    assert_eq!(
                        unsafe { cell.send(&mut state, image.as_mut_ptr().cast(), false) }.0,
                        DONE
                    );
                    let version = cell.next_event();
                    assert_eq!(image, [0, 0]);
                    sent.push((version, value));
                }
                sent
            }));
        }
        for _ in 0..128 {
            channels.reserve_open();
        }
        let mut sent: Vec<_> = threads
            .into_iter()
            .flat_map(|t| t.join().unwrap())
            .collect();
        sent.sort_unstable();
        assert_eq!(
            sent.iter().map(|&(v, _)| v).collect::<Vec<_>>(),
            (2..=257).collect::<Vec<_>>()
        );
        // The registry may disappear while a resolved cell is retained.
        drop(channels);
        for (_, value) in sent {
            let mut out = [0u64; 2];
            assert_eq!(
                unsafe { cell.receive(&mut cell.lock(), out.as_mut_ptr().cast()) },
                Ok(true)
            );
            assert_eq!(out, [1, value]);
        }
        assert_eq!(cell.lock().queue.len(), 0);
    }

    #[test]
    fn a_retained_cell_observes_receiver_close_before_another_send() {
        let (mut channels, sender, receiver, index) = channel();
        let cell = channels.payload(index);
        channels.close(receiver);
        channels.close(sender);
        let mut image = [1u64, 99];
        assert_eq!(
            unsafe { cell.send(&mut cell.lock(), image.as_mut_ptr().cast(), false) }.0,
            CLOSED
        );
        assert_eq!(image, [1, 99]);
    }

    fn channel() -> (Channels, u64, u64, u32) {
        let mut channels = Channels::default();
        let layout = Layout {
            magic: crate::LAYOUT_MAGIC,
            version: crate::LAYOUT_VERSION,
            size: 16,
            align: 8,
            finalizer: None,
            steps: std::ptr::null(),
            step_count: 0,
            field_count: 0,
            flags: 0,
            logical: 0,
            logical_count: std::ptr::null(),
            scan: std::ptr::null(),
            scan_words: u64::MAX,
        };
        let none = [0u64; 2];
        let element = unsafe { Element::new(0, &layout, none.as_ptr().cast()) };
        let (sender, receiver) = channels.open(element, &layout);
        let channel = channels.channel(sender, true);
        (channels, sender, receiver, channel)
    }

    #[test]
    fn an_uncommitted_dup_does_not_keep_a_channel_open() {
        let (mut channels, sender, receiver, channel) = channel();
        let reservation = channels.reserve_dup();
        let word = reservation.word();
        assert!(!channels.initialized(word, true));
        channels.close(sender);
        assert!(channels.ready(channel));
        assert!(channels.waiting(channel).unwrap().1);
        let mut out = [0u64; 2];
        assert_eq!(
            unsafe { channels.receive(channel, out.as_mut_ptr().cast()) },
            Err(CLOSED)
        );
        drop(reservation);
        channels.close(receiver);
    }

    #[test]
    fn reservations_remain_distinct_when_channels_commit_out_of_order() {
        let mut channels = Channels::default();
        let first = channels.reserve_open();
        let first_words = first.words();
        let second = channels.reserve_open();
        let second_words = second.words();
        assert_ne!(first_words, second_words);
        let layout = Layout {
            magic: crate::LAYOUT_MAGIC,
            version: crate::LAYOUT_VERSION,
            size: 16,
            align: 8,
            finalizer: None,
            steps: std::ptr::null(),
            step_count: 0,
            field_count: 0,
            flags: 0,
            logical: 0,
            logical_count: std::ptr::null(),
            scan: std::ptr::null(),
            scan_words: 0,
        };
        let none = [0u64; 2];
        let element = || unsafe { Element::new(0, &layout, none.as_ptr().cast()) };
        channels.initialize_open(second, element(), &layout);
        assert!(!channels.initialized(first_words.0, true));
        assert!(channels.initialized(second_words.0, true));
        channels.initialize_open(first, element(), &layout);
        let first_channel = channels.channel(first_words.0, true);
        let second_channel = channels.channel(second_words.0, true);
        assert_ne!(first_channel, second_channel);
        let mut message = [1u64, 55];
        unsafe { channels.send(first_channel, message.as_mut_ptr().cast()) };
        assert!(channels.ready(first_channel));
        assert!(!channels.ready(second_channel));
        for word in [first_words.0, first_words.1, second_words.0, second_words.1] {
            channels.close(word);
        }
    }

    #[test]
    fn early_send_is_invisible_and_touches_at_retirement() {
        let (mut c, _, _, channel) = channel();
        c.wait(channel, 0, 7);
        let mut image = [1u64, 42];
        let (status, pending) = unsafe { c.send_pending(channel, image.as_mut_ptr().cast()) };
        assert_eq!(status, DONE);
        assert_eq!(image, [0, 0]);
        assert!(!c.ready(channel));
        assert_eq!(c.waiting(channel).unwrap().0, 0);
        let mut out = [0u64; 2];
        assert_eq!(
            unsafe { c.receive(channel, out.as_mut_ptr().cast()) },
            Ok(false)
        );
        for _ in 0..3 {
            let mut touched = Vec::new();
            c.take_touched(&mut touched);
            assert!(
                touched.is_empty(),
                "an older head must not consume the send's touch"
            );
        }
        c.publish(pending.unwrap());
        assert!(c.ready(channel));
        assert_eq!(c.waiting(channel).unwrap().0, 1);
        let mut touched = Vec::new();
        c.take_touched(&mut touched);
        assert_eq!(touched, [channel]);
        assert_eq!(
            unsafe { c.receive(channel, out.as_mut_ptr().cast()) },
            Ok(true)
        );
        assert_eq!(out, [1, 42]);
    }

    #[test]
    fn visible_prefix_stops_at_the_first_pending_message_through_ring_growth() {
        let (mut c, _, _, channel) = channel();
        let mut pending = Vec::new();
        for n in 0..20u64 {
            let mut image = [1, n];
            if n % 3 == 0 {
                pending.push(
                    unsafe { c.send_pending(channel, image.as_mut_ptr().cast()) }
                        .1
                        .unwrap(),
                );
            } else {
                assert_eq!(unsafe { c.send(channel, image.as_mut_ptr().cast()) }, DONE);
            }
        }
        assert!(!c.ready(channel));
        // A later publication does not reveal messages behind the FIFO head.
        let later = pending.pop().unwrap();
        c.publish(later);
        assert!(!c.ready(channel));
        for send in pending {
            c.publish(send);
        }
        assert_eq!(c.waiting(channel).unwrap().0, 20);
        for n in 0..20u64 {
            let mut out = [0u64; 2];
            assert_eq!(
                unsafe { c.receive(channel, out.as_mut_ptr().cast()) },
                Ok(true)
            );
            assert_eq!(out, [1, n]);
        }
        assert!(!c.ready(channel));
    }

    #[test]
    fn a_closed_receiver_keeps_the_early_senders_value() {
        let (mut c, _, receiver, channel) = channel();
        assert!(c.close(receiver).is_none());
        let mut image = [1u64, 42];
        assert_eq!(
            unsafe { c.send_pending(channel, image.as_mut_ptr().cast()) },
            (CLOSED, None)
        );
        assert_eq!(image, [1, 42]);
    }

    #[test]
    fn published_messages_and_closed_are_seen_in_fifo_order() {
        let (mut c, sender, _, channel) = channel();
        let mut image = [1u64, 42];
        let pending = unsafe { c.send_pending(channel, image.as_mut_ptr().cast()) }
            .1
            .unwrap();
        assert!(c.close(sender).is_none());
        assert!(!c.ready(channel));
        assert!(!c.waiting(channel).unwrap().1);
        c.publish(pending);
        let mut out = [0u64; 2];
        assert_eq!(
            unsafe { c.receive(channel, out.as_mut_ptr().cast()) },
            Ok(true)
        );
        assert_eq!(out, [1, 42]);
        assert!(c.ready(channel));
        assert_eq!(
            unsafe { c.receive(channel, out.as_mut_ptr().cast()) },
            Err(CLOSED)
        );
    }
}

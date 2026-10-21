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

use std::alloc::{Layout as Allocation, alloc, dealloc, handle_alloc_error};
use std::collections::VecDeque;
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

impl Ring {
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
    element: Element,
    queue: Ring,
    senders: u32,
    receivers: u32,
    /// The tasks parked receiving and their places in park order, ascending: a park appends.
    waiting: VecDeque<(u64, u32)>,
    /// A send or a sender's close since the last scheduling point.
    touched: bool,
}

#[derive(Clone, Copy)]
struct Endpoint {
    generation: u32,
    channel: u32,
    sender: bool,
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
    /// A fresh channel of `element`s, `layout` its descriptor: its Sender and Receiver words.
    pub(crate) fn open(&mut self, element: Element, layout: &Layout) -> (u64, u64) {
        let row = Channel {
            queue: Ring::new(element.size, layout.align as usize),
            element,
            senders: 1,
            receivers: 1,
            waiting: VecDeque::new(),
            touched: false,
        };
        let channel = match self.vacant_channels.pop() {
            Some(index) => {
                self.channels[index as usize] = Some(row);
                index
            }
            None => {
                self.channels.push(Some(row));
                (self.channels.len() - 1) as u32
            }
        };
        (self.endpoint(channel, true), self.endpoint(channel, false))
    }

    /// Another Sender of the channel `sender` names.
    pub(crate) fn dup(&mut self, sender: u64) -> u64 {
        let channel = self.channel(sender, true);
        self.row(channel).senders += 1;
        self.endpoint(channel, true)
    }

    fn endpoint(&mut self, channel: u32, sender: bool) -> u64 {
        // Below `tasks::TASK_SET`, which marks a task set's endpoint word.
        self.generation = (self.generation + 1) & 0x3fff_ffff;
        let row = Endpoint {
            generation: self.generation,
            channel,
            sender,
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
        ENDPOINT | u64::from(self.generation) << 32 | u64::from(index)
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
        self.channels[channel as usize]
            .as_ref()
            .is_some_and(|row| !row.queue.is_empty() || row.senders == 0)
    }

    fn lookup(&self, word: u64) -> Option<(u32, Endpoint)> {
        if word & ENDPOINT == 0 {
            return None;
        }
        let index = word as u32;
        let endpoint = (*self.endpoints.get(index as usize)?)?;
        (endpoint.generation == ((word & !ENDPOINT) >> 32) as u32).then_some((index, endpoint))
    }

    fn row(&mut self, channel: u32) -> &mut Channel {
        self.channels[channel as usize]
            .as_mut()
            .unwrap_or_else(|| fault("a vacant channel row"))
    }

    /// Queue the `Some` image in `slot` and leave `None` there, unless the channel has no
    /// receiver left (`CLOSED`).
    ///
    /// # Safety
    /// `slot` is a `Some` image of the channel's element in another frame.
    pub(crate) unsafe fn send(&mut self, channel: u32, slot: *mut u8) -> u32 {
        let row = self.row(channel);
        if row.receivers == 0 {
            return CLOSED;
        }
        let element = &row.element;
        row.queue.push(slot, element.size);
        // SAFETY: caller contract; the image moved, so the slot now holds `None`.
        unsafe { std::ptr::copy_nonoverlapping(element.none.as_ptr(), slot, element.size) };
        self.touch(channel);
        DONE
    }

    /// A parked receiver may be served: the next scheduling point wakes it.
    fn touch(&mut self, channel: u32) {
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
        self.row(channel).waiting.push_back((at, task));
    }
    /// The receiver parked at `at` is no longer waiting.
    pub(crate) fn unwait(&mut self, channel: u32, at: u64) {
        if let Some(Some(row)) = self.channels.get_mut(channel as usize)
            && let Some(i) = row.waiting.iter().position(|&(place, _)| place == at)
        {
            row.waiting.remove(i);
        }
    }
    /// A live channel's queued message count, whether no sender is left, and its parked
    /// receivers in park order.
    pub(crate) fn waiting(&self, channel: u32) -> Option<(usize, bool, &VecDeque<(u64, u32)>)> {
        let row = self.channels.get(channel as usize)?.as_ref()?;
        Some((row.queue.len(), row.senders == 0, &row.waiting))
    }

    /// Move the oldest queued message into the `None` slot `slot`: `Ok(true)`, `Err(CLOSED)`
    /// once the queue is empty and no sender is left, or `Ok(false)` while a sender may still
    /// send.
    ///
    /// # Safety
    /// `slot` is a `None` image of the channel's element in another frame.
    pub(crate) unsafe fn receive(&mut self, channel: u32, slot: *mut u8) -> Result<bool, u32> {
        let row = self.row(channel);
        if row.queue.pop(slot, row.element.size) {
            Ok(true)
        } else if row.senders == 0 {
            Err(CLOSED)
        } else {
            Ok(false)
        }
    }

    /// Close the endpoint `word`. The messages a channel's last receiver leaves are returned
    /// with their descriptor index, for the caller to drop oldest first; none when the element
    /// needs no drop.
    pub(crate) fn close(&mut self, word: u64) -> Option<(Ring, u32)> {
        let Some((index, endpoint)) = self.lookup(word) else {
            fault("a channel endpoint closed twice or of another run");
        };
        self.endpoints[index as usize] = None;
        self.vacant_endpoints.push(index);
        let row = self.row(endpoint.channel);
        let mut left = None;
        if endpoint.sender {
            row.senders -= 1;
        } else {
            row.receivers -= 1;
            if row.receivers == 0 && row.queue.len() > 0 {
                let empty = Ring::new(row.element.size, row.queue.align);
                let queue = std::mem::replace(&mut row.queue, empty);
                left = (!row.element.inert).then_some((queue, row.element.descriptor));
            }
        }
        let (senders, receivers) = (row.senders, row.receivers);
        if senders == 0 && receivers == 0 {
            self.channels[endpoint.channel as usize] = None;
            self.vacant_channels.push(endpoint.channel);
        } else if senders == 0 && endpoint.sender {
            // The parked receivers now see Closed.
            self.touch(endpoint.channel);
        }
        left
    }
}

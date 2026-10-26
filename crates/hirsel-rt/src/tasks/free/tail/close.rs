//! Nested built-in close obligations under a retained release cursor.
//! Every stack entry owns its cursor or close phase; no borrowed native Frame,
//! inline Cleanup, wake list or logical shield survives only on the Rust stack.
use super::*;
use crate::TaskCtx;
use crate::alloc::Header;
use crate::finalize::Cleanup;
use crate::release::{Cursor, ReleasePoll};
use crate::tasks::free::{channel, mutex, record, replay};
use crate::tasks::role;
use std::ptr;

pub(super) struct Driver {
    // Boxes keep inline Cleanup addresses fixed across nested vector growth.
    #[allow(clippy::vec_box)]
    stack: Vec<Box<Node>>,
    task: TaskKey,
    anchor: u64,
    operation_site: u64,
    rest: bool,
}

enum Node {
    Release {
        cursor: Box<Cursor>,
        // The cursor's tagged work pointer may name this pinned allocation.
        _inline: Option<Box<std::mem::MaybeUninit<Cleanup>>>,
        _image: Option<Ring>,
    },
    Close(Close),
}

struct Close {
    word: u64,
    channel: Option<(u32, bool)>,
    ticket: Ticket,
    image: Option<usize>,
    descriptor: u32,
    count: usize,
    next: usize,
    status: u8,
    shield_base: u32,
    done: bool,
}

impl Driver {
    /// The caller has proved custody of the cursor and its complete ancestry.
    /// This internal constructor grants no permission to leave a native stack.
    pub(super) fn new(
        task: TaskKey,
        cursor: Box<Cursor>,
        anchor: u64,
        operation_site: u64,
    ) -> Self {
        Self {
            stack: vec![Box::new(Node::Release {
                cursor,
                _inline: None,
                _image: None,
            })],
            task,
            anchor,
            operation_site,
            rest: false,
        }
    }

    /// Poll only while this driver's owning continuation is Active. The caller
    /// must retain this Driver in task storage before invoking it: a cut can
    /// abandon the current Rust stack. Pending retains every ancestor and shield.
    pub(super) unsafe fn poll(&mut self, ctx: *mut TaskCtx) -> Stopped<Poll<u8>> {
        let rest = std::cell::Cell::new(false);
        let result = unsafe {
            self.poll_with(ctx, |object, entered| {
                replay::poll_object_entered(ctx, object, entered, &rest)
            })
        };
        self.rest |= rest.get();
        result
    }

    unsafe fn poll_with(
        &mut self,
        ctx: *mut TaskCtx,
        mut ready: impl FnMut(hirsel_free::session::Handle, &mut bool) -> Stopped<bool>,
    ) -> Stopped<Poll<u8>> {
        self.rest = false;
        assert_eq!(
            self.task,
            crate::tasks::running_key(ctx),
            "stale cleanup driver task"
        );
        loop {
            let node = self
                .stack
                .last_mut()
                .expect("completed cleanup driver polled") as *mut Box<Node>;
            match unsafe { &mut **node } {
                Node::Release { cursor, .. } => match unsafe { cursor.poll(&mut *ctx) } {
                    ReleasePoll::Close(word) => {
                        let builtin =
                            word & crate::sync::MUTEX != 0 || word & crate::channels::ENDPOINT != 0;
                        if !builtin || word & crate::tasks::TASK_SET != 0 {
                            // Opaque adapters remain synchronous and cannot compact
                            // their unproved native caller or leak status Pending.
                            let status = unsafe {
                                crate::finalize::run_close(
                                    &mut *ctx,
                                    word,
                                    self.anchor,
                                    self.operation_site,
                                )
                            };
                            cursor.closed(status);
                            continue;
                        }
                        let channel = if word & crate::channels::ENDPOINT != 0 {
                            Some(role::with_role(ctx, |s| s.channels.endpoint_of(word)))
                        } else {
                            None
                        };
                        let object = channel.map_or((crate::free::MUTEX, word), |(id, _)| {
                            (crate::free::CHANNEL, u64::from(id))
                        });
                        let kind = if channel.is_some() {
                            Kind::CloseChannel
                        } else {
                            Kind::CloseMutex
                        };
                        // Reserve the control record before taking the shield or
                        // beginning the operation. It is attached before polling.
                        let ticket = unsafe {
                            (*ctx).free_ops.tails.prepare(
                                self.task,
                                Site {
                                    object,
                                    kind,
                                    anchor: self.anchor,
                                    operation: self.operation_site,
                                },
                                Owners::default(),
                            )
                        };
                        let shield_base = unsafe { (*ctx).shield };
                        let close = Box::new(Node::Close(Close {
                            word,
                            channel,
                            ticket,
                            image: None,
                            descriptor: 0,
                            count: 0,
                            next: 0,
                            status: STATUS_OK,
                            shield_base,
                            done: false,
                        }));
                        self.stack.push(close);
                        unsafe {
                            (*ctx).shield =
                                shield_base.checked_add(1).expect("close shield overflow");
                            crate::tasks::shield_changed(ctx);
                        }
                    }
                    ReleasePoll::Complete(result) => {
                        let status = result.status;
                        self.stack.pop();
                        match self.stack.last_mut().map(|n| &mut **n) {
                            Some(Node::Close(parent)) => parent.status |= status,
                            None => return Ok(Poll::Ready(status)),
                            Some(Node::Release { .. }) => {
                                unreachable!("release child without close")
                            }
                        }
                    }
                },
                Node::Close(close) => {
                    if !close.done {
                        let effect = unsafe {
                            (*ctx).free_ops.tails.active(self.task, close.ticket) as *mut Record
                        };
                        let object = unsafe { (*effect).site.object };
                        let result = unsafe { &mut *effect }.poll_entered(
                            || {
                                if !record::begin_entered(ctx) {
                                    return Err(crate::stop::Stop::Requested);
                                }
                                Ok(record::on(ctx).then(|| record::position(ctx).1.op))
                            },
                            |entered| ready(object, entered),
                            |_| {
                                let (left, inert) = match close.channel {
                                    Some((channel, sender)) => {
                                        let Some(left) = (unsafe {
                                            channel::close_action_entered(
                                                ctx, close.word, channel, sender, true,
                                            )
                                        })?
                                        else {
                                            return Ok(None);
                                        };
                                        left
                                    }
                                    None => {
                                        let Some(closed) = (unsafe {
                                            mutex::close_action_entered(ctx, close.word, true)
                                        })?
                                        else {
                                            return Ok(None);
                                        };
                                        (closed.into_left(), false)
                                    }
                                };
                                if let Some((image, descriptor)) = left {
                                    if !inert {
                                        close.count = image.len();
                                        close.descriptor = descriptor;
                                        // Transfer the detached buffer before any cut,
                                        // stop-aware accessor, finalizer or nested close.
                                        close.image =
                                            Some(unsafe { (*ctx).detached_images.keep(image) });
                                    }
                                }
                                Ok(Some(STATUS_OK))
                            },
                        )?;
                        if result.is_pending() {
                            self.rest = unsafe { (*effect).rest };
                            return Ok(Poll::Pending);
                        }
                        unsafe { (*effect).releasing() };
                        close.done = true;
                        #[cfg(test)]
                        cut_tests::flush_done();
                        if replay::at_cut(ctx) {
                            return Err(crate::stop::Stop::Requested);
                        }
                    }
                    if close.next < close.count {
                        let image = close.image.expect("detached close image");
                        let value = unsafe { (*ctx).detached_images.at(image, close.next) };
                        let mut inline = Box::new(std::mem::MaybeUninit::<Cleanup>::uninit());
                        let base = unsafe { (*ctx).work };
                        let item = unsafe {
                            crate::finalize::finalize_item(
                                inline.as_mut_ptr(),
                                close.descriptor,
                                value,
                            )
                        };
                        let cursor = Box::new(Cursor::new(
                            base,
                            item,
                            ptr::null(),
                            false,
                            self.anchor,
                            self.operation_site,
                        ));
                        // Commit the FIFO position only when its child has a sole
                        // pinned owner; the parent cannot resume before it returns.
                        close.next += 1;
                        self.stack.push(Box::new(Node::Release {
                            cursor,
                            _inline: Some(inline),
                            _image: None,
                        }));
                        unsafe { crate::release::push(&mut *ctx, item, true) };
                        continue;
                    }
                    if let Some(image) = close.image.take() {
                        unsafe { (*ctx).detached_images.release(image) };
                    }
                    assert_eq!(
                        unsafe { (*ctx).shield },
                        close.shield_base + 1,
                        "nested close leaked a shield"
                    );
                    unsafe {
                        (*ctx).shield = close.shield_base;
                        crate::tasks::shield_changed(ctx);
                        let effect = (*ctx).free_ops.tails.active(self.task, close.ticket);
                        effect.released(close.status);
                        effect.finish();
                        (*ctx).free_ops.tails.retire(self.task, close.ticket);
                    }
                    let status = close.status;
                    self.stack.pop();
                    let Some(Node::Release { cursor, .. }) =
                        self.stack.last_mut().map(|n| &mut **n)
                    else {
                        unreachable!("close without release parent")
                    };
                    cursor.closed(status);
                    // The child's ordinal remains current. Never restore the
                    // parent's ordinal or publish its Done phase again.
                    if replay::at_cut(ctx) {
                        return Err(crate::stop::Stop::Requested);
                    }
                }
            }
        }
    }
}

/// Attach before consuming the last root credit or entering any user cleanup.
/// The cursor and all masks/value locations are already in retained storage.
pub(in crate::tasks) unsafe fn attach(
    ctx: *mut TaskCtx,
    cursor: Box<Cursor>,
    anchor: u64,
    operation_site: u64,
) -> usize {
    let task = crate::tasks::running_key(ctx);
    let driver = Box::new(Driver::new(task, cursor, anchor, operation_site));
    let drivers = unsafe { &mut (*ctx).free_ops.tails.drivers };
    let index = drivers.len();
    drivers.push(driver);
    index
}

/// A task-owned driver is not removed from its box while it executes. Nested
/// opaque finalizers can grow the handle vector without moving any ancestor.
pub(in crate::tasks) unsafe fn poll(ctx: *mut TaskCtx, index: usize) -> Stopped<Poll<u8>> {
    let driver = unsafe { &mut *(&mut (*ctx).free_ops.tails.drivers)[index] as *mut Driver };
    let result = unsafe { (*driver).poll(ctx) }?;
    if result.is_ready() {
        assert_eq!(unsafe { (*ctx).free_ops.tails.drivers.len() }, index + 1);
        unsafe { (*ctx).free_ops.tails.drivers.pop() };
    }
    Ok(result)
}

#[cfg(test)]
mod tests;

/// Root credit consumption belongs to the first entry, never to its retry.
pub(crate) unsafe fn invoke_root_drop(ctx: *mut TaskCtx, block: *mut Header) -> Option<u8> {
    unsafe {
        invoke_cleanup(ctx, 48, |ctx, site| {
            let cursor = Box::new(Cursor::new(
                (*ctx).work,
                block,
                ptr::null(),
                false,
                site.anchor,
                site.operation_site,
            ));
            let index = attach(ctx, cursor, site.anchor, site.operation_site);
            if !crate::roots::consume(&mut *ctx, block) {
                (*ctx).free_ops.tails.drivers.pop();
                return None;
            }
            crate::release::push(&mut *ctx, block, true);
            Some(index)
        })
    }
}

/// Ordinary counted holders use the same last-credit rules as release_block.
pub(crate) unsafe fn invoke_release(ctx: *mut TaskCtx, block: *mut Header) -> Option<u8> {
    unsafe {
        invoke_cleanup(ctx, 49, |ctx, site| {
            let count = (*block).count;
            if count >= crate::count::COUNT_ARENA_SHARED {
                return None;
            }
            if count > 1 {
                (*block).count = count - 1;
                return None;
            }
            let cursor = Box::new(Cursor::new(
                (*ctx).work,
                block,
                ptr::null(),
                false,
                site.anchor,
                site.operation_site,
            ));
            let index = attach(ctx, cursor, site.anchor, site.operation_site);
            crate::release::push(&mut *ctx, block, true);
            Some(index)
        })
    }
}

/// A native inline temporary is moved into task-owned bytes before any push.
/// Its Cleanup is pinned before its tagged address reaches the work chain.
pub(crate) unsafe fn invoke_finalize(
    ctx: *mut TaskCtx,
    descriptor: u32,
    value: *mut u8,
) -> Option<u8> {
    unsafe {
        invoke_cleanup(ctx, 50, |ctx, site| {
            let layout = &*(*ctx).descriptor(descriptor);
            assert!(layout.flags & crate::release::LAYOUT_DYNAMIC == 0);
            let mut image = Ring::new(layout.size as usize, layout.align as usize);
            image.push(value, layout.size as usize);
            let mut inline = Box::new(std::mem::MaybeUninit::<Cleanup>::uninit());
            let item = crate::finalize::finalize_item(inline.as_mut_ptr(), descriptor, image.at(0));
            let cursor = Box::new(Cursor::new(
                (*ctx).work,
                item,
                ptr::null(),
                false,
                site.anchor,
                site.operation_site,
            ));
            let index = attach(ctx, cursor, site.anchor, site.operation_site);
            let driver = &mut *(&mut (*ctx).free_ops.tails.drivers)[index];
            let Node::Release {
                _inline, _image, ..
            } = &mut *driver.stack[0]
            else {
                unreachable!()
            };
            *_inline = Some(inline);
            *_image = Some(image);
            crate::release::push(&mut *ctx, item, true);
            Some(index)
        })
    }
}

/// Dispatch by entered invocation before reading the original operand. After
/// transfer, only the task-owned driver owns the in-progress obligation.
unsafe fn invoke_cleanup(
    ctx: *mut TaskCtx,
    kind: u32,
    prepare: impl FnOnce(*mut TaskCtx, crate::continuation::binding::BoundSite) -> Option<usize>,
) -> Option<u8> {
    let site = unsafe { crate::continuation::binding::take_bound(ctx, kind) }?;
    if !crate::tasks::free::enabled(ctx) {
        unsafe { crate::continuation::binding::complete_bound(ctx, site) };
        return None;
    }
    let previous = unsafe {
        (*ctx)
            .free_ops
            .tails
            .cleanup_sites
            .last()
            .filter(|(s, _)| *s == site)
            .map(|(_, index)| *index)
    };
    let index = match previous {
        Some(index) => index,
        None => match prepare(ctx, site) {
            Some(index) => {
                unsafe { (*ctx).free_ops.tails.cleanup_sites.push((site, index)) };
                index
            }
            None => {
                unsafe { crate::continuation::binding::complete_bound(ctx, site) };
                return Some(STATUS_OK);
            }
        },
    };
    crate::tasks::compact::waited(ctx);
    loop {
        match unsafe { poll(ctx, index) } {
            Ok(Poll::Ready(status)) => {
                unsafe {
                    assert_eq!(
                        (*ctx).free_ops.tails.cleanup_sites.pop(),
                        Some((site, index))
                    );
                    crate::continuation::binding::complete_bound(ctx, site);
                }
                return Some(status);
            }
            waiting => {
                let leave = unsafe { crate::continuation::binding::can_leave(ctx, site) };
                if let Err(stop) = waiting {
                    if !(leave && replay::cut_rejection(ctx, &stop)) {
                        unsafe { crate::stop::check(ctx, Err::<(), _>(stop)) };
                        unreachable!();
                    }
                    unsafe {
                        crate::stop::check(
                            ctx,
                            crate::tasks::park_left(ctx, crate::tasks::Wait::Final),
                        )
                    };
                } else if unsafe { (&(*ctx).free_ops.tails.drivers)[index].rest } {
                    if leave {
                        return Some(crate::STATUS_SUSPENDED);
                    }
                    unsafe { crate::tasks::free::suspend(&mut *ctx, true) };
                    continue;
                } else if leave {
                    unsafe {
                        crate::stop::check(
                            ctx,
                            crate::tasks::park_left(
                                ctx,
                                crate::tasks::Wait::Retry { cancelable: false },
                            ),
                        )
                    };
                } else {
                    unsafe {
                        crate::stop::check(
                            ctx,
                            crate::tasks::try_block(
                                &mut *ctx,
                                crate::tasks::Wait::Retry { cancelable: false },
                                false,
                                site.operation_site,
                            ),
                        )
                    };
                    continue;
                }
                crate::tasks::compact::wait(ctx);
                return Some(crate::STATUS_SUSPENDED);
            }
        }
    }
}

#[cfg(test)]
mod channel_tests;

#[cfg(test)]
mod cut_tests;

/// Whether a Pending publication was rejected before dependency subscription.
/// Requeue this owner; a subscribed object wait instead parks for its wake.
/// # Safety
/// The Active task owns a live driver index returned by attach, after Pending.
pub(in crate::tasks) unsafe fn requeue(ctx: *mut TaskCtx, index: usize) -> bool {
    unsafe { (&(*ctx).free_ops.tails.drivers)[index].rest }
}

//! Nested built-in close obligations under a retained release cursor.
//! Every stack entry owns its cursor or close phase; no borrowed native Frame,
//! inline Cleanup, wake list or logical shield survives only on the Rust stack.
use super::*;
use crate::alloc::Header;
use crate::finalize::Cleanup;
use crate::release::{Cursor, ReleasePoll};
use crate::tasks::free::{channel, mutex, record, replay};
use crate::tasks::role;
use crate::TaskCtx;
use std::ptr;

pub(super) struct Driver {
    // Boxes keep inline Cleanup addresses fixed across nested vector growth.
    #[allow(clippy::vec_box)]
    stack: Vec<Box<Node>>,
    task: TaskKey,
    anchor: u64,
    operation_site: u64,
}

enum Node {
    Release {
        cursor: Box<Cursor>,
        // The cursor's tagged work pointer may name this pinned allocation.
        _inline: Option<Box<std::mem::MaybeUninit<Cleanup>>>,
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
    pub(super) fn new(task: TaskKey, cursor: Box<Cursor>, anchor: u64, operation_site: u64) -> Self {
        Self {
            stack: vec![Box::new(Node::Release { cursor, _inline: None })],
            task, anchor, operation_site,
        }
    }

    /// Poll only while this driver's owning continuation is Active. The caller
    /// must retain this Driver in task storage before invoking it: a cut can
    /// abandon the current Rust stack. Pending retains every ancestor and shield.
    pub(super) unsafe fn poll(&mut self, ctx: *mut TaskCtx) -> Stopped<Poll<u8>> {
        unsafe { self.poll_with(ctx, |object, entered| replay::poll_object(ctx, object, entered)) }
    }

    unsafe fn poll_with(&mut self, ctx: *mut TaskCtx,
        mut ready: impl FnMut(hirsel_free::session::Handle, &mut bool) -> Stopped<bool>) -> Stopped<Poll<u8>> {
        assert_eq!(self.task, crate::tasks::running_key(ctx), "stale cleanup driver task");
        loop {
            let node = self.stack.last_mut().expect("completed cleanup driver polled") as *mut Box<Node>;
            match unsafe { &mut **node } {
                Node::Release { cursor, .. } => match unsafe { cursor.poll(&mut *ctx) } {
                    ReleasePoll::Close(word) => {
                        let builtin = word & crate::sync::MUTEX != 0
                            || word & crate::channels::ENDPOINT != 0;
                        if !builtin || word & crate::tasks::TASK_SET != 0 {
                            // Opaque adapters remain synchronous and cannot compact
                            // their unproved native caller or leak status Pending.
                            let status = unsafe { crate::finalize::run_close(&mut *ctx, word, self.anchor, self.operation_site) };
                            cursor.closed(status);
                            continue;
                        }
                        let channel = if word & crate::channels::ENDPOINT != 0 {
                            Some(role::with_role(ctx, |s| s.channels.endpoint_of(word)))
                        } else { None };
                        let object = channel.map_or((crate::free::MUTEX, word), |(id, _)| (crate::free::CHANNEL, u64::from(id)));
                        let kind = if channel.is_some() { Kind::CloseChannel } else { Kind::CloseMutex };
                        // Reserve the control record before taking the shield or
                        // beginning the operation. It is attached before polling.
                        let ticket = unsafe { (*ctx).free_ops.tails.prepare(self.task, Site { object, kind, anchor:self.anchor, operation:self.operation_site }, Owners::default()) };
                        let shield_base = unsafe { (*ctx).shield };
                        let close = Box::new(Node::Close(Close { word, channel, ticket,
                            image:None, descriptor:0, count:0, next:0, status:STATUS_OK,
                            shield_base, done:false }));
                        self.stack.push(close);
                        unsafe {
                            (*ctx).shield = shield_base.checked_add(1).expect("close shield overflow");
                            crate::tasks::shield_changed(ctx);
                        }
                    }
                    ReleasePoll::Complete(result) => {
                        let status = result.status;
                        self.stack.pop();
                        match self.stack.last_mut().map(|n| &mut **n) {
                            Some(Node::Close(parent)) => parent.status |= status,
                            None => return Ok(Poll::Ready(status)),
                            Some(Node::Release { .. }) => unreachable!("release child without close"),
                        }
                    }
                },
                Node::Close(close) => {
                    if !close.done {
                        let effect = unsafe { (*ctx).free_ops.tails.active(self.task, close.ticket) as *mut Record };
                        let object = unsafe { (*effect).site.object };
                        let result = unsafe { &mut *effect }.poll(
                            || { record::begin(ctx); record::on(ctx).then(|| record::position(ctx).1.op) },
                            |entered| ready(object, entered),
                            |_| {
                                let (left, inert) = match close.channel {
                                    Some((channel, sender)) => unsafe { channel::close_action(ctx, close.word, channel, sender) }?,
                                    None => (unsafe { mutex::close_action(ctx, close.word) }?.into_left(), false),
                                };
                                if let Some((image, descriptor)) = left {
                                    if !inert {
                                        close.count = image.len();
                                        close.descriptor = descriptor;
                                        // Transfer the detached buffer before any cut,
                                        // stop-aware accessor, finalizer or nested close.
                                        close.image = Some(unsafe { (*ctx).detached_images.keep(image) });
                                    }
                                }
                                Ok(STATUS_OK)
                            },
                        )?;
                        if result.is_pending() { return Ok(Poll::Pending); }
                        unsafe { (*effect).releasing() };
                        close.done = true;
                        if replay::at_cut(ctx) { return Err(crate::stop::Stop::Requested); }
                    }
                    if close.next < close.count {
                        let image = close.image.expect("detached close image");
                        let value = unsafe { (*ctx).detached_images.at(image, close.next) };
                        let mut inline = Box::new(std::mem::MaybeUninit::<Cleanup>::uninit());
                        let base = unsafe { (*ctx).work };
                        let item = unsafe { crate::finalize::finalize_item(inline.as_mut_ptr(), close.descriptor, value) };
                        let cursor = Box::new(Cursor::new(base, item, ptr::null(), false, self.anchor, self.operation_site));
                        // Commit the FIFO position only when its child has a sole
                        // pinned owner; the parent cannot resume before it returns.
                        close.next += 1;
                        self.stack.push(Box::new(Node::Release { cursor, _inline:Some(inline) }));
                        unsafe { crate::release::push(&mut *ctx, item, true) };
                        continue;
                    }
                    if let Some(image) = close.image.take() {
                        unsafe { (*ctx).detached_images.release(image) };
                    }
                    assert_eq!(unsafe { (*ctx).shield }, close.shield_base + 1, "nested close leaked a shield");
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
                    let Some(Node::Release { cursor, .. }) = self.stack.last_mut().map(|n| &mut **n) else { unreachable!("close without release parent") };
                    cursor.closed(status);
                    // The child's ordinal remains current. Never restore the
                    // parent's ordinal or publish its Done phase again.
                    if replay::at_cut(ctx) { return Err(crate::stop::Stop::Requested); }
                }
            }
        }
    }
}

/// Attach before consuming the last root credit or entering any user cleanup.
/// The cursor and all masks/value locations are already in retained storage.
pub(super) unsafe fn attach(ctx: *mut TaskCtx, cursor: Box<Cursor>, anchor:u64, operation_site:u64) -> usize {
    let task = crate::tasks::running_key(ctx);
    let driver = Box::new(Driver::new(task, cursor, anchor, operation_site));
    let drivers = unsafe { &mut (*ctx).free_ops.tails.drivers };
    let index = drivers.len();
    drivers.push(driver);
    index
}

/// A task-owned driver is not removed from its box while it executes. Nested
/// opaque finalizers can grow the handle vector without moving any ancestor.
pub(super) unsafe fn poll(ctx: *mut TaskCtx, index: usize) -> Stopped<Poll<u8>> {
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

/// Entered instruction site for a root drop. The saved operation, rather
/// than the reloaded source operand, owns the consumed root on every retry.
/// # Safety
/// All caller frames are certified, including value/mask/cleanup ancestry.
pub(crate) unsafe fn invoke_root_drop(ctx:*mut TaskCtx, block:*mut Header) -> Option<u8> {
    const ROOT_DROP:u32=48;
    let site=unsafe {crate::continuation::binding::take_bound(ctx,ROOT_DROP)}?;
    if !crate::tasks::free::enabled(ctx) {
        unsafe {crate::continuation::binding::complete_bound(ctx,site)};
        return None;
    }
    let previous=unsafe {(*ctx).free_ops.tails.cleanup_sites.last().filter(|(s,_)|*s==site).map(|(_,index)|*index)};
    let index=match previous {
        Some(index)=>index,
        None=> {
            // Both record and backing capacity exist before consuming credit.
            let cursor=Box::new(Cursor::new(unsafe {(*ctx).work},block,ptr::null(),false,site.anchor,site.operation_site));
            let index=unsafe {attach(ctx,cursor,site.anchor,site.operation_site)};
            unsafe {(*ctx).free_ops.tails.cleanup_sites.push((site,index))};
            if !unsafe {crate::roots::consume(&mut *ctx,block)} {
                unsafe {
                    (*ctx).free_ops.tails.drivers.pop();
                    (*ctx).free_ops.tails.cleanup_sites.pop();
                    crate::continuation::binding::complete_bound(ctx,site);
                }
                return Some(STATUS_OK);
            }
            unsafe {crate::release::push(&mut *ctx,block,true)};
            index
        }
    };
    crate::tasks::compact::waited(ctx);
    loop {
        let result=unsafe {poll(ctx,index)};
        match result {
            Ok(Poll::Ready(status))=> {
                unsafe {
                    assert_eq!((*ctx).free_ops.tails.cleanup_sites.pop(),Some((site,index)));
                    crate::continuation::binding::complete_bound(ctx,site);
                }
                return Some(status);
            }
            waiting=> {
                let leave=unsafe {crate::continuation::binding::can_leave(ctx,site)};
                if let Err(stop)=waiting {
                    if !(leave && replay::cut_rejection(ctx,&stop)) {
                        unsafe {crate::stop::check(ctx,Err::<(),_>(stop))};
                        unreachable!();
                    }
                    unsafe {crate::stop::check(ctx,crate::tasks::park_left(ctx,crate::tasks::Wait::Final))};
                } else if leave {
                    unsafe {crate::stop::check(ctx,crate::tasks::park_left(ctx,crate::tasks::Wait::Retry{cancelable:false}))};
                } else {
                    unsafe {crate::stop::check(ctx,crate::tasks::try_block(&mut *ctx,crate::tasks::Wait::Retry{cancelable:false},false,site.operation_site))};
                    continue;
                }
                crate::tasks::compact::wait(ctx);
                return Some(crate::STATUS_SUSPENDED);
            }
        }
    }
}

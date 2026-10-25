//! Free channel endpoint effects. Payload locks are acquired before a phase
//! opens and retained through its progress publication. The scheduler role
//! protects identity tables and wake metadata; sends release it while acting.
//! Tokio mpsc/list.rs:74-85,100-108 at ce685ee6916549ed assigns data and close
//! positions with fetch_add. Hirsel uses the same endpoint sequence for its
//! recorded versions, with the image ring protected by the object lock.

use super::super::*;

#[cfg(test)]
mod close_tests;
#[cfg(test)]
mod endpoint_tests;
#[cfg(test)]
mod receive_tests;
#[cfg(test)]
mod tests;

/// # Safety
/// The role is held; out and none satisfy ChannelOpen's ABI contract.
pub(crate) unsafe fn open(
    ctx: *mut TaskCtx,
    out: *mut u64,
    none: *const u8,
    descriptor: u32,
) -> Stopped<()> {
    role::admit(ctx);
    let layout = unsafe { (*ctx).descriptor(descriptor) };
    let element = unsafe { crate::channels::Element::new(descriptor, layout, none) };
    let cell = crate::channels::Channels::prepare_open(element, unsafe { &*layout });
    let reservation = unsafe { role::with_held(ctx, |s| s.channels.reserve_open()) };
    let channel = reservation.channel();
    let (sender, receiver) = reservation.words();
    super::record::begin(ctx);
    let object = super::record::object(ctx);
    let replay = super::replay::object(ctx, (crate::free::CHANNEL, u64::from(channel)));
    if let Some(object) = &object {
        cell.object.set(object.clone()).unwrap();
    }
    let publication = super::record::publish(ctx, Phase::Done, |writer| unsafe {
        if let Some(replay) = &replay {
            replay.create()?;
        }
        role::with_held(ctx, |s| s.channels.initialize_open_cell(reservation, cell));
        out.write(sender);
        out.add(1).write(receiver);
        if let Some(writer) = writer {
            writer.create(object.unwrap());
        }
        Ok(())
    });
    drop(replay);
    publication??;
    unsafe { try_yield(&mut *ctx) }
}

/// # Safety
/// The role is held; sender is open and out writable.
pub(crate) unsafe fn dup(ctx: *mut TaskCtx, sender: u64, out: *mut u64) -> Stopped<()> {
    super::record::begin(ctx);
    let channel = role::with_role(ctx, |s| s.channels.channel(sender, true));
    unsafe { super::replay::wait_object(ctx, (crate::free::CHANNEL, u64::from(channel))) }?;
    let replay = super::replay::object(ctx, (crate::free::CHANNEL, u64::from(channel)));
    let (cell, reservation) = unsafe {
        role::with_held(ctx, |s| {
            (s.channels.payload(channel), s.channels.reserve_dup())
        })
    };
    let word = reservation.word();
    let mut state = cell.lock();
    super::record::track(ctx, cell.object.get());
    let publication = super::record::publish_with_replay(
        ctx,
        Phase::Done,
        replay.as_ref().map(|object| object.token()),
        |writer| {
            cell.dup(&mut state);
            let version = cell.next_event();
            unsafe {
                role::with_held(ctx, |s| {
                    s.channels.initialize_dup_endpoint(reservation, channel)
                });
                out.write(word);
            }
            if let Some(writer) = writer {
                writer.touch(
                    cell.object.get().expect("a recorded channel identity"),
                    version,
                    0,
                    "dup",
                );
            } else if let Some(replay) = &replay {
                replay.touch("dup")?;
            }
            Ok(())
        },
    );
    drop(state);
    drop(cell);
    drop(replay);
    publication??;
    unsafe { try_yield(&mut *ctx) }
}

/// # Safety
/// The role is held; sender is open, message is its Some carrier, out writable.
pub(crate) unsafe fn send(
    ctx: *mut TaskCtx,
    sender: u64,
    message: *mut u8,
    out: *mut u32,
) -> Stopped<()> {
    super::record::begin(ctx);
    let channel = role::with_role(ctx, |s| s.channels.channel(sender, true));
    unsafe { super::replay::wait_object(ctx, (crate::free::CHANNEL, u64::from(channel))) }?;
    let replay = super::replay::object(ctx, (crate::free::CHANNEL, u64::from(channel)));
    let cell = unsafe { role::with_held(ctx, |s| s.channels.payload(channel)) };
    super::objects::keep_endpoint(ctx, sender, channel, true, &cell);
    unsafe { role::release(ctx) };
    let mut state = cell.lock();
    cell.prepare_send(&mut state);
    // Send deliberately does not observe cancellation (CONC-3).
    let publication = sent(ctx, &cell, &mut state, message, out, replay.as_ref());
    drop(state);
    drop(cell);
    drop(replay);
    unsafe { role::enter(ctx) };
    if publication? == crate::channels::DONE {
        role::with_role(ctx, |s| s.channels.touch(channel));
    }
    unsafe { try_yield(&mut *ctx) }
}

/// Send's bounded action and its evidence, under the held cell lock.
pub(super) fn sent(
    ctx: *mut TaskCtx,
    cell: &crate::channels::ChannelCell,
    state: &mut crate::channels::ChannelState,
    message: *mut u8,
    out: *mut u32,
    replay: Option<&super::replay::Object>,
) -> Stopped<u32> {
    super::record::track(ctx, cell.object.get());
    super::record::publish_with_replay(
        ctx,
        Phase::Done,
        replay.map(|object| object.token()),
        |writer| {
            let status = unsafe { cell.send(state, message, false) }.0;
            let version = cell.next_event();
            let outcome = if status == crate::channels::DONE {
                "send"
            } else {
                "send closed"
            };
            if let Some(writer) = writer {
                writer.touch(
                    cell.object.get().expect("a recorded channel identity"),
                    version,
                    0,
                    outcome,
                );
            } else if let Some(replay) = replay {
                replay.touch(outcome)?;
            }
            unsafe { out.write(status) };
            Ok(status)
        },
    )?
}

/// # Safety
/// The role is held; receiver is open, message is its None carrier, out writable.
pub(crate) unsafe fn receive(
    ctx: *mut TaskCtx,
    receiver: u64,
    message: *mut u8,
    out: *mut u32,
    caller_site: u64,
) -> Stopped<()> {
    super::record::begin(ctx);
    let channel = role::with_role(ctx, |s| s.channels.channel(receiver, false));
    let mut entered = false;
    loop {
        // Cancellation flags and the shield snapshot remain stable while
        // this role is held. Senders drop the object lock before reentering
        // the role, so acquiring this lock cannot invert their lock order.
        let shielded = unsafe { (*ctx).shield } != 0;
        let replay = super::replay::object(ctx, (crate::free::CHANNEL, u64::from(channel)));
        if let Some(replay) = &replay {
            replay.token().admit_done(ctx)?;
        }
        let live = unsafe { canceled(ctx) };
        if live.is_none() {
            unsafe {
                role::with_held(ctx, |s| {
                    super::objects::keep_uncanceled(ctx, s, (*ctx).shield)
                })
            };
        }
        let cause = replay
            .as_ref()
            .filter(|_| !shielded)
            .map(|object| object.token().cancel_peek(live))
            .unwrap_or(live);
        let canceled_outcome = cause.map(|cause| format!("canceled {cause}"));
        let (cell, me, owner) = unsafe {
            role::with_held(ctx, |s| {
                let me = s.current;
                let row = s.task(me);
                let owner = if row.pinned { Owner::Any } else { row.owner };
                (s.channels.payload(channel), me, owner)
            })
        };
        super::objects::keep_endpoint(ctx, receiver, channel, false, &cell);
        // A parking receiver's wake needs no role: a send that makes a message
        // visible takes the oldest token.
        let mut token = { unsafe { crate::pool::free::token((*ctx).run_ptr(), me, owner) } };
        let mut state = cell.lock();
        state.drop_token(me);
        let mut result = None;
        let publication = if cause.is_some() || cell.ready(&state) {
            super::record::publish_with_replay(
                ctx,
                Phase::Done,
                replay.as_ref().map(|object| object.token()),
                |writer| {
                    let (status, outcome) = if let Some(cause) = cause {
                        (
                            crate::channels::CANCELED + cause,
                            canceled_outcome.as_deref().unwrap(),
                        )
                    } else {
                        match unsafe { cell.receive(&mut state, message) } {
                            Ok(true) => (crate::channels::DONE, "msg"),
                            Err(status) => (status, "closed"),
                            Ok(false) => {
                                unreachable!("the receiver owns this channel's ready message")
                            }
                        }
                    };
                    if let Some(writer) = writer {
                        if !shielded {
                            writer.cancel(cause);
                        }
                        writer.receive(
                            cell.object
                                .get()
                                .expect("a recorded channel identity")
                                .clone(),
                            outcome,
                        );
                    } else if let Some(replay) = &replay {
                        if !shielded {
                            assert_eq!(replay.token().cancel(cause)?, cause);
                        }
                        replay.receive(outcome)?;
                    }
                    unsafe { out.write(status) };
                    result = Some(status);
                    Ok(())
                },
            )
        } else if !entered {
            super::record::publish_with_replay(
                ctx,
                Phase::Entered,
                replay.as_ref().map(|object| object.token()),
                |_| Ok(()),
            )
        } else {
            Ok(Ok(()))
        };
        if result.is_none()
            && let Some(token) = token.take()
        {
            state.keep_token(me, token);
        }
        drop(state);
        drop(cell);
        drop(canceled_outcome);
        drop(replay);
        publication??;
        if result.is_some() {
            if cause.is_some() {
                // A send may have woken this receiver alone: a canceled one
                // takes no message, so the policy serves the next in park order.
                role::with_role(ctx, |s| s.channels.touch(channel));
            }
            return unsafe { try_yield(&mut *ctx) };
        }
        entered = true;
        // Replay waits as live does: its recorded cancellation is fixed for
        // the op, so only a visible message or a close can end the wait.
        let wait = Wait::ReceiveReady { channel };
        // No object lock, Arc, recorder borrow, or frame slot is retained by
        // the poll. A send racing this park is caught by its first idle poll.
        unsafe { try_block(&mut *ctx, wait, false, caller_site) }?;
    }
}

/// # Safety
/// As for close_endpoint: the role and one endpoint credit are owned here.
pub(crate) unsafe fn close(
    ctx: *mut TaskCtx,
    word: u64,
    anchor: u64,
    operation_site: u64,
) -> Stopped<u8> {
    let status = unsafe { close_inner(ctx, word, anchor, operation_site) }?;
    // Close keeps its worker in live execution, but its replay cut still
    // precedes the next user segment. No detached ring or cell owner may
    // remain on this frame when it parks permanently.
    super::replay::start(ctx);
    Ok(status)
}

/// # Safety
/// As for close; the caller observes the cut after all temporary owners drop.
unsafe fn close_inner(
    ctx: *mut TaskCtx,
    word: u64,
    anchor: u64,
    operation_site: u64,
) -> Stopped<u8> {
    super::record::begin(ctx);
    let (channel, sender) = role::with_role(ctx, |s| s.channels.endpoint_of(word));
    unsafe { super::replay::wait_object(ctx, (crate::free::CHANNEL, u64::from(channel))) }?;
    let replay = super::replay::object(ctx, (crate::free::CHANNEL, u64::from(channel)));
    let cell = unsafe {
        role::with_held(ctx, |s| {
            let cell = s.channels.payload(channel);
            s.channels.prepare_close();
            cell
        })
    };
    let inert = cell.element.inert;
    let mut state = cell.lock();
    let outcome = if sender {
        "close sender"
    } else {
        "close receiver"
    };
    super::record::track(ctx, cell.object.get());
    let publication = super::record::publish_with_replay(
        ctx,
        Phase::Done,
        replay.as_ref().map(|object| object.token()),
        |writer| {
            let left = unsafe {
                role::with_held(ctx, |s| s.channels.close_locked(word, &cell, &mut state))
            };
            let version = cell.next_event();
            if let Some(writer) = writer {
                writer.touch(
                    cell.object.get().expect("a recorded channel identity"),
                    version,
                    0,
                    outcome,
                );
            } else if let Some(replay) = &replay {
                replay.touch(outcome)?;
            }
            Ok(left)
        },
    );
    drop(state);
    drop(cell);
    drop(replay);
    let left = publication??.into_left();
    // Close is Keep: it still publishes runnable receivers before returning
    // to a segment that may never yield. No stop-aware ctx accessor can
    // abandon a detached ring on this frame.
    let ready = unsafe {
        role::with_held(ctx, |s| {
            super::super::readiness::poll_free(s);
            super::take_ready(s)
        })
    };
    unsafe { super::schedule_ready((*ctx).run_ptr(), ready) };
    let Some((left, descriptor)) = left else {
        return Ok(STATUS_OK);
    };
    if inert {
        // Even an inert buffer's deallocation stays outside the section.
        return Ok(STATUS_OK);
    }
    let count = left.len();
    let image = unsafe { (*ctx).detached_images.keep(left) };
    // The close phase can be the cut. Its unadmitted user finalizer must
    // not start, and the task owns the ring if this save never returns.
    super::replay::start(ctx);
    let mut status = STATUS_OK;
    for index in 0..count {
        let value = unsafe { (*ctx).detached_images.at(image, index) };
        let mut continuation = std::mem::MaybeUninit::<crate::finalize::Cleanup>::uninit();
        // Finalizers can yield and open or close arbitrary other channels.
        // Only the index and raw slot pointer cross the user call. Nested
        // closes may grow task storage without invalidating this buffer.
        status |= unsafe {
            crate::finalize::hirsel_rt_v2_finalize(
                ctx,
                descriptor,
                value,
                std::ptr::null(),
                continuation.as_mut_ptr(),
                anchor,
                operation_site,
            )
        };
    }
    unsafe { (*ctx).detached_images.release(image) };
    Ok(status)
}

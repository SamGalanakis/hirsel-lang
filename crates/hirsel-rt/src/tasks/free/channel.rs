//! Free channel endpoint effects. Payload locks are acquired before a phase
//! opens and retained through its progress publication. The scheduler role
//! protects identity tables and wake metadata; sends release it while acting.
//! Tokio mpsc/list.rs:74-85,100-108 at ce685ee6916549ed assigns data and close
//! positions with fetch_add. Hirsel uses the same endpoint sequence for its
//! recorded versions, with the image ring protected by the object lock.

use super::super::*;

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
    let s = scheduler(ctx) as *mut Scheduler;
    let layout = unsafe { (*ctx).descriptor(descriptor) };
    let element = unsafe { crate::channels::Element::new(descriptor, layout, none) };
    let cell = crate::channels::Channels::prepare_open(element, unsafe { &*layout });
    let reservation = unsafe { (*s).channels.reserve_open() };
    let (sender, receiver) = reservation.words();
    super::record::begin(ctx);
    let object = super::record::object(ctx);
    if let Some(object) = &object {
        cell.object.set(object.clone()).unwrap();
    }
    super::record::publish(ctx, Phase::Done, |writer| unsafe {
        (*s).channels.initialize_open_cell(reservation, cell);
        out.write(sender);
        out.add(1).write(receiver);
        if let Some(writer) = writer {
            writer.create(object.unwrap());
        }
    })?;
    unsafe { try_yield(&mut *ctx) }
}

/// # Safety
/// The role is held; sender is open and out writable.
pub(crate) unsafe fn dup(ctx: *mut TaskCtx, sender: u64, out: *mut u64) -> Stopped<()> {
    super::record::begin(ctx);
    let s = scheduler(ctx) as *mut Scheduler;
    let channel = unsafe { (*s).channels.channel(sender, true) };
    let cell = unsafe { (*s).channels.payload(channel) };
    let reservation = unsafe { (*s).channels.reserve_dup() };
    let word = reservation.word();
    let mut state = cell.lock();
    let publication = super::record::publish(ctx, Phase::Done, |writer| {
        cell.dup(&mut state);
        let version = cell.next_event();
        unsafe {
            (*s).channels.initialize_dup_endpoint(reservation, channel);
            out.write(word);
        }
        if let Some(writer) = writer {
            writer.touch(
                cell.object.get().expect("a recorded channel identity"),
                version,
                0,
                "dup",
            );
        }
    });
    drop(state);
    drop(cell);
    publication?;
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
    let channel = scheduler(ctx).channels.channel(sender, true);
    let cell = scheduler(ctx).channels.payload(channel);
    unsafe { role::release(ctx) };
    let mut state = cell.lock();
    cell.prepare_send(&mut state);
    // Send deliberately does not observe cancellation (CONC-3).
    let publication = super::record::publish(ctx, Phase::Done, |writer| {
        let status = unsafe { cell.send(&mut state, message, false) }.0;
        let version = cell.next_event();
        if let Some(writer) = writer {
            writer.touch(
                cell.object.get().expect("a recorded channel identity"),
                version,
                0,
                if status == crate::channels::DONE {
                    "send"
                } else {
                    "send closed"
                },
            );
        }
        unsafe { out.write(status) };
        status
    });
    drop(state);
    drop(cell);
    unsafe { role::enter(ctx) };
    if publication? == crate::channels::DONE {
        scheduler(ctx).channels.touch(channel);
    }
    unsafe { try_yield(&mut *ctx) }
}

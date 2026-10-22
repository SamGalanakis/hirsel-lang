//! Request ownership and native recorded host phases. This foundation
//! admits explicit bounded host tests, home readiness, cancellation and retries.
//! General adapter routing, resource discovery and terminal activation remain
//! gated.
use super::super::*;
use crate::io::{AdapterError, Completion, Request};
use crate::io_keys::{KeySet, Mode, Positions};
use crate::tape_wire::Wire;
use hirsel_free::claim::Claim;
use hirsel_free::phase::Progress;
use std::sync::Arc;
pub(crate) mod concurrent;
pub(super) mod handles;
use handles::{Created, Envelope};

pub(crate) struct Pending {
    shared: Arc<super::record::Recording>,
    request: Arc<hirsel_free::claim::Request<Request>>,
    path: Vec<u32>,
    entered: Progress,
    completion: Option<Result<Completion, AdapterError>>,
    encoded: Vec<u8>,
    keys: KeySet,
    names: Vec<(u32, String)>,
    positions: Option<Positions>,
    attempt: u32,
    home_log: Option<Box<super::record::Task>>,
    home_token: Option<u64>,
    canceled: Option<u32>,
    created: Vec<Created>,
}

/// # Safety
/// Recorded Free admission, scheduler role held, and no outstanding request.
pub(crate) unsafe fn prepare(ctx: *mut TaskCtx, request: Request) -> Stopped<()> {
    unsafe { prepare_with_keys(ctx, request, KeySet::coupled()) }
}

/// # Safety
/// As for prepare; keys are logical resources declared by the admitted adapter.
pub(crate) unsafe fn prepare_with_keys(
    ctx: *mut TaskCtx,
    request: Request,
    keys: KeySet,
) -> Stopped<()> {
    let shared = super::record::owner(ctx).expect("recorded host-phase admission");
    let names = keys
        .keys()
        .iter()
        .map(|(key, _)| {
            let name = format!("host-resource/{}", key.0);
            (shared.recorder.key(&name), name)
        })
        .collect();
    assert!(unsafe { (*ctx).pending_host.is_none() });
    let request = shared.prepare_request(request);
    // The task owns all buffers before begin can reject a replay cut or
    // another runtime entry can observe an external stop.
    unsafe {
        (*ctx).pending_host = Some(Box::new(Pending {
            shared,
            request,
            path: Vec::new(),
            entered: Progress::START,
            completion: None,
            encoded: Vec::new(),
            keys,
            names,
            positions: None,
            attempt: 0,
            home_log: None,
            home_token: None,
            canceled: None,
            created: Vec::new(),
        }));
    }
    super::record::begin(ctx);
    let (path, entered) = super::record::position(ctx);
    let pending = unsafe { (*ctx).pending_host.as_deref_mut().unwrap() };
    pending.path = path;
    pending.entered = entered;
    let pending = pending as *mut Pending;
    let s = scheduler(ctx) as *mut Scheduler;
    let shield = unsafe { (*ctx).shield };
    super::record::publish(ctx, Phase::Entered, |writer| unsafe {
        let writer = writer.expect("a recorded host entry");
        writer.prepare(&(*pending).request);
        take_positions(&mut *pending, writer);
        observe_cancel(&mut *pending, &mut *s, shield, writer);
    })
}

/// # Safety
/// As for prepare_with_keys. The adapter assigns resource by live alias
/// discovery; it is a logical key, never an fd/inode or the new handle word.
pub(crate) unsafe fn prepare_created(
    ctx: *mut TaskCtx,
    request: Request,
    keys: KeySet,
    resource: crate::io_keys::Key,
) -> Stopped<()> {
    unsafe { prepare_creations(ctx, request, keys, &[resource]) }
}

/// Reserve one identity per initialized result handle, in field order. Split
/// endpoints may share a resource, but always receive distinct handle identities.
/// # Safety
/// As for prepare_created; resources come from admitted live alias discovery.
pub(crate) unsafe fn prepare_creations(
    ctx: *mut TaskCtx,
    request: Request,
    keys: KeySet,
    resources: &[crate::io_keys::Key],
) -> Stopped<()> {
    let resources: Vec<_> = resources
        .iter()
        .map(|&key| (key, crate::io_keys::Kind::Other))
        .collect();
    unsafe { prepare_typed_creations(ctx, request, keys, &resources) }
}

/// As prepare_creations, preserving the adapter's discovered resource kind.
/// # Safety
/// The owner holds the role; every (key, kind) is admitted live metadata.
pub(crate) unsafe fn prepare_typed_creations(
    ctx: *mut TaskCtx,
    request: Request,
    keys: KeySet,
    resources: &[(crate::io_keys::Key, crate::io_keys::Kind)],
) -> Stopped<()> {
    unsafe { prepare_with_keys(ctx, request, keys) }?;
    let ordinals: Vec<_> = resources
        .iter()
        .map(|_| super::record::reserve_host_handle(ctx))
        .collect();
    let pending = unsafe { (*ctx).pending_host.as_deref_mut().unwrap() };
    pending.created = resources
        .iter()
        .zip(ordinals)
        .map(|(&(resource, kind), ordinal)| Created {
            path: pending.path.clone(),
            ordinal,
            resource: resource.0,
            kind,
        })
        .collect();
    Ok(())
}

/// # Safety
/// The owner holds the role and has published its prepared request.
pub(crate) unsafe fn claim(ctx: *mut TaskCtx) -> Stopped<bool> {
    unsafe { durable_entered(ctx) }?;
    unsafe { wait_keys(ctx) }?;
    let pending = unsafe { (*ctx).pending_host.as_deref().unwrap() } as *const Pending;
    super::record::publish(ctx, Phase::Claimed, |writer| unsafe {
        (&(*pending).shared)
            .recorder
            .claim(writer.expect("a recorded claim"), &(*pending).request)
    })
}

unsafe fn durable_entered(ctx: *mut TaskCtx) -> Stopped<()> {
    let pending = unsafe { (*ctx).pending_host.as_deref().unwrap() } as *const Pending;
    let durable = unsafe {
        (&(*pending).shared)
            .recorder
            .durable_through(&(*pending).path, (*pending).entered)
    }
    .map_err(|error| {
        Stop::Environmental(hirsel_run_schema::Environmental::Recording(
            error.to_string(),
        ))
    })?;
    if !durable {
        return Err(Stop::Requested);
    }
    Ok(())
}

/// Prepare/durability and key waits ran on the owner. Home claims the same
/// request word against the cutter, then calls the adapter. The log and all
/// buffers live in Pending while the synchronous bridge owns their loan.
/// # Safety
/// The owner holds the role, Entered published, and the home Binding is live.
pub(crate) unsafe fn issue_home(ctx: *mut TaskCtx) -> Stopped<bool> {
    let run = unsafe { (*ctx).run_ptr() };
    unsafe {
        issue_home_with(ctx, |request| {
            let binding = &mut *(*run).io_binding().cast::<crate::io::Binding>();
            binding.adapter.call(request.clone())
        })
    }
}

/// # Safety
/// As for issue_home; call runs only on home, is bounded and cannot suspend
/// Hirsel or retain its request borrow. It must preserve home affinity.
unsafe fn issue_home_with(
    ctx: *mut TaskCtx,
    call: impl FnOnce(&Request) -> Result<Completion, AdapterError>,
) -> Stopped<bool> {
    unsafe { issue_home_loan(ctx, || (), call) }
}

unsafe fn issue_home_loan(
    ctx: *mut TaskCtx,
    before_claim: impl FnOnce(),
    call: impl FnOnce(&Request) -> Result<Completion, AdapterError>,
) -> Stopped<bool> {
    unsafe { durable_entered(ctx) }?;
    unsafe { wait_keys(ctx) }?;
    let pending = unsafe { (*ctx).pending_host.as_deref_mut().unwrap() } as *mut Pending;
    unsafe { (*pending).home_log = Some(super::record::lend_host(ctx)) };
    let mut call = Some(call);
    let mut before_claim = Some(before_claim);
    let mut issued = None;
    let bridged = {
        let _segment = unsafe { role::Segment::new(ctx) };
        unsafe {
            crate::pool::call_home(ctx, &mut || {
                before_claim.take().expect("one home loan")();
                let pending = &mut *pending;
                let claimed = pending
                    .home_log
                    .as_mut()
                    .unwrap()
                    .claim_host(&pending.request);
                if matches!(claimed, Ok(true)) {
                    let mut completion =
                        call.take().expect("one bounded home call")(pending.request.buffers());
                    let encoded = handles::encode(pending, &mut completion);
                    pending.completion = Some(completion);
                    if let Err(stop) = encoded {
                        issued = Some(Err(stop));
                        return;
                    }
                }
                issued = Some(claimed);
            })
        }
    };
    // call_home waits for a claimed callback, even when stop was requested.
    // Restore the encoder before propagating a withdrawal or terminal cut.
    unsafe { super::record::restore_host(ctx, (*pending).home_log.take().unwrap()) };
    bridged?;
    issued.expect("a served home claim")
}

/// Recorded home request with real token readiness and owner-side retries.
/// # Safety
/// Explicit recorded Free admission, role held, bound home adapter; keys and
/// payloads use the admitted subset. General production routing stays gated.
pub(crate) unsafe fn request_home(
    ctx: *mut TaskCtx,
    request: Request,
    keys: KeySet,
) -> Stopped<Result<Completion, AdapterError>> {
    unsafe { prepare_with_keys(ctx, request, keys) }?;
    let run = unsafe { (*ctx).run_ptr() };
    let pending = unsafe { (*ctx).pending_host.as_deref_mut().unwrap() } as *mut Pending;
    {
        let _segment = unsafe { role::Segment::new(ctx) };
        unsafe {
            crate::pool::call_home(ctx, &mut || {
                (*pending).home_token = Some(crate::host_waits::native_begin_home(
                    run,
                    (*pending).request.buffers(),
                ));
            })
        }?;
    }
    let token = unsafe { (*pending).home_token.unwrap() };
    let cause = unsafe { (*pending).canceled };
    if !unsafe {
        issue_home_with(ctx, |_| {
            crate::host_waits::native_call_home(run, token, cause)
        })
    }? {
        return Err(Stop::Requested);
    }
    unsafe { ended(ctx) }?;
    loop {
        if !matches!(
            unsafe { &(*pending).completion },
            Some(Err(AdapterError::Blocked))
        ) {
            break;
        }
        assert!(
            unsafe { (*pending).positions.is_none() },
            "no key across readiness"
        );
        // Ended released every key. Home observes only readiness, and wakes
        // only after this task's physical save; the owner issues Attempt.
        unsafe { try_block(&mut *ctx, Wait::Host { token }, false, 0) }?;
        unsafe { attempt(ctx) }?;
        let cause = unsafe { (*pending).canceled };
        let mut encoded = None;
        {
            let _segment = unsafe { role::Segment::new(ctx) };
            unsafe {
                crate::pool::call_home(ctx, &mut || {
                    let mut result = crate::host_waits::native_call_home(run, token, cause);
                    encoded = Some(handles::encode(&mut *pending, &mut result));
                    (*pending).completion = Some(result);
                })
            }?;
        }
        encoded.expect("one bounded home retry")?;
        unsafe { ended(ctx) }?;
    }
    {
        let _segment = unsafe { role::Segment::new(ctx) };
        unsafe {
            crate::pool::call_home(ctx, &mut || {
                crate::host_waits::native_finish_home(run, token)
            })
        }?;
    }
    unsafe { complete_result(ctx) }
}

/// # Safety
/// The owner holds the role and has claimed its request. The bounded host
/// call neither suspends Hirsel nor retains its borrowed request buffers.
pub(crate) unsafe fn invoke(
    ctx: *mut TaskCtx,
    call: impl FnOnce(&Request) -> Completion,
) -> Stopped<()> {
    unsafe { invoke_result(ctx, |request| Ok(call(request))) }
}

/// # Safety
/// As for invoke. The ConcurrentAdapter promises one bounded call; its owned
/// request clone does not escape into a Hirsel frame or a suspended callback.
pub(crate) unsafe fn invoke_concurrent(
    ctx: *mut TaskCtx,
    adapter: &dyn crate::io_concurrent::ConcurrentAdapter,
) -> Stopped<()> {
    let pending = unsafe { (*ctx).pending_host.as_ref().unwrap() };
    let (token, cause) = (pending.request.id, pending.canceled);
    unsafe {
        invoke_discovered(
            ctx,
            |request| {
                if let Some(cause) = cause
                    && adapter.cancellable(token)
                    && let Some(completion) =
                        crate::io_checks::cancel_concurrent(adapter, token, request, cause)?
                {
                    return Ok(completion);
                }
                adapter.call_at(token, request.clone())
            },
            |handle| adapter.identity(handle),
        )
    }
}

/// Discover every initialized handle after the bounded effect. The adapter
/// returns registered metadata, never a key inferred from the host word.
/// # Safety
/// As for invoke; identity is a bounded, read-only adapter lookup. No adapter
/// reference or scheduler role may survive into a readiness wait.
pub(crate) unsafe fn invoke_discovered(
    ctx: *mut TaskCtx,
    call: impl FnOnce(&Request) -> Result<Completion, AdapterError>,
    identity: impl Fn(u64) -> Option<(crate::io_keys::Key, crate::io_keys::Kind)>,
) -> Stopped<()> {
    unsafe {
        invoke_resolved(ctx, call, |completion| {
            handles::capture(completion)?;
            let fields =
                handles::fields(completion).map_err(|error| Stop::Refused(error.into()))?;
            let resources = fields
                .into_iter()
                .flatten()
                .map(|word| {
                    identity(*word).ok_or_else(|| {
                        Stop::Refused("created host handle has no registered resource".into())
                    })
                })
                .collect::<Stopped<Vec<_>>>()?;
            Ok(Some(resources))
        })
    }
}

unsafe fn invoke_result(
    ctx: *mut TaskCtx,
    call: impl FnOnce(&Request) -> Result<Completion, AdapterError>,
) -> Stopped<()> {
    unsafe { invoke_resolved(ctx, call, |_| Ok(None)) }
}

type Resources = Vec<(crate::io_keys::Key, crate::io_keys::Kind)>;

unsafe fn invoke_resolved(
    ctx: *mut TaskCtx,
    call: impl FnOnce(&Request) -> Result<Completion, AdapterError>,
    resolve: impl FnOnce(&mut Result<Completion, AdapterError>) -> Stopped<Option<Resources>>,
) -> Stopped<()> {
    crate::stop::observe(ctx)?;
    let pending = unsafe { (*ctx).pending_host.as_deref().unwrap() } as *const Pending;
    assert_eq!(
        unsafe { (*pending).request.state() },
        Claim::ClaimedForIssue
    );
    assert!(unsafe {
        (*pending)
            .positions
            .as_ref()
            .expect("an active host transition")
            .ready()
    });
    let (mut completion, resources) = {
        // No role, section, writer or session lock crosses either adapter call.
        let _segment = unsafe { role::Segment::new(ctx) };
        let mut completion = call(unsafe { (*pending).request.buffers() });
        let resources = resolve(&mut completion);
        (completion, resources)
    };
    let pending = unsafe { (*ctx).pending_host.as_deref_mut().unwrap() };
    // Keep a refused discovery's result task-owned for stop/quarantine cleanup.
    let resources = match resources {
        Ok(resources) => resources,
        Err(stop) => {
            pending.completion = Some(completion);
            return Err(stop);
        }
    };
    if let Some(resources) = resources {
        if pending.created.is_empty() {
            pending.created = resources
                .into_iter()
                .map(|(resource, kind)| Created {
                    path: pending.path.clone(),
                    ordinal: super::record::reserve_host_handle(ctx),
                    resource: resource.0,
                    kind,
                })
                .collect();
        } else if pending
            .created
            .iter()
            .map(|created| (crate::io_keys::Key(created.resource), created.kind))
            .ne(resources)
        {
            pending.completion = Some(completion);
            return Err(Stop::Refused(
                "created host resource differs from prepared metadata".into(),
            ));
        }
    }
    let encoded = handles::encode(pending, &mut completion);
    pending.completion = Some(completion);
    encoded
}

/// # Safety
/// The owner holds the role; the bounded call returned or registered a wait.
pub(crate) unsafe fn ended(ctx: *mut TaskCtx) -> Stopped<()> {
    let pending = unsafe { (*ctx).pending_host.as_deref_mut().unwrap() } as *mut Pending;
    let index = unsafe { (*pending).attempt };
    super::record::publish(ctx, Phase::Ended(index), |writer| unsafe {
        let writer = writer.expect("a recorded host observation");
        writer.observation(&(*pending).encoded);
        if matches!(&(*pending).completion, Some(Err(AdapterError::Blocked))) {
            writer.blocked();
        }
        (*pending)
            .positions
            .take()
            .expect("an active host transition")
            .end();
    })?;
    // Key positions ended in the same section as the publication. Wake
    // after leaving it, under scheduler ownership, with no key lock held.
    wake_retrying(ctx);
    Ok(())
}

fn take_positions(pending: &mut Pending, writer: &mut hirsel_free::concurrent::Writer<'_, '_>) {
    assert!(
        pending.positions.is_none(),
        "no key survives a readiness wait"
    );
    let positions = pending.shared.host_keys.take(&pending.keys);
    for ((_, mode, epoch), (id, name)) in positions.epochs().zip(&pending.names) {
        writer.key(*id, name, epoch, mode == Mode::Shared, pending.attempt);
    }
    pending.positions = Some(positions);
}

fn observe_cancel(
    pending: &mut Pending,
    s: &mut Scheduler,
    shield: u32,
    writer: &mut hirsel_free::concurrent::Writer<'_, '_>,
) {
    if shield == 0 && crate::io_checks::cancellation_result(pending.request.buffers(), 0).is_some()
    {
        pending.canceled = s.canceled(shield);
        writer.cancel(pending.canceled);
    } else {
        pending.canceled = None;
    }
}

/// Wait without a section, a key lock, or an owner on the saved stack. Ending
/// a predecessor and registering this wait both require the scheduler role,
/// so the readiness check cannot lose an end notification before the save.
unsafe fn wait_keys(ctx: *mut TaskCtx) -> Stopped<()> {
    loop {
        crate::stop::observe(ctx)?;
        let ready = unsafe { (*ctx).pending_host.as_deref().unwrap() }
            .positions
            .as_ref()
            .expect("a host transition")
            .ready();
        if ready {
            return Ok(());
        }
        let s = scheduler(ctx);
        let me = s.current;
        if !s.retrying.contains(&me) {
            s.retrying.push(me);
        }
        unsafe { try_block(&mut *ctx, Wait::Retry { cancelable: false }, false, 0) }?;
    }
}

/// # Safety
/// The owner holds the role; the previous bounded call ended and readiness
/// permits one more bounded attempt of this same semantic operation.
pub(crate) unsafe fn attempt(ctx: *mut TaskCtx) -> Stopped<()> {
    let pending = unsafe { (*ctx).pending_host.as_deref_mut().unwrap() } as *mut Pending;
    let index = unsafe {
        (*pending)
            .attempt
            .checked_add(1)
            .expect("host attempt overflow")
    };
    let s = scheduler(ctx) as *mut Scheduler;
    let shield = unsafe { (*ctx).shield };
    super::record::publish(ctx, Phase::Attempt(index), |writer| unsafe {
        (*pending).attempt = index;
        let writer = writer.expect("a recorded host attempt");
        writer.ready();
        take_positions(&mut *pending, writer);
        observe_cancel(&mut *pending, &mut *s, shield, writer);
    })?;
    unsafe { wait_keys(ctx) }
}

/// # Safety
/// The owner holds the role; its call returned and Ended was published.
pub(crate) unsafe fn complete(ctx: *mut TaskCtx) -> Stopped<Completion> {
    unsafe { complete_result(ctx) }?.map_err(Stop::refusal)
}

/// # Safety
/// As for complete. A Blocked adapter result needs readiness and another
/// attempt, so only a returned completion or terminal adapter error is Done.
pub(crate) unsafe fn complete_result(
    ctx: *mut TaskCtx,
) -> Stopped<Result<Completion, AdapterError>> {
    let pending = unsafe { (*ctx).pending_host.as_deref().unwrap() } as *const Pending;
    assert!(
        unsafe { (*pending).positions.is_none() },
        "end positions before completion"
    );
    assert!(
        !matches!(
            unsafe { &(*pending).completion },
            Some(Err(AdapterError::Blocked))
        ),
        "Blocked needs another bounded attempt"
    );
    super::record::publish(ctx, Phase::Done, |writer| unsafe {
        writer
            .expect("a recorded completion")
            .complete(&(*pending).request, &(*pending).encoded);
    })?;
    let durable = unsafe {
        (&(*pending).shared).recorder.durable_through(
            &(*pending).path,
            Progress::new((*pending).entered.op, Phase::Done),
        )
    }
    .map_err(|error| {
        Stop::Environmental(hirsel_run_schema::Environmental::Recording(
            error.to_string(),
        ))
    })?;
    if !durable {
        return Err(Stop::Requested);
    }
    // A terminal cut rejects this publication, so no late completion reaches
    // its caller. The task retains it until teardown on the error path.
    let mut pending = unsafe { (*ctx).pending_host.take().unwrap() };
    pending.shared.recorder.retire(pending.request.id);
    Ok(pending
        .completion
        .take()
        .expect("one returned host completion"))
}

/// Replay-only request ownership. Handle creations carry an explicit
/// creator-qualified identity and persisted resource; other shapes are gated.
pub(crate) struct ReplayHost {
    _request: Request,
    keys: Vec<(String, bool)>,
    positions: Vec<hirsel_free::recorder::Position>,
    attempt: u32,
    observed: Option<Result<Completion, AdapterError>>,
}

/// # Safety
/// Replay admission, role held; result uses an admitted payload/handle shape.
/// The caller delivers the result then checks replay::start before pure code.
pub(crate) unsafe fn replay_request(
    ctx: *mut TaskCtx,
    request: Request,
    keys: KeySet,
) -> Stopped<Result<Completion, AdapterError>> {
    assert!(unsafe { (*ctx).replay_host.is_none() });
    unsafe {
        (*ctx).replay_host = Some(Box::new(ReplayHost {
            _request: request,
            keys: keys
                .keys()
                .iter()
                .map(|(key, mode)| (format!("host-resource/{}", key.0), *mode == Mode::Shared))
                .collect(),
            positions: Vec::new(),
            attempt: 0,
            observed: None,
        }))
    };
    drop(keys);
    super::record::begin(ctx);
    {
        let token = super::replay::token(ctx).expect("native host replay admission");
        token.publish(ctx, Phase::Entered, || {
            let host = unsafe { (*ctx).replay_host.as_deref_mut().unwrap() };
            host.positions = token.host_positions(&host.keys, 0)?;
            if unsafe { (*ctx).shield } == 0
                && crate::io_checks::cancellation_result(&host._request, 0).is_some()
            {
                token.cancel(None)?;
            }
            Ok::<_, Stop>(())
        })??;
    }
    unsafe { replay_wait_keys(ctx) }?;
    {
        let token = super::replay::token(ctx).unwrap();
        token.publish(ctx, Phase::Claimed, || ())?;
    }
    loop {
        let index = unsafe { (*ctx).replay_host.as_ref().unwrap().attempt };
        {
            let token = super::replay::token(ctx).unwrap();
            token.publish(ctx, Phase::Ended(index), || {
                let host = unsafe { (*ctx).replay_host.as_deref_mut().unwrap() };
                let bytes = token.host_observation(&host.positions)?;
                host.positions.clear();
                let envelope = crate::tape_wire::decode(&bytes).map_err(Stop::Diverged)?;
                let result = token.host_result(envelope)?;
                if matches!(result, Err(AdapterError::Blocked)) {
                    token.host_readiness(false)?;
                }
                host.observed = Some(result);
                Ok::<_, Stop>(())
            })??;
        }
        if !matches!(
            unsafe { &(*ctx).replay_host.as_ref().unwrap().observed },
            Some(Err(AdapterError::Blocked))
        ) {
            break;
        }
        {
            let token = super::replay::token(ctx).unwrap();
            let index = index
                .checked_add(1)
                .ok_or_else(|| Stop::Diverged("host attempt overflow".into()))?;
            token.publish(ctx, Phase::Attempt(index), || {
                let host = unsafe { (*ctx).replay_host.as_deref_mut().unwrap() };
                host.attempt = index;
                token.host_readiness(true)?;
                host.positions = token.host_positions(&host.keys, index)?;
                if unsafe { (*ctx).shield } == 0
                    && crate::io_checks::cancellation_result(&host._request, 0).is_some()
                {
                    token.cancel(None)?;
                }
                Ok::<_, Stop>(())
            })??;
        }
        unsafe { replay_wait_keys(ctx) }?;
    }
    {
        let token = super::replay::token(ctx).unwrap();
        token.publish(ctx, Phase::Done, || {
            let bytes = token.host_completion()?;
            let envelope = crate::tape_wire::decode(&bytes).map_err(Stop::Diverged)?;
            let result = token.host_result(envelope)?;
            let host = unsafe { (*ctx).replay_host.as_deref_mut().unwrap() };
            if host.observed.as_ref() != Some(&result) {
                return Err(Stop::Diverged(
                    "host completion differs from final observation".into(),
                ));
            }
            host.observed = Some(result);
            Ok::<_, Stop>(())
        })??;
    }
    let mut host = unsafe { (*ctx).replay_host.take().unwrap() };
    Ok(host.observed.take().unwrap())
}

/// Keys of a handle created by an admitted host completion, reconstructed
/// solely from tape metadata. A missing mapping diverges without adapter use.
pub(crate) fn replay_created_identity(
    ctx: *mut TaskCtx,
    word: u64,
) -> Stopped<(crate::io_keys::Key, crate::io_keys::Kind)> {
    super::replay::token(ctx)
        .expect("host replay admission")
        .host_identity(word)
}

pub(crate) fn replay_created_key(ctx: *mut TaskCtx, word: u64) -> Stopped<crate::io_keys::Key> {
    super::replay::token(ctx)
        .expect("host replay admission")
        .host_resource(word)
}

unsafe fn replay_wait_keys(ctx: *mut TaskCtx) -> Stopped<()> {
    loop {
        let ready = {
            let token = super::replay::token(ctx).unwrap();
            token.host_ready(&unsafe { (*ctx).replay_host.as_ref().unwrap() }.positions)
        };
        if ready {
            return Ok(());
        }
        if super::replay::at_cut(ctx) {
            return Err(Stop::Requested);
        }
        let s = scheduler(ctx);
        let me = s.current;
        if !s.retrying.contains(&me) {
            s.retrying.push(me);
        }
        unsafe { try_block(&mut *ctx, Wait::Retry { cancelable: false }, false, 0) }?;
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod readiness_tests;

#[cfg(test)]
mod process_tests;

#[cfg(test)]
mod cancellation_tests;

#[cfg(test)]
mod concurrent_tests;

//! Retirement boundaries for endpoint/mutex closes, roots and thaw.
//!
//! A close detaches its images in a bounded head callback. The owning task
//! then scans and finalizes them outside Frontier, so nested ops may suspend
//! without retaining policy or object borrows. Shared-root releases instead
//! own their credits in the log and retire without user callbacks.

use super::super::*;
use super::engine;
use crate::ordered::objects::Kind;
use crate::ordered::{Next, Progress};

/// Reuse the compiler's complete type-edge graph. A cyclic component is safe
/// only if every layout it can reach has a known program without user code or
/// handle close. Missing/erased metadata never certifies a speculative drop.
/// # Safety
/// Admission only; the immutable linked descriptor registry is live.
pub(super) unsafe fn plain_layouts(
    ctx: *mut TaskCtx,
    types: &crate::ordered::capture::Types,
) -> std::collections::BTreeSet<u32> {
    let mut plain: std::collections::BTreeSet<_> = types
        .definitions
        .keys()
        .copied()
        .filter(|&id| {
            let layout = unsafe { &*(*ctx).descriptor(id) };
            if layout.finalizer.is_some() || layout.flags & crate::LAYOUT_MUTEX_ROOT != 0 {
                return false;
            }
            let steps = if layout.step_count == 0 {
                &[]
            } else {
                unsafe { std::slice::from_raw_parts(layout.steps, layout.step_count as usize) }
            };
            steps.iter().all(|step| {
                matches!(
                    step.kind,
                    crate::STEP_RELEASE
                        | crate::STEP_RELEASE_LAST
                        | crate::STEP_RELEASE_ROOT
                        | crate::STEP_TAG
                        | crate::STEP_NONNULL
                        | crate::STEP_LIVE_BIT
                        | crate::STEP_LOOP
                        | crate::STEP_COLUMN
                        | crate::STEP_THAW
                )
            }) && (steps.is_empty() || layout.scan_words != 0)
        })
        .collect();
    loop {
        let rejected: Vec<_> = plain
            .iter()
            .copied()
            .filter(|id| !types.definitions[id].is_subset(&plain))
            .collect();
        if rejected.is_empty() {
            break;
        }
        for id in rejected {
            plain.remove(&id);
        }
    }
    plain
}

/// # Safety
/// As for close_endpoint; this entry owns the role and one endpoint credit.
pub(crate) unsafe fn endpoint(
    ctx: *mut TaskCtx,
    word: u64,
    anchor: u64,
    operation_site: u64,
) -> Stopped<u8> {
    if word & TASK_SET != 0 {
        super::sets::close(ctx, word)?;
        return Ok(STATUS_OK);
    }
    // A sender is statically drop.trivial, including a provisional duplicate
    // whose earlier initialization is still in this same owner's log.
    if engine::is_sender(ctx, word) {
        engine::sender_close(ctx, word)?;
        return Ok(STATUS_OK);
    }
    let descriptor = engine::carrier_descriptor(ctx, Kind::Receiver, word);
    if engine::with_driver(ctx, |d| d.plain_layouts.contains(&descriptor)) {
        engine::submit(
            ctx,
            super::Command::PlainClose { word, mutex: false }.operation(engine::footprint(
                ctx,
                Kind::Receiver,
                word,
                true,
            )),
        )?;
        return Ok(STATUS_OK);
    }
    engine::at_head(ctx)?;
    let (_, sender) = role::with_role(ctx, |s| s.channels.endpoint_of(word));
    let name = if sender {
        "drop.trivial"
    } else {
        let descriptor = engine::carrier_descriptor(ctx, Kind::Receiver, word);
        let layout = unsafe { &*(*ctx).descriptor(descriptor) };
        if layout.finalizer.is_none() && layout.step_count == 0 {
            "drop.trivial"
        } else {
            "drop.leftovers"
        }
    };
    let (left, progress) = engine::head_canonical(
        ctx,
        name,
        || {
            let left = role::with_role(ctx, |s| {
                #[cfg(not(feature = "ownership-trace"))]
                {
                    if super::events::enabled(s) {
                        s.channels.close_images(word)
                    } else {
                        s.channels.close(word)
                    }
                }
                #[cfg(feature = "ownership-trace")]
                {
                    let observing = super::events::enabled(s);
                    s.channels
                        .close_observed(word, observing, unsafe { (*ctx).ownership_task })
                }
            });
            (left, Next::Keep)
        },
        |left| unsafe {
            super::events::close_observed(
                ctx,
                Kind::Receiver,
                word,
                left.as_ref()
                    .map(|(images, descriptor)| (images, *descriptor)),
            )
        },
    )?;
    assert!(matches!(progress, Progress::Absorbed(_)));
    let Some((left, descriptor)) = left else {
        return Ok(STATUS_OK);
    };
    // Finalizers can obtain these handles without a scheduling point. Refresh
    // before invoking any of them, including nested receivers in later images.
    for index in 0..left.len() {
        unsafe { engine::obtained_image(ctx, left.at(index), descriptor) };
    }
    let mut status = STATUS_OK;
    for index in 0..left.len() {
        let mut continuation = std::mem::MaybeUninit::<crate::finalize::Cleanup>::uninit();
        // SAFETY: the detached FIFO owns every live image through this drop.
        status |= unsafe {
            crate::finalize::hirsel_rt_v2_finalize(
                ctx,
                descriptor,
                left.at(index),
                std::ptr::null(),
                continuation.as_mut_ptr(),
                anchor,
                operation_site,
            )
        };
    }
    Ok(status)
}

/// # Safety
/// As for close_mutex; the last wrapper exclusively owns the cell.
pub(crate) unsafe fn mutex(
    ctx: *mut TaskCtx,
    word: u64,
    anchor: u64,
    operation_site: u64,
) -> Stopped<u8> {
    let descriptor = engine::carrier_descriptor(ctx, Kind::Mutex, word);
    if engine::with_driver(ctx, |d| d.plain_layouts.contains(&descriptor)) {
        engine::submit(
            ctx,
            super::Command::PlainClose { word, mutex: true }.operation(engine::footprint(
                ctx,
                Kind::Mutex,
                word,
                true,
            )),
        )?;
        return Ok(STATUS_OK);
    }
    engine::at_head(ctx)?;
    let descriptor = engine::carrier_descriptor(ctx, Kind::Mutex, word);
    let layout = unsafe { &*(*ctx).descriptor(descriptor) };
    let name = if layout.finalizer.is_none() && layout.step_count == 0 {
        "drop.trivial"
    } else {
        "drop.leftovers"
    };
    let (left, progress) = engine::head_canonical(
        ctx,
        name,
        || {
            let left = role::with_role(ctx, |s| {
                if super::events::enabled(s) {
                    s.sync.close_images(word)
                } else {
                    s.sync.close(word)
                }
            });
            (left, Next::Keep)
        },
        |left| unsafe {
            super::events::close_observed(
                ctx,
                Kind::Mutex,
                word,
                left.as_ref()
                    .map(|(images, descriptor)| (images, *descriptor)),
            )
        },
    )?;
    assert!(matches!(progress, Progress::Absorbed(_)));
    let Some((image, descriptor)) = left else {
        return Ok(STATUS_OK);
    };
    unsafe { engine::obtained_image(ctx, image.at(0), descriptor) };
    let mut continuation = std::mem::MaybeUninit::<crate::finalize::Cleanup>::uninit();
    Ok(unsafe {
        crate::finalize::hirsel_rt_v2_finalize(
            ctx,
            descriptor,
            image.at(0),
            std::ptr::null(),
            continuation.as_mut_ptr(),
            anchor,
            operation_site,
        )
    })
}

/// Drop detached, statically certified images. No scheduler borrow survives
/// this walk and nested roots apply in the same retirement credit event.
/// # Safety
/// The images and descriptor match and are owned by this retiring command;
/// admission certified their entire type graph has no finalizer/handle close.
pub(super) unsafe fn plain_images(
    ctx: *mut TaskCtx,
    images: &crate::channels::Ring,
    descriptor: u32,
) {
    unsafe {
        crate::roots::immediate(ctx, || {
            for index in 0..images.len() {
                let mut continuation = std::mem::MaybeUninit::<crate::finalize::Cleanup>::uninit();
                let status = crate::finalize::hirsel_rt_v2_finalize(
                    ctx,
                    descriptor,
                    images.at(index),
                    std::ptr::null(),
                    continuation.as_mut_ptr(),
                    0,
                    0,
                );
                assert_eq!(status, STATUS_OK, "certified cleanup cannot fail");
            }
        })
    };
}

/// Root decrement's actual atomic action is supplied by roots::consume. Its
/// walk happens after restoring Frontier and is owned by the releasing task.
/// # Safety
/// The caller owns the role and one exact root credit; action cannot suspend.
pub(crate) unsafe fn root_release(
    ctx: *mut TaskCtx,
    block: *mut Header,
    action: impl FnOnce() -> bool,
) -> bool {
    let result = engine::head_canonical(
        ctx,
        "root.release",
        || (action(), Next::Keep),
        |died| {
            // The root decrement does not release storage: the original owner
            // walks/finalizes only after this bounded observer has returned.
            let word = unsafe { crate::alloc::payload(block).cast::<u64>().read() };
            // Frontier is owned by the bounded callback. A stop-aware
            // scheduler lookup here could abandon it before restoration.
            let s = unsafe { (*(*(*ctx).run_ptr()).tasks.get()).as_deref().unwrap() };
            let Logical::Ordered(driver) = &s.logical else {
                return None;
            };
            let object = driver.objects.identity(Kind::Mutex, word)?.clone();
            Some(vec![(
                super::events::CanonicalItem::Object(object),
                super::events::CanonicalOutcome::RootReleased { last: *died },
            )])
        },
    );
    let (died, progress) = unsafe { crate::stop::check(ctx, result) };
    assert!(matches!(progress, Progress::Absorbed(_)));
    died
}

/// Transfer one Shared-root credit to its positional command. The root stays
/// alive even if this task continues past the release before retirement.
/// # Safety
/// The caller owns the role and one Shared-root credit, never a mutex root.
pub(crate) unsafe fn shared_release(ctx: *mut TaskCtx, block: *mut Header) {
    let op = super::Command::RootRelease { block }.operation(Default::default());
    let result = engine::submit(ctx, op);
    unsafe { crate::stop::check(ctx, result) };
}

/// # Safety
/// As for SharedThawTurn; the root remains borrowed through the head gate.
pub(crate) unsafe fn thaw(ctx: *mut TaskCtx, root: *mut Header, out: *mut u8) -> Stopped<u8> {
    let (last, progress) = engine::head_canonical(
        ctx,
        "shared.thaw",
        || (unsafe { crate::roots::sole(root) }, Next::Keep),
        |last| {
            let identity = unsafe { (*ctx).run_ref() }.ordered_roots.get()?.get(root)?;
            Some(vec![(
                super::events::CanonicalItem::SharedRoot(identity),
                super::events::CanonicalOutcome::Thaw { last: *last },
            )])
        },
    )?;
    assert!(matches!(progress, Progress::Absorbed(_)));
    unsafe { out.write(u8::from(last)) };
    Ok(STATUS_OK)
}

/// # Safety
/// As for MutexKill; the failure chain belongs to this head task.
pub(crate) unsafe fn kill(ctx: *mut TaskCtx, word: u64) -> Stopped<u8> {
    let (_, progress) = engine::head_canonical(
        ctx,
        "mutex.kill",
        || {
            let path = current_path(ctx);
            let chain = unsafe { (*ctx).failure.chain(&path) };
            let canonical = role::with_role(ctx, |s| {
                let canonical = unsafe { super::events::mutex_failures(ctx, s, word, &chain) };
                s.sync.kill(word, chain);
                canonical
            });
            (canonical, Next::Yield)
        },
        Clone::clone,
    )?;
    engine::continue_at_head(ctx, progress)?;
    Ok(STATUS_OK)
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use crate::ordered::Policy;

    const NONE: [u64; 2] = [0, 0];

    unsafe extern "C" fn log_drop(ctx: *mut TaskCtx, value: *mut u8, _: u64, _: u64) -> u8 {
        let [tag, id, mut sender] = unsafe { value.cast::<[u64; 3]>().read() };
        assert_eq!(tag, 1);
        assert_eq!(unsafe { crate::finalize::hirsel_rt_v2_shielded(ctx) }, 1);
        let mut canceled = u32::MAX;
        assert_eq!(
            unsafe { hirsel_rt_v2_task_canceled(ctx, &mut canceled, &0) },
            STATUS_OK
        );
        assert_eq!(canceled, 0, "destruction suppresses cancellation");
        let mut message = [1, id];
        let mut status = u32::MAX;
        assert_eq!(
            unsafe {
                hirsel_rt_v2_channel_send(
                    ctx,
                    &mut status,
                    &mut sender,
                    message.as_mut_ptr().cast(),
                )
            },
            STATUS_OK
        );
        assert_eq!(status, crate::channels::DONE);
        assert_eq!(message, NONE);
        STATUS_OK
    }

    unsafe extern "C" fn sibling(ctx: *mut TaskCtx, _: *mut Header, _: u64, _: u64) -> u8 {
        for _ in 0..8 {
            let mut canceled = 0;
            assert_eq!(
                unsafe { hirsel_rt_v2_task_canceled(ctx, &mut canceled, &0) },
                STATUS_OK
            );
        }
        let mut result = [1u64, 9];
        unsafe {
            hirsel_rt_v2_task_finish(
                ctx,
                result.as_mut_ptr().cast(),
                NONE.as_ptr().cast(),
                16,
                u32::MAX,
            )
        }
    }

    fn layout(size: u64) -> crate::Layout {
        crate::Layout {
            magic: crate::LAYOUT_MAGIC,
            version: crate::LAYOUT_VERSION,
            size,
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
        }
    }

    fn step(kind: u32, offset: u64) -> crate::Step {
        crate::Step {
            kind,
            field: 0,
            variant: 0,
            span: 0,
            offset,
            stride: 0,
            count: 0,
            count_offset: 0,
            layout: std::ptr::null(),
        }
    }

    /// A language-finalizer descriptor whose drop program ends its own scope
    /// with the self-referencing root end the static scope proof requires,
    /// as `tests/support` builds them. Leaked: run tables keep the pointer.
    pub(crate) fn finalized(base: crate::Layout, steps: &[crate::Step]) -> &'static crate::Layout {
        let descriptor = Box::into_raw(Box::new(base));
        let mut program = steps.to_vec();
        program.push(crate::Step {
            field: crate::WHOLE_VALUE,
            variant: 1,
            layout: descriptor,
            ..step(crate::STEP_END_FINALIZE, 0)
        });
        let program = Box::leak(program.into_boxed_slice());
        // SAFETY: the fresh leaked descriptor has no other reference yet.
        unsafe {
            (*descriptor).steps = program.as_ptr();
            (*descriptor).step_count = program.len() as u64;
            &*descriptor
        }
    }

    #[test]
    fn plain_cleanup_certificate_requires_a_complete_transitively_safe_type_graph() {
        use hirsel_core_contract::handle_scan::TYPE_EDGE;
        let release = step(crate::STEP_RELEASE, 0);
        let close = step(crate::STEP_CLOSE_HANDLE, 0);
        // Linked user layout IDs start at two, after the runtime builtins.
        let edge = |id| [TYPE_EDGE, 0, id, 0, 0, 0, 0, 0];
        let safe_cycle = edge(3);
        let to_finalizer = edge(4);
        let to_handle = edge(5);
        let to_unknown = edge(6);
        let scalar = layout(8);
        let cycle = crate::Layout {
            steps: &release,
            step_count: 1,
            field_count: 1,
            scan: safe_cycle.as_ptr(),
            scan_words: 8,
            ..layout(8)
        };
        let finalizer = finalized(
            crate::Layout {
                finalizer: Some(log_drop),
                ..layout(24)
            },
            &[],
        );
        let handle = crate::Layout {
            steps: &close,
            step_count: 1,
            field_count: 1,
            ..layout(8)
        };
        let unknown = crate::Layout {
            scan_words: u64::MAX,
            ..layout(8)
        };
        let reaches_finalizer = crate::Layout {
            scan: to_finalizer.as_ptr(),
            scan_words: 8,
            ..cycle
        };
        let reaches_handle = crate::Layout {
            scan: to_handle.as_ptr(),
            scan_words: 8,
            ..cycle
        };
        let reaches_unknown = crate::Layout {
            scan: to_unknown.as_ptr(),
            scan_words: 8,
            ..cycle
        };
        let missing_scan = crate::Layout {
            scan_words: 0,
            ..cycle
        };
        let layouts = [
            &scalar as *const crate::Layout,
            &cycle,
            finalizer,
            &handle,
            &unknown,
            &reaches_finalizer,
            &reaches_handle,
            &reaches_unknown,
            &missing_scan,
        ];
        let mut run = crate::host::BodyRun::new(1, true);
        run.config.layouts = layouts.as_ptr();
        run.config.layout_count = layouts.len() as u32;
        let outcome = run.run(
            |_| {},
            |ctx| unsafe {
                let types = crate::ordered::capture::types(ctx);
                let plain = plain_layouts(ctx, &types);
                assert!(plain.contains(&2), "null empty step table is plain");
                assert!(plain.contains(&3), "safe cyclic type closure is plain");
                for id in 4..=10 {
                    assert!(
                        !plain.contains(&id),
                        "unsafe or unknown graph {id} must stay at head"
                    );
                }
                STATUS_OK
            },
        );
        assert_eq!(outcome.ended, Ok(STATUS_OK));
    }

    #[test]
    fn another_tasks_plain_close_credits_before_its_following_allocation_summary() {
        use super::super::{Command, Objects, Outcome};
        use crate::budget::stretch::{Budgets, Settled, Stretch};
        use crate::ordered::{Frontier, Offer};
        use hirsel_core_contract::handle_scan::{ROOT, TYPE_EDGE};

        // The queued image holds a Shared root. Its nested owned child dies
        // during close too, even if the committer is a speculative task.
        let release_child = step(crate::STEP_RELEASE, 0);
        let release_root = step(crate::STEP_RELEASE_ROOT, 0);
        let child_edge = [TYPE_EDGE, 0, 2, 0, 0, 0, 0, 0];
        let root_edge = [ROOT, 0, 0, 0, 0, 0, 0, 0, TYPE_EDGE, 0, 3, 0, 0, 0, 0, 0];
        let scalar = layout(8);
        let root_layout = crate::Layout {
            flags: crate::LAYOUT_FROZEN,
            steps: &release_child,
            step_count: 1,
            field_count: 1,
            scan: child_edge.as_ptr(),
            scan_words: 8,
            ..layout(8)
        };
        let image_layout = crate::Layout {
            steps: &release_root,
            step_count: 1,
            field_count: 1,
            scan: root_edge.as_ptr(),
            scan_words: 16,
            ..layout(8)
        };
        let layouts = [&scalar as *const crate::Layout, &root_layout, &image_layout];
        for mutex in [false, true] {
            let mut run = crate::host::BodyRun::new(2, true);
            run.config.layouts = layouts.as_ptr();
            run.config.layout_count = layouts.len() as u32;
            unsafe { run.use_physical_dispatch() };
            let outcome = run.run(
                |ctx| unsafe {
                    let charge = crate::alloc::allocation_charge(ctx, 2, 8, 8);
                    (*ctx).set_budgets(None, Some(2 * charge));
                },
                move |ctx| unsafe {
                    let _role = role::Guard::new(ctx);
                    let types = crate::ordered::capture::types(ctx);
                    assert!(plain_layouts(ctx, &types).contains(&4));
                    let charge = crate::alloc::allocation_charge(ctx, 2, 8, 8);
                    let child = crate::alloc::hirsel_rt_v2_alloc(
                        ctx,
                        2,
                        8,
                        8,
                        crate::alloc::DOMAIN_HEAP,
                        0,
                    );
                    let root = crate::alloc::hirsel_rt_v2_alloc(
                        ctx,
                        3,
                        8,
                        8,
                        crate::alloc::DOMAIN_HEAP,
                        0,
                    );
                    crate::alloc::payload(root)
                        .cast::<*mut Header>()
                        .write(child);
                    let mut image = root;
                    let none = std::ptr::null::<Header>();
                    let image_layout = &*(*ctx).descriptor(4);
                    let element = crate::channels::Element::new(
                        4,
                        image_layout,
                        (&none as *const *const Header).cast(),
                    );
                    let (word, sender) = if mutex {
                        (
                            scheduler(ctx).sync.create_mutex(
                                element,
                                image_layout,
                                (&mut image as *mut *mut Header).cast(),
                            ),
                            None,
                        )
                    } else {
                        let (sender, receiver) = crate::tasks::role::with_role(ctx, |s| {
                            s.channels.open(element, image_layout)
                        });
                        let (channel, _) =
                            crate::tasks::role::with_role(ctx, |s| s.channels.endpoint_of(sender));
                        assert_eq!(
                            scheduler(ctx)
                                .channels
                                .send(channel, (&mut image as *mut *mut Header).cast()),
                            crate::channels::DONE
                        );
                        (receiver, Some(sender))
                    };
                    assert!(image.is_null(), "ownership moved into the carrier");
                    let atomic =
                        scheduler(ctx)
                            .sync
                            .create_image((&0u64 as *const u64).cast(), 8, 0, 8);
                    (*ctx).heap_ref().meter.ordered((*ctx).run_ptr());
                    let empty = || {
                        Stretch::new(Budgets {
                            fuel: false,
                            memory: true,
                        })
                    };
                    let mut frontier = Frontier::new(
                        0,
                        Policy::admit(&Default::default(), None),
                        Settled {
                            level: 2 * charge,
                            limit: 2 * charge,
                            fuel: u64::MAX,
                        },
                    );
                    frontier.enqueue(2, Default::default());
                    let mut records = Vec::new();
                    let mut record = |task, name, _: &Outcome, _| records.push((task, name));
                    let mut poll = Vec::new;
                    let mut objects = Objects::new(ctx, &mut record, &mut poll);
                    let close = Command::PlainClose { word, mutex }.operation(Default::default());
                    assert_eq!(
                        frontier.offer(2, empty(), close, &mut objects).0,
                        Offer::Logged
                    );
                    assert!(crate::roots::sole(root), "logged close retains its image");
                    let store = || {
                        Command::AtomicStore {
                            word: atomic,
                            image: 7u64.to_ne_bytes().to_vec(),
                        }
                        .operation(Default::default())
                    };
                    let mut after = empty();
                    after.alloc(2 * charge).unwrap();
                    frontier.offer(2, after, store(), &mut objects);
                    frontier.apply_head(empty(), store(), &mut objects).unwrap();
                    (*ctx).roots.speculative = true;
                    assert_eq!(frontier.absorb(&mut objects), Ok(Progress::Absorbed(2)));
                    (*ctx).roots.speculative = false;
                    assert_eq!(
                        frontier.settled.level, 0,
                        "both deaths belong to close's position"
                    );
                    assert_eq!(
                        (*ctx).ordered_freed.get(),
                        0,
                        "no credit leaks to the committer"
                    );
                    frontier.absorb(&mut objects).unwrap();
                    assert_eq!(
                        frontier.settled.level,
                        2 * charge,
                        "next allocation fits exactly"
                    );
                    assert_eq!((*ctx).ordered_freed.get(), 0);
                    drop(objects);
                    assert_eq!(records[1..], [(2, "drop.trivial"), (2, "atomic.store")]);
                    if let Some(sender) = sender {
                        crate::tasks::role::with_role(ctx, |s| s.channels.close(sender));
                    }
                    STATUS_OK
                },
            );
            assert_eq!(outcome.ended, Ok(STATUS_OK));
        }
    }

    #[test]
    fn native_plain_receiver_close_updates_the_live_meter_before_reallocation() {
        use hirsel_core_contract::handle_scan::TYPE_EDGE;
        let release = step(crate::STEP_RELEASE, 0);
        let edge = [TYPE_EDGE, 0, 2, 0, 0, 0, 0, 0];
        let scalar = layout(8);
        let image = crate::Layout {
            steps: &release,
            step_count: 1,
            field_count: 1,
            scan: edge.as_ptr(),
            scan_words: 8,
            ..layout(8)
        };
        let layouts = [&scalar as *const crate::Layout, &image];
        for width in [2, 4, 8] {
            let mut run = crate::host::BodyRun::new(width, true);
            run.config.layouts = layouts.as_ptr();
            run.config.layout_count = layouts.len() as u32;
            unsafe { run.use_physical_dispatch() };
            let outcome = run.run(
                |ctx| unsafe {
                    let charge = crate::alloc::allocation_charge(ctx, 2, 8, 8);
                    (*ctx).set_budgets(None, Some(charge));
                    engine::enable(ctx, Policy::admit(&Default::default(), Some(charge)));
                },
                |ctx| unsafe {
                    for _ in 0..3 {
                        let mut endpoints = [0u64; 2];
                        let none = std::ptr::null::<Header>();
                        assert_eq!(
                            hirsel_rt_v2_channel_open(
                                ctx,
                                std::ptr::null_mut(),
                                endpoints.as_mut_ptr(),
                                &0,
                                (&none as *const *const Header).cast(),
                                3
                            ),
                            STATUS_OK
                        );
                        let mut block = crate::alloc::hirsel_rt_v2_alloc(
                            ctx,
                            2,
                            8,
                            8,
                            crate::alloc::DOMAIN_HEAP,
                            0,
                        );
                        let mut sent = u32::MAX;
                        assert_eq!(
                            hirsel_rt_v2_channel_send(
                                ctx,
                                &mut sent,
                                &mut endpoints[0],
                                (&mut block as *mut *mut Header).cast()
                            ),
                            STATUS_OK
                        );
                        assert_eq!(sent, crate::channels::DONE);
                        assert!(block.is_null());
                        assert_eq!(
                            crate::finalize::hirsel_rt_v2_close_handle(ctx, endpoints[1], 0, 0),
                            STATUS_OK
                        );
                        let _role = role::Guard::new(ctx);
                        engine::at_head(ctx).unwrap();
                        assert_eq!(
                            crate::tasks::ordered::engine::with_driver(ctx, |d| d
                                .frontier
                                .as_ref()
                                .unwrap()
                                .settled
                                .level),
                            0
                        );
                        assert_eq!((*ctx).heap_ref().meter.held(), 0);
                        assert_eq!((*ctx).ordered_freed.get(), 0, "credit consumed once");
                        assert_eq!(
                            crate::finalize::hirsel_rt_v2_close_handle(ctx, endpoints[0], 0, 0),
                            STATUS_OK
                        );
                    }
                    let _role = role::Guard::new(ctx);
                    let task = engine::current(ctx);
                    crate::tasks::ordered::engine::with_driver(ctx, |d| d.finishing(task));
                    STATUS_OK
                },
            );
            assert_eq!(outcome.ended, Ok(STATUS_OK));
        }
    }

    pub(crate) fn run_drops(width: u32) {
        use hirsel_core_contract::handle_scan::TYPED_HANDLE;
        let scan = [TYPED_HANDLE, 16, Kind::Sender.code(), 3, 0, 0, 0, 0];
        let image = layout(16);
        let resource = finalized(
            crate::Layout {
                finalizer: Some(log_drop),
                field_count: 1,
                scan: scan.as_ptr(),
                scan_words: scan.len() as u64,
                ..layout(24)
            },
            &[step(crate::STEP_CLOSE_HANDLE, 16)],
        );
        let close_mutex = step(crate::STEP_CLOSE_HANDLE, 0);
        let wrapper = crate::Layout {
            flags: crate::LAYOUT_FROZEN | crate::LAYOUT_MUTEX_ROOT,
            steps: &close_mutex,
            step_count: 1,
            field_count: 1,
            ..layout(8)
        };
        let scalar_root = crate::Layout {
            flags: crate::LAYOUT_FROZEN,
            ..layout(8)
        };
        let environment = layout(8);
        let layouts = [
            &image as *const crate::Layout,
            resource,
            &wrapper,
            &scalar_root,
            &environment,
        ];
        let mut run = crate::host::BodyRun::new(width, false);
        run.config.layouts = layouts.as_ptr();
        run.config.layout_count = layouts.len() as u32;
        run.config.count_max = 3;
        // SAFETY: supported task/E/drop/root/thaw boundaries, unbudgeted.
        // Finalizers use only supported task and channel entries; no host.
        unsafe { run.use_physical_dispatch() };
        let outcome = run.run(
            |ctx| unsafe { engine::enable(ctx, Policy::admit(&Default::default(), None)) },
            |ctx| unsafe {
                let mut scope = 0;
                assert_eq!(
                    hirsel_rt_v2_task_scope_enter(ctx, std::ptr::null_mut(), &mut scope, &0),
                    STATUS_OK
                );
                let environment =
                    crate::alloc::hirsel_rt_v2_alloc(ctx, 6, 8, 8, crate::alloc::DOMAIN_HEAP, 0);
                crate::alloc::payload(environment).cast::<u64>().write(0);
                let mut child = 0;
                assert_eq!(
                    hirsel_rt_v2_task_spawn(
                        ctx,
                        0,
                        0,
                        std::ptr::null_mut(),
                        &mut child,
                        &scope,
                        sibling,
                        environment,
                        0,
                    ),
                    STATUS_OK
                );
                let mut log = [0u64; 2];
                assert_eq!(
                    hirsel_rt_v2_channel_open(
                        ctx,
                        std::ptr::null_mut(),
                        log.as_mut_ptr(),
                        &0,
                        NONE.as_ptr().cast(),
                        2
                    ),
                    STATUS_OK
                );
                let none = [0u64; 3];
                let mut resources = [0u64; 2];
                assert_eq!(
                    hirsel_rt_v2_channel_open(
                        ctx,
                        std::ptr::null_mut(),
                        resources.as_mut_ptr(),
                        &0,
                        none.as_ptr().cast(),
                        3
                    ),
                    STATUS_OK
                );
                for id in [101, 102] {
                    let mut duplicate = 0;
                    assert_eq!(
                        hirsel_rt_v2_sender_dup(
                            ctx,
                            std::ptr::null_mut(),
                            &mut duplicate,
                            &0,
                            &log[0]
                        ),
                        STATUS_OK
                    );
                    let mut payload = [1, id, duplicate];
                    let mut status = u32::MAX;
                    assert_eq!(
                        hirsel_rt_v2_channel_send(
                            ctx,
                            &mut status,
                            &mut resources[0],
                            payload.as_mut_ptr().cast()
                        ),
                        STATUS_OK
                    );
                    assert_eq!(status, crate::channels::DONE);
                    assert_eq!(payload, none);
                }
                assert_eq!(
                    crate::finalize::hirsel_rt_v2_close_handle(ctx, resources[1], 0, 0),
                    STATUS_OK
                );
                assert_eq!(
                    crate::finalize::hirsel_rt_v2_close_handle(ctx, resources[0], 0, 0),
                    STATUS_OK
                );
                let mut duplicate = 0;
                assert_eq!(
                    hirsel_rt_v2_sender_dup(ctx, std::ptr::null_mut(), &mut duplicate, &0, &log[0]),
                    STATUS_OK
                );
                let mut payload = [1, 103, duplicate];
                let mut mutex = 0;
                assert_eq!(
                    hirsel_rt_v2_mutex_create(
                        ctx,
                        std::ptr::null_mut(),
                        &mut mutex,
                        payload.as_mut_ptr().cast(),
                        none.as_ptr().cast(),
                        3,
                        4
                    ),
                    STATUS_OK
                );
                let block = mutex as *mut Header;
                crate::roots::hirsel_rt_v2_root_dup(ctx, block);
                assert_eq!(crate::roots::count(&*ctx, block), 2);
                assert_eq!(
                    crate::roots::hirsel_rt_v2_root_drop(ctx, block, 0, 0),
                    STATUS_OK
                );
                assert_eq!(crate::roots::count(&*ctx, block), 1);
                assert_eq!(
                    crate::roots::hirsel_rt_v2_root_drop(ctx, block, 0, 0),
                    STATUS_OK
                );
                for id in [101, 102, 103] {
                    let mut message = NONE;
                    let mut status = u32::MAX;
                    assert_eq!(
                        hirsel_rt_v2_channel_receive(
                            ctx,
                            0,
                            0,
                            &mut status,
                            &mut log[1],
                            message.as_mut_ptr().cast()
                        ),
                        STATUS_OK
                    );
                    assert_eq!(status, crate::channels::DONE);
                    assert_eq!(
                        message,
                        [1, id],
                        "leftover and mutex finalizers run in release order"
                    );
                }
                assert_eq!(
                    crate::finalize::hirsel_rt_v2_close_handle(ctx, log[0], 0, 0),
                    STATUS_OK
                );
                let mut message = NONE;
                let mut status = u32::MAX;
                assert_eq!(
                    hirsel_rt_v2_channel_receive(
                        ctx,
                        0,
                        0,
                        &mut status,
                        &mut log[1],
                        message.as_mut_ptr().cast()
                    ),
                    STATUS_OK
                );
                assert_eq!(
                    status,
                    crate::channels::CLOSED,
                    "last sender close is visible at the receive boundary"
                );
                assert_eq!(
                    crate::finalize::hirsel_rt_v2_close_handle(ctx, log[1], 0, 0),
                    STATUS_OK
                );
                let root =
                    crate::alloc::hirsel_rt_v2_alloc(ctx, 5, 8, 8, crate::alloc::DOMAIN_HEAP, 0);
                crate::alloc::payload(root).cast::<u64>().write(77);
                crate::shared::hirsel_rt_v2_freeze(ctx, root);
                crate::roots::hirsel_rt_v2_root_dup(ctx, root);
                crate::roots::hirsel_rt_v2_root_dup(ctx, root);
                assert_eq!(crate::roots::count(&*ctx, root), 3);
                crate::roots::hirsel_rt_v2_root_release(ctx, root);
                let mut last = 99;
                assert_eq!(
                    crate::shared::hirsel_rt_v2_thaw_turn(ctx, &mut last, root),
                    STATUS_OK
                );
                assert_eq!(last, 0);
                crate::roots::hirsel_rt_v2_root_release(ctx, root);
                assert_eq!(
                    crate::shared::hirsel_rt_v2_thaw_turn(ctx, &mut last, root),
                    STATUS_OK
                );
                assert_eq!(last, 1);
                crate::roots::hirsel_rt_v2_root_release(ctx, root);
                let mut result = NONE;
                assert_eq!(
                    hirsel_rt_v2_task_join(
                        ctx,
                        0,
                        0,
                        &scope,
                        &mut child,
                        result.as_mut_ptr().cast(),
                        16
                    ),
                    STATUS_OK
                );
                assert_eq!(result, [1, 9]);
                assert_eq!(
                    hirsel_rt_v2_task_scope_exit(ctx, 0, 0, &scope, 0),
                    STATUS_OK
                );
                role::enter(ctx);
                engine::at_head(ctx).unwrap();
                let task = engine::current(ctx);
                crate::tasks::ordered::engine::with_driver(ctx, |d| d.finishing(task));
                role::release(ctx);
                STATUS_OK
            },
        );
        assert_eq!(outcome.ended, Ok(STATUS_OK));
        assert_eq!(outcome.pending, 0);
    }

    #[test]
    fn receiver_leftovers_and_last_mutex_roots_run_yielding_finalizers_at_the_head() {
        for width in [2, 4, 8] {
            run_drops(width);
        }
    }

    #[test]
    fn nested_shared_root_deaths_credit_the_same_retirement_before_the_next_allocation() {
        let release_inner = step(crate::STEP_RELEASE_ROOT, 0);
        let scalar = crate::Layout {
            flags: crate::LAYOUT_FROZEN,
            ..layout(8)
        };
        let outer = crate::Layout {
            flags: crate::LAYOUT_FROZEN,
            steps: &release_inner,
            step_count: 1,
            field_count: 1,
            ..layout(8)
        };
        let layouts = [&scalar as *const crate::Layout, &outer];
        for width in [2, 4, 8] {
            let mut run = crate::host::BodyRun::new(width, true);
            run.config.layouts = layouts.as_ptr();
            run.config.layout_count = layouts.len() as u32;
            unsafe { run.use_physical_dispatch() };
            let outcome = run.run(
                |ctx| unsafe {
                    let charge = crate::alloc::allocation_charge(ctx, 2, 8, 8);
                    (*ctx).set_budgets(None, Some(2 * charge));
                    engine::enable(ctx, Policy::admit(&Default::default(), Some(2 * charge)));
                    super::super::events::enable(ctx);
                },
                |ctx| unsafe {
                    for iteration in 0..3u64 {
                        let inner = crate::alloc::hirsel_rt_v2_alloc(
                            ctx,
                            2,
                            8,
                            8,
                            crate::alloc::DOMAIN_HEAP,
                            0,
                        );
                        let root = crate::alloc::hirsel_rt_v2_alloc(
                            ctx,
                            3,
                            8,
                            8,
                            crate::alloc::DOMAIN_HEAP,
                            0,
                        );
                        crate::alloc::payload(root)
                            .cast::<*mut Header>()
                            .write(inner);
                        crate::shared::hirsel_rt_v2_freeze(ctx, inner);
                        crate::shared::hirsel_rt_v2_freeze(ctx, root);
                        crate::roots::hirsel_rt_v2_root_release(ctx, root);
                        let _role = role::Guard::new(ctx);
                        assert_eq!(
                            crate::tasks::ordered::engine::with_driver(ctx, |d| d
                                .frontier
                                .as_ref()
                                .unwrap()
                                .settled
                                .level),
                            0
                        );
                        assert_eq!((*ctx).heap_ref().meter.held(), 0);
                        assert_eq!((*ctx).ordered_freed.get(), 0, "credits cannot apply twice");
                        assert!((*ctx).roots.trace_releases.is_empty());
                        let log = engine::driver(ctx).events.as_ref().unwrap();
                        let events = log.canonical().unwrap();
                        let tail = &events[events.len() - 2..];
                        for (row, freeze) in tail.iter().zip([iteration * 2 + 1, iteration * 2]) {
                            assert_eq!(
                                row.item,
                                super::super::events::CanonicalItem::SharedRoot(
                                    crate::ordered::root_ids::SharedRootId {
                                        creator: vec![],
                                        freeze
                                    }
                                )
                            );
                            assert_eq!(
                                row.outcome,
                                super::super::events::CanonicalOutcome::RootReleased { last: true }
                            );
                            assert_eq!(row.ordinal, iteration);
                        }
                        assert_eq!((tail[0].subevent, tail[1].subevent), (0, 1));
                    }
                    let _role = role::Guard::new(ctx);
                    let task = engine::current(ctx);
                    crate::tasks::ordered::engine::with_driver(ctx, |d| d.finishing(task));
                    STATUS_OK
                },
            );
            assert_eq!(outcome.ended, Ok(STATUS_OK));
        }
    }

    #[test]
    fn another_tasks_logged_root_death_credits_before_its_following_allocation_summary() {
        use super::super::{Command, Objects, Outcome};
        use crate::budget::stretch::{Budgets, Settled, Stretch};
        use crate::ordered::{Frontier, Offer};

        let release_inner = step(crate::STEP_RELEASE_ROOT, 0);
        let scalar = crate::Layout {
            flags: crate::LAYOUT_FROZEN,
            ..layout(8)
        };
        let outer = crate::Layout {
            flags: crate::LAYOUT_FROZEN,
            steps: &release_inner,
            step_count: 1,
            field_count: 1,
            ..layout(8)
        };
        let layouts = [&scalar as *const crate::Layout, &outer];
        let mut run = crate::host::BodyRun::new(2, true);
        run.config.layouts = layouts.as_ptr();
        run.config.layout_count = layouts.len() as u32;
        unsafe { run.use_physical_dispatch() };
        let outcome = run.run(
            |ctx| unsafe {
                let charge = crate::alloc::allocation_charge(ctx, 2, 8, 8);
                (*ctx).set_budgets(None, Some(2 * charge));
            },
            |ctx| unsafe {
                let _role = role::Guard::new(ctx);
                let charge = crate::alloc::allocation_charge(ctx, 2, 8, 8);
                let inner =
                    crate::alloc::hirsel_rt_v2_alloc(ctx, 2, 8, 8, crate::alloc::DOMAIN_HEAP, 0);
                let root =
                    crate::alloc::hirsel_rt_v2_alloc(ctx, 3, 8, 8, crate::alloc::DOMAIN_HEAP, 0);
                crate::alloc::payload(root)
                    .cast::<*mut Header>()
                    .write(inner);
                let atomic =
                    scheduler(ctx)
                        .sync
                        .create_image((&0u64 as *const u64).cast(), 8, 0, 8);
                (*ctx).heap_ref().meter.ordered((*ctx).run_ptr());
                let budgets = Budgets {
                    fuel: false,
                    memory: true,
                };
                let empty = || Stretch::new(budgets);
                let mut frontier = Frontier::new(
                    0,
                    Policy::admit(&Default::default(), None),
                    Settled {
                        level: 2 * charge,
                        limit: 2 * charge,
                        fuel: u64::MAX,
                    },
                );
                frontier.enqueue(2, Default::default());
                let mut records = Vec::new();
                let mut record = |task, name, _: &Outcome, _| records.push((task, name));
                let mut poll = Vec::new;
                let mut objects = Objects::new(ctx, &mut record, &mut poll);
                let release = Command::RootRelease { block: root }.operation(Default::default());
                assert_eq!(
                    frontier.offer(2, empty(), release, &mut objects).0,
                    Offer::Logged
                );
                assert!(crate::roots::sole(root), "the logged root remains live");
                let store = || {
                    Command::AtomicStore {
                        word: atomic,
                        image: 7u64.to_ne_bytes().to_vec(),
                    }
                    .operation(Default::default())
                };
                let mut after = empty();
                after.alloc(2 * charge).unwrap();
                frontier.offer(2, after, store(), &mut objects);
                // The physical current task is zero while logical task two's
                // root walk discovers both credits. Neither may attach to the
                // committer's next stretch or be applied twice.
                frontier.apply_head(empty(), store(), &mut objects).unwrap();
                (*ctx).roots.speculative = true;
                assert_eq!(frontier.absorb(&mut objects), Ok(Progress::Absorbed(2)));
                (*ctx).roots.speculative = false;
                assert_eq!(frontier.settled.level, 0);
                assert_eq!((*ctx).ordered_freed.get(), 0);
                frontier.absorb(&mut objects).unwrap();
                assert_eq!(frontier.settled.level, 2 * charge);
                assert_eq!((*ctx).ordered_freed.get(), 0);
                drop(objects);
                assert_eq!(records[1..], [(2, "root.release"), (2, "atomic.store")]);
                STATUS_OK
            },
        );
        assert_eq!(outcome.ended, Ok(STATUS_OK));
    }
}

//! Compact frames leave their stacks under the shared physical dispatcher
//! (resume-dispatch-51). Children run a Rust stand-in for a compiled
//! resumable body through `hirsel_rt_v2_resume_run`; it saves its frame at
//! every boundary. The canonical Ordered trace must equal that of the same
//! children written as straight-line stackful bodies, at widths 1/2/4/8,
//! reorder off/auto and forced migration: a leave adds no language event.
//!
//! The vectors follow Rayon's join tests (`rayon-core/src/join/test.rs:60-109`):
//! work that runs locally or is stolen produces the same result, and a stop
//! raised inside one job still settles the run. Here "stolen" is a compact
//! frame activated on another worker after its own worker took its stack back.

use super::{engine, events, values};
use crate::tasks;
use hirsel_core_contract::handle_scan::{TYPED_HANDLE, WORDS};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

const CHILDREN: u64 = 4;
const BOUNDARIES: u32 = 3;
const FRAME_BYTES: u32 = 32;

/// What the dispatcher and the dispatcher's compact hook saw.
#[derive(Default)]
struct Log {
    /// Per child: each dispatcher entry's state, context and worker.
    entries: BTreeMap<u64, Vec<(u32, usize, Option<u16>)>>,
    /// Per child: each save's next state and worker.
    saves: BTreeMap<u64, Vec<(u32, Option<u16>)>>,
    /// Per task row: each released stack's context slot, and whether it was pooled.
    released: BTreeMap<u32, Vec<(usize, bool)>>,
    /// Task row of each child.
    rows: BTreeMap<u64, u32>,
    /// The child that requests a stop at this save, if any.
    stop_at: Option<(u64, u32)>,
}

fn handle(offset: u64) -> [u64; WORDS] {
    let mut word = [0; WORDS];
    word[..4].copy_from_slice(&[
        TYPED_HANDLE,
        offset,
        crate::ordered::objects::Kind::Atomic.code(),
        2,
    ]);
    word
}

fn layout(size: u64, scan: &[u64]) -> crate::Layout {
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
        scan: if scan.is_empty() {
            std::ptr::null()
        } else {
            scan.as_ptr()
        },
        scan_words: scan.len() as u64,
    }
}

unsafe fn install_result(ctx: *mut crate::TaskCtx) {
    use values::{Field, Node, Program, Tag};
    unsafe {
        events::install_value(
            ctx,
            3,
            Program::new(
                vec![
                    Node::Variant {
                        extent: 16,
                        tag: Tag::Ordinal { offset: 0 },
                        arms: vec![vec![], vec![Field { offset: 8, node: 1 }]],
                    },
                    Node::Bits(8),
                ],
                0,
            )
            .unwrap(),
        )
    };
}

unsafe fn environment(environment: *mut crate::Header) -> [u64; 4] {
    unsafe { crate::alloc::payload(environment).cast::<[u64; 4]>().read() }
}

/// Boundary `k` of a child: a store and a load of its own atomic.
unsafe fn boundary(ctx: *mut crate::TaskCtx, own: u64, identity: u64, k: u32) {
    let value = identity * 100 + u64::from(k);
    let mut seen = 0u64;
    unsafe {
        tasks::hirsel_rt_v2_atomic_store(ctx, &own, (&raw const value).cast(), 8);
        tasks::hirsel_rt_v2_atomic_load(ctx, &own, (&raw mut seen).cast(), 8);
    }
    assert_eq!(seen, value);
}

/// The child's last operations: the shared atomic, then its result.
unsafe fn finish(ctx: *mut crate::TaskCtx, shared: u64, identity: u64) -> u8 {
    let mut observed = 0u64;
    unsafe {
        if identity.is_multiple_of(2) {
            tasks::hirsel_rt_v2_atomic_store(ctx, &shared, (&raw const identity).cast(), 8);
        } else {
            tasks::hirsel_rt_v2_atomic_load(ctx, &shared, (&raw mut observed).cast(), 8);
        }
        let mut result = [1u64, identity * 1000 + observed];
        tasks::hirsel_rt_v2_task_finish(
            ctx,
            result.as_mut_ptr().cast(),
            [0u64; 2].as_ptr().cast(),
            16,
            3,
        )
    }
}

/// The straight-line stackful reference.
unsafe extern "C" fn stackful(
    ctx: *mut crate::TaskCtx,
    environment: *mut crate::Header,
    _: u64,
    _: u64,
) -> u8 {
    let [own, shared, identity, _] = unsafe { self::environment(environment) };
    for k in 1..=BOUNDARIES {
        unsafe { boundary(ctx, own, identity, k) };
    }
    unsafe { finish(ctx, shared, identity) }
}

/// The resumable body's ordinary entry, as both backends emit it.
unsafe extern "C" fn resumable(
    ctx: *mut crate::TaskCtx,
    environment: *mut crate::Header,
    caller: u64,
    site: u64,
) -> u8 {
    unsafe {
        crate::continuation::hirsel_rt_v2_resume_run(
            ctx,
            environment,
            caller,
            site,
            dispatcher,
            FRAME_BYTES,
        )
    }
}

/// The body's dispatcher. State k reissues boundary k and nothing before it,
/// then saves at the next boundary; the frame's live word counts the
/// boundaries already done.
unsafe extern "C" fn dispatcher(
    ctx: *mut crate::TaskCtx,
    environment: *mut crate::Header,
    _: u64,
    _: u64,
    frame: *mut u8,
) -> u8 {
    let [own, shared, identity, log] = unsafe { self::environment(environment) };
    let log = unsafe { &*(log as *const Mutex<Log>) };
    let header = frame.cast::<u32>();
    let live = unsafe { frame.add(8).cast::<u64>() };
    let mut state = unsafe { header.read() };
    {
        let mut log = log.lock().unwrap();
        log.entries.entry(identity).or_default().push((
            state,
            ctx as usize,
            crate::pool::compute_index(),
        ));
        log.rows.insert(identity, unsafe { (*ctx).task_id });
    }
    unsafe {
        if state == 0 {
            live.write(identity * 7);
        }
        assert_eq!(
            live.read(),
            identity * 7 + u64::from(state.saturating_sub(1))
        );
        if state != 0 {
            boundary(ctx, own, identity, state);
            live.write(live.read() + 1);
        }
        if state == BOUNDARIES {
            return finish(ctx, shared, identity);
        }
        state += 1;
        header.write(state);
        let mut log = log.lock().unwrap();
        log.saves
            .entry(identity)
            .or_default()
            .push((state, crate::pool::compute_index()));
        if log.stop_at == Some((identity, state)) {
            (*ctx)
                .run_ref()
                .control
                .request(crate::stop::Stop::Requested);
        }
        crate::STATUS_SUSPENDED
    }
}

struct Cell {
    width: u32,
    reorder: hirsel_run_config::Reorder,
    migrate: bool,
    compact: bool,
    stop_at: Option<(u64, u32)>,
}

/// Run `CHILDREN` children in one scope; the outcome, the canonical trace and the log.
fn run(
    cell: &Cell,
) -> (
    crate::host::BodyOutcome,
    Result<events::Trace, &'static str>,
    Arc<Mutex<Log>>,
) {
    let scan: Vec<u64> = [handle(0), handle(8)].concat();
    let environment = layout(32, &scan);
    let result = layout(16, &[]);
    let layouts = [&environment as *const crate::Layout, &result];
    let log = Arc::new(Mutex::new(Log {
        stop_at: cell.stop_at,
        ..Log::default()
    }));
    let address = Arc::as_ptr(&log) as u64;
    let observer = log.clone();
    let mut run = crate::host::BodyRun::new(cell.width, cell.migrate);
    run.config.layouts = layouts.as_ptr();
    run.config.layout_count = 2;
    unsafe { run.use_physical_dispatch() };
    let (reorder, code): (_, tasks::TaskCode) = (
        cell.reorder,
        if cell.compact { resumable } else { stackful },
    );
    let (outcome, trace) = run.run_traced(
        move |ctx| unsafe {
            let mut policy = crate::ordered::Policy::admit(&Default::default(), None);
            policy.reorder = reorder;
            engine::enable(ctx, policy);
            events::enable(ctx);
            install_result(ctx);
        },
        move |ctx| unsafe {
            let observer = observer.clone();
            crate::pool::free::test_after_compact(ctx, move |task, slot, pooled| {
                observer
                    .lock()
                    .unwrap()
                    .released
                    .entry(task)
                    .or_default()
                    .push((slot, pooled));
            });
            let mut scope = 0;
            tasks::hirsel_rt_v2_task_scope_enter(ctx, std::ptr::null_mut(), &mut scope, &0);
            let mut atomics = [0u64; CHILDREN as usize + 1];
            for atomic in &mut atomics {
                tasks::hirsel_rt_v2_atomic_create(
                    ctx,
                    std::ptr::null_mut(),
                    atomic,
                    (&0u64 as *const u64).cast(),
                    8,
                    0,
                    8,
                );
            }
            for identity in 0..CHILDREN {
                let environment =
                    crate::alloc::hirsel_rt_v2_alloc(ctx, 2, 32, 8, crate::alloc::DOMAIN_HEAP, 0);
                crate::alloc::payload(environment)
                    .cast::<[u64; 4]>()
                    .write([
                        atomics[identity as usize],
                        atomics[CHILDREN as usize],
                        identity,
                        address,
                    ]);
                let mut task = 0;
                tasks::hirsel_rt_v2_task_spawn(
                    ctx,
                    0,
                    0,
                    std::ptr::null_mut(),
                    &mut task,
                    &scope,
                    code,
                    environment,
                    0,
                );
            }
            tasks::hirsel_rt_v2_task_scope_exit(ctx, 0, 0, &scope, 0);
            for (identity, atomic) in atomics[..CHILDREN as usize].iter().enumerate() {
                let mut value = 0u64;
                tasks::hirsel_rt_v2_atomic_load(ctx, atomic, (&raw mut value).cast(), 8);
                assert_eq!(value, identity as u64 * 100 + u64::from(BOUNDARIES));
            }
            crate::STATUS_OK
        },
    );
    (outcome, trace, log)
}

/// Every child entered its body once and each saved state exactly once, in
/// order: no restart, no double activation. Each save released the stack the
/// frame left, and its first release was the task's own first stack.
fn exact_custody(log: &Log, cell: &str, complete: bool) {
    if complete {
        assert_eq!(log.entries.len() as u64, CHILDREN, "{cell}");
    }
    for (identity, entries) in &log.entries {
        let states: Vec<u32> = entries.iter().map(|entry| entry.0).collect();
        let saves: Vec<u32> = log
            .saves
            .get(identity)
            .map(|saves| saves.iter().map(|save| save.0).collect())
            .unwrap_or_default();
        let expected: Vec<u32> = (0..states.len() as u32).collect();
        assert_eq!(states, expected, "{cell} child {identity}");
        if complete {
            assert_eq!(
                states.len() as u32,
                BOUNDARIES + 1,
                "{cell} child {identity}"
            );
        }
        let resumed: Vec<u32> = states[1..].to_vec();
        assert_eq!(
            saves[..resumed.len()],
            resumed[..],
            "{cell} child {identity}: each saved state resumed once"
        );
        let first = entries[0].1;
        let homes: Vec<usize> = entries[1..].iter().map(|entry| entry.1).collect();
        assert!(
            homes.iter().all(|&home| home != first && home == homes[0]),
            "{cell} child {identity}: the context left its first stack for one home"
        );
        let released = log
            .released
            .get(&log.rows[identity])
            .cloned()
            .unwrap_or_default();
        assert!(
            released.len() >= resumed.len(),
            "{cell} child {identity}: {released:?}"
        );
        if let Some(&(slot, _)) = released.first() {
            assert_eq!(slot, first, "{cell} child {identity}: its first stack");
        }
        assert!(released.iter().all(|&(_, pooled)| pooled), "{cell}");
    }
}

#[test]
fn runtime50_compact_frames_leave_their_stacks_and_agree_with_stackful_bodies() {
    use hirsel_run_config::Reorder;
    let mut reference: Option<Vec<u8>> = None;
    let mut cells = 0;
    let mut moved = 0;
    for width in crate::ordered::runtime50_model::WIDTHS {
        for reorder in [Reorder::Off, Reorder::Auto] {
            for migrate in [false, true] {
                for compact in [false, true] {
                    let cell = Cell {
                        width: width as u32,
                        reorder,
                        migrate,
                        compact,
                        stop_at: None,
                    };
                    let name =
                        format!("width {width} {reorder:?} migrate {migrate} compact {compact}");
                    let (outcome, trace, log) = run(&cell);
                    cells += 1;
                    assert_eq!(outcome.ended, Ok(crate::STATUS_OK), "{name}");
                    assert_eq!(outcome.pending, 0, "{name}");
                    let trace = trace.unwrap_or_else(|error| panic!("{name}: {error}"));
                    match &reference {
                        Some(reference) => assert_eq!(trace.bytes(), &reference[..], "{name}"),
                        None => reference = Some(trace.bytes().to_vec()),
                    }
                    let log = log.lock().unwrap();
                    if !compact {
                        assert!(log.entries.is_empty() && log.released.is_empty(), "{name}");
                        continue;
                    }
                    exact_custody(&log, &name, true);
                    if migrate && width > 1 {
                        // A frame saved on one worker activates on another.
                        for identity in 0..CHILDREN {
                            for (save, entry) in log.saves[&identity]
                                .iter()
                                .zip(&log.entries[&identity][1..])
                            {
                                assert_eq!(save.0, entry.0, "{name}");
                                assert_ne!(save.1, entry.2, "{name} child {identity}");
                                moved += 1;
                            }
                        }
                    }
                }
            }
        }
    }
    assert_eq!(cells, 32);
    // Widths 2, 4 and 8 under both reorders.
    assert_eq!(moved, 6 * CHILDREN * u64::from(BOUNDARIES));
}

#[test]
fn runtime50_compact_frame_stopped_while_saved_is_never_activated() {
    use hirsel_run_config::Reorder;
    for width in [1, 4] {
        let cell = Cell {
            width,
            reorder: Reorder::Auto,
            migrate: true,
            compact: true,
            stop_at: Some((1, 2)),
        };
        let (outcome, _, log) = run(&cell);
        assert_eq!(
            outcome.ended,
            Err(crate::stop::Stop::Requested),
            "width {width}"
        );
        let log = log.lock().unwrap();
        // The stopping child saved state 2 and was never activated again.
        let states: Vec<u32> = log.entries[&1].iter().map(|entry| entry.0).collect();
        assert_eq!(states, [0, 1], "width {width}");
        exact_custody(&log, &format!("stop width {width}"), false);
    }
}

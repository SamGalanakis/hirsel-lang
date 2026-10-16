//! Shared helpers for the runtime's integration tests: a per-thread counting host allocator,
//! a context guard, descriptor builders, a finalizer event log and abort probes.
#![allow(dead_code)]

pub mod io;
pub mod schema;

use std::alloc::{GlobalAlloc, Layout as HostLayout, System};
use std::cell::{Cell, RefCell};
use std::os::unix::process::ExitStatusExt;
use std::process::Command;
use std::ptr;

use hirsel_rt::context::{
    CONFIG_DIAGNOSTIC, hirsel_rt_v2_ctx_create, hirsel_rt_v2_ctx_destroy, hirsel_rt_v2_ctx_stats,
};
use hirsel_rt::failure::hirsel_rt_v2_failure_begin;
use hirsel_rt::{
    Config, DropThunk, Header, LAYOUT_MAGIC, LAYOUT_VERSION, Layout, NO_OFFSET, RunCtx,
    STEP_FINALIZE, STEP_LOOP, STEP_RELEASE, STEP_TAG, Stats, Step, WHOLE_VALUE,
};

// Test fixtures construct their complete descriptors before the first indexed operation.
// Binding once at that seam lets the older fixture construction order keep an immutable
// program table: no descriptor may be added after the first allocation.
struct FixtureBinding {
    config: Config,
    layouts: Option<Vec<*const Layout>>,
}
thread_local! {
    static CATALOG: RefCell<Vec<*const Layout>> = const { RefCell::new(Vec::new()) };
    static BINDINGS: RefCell<std::collections::BTreeMap<usize, FixtureBinding>> = RefCell::new(std::collections::BTreeMap::new());
}

pub fn cached_layout(
    key: &'static str,
    discriminator: u64,
    build: impl FnOnce() -> &'static Layout,
) -> &'static Layout {
    thread_local! { static CACHE: RefCell<std::collections::BTreeMap<(&'static str, u64), &'static Layout>> = const { RefCell::new(std::collections::BTreeMap::new()) }; }
    let layout = CACHE.with(|cache| {
        *cache
            .borrow_mut()
            .entry((key, discriminator))
            .or_insert_with(build)
    });
    register_fixture(layout);
    layout
}

pub fn register_fixture(layout: *const Layout) {
    CATALOG.with(|catalog| {
        let mut catalog = catalog.borrow_mut();
        if !catalog.contains(&layout) {
            catalog.push(layout);
        }
    });
}

pub fn index(ctx: *mut RunCtx, layout: *const Layout) -> u32 {
    BINDINGS.with(|bindings| {
        let mut bindings = bindings.borrow_mut();
        let binding = bindings.get_mut(&(ctx as usize)).expect("fixture context");
        if binding.layouts.is_none() {
            let mut layouts = CATALOG.with(|catalog| catalog.borrow().clone());
            if !layouts.contains(&layout) {
                layouts.push(layout);
            }
            let config = Config {
                layouts: layouts.as_ptr(),
                layout_count: layouts.len() as u32,
                ..binding.config
            };
            let mut replacement = ptr::null_mut();
            assert_eq!(
                unsafe { hirsel_rt_v2_ctx_create(&config, &mut replacement) },
                0
            );
            let mut stats = Stats::default();
            unsafe {
                hirsel_rt_v2_ctx_stats(ctx, &mut stats);
            }
            assert_eq!(
                stats.live_blocks, 0,
                "fixture binding must precede allocations"
            );
            unsafe {
                ptr::swap(ctx, replacement);
                hirsel_rt_v2_ctx_destroy(replacement);
            }
            binding.layouts = Some(layouts);
        }
        let layouts = binding.layouts.as_ref().unwrap();
        let position = layouts
            .iter()
            .position(|p| *p == layout)
            .expect("declare the complete fixture descriptors before its first allocation");
        position as u32 + 2
    })
}

/// Counts this thread's host allocations so tests can prove what the runtime returned.
pub struct Counting;

thread_local! {
    static OUTSTANDING: Cell<i64> = const { Cell::new(0) };
    static CALLS: Cell<u64> = const { Cell::new(0) };
}

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: HostLayout) -> *mut u8 {
        let _ = OUTSTANDING.try_with(|bytes| bytes.set(bytes.get() + layout.size() as i64));
        let _ = CALLS.try_with(|calls| calls.set(calls.get() + 1));
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, block: *mut u8, layout: HostLayout) {
        let _ = OUTSTANDING.try_with(|bytes| bytes.set(bytes.get() - layout.size() as i64));
        unsafe { System.dealloc(block, layout) }
    }
}

#[global_allocator]
static COUNTING: Counting = Counting;

/// Host bytes this thread has allocated and not freed.
pub fn host_outstanding() -> i64 {
    OUTSTANDING.with(Cell::get)
}

/// Host allocation calls this thread has made.
pub fn host_calls() -> u64 {
    CALLS.with(Cell::get)
}

/// A run context destroyed on drop.
pub struct Ctx(pub *mut RunCtx);

impl Ctx {
    pub fn with(config: Config) -> Ctx {
        let mut ctx = ptr::null_mut();
        let status = unsafe { hirsel_rt_v2_ctx_create(&config, &mut ctx) };
        assert_eq!(status, 0, "context creation failed");
        BINDINGS.with(|bindings| {
            bindings.borrow_mut().insert(
                ctx as usize,
                FixtureBinding {
                    config,
                    layouts: None,
                },
            );
        });
        Ctx(ctx)
    }

    pub fn production() -> Ctx {
        Ctx::with(Config::new())
    }

    pub fn diagnostic() -> Ctx {
        Ctx::with(Config {
            flags: CONFIG_DIAGNOSTIC,
            ..Config::new()
        })
    }

    pub fn stats(&self) -> Stats {
        let mut stats = Stats::default();
        unsafe { hirsel_rt_v2_ctx_stats(self.0, &mut stats) };
        stats
    }
}

impl Drop for Ctx {
    fn drop(&mut self) {
        BINDINGS.with(|bindings| {
            let mut bindings = bindings.borrow_mut();
            bindings.remove(&(self.0 as usize));
            if bindings.is_empty() {
                *bindings = std::collections::BTreeMap::new();
            }
        });
        CATALOG.with(|catalog| catalog.borrow_mut().clear());
        unsafe { hirsel_rt_v2_ctx_destroy(self.0) };
    }
}

pub fn leak<T: 'static>(value: T) -> &'static T {
    let value = Box::leak(Box::new(value));
    if let Some(layout) = (value as &dyn std::any::Any).downcast_ref::<Layout>() {
        register_fixture(layout);
    }
    value
}

pub fn step(kind: u32, field: u32, offset: u64) -> Step {
    Step {
        kind,
        field,
        variant: 0,
        span: 0,
        offset,
        stride: 0,
        count: 0,
        count_offset: NO_OFFSET,
        layout: ptr::null(),
    }
}

pub fn release_step(field: u32, offset: u64) -> Step {
    step(STEP_RELEASE, field, offset)
}

pub fn finalize_step(field: u32, offset: u64, layout: &'static Layout) -> Step {
    Step {
        layout,
        ..step(STEP_FINALIZE, field, offset)
    }
}

/// Skip the next `span` steps unless the `u32` tag at `offset` is `variant`.
pub fn tag_step(offset: u64, variant: u32, span: u32) -> Step {
    Step {
        variant,
        span,
        ..step(STEP_TAG, WHOLE_VALUE, offset)
    }
}

/// A loop over `count` static elements at `offset`, `stride` apart, owning the next `span` steps.
pub fn repeat_step(field: u32, offset: u64, count: u64, stride: u64, span: u32) -> Step {
    Step {
        count,
        stride,
        span,
        ..step(STEP_LOOP, field, offset)
    }
}

/// A loop over the `u64` count at `count_offset` of elements at `offset`, `stride` apart.
pub fn elements_step(field: u32, count_offset: u64, offset: u64, stride: u64, span: u32) -> Step {
    Step {
        count_offset,
        stride,
        span,
        ..step(STEP_LOOP, field, offset)
    }
}

/// A leaked descriptor with the given drop program, in drop order.
pub fn layout(
    size: u64,
    field_count: u32,
    finalizer: Option<DropThunk>,
    steps: Vec<Step>,
) -> &'static Layout {
    let steps: &'static [Step] = Box::leak(steps.into_boxed_slice());
    leak(Layout {
        magic: LAYOUT_MAGIC,
        version: LAYOUT_VERSION,
        size,
        align: 8,
        finalizer,
        steps: steps.as_ptr(),
        step_count: steps.len() as u64,
        field_count,
        flags: if steps
            .iter()
            .any(|s| matches!(s.kind, hirsel_rt::STEP_LOOP | hirsel_rt::STEP_COLUMN))
        {
            hirsel_rt::release::LAYOUT_WIDE_CURSOR
        } else {
            0
        },
    })
}

pub fn leaf(size: u64) -> &'static Layout {
    layout(size, 0, None, Vec::new())
}

pub fn with_flags(layout: &'static Layout, flags: u32) -> &'static Layout {
    leak(Layout { flags, ..*layout })
}

/// A fresh heap block of `extent` bytes, aligned to 8, whose header names `layout`.
pub fn alloc_block(ctx: *mut RunCtx, layout: &Layout, extent: u64) -> *mut Header {
    unsafe {
        hirsel_rt::alloc::hirsel_rt_v2_alloc(
            ctx,
            index(ctx, layout),
            extent,
            8,
            hirsel_rt::DOMAIN_HEAP,
            0,
        )
    }
}

pub fn payload(block: *mut Header) -> *mut u8 {
    hirsel_rt::alloc::hirsel_rt_v2_payload(block)
}

pub fn write_u64(block: *mut Header, offset: usize, value: u64) {
    unsafe {
        payload(block)
            .add(offset)
            .cast::<u64>()
            .write_unaligned(value)
    };
}

pub fn read_u64(block: *mut Header, offset: usize) -> u64 {
    unsafe { payload(block).add(offset).cast::<u64>().read_unaligned() }
}

pub fn write_ref(block: *mut Header, offset: usize, child: *mut Header) {
    unsafe {
        payload(block)
            .add(offset)
            .cast::<*mut Header>()
            .write_unaligned(child)
    };
}

pub fn state(ctx: *mut RunCtx, block: *mut Header) -> u8 {
    unsafe { (*ctx).diagnostic_state(block) }
}

pub fn descriptor(ctx: *mut RunCtx, block: *mut Header) -> *const Layout {
    let index = unsafe { (*block).index() };
    if index == 0 {
        return &hirsel_rt::text::hirsel_rt_v2_bytes_descriptor.0;
    }
    if index == 1 {
        return &hirsel_rt::io::hirsel_rt_v2_file_walk_descriptor.0;
    }
    BINDINGS.with(|bindings| {
        bindings
            .borrow()
            .get(&(ctx as usize))
            .unwrap()
            .layouts
            .as_ref()
            .unwrap()[(index - 2) as usize]
    })
}

pub fn count(block: *mut Header) -> u32 {
    unsafe { (*block).count }
}

thread_local! {
    static EVENTS: RefCell<Vec<(u64, u64)>> = const { RefCell::new(Vec::new()) };
}

/// Take the finalizer events logged on this thread: `(value id, anchor)` in call order.
pub fn take_events() -> Vec<(u64, u64)> {
    EVENTS.with(|events| std::mem::take(&mut *events.borrow_mut()))
}

/// A finalizer that logs the `u64` id at the start of its value.
pub unsafe extern "C" fn log_drop(_ctx: *mut RunCtx, value: *mut u8, anchor: u64) -> u8 {
    let id = unsafe { value.cast::<u64>().read_unaligned() };
    EVENTS.with(|events| events.borrow_mut().push((id, anchor)));
    0
}

/// A finalizer that logs its id and then fails with rule code = id.
pub unsafe extern "C" fn failing_drop(ctx: *mut RunCtx, value: *mut u8, anchor: u64) -> u8 {
    let id = unsafe { value.cast::<u64>().read_unaligned() };
    EVENTS.with(|events| events.borrow_mut().push((id, anchor)));
    unsafe { hirsel_rt_v2_failure_begin(ctx, 1, id as u32, anchor) }
}

/// Whether this process is the child run of `test`.
pub fn in_child(test: &str) -> bool {
    std::env::var("HIRSEL_RT_CHILD").as_deref() == Ok(test)
}

/// Run `test` alone in a child process and require that it aborts (SIGABRT) with `needle` on
/// stderr. Each test file is a module of the crate's one test binary, so the child selects
/// `<calling file's stem>::test`.
#[track_caller]
pub fn assert_child_aborts(test: &str, needle: &str) {
    let file = std::path::Path::new(std::panic::Location::caller().file());
    let module = file.file_stem().unwrap().to_str().unwrap();
    let output = Command::new(std::env::current_exe().unwrap())
        .arg(format!("{module}::{test}"))
        .args(["--exact", "--nocapture", "--test-threads=1"])
        .env("HIRSEL_RT_CHILD", test)
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(
        output.status.signal(),
        Some(6),
        "{test} did not abort: {:?}\n{stderr}",
        output.status
    );
    assert!(
        stderr.contains(needle),
        "{test} aborted without `{needle}`:\n{stderr}"
    );
}

pub const UNUSED: u32 = WHOLE_VALUE;

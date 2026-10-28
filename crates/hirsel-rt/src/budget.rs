//! Run budgets natively (`budgets.limits@2`, docs/spec/determinism.md "Budgets").
//!
//! **Fuel** is the `Compiled::fuel` word, the running task's chunk: a fuel build's code
//! charges it at each fuel point (`hirsel_lir::fuel`), and a negative result calls
//! [`hirsel_rt_v2_fuel_refill`], which takes the next chunk from the run's [`FuelPool`] and
//! returns, or stops the run when the pool is spent. A run with a fuel budget at width 1 takes
//! one chunk, the whole budget; without one, at width > 1 or under a host's stop handle,
//! chunks are [`FUEL_CHUNK`] points, so a fuel build enters the runtime at least that often
//! and observes a stop there. Exhaustion is exact either way: the last chunks shrink to what
//! is left.
//!
//! **Memory** is the run's logical storage ([`Meter`]), which depends only on requests and
//! logical lifetimes: a small block is charged its size class ([`small_charge`]), a large one
//! its request rounded up to 4 KiB ([`large_charge`]), and a region segment its canonical size
//! (the one an empty pool would give the region's request). A charge happens when a task
//! allocates and a credit when the block dies, on the task that frees it. Idle slabs, pooled
//! segments, run tables, scratch, stacks and immortal blocks are never charged. Under a memory
//! budget compiled code takes no inline allocation or free (`Heap::set_metered`), so every
//! charge and credit passes the meter. The first charge that exceeds the budget stops the run
//! with `Stop::BudgetExceeded`, as a Blocked run stops (`crate::stop`).
//!
//! An ordered executor settles non-head turns' charges at retirement from their summaries
//! ([`stretch`]); at width 1 every turn is the head, and the meter is checked live.
use crate::TaskCtx;
use crate::alloc::HEADER_SIZE;
use hirsel_run_schema::BudgetKind;
use std::sync::atomic::{AtomicBool, AtomicPtr, AtomicU64, Ordering};

pub mod stretch;
pub(crate) mod stretch_trace;

/// Large blocks are charged their request rounded up to this.
pub const LARGE_GRANULE: u64 = 4096;
/// The most fuel points one chunk holds.
pub const FUEL_CHUNK: u64 = 64 * 1024;

/// The logical charge of a small block of `capacity` payload bytes (its class capacity).
#[inline]
pub(crate) fn small_charge(capacity: u64) -> u64 {
    HEADER_SIZE as u64 + capacity
}
/// The logical charge of a large block of `capacity` payload bytes.
#[inline]
pub(crate) fn large_charge(capacity: u64) -> u64 {
    (HEADER_SIZE as u64)
        .saturating_add(capacity)
        .next_multiple_of(LARGE_GRANULE)
}

/// The run's logical storage against its `memory_bytes` budget; a zero limit is no budget.
/// Its counters move only under a budget, so an unbudgeted run pays nothing for them.
#[derive(Debug, Default)]
pub(crate) struct Meter {
    pub(crate) cpu: std::sync::OnceLock<std::sync::Arc<crate::pool::cpu_scoped::ledger::Ledger>>,
    /// Ordered charges were checked before borrowing the heap. Callbacks may
    /// validate a charge or record a free, but never suspend or stop here.
    ordered: AtomicPtr<crate::context::RunShared>,
    limit: AtomicU64,
    held: AtomicU64,
    /// Charges and credits applied (headroom metric `meter_ops_per_s`).
    ops: AtomicU64,
    /// Heap allocations charged (headroom metric `allocations_per_s`).
    allocations: AtomicU64,
}
impl Meter {
    pub(crate) fn ordered(&self, run: *mut crate::context::RunShared) {
        self.ordered.store(run, Ordering::Release);
    }
    pub(crate) fn limit(&self) -> u64 {
        self.limit.load(Ordering::Relaxed)
    }
    pub(crate) fn settled(&self, bytes: u64) {
        self.held.store(bytes, Ordering::Relaxed);
    }
    fn ordered_charge(&self, bytes: u64) -> bool {
        let run = self.ordered.load(Ordering::Acquire);
        if run.is_null() {
            return false;
        }
        let ctx = crate::stop::running();
        if !ctx.is_null() && unsafe { (*ctx).run_ptr() } == run {
            crate::tasks::ordered::engine::memory_charged(ctx, bytes);
        }
        self.ops.fetch_add(1, Ordering::Relaxed);
        true
    }
    pub(crate) fn set_limit(&self, limit: u64) {
        self.limit.store(limit, Ordering::Relaxed);
    }
    #[inline]
    pub(crate) fn metered(&self) -> bool {
        self.limit.load(Ordering::Relaxed) != 0
    }
    /// The logical bytes live now.
    pub(crate) fn held(&self) -> u64 {
        self.held.load(Ordering::Relaxed)
    }
    /// Charge `bytes` a task allocates, or end the run when they exceed the budget: the charge
    /// is not applied, so the stopped run's level is the last one within it.
    #[inline]
    pub(crate) fn charge(&self, bytes: u64) {
        if self.metered() {
            if !self.ordered_charge(bytes) {
                self.charge_metered(bytes);
            }
        }
    }
    #[cold]
    #[inline(never)]
    fn charge_metered(&self, bytes: u64) {
        let limit = self.limit.load(Ordering::Relaxed);
        let mut held = self.held.load(Ordering::Relaxed);
        loop {
            let next = held.saturating_add(bytes);
            if next > limit {
                exceeded(BudgetKind::Memory);
            }
            match self
                .held
                .compare_exchange_weak(held, next, Ordering::Relaxed, Ordering::Relaxed)
            {
                Ok(_) => break,
                Err(current) => held = current,
            }
        }
        self.ops.fetch_add(1, Ordering::Relaxed);
    }
    /// Charge a heap allocation of `bytes`.
    #[inline]
    pub(crate) fn charge_allocation(&self, bytes: u64) {
        if self.metered() {
            if !self.ordered_charge(bytes) {
                self.charge_metered(bytes);
            }
            self.allocations.fetch_add(1, Ordering::Relaxed);
        }
    }
    /// Balanced private CPU reuse cycles keep their admitted storage charge.
    /// Fold their telemetry at the same boundary as the arena's final frees;
    /// no policy, charge witness or budget state changes here.
    pub(crate) fn cpu_reuses(&self, count: u64) {
        if self.metered() && count != 0 {
            self.allocations.fetch_add(count, Ordering::Relaxed);
            self.ops.fetch_add(count.wrapping_mul(2), Ordering::Relaxed);
        }
    }
    /// Credit `bytes` whose logical lifetime ended.
    #[inline]
    pub(crate) fn credit(&self, bytes: u64) {
        if self.metered() {
            let run = self.ordered.load(Ordering::Acquire);
            if !run.is_null() {
                let ctx = crate::stop::running();
                if !ctx.is_null() && unsafe { (*ctx).run_ptr() } == run {
                    crate::tasks::ordered::engine::memory_freed(ctx, bytes);
                }
                self.ops.fetch_add(1, Ordering::Relaxed);
                return;
            }
            let previous = self.held.fetch_sub(bytes, Ordering::Relaxed);
            if previous < bytes {
                crate::fault("the memory meter credited more than it charged");
            }
            self.ops.fetch_add(1, Ordering::Relaxed);
        }
    }
    /// Forget every charge: task exit returned all of the run's storage.
    pub(crate) fn clear(&self) {
        self.held.store(0, Ordering::Relaxed);
    }
}

/// The fuel a run has not handed out as chunks yet.
#[derive(Debug)]
pub(crate) struct FuelPool {
    /// Points left to hand out; `u64::MAX` without a fuel budget, which never decreases.
    left: AtomicU64,
    active: AtomicBool,
    /// Whether the pool hands out its whole remainder as one chunk: a budgeted run at width 1,
    /// whose exhaustion is then exact.
    whole: bool,
    /// Chunks handed out (headroom metric `fuel_refills`).
    refills: AtomicU64,
}
impl Default for FuelPool {
    fn default() -> Self {
        FuelPool {
            left: AtomicU64::new(u64::MAX),
            active: AtomicBool::new(false),
            whole: false,
            refills: AtomicU64::new(0),
        }
    }
}
impl FuelPool {
    /// A pool of `fuel` points, or an unbounded one; `whole` hands out the remainder at once.
    pub(crate) fn new(fuel: Option<u64>, whole: bool) -> FuelPool {
        FuelPool {
            left: AtomicU64::new(fuel.unwrap_or(u64::MAX)),
            active: AtomicBool::new(true),
            whole: whole && fuel.is_some(),
            refills: AtomicU64::new(0),
        }
    }
    /// The next chunk, or 0 when the pool is spent. Chunks shrink as the pool empties: none is
    /// larger than [`FUEL_CHUNK`] (unless the pool hands out its whole remainder), and near the
    /// end they shrink to what is left.
    pub(crate) fn active(&self) -> bool {
        self.active.load(Ordering::Relaxed)
    }
    /// Admission snapshot before an ordered driver leases any compiled fuel.
    pub(crate) fn remaining(&self) -> u64 {
        self.left.load(Ordering::Relaxed)
    }
    /// A task that stops running returns only its own unspent points. The
    /// release/acquire run-token handoff publishes its updated fuel word.
    pub(crate) fn put(&self, points: u64) {
        if points == 0 {
            return;
        }
        if self.left.load(Ordering::Relaxed) != u64::MAX {
            self.left.fetch_add(points, Ordering::Relaxed);
        }
    }
    pub(crate) fn take(&self) -> u64 {
        self.active.store(true, Ordering::Relaxed);
        let mut left = self.left.load(Ordering::Relaxed);
        loop {
            if left == u64::MAX {
                self.refills.fetch_add(1, Ordering::Relaxed);
                return FUEL_CHUNK;
            }
            let chunk = if self.whole {
                left
            } else {
                left.min(FUEL_CHUNK)
            };
            if chunk == 0 {
                return 0;
            }
            match self.left.compare_exchange_weak(
                left,
                left - chunk,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => {
                    self.refills.fetch_add(1, Ordering::Relaxed);
                    return chunk;
                }
                Err(current) => left = current,
            }
        }
    }
}

/// What the run's budget machinery counted (bench/multicore/telemetry.json, R4 rows).
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Counters {
    /// Logical charges and credits applied under a memory budget.
    pub meter_ops: u64,
    /// Heap allocations charged under a memory budget.
    pub allocations: u64,
    /// Fuel chunks handed out.
    pub fuel_refills: u64,
    /// The logical bytes live now.
    pub held: u64,
}

impl TaskCtx {
    /// The run's budget counters.
    pub fn budget_counters(&mut self) -> Counters {
        let meter = &self.heap().meter;
        let (meter_ops, allocations, held) = (
            meter.ops.load(Ordering::Relaxed),
            meter.allocations.load(Ordering::Relaxed),
            meter.held(),
        );
        Counters {
            meter_ops,
            allocations,
            fuel_refills: self.run_ref().fuel.refills.load(Ordering::Relaxed),
            held,
        }
    }
}

/// Stop the running run: a charge found the budget spent.
#[cold]
#[inline(never)]
pub(crate) fn exceeded(kind: BudgetKind) -> ! {
    let ctx = crate::stop::running();
    if ctx.is_null() {
        crate::environmental("a run spent its budget outside its launcher");
    }
    // SAFETY: the running context is live while its run executes.
    unsafe { crate::stop::raise(ctx, crate::stop::Stop::BudgetExceeded(kind)) }
}

/// A fuel point found the running task's chunk spent (its fuel word is negative): store the
/// next chunk, less the point the failed charge spent, into the task's fuel word and return,
/// so the charge's code continues; or, when the run's fuel is spent, end the run with
/// BudgetExceeded (fuel) and never return.
///
/// # Safety
/// `ctx` is the running context.
#[cfg_attr(not(all(feature = "tsan", target_arch = "x86_64")), unsafe(no_mangle))]
pub unsafe extern "C" fn hirsel_rt_v2_fuel_refill(ctx: *mut TaskCtx) {
    unsafe { crate::pool::cpu_scoped::probe::checkpoint(ctx) };
    if crate::tasks::ordered::ahead::private(ctx) {
        unsafe { crate::stop::check(ctx, crate::stop::observe(ctx)) };
        unsafe { crate::tasks::ordered::ahead::refill(ctx) };
        return;
    }
    let result = unsafe { refill(ctx) };
    unsafe { crate::stop::check(ctx, result) };
}

// A duplicate resumes after the refill's Rust frames, at the emitted caller.
#[cfg(all(feature = "tsan", target_arch = "x86_64"))]
#[unsafe(naked)]
#[unsafe(export_name = "hirsel_rt_v2_fuel_refill")]
unsafe extern "C" fn tsan_fuel_refill(_ctx: *mut TaskCtx) {
    core::arch::naked_asm!(
        "push rdi", "call {refill}", "pop rdi",
        "sub rsp, 8", "call {prepare}", "add rsp, 8",
        "test rax, rax", "jz 2f",
        "mov rdi, rax", "mov rsi, rdx", "jmp {switch}",
        "2:", "ret",
        refill = sym hirsel_rt_v2_fuel_refill,
        prepare = sym crate::pool::free::double_claim::ordinary_prepare,
        switch = sym crate::fiber::fiber_switch,
    );
}

/// Charge deterministic primitive work even when the caller omitted loop fuel checks.
/// A checkpoint also observes the embedding host's stop without adding a scheduling point.
/// # Safety
/// `ctx` is the current live task; this helper returns before any stop switches stacks.
pub(crate) unsafe fn charge_primitive(
    ctx: *mut TaskCtx,
    mut cost: u64,
) -> crate::stop::Stopped<()> {
    crate::stop::observe(ctx)?;
    while cost != 0 {
        let available = unsafe { (*ctx).view.compiled.fuel.max(0) as u64 };
        let spent = available.min(cost);
        unsafe { (*ctx).view.compiled.fuel -= spent as i64 };
        cost -= spent;
        if cost != 0 {
            unsafe { (*ctx).view.compiled.fuel = -1 };
            unsafe { refill(ctx) }?;
            cost -= 1; // refill includes the point that crossed the chunk boundary.
        }
    }
    Ok(())
}

unsafe fn refill(ctx: *mut TaskCtx) -> crate::stop::Stopped<()> {
    crate::stop::observe(ctx)?;
    if unsafe { (*ctx).run_ref().ordered_mode.load(Ordering::Acquire) } {
        let _role = unsafe { crate::tasks::role::Guard::new(ctx) };
        return crate::tasks::ordered::engine::fuel_refill(ctx);
    }
    let chunk = unsafe { (*ctx).run_ref().fuel.take() };
    if chunk == 0 {
        return Err(crate::stop::Stop::BudgetExceeded(BudgetKind::Fuel));
    }
    unsafe {
        (*ctx)
            .heap()
            .set_fuel(i64::try_from(chunk - 1).unwrap_or(i64::MAX))
    };
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::alloc::{DOMAIN_HEAP, hirsel_rt_v2_alloc};
    use crate::context::{Config, hirsel_rt_v2_ctx_create, hirsel_rt_v2_ctx_destroy};
    use crate::region::{Region, hirsel_rt_v2_region_enter, hirsel_rt_v2_region_exit};
    use crate::stop::{Stop, run_body};
    use std::cell::Cell;
    use std::mem::MaybeUninit;

    #[test]
    fn a_parked_tasks_unspent_chunk_returns_to_the_pool() {
        let pool = FuelPool::new(Some(FUEL_CHUNK + 7), false);
        assert_eq!(pool.take(), FUEL_CHUNK);
        pool.put(FUEL_CHUNK - 3);
        assert_eq!(pool.take(), FUEL_CHUNK);
        assert_eq!(pool.take(), 4);
        assert_eq!(pool.take(), 0);
        let unbounded = FuelPool::default();
        assert!(!unbounded.active());
        assert_eq!(unbounded.take(), FUEL_CHUNK);
        unbounded.put(FUEL_CHUNK);
        assert_eq!(unbounded.take(), FUEL_CHUNK);
    }

    /// A fresh run context with `fuel` and `memory` budgets, or none.
    fn context(fuel: Option<u64>, memory: Option<u64>) -> *mut TaskCtx {
        let mut ctx = std::ptr::null_mut();
        // SAFETY: valid configuration and out-parameter.
        assert_eq!(
            unsafe { hirsel_rt_v2_ctx_create(&Config::new(), &mut ctx) },
            crate::context::LAUNCH_OK
        );
        if fuel.is_some() || memory.is_some() {
            // SAFETY: the context is live.
            unsafe { (*ctx).set_budgets(fuel, memory) };
        }
        ctx
    }

    /// Run `body` as the run's root; a stop is returned as the run's.
    fn run(ctx: *mut TaskCtx, mut body: impl FnMut()) -> Result<(), Stop> {
        // SAFETY: the context is live and driven once.
        let result = unsafe {
            run_body(ctx, &mut || {
                body();
                0
            })
        };
        if result.is_err() {
            // SAFETY: the stopped run's root.
            unsafe { (*ctx).abandon() };
        }
        result.map(|_| ())
    }

    /// One fuel point as compiled code charges it (`hirsel-llvm` `charge_fuel`).
    fn charge(ctx: *mut TaskCtx) {
        // SAFETY: the running context.
        unsafe {
            let word = &raw mut (*ctx).view.compiled.fuel;
            *word -= 1;
            if *word < 0 {
                hirsel_rt_v2_fuel_refill(ctx);
            }
        }
    }

    #[test]
    fn numeric_work_charges_exact_fuel_before_writing_scratch() {
        // Byte budgets below, at and above every chunk boundary also exercise
        // the shared bulk-charge path without primitive-specific arithmetic.
        for cost in [
            1,
            9,
            FUEL_CHUNK - 1,
            FUEL_CHUNK,
            FUEL_CHUNK + 1,
            FUEL_CHUNK * 3 + 7,
        ] {
            for fuel in [cost - 1, cost, cost + 1] {
                let ctx = context(Some(fuel), None);
                let mut written = false;
                let result = run(ctx, || {
                    let charged = unsafe { charge_primitive(ctx, cost) };
                    unsafe { crate::stop::check(ctx, charged) };
                    written = true;
                });
                assert_eq!(written, fuel >= cost);
                assert_eq!(
                    result,
                    if fuel < cost {
                        Err(Stop::BudgetExceeded(BudgetKind::Fuel))
                    } else {
                        Ok(())
                    }
                );
                unsafe { hirsel_rt_v2_ctx_destroy(ctx) };
            }
        }
        for fuel in [8, 9, 10] {
            let ctx = context(Some(fuel), None);
            let mut buffer = [0xa5; 20];
            let mut count = 0xa5;
            let result = run(ctx, || {
                assert_eq!(
                    unsafe {
                        crate::text::hirsel_rt_v2_bytes_decimal_word(
                            ctx,
                            &mut count,
                            1234567890,
                            &mut buffer,
                        )
                    },
                    crate::STATUS_OK
                );
            });
            if fuel < 9 {
                assert_eq!(result, Err(Stop::BudgetExceeded(BudgetKind::Fuel)));
                assert_eq!(buffer, [0xa5; 20]);
                assert_eq!(count, 0xa5);
            } else {
                assert_eq!(result, Ok(()));
                assert_eq!(count, 10);
                assert_eq!(&buffer[..10], b"1234567890");
                assert_eq!(&buffer[10..], &[0xa5; 10]);
            }
            unsafe { hirsel_rt_v2_ctx_destroy(ctx) };
        }
    }

    #[test]
    fn sha_work_fuel_padding_and_stop_checkpoints_are_exact() {
        use hirsel_kernels::sha::{Algorithm, digest};
        for algorithm in [
            Algorithm::Sha1,
            Algorithm::Sha224,
            Algorithm::Sha256,
            Algorithm::Sha384,
            Algorithm::Sha512,
            Algorithm::Sha512_256,
        ] {
            for length in [0, 55, 56, 63, 64, 119, 128, 8192] {
                let source = vec![42; length];
                let cost = algorithm.charge(length as u64);
                for fuel in [cost - 1, cost, cost + 1] {
                    let ctx = context(Some(fuel), None);
                    let mut completed = false;
                    let result = run(ctx, || {
                        let result =
                            digest(algorithm, &source, |n| unsafe { charge_primitive(ctx, n) });
                        let _output = unsafe { crate::stop::check(ctx, result) };
                        completed = true;
                    });
                    assert_eq!(completed, fuel >= cost, "{algorithm:?} {length} {fuel}");
                    assert_eq!(
                        result,
                        if fuel < cost {
                            Err(Stop::BudgetExceeded(BudgetKind::Fuel))
                        } else {
                            Ok(())
                        }
                    );
                    unsafe { hirsel_rt_v2_ctx_destroy(ctx) };
                }
            }
            let ctx = context(None, None);
            let source = vec![42; 32768];
            let seen = Cell::new(0);
            let result = run(ctx, || {
                let result = digest(algorithm, &source, |cost| {
                    unsafe { charge_primitive(ctx, cost) }?;
                    seen.set(seen.get() + 1);
                    if seen.get() == 2 {
                        unsafe { (*ctx).run_ref().control.request(Stop::Requested) };
                    }
                    Ok::<_, Stop>(())
                });
                let _output = unsafe { crate::stop::check(ctx, result) };
            });
            assert_eq!(result, Err(Stop::Requested));
            assert_eq!(seen.get(), 2, "stop must be observed at next checkpoint");
            unsafe { hirsel_rt_v2_ctx_destroy(ctx) };
        }
    }

    /// At width 1 a budget of F points allows exactly F charges: the run stops at charge
    /// F + 1, at every size, below, at and above one chunk.
    #[test]
    fn width_one_fuel_exhaustion_is_exact() {
        for fuel in [
            1,
            2,
            7,
            FUEL_CHUNK - 1,
            FUEL_CHUNK,
            FUEL_CHUNK + 1,
            3 * FUEL_CHUNK + 5,
        ] {
            let ctx = context(Some(fuel), None);
            let charged = Cell::new(0u64);
            let result = run(ctx, || {
                loop {
                    charge(ctx);
                    charged.set(charged.get() + 1);
                }
            });
            assert_eq!(
                result,
                Err(Stop::BudgetExceeded(BudgetKind::Fuel)),
                "{fuel}"
            );
            assert_eq!(charged.get(), fuel);
            // SAFETY: the context is live and not used again.
            unsafe {
                assert_eq!((*ctx).budget_counters().fuel_refills, 1, "{fuel}");
                hirsel_rt_v2_ctx_destroy(ctx);
            }
        }
    }

    #[test]
    fn small_heap_charge_includes_the_header_independently_of_the_charge_helper() {
        let ctx = context(None, Some(56));
        assert_eq!(
            run(ctx, || unsafe {
                let block = hirsel_rt_v2_alloc(ctx, 0, 40, 8, DOMAIN_HEAP, 0);
                // A 40-byte request occupies class 48 plus its 8-byte header.
                assert_eq!((*ctx).heap().meter.held(), 56);
                crate::release::hirsel_rt_v2_free(ctx, block);
                assert_eq!((*ctx).heap().meter.held(), 0);
            }),
            Ok(())
        );
        unsafe { hirsel_rt_v2_ctx_destroy(ctx) };
    }

    /// A small block is charged its size class and a large one its request rounded up to
    /// 4 KiB, whatever the slabs and the backing allocator hold; frees credit them.
    #[test]
    fn heap_blocks_charge_their_logical_size() {
        let ctx = context(None, Some(1 << 30));
        assert_eq!(
            run(ctx, || unsafe {
                let small = hirsel_rt_v2_alloc(ctx, 0, 40, 8, DOMAIN_HEAP, 0);
                let class = crate::alloc::request_capacity(40, 8);
                assert_eq!((*ctx).heap().meter.held(), small_charge(class));
                let large = hirsel_rt_v2_alloc(ctx, 0, 10_000, 8, DOMAIN_HEAP, 0);
                assert_eq!(
                    (*ctx).heap().meter.held(),
                    small_charge(class) + 12_288,
                    "10 000 bytes round up to three granules"
                );
                crate::release::hirsel_rt_v2_free(ctx, small);
                crate::release::hirsel_rt_v2_free(ctx, large);
                assert_eq!((*ctx).heap().meter.held(), 0);
            }),
            Ok(())
        );
        // SAFETY: the context is live and not used again.
        unsafe { hirsel_rt_v2_ctx_destroy(ctx) };
    }

    fn enter(ctx: *mut TaskCtx) -> Box<Region> {
        let mut region = Box::new(MaybeUninit::<Region>::uninit());
        // SAFETY: writable storage that outlives the region.
        unsafe { hirsel_rt_v2_region_enter(ctx, region.as_mut_ptr()) };
        // SAFETY: entered.
        unsafe { Box::from_raw(Box::into_raw(region).cast()) }
    }
    fn reserve(ctx: *mut TaskCtx, region: &mut Region, extent: u64) {
        let mut out = std::ptr::null_mut();
        // SAFETY: an active region.
        let status =
            unsafe { crate::region::hirsel_rt_v2_region_reserve(ctx, region, extent, 8, &mut out) };
        assert_eq!(status, 0);
    }

    /// How many 100 KiB reservations and small blocks a region and heap take before a 1 MiB
    /// budget stops the run, after `warm` ran in the same run.
    fn stop_position(warm: impl Fn(*mut TaskCtx)) -> u64 {
        let ctx = context(None, Some(1 << 20));
        let steps = Cell::new(0u64);
        let result = run(ctx, || {
            warm(ctx);
            assert_eq!(unsafe { (*ctx).heap().meter.held() }, 0);
            let mut region = enter(ctx);
            loop {
                reserve(ctx, &mut region, 100 * 1024);
                // SAFETY: the running context.
                unsafe { hirsel_rt_v2_alloc(ctx, 0, 200, 8, DOMAIN_HEAP, 0) };
                steps.set(steps.get() + 1);
            }
        });
        assert_eq!(result, Err(Stop::BudgetExceeded(BudgetKind::Memory)));
        // SAFETY: the context is live and not used again.
        unsafe { hirsel_rt_v2_ctx_destroy(ctx) };
        steps.get()
    }

    /// A stop's position never depends on what the pools hold: a warm region pool (an
    /// oversized idle segment that fits later requests, standard idle segments) and warm heap
    /// slabs stop the same workload exactly where a cold run does.
    #[test]
    fn stop_positions_are_equal_for_warm_and_cold_pools() {
        let cold = stop_position(|_| {});
        assert!(cold > 2, "{cold}");
        let oversized = stop_position(|ctx| {
            let mut region = enter(ctx);
            reserve(ctx, &mut region, 600 * 1024);
            for _ in 0..4 {
                reserve(ctx, &mut region, 40 * 1024);
            }
            // SAFETY: the innermost active region.
            assert_eq!(
                unsafe { hirsel_rt_v2_region_exit(ctx, &mut *region, 0, 0) },
                0
            );
        });
        let slabs = stop_position(|ctx| unsafe {
            let blocks: Vec<_> = (0..2000)
                .map(|_| hirsel_rt_v2_alloc(ctx, 0, 200, 8, DOMAIN_HEAP, 0))
                .collect();
            for block in blocks {
                crate::release::hirsel_rt_v2_free(ctx, block);
            }
        });
        assert_eq!((cold, oversized, slabs), (cold, cold, cold));
    }
}

//! Measurement-only accounting. This entire module and every caller are cfg-elided from
//! timing/production archives. Counts describe physical operations, including immortal no-ops.

use std::collections::BTreeMap;
use std::ffi::{CStr, c_char};
use std::io::Write;

use crate::{CLASS_COUNT, CLASS_SIZES, TaskCtx};

/// Opt-in, single-worker RSS diagnostic, separate from measurement counters and timings.
/// Dump at the scheduler's idle boundary so the external sampler sees settled lists. The
/// regular archive has neither this function nor its call. Addresses let the sampler count
/// resident pages rather than treating reserved backing bytes as RSS.
pub(crate) fn idle_snapshot(ctx: &mut TaskCtx) {
    if std::env::var_os("HIRSEL_RT_HEAP_SNAPSHOT").is_none() {
        return;
    }
    let mut stats = crate::context::Stats::default();
    // SAFETY: a live context on its own worker, outside a running compiled task.
    unsafe { crate::context::hirsel_rt_v2_ctx_stats(ctx, &mut stats) };
    let mut out = std::io::stderr().lock();
    let _ = writeln!(out, "hirsel-rss begin {stats:?}");
    let _ = ctx.heap_ref().write_residency(&mut out);
    let _ = writeln!(
        out,
        "hirsel-rss metadata task={} worker={} run={}",
        size_of::<TaskCtx>(),
        size_of::<crate::context::Worker>(),
        size_of::<crate::context::RunShared>(),
    );
    let _ = writeln!(out, "hirsel-rss end");
}

/// The last bin holds capacities outside the small class schedule, regardless of storage kind.
pub(crate) const BIN_COUNT: usize = CLASS_COUNT + 1;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Counters {
    pub allocations: [u64; BIN_COUNT],
    pub frees: [u64; BIN_COUNT],
    pub count_increments: u64,
    pub count_decrements: u64,
    pub cow_detaches: u64,
    pub deep_copies: u64,
    pub copied_blocks: u64,
    pub copied_bytes: u64,
    pub reuse_hits: u64,
    pub reuse_misses: u64,
    pub live_bytes: u64,
    pub peak_live_bytes: u64,
    pub region_high_water_bytes: u64,
    pub region_pool_high_water_bytes: u64,
    /// Frees of another heap's block, large block or region segment, published to its owner
    /// (design §5 headroom row "Root traffic; remote frees").
    pub remote_frees: u64,
    /// Published blocks, large blocks and segments this heap's collector took back.
    pub remote_collected: u64,
    /// Orphaned heaps a new worker took over.
    pub heap_adoptions: u64,
    /// Logical charges and credits under a memory budget, heap allocations charged and fuel
    /// chunks handed out (`crate::budget`; design §5 headroom row "Meter, permits").
    pub meter_ops: u64,
    pub metered_allocations: u64,
    pub fuel_refills: u64,
}

impl Default for Counters {
    fn default() -> Self {
        Self {
            allocations: [0; BIN_COUNT],
            frees: [0; BIN_COUNT],
            count_increments: 0,
            count_decrements: 0,
            cow_detaches: 0,
            deep_copies: 0,
            copied_blocks: 0,
            copied_bytes: 0,
            reuse_hits: 0,
            reuse_misses: 0,
            live_bytes: 0,
            peak_live_bytes: 0,
            region_high_water_bytes: 0,
            region_pool_high_water_bytes: 0,
            remote_frees: 0,
            remote_collected: 0,
            heap_adoptions: 0,
            meter_ops: 0,
            metered_allocations: 0,
            fuel_refills: 0,
        }
    }
}

impl Counters {
    fn bin(capacity: u64) -> usize {
        CLASS_SIZES
            .iter()
            .position(|&n| u64::from(n) == capacity)
            .unwrap_or(CLASS_COUNT)
    }
    pub(crate) fn allocate(&mut self, capacity: u64) {
        self.allocations[Self::bin(capacity)] += 1;
        self.live_bytes += capacity;
        self.peak_live_bytes = self.peak_live_bytes.max(self.live_bytes);
    }
    pub(crate) fn free_many(&mut self, capacity: u64, count: u64) {
        self.frees[Self::bin(capacity)] += count;
        self.live_bytes -= capacity * count;
    }
    pub(crate) fn write(&self, mut out: impl Write) -> std::io::Result<()> {
        writeln!(out, "hirsel-measurement 1")?;
        for (bin, capacity) in CLASS_SIZES
            .iter()
            .map(|n| n.to_string())
            .chain(std::iter::once("large".into()))
            .enumerate()
        {
            writeln!(
                out,
                "class {capacity} {} {}",
                self.allocations[bin], self.frees[bin]
            )?;
        }
        for (name, value) in [
            ("count_increments", self.count_increments),
            ("count_decrements", self.count_decrements),
            ("cow_detaches", self.cow_detaches),
            ("deep_copies", self.deep_copies),
            ("copied_blocks", self.copied_blocks),
            ("copied_bytes", self.copied_bytes),
            ("reuse_hits", self.reuse_hits),
            ("reuse_misses", self.reuse_misses),
            ("live_bytes", self.live_bytes),
            ("peak_live_bytes", self.peak_live_bytes),
            ("region_high_water_bytes", self.region_high_water_bytes),
            (
                "region_pool_high_water_bytes",
                self.region_pool_high_water_bytes,
            ),
            ("remote_frees", self.remote_frees),
            ("remote_collected", self.remote_collected),
            ("heap_adoptions", self.heap_adoptions),
            ("meter_ops", self.meter_ops),
            ("metered_allocations", self.metered_allocations),
            ("fuel_refills", self.fuel_refills),
        ] {
            writeln!(out, "{name} {value}")?;
        }
        // Native arenas are dependency-owned, not a measured zero.
        writeln!(out, "arena_high_water_bytes unavailable")?;
        out.flush()
    }
}

impl TaskCtx {
    /// Retain the output descriptor pinned and checked by the host adapter before
    /// entry. Only instrumented archives have this method; no path is reopened.
    pub fn set_measurement_output(&mut self, file: std::fs::File) {
        *self.run().measurement_output = Some(file);
    }
    pub fn set_profile_output(&mut self, file: std::fs::File) {
        *self.run().profile_output = Some(file);
    }
    pub fn measurements(&self) -> &Counters {
        &self.heap_ref().measurement.totals
    }
}

/// Account for an inline compiled drop edge. The caller performs the count operation;
/// runtime release paths account for their own edges instead.
/// # Safety
/// `ctx` is a live context. `block` is the live block whose credit will be consumed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_measure_release(
    ctx: *mut TaskCtx,
    _block: *mut crate::Header,
) {
    unsafe {
        (*ctx).heap().measurement.record("count_decrements", 1);
        (&mut (*ctx).heap().measurement).count_decrements += 1;
    }
}

/// Object-owned metadata. Strings are NUL-terminated UTF-8, live for the run.
#[repr(C)]
pub struct ProfileSite {
    pub id: u64,
    pub package: *const c_char,
    pub file: *const c_char,
    pub function: *const c_char,
    pub operation: *const c_char,
    pub line: u32,
    pub column: u32,
}
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct SiteCounts {
    package: String,
    file: String,
    function: String,
    operation: String,
    line: u32,
    column: u32,
    counts: BTreeMap<&'static str, u64>,
}
/// Enter one emitted call's site; generated glue and runtime child walks inherit it.
/// # Safety
/// `ctx` is live and `site` and each of its strings are immutable for the run.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_profile_enter(
    ctx: *mut TaskCtx,
    site: *const ProfileSite,
) -> u64 {
    // SAFETY: emitted constants obey ProfileSite's layout and lifetime contract.
    unsafe {
        let site = &*site;
        let counters = &mut (*ctx).heap().measurement;
        counters.sites.entry(site.id).or_insert_with(|| {
            let text = |p| {
                CStr::from_ptr(p)
                    .to_str()
                    .expect("profile UTF-8")
                    .to_owned()
            };
            SiteCounts {
                package: text(site.package),
                file: text(site.file),
                function: text(site.function),
                operation: text(site.operation),
                line: site.line,
                column: site.column,
                counts: BTreeMap::new(),
            }
        });
        std::mem::replace(&mut counters.active_site, site.id)
    }
}
/// Restore the caller's site, including after failed calls and finalizer callbacks.
/// # Safety
/// `ctx` is live and `previous` is an id returned by profile_enter on this context.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_profile_leave(ctx: *mut TaskCtx, previous: u64) {
    unsafe {
        (*ctx).heap().measurement.active_site = previous;
    }
}

/// Attribution is kept separately so the existing totals API stays unchanged.
#[derive(Default)]
pub(crate) struct Accounting {
    totals: Counters,
    pub(crate) active_site: u64,
    pub(crate) sites: BTreeMap<u64, SiteCounts>,
}
impl std::ops::Deref for Accounting {
    type Target = Counters;
    fn deref(&self) -> &Counters {
        &self.totals
    }
}
impl std::ops::DerefMut for Accounting {
    fn deref_mut(&mut self) -> &mut Counters {
        &mut self.totals
    }
}
impl Accounting {
    pub(crate) fn record(&mut self, kind: &'static str, count: u64) {
        let site = self.sites.entry(self.active_site).or_default();
        *site.counts.entry(kind).or_default() += count;
    }
    pub(crate) fn write_sites(&self, mut out: impl Write) -> std::io::Result<()> {
        let totals = serde_json::json!({
            "allocations": self.allocations.iter().sum::<u64>(), "frees": self.frees.iter().sum::<u64>(),
            "count_increments": self.count_increments, "count_decrements": self.count_decrements,
            "cow_detaches": self.cow_detaches, "deep_copies": self.deep_copies,
            "copied_blocks": self.copied_blocks, "copied_bytes": self.copied_bytes,
            "reuse_hits": self.reuse_hits, "reuse_misses": self.reuse_misses,
        });
        let sites: Vec<_> = self
            .sites
            .iter()
            .filter(|(_, site)| !site.counts.is_empty())
            .map(|(id, site)| {
                serde_json::json!({
                    "id": id, "package": site.package, "file": site.file, "function": site.function,
                    "line": site.line, "column": site.column, "operation": site.operation,
                    "counts": site.counts,
                })
            })
            .collect();
        serde_json::to_writer(
            &mut out,
            &serde_json::json!({"schema": 1, "totals": totals, "sites": sites}),
        )?;
        writeln!(out)?;
        out.flush()
    }

    pub(crate) fn allocate(&mut self, capacity: u64) {
        self.record("allocations", 1);
        self.totals.allocate(capacity);
    }
    pub(crate) fn free(&mut self, capacity: u64) {
        self.free_many(capacity, 1);
    }
    pub(crate) fn free_many(&mut self, capacity: u64, count: u64) {
        self.record("frees", count);
        self.totals.free_many(capacity, count);
    }
}

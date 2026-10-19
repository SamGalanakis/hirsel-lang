//! `task.isolate` (FAIL-2): the one place a failure is caught.
//!
//! The compiled body reads the record count before its callback runs
//! ([`hirsel_rt_v2_isolate_begin`]); when the callback fails, its failure cleanup ends in the
//! `Isolate` terminator, whose [`hirsel_rt_v2_isolate_catch`] moves every record since that count
//! out of the running task's failure state into the caught chain. The body then builds the
//! `Failure` value from the caught chain, record by record, through the `isolate_*` entries.
//!
//! A record holds compiler codes only, so the program's failure table names their text: each
//! site code's definition path and one-based line and column, and each program rule code's rule
//! ID. Every source program carries one in its descriptor (`crate::program`), and each run's
//! context reaches its own program's table, so runs of different programs share a process.

use std::sync::Arc;

use crate::alloc::{allocate_sequence, payload, sequence_elements_offset};
use crate::context::RunCtx;
use crate::failure::Chained;
use crate::text::allocate_bytes;
use crate::views::Slice;
use crate::{STATUS_OK, fault};

/// One failure site of the program: its emitted code and its source position.
struct Site {
    code: u64,
    definition: String,
    file: String,
    line: u64,
    column: u64,
}

/// A program's failure table.
pub struct FailureTable {
    /// Sorted by code.
    sites: Vec<Site>,
    /// The rule ID of program rule code `index + 1`.
    rules: Vec<String>,
}

impl FailureTable {
    /// Decode an encoded table: little-endian u64 words, the site count, then each site's code,
    /// line, column and definition length followed by the definition's bytes and a
    /// length-prefixed file path, in code order, then the rule count and each rule's length and
    /// bytes. `None` when malformed or out of site order.
    pub fn decode(bytes: &[u8]) -> Option<FailureTable> {
        decode(bytes).filter(|table| table.sites.is_sorted_by_key(|s| s.code))
    }

    fn site(&self, code: u64) -> &Site {
        self.sites
            .binary_search_by_key(&code, |s| s.code)
            .map(|i| &self.sites[i])
            .unwrap_or_else(|_| fault("a failure site the program's table does not list"))
    }

    /// The rule ID of a record's rule code.
    fn rule(&self, code: u32) -> &str {
        let rule = match code.checked_sub(hirsel_core_contract::RUNTIME_RULE_BASE) {
            Some(i) => hirsel_core_contract::RUNTIME_RULES.get(i as usize).copied(),
            None => code
                .checked_sub(1)
                .and_then(|i| self.rules.get(i as usize))
                .map(String::as_str),
        };
        rule.unwrap_or_else(|| fault("a failure rule the program's table does not list"))
    }

    pub(crate) fn located(&self, code: u64) -> hirsel_run_config::Site {
        let site = self.site(code);
        hirsel_run_config::Site {
            definition: site.definition.clone(),
            file: site.file.clone(),
            line: site.line,
            column: site.column,
        }
    }

    /// A failure record as its host reports it.
    pub(crate) fn failure(
        &self,
        record: &crate::failure::FailureRecord,
    ) -> hirsel_run_config::Failure {
        hirsel_run_config::Failure {
            rule: self.rule(record.rule).into(),
            site: self.located(record.site),
            operation: (record.operation_site != 0 && record.operation_site != record.site)
                .then(|| self.located(record.operation_site)),
        }
    }
}

fn decode(mut bytes: &[u8]) -> Option<FailureTable> {
    fn word(bytes: &mut &[u8]) -> Option<u64> {
        let (head, rest) = bytes.split_first_chunk::<8>()?;
        *bytes = rest;
        Some(u64::from_le_bytes(*head))
    }
    fn text(bytes: &mut &[u8]) -> Option<String> {
        let length = usize::try_from(word(bytes)?).ok()?;
        let (head, rest) = bytes.split_at_checked(length)?;
        *bytes = rest;
        String::from_utf8(head.to_vec()).ok()
    }
    let sites = (0..word(&mut bytes)?)
        .map(|_| {
            let (code, line, column) = (word(&mut bytes)?, word(&mut bytes)?, word(&mut bytes)?);
            Some(Site {
                code,
                definition: text(&mut bytes)?,
                line,
                column,
                file: text(&mut bytes)?,
            })
        })
        .collect::<Option<_>>()?;
    let rules = (0..word(&mut bytes)?)
        .map(|_| text(&mut bytes))
        .collect::<Option<_>>()?;
    bytes.is_empty().then_some(FailureTable { sites, rules })
}

/// The failure table of the program `ctx` runs.
fn table(ctx: *mut RunCtx) -> &'static FailureTable {
    // SAFETY: the context is live and keeps its table until it is destroyed.
    unsafe { (*ctx).failure_table.as_deref() }
        .map(|table| unsafe { &*std::ptr::from_ref::<FailureTable>(table) })
        .unwrap_or_else(|| fault("an isolated failure without the program's failure table"))
}

fn site(ctx: *mut RunCtx, code: u64) -> &'static Site {
    table(ctx).site(code)
}

fn rule(ctx: *mut RunCtx, code: u32) -> &'static str {
    table(ctx).rule(code)
}

/// Human failure text for a standalone launch, outside program Io and replay tapes.
pub fn report(failure: &hirsel_run_config::Failure) -> String {
    format!(
        "{}\n",
        failure
            .site
            .report(&failure.rule, false, failure.operation.as_ref())
    )
}

/// Give `ctx` its program's failure table.
pub(crate) fn attach(ctx: &mut RunCtx, table: Arc<FailureTable>) {
    ctx.failure_table = Some(table);
}

/// The running task's caught chain.
fn caught(ctx: *mut RunCtx) -> &'static mut Vec<Chained> {
    &mut crate::tasks::scheduler(ctx).isolated
}

fn record(ctx: *mut RunCtx, index: u64) -> &'static Chained {
    usize::try_from(index)
        .ok()
        .and_then(|i| caught(ctx).get(i))
        .unwrap_or_else(|| fault("an isolated failure index out of range"))
}

/// `$intrinsic.IsolateBegin(tasks)`: the running task's record count, which the catch keeps
/// from; the operation yields, as `task.isolate` does before its callback runs.
///
/// # Safety
/// `ctx` is live and `out` writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_isolate_begin(
    ctx: *mut RunCtx,
    out: *mut u64,
    _tasks: *const u64,
) -> u8 {
    // SAFETY: caller contract.
    unsafe {
        #[cfg(feature = "measurement")]
        let _segment = crate::tasks::profile::Operation::new(ctx, "isolate.begin");
        out.write((*ctx).failure.count() as u64);
        crate::tasks::yield_now(ctx);
    }
    STATUS_OK
}

/// The `Isolate` terminator: the records since `start` become the caught chain, the running
/// task's own attributed to its spawn path, and are forgotten.
///
/// # Safety
/// `ctx` is live and `start` a record count `hirsel_rt_v2_isolate_begin` returned.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_isolate_catch(ctx: *mut RunCtx, start: u64) {
    let path = crate::tasks::current_path(ctx);
    let start = usize::try_from(start).unwrap_or_else(|_| fault("an isolation start overflows"));
    // SAFETY: caller contract.
    let chain = unsafe { (*ctx).failure.split_off(start, &path) };
    if chain.is_empty() {
        fault("an isolation caught no failure");
    }
    *caught(ctx) = chain;
}

/// `$intrinsic.IsolateCount(tasks)`: the caught chain's length.
///
/// # Safety
/// `ctx` is live and `out` writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_isolate_count(
    ctx: *mut RunCtx,
    out: *mut u64,
    _tasks: *const u64,
) -> u8 {
    // SAFETY: caller contract.
    unsafe { out.write(caught(ctx).len() as u64) };
    STATUS_OK
}

/// `$intrinsic.IsolateKind(tasks, index)`: the `FailureKind` ordinal of caught record `index`.
///
/// # Safety
/// `ctx` is live and `out` writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_isolate_kind(
    ctx: *mut RunCtx,
    out: *mut u32,
    _tasks: *const u64,
    index: u64,
) -> u8 {
    let kind = hirsel_core_contract::failure_kind(rule(ctx, record(ctx, index).0.rule));
    // SAFETY: caller contract.
    unsafe { out.write(kind) };
    STATUS_OK
}

/// `$intrinsic.IsolateRule(tasks, index)`: the rule ID of caught record `index`.
///
/// # Safety
/// `ctx` is live and `out` writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_isolate_rule(
    ctx: *mut RunCtx,
    _destination: *mut u8,
    out: *mut Slice,
    _tasks: *const u64,
    index: u64,
) -> u8 {
    let rule = rule(ctx, record(ctx, index).0.rule);
    // SAFETY: caller contract.
    unsafe { out.write(allocate_bytes(ctx, rule.as_bytes())) };
    STATUS_OK
}

/// `$intrinsic.IsolateDefinition(tasks, index)`: the definition path of caught record
/// `index`'s site.
///
/// # Safety
/// `ctx` is live and `out` writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_isolate_definition(
    ctx: *mut RunCtx,
    _destination: *mut u8,
    out: *mut Slice,
    _tasks: *const u64,
    index: u64,
) -> u8 {
    let site = site(ctx, record(ctx, index).0.site);
    // SAFETY: caller contract.
    unsafe { out.write(allocate_bytes(ctx, site.definition.as_bytes())) };
    STATUS_OK
}

/// `$intrinsic.IsolateLine(tasks, index)`: the one-based line of caught record `index`'s site.
///
/// # Safety
/// `ctx` is live and `out` writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_isolate_line(
    ctx: *mut RunCtx,
    out: *mut u64,
    _tasks: *const u64,
    index: u64,
) -> u8 {
    // SAFETY: caller contract.
    unsafe { out.write(site(ctx, record(ctx, index).0.site).line) };
    STATUS_OK
}

/// `$intrinsic.IsolateColumn(tasks, index)`: the one-based column of caught record `index`'s
/// site.
///
/// # Safety
/// `ctx` is live and `out` writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_isolate_column(
    ctx: *mut RunCtx,
    out: *mut u64,
    _tasks: *const u64,
    index: u64,
) -> u8 {
    // SAFETY: caller contract.
    unsafe { out.write(site(ctx, record(ctx, index).0.site).column) };
    STATUS_OK
}

/// `$intrinsic.IsolateSpawn(tasks, index)`: a fresh `List[u64]` of caught record `index`'s
/// spawn path, in the runtime's pointer-free sequence storage.
///
/// # Safety
/// `ctx` is live and `out` writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_isolate_spawn(
    ctx: *mut RunCtx,
    _destination: *mut u8,
    out: *mut *mut crate::Header,
    _tasks: *const u64,
    index: u64,
) -> u8 {
    let path = &record(ctx, index).1;
    // SAFETY: the context is live; caller contract.
    unsafe {
        let block = allocate_sequence(ctx, 0, path.len() as u64, 8, 8);
        let first = payload(block).add(sequence_elements_offset(8) as usize);
        for (i, ordinal) in path.iter().enumerate() {
            first
                .add(i * 8)
                .cast::<u64>()
                .write_unaligned(u64::from(*ordinal));
        }
        out.write(block);
    }
    STATUS_OK
}

/// Source location of a foreign contract violation, outside the language failure chain.
pub(crate) fn foreign_location(ctx: *mut RunCtx, code: u64) -> String {
    if ctx.is_null() {
        return format!("site {code:#x}");
    }
    let source = site(ctx, code);
    format!(
        "{} ({}:{}:{})",
        source.definition, source.file, source.line, source.column
    )
}

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

use crate::alloc::{allocate_sequence, payload, sequence_elements_offset};
use crate::context::TaskCtx;
use crate::failure::Chained;
use crate::text::allocate_bytes;
use crate::views::Slice;
use crate::{STATUS_OK, fault};

pub(crate) mod entered;

/// Task-owned invocation records move with TaskCtx, never with a worker or a
/// native stack. A count is a failure mark, not an invocation identity.
#[derive(Default)]
pub(crate) struct Invocations {
    generation: u64,
    scopes: Vec<Invocation>,
}

struct Invocation {
    generation: u64,
    parent: Option<u64>,
    mark: usize,
    caught: Option<Vec<Chained>>,
}

impl Invocations {
    fn begin(&mut self, mark: usize) {
        self.generation = self
            .generation
            .checked_add(1)
            .unwrap_or_else(|| fault("isolation invocation generation exhausted"));
        self.scopes.push(Invocation {
            generation: self.generation,
            parent: self.scopes.last().map(|scope| scope.generation),
            mark,
            caught: None,
        });
    }

    fn active(&mut self) -> &mut Invocation {
        self.scopes
            .last_mut()
            .unwrap_or_else(|| fault("isolation without a live invocation"))
    }

    fn end(&mut self) {
        let scope = self
            .scopes
            .pop()
            .unwrap_or_else(|| fault("isolation ended twice"));
        if scope.parent != self.scopes.last().map(|scope| scope.generation) {
            fault("isolation invocation parent changed");
        }
    }
}

/// One failure site of the program: its source position.
#[derive(Clone, Copy)]
struct Site {
    definition: &'static str,
    file: &'static str,
    line: u64,
    column: u64,
}

/// Bytes of one site record and of one rule record.
const SITE_BYTES: usize = 32;
const RULE_BYTES: usize = 8;

/// A program's failure table, read in place from the program's immutable data: loading a
/// program decodes nothing, and a lookup reads only the record and strings it names.
#[derive(Clone, Copy)]
pub struct FailureTable {
    /// Site records, sorted by code.
    sites: &'static [[u8; SITE_BYTES]],
    /// The rule ID of program rule code `index + 1`.
    rules: &'static [[u8; RULE_BYTES]],
    pool: &'static [u8],
}

fn word(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap())
}

fn code(site: &[u8; SITE_BYTES]) -> u64 {
    u64::from_le_bytes(site[..8].try_into().unwrap())
}

impl FailureTable {
    /// Read an encoded table: little-endian, a u32 site count and a u32 rule count, then each
    /// site's 32-byte record (u64 code, u32 line, u32 column, then the u32 offset and length in
    /// the string pool of its definition path and of its file path) in increasing code order,
    /// each rule's u32 offset and length, then the UTF-8 string pool. `None` when the counts
    /// overrun the bytes; a malformed string is a fault when a lookup reads it.
    pub fn new(bytes: &'static [u8]) -> Option<FailureTable> {
        let (head, rest) = bytes.split_first_chunk::<8>()?;
        let count = |at| usize::try_from(word(head, at)).ok();
        let (sites, rest) = rest.split_at_checked(count(0)?.checked_mul(SITE_BYTES)?)?;
        let (rules, pool) = rest.split_at_checked(count(4)?.checked_mul(RULE_BYTES)?)?;
        let table = FailureTable {
            sites: sites.as_chunks().0,
            rules: rules.as_chunks().0,
            pool,
        };
        debug_assert!(table.sites.is_sorted_by_key(code));
        Some(table)
    }

    fn text(&self, record: &[u8], at: usize) -> &'static str {
        let (offset, length) = (word(record, at) as usize, word(record, at + 4) as usize);
        self.pool
            .get(offset..)
            .and_then(|tail| tail.get(..length))
            .and_then(|bytes| std::str::from_utf8(bytes).ok())
            .unwrap_or_else(|| fault("a malformed failure table string"))
    }

    fn site(&self, wanted: u64) -> Site {
        let record = self
            .sites
            .binary_search_by_key(&wanted, code)
            .map(|i| &self.sites[i])
            .unwrap_or_else(|_| fault("a failure site the program's table does not list"));
        Site {
            line: u64::from(word(record, 8)),
            column: u64::from(word(record, 12)),
            definition: self.text(record, 16),
            file: self.text(record, 24),
        }
    }

    /// The rule ID of a record's rule code.
    fn rule(&self, code: u32) -> &'static str {
        let rule = match code.checked_sub(hirsel_core_contract::RUNTIME_RULE_BASE) {
            Some(i) => hirsel_core_contract::RUNTIME_RULES.get(i as usize).copied(),
            None => code
                .checked_sub(1)
                .and_then(|i| self.rules.get(i as usize))
                .map(|record| self.text(record, 0)),
        };
        rule.unwrap_or_else(|| fault("a failure rule the program's table does not list"))
    }

    pub(crate) fn located(&self, code: u64) -> hirsel_run_schema::Site {
        let site = self.site(code);
        hirsel_run_schema::Site {
            definition: site.definition.into(),
            file: site.file.into(),
            line: site.line,
            column: site.column,
        }
    }

    /// Resolve a replayed public failure back into this exact program's
    /// source/rule table for the native observation reader. No source code or
    /// rule absent from the bound program can be manufactured by a terminal.
    pub(crate) fn record(
        &self,
        failure: &hirsel_run_schema::Failure,
    ) -> Option<crate::failure::FailureRecord> {
        let site = |wanted: &hirsel_run_schema::Site| {
            self.sites.iter().find_map(|row| {
                let key = code(row);
                (self.located(key) == *wanted).then_some(key)
            })
        };
        let rule = hirsel_core_contract::RUNTIME_RULES
            .iter()
            .position(|rule| *rule == failure.rule)
            .map(|index| hirsel_core_contract::RUNTIME_RULE_BASE + index as u32)
            .or_else(|| {
                self.rules
                    .iter()
                    .position(|row| self.text(row, 0) == failure.rule)
                    .map(|index| index as u32 + 1)
            })?;
        Some(crate::failure::FailureRecord {
            kind: 1,
            rule,
            site: site(&failure.site)?,
            operation_site: match &failure.operation {
                Some(operation) => site(operation)?,
                None => 0,
            },
        })
    }

    /// A failure record as its host reports it.
    pub(crate) fn failure(
        &self,
        record: &crate::failure::FailureRecord,
    ) -> hirsel_run_schema::Failure {
        hirsel_run_schema::Failure {
            rule: self.rule(record.rule).into(),
            site: self.located(record.site),
            operation: (record.operation_site != 0 && record.operation_site != record.site)
                .then(|| self.located(record.operation_site)),
        }
    }
}

/// The failure table of the program `ctx` runs.
fn table(ctx: *mut TaskCtx) -> FailureTable {
    // SAFETY: the context is live.
    unsafe { (*ctx).run_ref().failure_table }
        .unwrap_or_else(|| fault("an isolated failure without the program's failure table"))
}

fn site(ctx: *mut TaskCtx, code: u64) -> Site {
    table(ctx).site(code)
}

fn rule(ctx: *mut TaskCtx, code: u32) -> &'static str {
    table(ctx).rule(code)
}

/// Human failure text for a standalone launch, outside program Io and replay tapes.
pub fn report(failure: &hirsel_run_schema::Failure) -> String {
    format!(
        "{}\n",
        failure
            .site
            .report(&failure.rule, false, failure.operation.as_ref())
    )
}

/// Give `ctx` its program's failure table.
pub(crate) fn attach(ctx: &mut TaskCtx, table: FailureTable) {
    ctx.run().failure_table = Some(table);
}

/// The running task's caught chain.
pub(crate) fn caught(ctx: *mut TaskCtx) -> &'static mut Vec<Chained> {
    // SAFETY: the running task's context outlives its use of the scratch.
    unsafe { (*ctx).isolations.active().caught.as_mut() }
        .unwrap_or_else(|| fault("isolation read before catch"))
}

fn record(ctx: *mut TaskCtx, index: u64) -> &'static Chained {
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
    ctx: *mut TaskCtx,
    out: *mut u64,
    _tasks: *const u64,
) -> u8 {
    // SAFETY: this semantic entry owns its task until it suspends or returns.
    let _role = unsafe { crate::tasks::role::Guard::new(ctx) };
    // SAFETY: caller contract.
    unsafe {
        let mark = (*ctx).failure.count();
        (*ctx).isolations.begin(mark);
        out.write(mark as u64);
        crate::tasks::yield_now(&mut *ctx);
    }
    STATUS_OK
}

/// The `Isolate` terminator: the records since `start` become the caught chain, the running
/// task's own attributed to its spawn path, and are forgotten.
///
/// # Safety
/// `ctx` is live and `start` a record count `hirsel_rt_v2_isolate_begin` returned.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_isolate_catch(ctx: *mut TaskCtx, start: u64) {
    // SAFETY: this semantic entry owns its task until it suspends or returns.
    let _role = unsafe { crate::tasks::role::Guard::new(ctx) };
    let path = crate::tasks::current_path(ctx);
    let start = usize::try_from(start).unwrap_or_else(|_| fault("an isolation start overflows"));
    // SAFETY: the task owns its current invocation.
    let invocation = unsafe { (*ctx).isolations.active() };
    if invocation.mark != start || invocation.caught.is_some() {
        fault("isolation catch does not match its unhandled begin");
    }
    // SAFETY: caller contract.
    let chain = unsafe { (*ctx).failure.split_off(start, &path) };
    if chain.is_empty() {
        fault("an isolation caught no failure");
    }
    // SAFETY: catch cannot suspend or change the active invocation.
    unsafe { (*ctx).isolations.active().caught = Some(chain) };
}

/// Retire only invocation bookkeeping, leaving the language failure chain intact.
/// # Safety
/// `ctx` owns an active invocation and all its caught-chain readers have finished.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_isolate_end(ctx: *mut TaskCtx, _tasks: *const u64) -> u8 {
    // SAFETY: caller contract; End never yields or runs language cleanup.
    unsafe { (*ctx).isolations.end() };
    STATUS_OK
}

/// `$intrinsic.IsolateCount(tasks)`: the caught chain's length.
///
/// # Safety
/// `ctx` is live and `out` writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_isolate_count(
    ctx: *mut TaskCtx,
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
    ctx: *mut TaskCtx,
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
    ctx: *mut TaskCtx,
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
    ctx: *mut TaskCtx,
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
    ctx: *mut TaskCtx,
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
    ctx: *mut TaskCtx,
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
    ctx: *mut TaskCtx,
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
pub(crate) fn foreign_location(ctx: *mut TaskCtx, code: u64) -> String {
    if ctx.is_null() {
        return format!("site {code:#x}");
    }
    let source = site(ctx, code);
    format!(
        "{} ({}:{}:{})",
        source.definition, source.file, source.line, source.column
    )
}

/// A compiler-supplied site in the attached program's immutable source table.
pub(crate) fn source_site(ctx: *mut TaskCtx, code: u64) -> hirsel_run_schema::Site {
    table(ctx).located(code)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chain(site: u64) -> Vec<Chained> {
        vec![(
            crate::failure::FailureRecord {
                kind: 1,
                rule: 1,
                site,
                operation_site: site,
            },
            vec![2, 3],
        )]
    }

    #[test]
    fn equal_marks_have_distinct_generations_and_child_end_restores_parent_catch() {
        let mut scopes = Invocations::default();
        scopes.begin(0);
        let parent = scopes.active().generation;
        scopes.active().caught = Some(chain(11));
        scopes.begin(0);
        let child = scopes.active().generation;
        assert_ne!(parent, child);
        scopes.active().caught = Some(chain(22));
        scopes.begin(0);
        assert_ne!(child, scopes.active().generation);
        scopes.active().caught = Some(chain(33));
        scopes.end();
        assert_eq!(scopes.active().generation, child);
        assert_eq!(scopes.active().caught, Some(chain(22)));
        scopes.end();
        assert_eq!(scopes.active().generation, parent);
        assert_eq!(scopes.active().caught, Some(chain(11)));
        scopes.end();
        scopes.begin(0);
        assert!(scopes.active().generation > child);
        assert!(scopes.active().caught.is_none());
        scopes.end();
    }

    #[test]
    fn an_inner_catch_preserves_the_active_prefix_and_attribution() {
        let mut failures = crate::failure::FailureState::default();
        let before = chain(11).pop().unwrap().0;
        let after = chain(22).pop().unwrap().0;
        failures.record(before);
        let mut scopes = Invocations::default();
        scopes.begin(failures.count());
        failures.record(after);
        let suffix = failures.split_off(scopes.active().mark, &[7, 8]);
        scopes.active().caught = Some(suffix);
        assert_eq!(failures.take_chain(&[7, 8]), vec![(before, vec![7, 8])]);
        assert_eq!(scopes.active().caught, Some(vec![(after, vec![7, 8])]));
        scopes.end();
    }
}

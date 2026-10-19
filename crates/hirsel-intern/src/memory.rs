//! Heap accounting for `--mem-usage` (docs/building.md, "Compile timings").
//! A binary installs a meter that reads its allocator's live heap bytes;
//! without one, [`metered`] is false and nothing is recorded. Each record
//! names what holds the bytes: the net bytes a load kept ([`retained`]) or
//! the bytes a table freed when it was dropped ([`released`]). Records with
//! the same name add up.
use std::sync::{Mutex, OnceLock};

static METER: OnceLock<fn() -> i64> = OnceLock::new();
static RECORDS: Mutex<Vec<(&'static str, i64)>> = Mutex::new(Vec::new());

/// Measure with `live`, the allocator's current live heap bytes.
pub fn install(live: fn() -> i64) {
    let _ = METER.set(live);
}

/// Whether a meter is installed.
#[inline]
pub fn metered() -> bool {
    METER.get().is_some()
}

fn add(name: &'static str, bytes: i64) {
    let mut records = RECORDS.lock().unwrap_or_else(|e| e.into_inner());
    match records.iter_mut().find(|(n, _)| *n == name) {
        Some((_, total)) => *total += bytes,
        None => records.push((name, bytes)),
    }
}

/// Run `work`, recording as `name` the heap bytes it allocated and kept.
pub fn retained<T>(name: &'static str, work: impl FnOnce() -> T) -> T {
    let Some(live) = METER.get() else {
        return work();
    };
    let before = live();
    let value = work();
    add(name, live() - before);
    value
}

/// Drop `value`, recording as `name` the heap bytes that freed.
pub fn released<T>(name: &'static str, value: T) {
    let Some(live) = METER.get() else {
        return;
    };
    let before = live();
    drop(value);
    add(name, before - live());
}

/// Every record so far, largest first.
pub fn records() -> Vec<(&'static str, i64)> {
    let mut records = RECORDS.lock().unwrap_or_else(|e| e.into_inner()).clone();
    records.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
    records
}

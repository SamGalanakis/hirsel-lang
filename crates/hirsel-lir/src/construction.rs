//! Passive request-scoped Value construction counters. Producers install the
//! same trace on each active worker; other requests and threads stay isolated.
//! This retains scalar counts only, never native bodies or proof authority.
use crate::ir::UnitHeader;
use crate::stage::StageKind;
use hirsel_syntax::{ContentHash, DefPath};
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;
use std::sync::{Arc, Mutex};

type Owner = (DefPath, ContentHash);
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ValueCounts {
    pub created: u64,
    pub cloned: u64,
    pub retagged: u64,
    pub decoded: u64,
    pub rebound: u64,
    pub decoded_bytes: u64,
}
impl ValueCounts {
    fn add(&mut self, other: Self) {
        self.created += other.created;
        self.cloned += other.cloned;
        self.retagged += other.retagged;
        self.decoded += other.decoded;
        self.rebound += other.rebound;
        self.decoded_bytes += other.decoded_bytes;
    }
}
#[derive(Clone, Default)]
pub struct ValueTrace(Arc<Mutex<BTreeMap<Owner, ValueCounts>>>);
thread_local! {
    static TRACES: RefCell<Vec<(Rc<()>, ValueTrace)>> = const { RefCell::new(Vec::new()) };
}
/// Worker-local installation. Dropping any guard removes precisely its own
/// subscription, including during failure unwinding or out-of-order cleanup.
#[must_use = "dropping the guard ends observation on this worker"]
pub struct ValueTraceGuard(Rc<()>);
impl Drop for ValueTraceGuard {
    fn drop(&mut self) {
        TRACES.with_borrow_mut(|traces| traces.retain(|(token, _)| !Rc::ptr_eq(token, &self.0)));
    }
}
impl ValueTrace {
    pub fn enter(&self) -> ValueTraceGuard {
        let token = Rc::new(());
        TRACES.with_borrow_mut(|traces| traces.push((token.clone(), self.clone())));
        ValueTraceGuard(token)
    }
    pub fn snapshot(&self) -> BTreeMap<Owner, ValueCounts> {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
    pub fn totals(&self) -> ValueCounts {
        let mut total = ValueCounts::default();
        for value in self.0.lock().unwrap_or_else(|e| e.into_inner()).values() {
            total.add(*value);
        }
        total
    }
}
pub(crate) enum ValueWork {
    Create,
    Clone,
    Retag,
    Decode,
    Rebind,
    DecodedBytes(u64),
}
pub(crate) fn record(header: &UnitHeader, work: ValueWork) {
    if header.stage != StageKind::Value {
        return;
    }
    TRACES.with_borrow(|traces| {
        let Some((_, trace)) = traces.last() else {
            return;
        };
        let mut counts = trace.0.lock().unwrap_or_else(|e| e.into_inner());
        let row = counts
            .entry((header.definition.clone(), header.instance))
            .or_default();
        match work {
            ValueWork::Create => row.created += 1,
            ValueWork::Clone => row.cloned += 1,
            ValueWork::Retag => row.retagged += 1,
            ValueWork::Decode => row.decoded += 1,
            ValueWork::Rebind => row.rebound += 1,
            ValueWork::DecodedBytes(bytes) => row.decoded_bytes += bytes,
        }
    });
}

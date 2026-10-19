//! Validated grant subset relation.
use crate::ValidatedRun;
use hirsel_run_schema::{env_within, io_within};
/// Whether every grant of `a` is within `b`: Unsafe, every Io projection with its exact
/// provider, every mount, Net rule and process rule, and every Env name with its snapshot.
/// Absent Io or Env is within anything; a present one is within a present one only.
/// Program, entry, launch kind, inputs, Args, admission and scripted provider data
/// (peers, listeners, runs, children, signal scripts) are not authority and are not compared.
pub fn grants_within(a: &ValidatedRun, b: &ValidatedRun) -> bool {
    let (a, b) = (a.record(), b.record());
    (!a.unsafe_root || b.unsafe_root)
        && optional(&a.io, &b.io, io_within)
        && optional(&a.env, &b.env, env_within)
}

fn optional<T>(a: &Option<T>, b: &Option<T>, within: impl Fn(&T, &T) -> bool) -> bool {
    match (a, b) {
        (None, _) => true,
        (Some(_), None) => false,
        (Some(a), Some(b)) => within(a, b),
    }
}

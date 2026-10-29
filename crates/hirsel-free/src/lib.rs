//! The free-mode protocol of `tasks.free@1` (design §5), shared by the runtime and the
//! interpreter: op phases and published progress ([`phase`]), op sections and the cutter
//! ([`section`]), issue ownership ([`claim`]), the compressed records and the tape
//! ([`record`], [`tape`]), host-key epochs ([`keys`]), pending `Shared` releases
//! ([`pending`]), the normalised projection ([`projection`]), the recording and replay
//! sides ([`recorder`], [`replay`]), and the width-1 session an engine drives ([`session`]).
//!
//! `proofs/free` model-checks the concurrent modules (`sync`, `phase`, `section`, `claim`)
//! from this same source with loom, and the section handshake and claim protocol in GenMC.

#[cfg(test)]
hirsel_guard::install!();

pub mod atomic;
pub mod cancel;
pub mod claim;
pub mod executors;
pub mod frame;
pub mod keys;
pub mod mutex;
pub mod pending;
pub mod phase;
pub mod projection;
pub mod queue;
pub mod record;
pub mod recorder;
pub mod replay;
pub mod schedule;
pub mod section;
pub mod session;
pub mod sync;
pub mod tape;

pub mod work;

pub mod concurrent;

pub mod batch;

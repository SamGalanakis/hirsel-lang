//! Runner calls across machines (docs/spec/runners.md, `remote.tls@1`): the core
//! `RunFrame` wire type, a runner host's protocol and a caller's provider, both free of
//! I/O, the host's call journal seam, and the transports that carry frames.
//! The driver wires them to the interpreter.

#[cfg(test)]
hirsel_guard::install!();

pub mod client;
pub mod data;
pub mod frame;
pub mod host;
pub mod journal;
pub mod transport;
pub mod wire;

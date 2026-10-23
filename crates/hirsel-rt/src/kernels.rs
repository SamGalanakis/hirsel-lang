//! Typed leaf adapters. Every scalar instantiation has a fixed native signature.
pub use hirsel_kernels::{integer, report, siphash, unicode, walk};
pub fn string_from_bytes(bytes: Vec<u8>) -> Result<String, Vec<u8>> {
    String::from_utf8(bytes).map_err(|e| e.into_bytes())
}
pub fn string_slice(src: &str, start: u64, end: u64) -> Option<&str> {
    if start > end {
        return None;
    }
    src.get(usize::try_from(start).ok()?..usize::try_from(end).ok()?)
}

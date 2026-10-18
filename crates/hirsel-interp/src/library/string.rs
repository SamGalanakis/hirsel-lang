//! UTF-8 scalar, position and slice adapters.
pub use hirsel_kernels::unicode::positions;
pub fn slice(s: &str, start: usize, end: usize) -> Option<&str> {
    if start > end {
        return None;
    }
    s.get(start..end)
}

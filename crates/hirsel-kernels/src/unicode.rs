pub fn positions(s: &str) -> impl Iterator<Item = usize> + '_ {
    s.char_indices()
        .map(|(i, _)| i)
        .chain(std::iter::once(s.len()))
}

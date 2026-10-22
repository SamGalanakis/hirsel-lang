/// `text`, a Debug rendering of a value, with each `TypeId(n)` that `serialize`
/// writes named by that type's canonical table instead, so two sessions'
/// interners compare.
pub fn canonical_ids(
    t: &hirsel_types::TypeInterner,
    serialize: impl FnOnce() -> Vec<u8>,
    mut text: String,
) -> String {
    let (_, ids) = hirsel_types::with_table_collection(serialize);
    for id in ids {
        let (table, _) = hirsel_types::encode_table(t, &[id]);
        let content = hirsel_syntax::ContentHash::of_bytes(format!("{table:?}").as_bytes());
        // The closing parenthesis keeps TypeId(1) from matching TypeId(12).
        text = text.replace(&format!("{id:?}"), &format!("Type{content:?}"));
    }
    text
}
/// `Anchor(n)` in a Debug rendering, replaced by that anchor's file path and base.
pub fn placed(sources: &hirsel_span::SourceMap, text: String) -> String {
    let mut out = String::new();
    let mut rest = text.as_str();
    while let Some(at) = rest.find("Anchor(") {
        out.push_str(&rest[..at]);
        rest = &rest[at + "Anchor(".len()..];
        let end = rest.find(')').unwrap();
        let anchor = hirsel_span::Anchor(rest[..end].parse().unwrap());
        let place = sources
            .anchor(anchor)
            .map(|(file, base)| (sources.file(file).unwrap().path.clone(), base));
        out.push_str(&format!("Anchor({place:?})"));
        rest = &rest[end + 1..];
    }
    out + rest
}

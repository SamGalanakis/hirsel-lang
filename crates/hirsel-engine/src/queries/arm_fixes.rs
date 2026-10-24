//! LANG-8's "no arm" repair: the checker names the arm a plain `?` needs, and
//! this places it as an edit at the end of the error type's declaration.
use super::*;
use hirsel_syntax::TypeBody;

impl QueryEngine {
    /// Attach each body's declaration edits to their diagnostics. A placed
    /// edit reads the declaration's text, so the check depends on it.
    pub(super) fn place_arm_fixes(
        &self,
        output: &mut QueryOutput<TypedBody>,
        representations: &mut BTreeMap<DefPath, ContentHash>,
    ) {
        place(output, representations, &self.sources, &self.items);
    }
}

pub(super) fn place(
    output: &mut QueryOutput<TypedBody>,
    representations: &mut BTreeMap<DefPath, ContentHash>,
    sources: &SourceMap,
    items: &BTreeMap<DefPath, Vec<Arc<ItemSyntax>>>,
) {
    place_with(output, representations, |owner, site| {
        insertion(sources, items, owner, site)
    });
}
pub(super) fn insertion(
    sources: &SourceMap,
    items: &BTreeMap<DefPath, Vec<Arc<ItemSyntax>>>,
    owner: &DefPath,
    site: Span,
) -> Option<(Span, ContentHash)> {
    Some((
        arm_insertion(sources, items, owner, site)?,
        items.get(owner)?.first()?.text_hash,
    ))
}
pub(super) fn place_with(
    output: &mut QueryOutput<TypedBody>,
    representations: &mut BTreeMap<DefPath, ContentHash>,
    mut insertion: impl FnMut(&DefPath, Span) -> Option<(Span, ContentHash)>,
) {
    for fix in std::mem::take(&mut output.value.plans.arm_fixes) {
        let Some((span, hash)) = insertion(&fix.owner, fix.site) else {
            continue;
        };
        let Some(diagnostic) = output
            .diagnostics
            .iter_mut()
            .find(|d| d.primary == fix.site && d.rule.as_str() == "LANG-8")
        else {
            continue;
        };
        let wire = if fix.serializable {
            " and its stored and wire schema (STORE-7, SER-2)"
        } else {
            ""
        };
        // The edit replaces the checker's prose for repair (a).
        diagnostic
            .notes
            .retain(|note| !note.starts_with("help: (a) "));
        diagnostic.fix = Some(hirsel_diag::Fix {
            span,
            replacement: format!(" | {}", fix.arm),
            message: format!(
                "(a) add the arm `{}` to `{}`; this changes the error contract of every function that returns it{wire}",
                fix.arm, fix.target
            ),
        });
        representations.insert(fix.owner, hash);
    }
}
/// Capture the existing eligible edit sites while S0 syntax is borrowed.
/// A worker later reads these exact source facts without retaining an AST.
pub(super) fn capture(
    sources: &SourceMap,
    items: &BTreeMap<DefPath, Vec<Arc<ItemSyntax>>>,
) -> BTreeMap<DefPath, (Span, FileId, ContentHash)> {
    items
        .iter()
        .filter_map(|(path, candidates)| {
            let syntax = candidates.first()?;
            let span = arm_insertion(sources, items, path, syntax.item.name_span)?;
            Some((
                path.clone(),
                (
                    span,
                    sources.resolve(syntax.item.name_span).file,
                    syntax.text_hash,
                ),
            ))
        })
        .collect()
}
/// The empty span just after the last arm of `owner`'s variant
/// declaration, when that declaration is in the file of `site` and ends
/// with its arms.
fn arm_insertion(
    sources: &SourceMap,
    items: &BTreeMap<DefPath, Vec<Arc<ItemSyntax>>>,
    owner: &DefPath,
    site: Span,
) -> Option<Span> {
    let snapshot = items.get(owner)?.first()?;
    let file = sources.resolve(site).file;
    if sources.anchor(snapshot.anchor)?.0 != file {
        return None;
    }
    let item = &snapshot.item;
    let ItemKind::Type {
        body: TypeBody::Variants(arms),
        drop: None,
        ..
    } = &item.kind
    else {
        return None;
    };
    let origin = item.origin.span;
    let end = sources.resolve(origin).end;
    let text = sources.file(file)?.text();
    let last = text.get(..end as usize)?.chars().next_back()?;
    (!arms.is_empty() && (last == ')' || last.is_alphanumeric() || last == '_'))
        .then(|| Span::new(origin.anchor, origin.end, origin.end))
        .flatten()
}

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
        for fix in std::mem::take(&mut output.value.plans.arm_fixes) {
            let Some(span) = self.arm_insertion(&fix.owner, fix.site.file) else {
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
            if let Some(hash) = self.implementation_hash(&fix.owner) {
                representations.insert(fix.owner, hash);
            }
        }
    }
    /// The empty span just after the last arm of `owner`'s variant
    /// declaration, when that declaration is in `file` and ends with its arms.
    fn arm_insertion(&self, owner: &DefPath, file: FileId) -> Option<Span> {
        let snapshot = self.snapshot(owner).ok()?;
        if snapshot.file.file != file {
            return None;
        }
        let item = &snapshot.file.tree.items[snapshot.item.0 as usize];
        let ItemKind::Type {
            body: TypeBody::Variants(arms),
            drop: None,
            ..
        } = &item.kind
        else {
            return None;
        };
        let end = item.origin.span.end;
        let text = self.sources.file(file)?.text();
        let last = text.get(..end as usize)?.chars().next_back()?;
        (!arms.is_empty() && (last == ')' || last.is_alphanumeric() || last == '_'))
            .then(|| Span::new(file, end, end))
            .flatten()
    }
}

//! Output-only source provenance guidance. No filename participates in lookup.
//!
//! Adapts rust-analyzer's unresolved_module.rs:10-31,63-84 at
//! 3131751eaf1c3b5da6e618acdc2148ad7db65bc7 (MIT OR Apache-2.0): explain
//! selected source provenance and distinguish single/multiple candidates.
//! PROG-1 replaces discovery/CreateFile fixes with explicit manifest mappings.
use super::*;

/// The resolver's missing-name diagnostics retain the exact lookup spelling.
/// Other LANG-4 errors (fields, ambiguous heads, etc.) are not filename evidence.
fn missing(diagnostic: &Diagnostic) -> Option<(Option<PackageId>, Vec<String>)> {
    if diagnostic.rule.as_str() != "LANG-4" {
        return None;
    }
    let text = diagnostic
        .message
        .strip_prefix("unresolved name `")
        .or_else(|| {
            diagnostic
                .message
                .strip_prefix("unresolved module member `")
        })?
        .split_once('`')?
        .0;
    let (package, text) = if let Some((package, text)) = text.split_once('/') {
        let (name, version) = package.split_once('@')?;
        (
            Some(PackageId {
                name: name.into(),
                version: version.into(),
            }),
            text,
        )
    } else {
        (None, text)
    };
    let segments: Vec<String> = text.split(['.', '/']).map(str::to_owned).collect();
    (segments.len() >= 2).then_some((package, segments))
}

pub(crate) fn append(
    files: &[(FileId, &FileContext)],
    sources: &SourceMap,
    declarations: &crate::snapshot::Declarations,
    diagnostics: &mut [Diagnostic],
) {
    // In particular, a successful check never traverses selected sources or
    // builds a package index merely to improve diagnostic prose.
    for diagnostic in diagnostics {
        let Some((package, segments)) = missing(diagnostic) else {
            continue;
        };
        let site = sources.resolve(diagnostic.primary);
        let Some((_, viewer)) = files.iter().find(|(file, _)| *file == site.file) else {
            continue;
        };
        let (member, modules) = segments.split_last().unwrap();
        let (stem, prefix) = modules.split_last().unwrap();
        let mut index = None;
        let mut candidates = BTreeSet::new();
        for (file, registered) in files {
            let Some(source) = sources.file(*file) else {
                continue;
            };
            if source.path.file_stem().and_then(|s| s.to_str()) != Some(stem.as_str())
                || package.as_ref().is_some_and(|p| *p != registered.package)
                || (!prefix.is_empty()
                    && registered.modules.split_last().map(|(_, p)| p) != Some(prefix))
            {
                continue;
            }
            let index = index.get_or_insert_with(|| declarations.index(viewer.package.clone()));
            let mut proposed = registered.modules.clone();
            proposed.push(member.clone());
            // lookup_spelling proves both membership and PROG-2 access using
            // declaration metadata only; access alone does not prove existence.
            let Ok(definition) =
                index.lookup_spelling(viewer, Some(&registered.package), &proposed)
            else {
                continue;
            };
            // The name of a public opaque type is accessible, its constructor
            // is not. This output seam has no type/value-use context, so avoid
            // suggesting a repair that could cross the representation boundary.
            if index
                .entry(&definition)
                .is_some_and(|entry| entry.visibility == hirsel_syntax::Visibility::PublicOpaque)
            {
                continue;
            }
            // Keep the spelling legal even under aliases, same-package
            // precedence or ambiguous dependency heads. No signature demand.
            let module = format!(
                "{}@{}/{}",
                registered.package.name,
                registered.package.version,
                registered.modules.join(".")
            );
            let repair = if registered.modules.is_empty() {
                format!("{module}{member}")
            } else {
                format!("{module}.{member}")
            };
            let Some(filename) = source.path.file_name().and_then(|s| s.to_str()) else {
                continue;
            };
            candidates.insert(format!(
                "`{filename}` is registered as module `{module}` (use `{repair}`)"
            ));
        }
        if candidates.is_empty() {
            continue;
        }
        // Multi-file modules deduplicate, distinct visible targets stay plural.
        // Bound output; neither private members nor denied internal paths count.
        let count = candidates.len();
        let candidates = candidates
            .into_iter()
            .take(4)
            .collect::<Vec<_>>()
            .join(", ");
        diagnostic.notes.push(format!(
            "help: selected source mappings: {candidates}{}; \
             filenames do not name modules",
            if count > 4 {
                ", and other visible candidates"
            } else {
                ""
            }
        ));
    }
}

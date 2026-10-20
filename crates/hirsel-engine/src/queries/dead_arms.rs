//! A package-local lint (LANG-8): an arm of the package's own error type that
//! no body of the package constructs. Declared error types otherwise collect
//! arms nothing returns, which every exhaustive match then has to handle.
use super::*;
use hirsel_syntax::{Punct, TokenKind, TypeBody, Visibility};
use hirsel_types::{CallTarget, ContextualName, NameSite, SignatureKind, TypeKind};

impl QueryEngine {
    /// Warnings for the never-constructed arms of the error types declared in
    /// `files`, one package's sources. An error type is a variant some
    /// function or closure of the package returns as its `Result` error.
    /// Only package-private types are linted: another package, or a source
    /// file several packages share, can construct a `pub` type's arms. Any
    /// body that did not check cleanly leaves the package unlinted.
    pub(crate) fn dead_arm_warnings(&mut self, files: &[FileId]) -> Vec<Diagnostic> {
        let definitions: Vec<DefPath> = files
            .iter()
            .flat_map(|file| self.file_definitions(*file))
            .collect();
        let mut bodies = vec![];
        let mut declarations = BTreeMap::new();
        for definition in &definitions {
            let Ok(snapshot) = self.snapshot(definition) else {
                return vec![];
            };
            let item = &snapshot.item;
            match &item.kind {
                ItemKind::Type {
                    body: TypeBody::Variants(arms),
                    ..
                } if !arms.is_empty() && item.visibility == Visibility::Private => {
                    declarations.insert(definition.clone(), snapshot.clone());
                }
                ItemKind::Function { body: Some(_), .. } => bodies.push(definition.clone()),
                ItemKind::Const { .. } => bodies.push(definition.clone()),
                _ => {}
            }
        }
        if declarations.is_empty() {
            return vec![];
        }
        let mut errors = BTreeSet::new();
        let mut constructed = BTreeSet::new();
        let types = self.types.clone();
        for definition in &bodies {
            if let Ok(signature) = self.signature(DefId::intern(definition))
                && let SignatureKind::Function(f) = &signature.kind
                && let Some(error) = f.error
                && let Some(TypeKind::Named { definition, .. }) = types.get(error).map(|t| &t.kind)
            {
                errors.insert(definition.path().clone());
            }
            let Ok(checked) = self.check_query(definition) else {
                return vec![];
            };
            if has_errors(&checked.diagnostics) {
                return vec![];
            }
            let body = &checked.value;
            for plan in &body.plans.tries {
                if let Some(TypeKind::Named { definition, .. }) =
                    types.get(plan.error_type).map(|t| &t.kind)
                {
                    errors.insert(definition.path().clone());
                }
            }
            let mappers = body.plans.tries.iter().filter_map(|p| p.mapper.as_ref());
            let calls = body.checked.calls.iter().map(|c| &c.target);
            for target in mappers.chain(calls) {
                if let CallTarget::Constructor(ContextualName::Variant { owner, variant }) = target
                {
                    constructed.insert((owner.clone(), variant.0));
                }
            }
            for (site, name) in &body.checked.resolution.contextual {
                if let (NameSite::Expression(_), ContextualName::Variant { owner, variant }) =
                    (site, name)
                {
                    constructed.insert((owner.clone(), variant.0));
                }
            }
        }
        let mut warnings = vec![];
        for (owner, snapshot) in declarations {
            let item = &snapshot.item;
            if !errors.contains(&owner) {
                continue;
            }
            let ItemKind::Type {
                body: TypeBody::Variants(declared),
                ..
            } = &item.kind
            else {
                continue;
            };
            let names = arm_names(&snapshot);
            if names.len() != declared.len() {
                continue;
            }
            for (index, (name, span)) in names.into_iter().enumerate() {
                if constructed.contains(&(owner.clone(), index as u32)) {
                    continue;
                }
                let mut d = Diagnostic::error(
                    const { RuleId::of("LANG-8") },
                    span,
                    format!(
                        "arm `{name}` of error type `{}` is never constructed in this package",
                        owner.0.name
                    ),
                );
                d.severity = Severity::Warning;
                d.notes.push(format!(
                    "help: remove `{name}` if nothing fails that way any more (callers' matches lose an arm), or construct it where that failure occurs"
                ));
                warnings.push(d);
            }
        }
        warnings
    }
}

/// Each arm's name and site in a variant declaration: the identifiers that
/// follow its `=` and each top-level `|`.
fn arm_names(syntax: &ItemSyntax) -> Vec<(String, Span)> {
    let item = &syntax.item;
    let tokens =
        &syntax.tree.tokens[item.origin.tokens.start as usize..item.origin.tokens.end as usize];
    let mut arms = vec![];
    let mut depth = 0i32;
    let mut started = false;
    let mut expecting = false;
    for token in tokens {
        match token.kind {
            TokenKind::Trivia(_) => continue,
            TokenKind::Punct(Punct::LParen | Punct::LBracket | Punct::LBrace) => depth += 1,
            TokenKind::Punct(Punct::RParen | Punct::RBracket | Punct::RBrace) => depth -= 1,
            TokenKind::Punct(Punct::Eq) if depth == 0 && !started => {
                started = true;
                expecting = true;
                continue;
            }
            TokenKind::Punct(Punct::Pipe) if depth == 0 && started => {
                expecting = true;
                continue;
            }
            TokenKind::Ident(name) if expecting && depth == 0 => {
                arms.push((name.as_str().to_owned(), token.span));
            }
            _ => {}
        }
        expecting = false;
    }
    arms
}

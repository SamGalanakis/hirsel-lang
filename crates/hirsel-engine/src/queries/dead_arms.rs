//! A package-local lint (LANG-8): an arm of the package's own error type that
//! no body of the package constructs. Declared error types otherwise collect
//! arms nothing returns, which every exhaustive match then has to handle.
use super::*;
use hirsel_syntax::{Punct, TokenKind};
use hirsel_types::{CallTarget, ContextualName, NameSite, SignatureKind, TypeKind};

impl QueryEngine {
    /// Warnings for the never-constructed arms of the error types declared in
    /// `files`, one package's sources. An error type is a variant some
    /// function or closure of the package returns as its `Result` error.
    /// Only package-private types are linted: another package, or a source
    /// file several packages share, can construct a `pub` type's arms. Any
    /// body that did not check cleanly leaves the package unlinted.
    pub(crate) fn dead_arm_warnings(&mut self, files: &[FileId]) -> Vec<Diagnostic> {
        let mut bodies = vec![];
        let mut declarations = BTreeMap::new();
        for file in files {
            for item in self.source_items(*file) {
                self.work.lint_definition_visits += 1;
                if !item.facts.arms.is_empty() {
                    declarations.insert(item.definition.clone(), (item.anchor, item.facts.arms));
                }
                if item.facts.body {
                    bodies.push(item.definition);
                }
            }
        }
        if declarations.is_empty() {
            return vec![];
        }
        self.invalidate_front_products();
        let mut errors = BTreeSet::new();
        let mut constructed = BTreeSet::new();
        for definition in &bodies {
            self.work.lint_body_visits += 1;
            let Some(inputs) = self.dead_arm_use(definition) else {
                return vec![];
            };
            errors.extend(inputs.errors);
            constructed.extend(inputs.constructed);
        }
        let mut warnings = vec![];
        for (owner, (anchor, names)) in declarations {
            if !errors.contains(&owner) {
                continue;
            }
            for (index, (name, start, end)) in names.into_iter().enumerate() {
                let span = Span { anchor, start, end };
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
    fn dead_arm_use(&mut self, definition: &DefPath) -> Option<crate::tools::DeadArmUse> {
        let id = DefId::intern(definition);
        if let Some(inputs) = self.front_products.dead_arms.get(&id) {
            return Some(inputs.clone());
        }
        let signature = self.signature(id).ok();
        let checked = self.check_query(definition).ok()?;
        if has_errors(&checked.diagnostics) {
            return None;
        }
        let inputs = body_use(&self.types, signature.as_deref(), &checked.value);
        if self
            .checked
            .get(&id)
            .is_some_and(|entry| entry.consistent && !self.graph.is_dirty(&entry.memo.key))
            && self
                .checked_summaries
                .get(&id)
                .is_some_and(|entry| entry.consistent && !self.graph.is_dirty(&entry.memo.key))
        {
            self.front_products.dead_arms.insert(id, inputs.clone());
        }
        Some(inputs)
    }
}

/// The exact H facts used by the package lint, captured before H is released.
pub(super) fn body_use(
    types: &hirsel_types::TypeInterner,
    signature: Option<&Signature>,
    body: &TypedBody,
) -> crate::tools::DeadArmUse {
    let mut inputs = crate::tools::DeadArmUse::default();
    if let Some(signature) = signature
        && let SignatureKind::Function(f) = &signature.kind
        && let Some(error) = f.error
        && let Some(TypeKind::Named { definition, .. }) = types.get(error).map(|t| &t.kind)
    {
        inputs.errors.insert(definition.path().clone());
    }
    for plan in &body.plans.tries {
        if let Some(TypeKind::Named { definition, .. }) =
            types.get(plan.error_type).map(|t| &t.kind)
        {
            inputs.errors.insert(definition.path().clone());
        }
    }
    let mappers = body.plans.tries.iter().filter_map(|p| p.mapper.as_ref());
    let calls = body.checked.calls.iter().map(|c| &c.target);
    for target in mappers.chain(calls) {
        if let CallTarget::Constructor(ContextualName::Variant { owner, variant }) = target {
            inputs.constructed.insert((owner.clone(), variant.0));
        }
    }
    for (site, name) in &body.checked.resolution.contextual {
        if let (NameSite::Expression(_), ContextualName::Variant { owner, variant }) = (site, name)
        {
            inputs.constructed.insert((owner.clone(), variant.0));
        }
    }
    inputs
}

/// Each arm's name and site in a variant declaration: the identifiers that
/// follow its `=` and each top-level `|`.
pub(super) fn arm_names(syntax: &ItemSyntax) -> Vec<(String, Span)> {
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

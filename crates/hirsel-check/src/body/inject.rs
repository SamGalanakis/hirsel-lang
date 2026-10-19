//! LANG-8 payload-directed injection: a plain `e?` whose error differs from the
//! enclosing one wraps it through the enclosing variant's one unary arm whose
//! nominal payload is that error. The arm is selected once, here, against the
//! declared and possibly abstract types; instantiation never re-selects it.
use super::*;

/// An arm of the enclosing error that plain `?` may inject through.
struct Arm {
    index: usize,
    name: String,
    payload: TypeId,
}

impl Checker<'_, '_> {
    /// A nominal error type: a named definition or a type parameter. A
    /// primitive (`String`, `Bytes`, `Never`), tuple, function or builtin
    /// type names no error domain, so its identity is no evidence of a role.
    /// Aliases are followed through their declarations' own stores, so the
    /// test imports no type row and leaves the body's TIR as `? Arm` has it.
    fn nominal_error(&mut self, ty: TypeId) -> Result<bool, QueryError> {
        let TypeKind::Named { mut definition, .. } = self.kind(ty) else {
            return Ok(matches!(self.kind(ty), TypeKind::Parameter(_)));
        };
        let mut seen = BTreeSet::new();
        while seen.insert(definition.clone()) {
            let s = match self.snapshot(&definition) {
                Ok(s) => s,
                Err(QueryError::Diagnostics(_)) => return Ok(false),
                Err(e) => return Err(e),
            };
            let target = match &s.kind {
                SignatureKind::Type(t) if s.metadata.alias => t
                    .representation
                    .and_then(|r| s.types.get(r))
                    .map(|r| r.kind.clone()),
                _ => return Ok(true),
            };
            match target {
                Some(TypeKind::Named {
                    definition: next, ..
                }) => definition = next,
                _ => return Ok(false),
            }
        }
        Ok(false)
    }
    /// The `?` token of a plain `e?`, where every injection diagnostic points.
    fn question_mark(&self, id: ast::ExprId) -> Span {
        let span = self.span(id);
        Span::new(
            span.anchor,
            span.end.saturating_sub(1).max(span.start),
            span.end,
        )
        .unwrap_or(span)
    }
    /// The constructor of the unique injectable arm of `expected` carrying
    /// `error`, or None after reporting LANG-8.
    pub(super) fn inject(
        &mut self,
        id: ast::ExprId,
        value: ast::ExprId,
        error: TypeId,
        expected: TypeId,
    ) -> Result<Option<CallTarget>, QueryError> {
        const RULE: RuleId = RuleId::of("LANG-8");
        let (owner, representation) = self.representation(expected)?;
        let (Some(owner), TypeKind::Variant(variants)) = (owner, self.kind(representation)) else {
            // Not a visible variant (a parameter, an opaque foreign type, a
            // record): only identity applies.
            self.type_mismatch(
                RULE,
                self.span(id),
                "plain ? requires identical error types",
                error,
                expected,
            );
            return Ok(None);
        };
        let mut injectable = vec![];
        let mut carriers = vec![];
        for (index, variant) in variants.iter().enumerate() {
            let [(_, payload)] = variant.fields[..] else {
                continue;
            };
            if self.invalid(payload) {
                continue;
            }
            let arm = Arm {
                index,
                name: variant.name.clone(),
                payload,
            };
            if !self.nominal_error(payload)? {
                if self.compatible(payload, error) {
                    carriers.push(arm.name.clone());
                }
                continue;
            }
            injectable.push(arm);
        }
        let [source, target] = hirsel_type_analysis::spelling::type_spellings_apart(
            &self.body.types,
            [error, expected],
            &self.body.resolution.generic_declarations,
            &self.input.signature.definition.0.package,
            false,
        );
        let site = self.question_mark(id);
        let mut d = Diagnostic::error(RULE, site, "");
        d.secondary.push(hirsel_diag::Label {
            span: self.span(value),
            message: format!("propagates `{source}`"),
        });
        if !self.nominal_error(error)? {
            d.message = format!(
                "plain ? does not inject `{source}` errors: `{source}` is not a nominal error type"
            );
            d.notes.push(match carriers.as_slice() {
                [] => format!(
                    "help: map it explicitly with `? f`, a function from `{source}` to `{target}`, or handle it here"
                ),
                arms => format!(
                    "help: name the arm that carries it: {}",
                    arms.iter()
                        .map(|a| format!("`? {a}`"))
                        .collect::<Vec<_>>()
                        .join(" or ")
                ),
            });
            self.diagnostics.push(d);
            return Ok(None);
        }
        let matching: Vec<&Arm> = injectable
            .iter()
            .filter(|arm| self.compatible(arm.payload, error))
            .collect();
        match matching.as_slice() {
            [arm] => {
                return Ok(Some(CallTarget::Constructor(ContextualName::Variant {
                    owner,
                    variant: VariantId(arm.index as u32),
                })));
            }
            [] => {}
            arms => {
                d.message = format!(
                    "plain ? is ambiguous: `{source}` is the payload of {} arms of `{target}`",
                    arms.len()
                );
                d.notes.push(format!(
                    "candidate arms: {}",
                    arms.iter()
                        .map(|a| format!("{}({source})", a.name))
                        .collect::<Vec<_>>()
                        .join(", ")
                ));
                d.notes.push(format!(
                    "help: name the arm, since each one is a different role: {}",
                    arms.iter()
                        .map(|a| format!("`? {}`", a.name))
                        .collect::<Vec<_>>()
                        .join(" or ")
                ));
                self.diagnostics.push(d);
                return Ok(None);
            }
        }
        // No arm: offer (a) widening the declaration, (b) handling the error
        // here and (c) an explicit mapper. Only (a) is a mechanical edit.
        d.message = format!("plain ? has no arm for `{source}` in `{target}`");
        d.notes.push(match injectable.as_slice() {
            [] => format!("`{target}` has no arm with a single nominal payload"),
            arms => format!(
                "`{target}` has injectable arms {}",
                arms.iter()
                    .map(|a| format!("{}({})", a.name, self.type_spelling(a.payload)))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        });
        let name = arm_name(&source);
        let free = name
            .as_ref()
            .filter(|n| variants.iter().all(|v| v.name != **n));
        let declaration = self.snapshot(&owner)?;
        let serializable = matches!(&declaration.kind, SignatureKind::Type(t) if t.serializable);
        let local = owner.0.package == self.input.signature.definition.0.package;
        let arm = free.map(|n| format!("{n}({source})"));
        match &arm {
            Some(arm) => d.notes.push(format!(
                "help: (a) add the arm `{arm}` to `{target}`; this changes the error contract of every function that returns `{target}`"
            )),
            None => d.notes.push(format!(
                "help: (a) add an arm carrying `{source}` to `{target}`; this changes the error contract of every function that returns `{target}`"
            )),
        }
        if serializable {
            d.notes.push(format!(
                "`{target}` is serializable: adding an arm changes its stored and wire schema (STORE-7, SER-2)"
            ));
        }
        d.notes.push(
            "help: (b) handle the error at this site, with `match` or `let Ok(..) = .. else { .. }`"
                .into(),
        );
        d.notes.push(format!(
            "help: (c) map it explicitly with `? C`, where C is a function from `{source}` to `{target}` or an arm whose payload it is"
        ));
        // The declaration edit is offered only where it is spelled the same
        // way: a transparent type of this package, an error that names no
        // parameter of this definition, and a free arm name. The engine
        // places it, and only when the declaration is in this file.
        if let Some(arm) = arm
            && local
            && !hirsel_types::mentions_parameter(&self.body.types, error)
        {
            self.plans.arm_fixes.push(ArmFix {
                site,
                owner,
                arm,
                target,
                serializable,
            });
        }
        self.diagnostics.push(d);
        Ok(None)
    }
}

/// The arm name the "no arm" fix proposes: the payload type's last segment,
/// without a trailing `Error` when something remains (`FileError` gives
/// `File`, `std@1/http.HttpError` gives `Http`, `bufio.ReadError[E]` gives
/// `Read`). Core variant names are never proposed.
pub(super) fn arm_name(spelling: &str) -> Option<String> {
    let head = spelling.split('[').next()?;
    let last = head.rsplit(['.', '/']).next()?;
    let name = match last.strip_suffix("Error") {
        Some(stem) if !stem.is_empty() => stem,
        _ => last,
    };
    let valid = name.chars().next().is_some_and(|c| c.is_ascii_uppercase())
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
    (valid && !matches!(name, "Ok" | "Error" | "Some" | "None")).then(|| name.to_owned())
}

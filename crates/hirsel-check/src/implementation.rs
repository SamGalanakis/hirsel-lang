//! The one implementation matcher (GEN-6): an implementation's member actuals
//! against its family's members. Package implementation slots call it too.
//!
//! The structure follows OCaml's signature inclusion
//! (`typing/includemod.ml:711-820`): members pair by name and kind, every type
//! and size member enters the substitution first, and each operation is then
//! checked against its header instantiated at those actuals. Only implications
//! the existing callable relation proves are accepted; anything else is a
//! refusal naming the member, so an incompatible target needs an explicit
//! adapter function.
use hirsel_diag::RuleId;
use hirsel_syntax::{DefId, DefPath, PassingMode};
use hirsel_types::*;

fn internal() -> QueryError {
    QueryError::Operational(OperationalFailure {
        operation: "implementation matcher".into(),
        message: "a family or target declaration is malformed".into(),
    })
}
/// Why a member does not implement its family's member.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Refusal {
    pub member: String,
    pub rule: RuleId,
    pub message: String,
}
/// A matched implementation: the family's generic actuals (type and size
/// members in order) and each operation's exact target with the generic
/// actuals that instantiate it at the member's header.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CheckedImplementation {
    pub arguments: Vec<GenericArgument>,
    pub operations: Vec<(String, DefPath, Vec<GenericArgument>)>,
}

/// Match `actuals` against `family` (GEN-6). `Ok(Err(refusal))` names the
/// first member in family order that cannot be shown to implement its
/// specification; provider failures propagate.
pub fn match_implementation(
    types: &TypeInterner,
    memo: &TypeMemo,
    family: &DefPath,
    actuals: &ImplementationSignature,
    provider: &mut dyn SignatureProvider,
) -> Result<Result<CheckedImplementation, Refusal>, QueryError> {
    let refuse = |member: &str, rule: RuleId, message: String| {
        Ok(Err(Refusal {
            member: member.to_owned(),
            rule,
            message,
        }))
    };
    let family_signature = provider.signature(DefId::intern(family))?;
    let SignatureKind::Family(declared) = &family_signature.kind else {
        return refuse(
            "",
            const { RuleId::of("GEN-6") },
            format!("`{family}` is not a signature"),
        );
    };
    if actuals.family != DefId::intern(family) {
        return refuse(
            "",
            const { RuleId::of("GEN-6") },
            format!(
                "the implementation implements `{}`, not `{family}`",
                actuals.family
            ),
        );
    }
    // Pair by name and kind (includemod.ml `pair_components`).
    for member in &declared.members {
        let paired = match (&member.kind, actuals.member(&member.name)) {
            (FamilyMemberKind::Type { .. }, Some(MemberActual::Type(_)))
            | (FamilyMemberKind::Size, Some(MemberActual::Size(_)))
            | (FamilyMemberKind::Operation, Some(MemberActual::Operation(_))) => true,
            _ => false,
        };
        if !paired {
            return refuse(
                &member.name,
                const { RuleId::of("GEN-6") },
                format!("member `{}` is missing or of another kind", member.name),
            );
        }
    }
    let arguments = actuals.arguments();
    let roles = family_signature.metadata.roles.clone();
    let mut lookup = |d: DefId| provider.signature(d).map(Some);
    let generics = GenericDeclarations::default();
    let mut context = ContextMemo::new(types, &generics, &mut lookup);
    // Types first: each actual satisfies its declared bounds.
    for member in &declared.members {
        let (FamilyMemberKind::Type { bounds }, Some(MemberActual::Type(actual))) =
            (&member.kind, actuals.member(&member.name))
        else {
            continue;
        };
        for bound in bounds.iter() {
            let holds = match bound {
                hirsel_syntax::Bound::Inert => matches!(
                    context.authority(*actual, &roles)?,
                    AuthoritySummary::Known(terms) if terms.is_empty()
                ),
                _ => context.prove(*actual, *bound, STRUCTURAL)? == Sendability::Sendable,
            };
            if !holds {
                return refuse(
                    &member.name,
                    if *bound == hirsel_syntax::Bound::Inert {
                        const { RuleId::of("CAP-2") }
                    } else {
                        const { RuleId::of("GEN-1") }
                    },
                    format!(
                        "type member `{}` lacks its required bound `{}`",
                        member.name,
                        format!("{bound:?}").to_lowercase()
                    ),
                );
            }
        }
    }
    // Then operations against their instantiated headers.
    let mut operations = vec![];
    for member in &declared.members {
        let (FamilyMemberKind::Operation, Some(MemberActual::Operation(target))) =
            (&member.kind, actuals.member(&member.name))
        else {
            continue;
        };
        let header_path = family_operation_path(family, &member.name);
        let header = context
            .signature(DefId::intern(&header_path))?
            .ok_or_else(internal)?;
        let SignatureKind::Function(header) = &header.kind else {
            return Err(internal());
        };
        let wanted = instantiate_function(types, memo, header, &arguments, &roles)?;
        let target_signature = context
            .signature(DefId::intern(target))?
            .ok_or_else(internal)?;
        let SignatureKind::Function(actual) = &target_signature.kind else {
            return refuse(
                &member.name,
                const { RuleId::of("GEN-6") },
                format!(
                    "operation `{}` names `{target}`, which is not a function",
                    member.name
                ),
            );
        };
        // A generic target is instantiated from the complete header (GEN-1).
        let mut inferred = vec![None; actual.generics.len()];
        if wanted.parameters.len() == actual.parameters.len() {
            for (declared, wanted) in actual.parameters.iter().zip(wanted.parameters.iter()) {
                infer(types, declared.ty, wanted.ty, 0, &mut inferred);
            }
            infer(types, actual.result, wanted.result, 0, &mut inferred);
        }
        let Some(target_arguments) = inferred.into_iter().collect::<Option<Vec<_>>>() else {
            return refuse(
                &member.name,
                const { RuleId::of("LANG-2") },
                format!(
                    "the header of `{}` does not determine every generic parameter of `{target}`",
                    member.name
                ),
            );
        };
        let found_function = instantiate_function(
            types,
            memo,
            actual,
            &target_arguments,
            &target_signature.metadata.roles,
        )?;
        let found = named_row(types, &found_function);
        if let Some(refusal) = classify(
            types,
            &mut context,
            &member.name,
            target,
            &wanted,
            &found_function,
        )? {
            return Ok(Err(refusal));
        }
        let expected = named_row(types, &wanted);
        if !context.compatible(expected, found, TypeRelation::Value, CompatPolicy::RECEIVED)? {
            return refuse(
                &member.name,
                const { RuleId::of("GEN-6") },
                format!(
                    "`{target}` cannot stand where operation `{}` is specified; write an adapter",
                    member.name
                ),
            );
        }
        operations.push((member.name.clone(), target.clone(), target_arguments));
    }
    Ok(Ok(CheckedImplementation {
        arguments,
        operations,
    }))
}

/// A precise refusal for the first difference the callable relation would
/// reject, so each kind cites its own rule. None leaves the verdict to the
/// relation itself.
fn classify(
    types: &TypeInterner,
    context: &mut ContextMemo<'_>,
    member: &str,
    target: &DefPath,
    wanted: &FunctionType,
    found: &FunctionType,
) -> Result<Option<Refusal>, QueryError> {
    let refusal = |rule: RuleId, message: String| {
        Ok(Some(Refusal {
            member: member.to_owned(),
            rule,
            message,
        }))
    };
    if wanted.parameters.len() != found.parameters.len() {
        return refusal(
            const { RuleId::of("GEN-6") },
            format!(
                "`{target}` takes {} parameters; operation `{member}` takes {}",
                found.parameters.len(),
                wanted.parameters.len()
            ),
        );
    }
    for (i, (w, f)) in wanted
        .parameters
        .iter()
        .zip(found.parameters.iter())
        .enumerate()
    {
        if w.passing != f.passing {
            let spell = |p: PassingMode| match p {
                PassingMode::Borrow => "borrowed",
                PassingMode::Take => "take",
                PassingMode::Mut => "mut",
            };
            return refusal(
                const { RuleId::of("OWN-1") },
                format!(
                    "parameter {} of `{target}` is {}; operation `{member}` declares it {}",
                    i + 1,
                    spell(f.passing),
                    spell(w.passing)
                ),
            );
        }
        if !types.shape_eq(w.ty, f.ty).unwrap_or(false) {
            return refusal(
                const { RuleId::of("GEN-6") },
                format!(
                    "parameter {} of `{target}` has another type than operation `{member}`'s",
                    i + 1
                ),
            );
        }
    }
    if !types.shape_eq(wanted.result, found.result).unwrap_or(false) {
        return refusal(
            const { RuleId::of("GEN-6") },
            format!("`{target}` returns another type than operation `{member}`"),
        );
    }
    if wanted.allocation == Allocation::NoAlloc && found.allocation != Allocation::NoAlloc {
        return refusal(
            const { RuleId::of("MEM-5") },
            format!("operation `{member}` is @noalloc; `{target}` may allocate"),
        );
    }
    let effective = |f: &FunctionType| match &f.uses {
        Uses::Only(allowed) => allowed.clone(),
        Uses::Unrestricted => f.authority.clone(),
    };
    if !effective(found).is_subset_of(&effective(wanted)) {
        return refusal(
            const { RuleId::of("CAP-2") },
            format!("`{target}` uses authority operation `{member}` does not grant"),
        );
    }
    if found
        .contracts
        .iter()
        .any(|c| c.kind == hirsel_syntax::ContractKind::Requires)
        || wanted
            .contracts
            .iter()
            .any(|c| c.kind == hirsel_syntax::ContractKind::Ensures)
    {
        return refusal(
            const { RuleId::of("FACT-1") },
            format!(
                "the contracts of `{target}` are not implied by operation `{member}`: a target cannot require more, nor a header ensure more"
            ),
        );
    }
    // Parameters are received by the target; the result by the caller.
    let mut pairs: Vec<(TypeId, TypeId, TypeRelation, String)> = wanted
        .parameters
        .iter()
        .zip(found.parameters.iter())
        .enumerate()
        .map(|(i, (w, f))| {
            (
                f.ty,
                w.ty,
                TypeRelation::Parameter,
                format!("parameter {}", i + 1),
            )
        })
        .collect();
    pairs.push((
        wanted.result,
        found.result,
        TypeRelation::Value,
        "result".into(),
    ));
    for (expected, actual, relation, place) in pairs {
        if context.compatible(expected, actual, relation, CompatPolicy::RECEIVED)? {
            continue;
        }
        let (e, a) = (types.row(expected), types.row(actual));
        let (rule, what) = if e.modes.relation != a.modes.relation {
            (const { RuleId::of("MODE-1") }, "result relation")
        } else if e.brand != a.brand {
            (const { RuleId::of("PROV-1") }, "brand")
        } else {
            (const { RuleId::of("MODE-2") }, "mode")
        };
        return refusal(
            rule,
            format!("the {what} of {place} of `{target}` does not meet operation `{member}`"),
        );
    }
    Ok(None)
}

/// `f`'s header at `arguments` for its own binder, as a direct call or a
/// function value instantiates it.
fn instantiate_function(
    types: &TypeInterner,
    memo: &TypeMemo,
    f: &FunctionType,
    arguments: &[GenericArgument],
    roles: &NominalRoles,
) -> Result<FunctionType, QueryError> {
    let substitution = Substitution {
        binder: Some(GenericBinder::Bound(0)),
        arguments,
        authorities: &vec![AuthoritySummary::Unknown; arguments.len()],
        roles,
    };
    let at = |ty| substitute(types, memo, ty, &substitution).map_err(|_| internal());
    let mut f = f.clone();
    let mut parameters = f.parameters.to_vec();
    for parameter in &mut parameters {
        parameter.ty = at(parameter.ty)?;
    }
    f.parameters = parameters.into();
    f.result = at(f.result)?;
    f.error = f.error.map(at).transpose()?;
    f.generics = vec![].into();
    Ok(f)
}
fn function_row(types: &TypeInterner, f: &FunctionType) -> TypeId {
    types.intern(Type {
        drop_allocation: DropAllocation::MayAllocate,
        ..Type::with_kind(TypeKind::Function(f.clone()))
    })
}
/// A declared function as a value: its uses and authority agree (spec #58),
/// with no environment.
fn named_row(types: &TypeInterner, f: &FunctionType) -> TypeId {
    let mut f = f.clone();
    match &f.uses {
        Uses::Unrestricted => f.uses = Uses::Only(f.authority.clone()),
        Uses::Only(allowed) => f.authority = allowed.clone(),
    }
    f.generics = vec![].into();
    let mut row = types.row(function_row(types, &f)).clone();
    row.modes.sendability = Sendability::Sendable;
    types.intern(row)
}

/// Structural first-order inference of `template`'s binder-`depth` parameters
/// from `actual`; a parameter met twice keeps its first actual.
fn infer(
    types: &TypeInterner,
    template: TypeId,
    actual: TypeId,
    depth: u32,
    inferred: &mut [Option<GenericArgument>],
) {
    let arguments =
        |a: &[GenericArgument], b: &[GenericArgument], inferred: &mut [Option<GenericArgument>]| {
            for (a, b) in a.iter().zip(b) {
                match (a, b) {
                    (GenericArgument::Type(a), GenericArgument::Type(b)) => {
                        infer(types, *a, *b, depth, inferred)
                    }
                    (GenericArgument::Size(Size::Parameter(p)), GenericArgument::Size(size))
                        if p.binder() == GenericBinder::Bound(depth) =>
                    {
                        if let Some(slot @ None) = inferred.get_mut(p.parameter().0 as usize) {
                            *slot = Some(GenericArgument::Size(size.clone()));
                        }
                    }
                    _ => {}
                }
            }
        };
    match (&types.row(template).kind, &types.row(actual).kind) {
        (TypeKind::Parameter(p), _) if p.binder() == GenericBinder::Bound(depth) => {
            if let Some(slot @ None) = inferred.get_mut(p.parameter().0 as usize) {
                *slot = Some(GenericArgument::Type(actual));
            }
        }
        (TypeKind::Tuple(a), TypeKind::Tuple(b)) if a.len() == b.len() => {
            for (a, b) in a.iter().zip(b.iter()) {
                infer(types, *a, *b, depth, inferred);
            }
        }
        (TypeKind::Box(a), TypeKind::Box(b)) => infer(types, *a, *b, depth, inferred),
        (
            TypeKind::Named {
                definition: x,
                arguments: a,
            },
            TypeKind::Named {
                definition: y,
                arguments: b,
            },
        ) if x == y => arguments(a, b, inferred),
        (
            TypeKind::Builtin {
                kind: x,
                arguments: a,
            },
            TypeKind::Builtin {
                kind: y,
                arguments: b,
            },
        ) if x == y => arguments(a, b, inferred),
        (TypeKind::Function(a), TypeKind::Function(b))
            if a.parameters.len() == b.parameters.len() =>
        {
            for (a, b) in a.parameters.iter().zip(b.parameters.iter()) {
                infer(types, a.ty, b.ty, depth + 1, inferred);
            }
            infer(types, a.result, b.result, depth + 1, inferred);
        }
        _ => {}
    }
}

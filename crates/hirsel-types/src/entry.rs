//! The closed PROG-3 entry return contract. Nominal identity prevents lookalikes.
use crate::{GenericArgument, TypeId, TypeInterner, TypeKind};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EntryReturn {
    UnitResult,
    ExitCode,
    ExitCodeResult,
}

pub fn is_exit_code(types: &TypeInterner, ty: TypeId) -> bool {
    matches!(types.get(ty).map(|t| &t.kind),
        Some(TypeKind::Named { definition, arguments })
        if definition.path().package_is("std", "1")
            && definition.path().0.modules.as_ref() == ["process"]
            && definition.path().0.name == "ExitCode" && arguments.is_empty())
}

pub fn entry_return(types: &TypeInterner, ty: TypeId) -> Option<EntryReturn> {
    if is_exit_code(types, ty) {
        return Some(EntryReturn::ExitCode);
    }
    let TypeKind::Named {
        definition,
        arguments,
    } = &types.get(ty)?.kind
    else {
        return None;
    };
    let [GenericArgument::Type(ok), GenericArgument::Type(_)] = arguments.as_ref() else {
        return None;
    };
    if !definition.path().is_core("Result") {
        return None;
    }
    if is_exit_code(types, *ok) {
        Some(EntryReturn::ExitCodeResult)
    } else if matches!(types.get(*ok).map(|t| &t.kind), Some(TypeKind::Tuple(fields)) if fields.is_empty())
    {
        Some(EntryReturn::UnitResult)
    } else {
        None
    }
}

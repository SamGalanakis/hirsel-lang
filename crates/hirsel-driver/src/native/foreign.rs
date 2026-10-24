//! Extern dispatch (S-13): each extern's `hirsel_rt_v2_foreign_enter` dispatch word, from its
//! package's trusted `threads` and its `@blocking`, and the build check that a callback which
//! can reach a `home` package is handed only to a `home` package's extern.
use super::queries::root_source::LinkSource;
use super::stages::{NativeError, ReadyProgram, StageName};
use hirsel_lir::runtime_abi::{
    FOREIGN_BLOCKING, FOREIGN_PACKAGE_SHIFT, FOREIGN_THREADS_ANY, FOREIGN_THREADS_HOME,
    FOREIGN_THREADS_MASK, FOREIGN_THREADS_SERIAL,
};
use hirsel_syntax::DefPath;
use hirsel_types::{
    BuiltinType, ForeignThreads, FunctionType, GenericArgument, SignatureKind, Type, TypeId,
    TypeInterner, TypeKind,
};
use std::collections::{BTreeMap, BTreeSet};

/// Each extern's dispatch word: its package's `threads`, its `@blocking` and its package's
/// ordinal among the program's `unsafe ffi` packages, which names the package's `serial`
/// gate in the run.
pub(crate) fn dispatch(input: &LinkSource<'_>) -> BTreeMap<DefPath, u64> {
    let ordinals: BTreeMap<_, u64> = input
        .foreign_threads
        .keys()
        .enumerate()
        .map(|(ordinal, package)| (package, ordinal as u64))
        .collect();
    input
        .signatures
        .iter()
        .filter_map(|(path, signature)| {
            let SignatureKind::Function(function) = &signature.kind else {
                return None;
            };
            let contract = function.foreign.as_ref()?;
            let package = &path.0.package;
            let threads = match threads(input, path) {
                ForeignThreads::Home => FOREIGN_THREADS_HOME,
                ForeignThreads::Serial => FOREIGN_THREADS_SERIAL,
                ForeignThreads::Any => FOREIGN_THREADS_ANY,
            };
            let blocking = if contract.blocking {
                FOREIGN_BLOCKING
            } else {
                0
            };
            let ordinal = ordinals.get(package).copied().unwrap_or(0);
            Some((
                path.clone(),
                threads | blocking | ordinal << FOREIGN_PACKAGE_SHIFT,
            ))
        })
        .collect()
}

fn threads(input: &LinkSource<'_>, path: &DefPath) -> ForeignThreads {
    input
        .foreign_threads
        .get(&path.0.package)
        .copied()
        .unwrap_or_default()
}

/// S-13's nesting check: a callback runs on the thread of the extern call it was handed to,
/// and a `home` package's externs run only on the home thread, so a callback that can reach
/// a `home` package's extern may be handed only to a `home` package's extern. A callback may
/// be handed to an extern when its function type is the callback type of one of the
/// extern's parameters (a `FnPtr`, also inside an `Option` or other type argument):
/// callback values are opaque and only ever passed back to C, so their type is the whole of
/// their flow. Types compare by their nominal shape, since layouts do not tell two
/// packages' records apart.
pub(crate) fn check_callbacks(
    program: &ReadyProgram,
    input: &LinkSource<'_>,
) -> Result<(), NativeError> {
    let snapshot = input;
    // Each callback type a parameter of a non-home extern carries, with the first such
    // extern.
    let mut handed: BTreeMap<String, DefPath> = BTreeMap::new();
    for (path, signature) in snapshot.signatures {
        match program.foreign_dispatch.get(path) {
            Some(word) if word & FOREIGN_THREADS_MASK != FOREIGN_THREADS_HOME => {}
            _ => continue,
        }
        let SignatureKind::Function(function) = &signature.kind else {
            continue;
        };
        for parameter in function.parameters.iter() {
            callback_types(&snapshot.types, parameter.ty, &mut |shape| {
                handed.entry(shape).or_insert_with(|| path.clone());
            });
        }
    }
    if handed.is_empty() {
        return Ok(());
    }
    // Compact summaries survive both object hits and whole-plan assembly, where
    // no Ready bodies remain. Copied callbacks retain their independent owner.
    let callbacks: BTreeSet<_> = program
        .summaries
        .iter()
        .flat_map(|unit| unit.glue.callbacks.iter().map(|(path, _)| path))
        .collect();
    for callback in callbacks {
        let Some(signature) = snapshot.signatures.get(callback) else {
            continue;
        };
        let SignatureKind::Function(function) = &signature.kind else {
            continue;
        };
        let Some(extern_) = handed.get(&function_shape(&snapshot.types, function)) else {
            continue;
        };
        if let Some(reach) = input.semantic.home_reach(callback) {
            let home = reach.path.last().unwrap_or(callback);
            return Err(NativeError {
                definition: Some(callback.clone()),
                ..NativeError::new(
                    StageName::Import,
                    format!(
                        "PROG-4: the callback {callback} can reach {home}, an extern of the \
                     home package {}@{} (through {}), and its callback type is a \
                     parameter of {extern_}, an extern of the {} package {}@{}, \
                     whose calls do not run on the home thread; only a home \
                     package's extern may take such a callback (S-13)",
                        home.0.package.name,
                        home.0.package.version,
                        reach.spelled().join(" -> "),
                        threads(snapshot, extern_).name(),
                        extern_.0.package.name,
                        extern_.0.package.version,
                    ),
                )
            });
        }
    }
    Ok(())
}

/// Visit the shape of each callback type `ty` carries: a `FnPtr`'s function type, also as a
/// type argument or a tuple element.
fn callback_types(types: &TypeInterner, ty: TypeId, visit: &mut impl FnMut(String)) {
    let Some(row) = types.get(ty) else {
        return;
    };
    let arguments = |arguments: &[GenericArgument], visit: &mut dyn FnMut(TypeId)| {
        for argument in arguments {
            if let GenericArgument::Type(ty) = argument {
                visit(*ty);
            }
        }
    };
    match &row.kind {
        TypeKind::Builtin {
            kind: BuiltinType::FnPtr,
            arguments: fn_arguments,
        } => {
            for argument in fn_arguments.iter() {
                if let GenericArgument::Type(target) = argument
                    && let Some(Type {
                        kind: TypeKind::Function(function),
                        ..
                    }) = types.get(*target)
                {
                    visit(function_shape(types, function));
                }
            }
        }
        TypeKind::Builtin { arguments: a, .. } | TypeKind::Named { arguments: a, .. } => {
            arguments(a, &mut |ty| callback_types(types, ty, visit))
        }
        TypeKind::Tuple(elements) => {
            for element in elements.iter() {
                callback_types(types, *element, visit);
            }
        }
        TypeKind::Box(inner) => callback_types(types, *inner, visit),
        _ => {}
    }
}

/// A function type's nominal shape: its parameters' and result's types, without names,
/// passing modes or uniqueness, which a callback's definition and an extern's `FnPtr`
/// spell differently.
fn function_shape(types: &TypeInterner, function: &FunctionType) -> String {
    let mut out = String::from("fn(");
    for parameter in function.parameters.iter() {
        type_shape(types, parameter.ty, &mut out, 0);
        out.push(',');
    }
    out.push_str(")->");
    type_shape(types, function.result, &mut out, 0);
    out
}

fn type_shape(types: &TypeInterner, ty: TypeId, out: &mut String, depth: u32) {
    use std::fmt::Write;
    let Some(row) = types.get(ty).filter(|_| depth < 32) else {
        out.push('?');
        return;
    };
    let arguments = |arguments: &[GenericArgument], out: &mut String| {
        out.push('[');
        for argument in arguments {
            match argument {
                GenericArgument::Type(ty) => type_shape(types, *ty, out, depth + 1),
                GenericArgument::Size(size) => {
                    let _ = write!(out, "{size:?}");
                }
            }
            out.push(',');
        }
        out.push(']');
    };
    match &row.kind {
        TypeKind::Primitive(primitive) => {
            let _ = write!(out, "{primitive:?}");
        }
        TypeKind::Named {
            definition,
            arguments: a,
        } => {
            let _ = write!(out, "{definition}");
            arguments(a, out);
        }
        TypeKind::Builtin { kind, arguments: a } => {
            let _ = write!(out, "{kind:?}");
            arguments(a, out);
        }
        TypeKind::Tuple(elements) => {
            out.push('(');
            for element in elements.iter() {
                type_shape(types, *element, out, depth + 1);
                out.push(',');
            }
            out.push(')');
        }
        TypeKind::Box(inner) => {
            out.push_str("Box ");
            type_shape(types, *inner, out, depth + 1);
        }
        TypeKind::Function(function) => {
            out.push_str("fn(");
            for parameter in function.parameters.iter() {
                type_shape(types, parameter.ty, out, depth + 1);
                out.push(',');
            }
            out.push_str(")->");
            type_shape(types, function.result, out, depth + 1);
        }
        // Structural and generic rows spell their whole row: callbacks are monomorphic.
        other => {
            let _ = write!(out, "{other:?}");
        }
    }
}

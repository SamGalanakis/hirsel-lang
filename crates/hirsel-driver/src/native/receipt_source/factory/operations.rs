//! Exact reviewed source operation expansion. Unknown operations never supply
//! an empty requirement, even if their current native body is unavailable.
use super::{SourceError, provider};
use hirsel_core_contract::OperationId;
use hirsel_rc::sealed_source::demand::{ExternalDemand, SourceCall};
use hirsel_tir::Callee;
use hirsel_types::{GenericArgument, UseKind};

pub(super) enum Demand {
    External(ExternalDemand),
    ArenaScope { callback: u32 },
    TaskCallback { callback: u32, scoped: bool },
    RegionRead { owner: u32, callback: u32 },
}

pub(super) fn expand(
    provider: &dyn provider::Provider,
    call: &SourceCall,
) -> Result<Option<(OperationId, Demand)>, SourceError> {
    let Callee::Definition { path, arguments } = &call.callee else {
        return Ok(None);
    };
    let task_callback = if path.is_core("task.scope") {
        Some((true, UseKind::Borrow))
    } else if path.is_core("task.isolate") || path.is_core("task.async") {
        Some((false, UseKind::Take))
    } else {
        None
    };
    let simple = if path.is_core("$intrinsic.heap") {
        Some((0, 0, vec![], ExternalDemand::default()))
    } else if path.is_core("io.tasks") || path.is_core("io.stdout") {
        Some((0, 1, vec![UseKind::Borrow], ExternalDemand::default()))
    } else if path.is_core("out.print") {
        Some((
            0,
            2,
            vec![UseKind::Borrow, UseKind::Borrow],
            ExternalDemand::default(),
        ))
    } else if path.is_core("shared.freeze") || path.is_core("shared.thaw") {
        Some((
            1,
            1,
            vec![UseKind::Take],
            ExternalDemand {
                result: vec![0],
                ..Default::default()
            },
        ))
    } else if path.is_core("task.await") {
        Some((
            1,
            2,
            vec![UseKind::Borrow, UseKind::Take],
            ExternalDemand {
                result: vec![1],
                ..Default::default()
            },
        ))
    } else if path.is_core("list.len") {
        Some((1, 1, vec![UseKind::Borrow], ExternalDemand::default()))
    } else if path.is_core("bytes.len") {
        Some((0, 1, vec![UseKind::Borrow], ExternalDemand::default()))
    } else if path.is_core("copy_to") {
        Some((
            1,
            2,
            vec![UseKind::Borrow, UseKind::Borrow],
            ExternalDemand {
                result: vec![1],
                retained: vec![(0, vec![1])],
                ..Default::default()
            },
        ))
    } else if path.is_core("failure.check") {
        Some((
            0,
            2,
            vec![UseKind::Borrow, UseKind::Borrow],
            ExternalDemand::default(),
        ))
    } else if path.is_core("list.push") {
        Some((
            1,
            2,
            vec![UseKind::Mut, UseKind::Take],
            ExternalDemand {
                mutations: vec![(0, vec![0, 1])],
                ..Default::default()
            },
        ))
    } else {
        None
    };
    if !path.is_core("arena.seal")
        && !path.is_core("region.with")
        && !path.is_core("shared.with")
        && hirsel_types::core_roles::arena_scope_callback(path).is_none()
        && simple.is_none()
        && task_callback.is_none()
    {
        return Ok(None);
    }
    let registry = hirsel_engine::registry::BuiltinRegistry::shared();
    let declaration = provider.signature(path)?;
    let reviewed = registry
        .get(declaration.definition)
        .filter(|signature| signature.definition.path() == path)
        .ok_or(SourceError::Malformed(
            "receipt operation lacks its reviewed core declaration",
        ))?;
    let actual = hirsel_types::view_digest(provider.types(), &declaration)
        .map_err(|error| SourceError::Type(format!("{error:?}")))?;
    let expected = hirsel_types::view_digest(registry.types(), &reviewed)
        .map_err(|error| SourceError::Type(format!("{error:?}")))?;
    if declaration.definition.path() != path
        || declaration.fingerprint != reviewed.fingerprint
        || actual != expected
    {
        return Err(SourceError::Malformed(
            "receipt operation declaration differs from reviewed core",
        ));
    }
    if let Some((generics, arity, uses, demand)) = simple {
        if arguments.len() != generics
            || !arguments
                .iter()
                .all(|argument| matches!(argument, GenericArgument::Type(_)))
            || call.environment.is_some()
            || call.arguments.len() != arity
            || !uses.iter().enumerate().all(|(ordinal, expected)| {
                call.arguments
                    .get(&(ordinal as u32))
                    .is_some_and(|argument| {
                        argument.usage == *expected
                            || (*expected == UseKind::Borrow && argument.usage == UseKind::Copy)
                    })
            })
            || call.result.is_none()
        {
            return Err(SourceError::Malformed(
                "receipt collection original call differs",
            ));
        }
        return Ok(Some((
            OperationId {
                definition: path.clone(),
                declaration_digest: reviewed.fingerprint,
            },
            Demand::External(demand),
        )));
    }
    if let Some((scoped, usage)) = task_callback {
        if !matches!(arguments.as_slice(), [GenericArgument::Type(_)])
            || call.environment.is_some()
            || call.arguments.len() != 2
            || !call
                .arguments
                .get(&0)
                .is_some_and(|argument| matches!(argument.usage, UseKind::Borrow | UseKind::Copy))
            || !call.arguments.get(&1).is_some_and(|argument| {
                argument.usage == usage
                    || (usage == UseKind::Borrow && argument.usage == UseKind::Copy)
            })
            || call.result.is_none()
        {
            return Err(SourceError::Malformed(
                "receipt task callback original call differs",
            ));
        }
        return Ok(Some((
            OperationId {
                definition: path.clone(),
                declaration_digest: reviewed.fingerprint,
            },
            Demand::TaskCallback {
                callback: 1,
                scoped,
            },
        )));
    }
    if path.is_core("region.with") || path.is_core("shared.with") {
        let owner_usage = if path.is_core("region.with") {
            UseKind::Mut
        } else {
            UseKind::Borrow
        };
        if !matches!(
            arguments.as_slice(),
            [GenericArgument::Type(_), GenericArgument::Type(_)]
        ) || call.environment.is_some()
            || call.arguments.len() != 2
            || !call.arguments.get(&0).is_some_and(|value| {
                value.usage == owner_usage
                    || (owner_usage == UseKind::Borrow && value.usage == UseKind::Copy)
            })
            || !call
                .arguments
                .get(&1)
                .is_some_and(|value| matches!(value.usage, UseKind::Borrow | UseKind::Copy))
            || call.result.is_none()
        {
            return Err(SourceError::Malformed(
                "receipt sealed read original call differs",
            ));
        }
        return Ok(Some((
            OperationId {
                definition: path.clone(),
                declaration_digest: reviewed.fingerprint,
            },
            Demand::RegionRead {
                owner: 0,
                callback: 1,
            },
        )));
    }
    if let Some(callback) = hirsel_types::core_roles::arena_scope_callback(path) {
        if !matches!(arguments.as_slice(), [GenericArgument::Type(_)])
            || call.environment.is_some()
            || call.arguments.len() != callback as usize + 1
            || !(0..=callback).all(|ordinal| {
                call.arguments.get(&ordinal).is_some_and(|argument| {
                    matches!(argument.usage, UseKind::Borrow | UseKind::Copy)
                })
            })
            || call.result.is_none()
        {
            return Err(SourceError::Malformed(
                "receipt arena scope original call differs",
            ));
        }
        return Ok(Some((
            OperationId {
                definition: path.clone(),
                declaration_digest: reviewed.fingerprint,
            },
            Demand::ArenaScope { callback },
        )));
    }
    if !matches!(arguments.as_slice(), [GenericArgument::Type(_)])
        || call.environment.is_some()
        || call.arguments.len() != 2
        || ![0, 1].into_iter().all(|ordinal| {
            call.arguments
                .get(&ordinal)
                .is_some_and(|argument| argument.usage == UseKind::Take)
        })
        || call.result.is_none()
    {
        return Err(SourceError::Malformed(
            "receipt arena.seal original call differs",
        ));
    }
    Ok(Some((
        OperationId {
            definition: path.clone(),
            declaration_digest: reviewed.fingerprint,
        },
        Demand::External(ExternalDemand {
            parameters: vec![1],
            inventory: vec![0],
            ..Default::default()
        }),
    )))
}

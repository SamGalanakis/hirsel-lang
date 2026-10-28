//! Reconstruct GEN-6 graphs from independently validated initializer TIR.
//! A matching checksum or callable header alone cannot certify field selection.
use super::*;
use hirsel_tir::{
    Body, Constructor, InstructionKind, PlaceId, Rvalue, Terminator, ValueKind, ValueNode,
};
use hirsel_types::{SignatureKind, TypeKind};

fn callable(value: &Materialized) -> bool {
    let mut pending = vec![&value.root];
    while let Some(node) = pending.pop() {
        match &node.kind {
            ValueKind::FunctionReference { .. } => return true,
            ValueKind::Construct { fields, .. } => pending.extend(fields),
            _ => (),
        }
    }
    false
}

fn equal(types: &TypeInterner, left: &ValueNode, right: &ValueNode) -> bool {
    let mut pending = vec![(left, right)];
    while let Some((left, right)) = pending.pop() {
        if !types.semantic_eq(left.ty, right.ty) {
            return false;
        }
        match (&left.kind, &right.kind) {
            (ValueKind::Constant(a), ValueKind::Constant(b)) if a == b => (),
            (
                ValueKind::FunctionReference {
                    definition: a,
                    arguments: aa,
                },
                ValueKind::FunctionReference {
                    definition: b,
                    arguments: ba,
                },
            ) if a == b
                && aa.len() == ba.len()
                && aa.iter().zip(ba).all(|(a, b)| {
                    matches!((hirsel_types::encoding::canonical_argument_encoding(types, a),
                        hirsel_types::encoding::canonical_argument_encoding(types, b)),
                        (Ok(a), Ok(b)) if a == b)
                }) =>
            {
                ()
            }
            (
                ValueKind::Construct {
                    constructor: a,
                    fields: aa,
                },
                ValueKind::Construct {
                    constructor: b,
                    fields: ba,
                },
            ) if a == b && aa.len() == ba.len() => pending.extend(aa.iter().zip(ba)),
            _ => return false,
        }
    }
    true
}

/// GEN-6 lowering is a closed construction graph: no initializer call executes.
/// Recover single-assignment producers; V has already checked all their types,
/// callable actuals, modes, bounds, authority and result contracts.
fn recover(
    body: &Body,
    root: PlaceId,
    constants: &BTreeMap<&DefPath, &Materialized>,
) -> Option<(ValueNode, Option<DefPath>)> {
    let mut producers = BTreeMap::new();
    for block in &body.blocks {
        for instruction in &block.instructions {
            if let InstructionKind::Assign { destination, value } = instruction.kind {
                let place = body.places.get(destination.0 as usize)?;
                if !place.projection.is_empty() || producers.insert(place.root.0, value).is_some() {
                    return None;
                }
            }
        }
    }
    enum Work {
        Enter(PlaceId, bool),
        Use(PlaceId),
        Finish(PlaceId, Constructor, usize),
    }
    let mut pending = vec![Work::Enter(root, true)];
    let mut values: Vec<ValueNode> = vec![];
    let mut alias = None;
    let mut active = BTreeSet::new();
    while let Some(work) = pending.pop() {
        match work {
            Work::Use(place) => {
                values.last_mut()?.ty = body.places.get(place.0 as usize)?.ty;
                active.remove(&body.places.get(place.0 as usize)?.root.0);
            }
            Work::Finish(place, constructor, count) => {
                active.remove(&body.places.get(place.0 as usize)?.root.0);
                let fields = values.split_off(values.len().checked_sub(count)?);
                values.push(ValueNode {
                    ty: body.places.get(place.0 as usize)?.ty,
                    kind: ValueKind::Construct {
                        constructor,
                        fields,
                    },
                });
            }
            Work::Enter(place, at_root) => {
                let target = body.places.get(place.0 as usize)?;
                if !target.projection.is_empty() || !active.insert(target.root.0) {
                    return None;
                }
                let producer = body
                    .rvalues
                    .get(producers.get(&target.root.0)?.0 as usize)?;
                let kind = match producer {
                    Rvalue::Constant(value) => ValueKind::Constant(value.clone()),
                    Rvalue::FunctionReference {
                        definition,
                        arguments,
                    } => ValueKind::FunctionReference {
                        definition: definition.clone(),
                        arguments: arguments.clone(),
                    },
                    Rvalue::ReadConstant {
                        definition,
                        arguments,
                    } if arguments.is_empty() => {
                        if at_root {
                            alias = Some(definition.clone());
                        }
                        let mut node = constants.get(definition)?.root.clone();
                        node.ty = target.ty;
                        values.push(node);
                        active.remove(&target.root.0);
                        continue;
                    }
                    Rvalue::Use(value) => {
                        pending.push(Work::Use(place));
                        pending.push(Work::Enter(value.place, at_root));
                        continue;
                    }
                    Rvalue::Construct {
                        constructor,
                        fields,
                        ..
                    } => {
                        let mut fields = fields.iter().collect::<Vec<_>>();
                        fields.sort_by_key(|field| field.field);
                        if fields
                            .iter()
                            .enumerate()
                            .any(|(i, field)| field.field as usize != i)
                        {
                            return None;
                        }
                        pending.push(Work::Finish(place, constructor.clone(), fields.len()));
                        pending.extend(
                            fields
                                .into_iter()
                                .rev()
                                .map(|field| Work::Enter(field.value.place, false)),
                        );
                        continue;
                    }
                    _ => return None,
                };
                active.remove(&target.root.0);
                values.push(ValueNode {
                    ty: target.ty,
                    kind,
                });
            }
        }
    }
    (values.len() == 1).then(|| (values.pop().unwrap(), alias))
}

pub(super) fn validate(export: &Export) -> Result<(), ImportError> {
    let constants = export
        .constants
        .iter()
        .map(|s| (&s.path, &s.value))
        .collect::<BTreeMap<_, _>>();
    let signatures = export
        .signatures
        .iter()
        .map(|s| (&s.path, &s.public))
        .collect::<BTreeMap<_, _>>();
    for constant in &export.constants {
        let Some(signature) = signatures.get(&constant.path) else {
            // CT-2 generated data instances have no independent source header.
            // They cannot publish callable graphs through this exception.
            if callable(&constant.value) {
                return Err(refuse(
                    constant.path.to_string(),
                    "constant lacks a declaration",
                ));
            }
            continue;
        };
        let definition = export.definitions.iter().find(|d| d.path == constant.path);
        let has_callable_tir = definition.is_some_and(|d| {
            d.tir.bodies.iter().any(|b| {
                b.rvalues
                    .iter()
                    .any(|r| matches!(r, Rvalue::FunctionReference { .. }))
            })
        });
        let declared_dictionary = match &signature.kind {
            SignatureKind::Const { ty, .. } => match &export.types.row(*ty).kind {
                TypeKind::Named { definition, .. } => export.signatures.iter()
                    .find(|s| &s.path == definition.path())
                    .and_then(|s| match &s.public.kind { SignatureKind::Type(record) => record.representation, _ => None })
                    .is_some_and(|ty| matches!(&export.types.row(ty).kind,
                        TypeKind::Record(fields) if fields.iter().any(|f| matches!(export.types.row(f.ty).kind, TypeKind::Function(_))))),
                _ => false,
            },
            _ => false,
        };
        if !callable(&constant.value)
            && !declared_dictionary
            && !has_callable_tir
            && signature.metadata.constant_alias.is_none()
        {
            continue;
        }
        let at = constant.path.to_string();
        if !matches!(signature.kind, SignatureKind::Const { .. }) {
            return Err(refuse(at, "callable graph is not a constant declaration"));
        }
        let Some(definition) = definition else {
            // Registry constants have no source body; their declarations are
            // supplied by the owning compiler registry, as before GEN-6.
            if constant.path.0.package.name_is("core") {
                continue;
            }
            return Err(refuse(
                at,
                "dictionary graph lacks validated initializer TIR",
            ));
        };
        let body = &definition.tir.bodies[definition.tir.entry.0 as usize];
        let returns = body
            .blocks
            .iter()
            .filter_map(|b| match b.terminator {
                Terminator::Return { ref value, .. } => Some(value.place),
                _ => None,
            })
            .collect::<Vec<_>>();
        let [root] = returns.as_slice() else {
            return Err(refuse(
                at,
                "dictionary initializer must have one closed result",
            ));
        };
        let (recovered, alias) = recover(body, *root, &constants).ok_or_else(|| {
            refuse(
                at.clone(),
                "dictionary initializer is not a closed construction graph",
            )
        })?;
        if !equal(&export.types, &recovered, &constant.value.root) {
            return Err(refuse(
                at.clone(),
                "dictionary graph disagrees with validated initializer TIR",
            ));
        }
        if alias.as_ref() != signature.metadata.constant_alias.as_deref() {
            return Err(refuse(
                at,
                "dictionary alias disagrees with its initializer",
            ));
        }
    }
    Ok(())
}

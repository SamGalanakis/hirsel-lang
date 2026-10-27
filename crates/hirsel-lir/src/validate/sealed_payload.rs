//! Recipes and concrete operand bindings are independently checked. Equal
//! candidate recipes cannot manufacture their own expected source inventory.
use super::{Cx, Report, Site, ViolationKind as K};
use crate::ir::*;
use crate::sealed_payload::*;
use crate::stage::Stage;
pub(super) mod pairing;

pub(super) fn check<S: Stage>(cx: &Cx<'_, S>, report: &mut Report) {
    let unit = cx.unit;
    let expected = cx
        .operations
        .seal_payloads(&unit.header.definition, unit.header.instance);
    if unit.tables.seal_payloads.is_empty()
        && expected.as_ref().is_none_or(Vec::is_empty)
        && !unit
            .instructions
            .iter()
            .any(|i| matches!(i, Inst::ArenaInventory { .. } | Inst::PayloadReceipt { .. }))
    {
        return;
    }
    let pairs = pairing::Pairing::new(unit);
    let mut observed = vec![];
    let origins = producers(unit, &cx.layouts.0.fn_abis[unit.header.abi.0 as usize]);
    for inst in &unit.instructions {
        let Inst::PayloadReceipt {
            value,
            shape,
            binding,
            inputs,
            arena,
            ..
        } = *inst
        else {
            continue;
        };
        let Some((descriptor, binding)) = unit.tables.payload_binding(binding) else {
            continue;
        };
        let inputs = inputs.get(&unit.pools.receipt_inputs).unwrap_or(&[]);
        let mut reject = |text: &str| report.add(Site::UNIT, K::Sealed(text.into()));
        if descriptor != shape {
            reject("payload receipt binding selects another descriptor");
        }
        if let Err(error) = binding.check_receipt_inputs(inputs) {
            reject(error);
        }
        let source = &binding.recipe.source;
        if !matches!(origins[value.index()].as_slice(),
            [PayloadProducer::Construct(actual)] if actual == source)
        {
            reject("payload receipt value differs from original source producer");
        }
        // This local producer implements ordinary heap construction only.
        // Calls, held environments and mutation need the boundary custody
        // proof before their source recipe can authorize a fresh snapshot.
        let constructors: Vec<_> = unit
            .tables
            .constructs
            .iter()
            .filter(|site| &unit.tables.source_anchors[site.anchor.index()] == source)
            .collect();
        let ordered = matches!(constructors.as_slice(), [site]
            if site.fields.get(&unit.pools.fields).unwrap_or(&[]).iter()
                .map(|field| field.value).eq(binding.values.iter().copied()));
        let destination = match (
            &binding.recipe.operation,
            arena.get(),
            constructors.as_slice(),
        ) {
            (
                PayloadOperation::Allocate {
                    destination: PayloadDestination::Heap,
                },
                None,
                [site],
            ) => matches!(
                site.destination,
                DestinationClass::Heap | DestinationClass::Inferred
            ),
            (
                PayloadOperation::Allocate {
                    destination: PayloadDestination::Arena(original),
                },
                Some(arena),
                [site],
            ) => {
                let slot = match site.destination {
                    DestinationClass::Arena(slot) => Some(slot),
                    _ => None,
                };
                let parameter = slot.and_then(|slot| match unit.slots[slot.index()].kind {
                    SlotKind::Parameter(source) => unit.parameters.get(source as usize).copied(),
                    _ => None,
                });
                original.capture_slot.is_none()
                    && cx.repr(arena) == ValueRepr::Arena
                    && origins[arena.index()] == vec![original.producer.clone()]
                    && parameter
                        .is_some_and(|value| origins[value.index()] == origins[arena.index()])
            }
            _ => false,
        };
        if !ordered || !destination {
            reject("payload receipt requires its ordered original construction and destination");
        }
        for (input, source) in inputs.iter().zip(&binding.recipe.inputs) {
            match input.receipt {
                Some(receipt) => {
                    if !pairs.paired(input.original, receipt) {
                        reject("payload receipt child does not describe its paired original value");
                    }
                    let kind = cx.layouts.get(cx.value_layout(receipt)).map(|r| &r.kind);
                    if cx.repr(receipt) != ValueRepr::PayloadReceipt
                        || !matches!(kind, Some(crate::layout_types::LayoutKind::PayloadReceipt { root }) if root == &source.ty)
                    {
                        reject("payload receipt child has another semantic type");
                    }
                }
                None => {
                    let plain = cx
                        .operations
                        .seal_descriptor_for_source(
                            &unit.header.definition,
                            unit.header.instance,
                            &source.ty,
                            cx.layouts.0,
                            cx.target,
                        )
                        .is_ok_and(|descriptor| {
                            matches!(
                                descriptor.logical.shape.nodes.as_slice(),
                                [crate::structural::Shape::Leaf]
                            ) && matches!(
                                descriptor.logical.facts.as_slice(),
                                [crate::logical_storage::StorageFact {
                                    class: crate::logical_storage::StorageClass::Inline,
                                    ..
                                }]
                            )
                        });
                    if !plain && !matches!(source.producer, PayloadProducer::Literal(_)) {
                        reject("payload receipt omits demanded original provenance");
                    }
                }
            }
        }
    }
    for table in &unit.tables.seal_payloads {
        let descriptor = &unit.tables.seal_descriptors[table.descriptor.index()];
        for binding in &table.bindings {
            observed.push(SourcePayload {
                root_type: descriptor.root_type.clone(),
                recipe: binding.recipe.clone(),
            });
            let reject = |report: &mut Report, text: &str| {
                report.add(Site::UNIT, K::Sealed(text.into()));
            };
            if binding.values.len() != binding.recipe.inputs.len() {
                reject(report, "semantic payload input arity differs");
                continue;
            }
            let operands = site_operands(unit, &binding.recipe.source);
            for (&value, source) in binding.values.iter().zip(&binding.recipe.inputs) {
                if !operands.contains(&value) {
                    reject(
                        report,
                        "semantic payload input is not an original site operand",
                    );
                }
                if origins[value.index()] != vec![source.producer.clone()] {
                    reject(report, "semantic payload operand producer lineage differs");
                }
                match cx.operations.seal_descriptor_for_source(
                    &unit.header.definition,
                    unit.header.instance,
                    &source.ty,
                    cx.layouts.0,
                    cx.target,
                ) {
                    Ok(expected) if expected.root_layout == cx.value_layout(value) => (),
                    _ => reject(report, "semantic payload operand type differs from source"),
                }
                if let Some(slot) = source.capture_slot {
                    let capture = unit
                        .tables
                        .closures
                        .iter()
                        .filter(|c| {
                            unit.tables.source_anchors[c.anchor.index()] == binding.recipe.source
                        })
                        .find_map(|c| c.captures.get(&unit.pools.captures)?.get(slot as usize));
                    if capture.is_none_or(|c| c.value != value) {
                        reject(report, "semantic payload closed capture slot differs");
                    }
                }
            }
            match binding.recipe.operation {
                PayloadOperation::Replace { previous }
                | PayloadOperation::Transfer { previous }
                    if !binding
                        .recipe
                        .inputs
                        .iter()
                        .any(|input| input.place == previous) =>
                {
                    reject(report, "semantic payload previous place is absent")
                }
                PayloadOperation::Boundary
                    if descriptor.logical.facts.first().is_none_or(|fact| {
                        fact.class != crate::logical_storage::StorageClass::Shared
                    }) =>
                {
                    reject(report, "semantic payload boundary is not Shared")
                }
                _ => (),
            }
        }
    }
    for instruction in &unit.instructions {
        let Inst::ArenaInventory {
            arena,
            allocation,
            value,
            shape,
            previous,
            receipt,
            ..
        } = *instruction
        else {
            continue;
        };
        let source = &unit.tables.source_anchors[allocation.index()];
        let bindings: Vec<_> = unit
            .tables
            .seal_payloads
            .iter()
            .filter(|table| table.descriptor == shape)
            .flat_map(|table| &table.bindings)
            .filter(|binding| &binding.recipe.source == source)
            .collect();
        let [binding] = bindings.as_slice() else {
            report.add(
                Site::UNIT,
                K::Sealed("semantic allocation lacks one source payload recipe".into()),
            );
            continue;
        };
        if let Some(receipt) = receipt.get() {
            let kind = cx.layouts.get(cx.value_layout(receipt)).map(|r| &r.kind);
            if !pairs.paired(value, receipt)
                || cx.repr(receipt) != ValueRepr::PayloadReceipt
                || !matches!(kind, Some(crate::layout_types::LayoutKind::PayloadReceipt { root })
                    if root == &unit.tables.seal_descriptors[shape.index()].root_type)
            {
                report.add(
                    Site::UNIT,
                    K::Sealed("inventory receipt differs from its paired original root".into()),
                );
            }
        }
        match binding.recipe.operation {
            PayloadOperation::Allocate { ref destination } => {
                match destination {
                    PayloadDestination::Arena(source)
                        if source.capture_slot.is_none()
                            && origins[arena.index()] == vec![source.producer.clone()] =>
                    {
                        ()
                    }
                    _ => report.add(
                        Site::UNIT,
                        K::Sealed(
                            "semantic allocation arena differs from its source destination".into(),
                        ),
                    ),
                }
                let expected = vec![PayloadProducer::Construct(source.clone())];
                let closure = vec![PayloadProducer::Closure(source.clone())];
                if previous.is_some()
                    || (origins[value.index()] != expected && origins[value.index()] != closure)
                {
                    report.add(
                        Site::UNIT,
                        K::Sealed(
                            "semantic allocation value does not follow its source construction"
                                .into(),
                        ),
                    );
                }
            }
            PayloadOperation::Replace { previous: place }
            | PayloadOperation::Transfer { previous: place } => {
                let expected = binding
                    .recipe
                    .inputs
                    .iter()
                    .zip(&binding.values)
                    .find(|(input, _)| input.place == place)
                    .map(|(_, &value)| value);
                if previous.is_none() || previous != expected {
                    report.add(
                        Site::UNIT,
                        K::Sealed("semantic allocation previous value differs from source".into()),
                    );
                }
            }
            PayloadOperation::Boundary => report.add(
                Site::UNIT,
                K::Sealed("Shared boundary is not an ordinary arena allocation".into()),
            ),
        }
    }
    match expected {
        Some(expected) if expected == observed => (),
        None if observed.is_empty() => (),
        _ => report.add(
            Site::UNIT,
            K::Sealed("semantic payload recipes differ from independent source".into()),
        ),
    }
}

/// Only operands already used by the construction/transition may be captured.
/// Recipes introduce no hidden SSA uses or unvalidated lifetime extension.
fn site_operands<S>(unit: &LirUnit<S>, source: &SourceAnchor) -> Vec<ValueId> {
    let anchor = |id: SourceAnchorId| &unit.tables.source_anchors[id.index()] == source;
    let mut operands = vec![];
    for instruction in &unit.instructions {
        match *instruction {
            Inst::Construct { construct, .. }
            | Inst::Reuse { construct, .. }
            | Inst::UniqueCopy { construct, .. }
                if anchor(unit.tables.constructs[construct.index()].anchor) =>
            {
                operands.extend(
                    unit.tables.constructs[construct.index()]
                        .fields
                        .get(&unit.pools.fields)
                        .unwrap_or(&[])
                        .iter()
                        .map(|f| f.value),
                );
                if let Inst::UniqueCopy { value, .. } = instruction {
                    operands.push(*value);
                }
            }
            Inst::Closure { closure, .. }
                if anchor(unit.tables.closures[closure.index()].anchor) =>
            {
                operands.extend(
                    unit.tables.closures[closure.index()]
                        .captures
                        .get(&unit.pools.captures)
                        .unwrap_or(&[])
                        .iter()
                        .map(|c| c.value),
                );
            }
            Inst::SealArena {
                root,
                grant,
                origin,
                ..
            } if anchor(origin) => {
                operands.extend([root, grant]);
            }
            Inst::ArenaInventory {
                value,
                previous,
                allocation,
                ..
            } if anchor(allocation) => {
                operands.push(value);
                operands.extend(previous);
            }
            _ => (),
        }
    }
    for call in &unit.tables.calls {
        if anchor(call.anchor) {
            for argument in call
                .arguments
                .get(&unit.pools.call_arguments)
                .unwrap_or(&[])
            {
                match argument.value {
                    ArgumentValue::Value(value) => operands.push(value),
                    ArgumentValue::Address { version, .. } => operands.push(version),
                }
            }
        }
    }
    operands
}

fn literal(value: &ConstantValue) -> Option<PayloadLiteral> {
    Some(match value {
        ConstantValue::Unit => PayloadLiteral::Unit,
        ConstantValue::Bool(v) => PayloadLiteral::Bool(*v),
        ConstantValue::Integer(v) => PayloadLiteral::Integer(*v),
        ConstantValue::F32(v) => PayloadLiteral::F32(*v),
        ConstantValue::F64(v) => PayloadLiteral::F64(*v),
        ConstantValue::String(v) => PayloadLiteral::String(v.clone()),
        ConstantValue::Bytes(v) => PayloadLiteral::Bytes(v.clone()),
        ConstantValue::Heap | ConstantValue::Descriptor(_) => return None,
    })
}

/// Recover canonical producers through whole-value transport and CFG joins.
/// A projection or an unknown producer is not guessed to be an identity copy.
fn producers<S>(unit: &LirUnit<S>, abi: &crate::FnAbi) -> Vec<Vec<PayloadProducer>> {
    let mut result = vec![vec![]; unit.values.len()];
    for (parameter, value) in abi.parameters.iter().zip(&unit.parameters) {
        result[value.index()].push(PayloadProducer::Parameter(parameter.source));
    }
    // Filtered native capture order cannot establish original source ordinals.
    // A complete original environment association is required before accepting
    // captured producers; no candidate ordinal is guessed here.
    for inst in &unit.instructions {
        let (value, origin) = match *inst {
            Inst::Const { result, constant } => {
                let Some(value) = literal(&unit.tables.constants[constant.index()].value) else {
                    continue;
                };
                (result, PayloadProducer::Literal(value))
            }
            Inst::Construct {
                result, construct, ..
            }
            | Inst::Reuse {
                result, construct, ..
            } => (
                result,
                PayloadProducer::Construct(
                    unit.tables.source_anchors
                        [unit.tables.constructs[construct.index()].anchor.index()]
                    .clone(),
                ),
            ),
            Inst::Closure {
                result, closure, ..
            } => (
                result,
                PayloadProducer::Closure(
                    unit.tables.source_anchors
                        [unit.tables.closures[closure.index()].anchor.index()]
                    .clone(),
                ),
            ),
            _ => continue,
        };
        result[value.index()].push(origin);
    }
    for call in &unit.tables.calls {
        if let Some(value) = call.result {
            result[value.index()].push(PayloadProducer::Call(
                unit.tables.source_anchors[call.anchor.index()].clone(),
            ));
        }
    }
    let extend = |into: &mut Vec<PayloadProducer>, from: &[PayloadProducer]| {
        for item in from {
            if !into.contains(item) {
                into.push(item.clone());
            }
        }
    };
    loop {
        let previous = result.clone();
        for instruction in &unit.instructions {
            let pair = match *instruction {
                Inst::CopyValue { result, value }
                | Inst::Weaken { result, value }
                | Inst::Recast { result, value } => Some((result, value)),
                Inst::Read {
                    result,
                    reaching,
                    address,
                    ..
                }
                | Inst::Copy {
                    result,
                    reaching,
                    address,
                    ..
                } if identity_address(unit, address) => Some((result, reaching)),
                Inst::Store {
                    version,
                    value,
                    address,
                }
                | Inst::Replace {
                    version,
                    value,
                    address,
                    ..
                } if identity_address(unit, address) => Some((version, value)),
                _ => None,
            };
            if let Some((to, from)) = pair {
                extend(&mut result[to.index()], &previous[from.index()]);
            }
        }
        for edge in &unit.tables.edges {
            let parameters = unit.blocks[edge.target.index()]
                .parameters
                .get(&unit.pools.values)
                .unwrap_or(&[]);
            for (to, from) in parameters
                .iter()
                .zip(edge.arguments.get(&unit.pools.values).unwrap_or(&[]))
            {
                extend(&mut result[to.index()], &previous[from.index()]);
            }
        }
        if previous == result {
            return result;
        }
    }
}

fn identity_address<S>(unit: &LirUnit<S>, mut address: AddressId) -> bool {
    for _ in 0..unit.addresses.len() {
        let row = &unit.addresses[address.index()];
        if row.projection.len != 0 {
            return false;
        }
        match row.base {
            AddressBase::Address(parent) => address = parent,
            AddressBase::Value(_) | AddressBase::Slot(_) => return true,
        }
    }
    false
}

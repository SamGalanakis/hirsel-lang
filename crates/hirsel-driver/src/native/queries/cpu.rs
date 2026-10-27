//! CPU certificates consume current C-admitted Ready products. Ordinary roots
//! retain their descriptor-only assembly; CPU roots additionally prove the
//! mapper and the exact physical frames before selecting their start helper.
use super::*;
use hirsel_lir::{
    ahead,
    ir::*,
    stage::{Ready, ValidatedUnit},
};

pub(super) fn materialize(
    root: &mut RootProducts,
    selected_root: &Arc<NativeRootProduct>,
    products: &BTreeMap<ContentHash, &Arc<ReadyProduct>>,
    summaries: &[UnitSummary],
    binding: &Binding,
    operations: &hirsel_layout::operation::OperationRegistry,
    objects: &[hirsel_lir::stage::ObjectArtifact],
) -> Result<Vec<ValidatedUnit<Ready>>, String> {
    let maps: BTreeSet<_> = summaries
        .iter()
        .filter(|s| s.definition.is_core("cpu.map_scoped"))
        .map(|s| s.instance)
        .collect();
    if maps.is_empty() {
        return Ok(Vec::new());
    }
    let mut scope = maps.clone();
    scope.extend(
        summaries
            .iter()
            .filter(|s| s.definition.is_core("$native.cpu_worker"))
            .map(|s| s.instance),
    );
    // A library map can be copied into a caller's object. Re-emit every direct
    // ancestor under the same checked recipe, including such copied calls.
    loop {
        let previous = scope.len();
        for summary in summaries {
            if summary
                .calls
                .iter()
                .any(|s| s.instance.is_some_and(|id| scope.contains(&id)))
            {
                scope.insert(summary.instance);
            }
        }
        if scope.len() == previous {
            break;
        }
    }
    loop {
        let before = scope.len();
        let names: BTreeSet<_> = summaries
            .iter()
            .filter(|s| scope.contains(&s.instance))
            .map(|s| hirsel_lir::emission::symbol_name(&s.definition, Some(&s.instance)))
            .collect();
        for object in objects.iter().filter(|object| {
            object
                .exports
                .iter()
                .any(|symbol| names.iter().any(|name| symbol.name.starts_with(name)))
        }) {
            for summary in summaries {
                let name =
                    hirsel_lir::emission::symbol_name(&summary.definition, Some(&summary.instance));
                if object.exports.iter().any(|symbol| symbol.name == name) {
                    scope.insert(summary.instance);
                }
            }
        }
        if scope.len() == before {
            break;
        }
    }
    let snapshot = root.inner.snapshot.clone();
    let environment = snapshot.environment();
    let inputs = hirsel_lir::validate::Inputs {
        memo: Default::default(),
        target: environment.target,
        layouts: &binding.table,
        operations,
        manifest: environment.manifest,
        discharge: environment.discharge,
    };
    let mut read = |id: ContentHash| -> Result<ValidatedUnit<Ready>, String> {
        let owner = selected_root
            .instances
            .iter()
            .find(|key| key.instance.digest() == id)
            .ok_or("CPU proof owner is not in the root")?;
        let product = products
            .get(&owner.address(READY_KIND).0)
            .ok_or("CPU proof has no settled Ready read")?;
        let declarations = snapshot.instances.operations(owner, &environment)?;
        let scoped = NativeEnvironment {
            operations: &declarations,
            ..environment
        };
        let (_, body) = root.inner.local.queries.read(owner, product, &scoped)?;
        let raw = binding.materialize::<Ready>(&body)?;
        root.inner.materialized();
        hirsel_lir::validate::validate(raw, &inputs)
            .map_err(|e| format!("CPU Ready refused: {:?}", e.violations))
    };
    let mut units = BTreeMap::new();
    for id in scope {
        units.insert(id, read(id)?);
    }
    let callbacks: BTreeSet<_> = units
        .values()
        .flat_map(|u| ahead::cpu_mapper_candidates(u.unit(), &maps))
        .collect();
    // The existing scalar graph bound applies to each prospective mapper DAG.
    for callback in callbacks {
        let mut reached = BTreeSet::new();
        let mut pending = vec![callback];
        while let Some(id) = pending.pop() {
            if !reached.insert(id) {
                continue;
            }
            if reached.len() > ahead::MAX_UNITS {
                break;
            }
            let Some(summary) = summaries.iter().find(|s| s.instance == id) else {
                continue;
            };
            pending.extend(summary.calls.iter().filter_map(|s| s.instance));
            if let std::collections::btree_map::Entry::Vacant(entry) = units.entry(id) {
                entry.insert(read(id)?);
            }
        }
    }
    Ok(units.into_values().collect())
}

pub(super) fn emit(
    program: &crate::native::ReadyProgram,
    backend: crate::native::Backend,
    objects: &mut Vec<hirsel_lir::stage::ObjectArtifact>,
    opt: hirsel_lir::emission::OptLevel,
) -> Result<(), String> {
    if program.units.is_empty() {
        return Ok(());
    }
    let mut plan = crate::native::ahead::plan(program);
    plan.roots.retain(|_, root| root.cpu.is_some());
    let bodies: BTreeSet<_> = plan
        .roots
        .values()
        .flat_map(|root| root.graph.bodies.keys().copied())
        .collect();
    plan.bodies.retain(|id, _| bodies.contains(id));
    plan.spawns.retain(|_, id| plan.roots.contains_key(id));
    let maps: BTreeSet<_> = plan
        .roots
        .values()
        .filter_map(|root| root.cpu.as_ref().map(|cpu| cpu.binding.map))
        .collect();
    if maps.is_empty() {
        return Ok(());
    }
    if objects.iter().any(|object| object.ahead.is_some()) {
        return crate::native::ahead::check_objects_with_plan(program, objects, &plan)
            .map_err(|e| e.to_string());
    }
    let mut affected = maps;
    affected.extend(
        plan.roots
            .iter()
            .filter(|(_, root)| root.cpu.is_some())
            .map(|(id, _)| *id),
    );
    loop {
        let before = affected.len();
        for unit in &program.units {
            if unit.unit().tables.calls.iter().any(|call| {
                matches!(call.callee, Callee::Direct(s) | Callee::Bound { code: s, .. }
                    if unit.unit().tables.symbols[s.index()].instance.is_some_and(|id| affected.contains(&id)))
            }) { affected.insert(unit.unit().header.instance); }
        }
        if affected.len() == before {
            break;
        }
    }
    loop {
        let before = affected.len();
        let names: BTreeSet<_> = program
            .units
            .iter()
            .filter(|u| affected.contains(&u.unit().header.instance))
            .map(|u| {
                hirsel_lir::emission::symbol_name(
                    &u.unit().header.definition,
                    Some(&u.unit().header.instance),
                )
            })
            .collect();
        let removed: Vec<_> = objects
            .iter()
            .filter(|object| {
                object
                    .exports
                    .iter()
                    .any(|symbol| names.iter().any(|name| symbol.name.starts_with(name)))
            })
            .collect();
        for summary in &program.summaries {
            let name =
                hirsel_lir::emission::symbol_name(&summary.definition, Some(&summary.instance));
            if removed
                .iter()
                .any(|object| object.exports.iter().any(|s| s.name == name))
            {
                if !program
                    .units
                    .iter()
                    .any(|u| u.unit().header.instance == summary.instance)
                {
                    return Err(format!(
                        "CPU certificate omitted grouped owner {}",
                        summary.definition
                    ));
                }
                affected.insert(summary.instance);
            }
        }
        if affected.len() == before {
            break;
        }
    }
    let names: BTreeSet<_> = program
        .units
        .iter()
        .filter(|u| affected.contains(&u.unit().header.instance))
        .map(|u| {
            hirsel_lir::emission::symbol_name(
                &u.unit().header.definition,
                Some(&u.unit().header.instance),
            )
        })
        .collect();
    let units: Vec<_> = program
        .units
        .iter()
        .filter(|u| affected.contains(&u.unit().header.instance))
        .collect();
    let empty = BTreeSet::new();
    let inputs = hirsel_lir::emission::EmitInputs {
        ahead: Some(&plan),
        continuation: None,
        alternate: false,
        index: None,
        input: &program.input,
        target: &program.target,
        layouts: &program.table,
        keys: &program.keys,
        manifest: &program.manifest,
        statics: &program.statics,
        inline_instances: &empty,
        available_externally: &empty,
        callbacks: !program.callbacks.is_empty(),
        tracking: program.tracking(),
        foreign: &program.foreign_dispatch,
        opt,
        measurement: false,
        fuel: program.fuel,
        verify_backend: true,
    };
    let ordinary = match backend {
        crate::native::Backend::Llvm => {
            hirsel_llvm::emit_object(&units, &inputs).map_err(|e| e.to_string())?
        }
        crate::native::Backend::Cranelift => {
            hirsel_cranelift::emit_object(&units, &inputs).map_err(|e| e.to_string())?
        }
    };
    let registry = hirsel_lir::descriptor_registry::DescriptorRegistry::new(
        &program.table,
        &program.finalizers,
    );
    let ahead = crate::native::ahead::objects_with_plan(program, backend, opt, &registry, &plan)
        .map_err(|e| e.to_string())?;
    objects.retain(|object| {
        !object
            .exports
            .iter()
            .any(|symbol| names.iter().any(|name| symbol.name.starts_with(name)))
    });
    objects.push(ordinary);
    objects.extend(ahead);
    crate::native::ahead::check_objects_with_plan(program, objects, &plan)
        .map_err(|e| e.to_string())
}

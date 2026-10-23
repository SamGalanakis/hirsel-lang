//! Bounded explicit captures, following Goose 8511ae34 codegen_frames.h:115,
//! 210,329 and codegen_calls.h:245. Hirsel keeps source loans and cleanup and
//! requires independent strict V and complete C before and after the rewrite.
//! Unsupported use or a budget failure discards the complete site transaction.
use crate::{
    closure_calls::{MAX_PER_FUNCTION, MAX_SPECIALIZATIONS, Specializations},
    import::Lowered,
    instance::Program,
};
use hirsel_core_contract::Passing;
use hirsel_lir::{
    abi_types::*,
    ir::*,
    layout_types::{LayoutId, LayoutKind, LayoutTable},
    stage::Value,
};
use hirsel_syntax::ContentHash;
use std::collections::{BTreeMap, BTreeSet};

type Unit = LirUnit<Value>;

pub(crate) fn specialize(
    lowered: &mut Lowered,
    program: &Program,
    target: &hirsel_lir::TargetSpec,
    result: &mut Specializations,
) {
    // Rejection only: a surviving site still needs independent source V and
    // complete linked C, before and after its ordinary transaction.
    let mut candidates = discover(lowered);
    if candidates.iter().all(Vec::is_empty) {
        return;
    }
    if !source_qualifies(program) {
        return;
    }
    let manifest = hirsel_core_contract::Manifest::skeleton(program.operations.values().cloned());
    let operations = program.operation_registry();
    let mut qualification =
        hirsel_lir::validate::ScopedQualification::new(target, &manifest, &operations, None);
    let mut qualifies =
        |units: &[Unit], table: &LayoutTable| qualification.qualify(units, table).is_ok();
    if !qualifies(&lowered.units, &lowered.table) {
        return;
    }
    let baseline = lowered.instances.len();
    let mut at = 0;
    while at < lowered.units.len() {
        let mut changed = false;
        for candidate in &candidates[at] {
            let Candidate {
                root,
                site,
                body,
                aliases,
            } = candidate;
            let captures = lowered.units[at].tables.closures[site.index()]
                .captures
                .get(&lowered.units[at].pools.captures)
                .unwrap_or(&[]);
            let types: Vec<_> = captures
                .iter()
                .map(|c| {
                    lowered.units[at].types[lowered.units[at].values[c.value.index()].ty.index()]
                        .clone()
                })
                .collect();
            let mut transaction = Transaction {
                units: lowered.units.clone(),
                table: lowered.table.clone(),
                origins: result.origins.clone(),
                keys: result.keys.clone(),
                inline: result.inline.clone(),
                baseline,
                target: target.clone(),
                cache: BTreeMap::new(),
                active: BTreeSet::new(),
            };
            let attempt = (|| {
                let entry = transaction.capture_body(*body, &types)?;
                let values = captures.iter().map(|c| c.value).collect::<Vec<_>>();
                transaction.rewrite(at, aliases, &values, entry)?;
                erase(
                    &mut transaction.units[at],
                    aliases,
                    &transaction.table,
                    Some(*root),
                )?;
                Ok::<_, ()>(())
            })();
            if attempt.is_ok() && qualifies(&transaction.units, &transaction.table) {
                lowered.units = transaction.units;
                lowered.table = transaction.table;
                result.origins = transaction.origins;
                result.keys = transaction.keys;
                result.inline = transaction.inline;
                changed = true;
                break;
            }
        }
        if changed {
            // Every summary belongs to this exact immutable lowering. A
            // committed rewrite invalidates it, including new nested bodies.
            candidates = discover(lowered);
        } else {
            at += 1;
        }
    }
}

/// Reconstruct the defining package's source views from existing owner input.
/// Native export seals public opaque signatures but retains their layout shape.
/// Only ordinary non-carrier nominals are complete with that shape restored:
/// capability ceilings and carrier selectors need a fuller declaration and
/// therefore keep the original conservative refusal. V still checks every body.
fn source_qualifies(program: &Program) -> bool {
    let packages: BTreeSet<_> = program
        .definitions
        .values()
        .filter(|d| !d.bodies.is_empty())
        .map(|d| &d.path.path().0.package)
        .collect();
    for package in packages {
        let mut signatures = hirsel_validate::SignatureMap::new(program.types.clone());
        for (path, signature) in &program.signatures {
            let mut view = (**signature).clone();
            if &path.0.package == package
                && view.metadata.visibility == hirsel_syntax::Visibility::PublicOpaque
                && let hirsel_types::SignatureKind::Type(declaration) = &mut view.kind
                && declaration.representation.is_none()
                && declaration.capability.is_none()
                && declaration.captures.is_none()
                && let Some(hirsel_layout::layout::OwnerRepresentation::Defined(owner)) = program
                    .owners
                    .nominals
                    .get(&hirsel_syntax::DefId::intern(path))
                && owner.definition == *path
                && owner.generics == declaration.generics
                && owner.layout_promises == declaration.layout
                && owner.resource == declaration.resource
                && owner.finalizer.as_ref().map(|(path, _)| path) == declaration.drop.as_ref()
                && owner.unsafe_ffi_package == view.metadata.unsafe_ffi
            {
                declaration.representation = Some(owner.private_shape);
            }
            signatures.insert(hirsel_syntax::DefId::intern(path), view);
        }
        for definition in program
            .definitions
            .values()
            .filter(|d| !d.bodies.is_empty() && &d.path.path().0.package == package)
        {
            if !hirsel_validate::qualify_scoped_transport(
                definition,
                &program.types,
                &signatures,
                &mut vec![],
            )
            .violations
            .is_empty()
            {
                return false;
            }
        }
    }
    true
}

struct Candidate {
    root: ValueId,
    site: ClosureId,
    body: usize,
    aliases: BTreeSet<ValueId>,
}

fn discover(lowered: &Lowered) -> Vec<Vec<Candidate>> {
    lowered
        .units
        .iter()
        .enumerate()
        .map(|(at, u)| {
            u.instructions
                .iter()
                .filter_map(|i| {
                    let Inst::Closure {
                        result, closure, ..
                    } = *i
                    else {
                        return None;
                    };
                    let (body, aliases) = preflight(lowered, at, result, closure).ok()?;
                    Some(Candidate {
                        root: result,
                        site: closure,
                        body,
                        aliases,
                    })
                })
                .collect()
        })
        .collect()
}

/// Scan immutable completed bodies and follow capture consumers, after Goose
/// codegen_frames.h:115–154,210–235. Unlike its emission summaries, this is only
/// a structural rejection prepass: no retained proof or signature-only cache.
fn preflight(
    lowered: &Lowered,
    at: usize,
    root: ValueId,
    site: ClosureId,
) -> Result<(usize, BTreeSet<ValueId>), ()> {
    let u = &lowered.units[at];
    let closure = &u.tables.closures[site.index()];
    if !closure.scoped {
        return Err(());
    }
    let code = closure.code.ok_or(())?;
    let captures = closure.captures.get(&u.pools.captures).unwrap_or(&[]);
    // There is no environment allocation to eliminate. Ordinary code-target
    // specialization has already run, including these captureless callables.
    if captures.is_empty()
        && matches!(
            lowered.table.layouts[closure.environment.0 as usize].size,
            hirsel_lir::layout_types::SizeSpec::Fixed(0)
        )
    {
        return Err(());
    }

    if captures.iter().any(|c| {
        c.usage != CaptureUse::Borrow
            && !(c.usage == CaptureUse::Copy && u.values[c.value.index()].holding == Holding::Plain)
    }) {
        return Err(());
    }
    let body = find(&lowered.units, &u.tables.symbols[code.symbol.index()]).ok_or(())?;
    let captured = &lowered.units[body];
    let abi = &lowered.table.fn_abis[captured.header.abi.0 as usize];
    if captured.captures.len() != captures.len()
        || abi.multiplicity != CallMultiplicity::Many
        || !abi.nonretaining_captures.is_empty()
        || !captured.tables.init_flags.is_empty()
        || lowered.table.fn_abis[code.abi.0 as usize]
            .result
            .as_ref()
            .is_some_and(|r| {
                !matches!(
                    lowered.table.layouts[r.layout.0 as usize].kind,
                    LayoutKind::Scalar(_) | LayoutKind::Unit
                )
            })
    {
        return Err(());
    }
    let aliases = aliases(u, root)?;
    preflight_uses(lowered, at, &aliases, &mut BTreeSet::new())?;
    // Append retains the exact existing budgets and can reuse a content-derived
    // specialization even when its owner's budget is already full.
    Ok((body, aliases))
}

fn preflight_uses(
    lowered: &Lowered,
    at: usize,
    aliases: &BTreeSet<ValueId>,
    seen: &mut BTreeSet<(usize, u32)>,
) -> Result<(), ()> {
    // Bound this speculative walk by the existing transaction's global budget.
    // Reuse can avoid appending bodies, so exhaustion here only stops looking
    // for rejections; append still decides the exact per-owner/global limits.
    if seen.len() >= MAX_SPECIALIZATIONS {
        return Ok(());
    }
    let u = &lowered.units[at];
    carrier_slots(u, aliases)?;
    for call in &u.tables.calls {
        if call.callee.value().is_some_and(|v| aliases.contains(&v)) {
            continue;
        }
        let mut parameters = call
            .arguments
            .get(&u.pools.call_arguments)
            .unwrap_or(&[])
            .iter()
            .filter_map(|a| match a.value {
                ArgumentValue::Value(v) if aliases.contains(&v) => Some(a.parameter),
                _ => None,
            });
        let Some(parameter) = parameters.next() else {
            continue;
        };
        if parameters.next().is_some() {
            return Err(());
        }
        let Callee::Direct(symbol) = call.callee else {
            return Err(());
        };
        let body = find(&lowered.units, &u.tables.symbols[symbol.index()]).ok_or(())?;
        let consumer = &lowered.units[body];
        if !lowered.table.fn_abis[consumer.header.abi.0 as usize]
            .nonretaining
            .iter()
            .any(|p| p.parameter == parameter && p.fields.is_empty())
        {
            return Err(());
        }
        if seen.insert((body, parameter)) {
            let root = *consumer.parameters.get(parameter as usize).ok_or(())?;
            let aliases = self::aliases(consumer, root)?;
            preflight_uses(lowered, body, &aliases, seen)?;
        }
    }
    Ok(())
}

fn find(units: &[Unit], s: &Symbol) -> Option<usize> {
    units
        .iter()
        .position(|u| s.instance == Some(u.header.instance) && s.definition == u.header.definition)
}
fn symbol(u: &mut Unit, target: &Unit) -> SymbolId {
    let s = Symbol {
        kind: SymbolKind::Function,
        definition: target.header.definition.clone(),
        instance: Some(target.header.instance),
    };
    if let Some(i) = u.tables.symbols.iter().position(|p| p == &s) {
        return SymbolId(i as u32);
    }
    let id = SymbolId(u.tables.symbols.len() as u32);
    u.tables.symbols.push(s);
    id
}
fn whole(u: &Unit, a: AddressId) -> Option<SlotId> {
    let a = &u.addresses[a.index()];
    if a.projection.len != 0 {
        return None;
    }
    match a.base {
        AddressBase::Slot(s) => Some(s),
        AddressBase::Address(a) => whole(u, a),
        _ => None,
    }
}
/// A single creation's SSA carriers. A join with any other value, including an
/// equal-code/different-data creation, is unsupported as a whole site.
fn aliases(u: &Unit, root: ValueId) -> Result<BTreeSet<ValueId>, ()> {
    let mut out = BTreeSet::from([root]);
    loop {
        let before = out.len();
        for i in &u.instructions {
            let flow = match *i {
                Inst::Read {
                    result,
                    address,
                    reaching,
                    ..
                }
                | Inst::Copy {
                    result,
                    address,
                    reaching,
                    ..
                } if whole(u, address).is_some() => Some((result, reaching)),
                Inst::Store {
                    address,
                    value,
                    version,
                } if whole(u, address).is_some() => Some((version, value)),
                Inst::Weaken { result, value } => Some((result, value)),
                _ => None,
            };
            if let Some((to, from)) = flow {
                if out.contains(&from) {
                    out.insert(to);
                }
            }
        }
        for e in &u.tables.edges {
            for (&p, &a) in u.blocks[e.target.index()]
                .parameters
                .get(&u.pools.values)
                .unwrap_or(&[])
                .iter()
                .zip(e.arguments.get(&u.pools.values).unwrap_or(&[]))
            {
                if out.contains(&a) {
                    out.insert(p);
                }
            }
        }
        if before == out.len() {
            break;
        }
    }
    for e in &u.tables.edges {
        for (&p, &a) in u.blocks[e.target.index()]
            .parameters
            .get(&u.pools.values)
            .unwrap_or(&[])
            .iter()
            .zip(e.arguments.get(&u.pools.values).unwrap_or(&[]))
        {
            if out.contains(&p) != out.contains(&a) {
                return Err(());
            }
        }
    }
    Ok(out)
}

struct Transaction {
    units: Vec<Unit>,
    table: LayoutTable,
    origins: Vec<crate::instance::InstanceId>,
    keys: Vec<hirsel_layout::instance::InstanceKey>,
    inline: BTreeSet<ContentHash>,
    baseline: usize,
    target: hirsel_lir::TargetSpec,
    cache: BTreeMap<(usize, u32, usize), usize>,
    active: BTreeSet<(usize, u32, usize)>,
}
impl Transaction {
    fn append(
        &mut self,
        mut u: Unit,
        original: usize,
        mut abi: FnAbi,
        types: &[LirType],
        domain: &str,
        source_key: hirsel_layout::instance::InstanceKey,
    ) -> Result<(usize, bool), ()> {
        let owner = self.origins[original];
        for ty in types {
            let pointer = PhysicalSlot {
                ty: PhysicalType::Pointer,
                size: self
                    .target
                    .scalar_size(hirsel_lir::layout_types::ScalarKind::Pointer),
                align: self
                    .target
                    .scalar_align(hirsel_lir::layout_types::ScalarKind::Pointer),
            };
            let (passing, slots, extension) = match self.table.layouts[ty.layout.0 as usize].kind {
                LayoutKind::Scalar(k) => (
                    ParameterPassing::ByValue,
                    vec![hirsel_lir::abi::slot(&self.target, k)],
                    hirsel_lir::abi::extension_of(k),
                ),
                LayoutKind::Unit => (ParameterPassing::ByValue, Vec::new(), ScalarExtension::None),
                _ => (
                    ParameterPassing::ReadOnlyAddress,
                    vec![pointer],
                    ScalarExtension::None,
                ),
            };
            abi.parameters.push(AbiParameter {
                source: abi.parameters.len() as u32,
                convention: Passing::Borrow,
                passing,
                layout: ty.layout,
                slots,
                extension,
            });
        }
        abi.shape = CallableShape::Definition;
        abi.environment = false;
        abi.scoped_contract =
            hirsel_lir::abi::scoped_contract_digest(&abi.nonretaining, &abi.nonretaining_captures);
        let key = hirsel_lir::LayoutKeys::new(&self.table).fn_abi(&abi);
        let encoded = ContentHash::of_parts(domain, [key.0.as_slice()]);
        let effective = source_key.derived(
            "hirsel.scoped-explicit-captures.v1",
            &[encoded.0.as_slice()],
        );
        u.header.instance = effective.digest();
        abi.key.callable = ContentHash::of_parts(
            "hirsel.scoped-direct-abi.v1",
            [abi.key.callable.0.as_slice(), encoded.0.as_slice()],
        );
        if let Some(i) = self.units.iter().position(|old| {
            old.header.definition == u.header.definition && old.header.instance == u.header.instance
        }) {
            return Ok((i, false));
        }
        if self.units.len() - self.baseline >= MAX_SPECIALIZATIONS
            || self.origins.iter().filter(|&&o| o == owner).count() > MAX_PER_FUNCTION
        {
            return Err(());
        }
        u.header.abi = FnAbiId(self.table.fn_abis.len() as u32);
        self.table.fn_abis.push(abi);
        self.inline.insert(u.header.instance);
        self.units.push(u);
        self.origins.push(owner);
        self.keys.push(effective);
        Ok((self.units.len() - 1, true))
    }
    fn capture_body(&mut self, body: usize, types: &[LirType]) -> Result<usize, ()> {
        let mut u = self.units[body].clone();
        let mut abi = self.table.fn_abis[u.header.abi.0 as usize].clone();
        if u.captures.len() != types.len()
            || abi.multiplicity != CallMultiplicity::Many
            || !abi.nonretaining_captures.is_empty()
            || !u.tables.init_flags.is_empty()
        {
            return Err(());
        }
        let base = u.parameters.len() as u32;
        for s in &mut u.slots {
            if let SlotKind::Capture(i) = s.kind {
                s.kind = SlotKind::Parameter(base + i);
            }
        }
        u.parameters.append(&mut u.captures);
        abi.nonretaining_captures.clear();
        self.append(
            u,
            body,
            abi,
            types,
            "scoped-capture-body",
            self.keys[body].clone(),
        )
        .map(|(i, _)| i)
    }
    fn consumer(
        &mut self,
        original: usize,
        parameter: u32,
        entry: usize,
        types: &[LirType],
    ) -> Result<usize, ()> {
        let key = (original, parameter, entry);
        if let Some(&i) = self.cache.get(&key) {
            return Ok(i);
        }
        if !self.active.insert(key) {
            return Err(());
        }
        let mut u = self.units[original].clone();
        let mut abi = self.table.fn_abis[u.header.abi.0 as usize].clone();
        if !abi
            .nonretaining
            .iter()
            .any(|p| p.parameter == parameter && p.fields.is_empty())
        {
            return Err(());
        }
        let source_key = self.keys[original].derived(
            "hirsel.scoped-consumer-target.v1",
            &[
                self.units[entry].header.instance.0.as_slice(),
                &parameter.to_le_bytes(),
            ],
        );
        u.header.instance = source_key.digest();
        let root = *u.parameters.get(parameter as usize).ok_or(())?;
        let aliases = aliases(&u, root)?;
        let unit = self
            .table
            .layouts
            .iter()
            .position(|r| r.kind == LayoutKind::Unit)
            .ok_or(())?;
        let p = abi.parameters.get_mut(parameter as usize).ok_or(())?;
        p.layout = LayoutId(unit as u32);
        p.passing = ParameterPassing::ByValue;
        p.slots.clear();
        p.extension = ScalarExtension::None;
        abi.nonretaining.retain(|p| p.parameter != parameter);
        abi.scoped_entries
            .retain(|p| p.parameter.parameter != parameter);
        let mut values = Vec::new();
        for ty in types {
            let t = LirTypeId(u.types.len() as u32);
            u.types.push(ty.clone());
            let v = ValueId(u.values.len() as u32);
            u.values.push(ValueInfo {
                ty: t,
                holding: Holding::Plain,
            });
            u.parameters.push(v);
            values.push(v);
        }
        let (i, created) =
            self.append(u, original, abi, types, "scoped-consumer-body", source_key)?;
        self.cache.insert(key, i);
        if !created {
            self.active.remove(&key);
            return Ok(i);
        }
        self.rewrite(i, &aliases, &values, entry)?;
        erase(&mut self.units[i], &aliases, &self.table, None)?;
        self.active.remove(&key);
        Ok(i)
    }
    fn rewrite(
        &mut self,
        at: usize,
        aliases: &BTreeSet<ValueId>,
        captures: &[ValueId],
        entry: usize,
    ) -> Result<(), ()> {
        let types: Vec<_> = captures
            .iter()
            .map(|v| self.units[at].types[self.units[at].values[v.index()].ty.index()].clone())
            .collect();
        for c in 0..self.units[at].tables.calls.len() {
            let site = self.units[at].tables.calls[c].clone();
            let mut args = site
                .arguments
                .get(&self.units[at].pools.call_arguments)
                .unwrap_or(&[])
                .to_vec();
            let target = if site.callee.value().is_some_and(|v| aliases.contains(&v)) {
                Some(entry)
            } else {
                let params: Vec<_> = args
                    .iter()
                    .filter_map(|a| match a.value {
                        ArgumentValue::Value(v) if aliases.contains(&v) => Some(a.parameter),
                        _ => None,
                    })
                    .collect();
                if params.is_empty() {
                    continue;
                }
                if params.len() != 1 {
                    return Err(());
                }
                let Callee::Direct(s) = site.callee else {
                    return Err(());
                };
                let original =
                    find(&self.units, &self.units[at].tables.symbols[s.index()]).ok_or(())?;
                Some(self.consumer(original, params[0], entry, &types)?)
            };
            let target = target.unwrap();
            let target_unit = self.units[target].clone();
            let abi = target_unit.header.abi;
            let base = self.table.fn_abis[abi.0 as usize].parameters.len() - captures.len();
            args.extend(captures.iter().enumerate().map(|(i, &v)| CallArgument {
                parameter: (base + i) as u32,
                passing: Passing::Borrow,
                value: ArgumentValue::Value(v),
            }));
            let u = &mut self.units[at];
            let code = symbol(u, &target_unit);
            let arguments = PoolRange::push(&mut u.pools.call_arguments, args);
            u.tables.calls[c].callee = Callee::Direct(code);
            u.tables.calls[c].abi = abi;
            u.tables.calls[c].arguments = arguments;
        }
        Ok(())
    }
}

/// The same structural refusal checks used by erase, without editing the body.
fn carrier_slots(u: &Unit, aliases: &BTreeSet<ValueId>) -> Result<BTreeSet<SlotId>, ()> {
    let mut slots = BTreeSet::new();
    for inst in &u.instructions {
        match *inst {
            Inst::Store { address, value, .. } if aliases.contains(&value) => {
                slots.insert(whole(u, address).ok_or(())?);
            }
            Inst::Read {
                result, address, ..
            }
            | Inst::Copy {
                result, address, ..
            } if aliases.contains(&result) => {
                slots.insert(whole(u, address).ok_or(())?);
            }
            _ => {}
        }
    }
    for (i, s) in u.slots.iter().enumerate() {
        if let SlotKind::Parameter(p) = s.kind {
            if u.parameters
                .get(p as usize)
                .is_some_and(|v| aliases.contains(v))
            {
                slots.insert(SlotId(i as u32));
            }
        }
    }
    if u.tables
        .loans
        .iter()
        .any(|l| l.mutable && whole(u, l.address).is_some_and(|s| slots.contains(&s)))
    {
        return Err(());
    }
    for inst in &u.instructions {
        let used = crate::liveness::inst_uses(u, inst)
            .iter()
            .any(|v| aliases.contains(v));
        if used
            && !matches!(
                inst,
                Inst::Read { .. } | Inst::Copy { .. } | Inst::Store { .. } | Inst::Weaken { .. }
            )
        {
            return Err(());
        }
        if let Inst::Store { address, value, .. } = *inst {
            if whole(u, address).is_some_and(|s| slots.contains(&s)) && !aliases.contains(&value) {
                return Err(());
            }
        }
    }
    for term in &u.terminators {
        if !matches!(term, Terminator::Call { .. })
            && crate::liveness::terminator_uses(u, term)
                .iter()
                .any(|v| aliases.contains(v))
        {
            return Err(());
        }
    }
    Ok(slots)
}

/// Replace only the proved dead callable carriers with Unit. This is ordinary
/// value IR, with no pointer interpretation, storage extent or runtime change.
fn erase(
    u: &mut Unit,
    aliases: &BTreeSet<ValueId>,
    table: &LayoutTable,
    creation: Option<ValueId>,
) -> Result<(), ()> {
    let layout = LayoutId(
        table
            .layouts
            .iter()
            .position(|r| r.kind == LayoutKind::Unit)
            .ok_or(())? as u32,
    );
    let ty = LirTypeId(u.types.len() as u32);
    u.types.push(LirType {
        repr: ValueRepr::Unit,
        layout,
    });
    let slots = carrier_slots(u, aliases)?;
    for &v in aliases {
        u.values[v.index()].ty = ty;
        u.values[v.index()].holding = Holding::Plain;
    }
    for &s in &slots {
        u.slots[s.index()].ty = ty;
    }
    let addresses: Vec<_> = (0..u.addresses.len())
        .map(|i| whole(u, AddressId(i as u32)).is_some_and(|s| slots.contains(&s)))
        .collect();
    for (a, replace) in u.addresses.iter_mut().zip(addresses) {
        if replace {
            a.ty = ty;
        }
    }
    let constant = ConstantId(u.tables.constants.len() as u32);
    u.tables.constants.push(ConstantData {
        ty,
        value: ConstantValue::Unit,
    });
    for inst in &mut u.instructions {
        match *inst {
            Inst::Closure { result, .. } if creation == Some(result) => {
                *inst = Inst::Const { result, constant }
            }
            Inst::Copy {
                result,
                address,
                reaching,
                ..
            } if aliases.contains(&result) => {
                *inst = Inst::Read {
                    result,
                    address,
                    reaching,
                    mode: ReadMode::Bits,
                }
            }
            Inst::Read {
                result,
                ref mut mode,
                ..
            } if aliases.contains(&result) => *mode = ReadMode::Bits,
            _ => {}
        }
    }
    *u = crate::insert::Editor::new(u.clone()).finish();
    Ok(())
}

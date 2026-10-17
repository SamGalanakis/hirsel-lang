//! Local storage and places; the library extends values and projections here.
use crate::{Error, Value, invalid};
use hirsel_tir::LocalId;
use hirsel_tir::*;
use hirsel_types::UseKind;

/// Separate a source payload tag check from malformed storage instructions.
#[derive(Debug)]
pub(crate) enum AccessError {
    InactiveVariant,
    OutOfBounds,
    Api(Error),
}
impl From<Error> for AccessError {
    fn from(error: Error) -> Self {
        Self::Api(error)
    }
}
impl From<AccessError> for Error {
    fn from(error: AccessError) -> Self {
        match error {
            AccessError::Api(error) => error,
            // Compiler-owned writebacks keep the tag protected by the emitted
            // mutable loan. A changed tag there indicates inconsistent TIR.
            AccessError::InactiveVariant => {
                invalid("inactive variant in protected storage operation")
            }
            AccessError::OutOfBounds => invalid("out-of-bounds protected storage operation"),
        }
    }
}

enum AccessStep {
    Field(usize),
    VariantField { variant: u32, field: usize },
    Index(usize),
}

pub(crate) struct Continuation {
    /// None for a diverging call, whose callee never returns normally.
    pub returns: Option<hirsel_tir::CallReturn>,
    pub on_failure: CleanupId,
    pub writebacks: Vec<(PlaceId, LocalId)>,
}
pub(crate) struct Frame {
    pub definition: DefinitionRef,
    pub body_id: BodyId,
    pub mut_parameters: Vec<LocalId>,
    pub operation: Option<(crate::OperationResult, crate::resources::OperationState)>,
    pub callback: Option<crate::resources::Callback>,
    pub work: std::collections::VecDeque<crate::resources::Work>,
    pub ending: Option<crate::resources::Ending>,
    pub finalized: Option<(LocalId, crate::observation::Anchor)>,
    pub loop_owned: Vec<(Value, crate::observation::Anchor)>,
    pub slots: Vec<Option<Value>>,
    pub sizes: crate::value::SizeBindings,
    pub block: BlockId,
    pub instruction: usize,
    pub continuation: Option<Continuation>,
    pub depth_failure: bool,
    /// The site of an origin binding whose place is out of bounds or names an
    /// inactive variant. The access that then fails reports this site: the
    /// expression naming the place, not the statement reading it (docs/spec/diagnostics.md).
    pub place_failure_site: Option<hirsel_span::Span>,
    /// Lifetimes of this frame's values under checked execution.
    pub check: Option<crate::checked::FrameCheck>,
    /// An `arena.bounded` callback's boundary (MEM-6).
    pub bounded: Option<crate::bounded::Boundary>,
}
impl Frame {
    /// Replace value storage while retaining the lexical size environment and
    /// the caller's writeback/finalizer continuation. The witness proves that
    /// old-frame owners are moved, plain, or carried by `loop_owned`.
    pub fn restart_tail(&mut self, arguments: Vec<Value>) -> Result<(), Error> {
        let body = self.body();
        if arguments.len() != body.parameters.len() {
            return Err(invalid("tail parameter arity"));
        }
        let parameters = body.parameters.clone();
        let entry = body.entry;
        let mut slots = vec![None; body.locals.len()];
        for (local, mut value) in parameters.into_iter().zip(arguments) {
            value.canonicalize();
            let slot = slots
                .get_mut(local.0 as usize)
                .ok_or_else(|| invalid("tail parameter index"))?;
            if slot.is_some() {
                return Err(invalid("duplicate tail parameter"));
            }
            *slot = Some(value);
        }
        self.slots = slots;
        self.goto(entry);
        Ok(())
    }

    pub fn size(&self, parameter: hirsel_types::SizeParameterRef) -> Result<u64, Error> {
        let hirsel_types::SizeParameterRef::Body { scope, parameter } = parameter else {
            return Err(invalid("unclosed generic size parameter"));
        };
        self.sizes
            .get(&(scope, parameter))
            .copied()
            .ok_or_else(|| invalid("unbound generic size parameter"))
    }
    /// Bind the actual arguments of exactly one invocation, preserving captures.
    pub fn bind_sizes(
        sizes: &mut crate::value::SizeBindings,
        scope: hirsel_types::NameSite,
        arguments: impl IntoIterator<Item = Option<u64>>,
    ) {
        for (ordinal, size) in arguments.into_iter().enumerate() {
            if let Some(size) = size {
                sizes.insert((scope, hirsel_types::SizeParamId(ordinal as u32)), size);
            }
        }
    }
    pub fn new(definition: DefinitionRef, arguments: Vec<Value>) -> Result<Self, Error> {
        let entry = definition.entry;
        Self::new_body(definition, entry, arguments, vec![], Default::default())
    }
    pub fn new_body(
        definition: DefinitionRef,
        body_id: BodyId,
        arguments: Vec<Value>,
        captures: Vec<Option<Value>>,
        sizes: crate::value::SizeBindings,
    ) -> Result<Self, Error> {
        let body = definition
            .bodies
            .get(body_id.0 as usize)
            .ok_or_else(|| invalid("missing entry body"))?;
        if body.parameters.len() != arguments.len() {
            return Err(invalid("parameter count mismatch"));
        }
        let mut slots = vec![None; body.locals.len()];
        for (local, mut value) in body.parameters.iter().zip(arguments) {
            value.canonicalize();
            let slot = slots
                .get_mut(local.0 as usize)
                .ok_or_else(|| invalid("parameter slot out of range"))?;
            if slot.is_some() {
                return Err(invalid("duplicate parameter slot"));
            }
            *slot = Some(value);
        }
        let capture_slots: Vec<_> = body
            .locals
            .iter()
            .enumerate()
            .filter(|(_, l)| l.storage == Storage::Capture)
            .map(|(i, _)| i)
            .collect();
        if capture_slots.len() != captures.len() {
            return Err(invalid("capture count mismatch"));
        }
        for (index, value) in capture_slots.into_iter().zip(captures) {
            slots[index] = value;
        }
        let block = body.entry;
        Ok(Self {
            sizes,
            definition,
            body_id,
            mut_parameters: vec![],
            operation: None,
            callback: None,
            work: std::collections::VecDeque::new(),
            ending: None,
            finalized: None,
            loop_owned: vec![],
            slots,
            block,
            instruction: 0,
            continuation: None,
            depth_failure: false,
            place_failure_site: None,
            check: None,
            bounded: None,
        })
    }
    pub fn body(&self) -> &Body {
        &self.definition.bodies[self.body_id.0 as usize]
    }
    pub fn place(&self, id: PlaceId) -> Result<&Place, Error> {
        self.body()
            .places
            .get(id.0 as usize)
            .ok_or_else(|| invalid("place out of range"))
    }
    fn indices(&self, place: &Place) -> Result<Vec<AccessStep>, AccessError> {
        let mut steps = vec![];
        let mut value = self
            .slots
            .get(place.root.0 as usize)
            .and_then(Option::as_ref);
        for projection in &place.projection {
            let step = match *projection {
                Projection::Field(field) => AccessStep::Field(field as usize),
                Projection::VariantField { variant, field } => AccessStep::VariantField {
                    variant,
                    field: field as usize,
                },
                Projection::Index(LocalId(index)) => {
                    let index = match self.slots.get(index as usize).and_then(Option::as_ref) {
                        Some(Value::Integer(i)) if !i.is_signed() || i.signed() >= 0 => {
                            usize::try_from(i.bits()).map_err(|_| AccessError::OutOfBounds)?
                        }
                        _ => return Err(invalid("invalid index operand").into()),
                    };
                    // SortedMap's logical indexed payload is its physical map
                    // field. Value access and shadow access use the same path.
                    if matches!(value, Some(Value::Record { definition, .. })
                        if *definition == hirsel_syntax::DefPath::core("SortedMap"))
                    {
                        steps.push(AccessStep::Field(1));
                        value = value
                            .and_then(Value::fields)
                            .and_then(|f| f.get(1))
                            .and_then(Option::as_ref);
                    }
                    AccessStep::Index(index)
                }
            };
            let index = match step {
                AccessStep::Field(i)
                | AccessStep::VariantField { field: i, .. }
                | AccessStep::Index(i) => i,
            };
            value = value
                .and_then(Value::fields)
                .and_then(|f| f.get(index))
                .and_then(Option::as_ref);
            steps.push(step);
        }
        Ok(steps)
    }
    pub fn slot(&mut self, id: PlaceId) -> Result<&mut Option<Value>, AccessError> {
        let place = self.place(id)?.clone();
        let indices = self.indices(&place)?;
        let mut slot = self
            .slots
            .get_mut(place.root.0 as usize)
            .ok_or_else(|| invalid("local out of range"))?;
        for step in indices {
            let (index, variant, indexed) = match step {
                AccessStep::Field(index) => (index, None, false),
                AccessStep::VariantField { variant, field } => (field, Some(variant), false),
                AccessStep::Index(index) => (index, None, true),
            };
            let fields = match slot.as_mut() {
                Some(v)
                    if variant.is_none()
                        && !matches!(v, Value::Variant { .. } | Value::ResourceVariant { .. }) =>
                {
                    v.fields_mut()
                        .ok_or_else(|| invalid("projection on scalar"))?
                }
                Some(
                    Value::Variant {
                        variant: tag,
                        fields,
                    }
                    | Value::ResourceVariant {
                        variant: tag,
                        fields,
                        ..
                    },
                ) if variant == Some(*tag) => fields,
                Some(
                    Value::Variant { variant: tag, .. }
                    | Value::ResourceVariant { variant: tag, .. },
                ) if variant.is_some() && variant != Some(*tag) => {
                    return Err(AccessError::InactiveVariant);
                }
                _ => return Err(invalid("projection on dead or incompatible storage").into()),
            };
            slot = fields.get_mut(index).ok_or_else(|| {
                if indexed {
                    AccessError::OutOfBounds
                } else {
                    invalid("field out of range").into()
                }
            })?;
        }
        Ok(slot)
    }
    /// Observe table patterns without extracting, copying, or changing live bits.
    pub fn pattern_matches(&self, table: &MatchTable, root: MatchPatternId) -> Result<bool, Error> {
        let mut pending = vec![root];
        while let Some(id) = pending.pop() {
            let pattern = &table.patterns[id.0 as usize];
            let value = self.observe(pattern.place)?;
            match (&pattern.kind, value) {
                (MatchPatternKind::Wildcard | MatchPatternKind::Binding { .. }, _) => (),
                (MatchPatternKind::Literal(c), v) => {
                    let matches = match (c, v) {
                        (Constant::Unit, Value::Unit) => true,
                        (Constant::Bool(a), Value::Bool(b)) => a == b,
                        (Constant::Integer(a), Value::Integer(b)) => *a == b.bits(),
                        (Constant::F32(a), Value::F32(b)) => {
                            f32::from_bits(*a) == f32::from_bits(*b)
                        }
                        (Constant::F64(a), Value::F64(b)) => {
                            f64::from_bits(*a) == f64::from_bits(*b)
                        }
                        (Constant::String(a), Value::String(b)) => a == b,
                        (Constant::Bytes(a), Value::Bytes(b)) => a == b,
                        _ => false,
                    };
                    if !matches {
                        return Ok(false);
                    }
                }
                (
                    MatchPatternKind::Variant { variant, .. },
                    Value::Variant { variant: tag, .. }
                    | Value::ResourceVariant { variant: tag, .. },
                ) => {
                    if variant != tag {
                        return Ok(false);
                    }
                }
                (MatchPatternKind::Tuple(_), Value::Tuple(_) | Value::Unit)
                | (
                    MatchPatternKind::Record { .. },
                    Value::Record { .. } | Value::Resource { .. } | Value::Distinct { .. },
                ) => (),
                _ => return Err(invalid("table pattern/value shape")),
            }
            pending.extend(pattern.kind.fields().iter().rev().map(|(_, child)| *child));
        }
        Ok(true)
    }
    pub fn observe(&self, id: PlaceId) -> Result<&Value, Error> {
        let place = self.place(id)?;
        let mut value = self.slots[place.root.0 as usize]
            .as_ref()
            .ok_or_else(|| invalid("match on dead storage"))?;
        for step in self.indices(place)? {
            let (field, variant) = match step {
                AccessStep::Field(n) | AccessStep::Index(n) => (n, None),
                AccessStep::VariantField { variant, field } => (field, Some(variant)),
            };
            if let Some(expected) = variant {
                if !matches!(value, Value::Variant { variant, .. } | Value::ResourceVariant { variant, .. } if *variant == expected)
                {
                    return Err(invalid("match payload tag"));
                }
            }
            value = value
                .fields()
                .and_then(|fs| fs.get(field))
                .and_then(Option::as_ref)
                .ok_or_else(|| invalid("match field missing"))?;
        }
        Ok(value)
    }
    pub fn operand(&mut self, operand: &Operand) -> Result<Value, AccessError> {
        self.operand_shadow(operand).map(|(value, _)| value)
    }
    /// Extract an operand with its bound; checked execution traps on an ended one.
    pub fn operand_shadow(
        &mut self,
        operand: &Operand,
    ) -> Result<(Value, crate::checked::Shadow), AccessError> {
        let shadow = self.access(operand.place, "operand")?;
        let value = self.extract(operand)?;
        if operand.usage == UseKind::Copy {
            shadow.share();
        }
        let shadow = shadow.for_value(&value);
        Ok((value, shadow))
    }
    fn extract(&mut self, operand: &Operand) -> Result<Value, AccessError> {
        // Bytes indexing is a scalar snapshot, never mutable byte storage.
        // Retain ordinary address bounds and ownership access for the parent.
        let place = self.place(operand.place)?;
        let steps = self.indices(place)?;
        let mut source = self.slots[place.root.0 as usize].as_ref();
        for (depth, step) in steps.iter().enumerate() {
            if let (Some(Value::Bytes(bytes)), AccessStep::Index(index)) = (source, step) {
                if depth + 1 != steps.len() || operand.usage == UseKind::Mut {
                    return Err(invalid("mutable byte projection").into());
                }
                let byte = bytes.get(*index).ok_or(AccessError::OutOfBounds)?;
                return Ok(Value::Integer(
                    crate::Integer::new(hirsel_types::Primitive::U8, u128::from(*byte)).unwrap(),
                ));
            }
            let field = match step {
                AccessStep::Field(i)
                | AccessStep::Index(i)
                | AccessStep::VariantField { field: i, .. } => *i,
            };
            source = source
                .and_then(Value::fields)
                .and_then(|fields| fields.get(field))
                .and_then(Option::as_ref);
        }
        if operand.usage == UseKind::Copy {
            let ty = self
                .definition
                .types
                .get(self.place(operand.place)?.ty)
                .ok_or_else(|| invalid("operand type"))?;
            if ty.modes.multiplicity == hirsel_types::Multiplicity::Once {
                return Err(invalid("copy of once value").into());
            }
            let value = self
                .slot(operand.place)?
                .as_ref()
                .ok_or_else(|| invalid("dead storage"))?;
            let mut work = vec![value];
            while let Some(v) = work.pop() {
                if matches!(v, Value::Resource { .. } | Value::ResourceVariant { .. }) {
                    return Err(invalid("copy of resource-containing value").into());
                }
                if let Value::Closure {
                    captures,
                    owned_captures,
                    ..
                } = v
                {
                    if captures.len() != owned_captures.len() {
                        return Err(invalid("capture ownership count").into());
                    }
                    work.extend(
                        captures
                            .iter()
                            .zip(owned_captures)
                            .filter_map(|(v, owned)| owned.then(|| v.as_ref()).flatten()),
                    );
                } else if let Some(fields) = v.fields() {
                    work.extend(fields.iter().filter_map(Option::as_ref));
                }
            }
        }
        let root = self.place(operand.place)?.root;
        let preserve = self.mut_parameters.contains(&root);
        let slot = self.slot(operand.place)?;
        match operand.usage {
            UseKind::Take if preserve && slot.as_ref().is_some_and(Value::inline_bits) => {
                slot.clone()
            }
            UseKind::Take => slot.take(),
            UseKind::Copy | UseKind::Borrow | UseKind::Mut => slot.clone(),
        }
        .ok_or_else(|| invalid("use of dead storage").into())
    }
    pub fn assign(&mut self, destination: PlaceId, value: Value) -> Result<(), AccessError> {
        self.assign_shadow(destination, value, Default::default())
    }
    pub fn assign_shadow(
        &mut self,
        destination: PlaceId,
        mut value: Value,
        shadow: crate::checked::Shadow,
    ) -> Result<(), AccessError> {
        let slot = self.slot(destination)?;
        if slot.is_some() {
            return Err(invalid("assignment to live storage requires an explicit drop").into());
        }
        value.canonicalize();
        *slot = Some(value);
        self.set_shadow(destination, shadow)?;
        Ok(())
    }
    /// Install storage content at a place, live or dead, with its bound.
    pub fn put(
        &mut self,
        place: PlaceId,
        value: Option<Value>,
        shadow: crate::checked::Shadow,
    ) -> Result<(), AccessError> {
        *self.slot(place)? = value;
        self.set_shadow(place, shadow)?;
        Ok(())
    }
    /// The bound at a place, after checking that nothing on it has ended.
    pub fn access(&self, id: PlaceId, what: &str) -> Result<crate::checked::Shadow, AccessError> {
        let Some(check) = &self.check else {
            return Ok(Default::default());
        };
        let shadow = self.shadow_of(id)?;
        check.state.borrow().access(&shadow, what)?;
        Ok(shadow)
    }
    /// A write through a projection reads every enclosing block on its path.
    pub fn write_access(&self, id: PlaceId) -> Result<(), AccessError> {
        use crate::checked::Shadow;
        let Some(check) = &self.check else {
            return Ok(());
        };
        let place = self.place(id)?;
        if check.noalloc && !place.projection.is_empty() {
            let root = &self.body().locals[place.root.0 as usize];
            let row = self
                .definition
                .types
                .get(root.ty)
                .ok_or_else(|| invalid("write root type"))?;
            // Inline fields are independent stores. Traversing a counted block
            // needs its own uniqueness; arena descendants always detach (MEM-4).
            let mut value = self.slots[place.root.0 as usize].as_ref();
            let mut ownership = check.shadows[place.root.0 as usize].clone();
            for (depth, step) in self.indices(place)?.iter().enumerate() {
                if value.is_some_and(|v| {
                    crate::checked::inline_fields(v).is_none() && crate::checked::holds_storage(v)
                }) && (!ownership.is_unique()
                    || depth > 0
                        && row.modes.relation.reaches().iter().any(|r| {
                            matches!(r, hirsel_types::Reach::Place(o) if self.arena_of(o).is_some())
                        }))
                {
                    return Err(check
                        .state
                        .borrow()
                        .trap(
                            crate::checked::TrapRule::AllocationInNoalloc,
                            "projected mutation may detach shared or arena storage",
                        )
                        .into());
                }
                let index = match *step {
                    AccessStep::Field(i)
                    | AccessStep::VariantField { field: i, .. }
                    | AccessStep::Index(i) => i,
                };
                ownership = ownership.part(index);
                value = value
                    .and_then(Value::fields)
                    .and_then(|f| f.get(index))
                    .and_then(Option::as_ref);
            }
        }
        let mut node = check
            .shadows
            .get(place.root.0 as usize)
            .ok_or_else(|| invalid("local out of range"))?;
        for step in place.projection.iter() {
            let index = match step {
                Projection::Field(i) | Projection::VariantField { field: i, .. } => {
                    Some(*i as usize)
                }
                Projection::Index(_) => None,
            };
            match (node, index) {
                (Shadow::Parts(parts), Some(index)) => match parts.get(index) {
                    Some(part) => node = part,
                    None => break,
                },
                _ => {
                    check.state.borrow().access(node, "write")?;
                    break;
                }
            }
        }
        Ok(())
    }
    /// The shadow subtree of a place: fields of inline aggregates descend,
    /// an index or a block keeps the enclosing bound.
    pub fn shadow_of(&self, id: PlaceId) -> Result<crate::checked::Shadow, Error> {
        use crate::checked::Shadow;
        let Some(check) = &self.check else {
            return Ok(Shadow::default());
        };
        let place = self.place(id)?;
        let mut node = check
            .shadows
            .get(place.root.0 as usize)
            .ok_or_else(|| invalid("local out of range"))?
            .clone();
        for step in place.projection.iter() {
            let index = match step {
                Projection::Field(i) | Projection::VariantField { field: i, .. } => *i as usize,
                Projection::Index(_) => break,
            };
            node = node.part(index);
        }
        Ok(node)
    }
    /// Record a written value's bound: an `in a` destination row places it in
    /// a's arena (PROV-4); inline aggregates keep one bound per field.
    pub fn set_shadow(
        &mut self,
        id: PlaceId,
        mut shadow: crate::checked::Shadow,
    ) -> Result<(), Error> {
        use crate::checked::Shadow;
        if self.check.is_none() {
            return Ok(());
        }
        let place = self.place(id)?.clone();
        if let Some(arena) = self.placed_arena(place.ty) {
            let holds = self
                .observe(id)
                .map(crate::checked::holds_storage)
                .unwrap_or(false);
            if holds {
                shadow.add_all(arena);
            }
        }
        let indices = self.indices(&place)?;
        let Frame { check, slots, .. } = self;
        let check = check.as_mut().unwrap();
        let root = place.root.0 as usize;
        let mut value = slots.get(root).and_then(Option::as_ref);
        let mut node = check
            .shadows
            .get_mut(root)
            .ok_or_else(|| invalid("local out of range"))?;
        for step in indices {
            while let Shadow::Ownership { content, .. } = node {
                node = content;
            }
            let index = match step {
                AccessStep::Field(i)
                | AccessStep::VariantField { field: i, .. }
                | AccessStep::Index(i) => i,
            };
            let Some(fields) = value.and_then(Value::fields) else {
                node.add_bound(&shadow.total());
                return Ok(());
            };
            let bound = node.total();
            if value.and_then(crate::checked::inline_fields).is_none() {
                // Elements share a lifetime, but each counted child has its
                // own ownership witness for a later projected mutation.
                node.add_bound(&shadow.total());
                shadow.add_bound(&bound);
            }
            if !matches!(node, Shadow::Parts(parts) if parts.len() == fields.len()) {
                *node = Shadow::Parts(
                    fields
                        .iter()
                        .map(|field| {
                            field
                                .as_ref()
                                .map(|v| Shadow::of(bound.clone()).ownership(v, false))
                                .unwrap_or_default()
                        })
                        .collect(),
                );
            }
            let Shadow::Parts(parts) = node else {
                unreachable!()
            };
            node = parts
                .get_mut(index)
                .ok_or_else(|| invalid("write shadow field"))?;
            value = fields.get(index).and_then(Option::as_ref);
        }
        *node = shadow;
        Ok(())
    }
    /// The open arena named by a row placed `in a`, read from a's current slot.
    pub fn placed_arena(&self, ty: hirsel_types::TypeId) -> Option<u32> {
        let row = self.definition.types.get(ty)?;
        row.modes.relation.reaches().iter().find_map(|r| match r {
            hirsel_types::Reach::Place(origin) => self.arena_of(origin),
            _ => None,
        })
    }
    /// Whether a row's relation places its blocks in an open arena or a region.
    pub fn placed(&self, ty: hirsel_types::TypeId) -> bool {
        self.definition
            .types
            .get(ty)
            .is_some_and(|row| row.modes.relation.scoped(|o| self.arena_of(o).is_some()))
    }
    pub fn arena_of(&self, origin: &hirsel_types::Origin) -> Option<u32> {
        arena_of(self.body(), &self.slots, origin)
    }
    pub fn goto(&mut self, block: BlockId) {
        self.block = block;
        self.instruction = 0;
    }
}
impl Drop for Frame {
    fn drop(&mut self) {
        // Residual slots are borrowed snapshots or malformed-IR leftovers; TIR
        // cleanup already owns every semantic release. Dispose memory iteratively.
        for value in self.slots.iter_mut().filter_map(Option::take) {
            value.discard();
        }
        while let Some(work) = self.work.pop_front() {
            match work {
                crate::resources::Work::Release(value, _) => value.discard(),
                crate::resources::Work::Resume(result, _) => {
                    crate::resources::discard_result(result)
                }
                crate::resources::Work::Action(..) | crate::resources::Work::Chain(..) => (),
            }
        }
        if let Some(crate::resources::Ending::Return(value)) = self.ending.take() {
            value.discard();
        }
        if let Some((result, _)) = self.operation.take() {
            crate::resources::discard_result(result);
        }
    }
}
/// The open arena a relation place names, read from its binding's current slot.
pub(crate) fn arena_of(
    body: &hirsel_tir::Body,
    slots: &[Option<Value>],
    origin: &hirsel_types::Origin,
) -> Option<u32> {
    if !origin.fields.is_empty() {
        return None;
    }
    let root = match &origin.root {
        hirsel_types::OriginRoot::Parameter(hirsel_types::ParameterRef::Body {
            scope,
            parameter,
        }) if *scope == body.parameter_scope => body.parameters.get(parameter.0 as usize),
        _ => None,
    }
    .copied()
    .or_else(|| {
        body.places
            .iter()
            .find(|p| {
                p.projection.is_empty()
                    && p.binding
                        .as_ref()
                        .is_some_and(|b| b.root == origin.root && b.fields.is_empty())
            })
            .map(|p| p.root)
    })?;
    match slots.get(root.0 as usize)?.as_ref()? {
        Value::Handle { kind, id } if kind == crate::checked::ARENA_HANDLE => {
            u32::try_from(*id).ok()
        }
        _ => None,
    }
}

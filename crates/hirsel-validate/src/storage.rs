//! Independent whole-program storage proof from executable TIR. No source CFG
//! or producer behavior participates in reconstruction. Declared profiles are
//! checked only after the executable fixed point and read obligations settle.
use crate::{Reader, SignatureLookup, VRead, typing, validate_structure};
use hirsel_syntax::{ContentHash, DefId, DefPath, NameSite, PassingMode, UnaryOp};
use hirsel_tir as tir;
use hirsel_type_analysis::storage::{StorageDomain, program};
use hirsel_types::storage::graph::*;
use hirsel_types::storage::{
    CallableStorageProfile, StorageCallable, StorageInput, StorageProfile, StorageProfileError,
    StorageResult, StorageSelector,
};
use hirsel_types::*;
use std::{
    cell::RefCell,
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum StorageViolation {
    Malformed(&'static str),
    ExecutableRead(program::StorageReadViolation),
    MissingProfile(StorageCallable),
    UnexpectedProfile(StorageCallable),
    DuplicateProfile(StorageCallable),
    Profile {
        callable: StorageCallable,
        error: StorageProfileError,
    },
}
/// Actual observations are separate from every semantic profile hash. A body
/// change can keep the profile identical and still invalidate a prior verdict.
#[derive(Clone, Debug, Default)]
pub struct StorageReads {
    pub bodies: Vec<(DefId, ContentHash)>,
    pub profiles: Vec<(StorageCallable, Option<ContentHash>)>,
    pub constants: Vec<(DefId, Option<ContentHash>)>,
    pub derived: Vec<(StorageCallable, ContentHash)>,
    pub declarations: Vec<VRead>,
    pub selections: Vec<(DefId, Option<GeneratedActualKey>, Option<DefId>, bool)>,
}
#[derive(Clone, Debug, Default)]
pub struct StorageValidation {
    pub profiles: Vec<CallableStorageProfile>,
    pub violations: Vec<StorageViolation>,
    pub reads: StorageReads,
}

/// Ordinary V still validates each definition's types, modes and cleanup. This
/// additional mandatory import boundary checks the cross-definition placement
/// relation and every demanded profile, including known-empty claims.
pub fn validate_program(
    types: &TypeInterner,
    definitions: &[tir::DefinitionRef],
    signatures: &dyn SignatureLookup,
    constants: &BTreeMap<DefPath, tir::Materialized>,
    instances: &tir::InstanceBodies,
    declared: &[CallableStorageProfile],
) -> StorageValidation {
    let mut validation = StorageValidation::default();
    let reader = Reader::new(types, signatures);
    reconstruct(
        types,
        definitions,
        &reader,
        constants,
        instances,
        &mut validation,
    );
    reader.finish(&mut validation.reads.declarations);
    let mut claims = BTreeMap::new();
    for claim in declared {
        if claims.insert(claim.callable, &claim.profile).is_some() {
            validation
                .violations
                .push(StorageViolation::DuplicateProfile(claim.callable));
        }
    }
    for actual in &validation.profiles {
        let claimed = claims.remove(&actual.callable);
        validation.reads.profiles.push((
            actual.callable,
            claimed.map(|claim| {
                ContentHash::of_parts(
                    "hirsel.validate.storage-claim.v1",
                    [format!("{claim:?}").as_bytes()],
                )
            }),
        ));
        match claimed {
            None => validation
                .violations
                .push(StorageViolation::MissingProfile(actual.callable)),
            Some(claim) => {
                if let Err(error) = claim.check_against(&actual.profile) {
                    validation.violations.push(StorageViolation::Profile {
                        callable: actual.callable,
                        error,
                    });
                }
            }
        }
    }
    validation
        .violations
        .extend(claims.into_keys().map(StorageViolation::UnexpectedProfile));
    validation
}

fn reconstruct(
    types: &TypeInterner,
    definitions: &[tir::DefinitionRef],
    reader: &Reader<'_>,
    constants: &BTreeMap<DefPath, tir::Materialized>,
    instances: &tir::InstanceBodies,
    validation: &mut StorageValidation,
) {
    // Detect the obligation in actual executable calls, not a source flag or
    // the presence/absence of a producer profile table.
    if !definitions.iter().any(|definition| definition.bodies.iter().any(|body| body.blocks.iter().any(|block| matches!(&block.terminator, tir::Terminator::Call { callee: tir::Callee::Definition { path, .. }, .. } if path.is_core("region.with"))))) {
        return;
    }
    let observed: BTreeSet<_> = definitions
        .iter()
        .flat_map(|definition| definition.bodies.iter())
        .flat_map(|body| &body.rvalues)
        .filter_map(|value| match value {
            tir::Rvalue::ReadConstant { definition, .. } => Some(DefId::intern(definition)),
            _ => None,
        })
        .collect();
    validation.reads.constants = observed
        .into_iter()
        .map(|definition| {
            let value = constants.get(definition.path()).map(|value| {
                ContentHash::of_parts(
                    "hirsel.validate.storage-constant.v1",
                    [format!("{value:?}").as_bytes()],
                )
            });
            (definition, value)
        })
        .collect();
    let mut products = Vec::new();
    let selections = RefCell::new(Vec::new());
    for definition in definitions {
        validation.reads.bodies.push((
            definition.path,
            ContentHash::of_parts(
                "hirsel.validate.storage-body.v1",
                [format!("{definition:?}").as_bytes()],
            ),
        ));
        if !validate_structure(definition, types).is_empty() {
            validation
                .violations
                .push(StorageViolation::Malformed("TIR structure"));
            return;
        }
        let Some(declarations) =
            typing::declaration_tables(types, &crate::ReadDefinition::live(definition), reader)
        else {
            validation
                .violations
                .push(StorageViolation::Malformed("callable declaration binders"));
            return;
        };
        let mut callable_types = BTreeMap::new();
        if let Some(signature) = reader.get(definition.path) {
            if let SignatureKind::Function(function) = &signature.kind {
                callable_types.insert(
                    definition.entry.0,
                    types.intern(Type::with_kind(TypeKind::Function(function.clone()))),
                );
            }
        }
        for body in &definition.bodies {
            for block in &body.blocks {
                for instruction in &block.instructions {
                    if let tir::InstructionKind::Assign { destination, value } = instruction.kind {
                        if let tir::Rvalue::Closure { body: nested, .. } =
                            &body.rvalues[value.0 as usize]
                        {
                            let ty = body.places[destination.0 as usize].ty;
                            if callable_types
                                .insert(nested.0, ty)
                                .is_some_and(|old| !types.semantic_eq(old, ty))
                            {
                                validation.violations.push(StorageViolation::Malformed(
                                    "conflicting callable type producer",
                                ));
                                return;
                            }
                        }
                    }
                }
            }
        }
        for (index, body) in definition.bodies.iter().enumerate() {
            let producer = Producer {
                types,
                definition,
                body,
                declarations: &declarations[index],
                signatures: reader,
                constants,
            };
            match producer.derive(callable_types.get(&(index as u32)).copied()) {
                Ok(product) => products.push(product),
                Err(error) => {
                    validation
                        .violations
                        .push(StorageViolation::Malformed(error));
                    return;
                }
            }
        }
    }
    let domains: Vec<_> = products.iter().map(StorageDomain::for_body).collect();
    let select = |declaration, arguments: &[GenericArgument]| {
        let selected = instances.resolve(declaration, types, arguments);
        selections.borrow_mut().push((
            declaration,
            tir::InstanceBodies::key(types, arguments),
            selected.as_ref().ok().copied().flatten(),
            selected.is_ok(),
        ));
        match selected {
            Ok(Some(definition)) => Some(definition),
            Ok(None) => Some(declaration),
            Err(_) => None,
        }
    };
    let proved = program::analyze(types, &select, &products, &domains);
    validation.reads.selections = selections.into_inner();
    let proof = match proved {
        Ok(proof) => proof,
        Err(_) => {
            validation
                .violations
                .push(StorageViolation::Malformed("executable storage equations"));
            return;
        }
    };
    validation.violations.extend(
        proof
            .violations
            .into_iter()
            .map(StorageViolation::ExecutableRead),
    );
    for (body, behavior) in products.into_iter().zip(proof.behaviors) {
        if !proof.demanded.contains(&body.callable) {
            continue;
        }
        let Some(source_type) = body.source_type else {
            validation.violations.push(StorageViolation::Malformed(
                "missing canonical callable source type",
            ));
            continue;
        };
        match StorageProfile::new(source_type, behavior) {
            Ok(profile) => {
                validation
                    .reads
                    .derived
                    .push((body.callable, profile.digest));
                validation.profiles.push(CallableStorageProfile {
                    callable: body.callable,
                    profile,
                });
            }
            Err(error) => validation.violations.push(StorageViolation::Profile {
                callable: body.callable,
                error,
            }),
        }
    }
    validation.profiles.sort_by_key(|profile| profile.callable);
}

struct Producer<'a> {
    types: &'a TypeInterner,
    definition: &'a tir::Definition,
    body: &'a tir::Body,
    declarations: &'a GenericDeclarations,
    signatures: &'a Reader<'a>,
    constants: &'a BTreeMap<DefPath, tir::Materialized>,
}
impl Producer<'_> {
    fn callable(&self, body: &tir::Body) -> StorageCallable {
        StorageCallable {
            definition: self.definition.path,
            scope: body.parameter_scope,
        }
    }
    fn detaches(&self, ty: TypeId) -> bool {
        copy_detaches(self.types, ty, self.declarations, &mut |definition| {
            Ok(self.signatures.get(definition).cloned().map(Arc::new))
        })
        .unwrap_or(false)
    }
    fn place(&self, place: tir::PlaceId) -> StoragePlace {
        let place = &self.body.places[place.0 as usize];
        StoragePlace {
            local: place.root.0,
            parts: place
                .projection
                .iter()
                .map(|projection| match projection {
                    tir::Projection::Field(field) => OwnedTypePart::Field(*field),
                    tir::Projection::VariantField { variant, field } => {
                        OwnedTypePart::VariantField {
                            variant: *variant,
                            field: *field,
                        }
                    }
                    tir::Projection::Index(_) => OwnedTypePart::Elements,
                })
                .collect(),
        }
    }
    fn operand(&self, operand: &tir::Operand) -> StorageValue {
        StorageValue::Copy {
            value: self.place(operand.place),
            detaches: self.detaches(self.body.places[operand.place.0 as usize].ty),
        }
    }
    fn placement(&self, destination: &Destination) -> StoragePlacement {
        match destination {
            Destination::Heap | Destination::Inferred => StoragePlacement::Independent,
            Destination::Arena(origin) => self.body.places.iter().enumerate()
                .find(|(_, place)| place.binding.as_ref() == Some(origin))
                .map(|(index, _)| StoragePlacement::Sources(vec![self.place(tir::PlaceId(index as u32))]))
                .unwrap_or(StoragePlacement::Unknown),
            Destination::Caller(parameters) => parameters.iter().map(|parameter| {
                if matches!(parameter, ParameterRef::Bound { depth: 0, .. })
                    || matches!(parameter, ParameterRef::Body { scope, .. } if *scope == self.body.parameter_scope)
                {
                    self.body.parameters.get(parameter.parameter().0 as usize)
                        .map(|local| StoragePlace::root(local.0))
                } else {
                    None
                }
            }).collect::<Option<Vec<_>>>()
                .map(StoragePlacement::Sources)
                .unwrap_or(StoragePlacement::Unknown),
            _ => StoragePlacement::Unknown,
        }
    }
    // Executable constant graphs contain original code references. Their
    // declared field types alone cannot identify any code or environment.
    fn constant(&self, original: &tir::Materialized) -> StorageValue {
        let mut code = Vec::new();
        let mut complete = true;
        let mut pending = vec![(&original.root, hirsel_types::SharedPath::new())];
        while let Some((node, path)) = pending.pop() {
            match &node.kind {
                tir::ValueKind::Constant(_) => (),
                tir::ValueKind::FunctionReference {
                    definition,
                    arguments,
                } => {
                    complete &= generated_actual_key(self.types, arguments).is_some();
                    let id = DefId::intern(definition);
                    let primitive = if arguments.is_empty() {
                        self.integer_order(id)
                    } else {
                        None
                    };
                    code.push(StorageConstantCode {
                        parts: path,
                        definition: id,
                        arguments: arguments.clone(),
                        primitive,
                    });
                }
                tir::ValueKind::Construct {
                    constructor,
                    fields,
                } => {
                    for (ordinal, field) in fields.iter().enumerate().rev() {
                        let part = match constructor {
                            tir::Constructor::Variant { variant, .. } => {
                                OwnedTypePart::VariantField {
                                    variant: *variant,
                                    field: ordinal as u32,
                                }
                            }
                            tir::Constructor::Sequence
                            | tir::Constructor::Map
                            | tir::Constructor::Set => OwnedTypePart::Elements,
                            tir::Constructor::SortedMap { .. } => {
                                complete = false;
                                continue;
                            }
                            _ => OwnedTypePart::Field(ordinal as u32),
                        };
                        let mut child = path.clone();
                        child.push(part);
                        pending.push((field, child));
                    }
                }
            }
        }
        StorageValue::Constant { code, complete }
    }
    fn integer_order(&self, definition: DefId) -> Option<StoragePrimitive> {
        let kind = StoragePrimitive::integer_order_type(definition)?;
        let signature = self.signatures.get(definition)?;
        let SignatureKind::Function(function) = &signature.kind else {
            return None;
        };
        if signature.metadata.visibility != hirsel_syntax::Visibility::Private
            || signature.metadata.alias
            || signature.metadata.unsafe_ffi
            || signature.metadata.wrapper.is_some()
            || !function.generics.is_empty()
            || function.parameters.len() != 2
            || function.parameters.iter().any(|parameter| {
                parameter.passing != PassingMode::Borrow
                    || self.types.row(parameter.ty).kind != TypeKind::Primitive(kind)
            })
            || self.types.row(function.result).kind != TypeKind::Primitive(Primitive::I32)
            || function.error.is_some()
            || function.allocation != Allocation::NoAlloc
            || function.authority != AuthoritySummary::inert()
            || function.uses != Uses::Only(AuthoritySummary::inert())
            || !function.contracts.is_empty()
            || !function.state_promises.is_empty()
            || function.incomplete
            || function.foreign.is_some()
        {
            return None;
        }
        Some(StoragePrimitive::IntegerOrder)
    }
    fn rvalue(&self, value: &tir::Rvalue) -> StorageValue {
        match value {
            tir::Rvalue::Constant(_) | tir::Rvalue::Size(_) | tir::Rvalue::Shape(_) => {
                StorageValue::Fresh
            }
            tir::Rvalue::ReadConstant {
                definition,
                arguments,
            } => {
                if !arguments.is_empty() {
                    return StorageValue::Unknown;
                }
                self.constants
                    .get(definition)
                    .map_or(StorageValue::Unknown, |value| self.constant(value))
            }
            tir::Rvalue::FunctionReference {
                definition,
                arguments,
            } => StorageValue::Reference {
                definition: DefId::intern(definition),
                arguments: arguments.clone(),
            },
            tir::Rvalue::Use(operand) => self.operand(operand),
            tir::Rvalue::Unary {
                op: UnaryOp::Unique,
                value,
            } => StorageValue::Unique {
                value: self.place(value.place),
                allocates: !self.detaches(self.body.places[value.place.0 as usize].ty),
            },
            tir::Rvalue::Unary { value, .. } => self.operand(value),
            tir::Rvalue::Binary { .. } => StorageValue::Fresh,
            tir::Rvalue::Construct {
                constructor,
                fields,
                destination,
            } => {
                let placement = self.placement(destination);
                let fields = fields
                    .iter()
                    .map(|field| {
                        let part = match constructor {
                            tir::Constructor::Variant { variant, .. } => {
                                OwnedTypePart::VariantField {
                                    variant: *variant,
                                    field: field.field,
                                }
                            }
                            tir::Constructor::Sequence
                            | tir::Constructor::Map
                            | tir::Constructor::Set => OwnedTypePart::Elements,
                            _ => OwnedTypePart::Field(field.field),
                        };
                        (vec![part].into(), self.place(field.value.place))
                    })
                    .collect();
                StorageValue::Construct { fields, placement }
            }
            tir::Rvalue::Closure { body, captures, .. } => StorageValue::Callable {
                target: self.callable(&self.definition.bodies[body.0 as usize]),
                captures: captures
                    .iter()
                    .map(|capture| self.place(capture.place))
                    .collect(),
            },
        }
    }
    fn map_arguments(&self, ty: TypeId, ordered: bool) -> Option<Vec<TypeId>> {
        let mut ty = ty;
        if let TypeKind::Named {
            definition,
            arguments,
        } = &self.types.row(ty).kind
        {
            let signature = self.signatures.get(*definition)?;
            let SignatureKind::Type(declaration) = &signature.kind else {
                return None;
            };
            ty = substitute(
                self.types,
                &TypeMemo::new(),
                declaration.representation?,
                &Substitution {
                    binder: Some(GenericBinder::Bound(0)),
                    arguments,
                    authorities: &vec![AuthoritySummary::Unknown; arguments.len()],
                    roles: &signature.metadata.roles,
                },
            )
            .ok()?;
        }
        match &self.types.row(ty).kind {
            TypeKind::Builtin { kind, arguments }
                if *kind
                    == if ordered {
                        BuiltinType::SortedMap
                    } else {
                        BuiltinType::Map
                    }
                    && arguments.len() == 2 =>
            {
                arguments
                    .iter()
                    .map(|argument| match argument {
                        GenericArgument::Type(ty) => Some(*ty),
                        _ => None,
                    })
                    .collect()
            }
            _ => None,
        }
    }
    fn map_callee(
        &self,
        path: &hirsel_syntax::DefPath,
        function: &FunctionType,
        result: TypeId,
    ) -> Option<StorageCallee> {
        if function.generics.len() != 2
            || function.incomplete
            || function.foreign.is_some()
            || function.error.is_some()
        {
            return None;
        }
        let ordered = path.is_core("sorted_map.insert")
            || path.is_core("sorted_map.get")
            || path.is_core("sorted_map.remove");
        let offset = usize::from(ordered);
        let Some(first) = function.parameters.get(offset) else {
            return None;
        };
        let Some(arguments) = self.map_arguments(first.ty, ordered) else {
            return None;
        };
        if !matches!(&function.generics[0], GenericParameter::Type { bounds, .. } if ordered || bounds.contains(&Bound::Key))
            || !matches!(&function.generics[1], GenericParameter::Type { .. })
        {
            return None;
        }
        if ordered {
            let by = &function.parameters[0];
            let TypeKind::Named {
                definition,
                arguments: order_arguments,
            } = &self.types.row(by.ty).kind
            else {
                return None;
            };
            if by.passing != PassingMode::Borrow
                || !definition.path().is_core("Order")
                || !matches!(order_arguments.as_slice(), [GenericArgument::Type(key)] if self.types.semantic_eq(*key, arguments[0]))
            {
                return None;
            }
            let order = self.signatures.get(*definition)?;
            let SignatureKind::Type(declaration) = &order.kind else {
                return None;
            };
            let Some(representation) = declaration.representation else {
                return None;
            };
            let Ok(representation) = substitute(
                self.types,
                &TypeMemo::new(),
                representation,
                &Substitution {
                    binder: Some(GenericBinder::Bound(0)),
                    arguments: order_arguments,
                    authorities: &vec![AuthoritySummary::Unknown; order_arguments.len()],
                    roles: &order.metadata.roles,
                },
            ) else {
                return None;
            };
            let Ok(key) = shift_type_binders(self.types, arguments[0], 1) else {
                return None;
            };
            let TypeKind::Record(fields) = &self.types.row(representation).kind else {
                return None;
            };
            if fields.len() != 1 || fields[0].name.as_str() != "compare" {
                return None;
            }
            let TypeKind::Function(compare) = &self.types.row(fields[0].ty).kind else {
                return None;
            };
            if !compare.generics.is_empty()
                || compare.parameters.len() != 2
                || compare
                    .parameters
                    .iter()
                    .any(|parameter| parameter.passing != PassingMode::Borrow)
                || compare
                    .parameters
                    .iter()
                    .any(|parameter| self.types.row(parameter.ty).kind != self.types.row(key).kind)
                || self.types.row(compare.result).kind != TypeKind::Primitive(Primitive::I32)
                || compare.incomplete
                || compare.foreign.is_some()
                || compare.error.is_some()
            {
                return None;
            }
        }
        if path.is_core("map.insert") || path.is_core("sorted_map.insert") {
            let reviewed = function.parameters.len() == 3 + offset
                && first.passing == PassingMode::Mut
                && function.parameters[1 + offset].passing == PassingMode::Take
                && function.parameters[2 + offset].passing == PassingMode::Take
                && self
                    .types
                    .semantic_eq(function.parameters[1 + offset].ty, arguments[0])
                && self
                    .types
                    .semantic_eq(function.parameters[2 + offset].ty, arguments[1])
                && matches!(&self.types.row(function.result).kind, TypeKind::Tuple(fields) if fields.is_empty())
                && function.allocation == Allocation::MayAllocate;
            return reviewed.then_some(if ordered {
                StorageCallee::SortedMapInsert
            } else {
                StorageCallee::MapInsert
            });
        }
        let removing = path.is_core("map.remove") || path.is_core("sorted_map.remove");
        if function.parameters.len() != 2 + offset
            || first.passing
                != if removing {
                    PassingMode::Mut
                } else {
                    PassingMode::Borrow
                }
            || removing && self.types.row(first.ty).modes.uniqueness != Uniqueness::Unique
            || function.parameters[1 + offset].passing != PassingMode::Borrow
            || self.types.row(function.parameters[1 + offset].ty).kind
                != self.types.row(arguments[0]).kind
            || function.allocation
                != if removing || ordered {
                    Allocation::MayAllocate
                } else {
                    Allocation::NoAlloc
                }
        {
            return None;
        }
        let TypeKind::Named {
            definition,
            arguments: option_arguments,
        } = &self.types.row(function.result).kind
        else {
            return None;
        };
        if !definition.path().is_core("Option")
            || option_arguments.as_slice() != [GenericArgument::Type(arguments[1])]
        {
            return None;
        }
        let option = self.signatures.get(*definition)?;
        let SignatureKind::Type(declaration) = &option.kind else {
            return None;
        };
        let Some(representation) = declaration.representation else {
            return None;
        };
        let TypeKind::Variant(variants) = &self.types.row(representation).kind else {
            return None;
        };
        if variants.len() != 2
            || variants
                .iter()
                .filter(|variant| variant.name.as_str() == "None" && variant.fields.is_empty())
                .count()
                != 1
        {
            return None;
        }
        let Some(variant) = variants
            .iter()
            .position(|variant| variant.name.as_str() == "Some" && variant.fields.len() == 1)
        else {
            return None;
        };
        let result_modes = &self.types.row(function.result).modes;
        let reaches = result_modes.relation.reaches();
        if removing {
            if result_modes.uniqueness != Uniqueness::Unique || !result_modes.relation.is_global() {
                return None;
            }
        } else if reaches.len() != 1
            || !matches!(&reaches[0], Reach::Place(Origin { root: OriginRoot::Parameter(ParameterRef::Bound { depth: 0, parameter }), fields }) if *parameter == ParamId(offset as u32) && fields.is_empty())
        {
            return None;
        }
        let result_path = vec![OwnedTypePart::VariantField {
            variant: variant as u32,
            field: 0,
        }]
        .into();
        let requires_code = match &self.types.row(result).kind {
            TypeKind::Named { arguments, .. } => arguments.iter().any(|argument| matches!(argument, GenericArgument::Type(ty) if matches!(self.types.row(*ty).kind, TypeKind::Function(_)))),
            _ => false,
        };
        Some(if ordered {
            if removing {
                StorageCallee::SortedMapRemove {
                    result_path,
                    requires_code,
                }
            } else {
                StorageCallee::SortedMapGet {
                    result_path,
                    requires_code,
                }
            }
        } else if removing {
            StorageCallee::MapRemove {
                result_path,
                requires_code,
            }
        } else {
            StorageCallee::MapGet {
                result_path,
                requires_code,
            }
        })
    }
    fn declared(
        &self,
        path: &hirsel_syntax::DefPath,
        arguments: &[GenericArgument],
        result: Option<TypeId>,
    ) -> StorageCallee {
        if path.is_core("region.with") {
            return StorageCallee::ReadRegion;
        }
        if path.is_core("arena.seal") {
            return StorageCallee::SealArena;
        }
        if path.is_core("arena.with") {
            return StorageCallee::ScopeCallback {
                callback: ParamId(0),
                result_path: Default::default(),
            };
        }
        if path.0.package.name != "core" {
            return StorageCallee::Apply {
                definition: DefId::intern(path),
                arguments: arguments.to_vec(),
            };
        }
        let Some(signature) = self.signatures.get(DefId::intern(path)) else {
            return unknown_callee();
        };
        let SignatureKind::Function(function) = &signature.kind else {
            return unknown_callee();
        };
        // Independently check the executable operation's original declaration.
        // Only the real callback body contributes effects and result producers.
        if path.is_core("task.scope") {
            let reviewed = signature.definition.path() == path
                && match &signature.kind {
                    SignatureKind::Function(function) => {
                        function.generics.len() == 1
                            && matches!(&function.generics[0], GenericParameter::Type { bounds, .. } if bounds.is_empty())
                            && function.parameters.len() == 2
                            && !function.incomplete
                            && function.foreign.is_none()
                            && function.error.is_none()
                            && function.allocation == Allocation::MayAllocate
                            && function.parameters[0].passing == PassingMode::Borrow
                            && matches!(&self.types.row(function.parameters[0].ty).kind, TypeKind::Named { definition, arguments } if definition.path().is_core("Tasks") && arguments.is_empty())
                            && function.parameters[1].passing == PassingMode::Borrow
                            && self.types.row(function.parameters[1].ty).modes.multiplicity
                                == Multiplicity::Once
                            && matches!(
                                &self.types.row(function.result).kind,
                                TypeKind::Parameter(TypeParameterRef::Bound {
                                    depth: 0,
                                    parameter: TypeParamId(0)
                                })
                            )
                            && matches!(&self.types.row(function.parameters[1].ty).kind,
                                    TypeKind::Function(callback) if callback.generics.is_empty()
                                        && callback.parameters.len() == 1
                                        && callback.parameters[0].passing == PassingMode::Borrow
                                        && matches!(&self.types.row(callback.parameters[0].ty).kind, TypeKind::Named { definition, arguments } if definition.path().is_core("Scope") && arguments.is_empty())
                                        && matches!(&self.types.row(callback.result).kind, TypeKind::Parameter(TypeParameterRef::Bound { depth: 1, parameter: TypeParamId(0) })))
                    }
                    _ => false,
                };
            return if reviewed {
                StorageCallee::ScopeCallback {
                    callback: ParamId(1),
                    result_path: Default::default(),
                }
            } else {
                unknown_callee()
            };
        }
        if path.is_core("task.async") || path.is_core("task.await") {
            let reviewed = if path.is_core("task.async") {
                signature.definition.path() == path
                    && function.generics.len() == 1
                    && function.parameters.len() == 2
                    && !function.incomplete
                    && function.foreign.is_none()
                    && function.error.is_none()
                    && function.allocation == Allocation::MayAllocate
                    && function.parameters[0].passing == PassingMode::Borrow
                    && matches!(&self.types.row(function.parameters[0].ty).kind, TypeKind::Named {definition,arguments} if definition.path().is_core("Scope") && arguments.is_empty())
                    && function.parameters[1].passing == PassingMode::Take
                    && matches!(&function.generics[0],GenericParameter::Type {bounds,..} if bounds.as_slice() == [Bound::Sendable])
                    && self.types.row(function.parameters[1].ty).modes.multiplicity
                        == Multiplicity::Once
                    && self
                        .types
                        .row(function.parameters[1].ty)
                        .modes
                        .relation
                        .is_global()
                    && matches!(&self.types.row(function.parameters[1].ty).kind,TypeKind::Function(callback) if callback.generics.is_empty() && callback.parameters.is_empty()
                && matches!(self.types.row(callback.result).kind,TypeKind::Parameter(TypeParameterRef::Bound {depth:1,parameter:TypeParamId(0)})))
                    && matches!(&self.types.row(function.result).kind, TypeKind::Builtin {kind:BuiltinType::Task,arguments} if matches!(arguments.as_slice(),[GenericArgument::Type(result)] if matches!(self.types.row(*result).kind,TypeKind::Parameter(TypeParameterRef::Bound {depth:0,parameter:TypeParamId(0)}))))
                    && matches!(self.types.row(function.result).brand.alternatives.as_slice(),[Origin {root:OriginRoot::Parameter(ParameterRef::Bound {depth:0,parameter:ParamId(0)}),fields}] if fields.is_empty())
            } else {
                signature.definition.path() == path
                    && function.generics.len() == 1
                    && function.parameters.len() == 2
                    && !function.incomplete
                    && function.foreign.is_none()
                    && function.error.is_none()
                    && function.allocation == Allocation::MayAllocate
                    && function.parameters[0].passing == PassingMode::Borrow
                    && matches!(&self.types.row(function.parameters[0].ty).kind, TypeKind::Named {definition,arguments} if definition.path().is_core("Scope") && arguments.is_empty())
                    && function.parameters[1].passing == PassingMode::Take
                    && matches!(&function.generics[0],GenericParameter::Type {bounds,..} if bounds.is_empty())
                    && matches!(
                        self.types.row(function.result).kind,
                        TypeKind::Parameter(TypeParameterRef::Bound {
                            depth: 0,
                            parameter: TypeParamId(0)
                        })
                    )
                    && matches!(&self.types.row(function.parameters[1].ty).kind,TypeKind::Builtin {kind:BuiltinType::Task,arguments} if arguments.as_slice() == [GenericArgument::Type(function.result)])
                    && matches!(self.types.row(function.parameters[1].ty).brand.alternatives.as_slice(),[Origin {root:OriginRoot::Parameter(ParameterRef::Bound {depth:0,parameter:ParamId(0)}),fields}] if fields.is_empty())
            };
            return if !reviewed {
                unknown_callee()
            } else if path.is_core("task.async") {
                StorageCallee::ScopeCallback {
                    callback: ParamId(1),
                    result_path: vec![OwnedTypePart::Elements].into(),
                }
            } else {
                StorageCallee::TaskAwait
            };
        }
        if path.is_core("copy_to") {
            let reviewed = signature.definition.path() == path
                && function.parameters.len() == 2 && function.generics.len() == 1
                && function.parameters.iter().all(|parameter| parameter.passing == PassingMode::Borrow)
                && self.signatures.first_heap().is_some_and(|heap| is_arena_type(self.types, function.parameters[0].ty, &heap.arena))
                && matches!(&function.generics[0], GenericParameter::Type { bounds, .. } if bounds.contains(&Bound::Copyable))
                && matches!((&self.types.row(function.parameters[1].ty).kind, &self.types.row(function.result).kind), (TypeKind::Parameter(left), TypeKind::Parameter(right)) if left == right)
                && self.types.row(function.result).modes.relation.reaches().iter().any(|reach| matches!(reach,
                    Reach::Place(Origin { root: OriginRoot::Parameter(ParameterRef::Bound { depth: 0, parameter: ParamId(0) }), fields }) if fields.is_empty()));
            return if reviewed {
                StorageCallee::CopyTo {
                    holds_storage: result.is_none_or(|ty| !self.detaches(ty)),
                    requires_code: result
                        .is_some_and(|ty| matches!(self.types.row(ty).kind, TypeKind::Function(_))),
                }
            } else {
                unknown_callee()
            };
        }
        if path.is_core("shared.with") {
            let reviewed = signature.definition.path() == path
                && function.parameters.len() == 2
                && function.parameters[0].passing == PassingMode::Borrow
                && matches!(
                    self.types.row(function.parameters[0].ty).kind,
                    TypeKind::Builtin {
                        kind: BuiltinType::Shared,
                        ..
                    }
                )
                && function.parameters[1].passing == PassingMode::Borrow
                && matches!(&self.types.row(function.parameters[1].ty).kind, TypeKind::Function(callback)
                    if callback.parameters.len() == 1 && callback.parameters[0].passing == PassingMode::Borrow);
            return if reviewed {
                StorageCallee::ScopeCallback {
                    callback: ParamId(1),
                    result_path: Default::default(),
                }
            } else {
                unknown_callee()
            };
        }
        if path.is_core("arena.bounded") {
            let TypeKind::Named { definition, .. } = self.types.row(function.result).kind else {
                return unknown_callee();
            };
            let Some(result_signature) = self.signatures.get(definition) else {
                return unknown_callee();
            };
            let SignatureKind::Type(declaration) = &result_signature.kind else {
                return unknown_callee();
            };
            let Some(representation) = declaration.representation else {
                return unknown_callee();
            };
            let TypeKind::Variant(variants) = &self.types.row(representation).kind else {
                return unknown_callee();
            };
            let Some(variant) = variants
                .iter()
                .position(|variant| variant.name.as_str() == "Ok" && variant.fields.len() == 1)
            else {
                return unknown_callee();
            };
            return StorageCallee::ScopeCallback {
                callback: ParamId(1),
                result_path: vec![OwnedTypePart::VariantField {
                    variant: variant as u32,
                    field: 0,
                }]
                .into(),
            };
        }
        if path.is_core("map.insert")
            || path.is_core("map.get")
            || path.is_core("map.remove")
            || path.is_core("sorted_map.insert")
            || path.is_core("sorted_map.get")
            || path.is_core("sorted_map.remove")
        {
            return result
                .and_then(|result| self.map_callee(path, function, result))
                .unwrap_or_else(unknown_callee);
        }
        let mut results = Vec::new();
        let mut complete =
            !function.parameters.iter().any(|parameter| {
                matches!(self.types.row(parameter.ty).kind, TypeKind::Function(_))
            }) && ![
                "iter.next",
                "sorted_map.insert",
                "sorted_map.get",
                "sorted_map.remove",
                "runner.compute",
                "runner.call",
                "runner.spawn",
            ]
            .iter()
            .any(|name| path.is_core(name));
        if !self.detaches(function.result) && !path.is_core("arena.seal") {
            for reach in self.types.row(function.result).modes.relation.reaches() {
                match reach {
                    Reach::Place(Origin {
                        root:
                            OriginRoot::Parameter(parameter @ ParameterRef::Bound { depth: 0, .. }),
                        fields,
                    }) => results.push(StorageResult {
                        parts: Default::default(),
                        source: StorageSelector {
                            input: StorageInput::Parameter(*parameter),
                            parts: fields
                                .iter()
                                .map(|field| OwnedTypePart::Field(field.0))
                                .collect(),
                        },
                    }),
                    _ => complete = false,
                }
            }
        }
        let mut allocations = BTreeSet::new();
        if function.allocation == Allocation::MayAllocate {
            allocations.extend(results.iter().map(|result| result.source.clone()));
            for (ordinal, parameter) in function.parameters.iter().enumerate() {
                if parameter.passing == PassingMode::Mut {
                    allocations.insert(StorageSelector {
                        input: StorageInput::Parameter(ParameterRef::bound(ParamId(
                            ordinal as u32,
                        ))),
                        parts: Default::default(),
                    });
                }
            }
        }
        StorageCallee::Declared {
            allocations: allocations.into_iter().collect(),
            results,
            complete,
        }
    }
    fn mutations(
        &self,
        callee: &tir::Callee,
        arguments: &[tir::CallArgument],
    ) -> Result<Vec<ParamId>, &'static str> {
        if arguments.iter().any(|argument| {
            argument.value.usage == UseKind::Mut
                && self.body.locals
                    [self.body.places[argument.value.place.0 as usize].root.0 as usize]
                    .storage
                    == tir::Storage::Capture
        }) {
            return Err("storage mutation of captured binding");
        }
        let function = match callee {
            tir::Callee::Definition { path, .. } => match self
                .signatures
                .get(DefId::intern(path))
                .map(|signature| &signature.kind)
            {
                Some(SignatureKind::Function(function)) => function,
                _ => return Err("storage mutable callee declaration"),
            },
            tir::Callee::Indirect { value, .. } => match &self
                .types
                .row(self.body.places[value.place.0 as usize].ty)
                .kind
            {
                TypeKind::Function(function) => function,
                _ => return Err("storage mutable indirect declaration"),
            },
        };
        let declared: BTreeSet<_> = function
            .parameters
            .iter()
            .enumerate()
            .filter(|(_, parameter)| parameter.passing == PassingMode::Mut)
            .map(|(ordinal, _)| ParamId(ordinal as u32))
            .collect();
        let actual: BTreeSet<_> = arguments
            .iter()
            .filter(|argument| argument.value.usage == UseKind::Mut)
            .map(|argument| argument.parameter)
            .collect();
        if declared != actual
            || declared.iter().any(|parameter| {
                arguments
                    .iter()
                    .filter(|argument| argument.parameter == *parameter)
                    .count()
                    != 1
            })
        {
            return Err("storage mutable operand modes");
        }
        Ok(declared.into_iter().collect())
    }
    fn derive(&self, callable_type: Option<TypeId>) -> Result<StorageBody, &'static str> {
        let mut structures = std::collections::HashMap::new();
        for ty in self
            .body
            .locals
            .iter()
            .map(|local| local.ty)
            .chain(self.body.places.iter().map(|place| place.ty))
            .collect::<Vec<_>>()
        {
            if structures.contains_key(&ty) {
                continue;
            }
            let structure =
                hirsel_types::storage::storage_type_paths(self.types, ty, &mut |definition| {
                    Ok(self.signatures.get(definition).cloned().map(Arc::new))
                })
                .map_err(|_| "executable storage path domain")?;
            structures.insert(ty, structure);
        }
        let paths = structures
            .values()
            .flat_map(|structure| structure.paths.iter().cloned())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        let mut inputs = Vec::new();
        for (ordinal, local) in self.body.parameters.iter().enumerate() {
            let ty = self.body.locals[local.0 as usize].ty;
            inputs.push(StorageInputBinding {
                input: StorageInput::Parameter(ParameterRef::bound(ParamId(ordinal as u32))),
                place: StoragePlace::root(local.0),
                mutable: callable_type
                    .and_then(|ty| match &self.types.row(ty).kind {
                        TypeKind::Function(function) => function
                            .parameters
                            .get(ordinal)
                            .map(|parameter| parameter.passing == PassingMode::Mut),
                        _ => None,
                    })
                    .unwrap_or(true),
                holds_storage: !self.detaches(ty),
                callables: structures
                    .get(&ty)
                    .map(|structure| structure.callables.iter().cloned().collect())
                    .unwrap_or_default(),
            });
        }
        for (ordinal, (index, local)) in self
            .body
            .locals
            .iter()
            .enumerate()
            .filter(|(_, local)| local.storage == tir::Storage::Capture)
            .enumerate()
        {
            inputs.push(StorageInputBinding {
                input: StorageInput::Capture(ordinal as u32),
                mutable: false,
                place: StoragePlace::root(index as u32),
                holds_storage: !self.detaches(local.ty),
                callables: structures
                    .get(&local.ty)
                    .map(|structure| structure.callables.iter().cloned().collect())
                    .unwrap_or_default(),
            });
        }
        let mut starts = Vec::new();
        let mut offset = 0;
        for block in &self.body.blocks {
            starts.push(offset);
            offset += block.instructions.len() + 1;
        }
        let mut nodes = Vec::with_capacity(offset);
        for block in &self.body.blocks {
            for instruction in &block.instructions {
                let operation = match &instruction.kind {
                    tir::InstructionKind::Assign { destination, value } => {
                        StorageOp::Assign(vec![(
                            self.place(*destination),
                            self.rvalue(&self.body.rvalues[value.0 as usize]),
                        )])
                    }
                    tir::InstructionKind::Replace {
                        destination,
                        value,
                        displaced,
                        ..
                    } => StorageOp::Assign(vec![
                        (
                            self.place(*displaced),
                            StorageValue::Copy {
                                value: self.place(*destination),
                                detaches: false,
                            },
                        ),
                        (self.place(*destination), self.operand(value)),
                    ]),
                    tir::InstructionKind::Snapshot { destination, value } => {
                        StorageOp::Assign(vec![(self.place(*destination), self.operand(value))])
                    }
                    tir::InstructionKind::StorageLive(local) => StorageOp::Kill(vec![local.0]),
                    _ => StorageOp::Nop,
                };
                nodes.push(StorageNode {
                    operation,
                    successors: vec![nodes.len() + 1],
                    site: instruction.span,
                });
            }
            let (operation, successors) = match &block.terminator {
                tir::Terminator::Goto(next) => (StorageOp::Nop, vec![starts[next.0 as usize]]),
                tir::Terminator::Branch { yes, no, .. } => (
                    StorageOp::Nop,
                    vec![starts[yes.0 as usize], starts[no.0 as usize]],
                ),
                tir::Terminator::Switch {
                    arms, otherwise, ..
                } => (
                    StorageOp::Nop,
                    arms.iter()
                        .map(|(_, next)| starts[next.0 as usize])
                        .chain([starts[otherwise.0 as usize]])
                        .collect(),
                ),
                tir::Terminator::Match { table, .. } => (
                    StorageOp::Nop,
                    self.body.matches[table.0 as usize]
                        .arms
                        .iter()
                        .map(|arm| starts[arm.guard.unwrap_or(arm.target).0 as usize])
                        .collect(),
                ),
                tir::Terminator::Call {
                    callee,
                    arguments,
                    returns,
                    ..
                } => {
                    let mutations = self.mutations(callee, arguments)?;
                    let callee = match callee {
                        tir::Callee::Definition { path, arguments } => self.declared(
                            path,
                            arguments,
                            returns
                                .as_ref()
                                .map(|receipt| self.body.places[receipt.destination.0 as usize].ty),
                        ),
                        tir::Callee::Indirect { value, .. } => {
                            StorageCallee::Value(self.place(value.place))
                        }
                    };
                    (
                        StorageOp::Call {
                            mutations,
                            callee,
                            arguments: arguments
                                .iter()
                                .map(|argument| {
                                    (argument.parameter, self.place(argument.value.place))
                                })
                                .collect(),
                            destination: returns
                                .as_ref()
                                .map(|returns| self.place(returns.destination)),
                        },
                        returns
                            .iter()
                            .map(|returns| starts[returns.next.0 as usize])
                            .collect(),
                    )
                }
                tir::Terminator::Return { value, .. } => {
                    (StorageOp::Return(self.place(value.place)), vec![])
                }
                _ => (StorageOp::Nop, vec![]),
            };
            nodes.push(StorageNode {
                operation,
                successors,
                site: block.span,
            });
        }
        Ok(StorageBody {
            callable: self.callable(self.body),
            external_entry: self.body.parameter_scope.site == NameSite::Item
                && self
                    .signatures
                    .get(self.definition.path)
                    .is_none_or(|signature| {
                        signature.metadata.visibility != hirsel_syntax::Visibility::Private
                    }),
            source_type: callable_type
                .and_then(|ty| encoding::closed_storage_source_type(self.types, ty).ok()),
            paths,
            generics: callable_type
                .and_then(|ty| match &self.types.row(ty).kind {
                    TypeKind::Function(function) => Some(function.generics.to_vec()),
                    _ => None,
                })
                .unwrap_or_default(),
            inputs,
            nodes,
            entry: starts[self.body.entry.0 as usize],
        })
    }
}
fn unknown_callee() -> StorageCallee {
    StorageCallee::Declared {
        allocations: vec![],
        results: vec![],
        complete: false,
    }
}

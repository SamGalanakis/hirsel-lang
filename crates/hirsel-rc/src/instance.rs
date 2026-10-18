//! P1 instantiation of generic code and size parameters.
//!
//! Complete instances are expanded from the program's GEN-5 requirements, never
//! from another definition's body: every non-generic executable definition is a
//! root, and a generic owner instance reaches its callees only through the
//! requirements the driver collected (`NativeInput.instances`). A call site the
//! importer meets later must name an instance expanded here, or import fails
//! closed with `MissingInstance`.
//!
//! Each instance is an `InstanceView` over the shared generic TIR body: the
//! definition stays one `DefinitionRef` and the view owns only its substitution
//! store. Actuals are keyed by their lexical binders (`BinderRef::Body`) with
//! relations, brands, uniqueness and sendability erased by `InstanceKey`,
//! so two call sites that differ only there name
//! one instance.
use hirsel_core_contract::{OperationId, Passing};
use hirsel_layout::instance::{
    ActualBinder, CallableConvention, CaptureView, InstanceKey, InstanceKeyError, InstanceView,
    ParameterView, Substitution,
};
use hirsel_layout::layout::{LayoutBuilder, Owners};
use hirsel_lir::abi_types::{CallMultiplicity, CallableShape, FnAbiId};
use hirsel_lir::ir::CaptureUse;
use hirsel_lir::layout::LayoutError;
use hirsel_lir::layout_types::LayoutId;
use hirsel_lir::stage::InputIdentity;
use hirsel_span::Span;
use hirsel_syntax::{ContentHash, DefPath};
use hirsel_tir::{BodyId, DefinitionRef, Rvalue, Storage};
use hirsel_types::{
    AuthoritySummary, BinderRef, FunctionType, GenericArgument, GenericBinder, GraphError,
    Multiplicity, NameSite, SignatureKind, SignatureRef, Size, SizeParamId, SizeParameterRef, Type,
    TypeId, TypeKind, TypeParamId, TypeParameterRef, TypeStore, UseKind,
};
use std::collections::BTreeSet;
use std::collections::{BTreeMap, HashMap, VecDeque};

/// The tooling limit on instance expansion (a separate resource limit, never
/// a source rejection).
pub const MAX_INSTANCES: usize = 1 << 16;

/// Why an owner instance requires a callee's instance (the driver's
/// `generic_graph::InstanceKind`).
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub enum RequirementKind {
    Call,
    Reference,
    Drop,
    Representation,
    Materialized,
}

/// One GEN-5 requirement. `arguments` are rows of `types`; argument `i` binds
/// `BinderRef::Body { scope: binder, parameter: i }` of `callee`. Free `Body`
/// references name the owner's parameters, and when `owner_binder` is set (a
/// signature-owned store) so do `Bound` references at depth 0.
#[derive(Clone, Debug)]
pub struct Requirement {
    pub owner: DefPath,
    pub owner_binder: Option<NameSite>,
    pub site: Span,
    pub kind: RequirementKind,
    pub callee: DefPath,
    pub binder: NameSite,
    pub types: TypeStore,
    pub arguments: Vec<GenericArgument>,
}

/// Everything native passes read from a validated `NativeInput`, as plain
/// data. The driver fills it from `ValidatedNativeInput`; this crate
/// never depends on the driver.
#[derive(Clone, Debug)]
pub struct Program {
    pub input: InputIdentity,
    pub definitions: BTreeMap<DefPath, DefinitionRef>,
    pub signatures: BTreeMap<DefPath, SignatureRef>,
    /// Implementation revision of each executable definition's source owner.
    pub revisions: BTreeMap<DefPath, ContentHash>,
    /// The lexical binder of each source definition's generics.
    pub binders: BTreeMap<DefPath, NameSite>,
    /// Registry operations by definition path.
    pub operations: BTreeMap<DefPath, OperationId>,
    pub requirements: Vec<Requirement>,
    pub owners: Owners,
    /// Each constant's materialized value (CT-2), the source of its static data.
    pub constants: BTreeMap<DefPath, hirsel_tir::Materialized>,
    /// The instance definitions of generic generated declarations (CT-5).
    pub instance_bodies: hirsel_tir::InstanceBodies,
}

impl Program {
    /// Independent declaration input for C, separate from the lowered units.
    pub fn operation_registry(&self) -> hirsel_layout::operation::OperationRegistry {
        hirsel_layout::operation::OperationRegistry {
            signatures: self
                .operations
                .values()
                .filter_map(|op| {
                    self.signatures
                        .get(&op.definition)
                        .map(|s| (op.clone(), s.clone()))
                })
                .collect(),
            owners: crate::library::owners(&self.signatures).over(&self.owners),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct InstanceId(pub u32);

impl InstanceId {
    pub const fn index(self) -> usize {
        self.0 as usize
    }
}

/// What an instance executes.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum InstanceRole {
    /// A function definition's entry body.
    Function,
    /// A nominal's finalizer body (`TypeDefinition.drop`).
    Finalizer,
    /// A closure or contract body created inside `parent`.
    Closure { parent: InstanceId },
    /// A library body (`crate::library::BODIES[body]`), built in LIR and
    /// never read from TIR: its view's definition declares no body.
    /// `finalizer` marks the drop body of a library resource, which takes
    /// its resource borrowed as its one parameter.
    Library { body: usize, finalizer: bool },
}

impl InstanceRole {
    /// Whether the instance is the drop body of a resource layout.
    pub fn is_finalizer(&self) -> bool {
        matches!(
            self,
            Self::Finalizer
                | Self::Library {
                    finalizer: true,
                    ..
                }
        )
    }
    pub fn is_library(&self) -> bool {
        matches!(self, Self::Library { .. })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum InstanceError {
    MissingDefinition(DefPath),
    MissingSignature(DefPath),
    MissingBinder(DefPath),
    MissingRevision(DefPath),
    /// A generic actual that still names a parameter, or a size that is not constant.
    Incomplete(DefPath),
    Substitution(DefPath, GraphError),
    Key(DefPath, InstanceKeyError),
    Layout(DefPath, LayoutError),
    /// A call site whose instance the GEN-5 requirements never produced.
    MissingInstance(DefPath),
    /// More than `MAX_INSTANCES` instances: a tooling limit.
    Limit,
    Malformed(String),
}

/// The expanded program: instance views in discovery order, their roles, the
/// `FnAbi` of each and a key index.
pub struct Instances {
    pub views: Vec<InstanceView>,
    pub roles: Vec<InstanceRole>,
    /// The instance's callable type, a `Function` row of its own store.
    pub callables: Vec<TypeId>,
    pub abis: Vec<FnAbiId>,
    /// The actuals the instance's generic body is read under.
    pub actuals: Vec<Actuals>,
    /// A closure instance's environment layout.
    pub environments: Vec<Option<LayoutId>>,
    /// Ordinary data records created at generic nested-function declarations,
    /// keyed by the declaring instance and nested body. They contain no code.
    pub data_environments: HashMap<(InstanceId, BodyId), LayoutId>,
    /// The ordinary environment layout replacing each nonempty generic name.
    pub data_locals: HashMap<(InstanceId, hirsel_tir::LocalId), LayoutId>,
    /// Explicit capture parameters of direct nested-function instances.
    pub direct_captures: HashMap<InstanceId, Vec<DirectCapture>>,
    /// The instance each call of a generic nested function runs, by the
    /// calling instance and the TIR block whose terminator calls it.
    pub direct_calls: HashMap<(InstanceId, hirsel_tir::BlockId), InstanceId>,
    /// Generic function names with no runtime environment: calls name code
    /// directly, and captures/storage of these lexical names are unnecessary.
    pub elided: HashMap<InstanceId, std::collections::HashSet<hirsel_tir::LocalId>>,
    by_key: HashMap<InstanceKey, InstanceId>,
    producers: HashMap<(DefPath, BodyId, hirsel_tir::LocalId), Option<Producer>>,
}

/// One ordinary data parameter of a statically bound nested function.
#[derive(Clone, Debug)]
pub struct DirectCapture {
    pub local: hirsel_tir::LocalId,
    pub layout: LayoutId,
}

impl Instances {
    pub fn len(&self) -> usize {
        self.views.len()
    }
    pub fn is_empty(&self) -> bool {
        self.views.is_empty()
    }
    pub fn get(&self, key: &InstanceKey) -> Option<InstanceId> {
        self.by_key.get(key).copied()
    }
    pub fn view(&self, id: InstanceId) -> &InstanceView {
        &self.views[id.index()]
    }
    pub fn ids(&self) -> impl Iterator<Item = InstanceId> + '_ {
        (0..self.views.len() as u32).map(InstanceId)
    }
    /// The instance of `definition`'s entry body with these actuals.
    pub fn find(
        &self,
        program: &Program,
        definition: &DefPath,
        actuals: &Actuals,
    ) -> Result<InstanceId, InstanceError> {
        let def = program
            .definitions
            .get(definition)
            .ok_or_else(|| InstanceError::MissingDefinition(definition.clone()))?;
        let key = key_of(definition, def.entry, actuals, None, None)?;
        self.get(&key)
            .ok_or_else(|| InstanceError::MissingInstance(definition.clone()))
    }
    /// Register an instance under its key, with the `FnAbi` of `callable`
    /// at `shape`: its id, and whether it is new.
    pub(crate) fn push(
        &mut self,
        layouts: &mut dyn crate::library::builder::Layouts,
        mut view: InstanceView,
        role: InstanceRole,
        callable: TypeId,
        actuals: Actuals,
        shape: CallableShape,
        environment: Option<LayoutId>,
    ) -> Result<(InstanceId, bool), InstanceError> {
        if let Some(&id) = self.by_key.get(&view.key) {
            return Ok((id, false));
        }
        if self.views.len() >= MAX_INSTANCES {
            return Err(InstanceError::Limit);
        }
        let path = view.key.definition().clone();
        let abi = layouts
            .fn_abi(&view.substitution.store, callable, shape)
            .map_err(|e| InstanceError::Layout(path, e))?;
        let abi = if role.is_library() || view.definition.tracks_caller(view.body) {
            layouts
                .with_caller_site(abi)
                .map_err(|e| InstanceError::Layout(view.key.definition().clone(), e))?
        } else {
            abi
        };
        view.caller_destination = layouts.table().fn_abis[abi.0 as usize].destination;
        let id = InstanceId(self.views.len() as u32);
        self.by_key.insert(view.key.clone(), id);
        self.views.push(view);
        self.roles.push(role);
        self.callables.push(callable);
        self.abis.push(abi);
        self.actuals.push(actuals);
        self.environments.push(environment);
        Ok((id, true))
    }
    /// The closure instance of `body` created inside `parent`.
    pub fn closure(&self, parent: InstanceId, body: BodyId) -> Option<InstanceId> {
        self.ids().find(|&id| {
            self.roles[id.index()] == InstanceRole::Closure { parent }
                && self.views[id.index()].body == body
        })
    }
}

/// Complete actuals of one binder: an owned store and one argument per generic
/// parameter, in ordinal order. A generic nested function's instance (LIR
/// schema revision 9) also binds its enclosing functions' generics, which
/// its body names: `outer` lists each enclosing binder's arguments, rows of
/// the same store, innermost first.
#[derive(Clone, Debug)]
pub struct Actuals {
    pub binder: Option<NameSite>,
    pub store: TypeStore,
    pub arguments: Vec<GenericArgument>,
    pub outer: Vec<(NameSite, Vec<GenericArgument>)>,
}

impl Actuals {
    pub fn none() -> Self {
        Self {
            binder: None,
            store: TypeStore::default(),
            arguments: Vec::new(),
            outer: Vec::new(),
        }
    }
    /// Every binder these actuals bind with its arguments, own first.
    pub fn binders(&self) -> impl Iterator<Item = (NameSite, &[GenericArgument])> {
        self.binder
            .map(|scope| (scope, self.arguments.as_slice()))
            .into_iter()
            .chain(self.outer.iter().map(|(s, a)| (*s, a.as_slice())))
    }
    /// The view's substitution tables: binder-aware type and folded size actuals.
    pub fn substitution(&self, path: &DefPath) -> Result<Substitution, InstanceError> {
        let mut types = Vec::new();
        let mut sizes = Vec::new();
        for (scope, arguments) in self.binders() {
            for (ordinal, argument) in arguments.iter().enumerate() {
                match argument {
                    GenericArgument::Type(ty) => types.push((
                        TypeParameterRef::body(scope, TypeParamId(ordinal as u32)),
                        *ty,
                    )),
                    GenericArgument::Size(Size::Constant(n)) => sizes.push((
                        SizeParameterRef::body(scope, SizeParamId(ordinal as u32)),
                        *n,
                    )),
                    GenericArgument::Size(Size::Parameter(_)) => {
                        return Err(InstanceError::Incomplete(path.clone()));
                    }
                }
            }
        }
        Ok(Substitution {
            store: self.store.clone(),
            types,
            sizes,
        })
    }
    pub(crate) fn key_binders(&self) -> Vec<(ActualBinder, GenericArgument)> {
        self.binders()
            .flat_map(|(scope, arguments)| {
                arguments
                    .iter()
                    .enumerate()
                    .map(move |(ordinal, argument)| {
                        let binder =
                            match argument {
                                GenericArgument::Type(_) => ActualBinder::Type(
                                    TypeParameterRef::body(scope, TypeParamId(ordinal as u32)),
                                ),
                                GenericArgument::Size(_) => ActualBinder::Size(
                                    SizeParameterRef::body(scope, SizeParamId(ordinal as u32)),
                                ),
                            };
                        (binder, argument.clone())
                    })
            })
            .collect()
    }
}

/// Substitute `ty` of `source` (a store whose free generic references name
/// `owner`'s binder) into `destination`, replacing them by `owner`'s actuals.
/// Authority summaries of replaced parameters are conservatively unknown, the
/// same on every path, so keys computed by expansion and by import agree.
pub fn substitute(
    source: &TypeStore,
    ty: TypeId,
    owner: &Actuals,
    owner_actuals_in_destination: &[GenericArgument],
    destination: &mut TypeStore,
    also_bound: bool,
) -> Result<TypeId, GraphError> {
    let authorities = vec![AuthoritySummary::Unknown; owner_actuals_in_destination.len()];
    let body = hirsel_types::Substitution {
        binder: owner.binder.map(GenericBinder::Body),
        arguments: owner_actuals_in_destination,
        authorities: &authorities,
        ..Default::default()
    };
    let mut id = hirsel_types::substitute_type(source, ty, destination, body)?;
    if also_bound && !owner_actuals_in_destination.is_empty() {
        let bound = hirsel_types::Substitution {
            binder: Some(GenericBinder::Bound(0)),
            arguments: owner_actuals_in_destination,
            authorities: &authorities,
            ..Default::default()
        };
        id = hirsel_types::substitute_type_in_place(destination, id, bound)?;
    }
    // An enclosing function's generics, which a nested function's body names.
    for (scope, arguments) in &owner.outer {
        let arguments = import_arguments(&owner.store, arguments, destination)?;
        let authorities = vec![AuthoritySummary::Unknown; arguments.len()];
        let enclosing = hirsel_types::Substitution {
            binder: Some(GenericBinder::Body(*scope)),
            arguments: &arguments,
            authorities: &authorities,
            ..Default::default()
        };
        id = hirsel_types::substitute_type_in_place(destination, id, enclosing)?;
    }
    Ok(id)
}

/// `arguments`, rows of `source`, imported into `destination`.
fn import_arguments(
    source: &TypeStore,
    arguments: &[GenericArgument],
    destination: &mut TypeStore,
) -> Result<Vec<GenericArgument>, GraphError> {
    arguments
        .iter()
        .map(|argument| {
            Ok(match argument {
                GenericArgument::Type(ty) => {
                    GenericArgument::Type(hirsel_types::import_type(source, *ty, destination)?)
                }
                size => size.clone(),
            })
        })
        .collect()
}

/// Import `owner`'s actuals into `destination`.
pub fn import_actuals(
    owner: &Actuals,
    destination: &mut TypeStore,
) -> Result<Vec<GenericArgument>, GraphError> {
    import_arguments(&owner.store, &owner.arguments, destination)
}

/// The callee actuals of a requirement or call site whose `arguments` live in
/// `types` and name `owner`'s binder.
pub fn callee_actuals(
    callee_binder: Option<NameSite>,
    types: &TypeStore,
    arguments: &[GenericArgument],
    owner: &Actuals,
    also_bound: bool,
    path: &DefPath,
) -> Result<Actuals, InstanceError> {
    let mut store = TypeStore::default();
    let owner_actuals = import_actuals(owner, &mut store)
        .map_err(|e| InstanceError::Substitution(path.clone(), e))?;
    let mut out = Vec::new();
    for argument in arguments {
        out.push(match argument {
            GenericArgument::Type(ty) => GenericArgument::Type(
                substitute(types, *ty, owner, &owner_actuals, &mut store, also_bound)
                    .map_err(|e| InstanceError::Substitution(path.clone(), e))?,
            ),
            GenericArgument::Size(Size::Constant(n)) => GenericArgument::Size(Size::Constant(*n)),
            GenericArgument::Size(Size::Parameter(p)) => {
                let selected = match *p {
                    BinderRef::Body { scope, parameter } => owner
                        .binders()
                        .find(|(s, _)| *s == scope)
                        .and_then(|(_, arguments)| arguments.get(parameter.0 as usize)),
                    BinderRef::Bound {
                        depth: 0,
                        parameter,
                    } if also_bound => owner.arguments.get(parameter.0 as usize),
                    BinderRef::Bound { .. } => None,
                };
                match selected {
                    Some(GenericArgument::Size(Size::Constant(n))) => {
                        GenericArgument::Size(Size::Constant(*n))
                    }
                    _ => return Err(InstanceError::Incomplete(path.clone())),
                }
            }
        });
    }
    Ok(Actuals {
        binder: if out.is_empty() { None } else { callee_binder },
        store,
        arguments: out,
        outer: Vec::new(),
    })
}

/// The canonical key of one instance.
pub fn key_of(
    path: &DefPath,
    body: BodyId,
    actuals: &Actuals,
    convention: Option<&CallableConvention>,
    environment: Option<ContentHash>,
) -> Result<InstanceKey, InstanceError> {
    InstanceKey::new(
        path,
        body.0,
        &actuals.key_binders(),
        &actuals.store,
        convention,
        environment,
    )
    .map_err(|e| InstanceError::Key(path.clone(), e))
}

fn multiplicity(m: Multiplicity) -> CallMultiplicity {
    match m {
        Multiplicity::Many => CallMultiplicity::Many,
        Multiplicity::Once => CallMultiplicity::Once,
    }
}

/// A non-generic `Function` row: `function`'s parameters, result and error
/// substituted (their generic references are `Bound` at depth 0 inside the
/// declaring function type), pushed into `actuals`' store.
pub fn instantiate_function(
    signature: &Signature,
    function: &FunctionType,
    modes: hirsel_types::Modes,
    actuals: &mut Actuals,
) -> Result<TypeId, GraphError> {
    let arguments = actuals.arguments.clone();
    let authorities = vec![AuthoritySummary::Unknown; arguments.len()];
    let substitution = hirsel_types::Substitution {
        binder: Some(GenericBinder::Bound(0)),
        arguments: &arguments,
        authorities: &authorities,
        ..Default::default()
    };
    let one = |ty: TypeId, store: &mut TypeStore| {
        hirsel_types::substitute_type(&signature.types, ty, store, substitution)
    };
    let mut parameters = Vec::new();
    for parameter in function.parameters.iter() {
        let mut p = parameter.clone();
        p.ty = one(parameter.ty, &mut actuals.store)?;
        parameters.push(p);
    }
    let result = one(function.result, &mut actuals.store)?;
    let error = match function.error {
        Some(e) => Some(one(e, &mut actuals.store)?),
        None => None,
    };
    let row = Type {
        modes,
        ..Type::with_kind(TypeKind::Function(FunctionType {
            state_promises: function.state_promises.clone(),
            generics: Vec::new().into(),
            parameters: parameters.into(),
            result,
            error,
            allocation: function.allocation,
            authority: AuthoritySummary::Unknown,
            uses: function.uses.clone(),
            contracts: Vec::new().into(),
            incomplete: false,
            foreign: function.foreign.clone(),
        }))
    };
    Ok(actuals.store.push(row))
}

use hirsel_types::Signature;

/// The declared conventions of a `Function` row.
pub fn convention(store: &TypeStore, callable: TypeId) -> Option<CallableConvention> {
    let row = store.get(callable)?;
    let TypeKind::Function(f) = &row.kind else {
        return None;
    };
    Some(CallableConvention {
        parameters: f
            .parameters
            .iter()
            .map(|p| Passing::from(p.passing))
            .collect(),
        multiplicity: multiplicity(row.modes.multiplicity),
    })
}

fn capture_use(usage: UseKind) -> CaptureUse {
    match usage {
        UseKind::Borrow => CaptureUse::Borrow,
        UseKind::Take => CaptureUse::Take,
        UseKind::Copy => CaptureUse::Copy,
        UseKind::Mut => CaptureUse::Mut,
    }
}

/// The environment passing of one capture: copies and moves are owned.
pub fn capture_passing(usage: CaptureUse) -> Passing {
    match usage {
        CaptureUse::Borrow => Passing::Borrow,
        CaptureUse::Mut => Passing::Mut,
        CaptureUse::Take | CaptureUse::Copy => Passing::Take,
    }
}

struct Expander<'p, 'l, 'o> {
    program: &'p Program,
    layouts: &'l mut LayoutBuilder<'o>,
    out: Instances,
    queue: VecDeque<InstanceId>,
    /// Requirements by owner, in collection order.
    by_owner: BTreeMap<&'p DefPath, Vec<&'p Requirement>>,
    /// Nominal instances already visited (recursive representations).
    nominals: std::collections::HashSet<InstanceKey>,
}

/// Expand every instance the program executes.
pub fn expand(
    program: &Program,
    layouts: &mut LayoutBuilder<'_>,
) -> Result<Instances, InstanceError> {
    let mut by_owner: BTreeMap<&DefPath, Vec<&Requirement>> = BTreeMap::new();
    for requirement in &program.requirements {
        by_owner
            .entry(&requirement.owner)
            .or_default()
            .push(requirement);
    }
    let mut e = Expander {
        program,
        layouts,
        out: Instances {
            views: Vec::new(),
            roles: Vec::new(),
            callables: Vec::new(),
            abis: Vec::new(),
            actuals: Vec::new(),
            environments: Vec::new(),
            data_environments: HashMap::new(),
            data_locals: HashMap::new(),
            direct_captures: HashMap::new(),
            direct_calls: HashMap::new(),
            elided: HashMap::new(),
            by_key: HashMap::new(),
            producers: lexical_producers(program),
        },
        queue: VecDeque::new(),
        by_owner,
        nominals: Default::default(),
    };
    for (path, signature) in &program.signatures {
        if !program.definitions.contains_key(path) {
            continue;
        }
        match &signature.kind {
            SignatureKind::Function(f) if f.generics.is_empty() => {
                e.function(path, Actuals::none())?;
            }
            _ => {}
        }
    }
    // Every non-generic nominal: its finalizer, and what its representation requires.
    for (path, signature) in &program.signatures {
        if let SignatureKind::Type(t) = &signature.kind
            && t.generics.is_empty()
        {
            e.nominal(path, Actuals::none())?;
        }
    }
    while let Some(id) = e.queue.pop_front() {
        e.visit(id)?;
    }
    Ok(e.out)
}

impl Expander<'_, '_, '_> {
    fn signature(&self, path: &DefPath) -> Result<&SignatureRef, InstanceError> {
        self.program
            .signatures
            .get(path)
            .ok_or_else(|| InstanceError::MissingSignature(path.clone()))
    }

    fn insert(
        &mut self,
        view: InstanceView,
        role: InstanceRole,
        callable: TypeId,
        actuals: Actuals,
        shape: CallableShape,
        environment: Option<LayoutId>,
    ) -> Result<InstanceId, InstanceError> {
        let (id, fresh) = self.out.push(
            self.layouts,
            view,
            role,
            callable,
            actuals,
            shape,
            environment,
        )?;
        if fresh {
            self.queue.push_back(id);
        }
        Ok(id)
    }

    /// The entry-body instance of a function definition.
    fn function(&mut self, path: &DefPath, actuals: Actuals) -> Result<InstanceId, InstanceError> {
        let signature = self.signature(path)?.clone();
        let SignatureKind::Function(function) = &signature.kind else {
            return Err(InstanceError::Malformed(format!(
                "{path:?} is called but is not a function"
            )));
        };
        let definition = self
            .program
            .definitions
            .get(path)
            .ok_or_else(|| InstanceError::MissingDefinition(path.clone()))?
            .clone();
        let key = key_of(path, definition.entry, &actuals, None, None)?;
        if let Some(&id) = self.out.by_key.get(&key) {
            return Ok(id);
        }
        let mut actuals = actuals;
        let callable = instantiate_function(
            &signature,
            function,
            hirsel_types::Modes::global(),
            &mut actuals,
        )
        .map_err(|e| InstanceError::Substitution(path.clone(), e))?;
        let parameters = parameter_views(&actuals.store, callable);
        let view = InstanceView {
            key,
            body: definition.entry,
            definition,
            owner: CallableShape::Definition,
            substitution: actuals.substitution(path)?,
            parameters,
            captures: Vec::new(),
            caller_destination: false,
        };
        self.insert(
            view,
            InstanceRole::Function,
            callable,
            actuals,
            CallableShape::Definition,
            None,
        )
    }

    /// A nominal instance: its finalizer, if it declares one, and the
    /// requirements of its representation.
    fn nominal(&mut self, path: &DefPath, actuals: Actuals) -> Result<(), InstanceError> {
        let signature = self.signature(path)?.clone();
        let SignatureKind::Type(t) = &signature.kind else {
            return Ok(());
        };
        let visited = InstanceKey::new(path, 0, &actuals.key_binders(), &actuals.store, None, None)
            .map_err(|e| InstanceError::Key(path.clone(), e))?;
        if !self.nominals.insert(visited) {
            return Ok(());
        }
        self.follow(path, &actuals, None)?;
        // A library resource's drop body is a library body named by its
        // owner's native representation.
        if let Some(hirsel_layout::layout::OwnerRepresentation::Defined(record)) =
            self.layouts.owners().nominals.get(path)
            && let Some((finalizer, _)) = &record.finalizer
            && let Some(body) = crate::library::body_index(finalizer)
        {
            let request = crate::library::Request::finalizer(body, finalizer, path, &actuals);
            crate::library::instantiate(self.layouts, &mut self.out, request)?;
            return Ok(());
        }
        let Some(finalizer) = &t.drop else {
            return Ok(());
        };
        // A library finalizer is runtime code, not a TIR body.
        let Some(definition) = self.program.definitions.get(finalizer).cloned() else {
            return Ok(());
        };
        let key = key_of(finalizer, definition.entry, &actuals, None, None)?;
        if self.out.by_key.contains_key(&key) {
            return Ok(());
        }
        // The finalizer receives its value borrowed and returns unit (DROP-2).
        let mut actuals = actuals;
        let body = &definition.bodies[definition.entry.0 as usize];
        let parameter = body
            .parameters
            .first()
            .ok_or_else(|| InstanceError::Malformed(format!("{finalizer:?} has no self")))?;
        let own = actuals.clone();
        let owner_actuals = import_actuals(&own, &mut actuals.store)
            .map_err(|e| InstanceError::Substitution(path.clone(), e))?;
        let self_ty = substitute(
            &definition.types,
            body.locals[parameter.0 as usize].ty,
            &own,
            &owner_actuals,
            &mut actuals.store,
            false,
        )
        .map_err(|e| InstanceError::Substitution(path.clone(), e))?;
        let unit = actuals.store.push(Type::unit());
        let callable = actuals
            .store
            .push(Type::with_kind(TypeKind::Function(FunctionType {
                state_promises: Default::default(),
                generics: Vec::new().into(),
                parameters: vec![hirsel_types::Parameter {
                    name: None,
                    label: None,
                    passing: hirsel_syntax::PassingMode::Borrow,
                    ty: self_ty,
                }]
                .into(),
                result: unit,
                error: None,
                allocation: hirsel_types::Allocation::MayAllocate,
                authority: AuthoritySummary::Unknown,
                uses: hirsel_types::Uses::Unrestricted,
                contracts: Vec::new().into(),
                incomplete: false,
                foreign: None,
            })));
        let view = InstanceView {
            key,
            body: definition.entry,
            definition,
            owner: CallableShape::Definition,
            substitution: actuals.substitution(finalizer)?,
            parameters: parameter_views(&actuals.store, callable),
            captures: Vec::new(),
            caller_destination: false,
        };
        self.insert(
            view,
            InstanceRole::Finalizer,
            callable,
            actuals,
            CallableShape::Definition,
            None,
        )?;
        Ok(())
    }

    /// Visit one instance: its owner's requirements, the closures its body
    /// creates and the generic nested functions it calls.
    fn visit(&mut self, id: InstanceId) -> Result<(), InstanceError> {
        // A library body is built, not read: it has no requirements here.
        if self.out.roles[id.index()].is_library() {
            return Ok(());
        }
        let view = self.out.views[id.index()].clone();
        let actuals = self.out.actuals[id.index()].clone();
        let path = view.key.definition().clone();
        let body = &view.definition.bodies[view.body.0 as usize];
        let elided = body
            .locals
            .iter()
            .enumerate()
            .filter_map(|(local, row)| {
                let local = hirsel_tir::LocalId(local as u32);
                (generic_function(&view.definition.types, row.ty)
                    && nested_function(&self.out, id, local)
                        .is_some_and(|(_, _, _, _, empty)| empty))
                .then_some(local)
            })
            .collect();
        self.out.elided.insert(id, elided);
        for (i, local) in body.locals.iter().enumerate() {
            let local_id = hirsel_tir::LocalId(i as u32);
            if generic_function(&view.definition.types, local.ty)
                && !self.out.elided[&id].contains(&local_id)
            {
                self.data_local(id, local_id)?;
            }
        }

        match self.out.roles[id.index()] {
            // A generic nested function's instance follows the requirements
            // its own generics take part in; its enclosing instance, which
            // followed the rest, could not bind them.
            InstanceRole::Closure { .. } if actuals.binder == Some(body.parameter_scope) => {
                self.follow(&path, &actuals, Some(body.parameter_scope))?;
            }
            InstanceRole::Closure { .. } => {}
            _ => self.follow(&path, &actuals, None)?,
        }
        for block in &body.blocks {
            for instruction in &block.instructions {
                let hirsel_tir::InstructionKind::Assign { destination, value } = &instruction.kind
                else {
                    continue;
                };
                let Rvalue::Closure {
                    body: closure,
                    captures,
                    ..
                } = &body.rvalues[value.0 as usize]
                else {
                    continue;
                };
                let ty = body.places[destination.0 as usize].ty;
                if generic_function(&view.definition.types, ty) {
                    if self.out.elided.get(&id).is_some_and(|locals| {
                        locals.contains(&body.places[destination.0 as usize].root)
                    }) {
                        continue;
                    }
                    // Spec #42: retain only the declaration's data snapshot;
                    // every statically bound call names its concrete code.
                    let (environment, _, _) =
                        self.environment(id, &view, &actuals, *closure, captures)?;
                    self.out
                        .data_environments
                        .insert((id, *closure), environment);
                } else {
                    self.closure(id, &view, &actuals, *closure, ty, captures)?;
                }
            }
        }
        for (b, block) in body.blocks.iter().enumerate() {
            let hirsel_tir::Terminator::Call {
                callee:
                    hirsel_tir::Callee::Indirect {
                        forward_caller: _,
                        value,
                        arguments,
                    },
                ..
            } = &block.terminator
            else {
                continue;
            };
            if arguments.is_empty() {
                continue;
            }
            let local = body.places[value.place.0 as usize].root;
            let (producer, closure, ty, captures, empty) = nested_function(&self.out, id, local)
                .ok_or_else(|| InstanceError::Malformed(format!(
                    "{path:?}: a generic callee with no lexical nested-function producer (spec #42)"
                )))?;
            let captures = captures.to_vec();
            let producer_view = self.out.view(producer).clone();
            let producer_actuals = self.out.actuals[producer.index()].clone();
            let bound = self.direct_nested(
                producer,
                &producer_view,
                &producer_actuals,
                &actuals,
                closure,
                ty,
                &captures,
                empty,
                arguments,
            )?;
            self.out
                .direct_calls
                .insert((id, hirsel_tir::BlockId(b as u32)), bound);
        }
        Ok(())
    }

    fn data_local(
        &mut self,
        id: InstanceId,
        local: hirsel_tir::LocalId,
    ) -> Result<LayoutId, InstanceError> {
        if let Some(&layout) = self.out.data_locals.get(&(id, local)) {
            return Ok(layout);
        }
        let (parent, body, _, captures, empty) =
            nested_function(&self.out, id, local).ok_or_else(|| {
                InstanceError::Malformed("generic environment has no lexical producer".into())
            })?;
        if empty {
            return Err(InstanceError::Malformed(
                "empty generic environment should be elided".into(),
            ));
        }
        let captures = captures.to_vec();
        let view = self.out.view(parent).clone();
        let actuals = self.out.actuals[parent.index()].clone();
        let layout = match self.out.data_environments.get(&(parent, body)).copied() {
            Some(layout) => layout,
            None => {
                let (layout, _, _) = self.environment(parent, &view, &actuals, body, &captures)?;
                self.out.data_environments.insert((parent, body), layout);
                layout
            }
        };
        self.out.data_locals.insert((id, local), layout);
        Ok(layout)
    }

    /// The environment of closure `body_id` created in `parent_view` under
    /// `parent_actuals`: its layout, digest and capture views. It depends
    /// only on the enclosing instance, never on a nested function's own
    /// generics, so a generic nested function's value and every bound
    /// instance of it agree on it.
    fn environment(
        &mut self,
        parent: InstanceId,
        parent_view: &InstanceView,
        parent_actuals: &Actuals,
        body_id: BodyId,
        captures: &[hirsel_tir::Operand],
    ) -> Result<(LayoutId, ContentHash, Vec<CaptureView>), InstanceError> {
        let definition = &parent_view.definition;
        let path = definition.path.clone();
        let body = definition
            .bodies
            .get(body_id.0 as usize)
            .ok_or_else(|| InstanceError::Malformed(format!("{path:?}: closure body")))?;
        let mut actuals = parent_actuals.clone();
        let own = parent_actuals.clone();
        let owner_actuals = import_actuals(&own, &mut actuals.store)
            .map_err(|e| InstanceError::Substitution(path.clone(), e))?;
        let capture_locals: Vec<_> = body
            .locals
            .iter()
            .filter(|l| l.storage == Storage::Capture)
            .collect();
        if capture_locals.len() != captures.len() {
            return Err(InstanceError::Malformed(format!(
                "{path:?}: closure body {} declares {} captures, its literal supplies {}",
                body_id.0,
                capture_locals.len(),
                captures.len()
            )));
        }
        let taken = taken_capture_locals(body);
        let mut movable = Vec::new();
        let mut views = Vec::new();
        let mut shape = Vec::new();
        for (source_ordinal, (local, operand)) in capture_locals.iter().zip(captures).enumerate() {
            let source = parent_view.definition.bodies[parent_view.body.0 as usize].places
                [operand.place.0 as usize]
                .root;
            if generic_function(&definition.types, local.ty)
                && nested_function(&self.out, parent, source)
                    .is_some_and(|(_, _, _, _, empty)| empty)
            {
                continue;
            }
            let ordinal = views.len();
            let usage = capture_use(operand.usage);
            views.push(CaptureView {
                ordinal: ordinal as u32,
                usage,
            });
            let ty = substitute(
                &definition.types,
                local.ty,
                &own,
                &owner_actuals,
                &mut actuals.store,
                false,
            )
            .map_err(|e| InstanceError::Substitution(path.clone(), e))?;
            let layout = if generic_function(&definition.types, local.ty) {
                self.data_local(parent, source)?
            } else {
                self.layouts
                    .layout(&actuals.store, ty)
                    .map_err(|e| InstanceError::Layout(path.clone(), e))?
            };
            if taken.contains(&(source_ordinal as u32))
                && usage == CaptureUse::Take
                && self.layouts.record(layout).copy
                    == hirsel_lir::layout_types::CopyRecipe::NotCopyable
            {
                movable.push(ordinal as u32);
            }
            shape.push((layout, capture_passing(usage)));
        }
        let environment = self
            .layouts
            .environment_data(&path, body_id.0, &shape, &movable)
            .map_err(|e| InstanceError::Layout(path.clone(), e))?;
        let digest = self
            .layouts
            .table()
            .record_digest(environment)
            .ok_or_else(|| InstanceError::Malformed("environment digest".into()))?;
        Ok((environment, digest, views))
    }

    /// The instance a call of generic nested function `body_id` runs with
    /// generic `arguments` (rows of the definition's store, naming the
    /// caller's generics): its own generics bound to them and every
    /// enclosing function's to the caller's (LIR schema revision 9).
    #[allow(clippy::too_many_arguments)]
    fn direct_nested(
        &mut self,
        parent: InstanceId,
        parent_view: &InstanceView,
        parent_actuals: &Actuals,
        caller_actuals: &Actuals,
        body_id: BodyId,
        ty: TypeId,
        captures: &[hirsel_tir::Operand],
        empty: bool,
        arguments: &[GenericArgument],
    ) -> Result<InstanceId, InstanceError> {
        let definition = parent_view.definition.clone();
        let path = definition.path.clone();
        let scope = definition
            .bodies
            .get(body_id.0 as usize)
            .ok_or_else(|| InstanceError::Malformed(format!("{path:?}: closure body")))?
            .parameter_scope;
        let mut actuals = callee_actuals(
            Some(scope),
            &definition.types,
            arguments,
            caller_actuals,
            false,
            &path,
        )?;
        for (enclosing, arguments) in parent_actuals.binders() {
            let arguments = import_arguments(&parent_actuals.store, arguments, &mut actuals.store)
                .map_err(|e| InstanceError::Substitution(path.clone(), e))?;
            actuals.outer.push((enclosing, arguments));
        }
        let owner_actuals = import_actuals(parent_actuals, &mut actuals.store)
            .map_err(|e| InstanceError::Substitution(path.clone(), e))?;
        let generic = substitute(
            &definition.types,
            ty,
            parent_actuals,
            &owner_actuals,
            &mut actuals.store,
            false,
        )
        .map_err(|e| InstanceError::Substitution(path.clone(), e))?;
        let arguments = actuals.arguments.clone();
        let callable = instantiate_row(&mut actuals.store, generic, &arguments)
            .map_err(|e| InstanceError::Substitution(path.clone(), e))?;
        let (environment, digest, views) = if empty {
            (None, None, Vec::new())
        } else {
            let (environment, digest, views) =
                self.environment(parent, parent_view, parent_actuals, body_id, captures)?;
            (Some(environment), Some(digest), views)
        };
        let convention = convention(&actuals.store, callable).ok_or_else(|| {
            InstanceError::Malformed(format!("{path:?}: closure of a non-function type"))
        })?;
        let key = key_of(&path, body_id, &actuals, Some(&convention), digest)?;
        if let Some(id) = self.out.get(&key) {
            return Ok(id);
        }
        let shape = CallableShape::Definition;
        let view = InstanceView {
            key,
            body: body_id,
            definition,
            owner: shape,
            substitution: actuals.substitution(&path)?,
            parameters: parameter_views(&actuals.store, callable),
            captures: views,
            caller_destination: false,
        };
        let id = self.insert(
            view,
            InstanceRole::Closure { parent },
            callable,
            actuals,
            shape,
            None,
        )?;
        if let Some(environment) = environment {
            let body = &self.out.view(id).definition.bodies[body_id.0 as usize];
            let locals: Vec<_> = body
                .locals
                .iter()
                .enumerate()
                .filter(|(_, local)| local.storage == Storage::Capture)
                .map(|(i, _)| hirsel_tir::LocalId(i as u32))
                .collect();
            // Static generic names have no field in this record.
            let locals: Vec<_> = locals
                .into_iter()
                .filter(|local| {
                    !nested_function(&self.out, id, *local).is_some_and(|(_, _, _, _, empty)| empty)
                })
                .collect();
            let fields = self.layouts.record(environment).fields.clone();
            let mut explicit = Vec::new();
            let mut parameters = Vec::new();
            for (local, field) in locals.iter().zip(&fields) {
                explicit.push(DirectCapture {
                    local: *local,
                    layout: field.layout,
                });
                parameters.push((field.layout, Passing::Borrow));
            }
            let abi = self
                .layouts
                .with_capture_parameters(self.out.abis[id.index()], &parameters, true)
                .map_err(|e| InstanceError::Layout(path.clone(), e))?;
            self.out.abis[id.index()] = abi;
            self.out.direct_captures.insert(id, explicit);
        }
        Ok(id)
    }

    fn closure(
        &mut self,
        parent: InstanceId,
        parent_view: &InstanceView,
        parent_actuals: &Actuals,
        body_id: BodyId,
        ty: TypeId,
        captures: &[hirsel_tir::Operand],
    ) -> Result<InstanceId, InstanceError> {
        let definition = parent_view.definition.clone();
        let path = definition.path.clone();
        let mut actuals = parent_actuals.clone();
        let own = parent_actuals.clone();
        let owner_actuals = import_actuals(&own, &mut actuals.store)
            .map_err(|e| InstanceError::Substitution(path.clone(), e))?;
        let callable = substitute(
            &definition.types,
            ty,
            &own,
            &owner_actuals,
            &mut actuals.store,
            false,
        )
        .map_err(|e| InstanceError::Substitution(path.clone(), e))?;
        let (environment, digest, views) =
            self.environment(parent, parent_view, parent_actuals, body_id, captures)?;
        let convention = convention(&actuals.store, callable).ok_or_else(|| {
            InstanceError::Malformed(format!("{path:?}: closure of a non-function type"))
        })?;
        let key = key_of(&path, body_id, &actuals, Some(&convention), Some(digest))?;
        let view = InstanceView {
            key,
            body: body_id,
            definition: definition.clone(),
            owner: CallableShape::Closure,
            substitution: actuals.substitution(&path)?,
            parameters: parameter_views(&actuals.store, callable),
            captures: views,
            caller_destination: false,
        };
        self.insert(
            view,
            InstanceRole::Closure { parent },
            callable,
            actuals,
            CallableShape::Closure,
            Some(environment),
        )
    }

    /// Follow `owner`'s requirements under its actuals. A requirement naming
    /// generics the actuals do not bind lies in a generic nested function's
    /// body, whose instances follow it; with `only`, follow just those that
    /// name that nested function's generics.
    fn follow(
        &mut self,
        owner: &DefPath,
        actuals: &Actuals,
        only: Option<NameSite>,
    ) -> Result<(), InstanceError> {
        let requirements = self.by_owner.get(owner).cloned().unwrap_or_default();
        for requirement in requirements {
            let named = body_scopes(&requirement.types, &requirement.arguments);
            let bound: Vec<NameSite> = actuals.binders().map(|(s, _)| s).collect();
            if !named.iter().all(|s| bound.contains(s))
                || only.is_some_and(|scope| !named.contains(&scope))
            {
                continue;
            }
            let callee = &requirement.callee;
            let callee_actuals = callee_actuals(
                Some(requirement.binder),
                &requirement.types,
                &requirement.arguments,
                actuals,
                requirement.owner_binder.is_some(),
                callee,
            )?;
            match &self.signature(callee)?.kind {
                SignatureKind::Function(_) => {
                    if let Some(body) = crate::library::body_index(callee) {
                        // An operation with a library body resolves to its
                        // builder, never to a TIR body.
                        let request = crate::library::Request::operation(
                            self.program,
                            body,
                            callee,
                            callee_actuals.store,
                            callee_actuals.arguments,
                        )?;
                        crate::library::instantiate(self.layouts, &mut self.out, request)?;
                    } else if self.program.definitions.contains_key(callee) {
                        self.function(callee, callee_actuals)?;
                    }
                }
                SignatureKind::Type(_) => self.nominal(callee, callee_actuals)?,
                SignatureKind::Const { ty: _, .. } => {}
            }
        }
        Ok(())
    }
}

/// The identity parameter mapping of a callable, with its declared conventions.
pub fn parameter_views(store: &TypeStore, callable: TypeId) -> Vec<ParameterView> {
    let Some(Type {
        kind: TypeKind::Function(f),
        ..
    }) = store.get(callable)
    else {
        return Vec::new();
    };
    f.parameters
        .iter()
        .enumerate()
        .map(|(i, p)| ParameterView {
            source: i as u32,
            instance: i as u32,
            passing: Passing::from(p.passing),
        })
        .collect()
}

/// Whether `ty` is a generic function type: a generic nested function's
/// value (spec #42).
fn generic_function(types: &TypeStore, ty: TypeId) -> bool {
    matches!(
        types.get(ty).map(|t| &t.kind),
        Some(TypeKind::Function(f)) if !f.generics.is_empty()
    )
}

#[derive(Clone, Copy)]
struct Producer {
    owner: BodyId,
    value: hirsel_tir::RvalueId,
    ty: TypeId,
    empty: bool,
}

#[derive(Clone, Copy)]
enum ProducerSource {
    Literal(Producer),
    Alias(BodyId, hirsel_tir::LocalId),
}

/// Index each lexical producer edge once, then resolve shared alias/capture
/// paths once. Instance ancestry selects actuals later; generic binder
/// identities never enter an ordinal-only lookup.
fn lexical_producers(
    program: &Program,
) -> HashMap<(DefPath, BodyId, hirsel_tir::LocalId), Option<Producer>> {
    let mut out = HashMap::new();
    for (path, definition) in &program.definitions {
        let mut sources = HashMap::new();
        let mut creators = HashMap::new();
        for (b, body) in definition.bodies.iter().enumerate() {
            let owner = BodyId(b as u32);
            for (r, value) in body.rvalues.iter().enumerate() {
                if let Rvalue::Closure { body: child, .. } = value {
                    creators
                        .entry(*child)
                        .and_modify(|p| *p = None)
                        .or_insert(Some((owner, hirsel_tir::RvalueId(r as u32))));
                }
            }
            for instruction in body.blocks.iter().flat_map(|b| &b.instructions) {
                let hirsel_tir::InstructionKind::Assign { destination, value } = instruction.kind
                else {
                    continue;
                };
                let d = &body.places[destination.0 as usize];
                if !d.projection.is_empty() || !generic_function(&definition.types, d.ty) {
                    continue;
                }
                let source = match &body.rvalues[value.0 as usize] {
                    Rvalue::Closure { .. } => Some(ProducerSource::Literal(Producer {
                        owner,
                        value,
                        ty: d.ty,
                        empty: false,
                    })),
                    Rvalue::Use(operand) => {
                        let p = &body.places[operand.place.0 as usize];
                        p.projection
                            .is_empty()
                            .then_some(ProducerSource::Alias(owner, p.root))
                    }
                    _ => None,
                };
                sources
                    .entry((owner, d.root))
                    .and_modify(|p| *p = None)
                    .or_insert(source);
            }
        }
        for (b, body) in definition.bodies.iter().enumerate() {
            let owner = BodyId(b as u32);
            let Some(Some((parent, value))) = creators.get(&owner).copied() else {
                continue;
            };
            let parent_body = &definition.bodies[parent.0 as usize];
            let Rvalue::Closure { captures, .. } = &parent_body.rvalues[value.0 as usize] else {
                unreachable!();
            };
            for ((local, row), operand) in body
                .locals
                .iter()
                .enumerate()
                .filter(|(_, l)| l.storage == Storage::Capture)
                .zip(captures)
            {
                if !generic_function(&definition.types, row.ty) {
                    continue;
                }
                let p = &parent_body.places[operand.place.0 as usize];
                sources.insert(
                    (owner, hirsel_tir::LocalId(local as u32)),
                    p.projection
                        .is_empty()
                        .then_some(ProducerSource::Alias(parent, p.root)),
                );
            }
        }
        let mut resolved = HashMap::new();
        for &start in sources.keys() {
            let mut at = start;
            let mut visited = std::collections::HashSet::new();
            let result = loop {
                if let Some(&result) = resolved.get(&at) {
                    break result;
                }
                if !visited.insert(at) {
                    break None;
                }
                match sources.get(&at).copied().flatten() {
                    Some(ProducerSource::Literal(producer)) => break Some(producer),
                    Some(ProducerSource::Alias(body, local)) => at = (body, local),
                    None => break None,
                }
            };
            for key in visited {
                resolved.insert(key, result);
            }
        }
        // An environment containing only other static generic names needs
        // no runtime data either. Propagate ordinary-data requirements over
        // reverse edges; cycles of function names alone remain empty.
        let literals: HashMap<_, _> = resolved
            .values()
            .copied()
            .flatten()
            .map(|p| ((p.owner, p.value), p))
            .collect();
        let mut dependents: HashMap<_, Vec<_>> = HashMap::new();
        let mut nonempty = std::collections::HashSet::new();
        let mut queue = VecDeque::new();
        for (&key, producer) in &literals {
            let body = &definition.bodies[producer.owner.0 as usize];
            let Rvalue::Closure { captures, .. } = &body.rvalues[producer.value.0 as usize] else {
                unreachable!();
            };
            for operand in captures {
                let place = &body.places[operand.place.0 as usize];
                let dependency = generic_function(&definition.types, place.ty)
                    .then(|| {
                        resolved
                            .get(&(producer.owner, place.root))
                            .copied()
                            .flatten()
                    })
                    .flatten();
                if let Some(p) = dependency {
                    dependents.entry((p.owner, p.value)).or_default().push(key);
                } else if nonempty.insert(key) {
                    queue.push_back(key);
                }
            }
        }
        while let Some(key) = queue.pop_front() {
            for &dependent in dependents.get(&key).into_iter().flatten() {
                if nonempty.insert(dependent) {
                    queue.push_back(dependent);
                }
            }
        }
        for ((body, local), mut producer) in resolved {
            if let Some(p) = &mut producer {
                p.empty = !nonempty.contains(&(p.owner, p.value));
            }
            out.insert((path.clone(), body, local), producer);
        }
    }
    out
}

/// Resolve the declaration first, then select its lexical instance. The call
/// argument actuals and the producer's enclosing actuals remain separate.
fn nested_function(
    instances: &Instances,
    mut instance: InstanceId,
    local: hirsel_tir::LocalId,
) -> Option<(InstanceId, BodyId, TypeId, &[hirsel_tir::Operand], bool)> {
    let view = instances.view(instance);
    let producer = instances
        .producers
        .get(&(view.definition.path.clone(), view.body, local))
        .copied()
        .flatten()?;
    loop {
        let view = instances.view(instance);
        if view.body == producer.owner {
            let body = &view.definition.bodies[producer.owner.0 as usize];
            let Rvalue::Closure { body, captures, .. } = &body.rvalues[producer.value.0 as usize]
            else {
                return None;
            };
            return Some((instance, *body, producer.ty, captures, producer.empty));
        }
        let InstanceRole::Closure { parent } = instances.roles[instance.index()] else {
            return None;
        };
        instance = parent;
    }
}

/// The non-generic `Function` row of generic function row `row` at
/// `arguments`: its parameters, result and error, whose generic references
/// are `Bound` at depth 0 inside it, substituted.
fn instantiate_row(
    store: &mut TypeStore,
    row: TypeId,
    arguments: &[GenericArgument],
) -> Result<TypeId, GraphError> {
    let source = store.clone();
    let Some(Type {
        kind: TypeKind::Function(function),
        modes,
        ..
    }) = source.get(row)
    else {
        return Ok(row);
    };
    let authorities = vec![AuthoritySummary::Unknown; arguments.len()];
    let substitution = hirsel_types::Substitution {
        binder: Some(GenericBinder::Bound(0)),
        arguments,
        authorities: &authorities,
        ..Default::default()
    };
    let mut one = |ty: TypeId| hirsel_types::substitute_type(&source, ty, store, substitution);
    let mut parameters = Vec::new();
    for parameter in function.parameters.iter() {
        let mut p = parameter.clone();
        p.ty = one(parameter.ty)?;
        parameters.push(p);
    }
    let result = one(function.result)?;
    let error = match function.error {
        Some(e) => Some(one(e)?),
        None => None,
    };
    Ok(store.push(Type {
        modes: modes.clone(),
        ..Type::with_kind(TypeKind::Function(FunctionType {
            state_promises: Default::default(),
            generics: Vec::new().into(),
            parameters: parameters.into(),
            result,
            error,
            allocation: function.allocation,
            authority: AuthoritySummary::Unknown,
            uses: function.uses.clone(),
            contracts: Vec::new().into(),
            incomplete: false,
            foreign: function.foreign.clone(),
        }))
    }))
}

/// The lexical binders whose generics `arguments` (rows of `types`) name.
fn body_scopes(types: &TypeStore, arguments: &[GenericArgument]) -> Vec<NameSite> {
    let mut scopes = Vec::new();
    let size = |s: &Size, scopes: &mut Vec<NameSite>| {
        if let Size::Parameter(BinderRef::Body { scope, .. }) = s
            && !scopes.contains(scope)
        {
            scopes.push(*scope);
        }
    };
    let mut todo = Vec::new();
    for argument in arguments {
        match argument {
            GenericArgument::Type(t) => todo.push(*t),
            GenericArgument::Size(s) => size(s, &mut scopes),
        }
    }
    let mut seen = std::collections::BTreeSet::new();
    while let Some(ty) = todo.pop() {
        if !seen.insert(ty) {
            continue;
        }
        let Some(row) = types.get(ty) else { continue };
        match &row.kind {
            TypeKind::Parameter(BinderRef::Body { scope, .. }) => {
                if !scopes.contains(scope) {
                    scopes.push(*scope);
                }
            }
            TypeKind::Tuple(ts) => todo.extend(ts.iter().copied()),
            TypeKind::Record(fs) => todo.extend(fs.iter().map(|f| f.ty)),
            TypeKind::Variant(vs) => {
                todo.extend(vs.iter().flat_map(|v| v.fields.iter().map(|(_, t)| *t)))
            }
            TypeKind::Named { arguments, .. } | TypeKind::Builtin { arguments, .. } => {
                for argument in arguments.iter() {
                    match argument {
                        GenericArgument::Type(t) => todo.push(*t),
                        GenericArgument::Size(s) => size(s, &mut scopes),
                    }
                }
            }
            TypeKind::Box(t) => todo.push(*t),
            TypeKind::Function(f) => {
                todo.extend(f.parameters.iter().map(|p| p.ty));
                todo.push(f.result);
                todo.extend(f.error);
            }
            _ => {}
        }
    }
    scopes
}

fn taken_capture_locals(body: &hirsel_tir::Body) -> BTreeSet<u32> {
    let ordinals: BTreeMap<usize, u32> = body
        .locals
        .iter()
        .enumerate()
        .filter(|(_, l)| l.storage == Storage::Capture)
        .enumerate()
        .map(|(ordinal, (local, _))| (local, ordinal as u32))
        .collect();
    let mut out = BTreeSet::new();
    let mut take = |o: &hirsel_tir::Operand| {
        if o.usage == hirsel_types::UseKind::Take {
            if let Some(&ordinal) = ordinals.get(&(body.places[o.place.0 as usize].root.0 as usize))
            {
                out.insert(ordinal);
            }
        }
    };
    for value in &body.rvalues {
        match value {
            Rvalue::Use(o) | Rvalue::Unary { value: o, .. } => take(o),
            Rvalue::Binary { left, right, .. } => {
                take(left);
                take(right);
            }
            Rvalue::Construct {
                constructor,
                fields,
                ..
            } => {
                if let hirsel_tir::Constructor::SortedMap { by } = constructor {
                    take(by);
                }
                for f in fields {
                    take(&f.value);
                }
            }
            Rvalue::Closure { captures, .. } => {
                for o in captures {
                    take(o);
                }
            }
            _ => {}
        }
    }
    for block in &body.blocks {
        for i in &block.instructions {
            match &i.kind {
                hirsel_tir::InstructionKind::Replace { value, .. }
                | hirsel_tir::InstructionKind::Snapshot { value, .. } => take(value),
                _ => {}
            }
        }
        match &block.terminator {
            hirsel_tir::Terminator::Call { arguments, .. } => {
                for a in arguments {
                    take(&a.value);
                }
            }
            hirsel_tir::Terminator::Return { value, .. } => take(value),
            _ => {}
        }
    }
    out
}

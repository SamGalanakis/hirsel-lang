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
    ActualBinder, CallableConvention, CaptureView, InstanceDeclaration, InstanceKey,
    InstanceKeyError, InstanceView, ParameterView, Substitution,
};
use hirsel_layout::layout::{LayoutBuilder, Owners};
use hirsel_lir::abi_types::{CallMultiplicity, CallableShape, FnAbiId};
use hirsel_lir::ir::CaptureUse;
use hirsel_lir::layout::LayoutError;
use hirsel_lir::layout_types::LayoutId;
use hirsel_lir::stage::InputIdentity;
use hirsel_span::Span;
use hirsel_syntax::{ContentHash, DefId, DefPath};
use hirsel_tir::{BodyId, DefinitionRef, Rvalue, Storage};
use hirsel_types::{
    AuthoritySummary, BinderRef, FunctionType, GenericArgument, GenericBinder, GraphError,
    Multiplicity, SignatureKind, SignatureRef, SiteRef, Size, SizeParamId, SizeParameterRef, Type,
    TypeId, TypeInterner, TypeKind, TypeMemo, TypeParamId, TypeParameterRef, UseKind,
};
use std::collections::BTreeSet;
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::sync::Arc;

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

/// One GEN-5 requirement. Argument `i` binds `BinderRef::Body { scope:
/// binder, parameter: i }` of `callee`. Free `Body` references name the
/// owner's parameters, and when `owner_binder` is set (arguments read from the
/// owner's signature) so do `Bound` references at depth 0.
#[derive(Clone, Debug)]
pub struct Requirement {
    pub owner: DefPath,
    pub owner_binder: Option<SiteRef>,
    pub site: Span,
    pub kind: RequirementKind,
    pub callee: DefPath,
    pub binder: SiteRef,
    pub arguments: Vec<GenericArgument>,
}

/// Everything native passes read from a validated `NativeInput`, as plain
/// data. The driver fills it from `ValidatedNativeInput`; this crate
/// never depends on the driver.
#[derive(Clone, Debug)]
pub struct Program {
    pub input: InputIdentity,
    /// The session interner: every TypeId of the program is one of its rows.
    pub types: Arc<TypeInterner>,
    /// Structural substitution memo shared by every native pass.
    pub memo: Arc<TypeMemo>,
    pub definitions: BTreeMap<DefPath, DefinitionRef>,
    pub signatures: BTreeMap<DefPath, SignatureRef>,
    /// Implementation revision of each executable definition's source owner.
    pub revisions: BTreeMap<DefPath, ContentHash>,
    /// The lexical binder of each source definition's generics.
    pub binders: BTreeMap<DefPath, SiteRef>,
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
        let mut signatures = hirsel_validate::SignatureMap::new(self.types.clone());
        signatures.extend(
            self.signatures
                .iter()
                .map(|(path, signature)| (path, (**signature).clone())),
        );
        let invocations = self
            .definitions
            .iter()
            .map(|(path, definition)| {
                let sites = crate::invocations::source_rows(
                    &self.types,
                    definition,
                    &signatures,
                    &self.operations,
                )
                .expect("native input already passed independent TIR validation");
                (path.clone(), sites)
            })
            .collect();
        hirsel_layout::operation::OperationRegistry {
            seal_sources: Default::default(),
            receipt_sources: Default::default(),
            pretransfer_adapters: Vec::new(),
            pretransfer_actuals: Vec::new(),
            invocation_bodies: Vec::new(),
            invocations,
            type_declarations: self
                .signatures
                .values()
                .filter(|s| matches!(s.kind, SignatureKind::Type(_)))
                .map(|s| (s.definition, s.clone()))
                .collect(),
            types: self.types.clone(),
            signatures: self
                .operations
                .values()
                .filter_map(|op| {
                    self.signatures
                        .get(&op.definition)
                        .map(|s| (op.clone(), s.clone()))
                })
                .collect(),
            owners: crate::library::owners(&self.types, &self.signatures).over(&self.owners),
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
    Body(hirsel_layout::instance::InstanceBodyError),
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

impl From<hirsel_layout::instance::InstanceBodyError> for InstanceError {
    fn from(error: hirsel_layout::instance::InstanceBodyError) -> Self {
        Self::Body(error)
    }
}

/// A native CT-5 lookup, including a declaration-absence read. Arguments use
/// the existing canonical CT-5 spellings, so rechecking needs no TypeId decode.
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, serde::Serialize, serde::Deserialize)]
pub struct SelectionRead {
    pub declaration: DefPath,
    pub arguments: Option<Vec<String>>,
    pub generated: bool,
    pub selected: Option<DefPath>,
}
impl SelectionRead {
    pub fn current(&self, bodies: &hirsel_tir::InstanceBodies) -> Self {
        let declaration = DefId::intern(&self.declaration);
        let generated = bodies.declarations.contains(&declaration);
        let selected = if generated {
            self.arguments.as_ref().and_then(|arguments| {
                bodies
                    .instances
                    .get(&declaration)
                    .and_then(|rows| {
                        rows.iter().find(|(key, _)| {
                            key.spellings()
                                .iter()
                                .copied()
                                .eq(arguments.iter().map(String::as_str))
                        })
                    })
                    .map(|(_, row)| row.definition.path().clone())
            })
        } else {
            None
        };
        Self {
            generated,
            selected,
            ..self.clone()
        }
    }
    pub fn valid(&self) -> bool {
        if self.generated {
            self.arguments.is_some() && self.selected.is_some()
        } else {
            self.arguments.is_none() && self.selected.is_none()
        }
    }
}

/// The expanded program: instance views in discovery order, their roles, the
/// `FnAbi` of each and a key index.
pub struct Instances {
    /// The session interner every view's TypeIds are rows of.
    pub types: Arc<TypeInterner>,
    pub memo: Arc<TypeMemo>,
    pub views: Vec<InstanceView>,
    pub roles: Vec<InstanceRole>,
    /// The instance's callable type, a `Function` row.
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
    selection_reads: std::sync::Mutex<std::collections::BTreeSet<SelectionRead>>,
    by_key: HashMap<InstanceKey, InstanceId>,
    payload_sources: HashMap<InstanceId, Vec<hirsel_lir::sealed_payload_source::SourcePayload>>,
    receipt_sources: HashMap<InstanceId, hirsel_lir::sealed_payload_source::ReceiptSource>,
    lexical_keys: HashMap<InstanceId, InstanceKey>,
    function_entries: BTreeMap<DefPath, BodyId>,
    producers: BTreeMap<DefPath, HashMap<(BodyId, hirsel_tir::LocalId), Option<Producer>>>,
}

/// One ordinary data parameter of a statically bound nested function.
#[derive(Clone, Debug)]
pub struct DirectCapture {
    pub local: hirsel_tir::LocalId,
    pub layout: LayoutId,
}

impl Instances {
    /// An empty demand registry owns no source bodies or native Values.
    pub fn new(types: Arc<TypeInterner>, memo: Arc<TypeMemo>) -> Self {
        Self {
            types,
            memo,
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
            selection_reads: Default::default(),
            by_key: HashMap::new(),
            payload_sources: HashMap::new(),
            receipt_sources: HashMap::new(),
            lexical_keys: HashMap::new(),
            function_entries: BTreeMap::new(),
            producers: BTreeMap::new(),
        }
    }

    pub fn selection_reads(&self) -> Vec<SelectionRead> {
        self.selection_reads
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .cloned()
            .collect()
    }

    /// The sole CT-5 resolution used by source discovery and native import.
    /// Successful ordinary and generated lookups record their exact read.
    pub fn resolve_generated(
        &self,
        program: &Program,
        path: DefId,
        arguments: &[GenericArgument],
    ) -> Result<Option<DefId>, hirsel_types::QueryError> {
        let selected = program
            .instance_bodies
            .resolve(path, &program.types, arguments)?;
        let generated = program.instance_bodies.declarations.contains(&path);
        let arguments = if generated {
            Some(
                hirsel_tir::InstanceBodies::key(&program.types, arguments)
                    .expect("a successful CT-5 resolution has a closed key")
                    .spellings()
                    .into_iter()
                    .map(str::to_owned)
                    .collect(),
            )
        } else {
            None
        };
        self.selection_reads
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(SelectionRead {
                declaration: path.path().clone(),
                arguments,
                generated,
                selected: selected.map(|path| path.path().clone()),
            });
        Ok(selected)
    }

    /// Register a source callable from its compact declaration and concrete
    /// actuals. Only a subsequent miss may attach its demanded source body.
    pub fn register_function(
        &mut self,
        program: &Program,
        declaration: Arc<InstanceDeclaration>,
        entry: BodyId,
        actuals: Actuals,
        layouts: &mut dyn crate::library::builder::Layouts,
    ) -> Result<(InstanceId, bool), InstanceError> {
        let path = &declaration.definition;
        let signature = program
            .signatures
            .get(path)
            .ok_or_else(|| InstanceError::MissingSignature(path.clone()))?;
        let SignatureKind::Function(function) = &signature.kind else {
            return Err(InstanceError::Malformed(format!(
                "{path:?} is called but is not a function"
            )));
        };
        if !Arc::ptr_eq(&self.types, &program.types)
            || declaration.signature != signature.fingerprint
            || entry.0 >= declaration.body_count
        {
            return Err(InstanceError::Malformed(format!(
                "{path:?}: stale or foreign instance declaration"
            )));
        }
        let key = key_of(&self.types, path, entry, &actuals, None, None)?;
        if let Some(id) = self.get(&key) {
            return Ok((id, false));
        }
        let callable = instantiate_function(
            &self.types,
            &self.memo,
            function,
            hirsel_types::Modes::global(),
            &actuals,
        )
        .map_err(|e| InstanceError::Substitution(path.clone(), e))?;
        let substitution = actuals.substitution(path)?;
        let parameters = parameter_views(&self.types, callable);
        let view = InstanceView::from_declaration(
            key,
            declaration,
            entry,
            CallableShape::Definition,
            substitution,
            parameters,
            Vec::new(),
            false,
        );
        self.push(
            layouts,
            view,
            InstanceRole::Function,
            callable,
            actuals,
            CallableShape::Definition,
            None,
        )
    }

    /// Register a source finalizer from its compact self-parameter contract.
    /// This is the same construction used by nominal discovery, without a TIR
    /// body or native Value. Only a miss attaches the independently checked body.
    pub fn register_finalizer(
        &mut self,
        program: &Program,
        source: &SourceDeclaration,
        actuals: Actuals,
        layouts: &mut dyn crate::library::builder::Layouts,
    ) -> Result<(InstanceId, bool), InstanceError> {
        if !Arc::ptr_eq(&self.types, &program.types)
            || source.entry.0 >= source.declaration.body_count
        {
            return Err(InstanceError::Malformed(
                "foreign finalizer declaration".into(),
            ));
        }
        let finalizer = &source.declaration.definition;
        let key = key_of(
            &program.types,
            finalizer,
            source.entry,
            &actuals,
            None,
            None,
        )?;
        if let Some(id) = self.get(&key) {
            return Ok((id, false));
        }
        // The independently supplied source contract gives the finalizer's
        // self parameter without demanding that implementation body.
        let types = &*program.types;
        let parameter = source
            .entry_parameters
            .first()
            .ok_or_else(|| InstanceError::Malformed(format!("{finalizer:?} has no self")))?;
        let self_ty = substitute(types, &program.memo, *parameter, &actuals, false)
            .map_err(|e| InstanceError::Substitution(finalizer.clone(), e))?;
        let unit = types.intern(Type::unit());
        let callable = types.intern(Type::with_kind(TypeKind::Function(FunctionType {
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
        let view = InstanceView::from_declaration(
            key,
            source.declaration.clone(),
            source.entry,
            CallableShape::Definition,
            actuals.substitution(finalizer)?,
            parameter_views(types, callable),
            Vec::new(),
            false,
        );
        self.push(
            layouts,
            view,
            InstanceRole::Finalizer,
            callable,
            actuals,
            CallableShape::Definition,
            None,
        )
    }

    /// Attach one demanded implementation and index its lexical producers once
    /// per source owner. Other source bodies are neither traversed nor retained.
    pub fn attach_definition(
        &mut self,
        id: InstanceId,
        definition: DefinitionRef,
    ) -> Result<(), InstanceError> {
        self.views[id.index()].attach_definition(definition.clone())?;
        let path = definition.path.path();
        if !self.producers.contains_key(path) {
            let producers = lexical_producers(&self.types, &definition);
            self.producers.insert(path.clone(), producers);
        }
        Ok(())
    }

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
    /// Source discovery keeps its lexical address even when native transport
    /// selects a distinct receipt-aware implementation of that same body.
    pub fn lexical_key(&self, id: InstanceId) -> &InstanceKey {
        self.lexical_keys.get(&id).unwrap_or(&self.view(id).key)
    }

    pub fn receipt_source(
        &self,
        id: InstanceId,
    ) -> Option<&hirsel_lir::sealed_payload_source::ReceiptSource> {
        self.receipt_sources.get(&id)
    }

    /// Match source-only closure identity to its independently reconstructed native
    /// convention and environment. Nothing here grants executable C custody.
    pub fn receipt_lexical_matches(
        &self,
        id: InstanceId,
        request: &crate::sealed_source::contract::registry::Request,
        layouts: &mut LayoutBuilder<'_>,
    ) -> Result<bool, InstanceError> {
        if self.lexical_key(id) == request.ordinary_key() {
            return Ok(true);
        }
        let InstanceRole::Closure { parent } = self.roles[id.index()] else {
            return Ok(false);
        };
        let entry = request.declaration().entry();
        let crate::sealed_source::SourceEntryKind::Closure { producer, .. } = entry.kind() else {
            return Ok(false);
        };
        let actuals = &self.actuals[id.index()];
        if !actuals.arguments.is_empty() || !actuals.outer.is_empty() {
            return Err(InstanceError::Malformed(
                "receipt closure needs complete enclosing actuals".into(),
            ));
        }
        let lexical = self.lexical_key(id);
        if lexical.definition() != request.ordinary_key().definition()
            || lexical.body() != request.ordinary_key().body()
            || self.view(parent).body.0 != producer.body
        {
            return Ok(false);
        }
        let definition = self.view(id).definition()?.clone();
        let path = definition.path.path();
        let source_base = key_of(&self.types, path, self.view(id).body, actuals, None, None)?;
        if &source_base != request.ordinary_key() {
            return Ok(false);
        }
        let (expected, environment) =
            receipt_closure_binding(&self.types, &definition, entry, layouts)?;
        let Some(actual_environment) = self.environments[id.index()] else {
            return Ok(false);
        };
        if hirsel_lir::LayoutKeys::rooted(layouts.table(), actual_environment).0
            != hirsel_lir::LayoutKeys::rooted(layouts.table(), environment).0
        {
            return Ok(false);
        }
        Ok(&expected == lexical)
    }

    /// Bind an independently selected source request after lexical discovery.
    /// This registers producer inputs; only strict C can accept their custody.
    pub fn bind_receipt_request(
        &mut self,
        id: InstanceId,
        request: &crate::sealed_source::contract::registry::Request,
        layouts: &mut LayoutBuilder<'_>,
    ) -> Result<(), InstanceError> {
        if !self.receipt_lexical_matches(id, request, layouts)?
            || self.view(id).body.0 != request.declaration().entry().reference().body
        {
            return Err(InstanceError::Malformed(
                "receipt request differs from lexical source".into(),
            ));
        }
        let Some(source) = request
            .receipt_source()
            .map_err(|e| InstanceError::Malformed(format!("receipt source: {e:?}")))?
        else {
            return Ok(());
        };
        if let Some(previous) = self.receipt_sources.get(&id) {
            return if previous == &source {
                Ok(())
            } else {
                Err(InstanceError::Malformed(
                    "conflicting receipt request".into(),
                ))
            };
        }
        let key = request
            .aware_key()
            .ok_or_else(|| InstanceError::Malformed("missing aware key".into()))?
            .clone();
        if self.by_key.get(&key).is_some_and(|other| *other != id) {
            return Err(InstanceError::Malformed(
                "receipt key already names another instance".into(),
            ));
        }
        let abi = layouts
            .receipt_fn_abi(self.abis[id.index()], &source.contract)
            .map_err(|e| InstanceError::Layout(key.definition().clone(), e))?;
        self.lexical_keys.insert(id, self.view(id).key.clone());
        self.views[id.index()].key = key.clone();
        self.abis[id.index()] = abi;
        self.by_key.insert(key, id);
        self.receipt_sources.insert(id, source);
        Ok(())
    }

    pub fn bind_payload_sources(
        &mut self,
        id: InstanceId,
        payloads: Vec<hirsel_lir::sealed_payload_source::SourcePayload>,
    ) {
        self.payload_sources.insert(id, payloads);
    }

    pub(crate) fn payload_sources(
        &self,
        id: InstanceId,
    ) -> &[hirsel_lir::sealed_payload_source::SourcePayload] {
        self.payload_sources.get(&id).map_or(&[], Vec::as_slice)
    }

    pub(crate) fn register_library_receipt(
        &mut self,
        id: InstanceId,
        source: hirsel_lir::sealed_payload_source::ReceiptSource,
    ) {
        self.receipt_sources.insert(id, source);
    }

    /// Source callable of a proof owner, never an appended Value-unit index.
    pub(crate) fn callable(&self, id: InstanceId) -> &FunctionType {
        match self.types.get(self.callables[id.index()]) {
            Some(Type {
                kind: TypeKind::Function(f),
                ..
            }) => f,
            _ => panic!("instance {id:?} has no callable Function row"),
        }
    }
    pub fn ids(&self) -> impl Iterator<Item = InstanceId> + '_ {
        (0..self.views.len() as u32).map(InstanceId)
    }
    /// The instance of `definition`'s entry body with these actuals.
    /// Whether an ordinary source callable has a registered entry contract.
    /// This lookup never observes an implementation body.
    pub fn has_function(&self, definition: &DefPath) -> bool {
        self.function_entries.contains_key(definition)
    }

    pub fn find(
        &self,
        program: &Program,
        definition: &DefPath,
        actuals: &Actuals,
    ) -> Result<InstanceId, InstanceError> {
        if !program.signatures.contains_key(definition) {
            return Err(InstanceError::MissingSignature(definition.clone()));
        }
        let entry = self
            .function_entries
            .get(definition)
            .copied()
            .ok_or_else(|| InstanceError::MissingInstance(definition.clone()))?;
        let key = key_of(&self.types, definition, entry, actuals, None, None)?;
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
            .fn_abi(&self.types, callable, shape)
            .map_err(|e| InstanceError::Layout(path, e))?;
        let capture_paths = view
            .captures
            .iter()
            .filter(|c| c.scoped)
            .map(|c| hirsel_lir::abi_types::ParameterPath {
                parameter: c.ordinal,
                fields: Vec::new(),
            })
            .collect::<Vec<_>>();
        let parameters = view
            .declaration()
            .scoped_parameters
            .get(view.body.0 as usize)
            .cloned()
            .unwrap_or_else(|| layouts.table().fn_abis[abi.0 as usize].nonretaining.clone());
        let abi = layouts
            .with_scoped_contract(abi, parameters, capture_paths)
            .map_err(|e| InstanceError::Layout(view.key.definition().clone(), e))?;
        let abi = if role.is_library() && view.key.definition().is_core("region.with") {
            let TypeKind::Function(function) = &self.types.row(callable).kind else {
                return Err(InstanceError::Malformed("region.with callable".into()));
            };
            let callback = function.parameters[1].ty;
            let nested = layouts
                .fn_abi(&self.types, callback, CallableShape::Closure)
                .map_err(|e| InstanceError::Layout(view.key.definition().clone(), e))?;
            let nested = layouts
                .with_scoped_contract(
                    nested,
                    vec![hirsel_lir::abi_types::ParameterPath {
                        parameter: 0,
                        fields: Vec::new(),
                    }],
                    Vec::new(),
                )
                .map_err(|e| InstanceError::Layout(view.key.definition().clone(), e))?;
            layouts
                .with_scoped_entry(
                    abi,
                    hirsel_lir::abi_types::ParameterPath {
                        parameter: 1,
                        fields: Vec::new(),
                    },
                    nested,
                )
                .map_err(|e| InstanceError::Layout(view.key.definition().clone(), e))?
        } else {
            abi
        };
        let abi = if role.is_library() || view.tracks_caller() {
            layouts
                .with_caller_site(abi)
                .map_err(|e| InstanceError::Layout(view.key.definition().clone(), e))?
        } else {
            abi
        };
        view.caller_destination = layouts.table().fn_abis[abi.0 as usize].destination;
        if role == InstanceRole::Function {
            if self
                .function_entries
                .get(view.key.definition())
                .is_some_and(|b| *b != view.body)
            {
                return Err(InstanceError::Malformed(
                    "function entry declaration changed".into(),
                ));
            }
            self.function_entries
                .insert(view.key.definition().clone(), view.body);
        }
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

/// Complete actuals of one binder: one argument per generic parameter, in
/// ordinal order. A generic nested function's instance (LIR schema revision 9)
/// also binds its enclosing functions' generics, which its body names: `outer`
/// lists each enclosing binder's arguments, innermost first.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct Actuals {
    pub binder: Option<SiteRef>,
    pub arguments: Vec<GenericArgument>,
    pub outer: Vec<(SiteRef, Vec<GenericArgument>)>,
}

impl Actuals {
    pub fn none() -> Self {
        Self {
            binder: None,
            arguments: Vec::new(),
            outer: Vec::new(),
        }
    }
    /// Every binder these actuals bind with its arguments, own first.
    pub fn binders(&self) -> impl Iterator<Item = (SiteRef, &[GenericArgument])> {
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
        Ok(Substitution { types, sizes })
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

/// Substitute `ty` (whose free generic references name `owner`'s binder),
/// replacing them by `owner`'s actuals. Authority summaries of replaced
/// parameters are conservatively unknown, the same on every path, so keys
/// computed by expansion and by import agree.
pub fn substitute(
    types: &TypeInterner,
    memo: &TypeMemo,
    ty: TypeId,
    owner: &Actuals,
    also_bound: bool,
) -> Result<TypeId, GraphError> {
    let authorities = vec![AuthoritySummary::Unknown; owner.arguments.len()];
    let body = hirsel_types::Substitution {
        binder: owner.binder.map(GenericBinder::Body),
        arguments: &owner.arguments,
        authorities: &authorities,
        ..Default::default()
    };
    let mut id = hirsel_types::substitute(types, memo, ty, &body)?;
    if also_bound && !owner.arguments.is_empty() {
        let bound = hirsel_types::Substitution {
            binder: Some(GenericBinder::Bound(0)),
            arguments: &owner.arguments,
            authorities: &authorities,
            ..Default::default()
        };
        id = hirsel_types::substitute(types, memo, id, &bound)?;
    }
    // An enclosing function's generics, which a nested function's body names.
    for (scope, arguments) in &owner.outer {
        let authorities = vec![AuthoritySummary::Unknown; arguments.len()];
        let enclosing = hirsel_types::Substitution {
            binder: Some(GenericBinder::Body(*scope)),
            arguments,
            authorities: &authorities,
            ..Default::default()
        };
        id = hirsel_types::substitute(types, memo, id, &enclosing)?;
    }
    Ok(id)
}

/// The callee actuals of a requirement or call site whose `arguments` name
/// `owner`'s binder.
pub fn callee_actuals(
    types: &TypeInterner,
    memo: &TypeMemo,
    callee_binder: Option<SiteRef>,
    arguments: &[GenericArgument],
    owner: &Actuals,
    also_bound: bool,
    path: &DefPath,
) -> Result<Actuals, InstanceError> {
    let mut out = Vec::new();
    for argument in arguments {
        out.push(match argument {
            GenericArgument::Type(ty) => GenericArgument::Type(
                substitute(types, memo, *ty, owner, also_bound)
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
        arguments: out,
        outer: Vec::new(),
    })
}

/// The canonical key of one instance.
pub fn key_of(
    types: &TypeInterner,
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
        types,
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
/// declaring function type) by `actuals`.
pub fn instantiate_function(
    types: &TypeInterner,
    memo: &TypeMemo,
    function: &FunctionType,
    modes: hirsel_types::Modes,
    actuals: &Actuals,
) -> Result<TypeId, GraphError> {
    let authorities = vec![AuthoritySummary::Unknown; actuals.arguments.len()];
    let substitution = hirsel_types::Substitution {
        binder: Some(GenericBinder::Bound(0)),
        arguments: &actuals.arguments,
        authorities: &authorities,
        ..Default::default()
    };
    let one = |ty: TypeId| hirsel_types::substitute(types, memo, ty, &substitution);
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
    Ok(types.intern(row))
}

/// Reconstruct the native lexical closure key from current original source.
/// No native candidate, source recipe or cached Value supplies its environment.
pub fn receipt_closure_binding(
    types: &TypeInterner,
    definition: &DefinitionRef,
    entry: &crate::sealed_source::SourceCallableEntry,
    layouts: &mut LayoutBuilder<'_>,
) -> Result<(InstanceKey, LayoutId), InstanceError> {
    let crate::sealed_source::SourceEntryKind::Closure { producer, .. } = entry.kind() else {
        return Err(InstanceError::Malformed(
            "receipt entry is not a closure".into(),
        ));
    };
    let path = definition.path.path();
    let body_id = BodyId(entry.reference().body);
    let source_base = key_of(types, path, body_id, &Actuals::none(), None, None)?;
    if source_base.digest() != entry.reference().instance {
        return Err(InstanceError::Malformed(
            "receipt closure source key differs".into(),
        ));
    }
    let input = crate::sealed_source::SourceInput::capture(
        entry.source_identity(),
        entry.implementation_revision(),
        definition.clone(),
    );
    let checked = input
        .closure_entry(
            &source_base,
            types,
            BodyId(producer.body),
            hirsel_tir::BlockId(producer.block),
            producer.statement,
            Ok,
        )
        .map_err(|e| InstanceError::Malformed(format!("receipt closure original entry: {e:?}")))?;
    if &checked != entry {
        return Err(InstanceError::Malformed(
            "receipt closure original entry differs".into(),
        ));
    }
    let parent_body = &definition.bodies[producer.body as usize];
    let hirsel_tir::InstructionKind::Assign { destination, .. } =
        parent_body.blocks[producer.block as usize].instructions[producer.statement as usize].kind
    else {
        return Err(InstanceError::Malformed(
            "receipt closure original entry differs".into(),
        ));
    };
    let callable = parent_body.places[destination.0 as usize].ty;
    let Some(convention) = convention(types, callable) else {
        return Err(InstanceError::Malformed(
            "receipt closure original entry differs".into(),
        ));
    };
    let body = &definition.bodies[body_id.0 as usize];
    let taken = taken_capture_locals(body);
    let scoped = source_scoped_captures(&definition, body_id);
    let mut shape = Vec::new();
    let mut movable = Vec::new();
    let mut scoped_paths = Vec::new();
    for capture in checked.captures() {
        let ty = body.locals[capture.local as usize].ty;
        if generic_function(types, ty) {
            return Err(InstanceError::Malformed(
                "receipt closure needs complete generic capture expansion".into(),
            ));
        }
        let layout = layouts
            .layout(types, ty)
            .map_err(|e| InstanceError::Layout(path.clone(), e))?;
        let usage = capture_use(capture.usage);
        shape.push((layout, capture_passing(usage)));
        if taken.contains(&capture.ordinal)
            && usage == CaptureUse::Take
            && layouts.record(layout).copy != hirsel_lir::layout_types::CopyRecipe::Bits
        {
            movable.push(capture.ordinal);
        }
        if scoped.contains(&capture.ordinal) {
            scoped_paths.push(hirsel_lir::abi_types::ParameterPath {
                parameter: capture.ordinal,
                fields: Vec::new(),
            });
        }
    }
    let environment = layouts
        .environment_data(path, body_id.0, &shape, &movable)
        .map_err(|e| InstanceError::Layout(path.clone(), e))?;
    let layout = hirsel_lir::LayoutKeys::rooted(layouts.table(), environment).0;
    let scoped = hirsel_lir::abi::scoped_contract_digest(&[], &scoped_paths);
    let environment_digest = ContentHash::of_parts(
        "hirsel.scoped-environment-instance.v1",
        [layout.0.as_slice(), scoped.0.as_slice()],
    );
    let expected = key_of(
        types,
        path,
        body_id,
        &Actuals::none(),
        Some(&convention),
        Some(environment_digest),
    )?;
    Ok((expected, environment))
}

pub fn convention(types: &TypeInterner, callable: TypeId) -> Option<CallableConvention> {
    let row = types.get(callable)?;
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

/// Source implementation contracts needed to register an instance before its
/// executable body is demanded. These observations carry no source V authority.
#[derive(Clone, Debug)]
pub struct SourceDeclaration {
    pub entry: BodyId,
    pub declaration: Arc<InstanceDeclaration>,
    pub entry_parameters: Vec<TypeId>,
}
impl SourceDeclaration {
    pub fn of(definition: &hirsel_tir::Definition) -> Result<Self, InstanceError> {
        let entry = definition
            .bodies
            .get(definition.entry.0 as usize)
            .ok_or_else(|| {
                InstanceError::Malformed("source entry outside body contracts".into())
            })?;
        let entry_parameters = entry
            .parameters
            .iter()
            .map(|id| {
                entry
                    .locals
                    .get(id.0 as usize)
                    .map(|local| local.ty)
                    .ok_or_else(|| {
                        InstanceError::Malformed("source parameter outside body contracts".into())
                    })
            })
            .collect::<Result<_, _>>()?;
        Ok(Self {
            entry: definition.entry,
            declaration: Arc::new(InstanceDeclaration::of(definition)),
            entry_parameters,
        })
    }
}

/// Immutable discovery inputs. Requirements belong to their source-owner
/// products; this registry contains no implementation bodies or native Values.
#[derive(Clone, Debug, Default)]
pub struct DiscoveryContext {
    pub sources: BTreeMap<DefPath, SourceDeclaration>,
    pub requirements: BTreeMap<DefPath, Vec<Requirement>>,
}
impl DiscoveryContext {
    pub fn of(program: &Program) -> Result<Self, InstanceError> {
        let mut context = Self::default();
        for (path, definition) in &program.definitions {
            context
                .sources
                .insert(path.clone(), SourceDeclaration::of(definition)?);
        }
        for requirement in &program.requirements {
            context
                .requirements
                .entry(requirement.owner.clone())
                .or_default()
                .push(requirement.clone());
        }
        Ok(context)
    }
}

struct Expander<'p, 'l, 'o> {
    program: &'p Program,
    layouts: &'l mut LayoutBuilder<'o>,
    out: Instances,
    context: &'p DiscoveryContext,
    /// Nominal instances already visited (recursive representations).
    nominals: std::collections::HashSet<InstanceKey>,
}

/// Discover precisely one demanded source instance. Callee registration reads
/// compact contracts only; their bodies remain unavailable until separate
/// demands. The returned IDs include source, closure and library discoveries.
pub fn discover_instance(
    program: &Program,
    context: &DiscoveryContext,
    layouts: &mut LayoutBuilder<'_>,
    mut instances: Instances,
    id: InstanceId,
    definition: DefinitionRef,
) -> Result<(Instances, Vec<InstanceId>), InstanceError> {
    let first = instances.len();
    instances.attach_definition(id, definition)?;
    let mut discovery = Expander {
        program,
        layouts,
        out: instances,
        context,
        nominals: Default::default(),
    };
    discovery.visit(id)?;
    let required = (first..discovery.out.len())
        .map(|i| InstanceId(i as u32))
        .collect();
    Ok((discovery.out, required))
}

/// Discover cleanup dependencies of a restored library instance from its
/// concrete type arguments and callable header. Library-generated temporaries
/// (such as channel.open's Option[T]) have no source body whose locals could
/// supply these dependencies. No implementation body is read here.
pub fn discover_library_instance(
    program: &Program,
    context: &DiscoveryContext,
    layouts: &mut LayoutBuilder<'_>,
    instances: Instances,
    id: InstanceId,
) -> Result<Instances, InstanceError> {
    if !instances
        .roles
        .get(id.index())
        .is_some_and(InstanceRole::is_library)
    {
        return Err(InstanceError::Malformed(
            "expected a library instance".into(),
        ));
    }
    let mut types = instances.actuals[id.index()]
        .arguments
        .iter()
        .filter_map(|argument| match argument {
            GenericArgument::Type(ty) => Some(*ty),
            _ => None,
        })
        .collect::<Vec<_>>();
    if let Some(callable) = program.types.get(instances.callables[id.index()])
        && let TypeKind::Function(callable) = &callable.kind
    {
        types.extend(callable.parameters.iter().map(|parameter| parameter.ty));
        types.push(callable.result);
        types.extend(callable.error);
    }
    let mut discovery = Expander {
        program,
        layouts,
        out: instances,
        context,
        nominals: Default::default(),
    };
    discovery.local_nominals(types)?;
    Ok(discovery.out)
}

/// Register source, nominal-finalizer and registry-constant roots using only
/// compact contracts. Every returned source instance starts without a body.
pub fn seed_instances(
    program: &Program,
    context: &DiscoveryContext,
    layouts: &mut LayoutBuilder<'_>,
) -> Result<(Instances, Vec<InstanceId>), InstanceError> {
    // Explicit scoped capture specialization erases only dead callable carriers
    // to Unit; reserve its ordinary layout before the table is sealed.
    layouts
        .layout(&program.types, program.types.intern(Type::unit()))
        .map_err(|e| InstanceError::Layout(DefPath::core("Unit"), e))?;
    let mut e = Expander {
        program,
        layouts,
        out: Instances::new(program.types.clone(), program.memo.clone()),
        context,
        nominals: Default::default(),
    };
    for (path, signature) in &program.signatures {
        if !context.sources.contains_key(path) {
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
    register_static_callables(
        program,
        context,
        e.layouts,
        &mut e.out,
        program.constants.values(),
    )?;
    let roots = e.out.ids().collect();
    Ok((e.out, roots))
}

/// Register library implementations named by selected materialized static values.
/// Like source function-reference discovery, this selects registry recipes only;
/// it does not import another instance or construct a Value body.
pub fn register_static_callables<'a>(
    program: &Program,
    context: &DiscoveryContext,
    layouts: &mut LayoutBuilder<'_>,
    instances: &mut Instances,
    constants: impl IntoIterator<Item = &'a hirsel_tir::Materialized>,
) -> Result<(), InstanceError> {
    for constant in constants {
        let mut pending = vec![&constant.root];
        while let Some(node) = pending.pop() {
            match &node.kind {
                hirsel_tir::ValueKind::FunctionReference {
                    definition,
                    arguments,
                } => {
                    let request = if let Some(body) = crate::library::body_index(definition) {
                        Some(crate::library::Request::operation(
                            program,
                            body,
                            definition,
                            arguments.clone(),
                        )?)
                    } else if program.operations.contains_key(definition)
                        && !context.sources.contains_key(definition)
                        && crate::library::value::forwards(definition)
                    {
                        Some(crate::library::Request::operation_value(
                            program,
                            definition,
                            arguments.clone(),
                        )?)
                    } else {
                        None
                    };
                    if let Some(request) = request {
                        crate::library::instantiate(layouts, instances, request)?;
                    }
                }
                hirsel_tir::ValueKind::Construct { fields, .. } => pending.extend(fields),
                _ => (),
            }
        }
    }
    Ok(())
}

/// Expand every instance the program executes. The current whole-program
/// caller uses the same registration and one-instance discovery operations;
/// query consumers demand these operations independently.
pub fn expand(
    program: &Program,
    layouts: &mut LayoutBuilder<'_>,
) -> Result<Instances, InstanceError> {
    let context = DiscoveryContext::of(program)?;
    let (mut instances, roots) = seed_instances(program, &context, layouts)?;
    let mut pending: VecDeque<_> = roots.into();
    while let Some(id) = pending.pop_front() {
        if instances.roles[id.index()].is_library() {
            continue;
        }
        let path = instances.view(id).key.definition();
        let definition = program
            .definitions
            .get(path)
            .ok_or_else(|| InstanceError::MissingDefinition(path.clone()))?
            .clone();
        let (next, required) =
            discover_instance(program, &context, layouts, instances, id, definition)?;
        instances = next;
        pending.extend(required);
    }
    Ok(instances)
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
            if let Ok(definition) = self.out.view(id).definition().cloned() {
                self.out.attach_definition(id, definition)?;
            }
        }
        Ok(id)
    }

    /// The entry-body instance of a function definition.
    fn function(&mut self, path: &DefPath, actuals: Actuals) -> Result<InstanceId, InstanceError> {
        let source = self
            .context
            .sources
            .get(path)
            .ok_or_else(|| InstanceError::MissingDefinition(path.clone()))?;
        let (id, _) = self.out.register_function(
            self.program,
            source.declaration.clone(),
            source.entry,
            actuals,
            self.layouts,
        )?;
        Ok(id)
    }

    /// A nominal instance: its finalizer, if it declares one, and the
    /// requirements of its representation.
    fn nominal(&mut self, path: &DefPath, actuals: Actuals) -> Result<(), InstanceError> {
        let signature = self.program.signatures.get(path).cloned();
        let declaration = match signature.as_deref().map(|signature| &signature.kind) {
            Some(SignatureKind::Type(declaration)) => Some(declaration),
            Some(_) => return Ok(()),
            // Private native nominals such as MapEntry have an independent
            // representation contract, but no source declaration signature.
            None if matches!(
                self.layouts.owners().nominals.get(&DefId::intern(path)),
                Some(hirsel_layout::layout::OwnerRepresentation::Defined(_))
            ) =>
            {
                None
            }
            None => return Err(InstanceError::MissingSignature(path.clone())),
        };
        let program = self.program;
        let visited = key_of(&program.types, path, BodyId(0), &actuals, None, None)?;
        if !self.nominals.insert(visited) {
            return Ok(());
        }
        self.follow(path, &actuals, None)?;
        // A library resource's drop body is a library body named by its
        // owner's native representation.
        if let Some(hirsel_layout::layout::OwnerRepresentation::Defined(record)) =
            self.layouts.owners().nominals.get(&DefId::intern(path))
            && let Some((finalizer, _)) = &record.finalizer
            && let Some(body) = crate::library::body_index(finalizer)
        {
            let request =
                crate::library::Request::finalizer(&program.types, body, finalizer, path, &actuals);
            crate::library::instantiate(self.layouts, &mut self.out, request)?;
            return Ok(());
        }
        let Some(finalizer) = declaration.and_then(|declaration| declaration.drop.as_ref()) else {
            return Ok(());
        };
        // A library finalizer is runtime code, not a TIR body.
        let Some(source) = self.context.sources.get(finalizer) else {
            return Ok(());
        };
        self.out
            .register_finalizer(program, source, actuals, self.layouts)?;
        Ok(())
    }

    /// Visit one instance: its owner's requirements, the closures its body
    /// creates and the generic nested functions it calls.
    fn visit(&mut self, id: InstanceId) -> Result<(), InstanceError> {
        // A library body is built, not read: it has no requirements here.
        if self.out.roles[id.index()].is_library() {
            return Ok(());
        }
        let types = &*self.program.types;
        let view = self.out.views[id.index()].clone();
        let actuals = self.out.actuals[id.index()].clone();
        let path = view.key.definition().clone();
        let body = &view.definition()?.bodies[view.body.0 as usize];
        let mut elided = std::collections::HashSet::new();
        for (local, row) in body.locals.iter().enumerate() {
            let local = hirsel_tir::LocalId(local as u32);
            if generic_function(types, row.ty)
                && nested_function(&self.out, id, local)?.is_some_and(|(_, _, _, _, empty)| empty)
            {
                elided.insert(local);
            }
        }
        self.out.elided.insert(id, elided);
        for (i, local) in body.locals.iter().enumerate() {
            let local_id = hirsel_tir::LocalId(i as u32);
            if generic_function(types, local.ty) && !self.out.elided[&id].contains(&local_id) {
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
        // A demanded source instance has no program-wide nominal seed. Its
        // concrete local types still require the same finalizers and private
        // representation dependencies as whole-program expansion. Discover
        // them from the original body, without loading their implementations.
        let local_types = body
            .locals
            .iter()
            .map(|local| {
                substitute(types, &self.program.memo, local.ty, &actuals, false)
                    .map_err(|e| InstanceError::Substitution(path.clone(), e))
            })
            .collect::<Result<Vec<_>, _>>()?;
        self.local_nominals(local_types)?;
        // Whole-program expansion seeded nongeneric callees in advance. A
        // demanded instance must register those ordinary calls/references from
        // its own body, without loading any callee implementation.
        for block in &body.blocks {
            if let hirsel_tir::Terminator::Call {
                callee: hirsel_tir::Callee::Definition { path, arguments },
                ..
            } = &block.terminator
            {
                self.source_reference(path, arguments, &actuals)?;
            }
        }
        for value in &body.rvalues {
            if let Rvalue::FunctionReference {
                definition,
                arguments,
            } = value
            {
                self.source_reference(definition, arguments, &actuals)?;
            }
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
                if generic_function(types, ty) {
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
            let (producer, closure, ty, captures, empty) = nested_function(&self.out, id, local)?
                .ok_or_else(|| {
                InstanceError::Malformed(format!(
                    "{path:?}: a generic callee with no lexical nested-function producer (spec #42)"
                ))
            })?;
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

    fn local_nominals(&mut self, mut pending: Vec<TypeId>) -> Result<(), InstanceError> {
        let program = self.program;
        let mut seen = std::collections::HashSet::new();
        while let Some(ty) = pending.pop() {
            if !seen.insert(ty) {
                continue;
            }
            let Some(row) = program.types.get(ty) else {
                return Err(InstanceError::Malformed(
                    "missing local nominal type".into(),
                ));
            };
            pending.extend(row.kind.value_parts());
            let TypeKind::Named {
                definition,
                arguments,
            } = &row.kind
            else {
                continue;
            };
            // Lexical generic callable carriers can contain unbound body
            // actuals. Their selected concrete instances discover those later.
            if hirsel_types::mentions_parameter(&program.types, ty) {
                continue;
            }
            let path = definition.path();
            let actuals = callee_actuals(
                &program.types,
                &program.memo,
                program.binders.get(path).copied(),
                arguments,
                &Actuals::none(),
                false,
                path,
            )?;
            self.nominal(path, actuals.clone())?;
            let shape = match self.layouts.owners().nominals.get(definition) {
                Some(hirsel_layout::layout::OwnerRepresentation::Defined(record)) => {
                    Some(record.private_shape)
                }
                _ => match &self.signature(path)?.kind {
                    SignatureKind::Type(declaration) => declaration.representation,
                    _ => None,
                },
            };
            if let Some(shape) = shape {
                pending.push(
                    substitute(&program.types, &program.memo, shape, &actuals, true)
                        .map_err(|e| InstanceError::Substitution(path.clone(), e))?,
                );
            }
        }
        Ok(())
    }

    fn source_reference(
        &mut self,
        path: &DefPath,
        arguments: &[GenericArgument],
        owner_actuals: &Actuals,
    ) -> Result<(), InstanceError> {
        let generated = self
            .program
            .instance_bodies
            .declarations
            .contains(&DefId::intern(path));
        if !generated && !self.context.sources.contains_key(path) {
            return Ok(());
        }
        let actuals = callee_actuals(
            &self.program.types,
            &self.program.memo,
            if generated {
                None
            } else {
                self.program.binders.get(path).copied()
            },
            arguments,
            owner_actuals,
            false,
            path,
        )?;
        let selected = self
            .out
            .resolve_generated(self.program, DefId::intern(path), &actuals.arguments)
            .map_err(|e| InstanceError::Malformed(format!("generated source reference: {e:?}")))?;
        if let Some(selected) = selected {
            self.function(selected.path(), Actuals::none())?;
        } else {
            self.function(path, actuals)?;
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
        let (parent, body, _, captures, empty) = nested_function(&self.out, id, local)?
            .ok_or_else(|| {
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
        let program = self.program;
        let types = &*program.types;
        let definition = parent_view.definition()?;
        let path = definition.path.path().clone();
        let body = definition
            .bodies
            .get(body_id.0 as usize)
            .ok_or_else(|| InstanceError::Malformed(format!("{path:?}: closure body")))?;
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
        let scoped_captures = source_scoped_captures(definition, body_id);
        let mut movable = Vec::new();
        let mut views = Vec::new();
        let mut shape = Vec::new();
        for (source_ordinal, (local, operand)) in capture_locals.iter().zip(captures).enumerate() {
            let source = parent_view.definition()?.bodies[parent_view.body.0 as usize].places
                [operand.place.0 as usize]
                .root;
            if generic_function(types, local.ty)
                && nested_function(&self.out, parent, source)?
                    .is_some_and(|(_, _, _, _, empty)| empty)
            {
                continue;
            }
            let ordinal = views.len();
            let usage = capture_use(operand.usage);
            views.push(CaptureView {
                // A lexical generic function lowers to its ordinary data
                // environment, with no callable entry in this field. Its
                // source CLO-1 membership cannot describe that physical data.
                scoped: scoped_captures.contains(&(source_ordinal as u32))
                    && !generic_function(types, local.ty),
                ordinal: ordinal as u32,
                usage,
            });
            let ty = substitute(types, &program.memo, local.ty, parent_actuals, false)
                .map_err(|e| InstanceError::Substitution(path.clone(), e))?;
            let layout = if generic_function(types, local.ty) {
                self.data_local(parent, source)?
            } else {
                self.layouts
                    .layout(types, ty)
                    .map_err(|e| InstanceError::Layout(path.clone(), e))?
            };
            // A taken capture holding more than bits moves out of the
            // environment under a live bit: a copy would leave the environment
            // a second holder until the call returns, so the taken value would
            // not be unique (a freeze or an in-place update needs it to be).
            if taken.contains(&(source_ordinal as u32))
                && usage == CaptureUse::Take
                && self.layouts.record(layout).copy != hirsel_lir::layout_types::CopyRecipe::Bits
            {
                movable.push(ordinal as u32);
            }
            shape.push((layout, capture_passing(usage)));
        }
        let environment = self
            .layouts
            .environment_data(&path, body_id.0, &shape, &movable)
            .map_err(|e| InstanceError::Layout(path.clone(), e))?;
        // A closure symbol survives import replay in a different program. Its
        // environment identity must include child content, never LayoutId numbering.
        let layout_digest = hirsel_lir::LayoutKeys::rooted(self.layouts.table(), environment).0;
        let paths: Vec<_> = views
            .iter()
            .filter(|c| c.scoped)
            .map(|c| hirsel_lir::abi_types::ParameterPath {
                parameter: c.ordinal,
                fields: Vec::new(),
            })
            .collect();
        let contract = hirsel_lir::abi::scoped_contract_digest(&[], &paths);
        let digest = ContentHash::of_parts(
            "hirsel.scoped-environment-instance.v1",
            [layout_digest.0.as_slice(), contract.0.as_slice()],
        );
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
        let program = self.program;
        let types = &*program.types;
        let definition = parent_view.definition()?.clone();
        let path = definition.path.path().clone();
        let scope = definition
            .bodies
            .get(body_id.0 as usize)
            .ok_or_else(|| InstanceError::Malformed(format!("{path:?}: closure body")))?
            .parameter_scope;
        let mut actuals = callee_actuals(
            types,
            &program.memo,
            Some(scope),
            arguments,
            caller_actuals,
            false,
            &path,
        )?;
        for (enclosing, arguments) in parent_actuals.binders() {
            actuals.outer.push((enclosing, arguments.to_vec()));
        }
        let generic = substitute(types, &program.memo, ty, parent_actuals, false)
            .map_err(|e| InstanceError::Substitution(path.clone(), e))?;
        let callable = instantiate_row(types, &program.memo, generic, &actuals.arguments)
            .map_err(|e| InstanceError::Substitution(path.clone(), e))?;
        let (environment, digest, views) = if empty {
            (None, None, Vec::new())
        } else {
            let (environment, digest, views) =
                self.environment(parent, parent_view, parent_actuals, body_id, captures)?;
            (Some(environment), Some(digest), views)
        };
        let convention = convention(types, callable).ok_or_else(|| {
            InstanceError::Malformed(format!("{path:?}: closure of a non-function type"))
        })?;
        let key = key_of(types, &path, body_id, &actuals, Some(&convention), digest)?;
        if let Some(id) = self.out.get(&key) {
            return Ok(id);
        }
        let shape = CallableShape::Definition;
        let view = InstanceView::from_definition(
            key,
            definition,
            body_id,
            shape,
            actuals.substitution(&path)?,
            parameter_views(types, callable),
            views,
            false,
        );
        let id = self.insert(
            view,
            InstanceRole::Closure { parent },
            callable,
            actuals,
            shape,
            None,
        )?;
        if let Some(environment) = environment {
            let body = &self.out.view(id).definition()?.bodies[body_id.0 as usize];
            let locals: Vec<_> = body
                .locals
                .iter()
                .enumerate()
                .filter(|(_, local)| local.storage == Storage::Capture)
                .map(|(i, _)| hirsel_tir::LocalId(i as u32))
                .collect();
            // Static generic names have no field in this record.
            let mut retained = Vec::new();
            for local in locals {
                if !nested_function(&self.out, id, local)?.is_some_and(|(_, _, _, _, empty)| empty)
                {
                    retained.push(local);
                }
            }
            let locals = retained;
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
        let program = self.program;
        let types = &*program.types;
        let definition = parent_view.definition()?.clone();
        let path = definition.path.path().clone();
        let actuals = parent_actuals.clone();
        let callable = substitute(types, &program.memo, ty, &actuals, false)
            .map_err(|e| InstanceError::Substitution(path.clone(), e))?;
        let (environment, digest, views) =
            self.environment(parent, parent_view, parent_actuals, body_id, captures)?;
        let convention = convention(types, callable).ok_or_else(|| {
            InstanceError::Malformed(format!("{path:?}: closure of a non-function type"))
        })?;
        let key = key_of(
            types,
            &path,
            body_id,
            &actuals,
            Some(&convention),
            Some(digest),
        )?;
        let view = InstanceView::from_definition(
            key,
            definition.clone(),
            body_id,
            CallableShape::Closure,
            actuals.substitution(&path)?,
            parameter_views(types, callable),
            views,
            false,
        );
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
        only: Option<SiteRef>,
    ) -> Result<(), InstanceError> {
        let program = self.program;
        let types = &*program.types;
        let requirements = self
            .context
            .requirements
            .get(owner)
            .map_or(&[][..], Vec::as_slice);
        for requirement in requirements {
            let named = body_scopes(types, &requirement.arguments);
            let bound: Vec<SiteRef> = actuals.binders().map(|(s, _)| s).collect();
            if !named.iter().all(|s| bound.contains(s))
                || only.is_some_and(|scope| !named.contains(&scope))
            {
                continue;
            }
            let callee = &requirement.callee;
            let callee_actuals = callee_actuals(
                types,
                &program.memo,
                Some(requirement.binder),
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
                            callee_actuals.arguments,
                        )?;
                        crate::library::instantiate(self.layouts, &mut self.out, request)?;
                    } else if self.context.sources.contains_key(callee) {
                        self.function(callee, callee_actuals)?;
                    }
                }
                SignatureKind::Type(_) => self.nominal(callee, callee_actuals)?,
                SignatureKind::Const { ty: _, .. }
                | SignatureKind::Family(_)
                | SignatureKind::Implementation(_) => {}
            }
        }
        Ok(())
    }
}

/// The identity parameter mapping of a callable, with its declared conventions.
pub fn parameter_views(types: &TypeInterner, callable: TypeId) -> Vec<ParameterView> {
    let Some(Type {
        kind: TypeKind::Function(f),
        ..
    }) = types.get(callable)
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
fn generic_function(types: &TypeInterner, ty: TypeId) -> bool {
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
    types: &TypeInterner,
    definition: &hirsel_tir::Definition,
) -> HashMap<(BodyId, hirsel_tir::LocalId), Option<Producer>> {
    let mut out = HashMap::new();
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
            if !d.projection.is_empty() || !generic_function(types, d.ty) {
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
            if !generic_function(types, row.ty) {
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
            let dependency = generic_function(types, place.ty)
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
        out.insert((body, local), producer);
    }
    out
}

/// Resolve the declaration first, then select its lexical instance. The call
/// argument actuals and the producer's enclosing actuals remain separate.
fn nested_function(
    instances: &Instances,
    mut instance: InstanceId,
    local: hirsel_tir::LocalId,
) -> Result<Option<(InstanceId, BodyId, TypeId, &[hirsel_tir::Operand], bool)>, InstanceError> {
    let view = instances.view(instance);
    let Some(producer) = instances
        .producers
        .get(view.key.definition())
        .and_then(|owner| owner.get(&(view.body, local)))
        .copied()
        .flatten()
    else {
        return Ok(None);
    };
    loop {
        let view = instances.view(instance);
        if view.body == producer.owner {
            let body = &view.definition()?.bodies[producer.owner.0 as usize];
            let Rvalue::Closure { body, captures, .. } = &body.rvalues[producer.value.0 as usize]
            else {
                return Ok(None);
            };
            return Ok(Some((
                instance,
                *body,
                producer.ty,
                captures,
                producer.empty,
            )));
        }
        let InstanceRole::Closure { parent } = instances.roles[instance.index()] else {
            return Ok(None);
        };
        instance = parent;
    }
}

/// The non-generic `Function` row of generic function row `row` at
/// `arguments`: its parameters, result and error, whose generic references
/// are `Bound` at depth 0 inside it, substituted.
fn instantiate_row(
    types: &TypeInterner,
    memo: &TypeMemo,
    row: TypeId,
    arguments: &[GenericArgument],
) -> Result<TypeId, GraphError> {
    let Some(Type {
        kind: TypeKind::Function(function),
        modes,
        ..
    }) = types.get(row)
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
    let one = |ty: TypeId| hirsel_types::substitute(types, memo, ty, &substitution);
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
    Ok(types.intern(Type {
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
pub(crate) fn body_scopes(types: &TypeInterner, arguments: &[GenericArgument]) -> Vec<SiteRef> {
    let mut scopes = Vec::new();
    let size = |s: &Size, scopes: &mut Vec<SiteRef>| {
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
    let mut seen = std::collections::HashSet::new();
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

/// Source membership for physical capture selectors. This compiler-produced
/// metadata is checked again by V and by C; it is never a validation receipt.
fn source_scoped_captures(
    definition: &hirsel_tir::Definition,
    selected: BodyId,
) -> std::collections::BTreeSet<u32> {
    use hirsel_tir::{InstructionKind, Rvalue, Storage};
    if definition.bodies.get(selected.0 as usize).is_none() {
        return Default::default();
    }
    let mut local: Vec<Vec<bool>> = definition
        .bodies
        .iter()
        .map(|b| vec![false; b.locals.len()])
        .collect();
    for (b, body) in definition.bodies.iter().enumerate() {
        for p in &body.scoped_parameters {
            if let Some(l) = body.parameters.get(p.parameter as usize) {
                local[b][l.0 as usize] = true;
            }
        }
    }
    loop {
        let mut changed = false;
        for (b, body) in definition.bodies.iter().enumerate() {
            for inst in body.blocks.iter().flat_map(|b| &b.instructions) {
                let InstructionKind::Assign { destination, value } = inst.kind else {
                    continue;
                };
                let value = &body.rvalues[value.0 as usize];
                let mut scoped = match value {
                    Rvalue::Closure {
                        context: hirsel_types::CaptureContext::Scoped,
                        ..
                    } => true,
                    Rvalue::Use(o) => local[b][body.places[o.place.0 as usize].root.0 as usize],
                    Rvalue::Construct { fields, .. } => fields
                        .iter()
                        .any(|f| local[b][body.places[f.value.place.0 as usize].root.0 as usize]),
                    _ => false,
                };
                if let Rvalue::Closure {
                    body: child,
                    captures,
                    ..
                } = value
                {
                    let slots = definition.bodies[child.0 as usize]
                        .locals
                        .iter()
                        .enumerate()
                        .filter(|(_, l)| l.storage == Storage::Capture);
                    for ((i, _), capture) in slots.zip(captures) {
                        let from = local[b][body.places[capture.place.0 as usize].root.0 as usize];
                        scoped |= from;
                        if from && !local[child.0 as usize][i] {
                            local[child.0 as usize][i] = true;
                            changed = true;
                        }
                    }
                }
                let to = body.places[destination.0 as usize].root.0 as usize;
                if scoped && !local[b][to] {
                    local[b][to] = true;
                    changed = true;
                }
            }
        }
        if !changed {
            break;
        }
    }
    definition.bodies[selected.0 as usize]
        .locals
        .iter()
        .enumerate()
        .filter(|(_, l)| l.storage == hirsel_tir::Storage::Capture)
        .enumerate()
        .filter_map(|(ordinal, (i, _))| local[selected.0 as usize][i].then_some(ordinal as u32))
        .collect()
}

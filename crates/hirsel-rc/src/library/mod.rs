//! The native core library in LIR.
//!
//! A generic registry operation is realised in one of two ways here. An
//! *inline lowering* replaces the call by instructions while its caller is
//! imported (`INLINE`); no call remains. A *library body* is one monomorphic
//! `LirUnit<Value>` per operation and complete type and size actuals
//! (`BODIES`), built with `builder` and then placed, counted, reused and
//! lowered to Ready exactly like a unit imported from TIR; its call is
//! `Callee::Direct`. Non-generic leaves stay runtime primitives and never
//! come through this module.
//!
//! Each family owns one module beside this file and one line in each list
//! below. No family edits another's module.
pub mod arena;
pub mod builder;
pub mod bytes;
pub mod channels;
pub mod codec;
pub mod collection;
pub mod cpu;
pub mod failure;
pub mod isolate;
pub mod iter;
pub mod keyed;
pub mod length;
pub mod list;
pub mod map;
pub mod mapped;
pub mod net;
pub mod owners;
mod packed_list_9;
pub mod runners;
pub mod sealed_region;
pub mod shared;
pub mod slots;
pub mod sorted;
pub mod sync;
pub mod tasks;
pub mod value;
pub mod views;

use crate::PassError;
use crate::import::ImportCx;
use crate::instance::{
    Actuals, InstanceError, InstanceId, InstanceRole, Instances, Program, instantiate_function,
};
use hirsel_layout::instance::{InstanceKey, InstanceView};
use hirsel_lir::abi_types::CallableShape;
use hirsel_lir::ir::LirUnit;
use hirsel_lir::layout_types::LayoutId;
use hirsel_lir::stage::Value;
use hirsel_syntax::{ContentHash, DefId, DefPath};
use hirsel_tir as tir;
use hirsel_types::{GenericArgument, NameSite, SignatureKind, SiteRef, Type, TypeId, TypeKind};

pub use builder::Cx;
pub use owners::{LibraryOwners, owners};

/// One library body: its operation path, how to build it, and what else
/// it requires.
pub struct LibraryBody {
    /// `core@1/...` operation path, or a `core@1/$native.<name>` path for a
    /// helper or finalizer no registry row names.
    pub path: fn() -> DefPath,
    /// Build the Value-stage unit for these actuals. `Cx` gives the layout
    /// table, the call's `FnAbi`, type and size actuals and symbol interning.
    pub build: fn(&mut Cx<'_>) -> Result<LirUnit<Value>, PassError>,
}

/// Every registered body. A family adds its rows in its own module and
/// one line here.
pub const BODIES: &[&LibraryBody] = &[
    &keyed::MAP_INSERT,
    &keyed::SET_INSERT,
    &keyed::MAP_GET,
    &keyed::SET_CONTAINS,
    &arena::WITH,
    &arena::BOUNDED,
    &arena::COPY_TO,
    &slots::INSERT,
    &slots::GET,
    &slots::REMOVE,
    &shared::FREEZE,
    &shared::THAW,
    &shared::WITH,
    &failure::CHECK,
    &bytes::APPEND_FROM_HISTORY,
    &bytes::REPLACE_RANGE,
    &collection::TRUNCATE,
    &collection::POP,
    &collection::REMOVE,
    &collection::SWAP,
    &collection::RESERVE,
    &list::PUSH,
    &list::PUSH_SPARE,
    &list::GROW,
    &list::LIST_OPEN,
    &list::ARRAY_OPEN,
    &list::INLINE_OPEN,
    &list::LIST_NEXT,
    &list::ARRAY_NEXT,
    &list::INLINE_NEXT,
    &list::CURSOR_DROP,
    &map::GET,
    &map::CONTAINS_MAP,
    &map::REMOVE_MAP,
    &map::REMOVE_SET,
    &map::CONTAINS,
    &map::INSERT_MAP,
    &map::UPSERT_MAP,
    &map::UPDATE_MAP,
    &map::UPDATE_VALUE_MAP,
    &map::INSERT_SET,
    &map::MAP_PROBE,
    &map::SET_PROBE,
    &map::SET_OPEN,
    &map::SET_NEXT,
    &map::MAP_OPEN,
    &map::MAP_NEXT,
    &map::MAP_DROP,
    &map::MAP_INDEX,
    &map::MAP_PREPARE,
    &map::MAP_WARM,
    &map::MAP_ADVANCE,
    &map::SET_INDEX,
    &map::SET_WARM,
    &map::SET_PREPARE,
    // sorted (lib-sorted): sorted_map keyed access, its cursor, the integer comparators.
    &sorted::INSERT,
    &sorted::GET,
    &sorted::REMOVE,
    &sorted::OPEN,
    &sorted::NEXT,
    &sorted::CURSOR_DROP,
    &sorted::U8,
    &sorted::U16,
    &sorted::U32,
    &sorted::U64,
    &sorted::U128,
    &sorted::I8,
    &sorted::I16,
    &sorted::I32,
    &sorted::I64,
    &sorted::I128,
    &iter::FROM_STEP,
    // net (net-t2): net.read_into and read_half.read_into over their fixed runtime entry.
    &net::READ_INTO,
    &net::HALF_READ_INTO,
    // tasks (tasks-native): structured concurrency over the scheduler's entries.
    &tasks::SCOPE,
    &tasks::SCOPE_UNTIL,
    &tasks::ASYNC,
    &tasks::AWAIT,
    &tasks::AWAIT_AT,
    &tasks::MAIN,
    &cpu::WORKER,
    &cpu::MAP,
    &tasks::CHECK_CANCELED,
    // mapped (fs-watch-mmap): mapped.with over its view and check entries.
    &mapped::WITH,
    &isolate::ISOLATE,
    // runners (native-runner-entry): runner calls over the runtime's runner entries.
    &runners::COMPUTE,
    &runners::CALL,
    &runners::SPAWN,
    &runners::GRANTS,
    &runners::MAIN,
    &runners::AWAIT,
    &channels::OPEN,
    &channels::SEND,
    &channels::RECEIVE,
    &sync::MUTEX_CREATE,
    &sync::MUTEX_WITH,
    &sync::ATOMIC_CREATE,
    &sync::ATOMIC_LOAD,
    &sync::ATOMIC_STORE,
    // value: an operation used as a function value.
    &value::FORWARD,
    // views (lib-views): none; its cursor protocols are runtime primitives.
    &iter::NEXT,
    &sealed_region::SEAL,
    &sealed_region::WITH,
];

/// Operation calls a family lowers inline at import. Called before the
/// generic operation-call path; returns false to decline.
pub struct InlineLowering {
    pub path: fn() -> DefPath,
    pub lower: fn(&mut ImportCx<'_>, &CallShape<'_>) -> R<bool>,
}

pub const INLINE: &[&InlineLowering] = &[
    &arena::HEAP,
    &bytes::FROM_LIST,
    &bytes::TEXT_FINISH,
    &slots::EMPTY,
    &list::FORGET,
    &map::EMPTY_MAP,
    &map::EMPTY_SET,
    &map::MAP_LEN,
    &map::PUBLIC_MAP_LEN,
    &map::PUBLIC_SET_LEN,
    // sorted (lib-sorted): sorted_map.empty/len, SortedMapOrder_len.
    &sorted::EMPTY,
    &sorted::LEN,
    &sorted::PUBLIC_LEN,
    &iter::FROM,
    &views::AS_BYTES,
    &views::SCALARS,
    &views::POSITIONS,
    // core (lib-core): list.len, array.len, string.byte_len.
    &length::BYTES_LEN,
    &length::LIST_LEN,
    &length::ARRAY_LEN,
    &length::STRING_BYTE_LEN,
    // codec (ser2-native): codec.encode and codec.decode.
    &codec::ENCODE,
    &codec::DECODE,
];

/// The result of an import hook.
pub type R<T> = Result<T, PassError>;

/// The operation call an inline lowering is asked to replace: the callee,
/// its generic actuals, the TIR
/// arguments in source order, and where the result goes.
pub struct CallShape<'a> {
    pub path: &'a DefPath,
    pub generics: &'a [GenericArgument],
    pub arguments: &'a [tir::CallArgument],
    pub destination: tir::PlaceId,
    /// The TIR block the call continues in.
    pub next: tir::BlockId,
}

/// `core@1/$native.<name>`: a helper or finalizer no registry row names.
/// `$` cannot occur in a source identifier.
/// Whether `ty` is a List or Array: the blocks MEM-6 tracks in an arena.
pub(crate) fn list_or_array(types: &hirsel_types::TypeInterner, ty: TypeId) -> bool {
    matches!(
        types.get(ty).map(|t| &t.kind),
        Some(TypeKind::Builtin {
            kind: hirsel_types::BuiltinType::List | hirsel_types::BuiltinType::Array,
            ..
        })
    )
}

/// Whether a block of the TIR type `ty`, whose block payload is
/// `payload`, is tracked in an arena (MEM-6): a List or Array of rows, of
/// `@soa` columns or of `Packed` values, or a Map or Set.
pub(crate) fn tracked_block(
    types: &hirsel_types::TypeInterner,
    ty: TypeId,
    table: &hirsel_lir::layout_types::LayoutTable,
    payload: hirsel_lir::layout_types::LayoutId,
) -> bool {
    hirsel_lir::layout::keyed(table, payload)
        || (list_or_array(types, ty)
            && matches!(
                table.layout(payload).map(|r| &r.kind),
                Some(
                    hirsel_lir::layout_types::LayoutKind::Sequence { .. }
                        | hirsel_lir::layout_types::LayoutKind::SoaColumns { .. }
                        | hirsel_lir::layout_types::LayoutKind::Sequential { .. }
                )
            ))
}

pub fn native_path(name: &str) -> DefPath {
    hirsel_syntax::DefPath::core(&format!("$native.{name}"))
}

/// The `BODIES` row of `path`.
pub fn body_index(path: &DefPath) -> Option<usize> {
    BODIES.iter().position(|b| (b.path)() == *path)
}

/// The `INLINE` row of `path`.
pub fn inline_lowering(path: &DefPath) -> Option<&'static InlineLowering> {
    INLINE.iter().copied().find(|l| (l.path)() == *path)
}

/// Bumped by any change to a body's generated code, so every artifact keyed
/// by an instance of it is invalidated.
pub const REVISION: u32 = 38;

/// The implementation revision of the library body at `path`.
pub fn revision(path: &DefPath) -> ContentHash {
    ContentHash::of_parts(
        "hirsel.library-body.v1",
        [REVISION.to_le_bytes().as_slice(), &path.fingerprint().0],
    )
}

/// The binder of a library body's generic parameters: the item site of the
/// operation's own path, since a registry operation has no lexical item.
pub fn binder(path: &DefPath) -> SiteRef {
    hirsel_syntax::LexicalOwner::first(path).site(NameSite::Item)
}

/// One required library instance: the body, its complete actuals and its
/// callable type (a `Function` row).
#[derive(Clone, Debug)]
pub struct Request {
    pub body: usize,
    pub path: DefPath,
    pub actuals: Actuals,
    pub callable: TypeId,
    pub shape: CallableShape,
    /// A step thunk's environment layout.
    pub environment: Option<LayoutId>,
    pub finalizer: bool,
    pub receipt: Option<hirsel_lir::sealed_payload_source::ReceiptSource>,
}

impl Request {
    /// The instance of a registry operation: its callable type is the
    /// registry signature under `arguments`, exactly the type whose `FnAbi`
    /// the call site computed.
    pub fn operation(
        program: &Program,
        body: usize,
        path: &DefPath,
        arguments: Vec<GenericArgument>,
    ) -> Result<Self, InstanceError> {
        let signature = program
            .signatures
            .get(path)
            .ok_or_else(|| InstanceError::MissingSignature(path.clone()))?
            .clone();
        let SignatureKind::Function(function) = &signature.kind else {
            return Err(InstanceError::Malformed(format!(
                "{path:?} is called but is not a function"
            )));
        };
        let actuals = Actuals {
            binder: (!arguments.is_empty()).then(|| binder(path)),
            arguments,
            outer: Vec::new(),
        };
        let callable = instantiate_function(
            &program.types,
            &program.memo,
            function,
            hirsel_types::Modes::global(),
            &actuals,
        )
        .map_err(|e| InstanceError::Substitution(path.clone(), e))?;
        let receipt = if path.is_core("arena.seal") {
            let [GenericArgument::Type(root)] = actuals.arguments.as_slice() else {
                return Err(InstanceError::Malformed("seal root actual".into()));
            };
            let key = InstanceKey::new(path, 0, &actuals.key_binders(), &program.types, None, None)
                .map_err(|e| InstanceError::Key(path.clone(), e))?;
            let kind = &program.types.row(*root).kind;
            if matches!(kind, TypeKind::Primitive(_))
                || matches!(kind, TypeKind::Tuple(parts) if parts.is_empty())
            {
                None
            } else {
                Some(
                    sealed_region::receipt_source(program, *root, &key)
                        .map_err(|e| InstanceError::Malformed(e.to_string()))?,
                )
            }
        } else {
            None
        };
        Ok(Self {
            receipt,
            body,
            path: path.clone(),
            actuals,
            callable,
            shape: CallableShape::Definition,
            environment: None,
            finalizer: false,
        })
    }

    /// The function value of registry operation `path`, which has neither a
    /// library body nor an inline lowering: the forwarding body
    /// (`value::FORWARD`) keyed by the operation's own path and actuals, so
    /// its callable type and `FnAbi` are the operation's.
    pub fn operation_value(
        program: &Program,
        path: &DefPath,
        arguments: Vec<GenericArgument>,
    ) -> Result<Self, InstanceError> {
        let forward = body_index(&native_path("operation_value")).expect("registered forwarder");
        Self::operation(program, forward, path, arguments)
    }

    /// The instance of a helper no registry row names: `callable` is the
    /// `Function` row the requiring body states.
    pub fn native(
        body: usize,
        path: &DefPath,
        arguments: Vec<GenericArgument>,
        callable: TypeId,
    ) -> Self {
        Self {
            receipt: None,
            body,
            path: path.clone(),
            actuals: Actuals {
                binder: (!arguments.is_empty()).then(|| binder(path)),
                arguments,
                outer: Vec::new(),
            },
            callable,
            shape: CallableShape::Definition,
            environment: None,
            finalizer: false,
        }
    }

    /// The drop body of the library resource `nominal` under its actuals:
    /// it takes the resource mut and returns unit; source drop bodies stay borrowed.
    pub fn finalizer(
        types: &hirsel_types::TypeInterner,
        body: usize,
        path: &DefPath,
        nominal: &DefPath,
        actuals: &Actuals,
    ) -> Self {
        let this = types.intern(Type::with_kind(TypeKind::Named {
            definition: DefId::intern(nominal),
            arguments: actuals.arguments.clone().into(),
        }));
        let unit = types.intern(Type::unit());
        let callable = types.intern(Type::with_kind(TypeKind::Function(
            hirsel_types::FunctionType {
                state_promises: Default::default(),
                generics: Vec::new().into(),
                parameters: vec![hirsel_types::Parameter {
                    name: None,
                    label: None,
                    passing: hirsel_syntax::PassingMode::Mut,
                    ty: this,
                }]
                .into(),
                result: unit,
                error: None,
                allocation: hirsel_types::Allocation::NoAlloc,
                authority: hirsel_types::AuthoritySummary::Unknown,
                uses: hirsel_types::Uses::Unrestricted,
                contracts: Vec::new().into(),
                incomplete: false,
                foreign: None,
            },
        )));
        Self {
            finalizer: true,
            ..Self::native(body, path, actuals.arguments.clone(), callable)
        }
    }

    /// The canonical key: the path and the complete actuals, with relations,
    /// brands, uniqueness and sendability erased.
    pub fn key(
        &self,
        types: &hirsel_types::TypeInterner,
        layouts: &dyn builder::Layouts,
    ) -> Result<InstanceKey, InstanceError> {
        let convention = match self.shape {
            CallableShape::Definition => None,
            _ => crate::instance::convention(types, self.callable),
        };
        let environment = self.environment.map(|e| {
            // The imported caller and its replayed library request must agree
            // even when unrelated layouts precede this environment.
            hirsel_lir::LayoutKeys::rooted(layouts.table(), e).0
        });
        InstanceKey::new(
            &self.path,
            0,
            &self.actuals.key_binders(),
            types,
            convention.as_ref(),
            environment,
        )
        .map_err(|e| InstanceError::Key(self.path.clone(), e))
    }
}

/// The view's definition of a library instance: a declaration with no TIR
/// body. Nothing reads a body through it; `Cx` builds the unit.
fn declaration(path: &DefPath) -> tir::DefinitionRef {
    std::sync::Arc::new(tir::Definition {
        caller_bodies: vec![],
        protocol_associations: vec![],
        types: Default::default(),
        path: DefId::intern(path),
        inferred_unique: Default::default(),
        signature_hash: revision(path),
        bodies: Vec::new(),
        entry: tir::BodyId(0),
        contracts: Vec::new(),
    })
}

/// Register the instance a request names, or find it.
pub fn instantiate(
    layouts: &mut dyn builder::Layouts,
    instances: &mut Instances,
    request: Request,
) -> Result<InstanceId, InstanceError> {
    let key = request.key(&instances.types, layouts)?;
    if let Some(id) = instances.get(&key) {
        return Ok(id);
    }
    let view = InstanceView::from_definition(
        key,
        declaration(&request.path),
        tir::BodyId(0),
        request.shape,
        request.actuals.substitution(&request.path)?,
        crate::instance::parameter_views(&instances.types, request.callable),
        Vec::new(),
        false,
    );
    let receipt = request.receipt;
    let (id, _) = instances.push(
        layouts,
        view,
        InstanceRole::Library {
            body: request.body,
            finalizer: request.finalizer,
        },
        request.callable,
        request.actuals,
        request.shape,
        request.environment,
    )?;
    if let Some(source) = receipt {
        instances.abis[id.index()] = layouts
            .receipt_fn_abi(instances.abis[id.index()], &source.contract)
            .map_err(|e| InstanceError::Layout(request.path.clone(), e))?;
        instances.register_library_receipt(id, source);
    }
    Ok(id)
}

/// Build the unit of library instance `id`. Instances its builder requires
/// are appended to `instances`.
pub fn build(
    program: &Program,
    layouts: &mut dyn builder::Layouts,
    instances: &mut Instances,
    id: InstanceId,
) -> Result<LirUnit<Value>, PassError> {
    let InstanceRole::Library { body, .. } = instances.roles[id.index()] else {
        return Err(PassError::Malformed(format!(
            "{id:?} is not a library instance"
        )));
    };
    let mut cx = Cx::new(program, layouts, instances, id);
    (BODIES[body].build)(&mut cx)
}

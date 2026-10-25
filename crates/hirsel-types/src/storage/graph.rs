//! Compact static body products. Source derives these from its own ownership
//! CFG; V rebuilds them from executable operands rather than accepting them.
//! Local indices and source sites are proof observations, not semantic hashes.
use super::{StorageCallable, StorageInput, StorageResult, StorageSelector};
use crate::{OwnedTypePart, ParamId, SharedPath};
use hirsel_span::Span;
use hirsel_syntax::{ContentHash, DefId};

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct StoragePlace {
    pub local: u32,
    pub parts: SharedPath<OwnedTypePart>,
}
impl StoragePlace {
    pub fn root(local: u32) -> Self {
        Self {
            local,
            parts: SharedPath::new(),
        }
    }
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq)]
pub struct StorageInputBinding {
    pub input: StorageInput,
    pub place: StoragePlace,
    /// A proved plain value or immutable handle boundary has no owned storage
    /// under the caller's placement. No type name alone supplies this proof.
    pub holds_storage: bool,
    pub callables: Vec<SharedPath<OwnedTypePart>>,
    pub mutable: bool,
}

/// Allocation placement is an effect even when its result is unused. Missing
/// explicit source operands never mean that an allocation is independent.
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq)]
pub enum StoragePlacement {
    Independent,
    Sources(Vec<StoragePlace>),
    Unknown,
}

/// A bodyless core callable whose storage effect is modeled directly.
#[derive(serde::Serialize, serde::Deserialize, Clone, Copy, Debug, Eq, PartialEq)]
pub enum StoragePrimitive {
    IntegerOrder,
}

impl StoragePrimitive {
    /// Identity vocabulary only. Source and V must independently review the
    /// actual declaration before assigning a primitive effect model.
    pub fn integer_order_type(definition: DefId) -> Option<crate::Primitive> {
        use crate::Primitive::*;
        [
            ("u8", U8),
            ("u16", U16),
            ("u32", U32),
            ("u64", U64),
            ("u128", U128),
            ("i8", I8),
            ("i16", I16),
            ("i32", I32),
            ("i64", I64),
            ("i128", I128),
        ]
        .into_iter()
        .find_map(|(name, kind)| {
            definition
                .path()
                .is_core(&format!("$intrinsic.OrderCompare_{name}"))
                .then_some(kind)
        })
    }
}

/// A capture-free code leaf observed in a declared immutable field graph.
/// Paths describe the actual value, never callable headers or inferred bodies.
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq)]
pub struct StorageConstantCode {
    pub parts: SharedPath<OwnedTypePart>,
    pub definition: DefId,
    pub arguments: Vec<crate::GenericArgument>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub primitive: Option<StoragePrimitive>,
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq)]
pub enum StorageValue {
    /// Original source reference, resolved by the program-level source query.
    ConstantReference {
        definition: DefId,
        arguments: Vec<crate::Size>,
    },
    /// Independently observed immutable data with actual code leaves.
    Constant {
        code: Vec<StorageConstantCode>,
        complete: bool,
    },
    /// Original named code and selected generic actuals. Source program
    /// assembly must resolve this to the actual original entry before solving.
    Reference {
        definition: DefId,
        arguments: Vec<crate::GenericArgument>,
    },
    Fresh,
    Unknown,
    Copy {
        value: StoragePlace,
        detaches: bool,
    },
    Unique {
        value: StoragePlace,
        allocates: bool,
    },
    Construct {
        fields: Vec<(SharedPath<OwnedTypePart>, StoragePlace)>,
        /// Independently reconstructed allocation sources. Owned fields keep
        /// their own dependencies even when fresh placement is independent.
        placement: StoragePlacement,
    },
    Callable {
        target: StorageCallable,
        captures: Vec<StoragePlace>,
    },
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq)]
pub enum StorageCallee {
    Definition(DefId),
    Apply {
        definition: DefId,
        arguments: Vec<crate::GenericArgument>,
    },
    Value(StoragePlace),
    /// Reviewed bodyless operations. The two checkers independently derive
    /// their selector recipes from the real nominal operation/declaration.
    Declared {
        allocations: Vec<StorageSelector>,
        results: Vec<StorageResult>,
        complete: bool,
    },
    /// The reviewed deep copy keeps original code/capture producers while
    /// relocating owned storage to argument0. It never invokes that code.
    CopyTo {
        holds_storage: bool,
        requires_code: bool,
    },
    /// Retain both actual inserted operands in the mutable receiver. Paths
    /// follow the existing owned Map element/key/value vocabulary.
    MapInsert,
    /// Select actual stored value producers, never the callable result type.
    MapGet {
        result_path: SharedPath<OwnedTypePart>,
        requires_code: bool,
    },
    /// Removal may allocate from its receiver and returns an old stored value.
    /// The remaining receiver conservatively retains every possible producer.
    MapRemove {
        result_path: SharedPath<OwnedTypePart>,
        requires_code: bool,
    },
    /// Move the actual child's stored result out of a reviewed Task carrier,
    /// preserving code producers as well as storage dependencies.
    TaskAwait,
    ReadRegion,
    /// Static code/root producer transport for the reviewed ownership transfer.
    SealArena,
    ScopeCallback {
        callback: ParamId,
        /// The actual success payload path for a bounded scope, or root for with.
        result_path: SharedPath<OwnedTypePart>,
    },
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq)]
pub enum StorageOp {
    Nop,
    Assign(Vec<(StoragePlace, StorageValue)>),
    Call {
        callee: StorageCallee,
        arguments: Vec<(ParamId, StoragePlace)>,
        /// Actual mutable operand slots, independently read by each checker.
        mutations: Vec<ParamId>,
        destination: Option<StoragePlace>,
    },
    Return(StoragePlace),
    Kill(Vec<u32>),
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq)]
pub struct StorageNode {
    pub operation: StorageOp,
    pub successors: Vec<usize>,
    pub site: Span,
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq)]
pub struct StorageBody {
    pub callable: StorageCallable,
    /// Public original entry points remain roots even when a known caller
    /// also selects them. This is a declaration observation, not code authority.
    pub external_entry: bool,
    pub paths: Vec<SharedPath<OwnedTypePart>>,
    pub generics: Vec<crate::GenericParameter>,
    /// Existing canonical original callable TYPE identity, when it can be
    /// closed/abstracted. Absence never licenses a heap-independent profile.
    pub source_type: Option<ContentHash>,
    pub inputs: Vec<StorageInputBinding>,
    pub nodes: Vec<StorageNode>,
    pub entry: usize,
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Default, Eq, PartialEq)]
pub struct StorageDefinition {
    pub bodies: Vec<StorageBody>,
}

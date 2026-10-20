//! Compiler roles of core-library operations: the borrowed-loop protocols the body
//! checker plans and the authority intrinsics the mode checker dispatches on. The
//! driver's registry declares the operations; these tables are the single source of
//! their roles, which checker tests read without the driver.
use hirsel_syntax::DefPath;

/// These labels match the checker's ForPlan protocols. `$` cannot occur in a
/// source identifier; consumers use these paths directly, never source lookup.
pub const LOOP_PROTOCOLS: &[&str] = &[
    "MapInsertionOrder",
    "SortedMapOrder",
    "SetElements",
    "ArrayConsume",
    "ListConsume",
    "InlineConsume",
];

/// Each protocol with its `$intrinsic` open and next operations.
pub fn loop_protocols() -> Vec<(&'static str, DefPath, DefPath)> {
    LOOP_PROTOCOLS
        .iter()
        .map(|&protocol| {
            (
                protocol,
                hirsel_syntax::DefPath::core(&format!("$intrinsic.{protocol}_open")),
                hirsel_syntax::DefPath::core(&format!("$intrinsic.{protocol}_next")),
            )
        })
        .collect()
}

/// Nominal identities supplied by the core-library registry. Spelling is never a role.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Intrinsic {
    SharedFreeze,
    ArenaSeal,
    TaskAsync,
    TaskIsolate,
    TaskAwait,
    TaskDetach,
    MutexCreate,
    MutexWith,
    ChannelOpen,
    ChannelSend,
    AtomicCreate,
    Synchronization,
    HandleOperation,
    UnsafeOperation,
}

/// Checker dispatch is by nominal path and typed authority role.
/// A user same-spelled function has no role.
pub fn intrinsic_roles() -> Vec<(DefPath, Intrinsic)> {
    use Intrinsic::*;
    [
        ("task.async", TaskAsync),
        ("task.isolate", TaskIsolate),
        ("task.await", TaskAwait),
        ("task.detach", TaskDetach),
        ("task.check_canceled", HandleOperation),
        ("mutex.create", MutexCreate),
        ("mutex.with", MutexWith),
        ("channel.open", ChannelOpen),
        ("channel.send", ChannelSend),
        ("channel.receive", HandleOperation),
        ("sender.dup", HandleOperation),
        ("atomic.create", AtomicCreate),
        ("atomic.load", Synchronization),
        ("atomic.store", Synchronization),
        ("shared.freeze", SharedFreeze),
        ("arena.seal", ArenaSeal),
        ("int.unchecked_add", UnsafeOperation),
        ("file.narrow", HandleOperation),
        ("file.write_all", HandleOperation),
        ("file.commit", HandleOperation),
    ]
    .into_iter()
    .map(|(name, role)| (hirsel_syntax::DefPath::core(name), role))
    .collect()
}

/// Operations eliminated into existing TIR instructions by source lowering.
/// They have declarations but no runtime dispatch entry or operation-manifest row.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LoweredIntrinsic {
    Replace,
}

impl LoweredIntrinsic {
    pub const ALL: [Self; 1] = [Self::Replace];

    pub fn definition(self) -> DefPath {
        match self {
            Self::Replace => DefPath::core("replace"),
        }
    }

    pub fn for_definition(path: &DefPath) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|intrinsic| intrinsic.definition() == *path)
    }
}

/// Key type argument positions of the typed store surface. This identifies an
/// operation only; source and IR checkers independently prove its key schema.
pub fn table_key_parameters(path: &DefPath) -> &'static [usize] {
    if !path.package_is("std", "1") || path.0.modules.as_slice() != ["table"] {
        return &[];
    }
    match path.0.name.as_str() {
        "named" | "create" | "drop" | "get" | "put" | "delete" | "range" => &[0],
        "migrate" => &[0, 2],
        _ => &[],
    }
}

//! Compiler roles of core-library operations: the borrowed-loop protocols the body
//! checker plans and the authority intrinsics the mode checker dispatches on. The
//! driver's registry declares the operations; these tables are the single source of
//! their roles, which checker tests read without the driver.
use hirsel_syntax::{DefPath, ItemPath, PackageId};

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
                core_path(&format!("$intrinsic.{protocol}_open")),
                core_path(&format!("$intrinsic.{protocol}_next")),
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
    .map(|(name, role)| (core_path(name), role))
    .collect()
}

/// `core@1` paths, as the registry spells them (`module.name`).
fn core_path(name: &str) -> DefPath {
    let mut modules: Vec<_> = name.split('.').map(str::to_owned).collect();
    let name = modules.pop().expect("core name");
    DefPath(ItemPath {
        package: PackageId {
            name: "core".into(),
            version: "1".into(),
        },
        modules,
        name,
    })
}

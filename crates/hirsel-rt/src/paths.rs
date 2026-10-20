//! A run's Path identities and launch arguments, shared by its Io binding and its tasks
//! (`RunShared::names`), so interning is a concurrent service that never reaches the binding
//! (multicore design §4.1, locality C).
//!
//! The intern table follows lasso's `ThreadedRodeo` (`lasso/src/threaded_rodeo.rs`): a map from
//! text to key plus an append-only store of the interned strings, whose heap text never moves,
//! so a lookup by key hands out `&str` for the table's lifetime. Lasso shards its map
//! (`DashMap`); one mutex serves here until a benchmark row asks for more. A Path's word is
//! unobservable (design §2): equality is by word after deduplication, and order is by text.

use std::collections::HashMap;
use std::sync::Mutex;

use crate::context::TaskCtx;

/// Interned Path texts: identity 1 is the first text interned.
#[derive(Default)]
pub struct PathTable {
    inner: Mutex<Table>,
}

#[derive(Default)]
struct Table {
    ids: HashMap<&'static str, u64>,
    /// Owned texts by identity minus one. A text is never removed or moved before the table
    /// drops, so the `&'static str` keys above and the borrows `text` hands out stay valid.
    texts: Vec<Box<str>>,
}

impl PathTable {
    /// The identity of `path`, interning it if it is new.
    pub fn intern(&self, path: &str) -> u64 {
        let mut table = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(&id) = table.ids.get(path) {
            return id;
        }
        let text: Box<str> = path.into();
        // SAFETY: the box's heap text lives until the table drops; `ids` drops with it.
        let key: &'static str = unsafe { &*std::ptr::from_ref::<str>(&text) };
        table.texts.push(text);
        let id = table.texts.len() as u64;
        table.ids.insert(key, id);
        id
    }

    /// The text of identity `id`, when it was interned here.
    pub fn text(&self, id: u64) -> Option<&str> {
        let table = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let text = table.texts.get(usize::try_from(id.checked_sub(1)?).ok()?)?;
        // SAFETY: the text's heap storage is never freed or moved while `self` lives.
        Some(unsafe { &*std::ptr::from_ref::<str>(text) })
    }
}

/// What the Io binding and the run's tasks share: Path identities and the launch arguments.
#[derive(Default)]
pub struct Names {
    pub(crate) paths: PathTable,
    pub(crate) arguments: Vec<String>,
}

impl Names {
    pub(crate) fn path_text(&self, id: u64) -> &str {
        self.paths
            .text(id)
            .unwrap_or_else(|| crate::fault("unknown run-context Path identity"))
    }
}

/// The run's names, which binding its Io installed.
pub(crate) fn names(ctx: *mut TaskCtx) -> &'static Names {
    // SAFETY: the context and its run are live for the whole run, and the names are replaced
    // only by binding another adapter, which no running task does.
    let run = unsafe { &*(*ctx).run_ptr() };
    run.names
        .as_deref()
        .unwrap_or_else(|| crate::fault("Io call without an explicitly bound adapter"))
}

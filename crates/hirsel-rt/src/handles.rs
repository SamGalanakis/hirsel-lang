//! Logical capability identities, scoped to one adapter. No host address is a handle.
//!
//! Handle numbers are allocated in increasing order, never reused, and recorded on tapes, so
//! both engines see the same numbers; the rows live in an [`IdMap`], so a lookup costs the
//! same however many handles are open.
use crate::id_map::IdMap;
use std::collections::BTreeMap;
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Grant {
    Io,
    Stdin,
    Stderr,
    Stdout,
    MonotonicClock,
    WallClock,
    Random,
    Tasks,
    /// A rule set and the world that serves it.
    Net {
        rules: std::collections::BTreeSet<crate::net::NetRule>,
        world: crate::net::NetWorld,
    },
    /// Branded by its Net; `address`/`port` name the rule that authorized it, and a
    /// ConnectTls stream carries plaintext only once `tls` is established.
    Stream {
        net: u64,
        address: String,
        port: u16,
        operation: crate::net::NetOperation,
        tls: bool,
    },
    UdpSocket {
        net: u64,
    },
    /// Branded by its Net; `operation` is the rule that opened it, which its accepted
    /// streams keep.
    Listener {
        net: u64,
        operation: crate::net::NetOperation,
    },
    Test {
        seed: u64,
    },
    /// Exact program rules and the world that serves them.
    Process {
        rules: Vec<hirsel_run_schema::ProcessRule>,
        world: crate::process::ProcessWorld,
    },
    /// A streaming child of Proc `process`, served by that Proc's world; with `group` a
    /// spawned pipeline, its stage count and standard-error bound.
    Child {
        process: u64,
        world: crate::process::ProcessWorld,
        stdin_open: bool,
        group: Option<(usize, u64)>,
    },
    /// An `env.narrow` result: exactly these names of the run's Env. The root Env is
    /// `io::ENV_HANDLE`, which has no row.
    Env {
        names: std::collections::BTreeSet<String>,
    },
    Directory {
        root: String,
        access: hirsel_run_schema::Access,
    },
    File {
        path: String,
        directory: u64,
        position: u64,
        writable: bool,
    },
    AtomicFile {
        temporary: String,
        destination: String,
        directory: u64,
    },
    /// A read-only mapping of `length` bytes, branded by its Dir; `truncated` once a check
    /// found the file shorter than the mapping, after which every check reports it.
    Mapped {
        directory: u64,
        length: u64,
        truncated: bool,
    },
    /// A change watcher branded by its Dir, over tree path `root` and, with `recursive`,
    /// everything below it.
    Watcher {
        directory: u64,
        root: String,
        recursive: bool,
    },
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Handles {
    pub(crate) rows: IdMap<Grant>,
    next: u64,
}
impl Default for Handles {
    fn default() -> Self {
        Self {
            rows: IdMap::from_iter([(1, Grant::Io)]),
            next: 2,
        }
    }
}
impl Handles {
    pub(crate) fn test(seed: u64) -> Self {
        Self {
            rows: IdMap::from_iter([(1, Grant::Test { seed })]),
            next: 2,
        }
    }
    /// The open handles in number order.
    pub fn observed(&self) -> BTreeMap<u64, Grant> {
        self.rows.to_btree()
    }
    pub(crate) fn insert(&mut self, grant: Grant) -> u64 {
        let id = self.next;
        self.next = self
            .next
            .checked_add(1)
            .expect("logical handle space exhausted");
        assert!(self.rows.insert(id, grant).is_none());
        id
    }
    pub(crate) fn next(&self) -> u64 {
        self.next
    }
}

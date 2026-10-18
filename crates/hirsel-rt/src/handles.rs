//! Logical capability identities, scoped to one adapter. No host address is a handle.
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
        rules: Vec<hirsel_run_config::ProcessRule>,
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
        access: hirsel_run_config::Access,
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
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Handles {
    pub(crate) rows: BTreeMap<u64, Grant>,
    next: u64,
}
impl Default for Handles {
    fn default() -> Self {
        Self {
            rows: BTreeMap::from([(1, Grant::Io)]),
            next: 2,
        }
    }
}
impl Handles {
    pub(crate) fn test(seed: u64) -> Self {
        Self {
            rows: BTreeMap::from([(1, Grant::Test { seed })]),
            next: 2,
        }
    }
    pub fn observed(&self) -> &BTreeMap<u64, Grant> {
        &self.rows
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

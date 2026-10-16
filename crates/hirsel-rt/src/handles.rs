//! Logical capability identities, scoped to one adapter. No host address is a handle.
use std::collections::BTreeMap;
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct Io(pub u64);
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct Stdout(pub u64);
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct Dir(pub u64);
#[derive(Debug, Eq, PartialEq)]
pub struct File(pub u64);
#[derive(Debug, Eq, PartialEq)]
pub struct AtomicFile(pub u64);
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Grant {
    Io,
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
    Listener {
        net: u64,
    },
    Test {
        seed: u64,
    },
    Directory {
        root: String,
        access: hirsel_run_config::Access,
    },
    File {
        path: String,
        directory: u64,
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

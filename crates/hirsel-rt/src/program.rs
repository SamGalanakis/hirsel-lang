//! A compiled program as its runs see it. The generated `main` passes one
//! [`ProgramDescriptor`] to [`crate::launch::hirsel_rt_v2_main`]; the runtime loads it into a
//! [`Program`], which owns everything a run of that program needs from the process: its entry,
//! descriptor registry, failure table, semantic digest, entry name, roots and flags. Nothing of
//! a program is process-global, so runs of different programs share a process.
use std::sync::Arc;

use crate::isolate::FailureTable;
use crate::launch::{Config, Entry, RootsEntry, TestEntry, ZeroEntry};
use crate::{ABI_VERSION, Layout};

/// [`ProgramDescriptor::flags`]: the program calls C outside a deterministic wrapper, so its
/// runs cannot be recorded or replayed (PROG-4).
pub const UNREPLAYABLE: u32 = 1;
/// [`ProgramDescriptor::flags`]: the executable is its own host and takes fresh grants at
/// each launch (`hirsel build`); otherwise it runs the run its [`Config`] binds.
pub const STANDALONE: u32 = 2;
/// [`ProgramDescriptor::flags`]: the entry reaches an escape from capability checking, so an
/// `admit.safe@1` run of it is refused (CAP-6).
pub const ESCAPES: u32 = 4;
/// [`ProgramDescriptor::flags`]: the code charges fuel at its fuel points, so a run of it can
/// be given a fuel budget (`budgets.limits@1`).
pub const FUELED: u32 = 8;
const KNOWN_FLAGS: u32 = UNREPLAYABLE | STANDALONE | ESCAPES | FUELED;

/// A compiled program at the runtime boundary (`hirsel_rt_v2.h`).
#[repr(C)]
pub struct ProgramDescriptor {
    /// Must equal [`ABI_VERSION`].
    pub abi_version: u32,
    pub flags: u32,
    /// The entry's roots: bits 0..4 for Io, Args, Env, Unsafe and Test.
    pub roots: u32,
    pub layout_count: u32,
    /// The compiler's root-order adapter, which receives [Io, Args, Env, Unsafe, Test].
    pub entry: RootsEntry,
    /// The entry's inline result size.
    pub result_size: u64,
    pub layouts: *const *const Layout,
    /// The encoded failure table ([`FailureTable::decode`]).
    pub failure_table: *const u8,
    pub failure_table_length: u64,
    /// The entry's full definition path, UTF-8.
    pub entry_name: *const u8,
    pub entry_name_length: u64,
    /// The semantic program digest runs bind to.
    pub digest: [u8; 32],
    /// The bound run's seeds and identity; zero for a [`STANDALONE`] program.
    pub config: Config,
}

/// How the runtime calls a program's entry.
#[derive(Clone, Copy)]
pub(crate) enum Code {
    Zero(ZeroEntry),
    Main(Entry),
    Test(TestEntry),
    Roots(RootsEntry, u8),
}

impl Code {
    pub(crate) fn is_test(self) -> bool {
        matches!(
            self,
            Self::Test(_) | Self::Roots(_, crate::standalone::ROOT_TEST)
        )
    }
    pub(crate) fn is_zero(self) -> bool {
        matches!(self, Self::Zero(_) | Self::Roots(_, 0))
    }
    /// The entry's roots, as the standalone mask spells them.
    pub(crate) fn roots(self) -> u8 {
        use crate::standalone::{ROOT_ARGS, ROOT_IO, ROOT_TEST};
        match self {
            Self::Zero(_) => 0,
            Self::Main(_) => ROOT_IO | ROOT_ARGS,
            Self::Test(_) => ROOT_TEST,
            Self::Roots(_, mask) => mask,
        }
    }
}

/// A loaded program: what each of its runs reads, shared by all of them.
pub struct Program {
    pub(crate) code: Code,
    pub(crate) result_size: u64,
    pub(crate) layouts: *const *const Layout,
    pub(crate) layout_count: u32,
    pub(crate) failure_table: Option<Arc<FailureTable>>,
    pub(crate) digest: [u8; 32],
    pub(crate) entry_name: String,
    pub(crate) flags: u32,
    pub(crate) config: Config,
}

impl Program {
    /// Load a compiled program.
    ///
    /// # Safety
    /// `descriptor` is null or points to a descriptor whose pointers name live, immutable data
    /// for as long as the program and its runs exist.
    pub unsafe fn load(descriptor: *const ProgramDescriptor) -> Result<Program, String> {
        // SAFETY: caller contract.
        let Some(d) = (unsafe { descriptor.as_ref() }) else {
            return Err("missing program descriptor".into());
        };
        if d.abi_version != ABI_VERSION {
            return Err(format!(
                "runtime ABI {} but the program was built for {}",
                ABI_VERSION, d.abi_version
            ));
        }
        if d.flags & !KNOWN_FLAGS != 0 {
            return Err("unknown program flags".into());
        }
        let roots = u8::try_from(d.roots).map_err(|_| "invalid compiled root selection")?;
        crate::standalone::required_roots(roots).map_err(|e| e.to_string())?;
        // SAFETY: caller contract.
        let bytes = |data: *const u8, length: u64| -> Result<&[u8], String> {
            let length = usize::try_from(length).map_err(|_| "a descriptor length overflows")?;
            if length == 0 {
                return Ok(&[]);
            }
            if data.is_null() {
                return Err("a descriptor names null data".into());
            }
            Ok(unsafe { std::slice::from_raw_parts(data, length) })
        };
        let table = FailureTable::decode(bytes(d.failure_table, d.failure_table_length)?)
            .ok_or("a malformed failure table")?;
        let entry_name = std::str::from_utf8(bytes(d.entry_name, d.entry_name_length)?)
            .map_err(|_| "invalid entry name")?
            .to_owned();
        Ok(Program {
            code: Code::Roots(d.entry, roots),
            result_size: d.result_size,
            layouts: d.layouts,
            layout_count: d.layout_count,
            failure_table: Some(Arc::new(table)),
            digest: d.digest,
            entry_name,
            flags: d.flags,
            config: d.config,
        })
    }

    /// A harness program: an entry and its descriptors, with no failure table or binding.
    pub(crate) fn harness(
        code: Code,
        result_size: u64,
        layouts: *const *const Layout,
        layout_count: u32,
        config: Config,
    ) -> Program {
        Program {
            code,
            result_size,
            layouts,
            layout_count,
            failure_table: None,
            digest: [0; 32],
            entry_name: String::new(),
            flags: 0,
            config,
        }
    }

    pub(crate) fn standalone(&self) -> bool {
        self.flags & STANDALONE != 0
    }
    pub(crate) fn unreplayable(&self) -> bool {
        self.flags & UNREPLAYABLE != 0
    }
    pub(crate) fn escapes(&self) -> bool {
        self.flags & ESCAPES != 0
    }
    pub(crate) fn fueled(&self) -> bool {
        self.flags & FUELED != 0
    }
}

//! A compiled program as its runs see it. The generated `main` passes one
//! [`ProgramDescriptor`] to [`crate::launch::hirsel_rt_v2_main`]; the runtime loads it into a
//! [`Program`], which owns everything a run of that program needs from the process: its entry,
//! descriptor registry, failure table, semantic digest, entry name, roots and flags. Nothing of
//! a program is process-global, so runs of different programs share a process.
use crate::isolate::FailureTable;
use crate::launch::{Config, Entry, RootsEntry, TestEntry, ZeroEntry};
use crate::{ABI_VERSION, Layout};
mod callees;
#[doc(hidden)]
pub use callees::{
    Metadata as CalleeMetadata, RequestSchemas as RunnerRequestSchemas,
    identity as callee_identity, metadata_identity as callee_metadata_identity,
};

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
/// be given a fuel budget (`budgets.limits@2`).
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
    /// The encoded failure table ([`FailureTable::new`]).
    pub failure_table: *const u8,
    pub failure_table_length: u64,
    /// The entry's full definition path, UTF-8.
    pub entry_name: *const u8,
    pub entry_name_length: u64,
    /// The semantic program digest runs bind to.
    pub digest: [u8; 32],
    /// The bound run's seeds and identity; zero for a [`STANDALONE`] program.
    pub config: Config,
    /// The definitions the program's units name as Entry values (ABI 30, RUN-3): each one's
    /// function-reference code and full path, which runner calls are made by.
    pub entries: *const EntryRow,
    pub entry_count: u64,
    /// ABI 45: 0 = Result[(), E], 1 = ExitCode, 2 = Result[ExitCode, E].
    pub return_kind: u64,
    /// Offset of the unsigned status byte for return kinds 1 and 2.
    pub success_offset: u64,
    /// ABI 48: stable identities for callable/iterator code, separate from
    /// RUN-3 Entry authority. These rows never grant runner entry admission.
    pub value_codes: *const EntryRow,
    pub value_code_count: u64,
    /// ABI 49: canonical callee metadata JSON, sealed with the program identity.
    /// Missing data carries no authority to launch production runner calls.
    pub callee_facts: *const u8,
    pub callee_facts_length: u64,
    /// Code references for the facts; each must match the RUN-3 Entry table.
    pub callee_codes: *const EntryRow,
    pub callee_code_count: u64,
    pub callee_identity: [u8; 32],
}

/// One row of [`ProgramDescriptor::entries`].
#[repr(C)]
pub struct EntryRow {
    pub code: *const std::ffi::c_void,
    pub path: *const u8,
    pub path_length: u64,
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
    pub(crate) return_kind: u64,
    pub(crate) success_offset: u64,
    pub(crate) layouts: *const *const Layout,
    pub(crate) layout_count: u32,
    pub(crate) failure_table: Option<FailureTable>,
    pub(crate) digest: [u8; 32],
    pub(crate) entry_name: String,
    pub(crate) flags: u32,
    pub(crate) config: Config,
    /// Each Entry's code word and its definition's full path.
    pub(crate) entries: Vec<(usize, String)>,
    pub(crate) value_codes: std::sync::Arc<ValueCodes>,
    pub(crate) ordered_image: std::sync::Arc<crate::tasks::ordered::image::Image>,
    pub(crate) callees: Option<CalleeMetadata>,
}

impl Program {
    /// Load a compiled program.
    ///
    /// # Safety
    /// `descriptor` is null or points to a descriptor whose pointers name live, immutable data
    /// for as long as the program and its runs exist.
    pub unsafe fn load(descriptor: *const ProgramDescriptor) -> Result<Program, String> {
        // SAFETY: caller contract.
        if descriptor.is_null() {
            return Err("missing program descriptor".into());
        }
        // Read the common prefix before borrowing the current descriptor:
        // older ABI records may be shorter than this version's structure.
        let abi_version = unsafe { descriptor.cast::<u32>().read() };
        if abi_version != ABI_VERSION {
            return Err(format!(
                "runtime ABI {} but the program was built for {}",
                ABI_VERSION, abi_version
            ));
        }
        let d = unsafe { &*descriptor };
        if d.flags & !KNOWN_FLAGS != 0 {
            return Err("unknown program flags".into());
        }
        if d.return_kind > 2 || (d.return_kind != 0 && d.success_offset >= d.result_size) {
            return Err("invalid entry return description".into());
        }
        let roots = u8::try_from(d.roots).map_err(|_| "invalid compiled root selection")?;
        crate::standalone::required_roots(roots).map_err(|e| e.to_string())?;
        // SAFETY: caller contract: the data outlives the program and its runs, which hold
        // the only views of it.
        let bytes = |data: *const u8, length: u64| -> Result<&'static [u8], String> {
            let length = usize::try_from(length).map_err(|_| "a descriptor length overflows")?;
            if length == 0 {
                return Ok(&[]);
            }
            if data.is_null() {
                return Err("a descriptor names null data".into());
            }
            Ok(unsafe { std::slice::from_raw_parts(data, length) })
        };
        let table = FailureTable::new(bytes(d.failure_table, d.failure_table_length)?)
            .ok_or("a malformed failure table")?;
        let entry_name = std::str::from_utf8(bytes(d.entry_name, d.entry_name_length)?)
            .map_err(|_| "invalid entry name")?
            .to_owned();
        let count = usize::try_from(d.entry_count).map_err(|_| "a descriptor length overflows")?;
        if count != 0 && d.entries.is_null() {
            return Err("a descriptor names null data".into());
        }
        let mut entries = Vec::with_capacity(count);
        for index in 0..count {
            // SAFETY: caller contract: `entry_count` rows are live.
            let row = unsafe { &*d.entries.add(index) };
            let path = std::str::from_utf8(bytes(row.path, row.path_length)?)
                .map_err(|_| "invalid entry path")?;
            entries.push((row.code as usize, path.to_owned()));
        }
        let value_codes = unsafe { load_value_codes(d.value_codes, d.value_code_count) }?;
        let callees = unsafe { callees::load(d, &entries) }?;
        Ok(Program {
            code: Code::Roots(d.entry, roots),
            result_size: d.result_size,
            return_kind: d.return_kind,
            success_offset: d.success_offset,
            layouts: d.layouts,
            layout_count: d.layout_count,
            failure_table: Some(table),
            digest: d.digest,
            entry_name,
            flags: d.flags,
            config: d.config,
            entries,
            value_codes: std::sync::Arc::new(value_codes),
            ordered_image: Default::default(),
            callees,
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
            return_kind: 0,
            success_offset: 0,
            layouts,
            layout_count,
            failure_table: None,
            digest: [0; 32],
            entry_name: String::new(),
            flags: 0,
            config,
            entries: Vec::new(),
            value_codes: Default::default(),
            ordered_image: Default::default(),
            callees: None,
        }
    }

    pub(crate) fn runner_callees(
        &self,
    ) -> Result<&std::collections::BTreeMap<String, hirsel_run_schema::CalleeFacts>, String> {
        self.callees.as_ref().map(|m| &m.facts).ok_or_else(|| {
            "production runners require compiled callee facts (runtime ABI 49)".into()
        })
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

/// Validated callable identities borrow the program's immutable descriptor data.
/// The owned address index is needed only when observation encodes a callable.
#[derive(Default)]
pub(crate) struct ValueCodes {
    rows: Vec<(usize, &'static str)>,
    index: std::sync::OnceLock<std::sync::Arc<std::collections::BTreeMap<usize, String>>>,
}

impl ValueCodes {
    pub(crate) fn index(&self) -> std::sync::Arc<std::collections::BTreeMap<usize, String>> {
        self.index
            .get_or_init(|| {
                let mut index = std::collections::BTreeMap::new();
                for &(code, name) in &self.rows {
                    // Linkers may coalesce equivalent code. Keep the first stable identity.
                    index.entry(code).or_insert_with(|| name.to_owned());
                }
                std::sync::Arc::new(index)
            })
            .clone()
    }
}

/// # Safety
/// `rows` names `length` immutable rows and their name bytes for this program.
unsafe fn load_value_codes(rows: *const EntryRow, length: u64) -> Result<ValueCodes, String> {
    let count = usize::try_from(length).map_err(|_| "too many value code identities")?;
    if count > 65_536 || (count != 0 && rows.is_null()) {
        return Err("invalid value code identity table".into());
    }
    let mut value_codes = ValueCodes {
        rows: Vec::with_capacity(count),
        index: Default::default(),
    };
    let mut identity_bytes = 0u64;
    for index in 0..count {
        let row = unsafe { &*rows.add(index) };
        if row.code.is_null() || row.path_length == 0 || row.path_length > 1 << 20 {
            return Err("invalid value code identity".into());
        }
        identity_bytes = identity_bytes
            .checked_add(row.path_length)
            .ok_or("value code identity limit")?;
        if identity_bytes > 16 << 20 {
            return Err("value code identity limit".into());
        }
        if row.path.is_null() {
            return Err("invalid value code identity data".into());
        }
        // SAFETY: descriptor data lives for the program and all its runs. Only those
        // owners retain this view; the index returned to observers owns its strings.
        let name = std::str::from_utf8(unsafe {
            std::slice::from_raw_parts(row.path, row.path_length as usize)
        })
        .map_err(|_| "invalid value code identity text")?;
        value_codes.rows.push((row.code as usize, name));
    }
    Ok(value_codes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn value_code_table_is_bounded_text_and_never_runner_entry_authority() {
        let row = |code: usize, name: &'static [u8]| EntryRow {
            code: code as *const std::ffi::c_void,
            path: name.as_ptr(),
            path_length: name.len() as u64,
        };
        let rows = [row(7, b"a"), row(7, b"b"), row(9, b"different")];
        let codes = unsafe { load_value_codes(rows.as_ptr(), rows.len() as u64) }.unwrap();
        assert_eq!(
            *codes.index(),
            std::collections::BTreeMap::from([(7, "a".into()), (9, "different".into())])
        );
        let mut program =
            Program::harness(Code::Zero(noop), 0, std::ptr::null(), 0, Config::default());
        program.value_codes = codes.into();
        assert!(
            program.entries.is_empty(),
            "code identities grant no RUN-3 authority"
        );
        assert!(
            unsafe { load_value_codes(std::ptr::null(), 0) }
                .unwrap()
                .index()
                .is_empty()
        );
        assert!(unsafe { load_value_codes(std::ptr::null(), 1) }.is_err());
        assert!(unsafe { load_value_codes(std::ptr::null(), 65_537) }.is_err());
        // A coalesced address does not excuse a malformed later identity.
        let malformed_duplicate = [row(7, b"valid"), row(7, b"\xff")];
        assert!(
            unsafe {
                load_value_codes(
                    malformed_duplicate.as_ptr(),
                    malformed_duplicate.len() as u64,
                )
            }
            .is_err()
        );
        for row in [
            row(0, b"code"),
            row(1, b""),
            row(1, b"\xff"),
            EntryRow {
                code: 1usize as *const _,
                path: std::ptr::null(),
                path_length: 1,
            },
            EntryRow {
                code: 1usize as *const _,
                path: std::ptr::null(),
                path_length: (1 << 20) + 1,
            },
        ] {
            assert!(unsafe { load_value_codes(&row, 1) }.is_err());
        }
    }

    unsafe extern "C" fn noop(_: *mut crate::TaskCtx, _: *mut u8, _: *mut u8) -> u8 {
        crate::STATUS_OK
    }
}

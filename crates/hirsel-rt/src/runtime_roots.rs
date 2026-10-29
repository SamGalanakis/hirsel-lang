//! Per-program runtime roots. A compiler-generated constant names each selected
//! provider directly. ELF section collection drops unselected providers.
//! Embedding hosts use these weak defaults and keep the complete runtime;
//! images share their host's runtime rather than replacing its roots.

pub(crate) struct Free {
    pub(crate) close_mutex: unsafe fn(*mut crate::TaskCtx, u64, u64, u64) -> u8,
    pub(crate) close_endpoint: unsafe fn(*mut crate::TaskCtx, u64, u64, u64) -> u8,
    pub(crate) finalize: unsafe fn(*mut crate::TaskCtx, u32, *mut u8) -> Option<u8>,
    pub(crate) root_drop: unsafe fn(*mut crate::TaskCtx, *mut crate::Header) -> Option<u8>,
    pub(crate) release: unsafe fn(*mut crate::TaskCtx, *mut crate::Header) -> Option<u8>,
    pub(crate) issue: unsafe fn(
        *mut crate::TaskCtx,
        crate::io::Request,
    ) -> crate::stop::Stopped<(
        Result<crate::io::Completion, crate::io::AdapterError>,
        bool,
    )>,
}

pub(crate) struct Script {
    pub(crate) new: fn(
        Vec<hirsel_run_schema::NetPeer>,
        Vec<hirsel_run_schema::NetListener>,
    ) -> Result<crate::io_net_script::ScriptNet, crate::io::AdapterError>,
}

pub(crate) struct Telemetry {
    pub(crate) observe_span: fn(
        &mut crate::tasks::free::host::native::SpanState,
        &crate::io::Request,
        &Result<crate::io::Completion, crate::io::AdapterError>,
    ) -> crate::stop::Stopped<()>,
    pub(crate) draw: fn(
        &mut crate::io::State,
        &crate::io::Request,
    ) -> Result<crate::io::Completion, crate::io::AdapterError>,
    pub(crate) context: fn(&crate::io::State, u64, u64) -> Result<Vec<u8>, crate::io::AdapterError>,
    pub(crate) execute: fn(
        &mut crate::io::State,
        &crate::io::Request,
    ) -> Result<crate::io::Completion, crate::io::AdapterError>,
    pub(crate) sample: fn(
        &mut crate::io::State,
        &crate::io::Request,
        crate::io::Completion,
    ) -> Result<crate::io::Completion, crate::io::AdapterError>,
}

macro_rules! provider {
    ($get:ident, $ty:ty, $symbol:literal, $provider:literal) => {
        core::arch::global_asm!(
            concat!(".pushsection .data.rel.ro.", $symbol, ",\"aw\",%progbits"),
            concat!(".weak ", $symbol),
            concat!(".hidden ", $symbol),
            concat!(".type ", $symbol, ", %object"),
            ".p2align 3",
            concat!($symbol, ":"),
            concat!(".quad ", $provider),
            concat!(".size ", $symbol, ", 8"),
            ".popsection",
        );
        pub(crate) fn $get() -> Option<&'static $ty> {
            unsafe extern "C" {
                #[link_name = $symbol]
                static ROOT: *const std::ffi::c_void;
            }
            // SAFETY: the compiler or embedding default defines an immutable
            // null or process-lifetime provider pointer.
            unsafe { ROOT.cast::<$ty>().as_ref() }
        }
    };
}
provider!(
    free,
    Free,
    "hirsel.runtime.free",
    "hirsel.runtime.free.provider"
);
provider!(
    script,
    Script,
    "hirsel.runtime.script",
    "hirsel.runtime.script.provider"
);
provider!(
    telemetry,
    Telemetry,
    "hirsel.runtime.telemetry",
    "hirsel.runtime.telemetry.provider"
);

core::arch::global_asm!(
    ".pushsection .data.rel.ro.hirsel_runtime_runners,\"aw\",%progbits",
    ".weak hirsel.runtime.runners",
    ".hidden hirsel.runtime.runners",
    ".type hirsel.runtime.runners, %object",
    ".p2align 3",
    "hirsel.runtime.runners:",
    ".quad hirsel.runtime.runners.provider",
    ".size hirsel.runtime.runners, 8",
    ".popsection",
    ".pushsection .rodata.hirsel_runtime_tasks,\"a\",%progbits",
    ".weak hirsel.runtime.tasks",
    ".hidden hirsel.runtime.tasks",
    ".type hirsel.runtime.tasks, %object",
    "hirsel.runtime.tasks:",
    ".byte 1",
    ".size hirsel.runtime.tasks, 1",
    ".popsection",
);

pub(crate) fn runners() -> Option<&'static crate::runners::Launch> {
    unsafe extern "C" {
        #[link_name = "hirsel.runtime.runners"]
        static ROOT: *const std::ffi::c_void;
    }
    // SAFETY: the linker binds the immutable root to null or the runtime's
    // runner provider. Both have process lifetime.
    unsafe { ROOT.cast::<crate::runners::Launch>().as_ref() }
}

pub(crate) fn tasks() -> bool {
    unsafe extern "C" {
        #[link_name = "hirsel.runtime.tasks"]
        static ROOT: u8;
    }
    // SAFETY: the compiler or embedding default defines one immutable byte.
    unsafe { ROOT != 0 }
}

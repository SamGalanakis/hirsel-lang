//! P8: link the program's object with the runtime archive and a `main` that
//! hands the entry to `hirsel_rt_v2_launch`, then run it with explicit grants.
use super::emit::{Backend, check_runtime_calls, failure_rules};
use super::observe::{Observation, Sites, site_codes};
use super::stages::{NativeError, ReadyProgram, StageName};
use hirsel_lir::abi_types::{ParameterPassing, PhysicalType};
use hirsel_lir::emission::OptLevel;
use hirsel_lir::layout_types::{SizeSpec, TargetSpec};
use hirsel_syntax::DefPath;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

/// The external tools and archives linking and launching use. Nothing here is
/// discovered ambiently at link time: a missing tool is a `NativeError`.
#[derive(Clone, Debug)]
pub struct Toolchain {
    /// The archive variant `link` links; measurement also selects emitted
    /// inline-edge hooks. `with_runtime` selects another built variant.
    pub runtime_build: RuntimeBuild,
    /// The pinned clang, which compiles C test harnesses. No link runs it.
    pub clang: PathBuf,
    /// The AArch64 cross compiler driver, which links the Cargo-built AArch64
    /// runtime's build scripts and C test harnesses. No program link runs it.
    pub aarch64_cc: Option<PathBuf>,
    /// The pinned `ld.lld`, which links every executable and program library
    /// with the kit's line (`native/kit.rs`).
    pub lld: PathBuf,
    /// The link kit (`HIRSEL_LINK_KIT`): every link input besides the program,
    /// its runtime archive and foreign libraries.
    pub kit: PathBuf,
    /// Link the debug-bearing runtime archive and split each executable's debug
    /// information into `<build-id>.debug` beside it (`hirsel build --debug`).
    pub debug_info: bool,
    /// Launch executables (other than under a wrapper) through the glibc floor
    /// release's own loader and libraries in `<dir>/<triple>/`
    /// (`HIRSEL_GLIBC_FLOOR`) instead of the system's or the cross sysroot's.
    pub glibc_floor: Option<PathBuf>,
    /// The user-mode emulator AArch64 executables run under, and its sysroot.
    pub qemu_aarch64: Option<PathBuf>,
    pub aarch64_sysroot: Option<PathBuf>,
    /// The pinned memcheck executable and its relocated support directory.
    pub valgrind: Option<PathBuf>,
    pub valgrind_lib: Option<PathBuf>,
    /// Matching libc/loader and debug symbols for memcheck's mandatory redirects.
    pub valgrind_runtime: Option<PathBuf>,
    /// The debug-stripped `libhirsel_rt.a` per target triple and archive variant.
    pub runtime: BTreeMap<(String, RuntimeBuild), PathBuf>,
    /// The same archives with their debug information, which `debug_info` links read.
    pub runtime_debug: BTreeMap<(String, RuntimeBuild), PathBuf>,
    /// Per target triple, the default runtime archive's `compiler_builtins`
    /// members alone, which program libraries link.
    pub builtins: BTreeMap<String, PathBuf>,
    /// An x86-64 launch wrapper command and its arguments, or empty. Selecting
    /// the pinned Valgrind executable also enables its loader and environment.
    pub wrapper: Vec<String>,
    /// How long one launched process may run before it is killed and its
    /// launch fails.
    pub launch_timeout: Duration,
    /// Where runners and program images are kept (`native/runner.rs`).
    pub cache: PathBuf,
    /// How a program linked for a launch runs: in a runner unless an executable is selected.
    pub execution: super::runner::Execution,
}

/// A corpus program finishes in well under a second natively and in seconds
/// under qemu: a launch still running after this is a hang.
pub const LAUNCH_TIMEOUT: Duration = Duration::from_secs(120);

/// Independent link-time runtime choices. The default archive has neither measurement
/// hooks nor a system allocator override nor Valgrind client requests.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct RuntimeBuild {
    pub diagnostic: bool,
    pub system_allocator: bool,
    pub measurement: bool,
    /// Valgrind memcheck client requests for the runtime's own blocks, with every
    /// allocation, free and arena bump kept in the runtime.
    pub memcheck: bool,
}
impl RuntimeBuild {
    pub(crate) fn features(self) -> String {
        [
            self.diagnostic.then_some("diagnostic"),
            self.system_allocator.then_some("system-allocator"),
            self.measurement.then_some("measurement"),
            self.memcheck.then_some("memcheck"),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(",")
    }

    /// The directory name of this build's archive under `HIRSEL_RUNTIME_ARCHIVES`.
    pub(crate) fn variant(self) -> String {
        let features = self.features();
        if features.is_empty() {
            "default".into()
        } else {
            features.replace(',', "+")
        }
    }
}

fn tool(stage: StageName, command: &mut Command) -> Result<std::process::Output, NativeError> {
    command.output().map_err(|e| {
        NativeError::new(
            stage,
            format!("could not run {:?}: {e}", command.get_program()),
        )
    })
}

/// Run `command` in its own process group to completion, or kill the group
/// once `timeout` has passed and fail.
fn bounded(command: &mut Command, timeout: Duration) -> Result<std::process::Output, NativeError> {
    use std::os::unix::process::CommandExt;
    use std::process::Stdio;
    let child = command
        .process_group(0)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| {
            NativeError::new(
                StageName::Launch,
                format!("could not run {:?}: {e}", command.get_program()),
            )
        })?;
    finish(super::runner::Started::spawned(child), Some(timeout))
}

/// Wait for `started`, its own process group, to complete, or kill the group once `timeout`
/// has passed and fail.
fn finish(
    mut started: super::runner::Started,
    timeout: Option<Duration>,
) -> Result<std::process::Output, NativeError> {
    use std::io::Read;
    // Each pipe reaches its end when every process of the group has exited.
    let (sender, ended) = std::sync::mpsc::channel();
    let drain = |pipe: Option<Box<dyn Read + Send>>| {
        let sender = sender.clone();
        std::thread::spawn(move || {
            let mut bytes = Vec::new();
            if let Some(mut pipe) = pipe {
                let _ = pipe.read_to_end(&mut bytes);
            }
            let _ = sender.send(());
            bytes
        })
    };
    let stdout = drain(started.stdout.take());
    let stderr = drain(started.stderr.take());
    let deadline = timeout.map(|timeout| Instant::now() + timeout);
    let timed_out = (0..2).any(|_| match deadline {
        Some(deadline) => ended
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            .is_err(),
        None => ended.recv().is_err(),
    });
    let mut timeout_state = String::new();
    if timed_out {
        // The stat state distinguishes an executing child from a zombie
        // whose pipes are still held open. Do not reap it before kill: the
        // child's PID must remain reserved while we address its process group.
        timeout_state = format!(
            "stat {:?}; wchan {:?}",
            std::fs::read_to_string(format!("/proc/{}/stat", started.id)),
            std::fs::read_to_string(format!("/proc/{}/wchan", started.id)),
        );
        // SAFETY: kill(2) on the child's own process group.
        unsafe { libc::kill(-(started.id as libc::pid_t), libc::SIGKILL) };
    }
    let status = started
        .wait()
        .map_err(|e| NativeError::new(StageName::Launch, format!("waiting for a launch: {e}")))?;
    let stdout = stdout.join().unwrap_or_default();
    let stderr = stderr.join().unwrap_or_default();
    if let (true, Some(timeout)) = (timed_out, timeout) {
        return Err(NativeError::new(
            StageName::Launch,
            format!(
                "the process ran past the {} s launch timeout and was killed (stderr {:?}); {timeout_state}",
                timeout.as_secs_f64(),
                String::from_utf8_lossy(&stderr)
            ),
        ));
    }
    Ok(std::process::Output {
        status,
        stdout,
        stderr,
    })
}

pub(crate) fn checked(stage: StageName, command: &mut Command) -> Result<(), NativeError> {
    let output = tool(stage, command)?;
    if output.status.success() {
        return Ok(());
    }
    Err(NativeError::new(
        stage,
        format!(
            "{:?} {:?} failed with {}: {}{}",
            command.get_program(),
            command.get_args().collect::<Vec<_>>(),
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        ),
    ))
}

impl Toolchain {
    /// The toolchain of a source checkout: the pinned LLVM prefix
    /// (`LLVM_SYS_211_PREFIX`), explicit cross-tool inputs, and the runtime
    /// archive of each listed target: Buck2's under `HIRSEL_RUNTIME_ARCHIVES`, or
    /// built by Cargo with aborting panics into `build_dir`.
    pub fn in_tree(build_dir: &Path, targets: &[&TargetSpec]) -> Result<Self, NativeError> {
        Self::in_tree_with_runtime(build_dir, targets, RuntimeBuild::default())
    }

    /// Correctness archives retain the storage/leak oracle even when poisoning is disabled
    /// at launch. These archives are never benchmark timing inputs.
    pub fn in_tree_diagnostic(
        build_dir: &Path,
        targets: &[&TargetSpec],
    ) -> Result<Self, NativeError> {
        Self::in_tree_with_runtime(
            build_dir,
            targets,
            RuntimeBuild {
                diagnostic: true,
                ..Default::default()
            },
        )
    }

    pub fn in_tree_with_runtime(
        build_dir: &Path,
        targets: &[&TargetSpec],
        runtime_build: RuntimeBuild,
    ) -> Result<Self, NativeError> {
        Self::in_tree_with_runtimes(build_dir, targets, &[runtime_build])
    }

    /// The toolchain with every listed archive variant built for every
    /// target; the first is selected.
    pub fn in_tree_with_runtimes(
        build_dir: &Path,
        targets: &[&TargetSpec],
        runtime_builds: &[RuntimeBuild],
    ) -> Result<Self, NativeError> {
        Self::in_tree_inputs(build_dir, targets, runtime_builds, None)
    }

    /// Construct a toolchain against a resident session's frozen Cargo inputs.
    pub fn in_tree_with_inputs(
        build_dir: &Path,
        targets: &[&TargetSpec],
        runtime_builds: &[RuntimeBuild],
        inputs: &super::RuntimeInputs,
    ) -> Result<Self, NativeError> {
        Self::in_tree_inputs(build_dir, targets, runtime_builds, Some(inputs))
    }

    fn in_tree_inputs(
        build_dir: &Path,
        targets: &[&TargetSpec],
        runtime_builds: &[RuntimeBuild],
        inputs: Option<&super::RuntimeInputs>,
    ) -> Result<Self, NativeError> {
        let _timing = hirsel_intern::timings::interval(hirsel_intern::timings::Interval::Link);
        let [runtime_build, ..] = *runtime_builds else {
            return Err(NativeError::new(
                StageName::Link,
                "a toolchain needs a runtime archive",
            ));
        };
        let prefix = environment_path("LLVM_SYS_211_PREFIX")?;
        let needs_aarch64 = targets.iter().any(|target| aarch64(target));
        let needs_memcheck = runtime_builds.iter().any(|build| build.memcheck);
        // No program link runs the cross driver: it is an input only where configured.
        let aarch64_cc = selected_path(
            "HIRSEL_AARCH64_CC",
            needs_aarch64 && std::env::var_os("HIRSEL_AARCH64_CC").is_some(),
        )?;
        let qemu_aarch64 = selected_path("HIRSEL_QEMU_AARCH64", needs_aarch64)?;
        let aarch64_sysroot = selected_path("HIRSEL_AARCH64_SYSROOT", needs_aarch64)?;
        let valgrind = selected_path("HIRSEL_VALGRIND", needs_memcheck)?;
        let valgrind_lib = selected_path("VALGRIND_LIB", needs_memcheck)?;
        let valgrind_runtime = selected_path("HIRSEL_VALGRIND_RUNTIME", needs_memcheck)?;
        // A build that makes the runtime archives itself (Buck2, tools/buck2/README.md)
        // names their directory: `<dir>/<triple>/<features or default>/libhirsel_rt.a`.
        // Cargo is then never run, and a missing archive is an error.
        let prebuilt = std::env::var_os("HIRSEL_RUNTIME_ARCHIVES").map(PathBuf::from);
        let mut runtime = BTreeMap::new();
        let mut runtime_debug = BTreeMap::new();
        let mut builtins = BTreeMap::new();
        for (target, &runtime_build) in targets
            .iter()
            .flat_map(|t| runtime_builds.iter().map(move |b| (t, b)))
        {
            if let Some(dir) = &prebuilt {
                let variant = dir.join(&target.triple).join(runtime_build.variant());
                let archive = variant.join("libhirsel_rt.a");
                if !archive.is_file() {
                    return Err(NativeError::new(
                        StageName::Link,
                        format!("no runtime archive {}", archive.display()),
                    ));
                }
                let key = (target.triple.clone(), runtime_build);
                runtime.insert(key.clone(), archive);
                runtime_debug.insert(key, variant.join("debug/libhirsel_rt.a"));
                builtins.insert(
                    target.triple.clone(),
                    dir.join(&target.triple).join("libhirsel_builtins.a"),
                );
                continue;
            }
            let archive = super::runtime_cache::archive(
                inputs,
                build_dir,
                target,
                &prefix,
                runtime_build,
                |build_dir, environment| {
                    let cargo = environment
                        .get(std::ffi::OsStr::new("CARGO"))
                        .map_or(std::ffi::OsStr::new("cargo"), std::ffi::OsString::as_os_str);
                    let mut command = Command::new(cargo);
                    command
                        .env_clear()
                        .envs(environment)
                        .current_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."))
                        .args(["build", "--quiet", "--release", "-p", "hirsel-rt", "--lib"])
                        .arg("--manifest-path")
                        .arg(concat!(env!("CARGO_MANIFEST_DIR"), "/../../Cargo.toml"))
                        .arg("--target-dir")
                        .arg(build_dir)
                        .env("CARGO_PROFILE_RELEASE_PANIC", "abort");
                    let features = runtime_build.features();
                    if !features.is_empty() {
                        command.args(["--features", &features]);
                    }
                    let archive = if aarch64(target) {
                        command.args(["--target", &target.triple]).env(
                            "CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER",
                            configured_path(
                                "HIRSEL_AARCH64_CC",
                                environment
                                    .get(std::ffi::OsStr::new("HIRSEL_AARCH64_CC"))
                                    .cloned(),
                            )?,
                        );
                        build_dir
                            .join(&target.triple)
                            .join("release/libhirsel_rt.a")
                    } else {
                        build_dir.join("release/libhirsel_rt.a")
                    };
                    checked(StageName::Link, &mut command)?;
                    Ok(archive)
                },
            )?;
            // Cargo builds the debug-bearing archive; derive the link forms Buck
            // builds beside it (tools/buck2/runtime_parts.py).
            let stripped =
                super::runtime_cache::derived(build_dir, &archive, "stripped", |scratch| {
                    let stripped = scratch.join("libhirsel_rt.a");
                    checked(
                        StageName::Link,
                        llvm_tool(&prefix, "llvm-objcopy")
                            .arg("--strip-debug")
                            .args([&archive, &stripped]),
                    )?;
                    Ok(stripped)
                })?;
            if runtime_build == RuntimeBuild::default() || !builtins.contains_key(&target.triple) {
                let derived =
                    super::runtime_cache::derived(build_dir, &archive, "builtins", |scratch| {
                        compiler_builtins(&prefix, &stripped, scratch)
                    })?;
                builtins.insert(target.triple.clone(), derived);
            }
            let key = (target.triple.clone(), runtime_build);
            runtime.insert(key.clone(), stripped);
            runtime_debug.insert(key, archive);
        }
        Ok(Self {
            runtime_build,
            clang: prefix.join("bin/hirsel-clang"),
            aarch64_cc,
            lld: prefix.join("bin/ld.lld"),
            kit: environment_path("HIRSEL_LINK_KIT")?,
            debug_info: false,
            glibc_floor: None,
            qemu_aarch64,
            aarch64_sysroot,
            valgrind,
            valgrind_lib,
            valgrind_runtime,
            runtime,
            runtime_debug,
            builtins,
            wrapper: Vec::new(),
            launch_timeout: LAUNCH_TIMEOUT,
            cache: build_dir.to_path_buf(),
            execution: Default::default(),
        })
    }

    /// This toolchain linking `runtime_build`'s archive, which must be one it
    /// built.
    pub fn with_runtime(&self, runtime_build: RuntimeBuild) -> Self {
        Self {
            runtime_build,
            ..self.clone()
        }
    }

    /// The selected (debug-stripped) archive for `triple`.
    pub fn archive(&self, triple: &str) -> Option<&Path> {
        self.runtime
            .get(&(triple.to_string(), self.runtime_build))
            .map(PathBuf::as_path)
    }

    /// The selected archive with its debug information.
    fn debug_archive(&self, triple: &str) -> Option<&Path> {
        self.runtime_debug
            .get(&(triple.to_string(), self.runtime_build))
            .map(PathBuf::as_path)
    }

    /// `triple`'s compiler builtins alone, which program images link.
    pub(crate) fn builtins_archive(&self, triple: &str) -> Result<&Path, NativeError> {
        self.builtins
            .get(triple)
            .map(PathBuf::as_path)
            .ok_or_else(|| {
                NativeError::new(
                    StageName::Link,
                    format!("no compiler builtins archive for {triple}"),
                )
            })
    }
}

/// `tool` from the LLVM tree at `prefix`, with an environment holding only its
/// library directory.
fn llvm_tool(prefix: &Path, tool: &str) -> Command {
    let mut command = Command::new(prefix.join("bin").join(tool));
    command
        .env_clear()
        .env("LD_LIBRARY_PATH", prefix.join("lib"));
    command
}

/// `stripped`'s `compiler_builtins` members, in archive order, as an archive in
/// `scratch` (tools/buck2/runtime_parts.py).
fn compiler_builtins(
    prefix: &Path,
    stripped: &Path,
    scratch: &Path,
) -> Result<PathBuf, NativeError> {
    let listed = llvm_tool(prefix, "llvm-ar")
        .arg("t")
        .arg(stripped)
        .output()
        .map_err(|error| NativeError::new(StageName::Link, format!("llvm-ar: {error}")))?;
    if !listed.status.success() {
        return Err(NativeError::new(
            StageName::Link,
            format!(
                "llvm-ar t {}: {}",
                stripped.display(),
                String::from_utf8_lossy(&listed.stderr)
            ),
        ));
    }
    let members: Vec<&str> = std::str::from_utf8(&listed.stdout)
        .unwrap_or_default()
        .lines()
        .filter(|member| member.starts_with("compiler_builtins-"))
        .collect();
    let unique: std::collections::BTreeSet<_> = members.iter().collect();
    if members.is_empty() || unique.len() != members.len() {
        return Err(NativeError::new(
            StageName::Link,
            format!(
                "{}: no or duplicate compiler_builtins members",
                stripped.display()
            ),
        ));
    }
    let members_dir = scratch.join("members");
    let _ = std::fs::remove_dir_all(&members_dir);
    std::fs::create_dir_all(&members_dir)
        .map_err(|error| NativeError::new(StageName::Link, error.to_string()))?;
    let mut output = std::ffi::OsString::from("--output=");
    output.push(&members_dir);
    checked(
        StageName::Link,
        llvm_tool(prefix, "llvm-ar")
            .arg("x")
            .arg(output)
            .arg(stripped)
            .args(&members),
    )?;
    let builtins = scratch.join("libhirsel_builtins.a");
    let _ = std::fs::remove_file(&builtins);
    checked(
        StageName::Link,
        llvm_tool(prefix, "llvm-ar")
            .current_dir(&members_dir)
            .arg("rcsD")
            .arg(&builtins)
            .args(&members),
    )?;
    Ok(builtins)
}

/// Tools are explicit action inputs. Bare executable names would search libc's
/// default PATH after launch clears the environment, escaping the selected tree.
pub(super) fn environment_path(name: &str) -> Result<PathBuf, NativeError> {
    configured_path(name, std::env::var_os(name))
}

fn selected_path(name: &str, required: bool) -> Result<Option<PathBuf>, NativeError> {
    required.then(|| environment_path(name)).transpose()
}

fn required_path<'a>(path: &'a Option<PathBuf>, name: &str) -> Result<&'a Path, NativeError> {
    path.as_deref().ok_or_else(|| {
        NativeError::new(
            StageName::Link,
            format!("toolchain was not configured for {name}"),
        )
    })
}

pub(super) fn configured_path(
    name: &str,
    value: Option<std::ffi::OsString>,
) -> Result<PathBuf, NativeError> {
    let path = value.map(PathBuf::from).ok_or_else(|| {
        NativeError::new(
            StageName::Link,
            format!("{name} must name an absolute toolchain input"),
        )
    })?;
    if !path.is_absolute() {
        return Err(NativeError::new(
            StageName::Link,
            format!("{name} is not absolute: {}", path.display()),
        ));
    }
    Ok(path)
}

fn aarch64(target: &TargetSpec) -> bool {
    target.triple.starts_with("aarch64")
}

/// Validated native entry roots, preserving source parameter order.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum LaunchKind {
    Zero,
    Main(Vec<hirsel_run_config::Root>),
    Test,
}
impl LaunchKind {
    pub(crate) fn roots(&self) -> &[hirsel_run_config::Root] {
        match self {
            Self::Zero => &[],
            Self::Main(roots) => roots,
            Self::Test => &[hirsel_run_config::Root::Test],
        }
    }
    fn mask(&self) -> u8 {
        self.roots()
            .iter()
            .fold(0, |mask, root| mask | (1 << root_index(*root)))
    }
}
pub(crate) fn root_index(root: hirsel_run_config::Root) -> usize {
    use hirsel_run_config::Root;
    match root {
        Root::Io => 0,
        Root::Args => 1,
        Root::Env => 2,
        Root::Unsafe => 3,
        Root::Test => 4,
    }
}
pub(crate) fn root_handle(root: hirsel_run_config::Root) -> u64 {
    [
        hirsel_rt::launch::IO_HANDLE,
        hirsel_rt::launch::ARGS_HANDLE,
        u64::MAX,
        0,
        hirsel_rt::launch::IO_HANDLE,
    ][root_index(root)]
}
pub(crate) fn launch_kind(signature: &hirsel_types::Signature) -> Option<LaunchKind> {
    use hirsel_run_config::LaunchKind as Kind;
    if hirsel_engine::entry_launch_kind(signature).ok()? == Kind::Test {
        return Some(LaunchKind::Test);
    }
    let roots = hirsel_engine::entry_roots(signature, Kind::Main).ok()?;
    Some(if roots.is_empty() {
        LaunchKind::Zero
    } else {
        LaunchKind::Main(roots)
    })
}
fn entry_result(
    program: &ReadyProgram,
    entry: &DefPath,
) -> Result<(String, u64, bool), NativeError> {
    let refuse = |what: &str| NativeError {
        definition: Some(entry.clone()),
        ..NativeError::new(StageName::Launch, format!("the entry contract: {what}"))
    };
    let instance = program
        .functions
        .get(entry)
        .ok_or_else(|| refuse("no such non-generic function"))?;
    let unit = program
        .units
        .iter()
        .map(|u| u.unit())
        .find(|u| u.header.definition == *entry && u.header.instance == *instance)
        .ok_or_else(|| refuse("no unit"))?;
    let abi = &program.table.fn_abis[unit.header.abi.0 as usize];
    let handle = |p: &hirsel_lir::abi_types::AbiParameter| {
        matches!(
            p.passing,
            ParameterPassing::ReadOnlyAddress
                | ParameterPassing::TransferAddress
                | ParameterPassing::ExclusiveAddress
        ) && p.slots.len() == 1
            && p.slots[0].ty == PhysicalType::Pointer
    };
    let kind = program
        .launches
        .get(entry)
        .ok_or_else(|| refuse("no typed launch kind"))?;
    if !abi.context || abi.environment || !abi.destination {
        return Err(refuse(
            "hidden arguments other than context and destination",
        ));
    }
    let parameter_count = kind.roots().len();
    if abi.parameters.len() != parameter_count || !abi.parameters.iter().all(handle) {
        return Err(refuse(
            "parameters do not match the selected root capabilities",
        ));
    }
    let size = abi
        .result
        .as_ref()
        .and_then(|r| program.table.layout(r.layout))
        .and_then(|l| match l.size {
            SizeSpec::Fixed(n) => Some(n),
            SizeSpec::Dynamic(_) => None,
        })
        .filter(|n| (4..=hirsel_rt::launch::RESULT_MAX).contains(n))
        .ok_or_else(|| refuse("a result that is not an inline Result"))?;
    Ok((
        hirsel_lir::emission::symbol_name(entry, Some(instance)),
        size,
        abi.caller_site,
    ))
}

/// How a program's units are divided into objects (PROG-5): the division is
/// a build topology, and no observation may depend on it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Partition {
    /// One object for the whole program.
    Whole,
    /// One object per unit.
    Singleton,
    /// Units assigned to two to five objects by this seed.
    Random(u64),
}

/// SplitMix64's finalizer: explicit, wrapping, independent of host hashing.
fn mix(mut z: u64) -> u64 {
    z = z.wrapping_add(0x9e37_79b9_7f4a_7c15);
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

impl Partition {
    pub fn name(self) -> String {
        match self {
            Self::Whole => "whole".into(),
            Self::Singleton => "singleton".into(),
            Self::Random(seed) => format!("random-{seed}"),
        }
    }

    /// The object each of `units` units goes to, by unit index.
    pub fn assign(self, units: usize) -> Vec<usize> {
        match self {
            Self::Whole => vec![0; units],
            Self::Singleton => (0..units).collect(),
            Self::Random(seed) => {
                let objects = 2 + mix(seed) % 4;
                (0..units as u64)
                    .map(|unit| (mix(seed ^ mix(unit + 1)) % objects) as usize)
                    .collect()
            }
        }
    }
}

/// The most LIR instructions a source unit may have and still be copied into
/// the objects that call it. LIR is verbose: `step` (slots-churn) and `event`
/// (resource-failure) are 85, `whitespace` (wordfreq) 168, `destination_digest`
/// 185 and `decimal` (the benchmarks' parser) 257, and whole inlines each. LLVM's
/// own O3 cost still decides whether a copied body inlines; the bound only
/// limits the extra code each object compiles.
pub const SMALL_UNIT_INSTRUCTIONS: usize = 512;

/// The objects of `partition`, one per group of units it assigns, in group
/// order. Library templates are in every object, and known source callable
/// bodies only in their transitive calling objects, both `linkonce_odr`. A
/// small source unit (`SMALL_UNIT_INSTRUCTIONS`) is also an
/// `available_externally` body in every object whose units call it, so with
/// optimization whether a call inlines does not depend on the partition; the
/// unit's own object still implements and exports it. An object's code thus
/// depends on the small callee bodies it copies as well as on its own units.
/// At `OptLevel::None` nothing inlines and nothing is copied.
pub fn partition_objects(
    program: &ReadyProgram,
    opt: OptLevel,
    partition: Partition,
    registry: &hirsel_lir::descriptor_registry::DescriptorRegistry,
    measurement: bool,
    sources: Option<&hirsel_lir::profile::ProfileSources>,
) -> Result<Vec<hirsel_lir::stage::ObjectArtifact>, NativeError> {
    partition_backend_objects(
        program,
        Backend::Llvm,
        opt,
        partition,
        registry,
        measurement,
        sources,
    )
}

/// `partition_objects` by `backend`. Cranelift does not inline, so it copies no small
/// callee bodies at any optimization level.
pub fn partition_backend_objects(
    program: &ReadyProgram,
    backend: Backend,
    opt: OptLevel,
    partition: Partition,
    registry: &hirsel_lir::descriptor_registry::DescriptorRegistry,
    measurement: bool,
    sources: Option<&hirsel_lir::profile::ProfileSources>,
) -> Result<Vec<hirsel_lir::stage::ObjectArtifact>, NativeError> {
    use hirsel_lir::ir::{Callee, Inst, UnitOrigin};
    let _timing = hirsel_intern::timings::phase(hirsel_intern::timings::Phase::Codegen);
    let assignment = partition.assign(program.units.len());
    let mut groups: Vec<usize> = assignment.clone();
    groups.sort_unstable();
    groups.dedup();
    let library: Vec<bool> = program
        .units
        .iter()
        .map(|u| u.unit().header.origin == UnitOrigin::Library)
        .collect();
    let inline: Vec<bool> = program
        .units
        .iter()
        .map(|u| program.inline_instances.contains(&u.unit().header.instance))
        .collect();
    let small: Vec<bool> = program
        .units
        .iter()
        .enumerate()
        .map(|(i, u)| {
            backend == Backend::Llvm
                && opt != OptLevel::None
                && !library[i]
                && !inline[i]
                && u.unit().instructions.len() <= SMALL_UNIT_INSTRUCTIONS
        })
        .collect();
    let units: BTreeMap<_, _> = program
        .units
        .iter()
        .enumerate()
        .map(|(i, u)| ((&u.unit().header.definition, u.unit().header.instance), i))
        .collect();
    let unit_of = |u: &hirsel_lir::ir::LirUnit<hirsel_lir::stage::Ready>,
                   s: hirsel_lir::ir::SymbolId| {
        let symbol = &u.tables.symbols[s.index()];
        units.get(&(&symbol.definition, symbol.instance?)).copied()
    };
    // The units each unit calls, and those it calls or refers to by FunctionRef.
    let calls: Vec<Vec<usize>> = program
        .units
        .iter()
        .map(|u| {
            let u = u.unit();
            u.tables
                .calls
                .iter()
                .filter_map(|c| match c.callee {
                    Callee::Direct(s) | Callee::Bound { code: s, .. } => unit_of(u, s),
                    _ => None,
                })
                .collect()
        })
        .collect();
    let references: Vec<Vec<usize>> = program
        .units
        .iter()
        .enumerate()
        .map(|(i, u)| {
            let u = u.unit();
            let refs = u.instructions.iter().filter_map(|inst| match inst {
                Inst::FunctionRef { symbol, .. } => unit_of(u, *symbol),
                _ => None,
            });
            calls[i].iter().copied().chain(refs).collect()
        })
        .collect();
    groups
        .iter()
        .map(|group| {
            let own: Vec<bool> = (0..program.units.len())
                .map(|i| library[i] || assignment[i] == *group)
                .collect();
            let mut selected = own.clone();
            // Small callees, transitively through small callees.
            let mut copies = std::collections::BTreeSet::new();
            let mut pending: Vec<usize> = (0..own.len()).filter(|&i| own[i]).collect();
            while let Some(i) = pending.pop() {
                for &callee in &calls[i] {
                    if small[callee] && !std::mem::replace(&mut selected[callee], true) {
                        copies.insert(program.units[callee].unit().header.instance);
                        pending.push(callee);
                    }
                }
            }
            // Known callable bodies, transitively, from every unit emitted.
            let mut pending: Vec<usize> = (0..selected.len()).filter(|&i| selected[i]).collect();
            while let Some(i) = pending.pop() {
                for &dependency in &references[i] {
                    if inline[dependency] && !std::mem::replace(&mut selected[dependency], true) {
                        pending.push(dependency);
                    }
                }
            }
            super::emit::emit_registered(
                program,
                backend,
                opt,
                |unit| selected[unit],
                &copies,
                registry,
                measurement,
                sources,
            )
        })
        .collect()
}

/// Emit `program` as the objects `partition` names, check the link manifest
/// and link them with `entry` as the launch entry. The executable is
/// `dir/program`.
pub fn link(
    program: &ReadyProgram,
    entry: &DefPath,
    opt: OptLevel,
    partition: Partition,
    toolchain: &Toolchain,
    dir: &Path,
) -> Result<PathBuf, NativeError> {
    link_profile(program, entry, opt, partition, toolchain, dir, None)
}

/// Link with optional source line tables. Measurement archives additionally tag call sites.
pub fn link_profile(
    program: &ReadyProgram,
    entry: &DefPath,
    opt: OptLevel,
    partition: Partition,
    toolchain: &Toolchain,
    dir: &Path,
    sources: Option<&hirsel_lir::profile::ProfileSources>,
) -> Result<PathBuf, NativeError> {
    let objects = emit_objects(
        program,
        opt,
        partition,
        toolchain.runtime_build.measurement,
        sources,
    )?;
    link_objects(program, entry, &objects, toolchain, dir)
}

/// The objects of `partition` at `opt`, as `link_profile` links them. They do
/// not depend on the runtime archive, except that `measurement` objects tag
/// call sites for the measurement archive.
pub fn emit_objects(
    program: &ReadyProgram,
    opt: OptLevel,
    partition: Partition,
    measurement: bool,
    sources: Option<&hirsel_lir::profile::ProfileSources>,
) -> Result<Vec<hirsel_lir::stage::ObjectArtifact>, NativeError> {
    emit_backend_objects(program, Backend::Llvm, opt, partition, measurement, sources)
}

/// `emit_objects` by `backend`. The objects name their backend, and linking them
/// compiles the launcher with the same one.
pub fn emit_backend_objects(
    program: &ReadyProgram,
    backend: Backend,
    opt: OptLevel,
    partition: Partition,
    measurement: bool,
    sources: Option<&hirsel_lir::profile::ProfileSources>,
) -> Result<Vec<hirsel_lir::stage::ObjectArtifact>, NativeError> {
    let _interval = hirsel_intern::timings::interval(hirsel_intern::timings::Interval::Units);
    let _timing = hirsel_intern::timings::phase(hirsel_intern::timings::Phase::Codegen);
    check_runtime_calls(program)?;
    let registry = hirsel_lir::descriptor_registry::DescriptorRegistry::new(
        &program.table,
        &program.finalizers,
    );
    partition_backend_objects(
        program,
        backend,
        opt,
        partition,
        &registry,
        measurement,
        sources,
    )
}

/// Check the link manifest of `objects` (from `emit_objects`) and link them
/// with `entry` as the launch entry and the toolchain's selected archive. The
/// executable is `dir/program`.
pub fn link_objects(
    program: &ReadyProgram,
    entry: &DefPath,
    objects: &[hirsel_lir::stage::ObjectArtifact],
    toolchain: &Toolchain,
    dir: &Path,
) -> Result<PathBuf, NativeError> {
    link_objects_with_run(program, entry, objects, toolchain, dir, None)
}

pub(crate) fn link_objects_with_run(
    program: &ReadyProgram,
    entry: &DefPath,
    objects: &[hirsel_lir::stage::ObjectArtifact],
    toolchain: &Toolchain,
    dir: &Path,
    requested: Option<&hirsel_run_config::ValidatedRun>,
) -> Result<PathBuf, NativeError> {
    link_objects_impl(program, entry, objects, toolchain, dir, requested, false)
        .map(|linked| linked.path)
}

/// Link `objects` to launch with `entry` as the launch entry, bound to `requested` (or the
/// fixture run) as [`link_objects`] binds an executable: as the image of the toolchain's runner
/// (`native/runner.rs`), or, when the toolchain selects executables, the program needs one or
/// its image is refused, as the executable `dir/program`.
pub fn link_runnable(
    program: &ReadyProgram,
    entry: &DefPath,
    objects: &[hirsel_lir::stage::ObjectArtifact],
    toolchain: &Toolchain,
    dir: &Path,
    requested: Option<&hirsel_run_config::ValidatedRun>,
) -> Result<Runnable, NativeError> {
    if toolchain.execution == super::runner::Execution::Executable {
        return link_objects_with_run(program, entry, objects, toolchain, dir, requested)
            .map(Runnable::Executable);
    }
    let _timing = hirsel_intern::timings::interval(hirsel_intern::timings::Interval::Link);
    let runner = super::runner::runner(toolchain, &program.target)?;
    let main = launcher_object(program, entry, objects, dir, requested, false, "main")?;
    let main = write(dir, "main.o", &main)?;
    let linked = link_launcher(
        program,
        entry,
        objects,
        toolchain,
        dir,
        Output::Image(&runner),
        &[main],
    )?;
    Ok(linked.runnable())
}

/// A standalone executable `dir/program` and, for a `debug_info` toolchain, its
/// `<build-id>.debug` beside it.
pub(crate) fn link_standalone(
    program: &ReadyProgram,
    entry: &DefPath,
    objects: &[hirsel_lir::stage::ObjectArtifact],
    toolchain: &Toolchain,
    dir: &Path,
) -> Result<Linked, NativeError> {
    link_objects_impl(program, entry, objects, toolchain, dir, None, true)
}

/// A compiled program as an image a host process loads ([`link_host`]): the image and the run
/// record it binds.
#[derive(Clone, Debug)]
pub struct Library {
    pub path: PathBuf,
    pub run: PathBuf,
}

/// Link `objects` as the image of the toolchain's runner (`native/runner.rs`) whose `main`
/// runs `entry` once, without a runtime of its own, bound to `run` (or the fixture run) as
/// [`link_objects`] binds an executable; `dir/run.json` holds the bound run's record.
pub fn link_library(
    program: &ReadyProgram,
    entry: &DefPath,
    objects: &[hirsel_lir::stage::ObjectArtifact],
    toolchain: &Toolchain,
    dir: &Path,
    run: Option<&hirsel_run_config::ValidatedRun>,
) -> Result<Library, NativeError> {
    let main = launcher_object(program, entry, objects, dir, run, false, "main")?;
    let main = write(dir, "main.o", &main)?;
    let runner = super::runner::runner(toolchain, &program.target)?;
    let linked = link_launcher(
        program,
        entry,
        objects,
        toolchain,
        dir,
        Output::Image(&runner),
        &[main],
    )?;
    match linked.image {
        Some(image) => Ok(Library {
            path: image.path,
            run: dir.join("run.json"),
        }),
        None => Err(NativeError::new(
            StageName::Link,
            "the program links as an executable, not an image",
        )),
    }
}

/// Link a host executable in `dir` (`host`) holding one runtime, which loads each image named
/// by its arguments in turn into its own process, runs its `main` once with the run record
/// after it (`--hirsel-run <record>`) and prints `host: run <i> exited <status>` after it. Every
/// run ends at its host; none ends the process.
pub fn link_host(
    target: &TargetSpec,
    toolchain: &Toolchain,
    dir: &Path,
) -> Result<PathBuf, NativeError> {
    let archive = toolchain.archive(&target.triple).ok_or_else(|| {
        NativeError::new(
            StageName::Link,
            format!(
                "no {} runtime archive for {}",
                toolchain.runtime_build.variant(),
                target.triple
            ),
        )
    })?;
    // The libc declarations it uses, written out so the compile reads no
    // headers: glibc's RTLD_NOW is 2 and RTLD_LOCAL 0 on both targets.
    let source = "typedef struct _IO_FILE FILE;\n\
         extern FILE *stdout, *stderr;\n\
         void *dlopen(const char *, int);\nvoid *dlsym(void *, const char *);\n\
         char *dlerror(void);\nint fprintf(FILE *, const char *, ...);\n\
         int printf(const char *, ...);\nint fflush(FILE *);\n\
         int main(int argc, char **argv) {\n\
         for (int i = 1; i + 1 < argc; i += 2) {\n\
         void *image = dlopen(argv[i], 2);\n\
         int (*run)(int, const char *const *) = image\n\
           ? (int (*)(int, const char *const *))dlsym(image, \"main\") : 0;\n\
         if (!run) { fprintf(stderr, \"host: %s\\n\", dlerror()); return 3; }\n\
         const char *const arguments[] = {argv[i], \"--hirsel-run\", argv[i + 1]};\n\
         fflush(stdout);\n\
         int status = run(3, arguments);\n\
         printf(\"host: run %d exited %d\\n\", i / 2 + 1, status);\n\
         fflush(stdout);\n\
         }\n\
         return 0;\n}\n";
    let source_path = write(dir, "host.c", source.as_bytes())?;
    let object = dir.join("host.o");
    let target_option = format!("--target={}", target.triple);
    checked(
        StageName::Link,
        Command::new(toolchain.lld.with_file_name("clang"))
            .env_clear()
            .env(
                "LD_LIBRARY_PATH",
                toolchain
                    .lld
                    .parent()
                    .and_then(Path::parent)
                    .map_or_else(PathBuf::new, |t| t.join("lib")),
            )
            .args([target_option.as_str(), "-nostdinc", "-fPIC", "-O1", "-c"])
            .arg(&source_path)
            .arg("-o")
            .arg(&object),
    )?;
    let host = dir.join("host");
    let kit = super::kit::Kit::load(&toolchain.kit)?;
    let link = super::kit::Link {
        kit: &kit,
        triple: &target.triple,
        kind: super::kit::Kind::Host {
            exports: super::runner::export_manifest(
                archive,
                toolchain.builtins_archive(&target.triple)?,
            )?,
            main: None,
        },
        objects: &[object],
        runtime: Some(archive),
        unwind: true,
        foreign: &[],
    };
    let (mut command, _) = link.command(&toolchain.lld, &host)?;
    checked(StageName::Link, &mut command)?;
    Ok(host)
}

/// Run a [`link_host`] executable over `libraries`, bounded like every launch.
pub fn run_host(
    host: &Path,
    libraries: &[Library],
    toolchain: &Toolchain,
) -> Result<std::process::Output, NativeError> {
    let mut command = Command::new(host);
    command.env_clear();
    for library in libraries {
        command.arg(&library.path).arg(absolute(&library.run)?);
    }
    bounded(&mut command, toolchain.launch_timeout)
}

fn link_objects_impl(
    program: &ReadyProgram,
    entry: &DefPath,
    objects: &[hirsel_lir::stage::ObjectArtifact],
    toolchain: &Toolchain,
    dir: &Path,
    requested: Option<&hirsel_run_config::ValidatedRun>,
    standalone: bool,
) -> Result<Linked, NativeError> {
    let _timing = hirsel_intern::timings::interval(hirsel_intern::timings::Interval::Link);
    let main = launcher_object(program, entry, objects, dir, requested, standalone, "main")?;
    let main = write(dir, "main.o", &main)?;
    let output = if standalone {
        Output::Standalone
    } else {
        Output::Launched
    };
    link_launcher(program, entry, objects, toolchain, dir, output, &[main])
}

fn write(dir: &Path, name: &str, bytes: &[u8]) -> Result<PathBuf, NativeError> {
    let path = dir.join(name);
    std::fs::write(&path, bytes)
        .map(|()| path)
        .map_err(|e| NativeError::new(StageName::Link, format!("{name}: {e}")))
}

fn launcher_object(
    program: &ReadyProgram,
    entry: &DefPath,
    objects: &[hirsel_lir::stage::ObjectArtifact],
    dir: &Path,
    requested: Option<&hirsel_run_config::ValidatedRun>,
    standalone: bool,
    main_symbol: &str,
) -> Result<Vec<u8>, NativeError> {
    let (symbol, result_size, caller_site) = entry_result(program, entry)?;
    let registry = hirsel_lir::descriptor_registry::DescriptorRegistry::new(
        &program.table,
        &program.finalizers,
    );
    let target = &program.target;
    let kind = &program.launches[entry];
    let roots = kind.roots();
    let parameters = roots.iter().map(|_| ", ptr").collect::<String>();
    let mut loads = String::new();
    let mut arguments = String::new();
    for (index, root) in roots.iter().enumerate() {
        loads.push_str(&format!(
            " %root{index} = getelementptr i64, ptr %roots, i64 {}\n",
            root_index(*root)
        ));
        arguments.push_str(&format!(", ptr %root{index}"));
    }
    let report = program.exit_reports.get(entry).ok_or_else(|| {
        NativeError::new(StageName::Launch, "missing checked exit-report description")
    })?;
    let report_declarations = &report.declarations;
    let report_root = report.root;
    let report_offset = report.offset;
    let result_index = registry
        .index(report.result)
        .ok_or_else(|| NativeError::new(StageName::Launch, "missing entry-result descriptor"))?;
    let caller_parameters = if caller_site { ", i64, i64" } else { "" };
    let caller_arguments = if caller_site { ", i64 0, i64 0" } else { "" };
    let declaration = format!(
        r#"{report_declarations}
declare void @hirsel_rt_v2_exit_report(ptr, ptr, i32, i64, ptr)
declare i8 @"{symbol}"(ptr{caller_parameters}, ptr, ptr{parameters})
define private i8 @hirsel.entry(ptr %ctx, ptr %dest, ptr %out, ptr %roots) {{
{loads} %s = call i8 @"{symbol}"(ptr %ctx{caller_arguments}, ptr %dest, ptr %out{arguments})
 %ok = icmp eq i8 %s, 0
 br i1 %ok, label %returned, label %done
returned:
 %tag = load i32, ptr %out, align 1
 %error = icmp eq i32 %tag, 1
 br i1 %error, label %report, label %done
report:
 call void @hirsel_rt_v2_exit_report(ptr %ctx, ptr %out, i32 {result_index}, i64 {report_offset}, ptr @hirsel.report.node.{report_root})
 br label %done
done:
 ret i8 %s
}}"#
    );
    let roots_mask = kind.mask();
    let registry_declaration = format!(
        "@\"{}\" = external constant [{} x ptr]",
        registry.symbol(),
        registry.external_count()
    );
    let revision = program.revisions.get(entry).ok_or_else(|| {
        NativeError::new(
            StageName::Launch,
            "missing checked semantic program revision",
        )
    })?;
    let launch = if *kind == LaunchKind::Test {
        hirsel_run_config::LaunchKind::Test
    } else {
        hirsel_run_config::LaunchKind::Main
    };
    let mut record = hirsel_run_config::RunConfig::for_program(revision, launch);
    record.inputs = program.launch.clone();
    // The existing conformance launcher supplies this explicit fixture profile.
    // Standalone admission will consume the record at the C entry boundary.
    if matches!(kind, LaunchKind::Main(_)) && !standalone {
        use hirsel_run_config::{Access, IoGrants, Mount, ProviderContract};
        let root = std::fs::canonicalize(dir)
            .map_err(|error| NativeError::new(StageName::Link, error.to_string()))?
            .join("root");
        let host = root.into_os_string().into_string().map_err(|_| {
            NativeError::new(StageName::Launch, "fixture mount designator is not UTF-8")
        })?;
        record.io = Some(IoGrants {
            stdout: Some(ProviderContract::StdoutBytes.name().into()),
            random: Some(ProviderContract::RandomSplitMix64.name().into()),
            dirs: vec![Mount {
                path: String::new(),
                host,
                access: Access::ReadWrite,
            }],
            ..Default::default()
        });
        record.args = Some(
            hirsel_run_config::snapshot_args([])
                .map_err(|error| NativeError::new(StageName::Launch, error.to_string()))?,
        );
    }
    let default = record
        .validated()
        .map_err(|error| NativeError::new(StageName::Launch, error.to_string()))?;
    let run = requested.unwrap_or(&default);
    run.check_revision(revision, launch)
        .map_err(|e| NativeError::new(StageName::Launch, e.to_string()))?;
    if !standalone {
        std::fs::write(dir.join("run.json"), run.canonical())
            .map_err(|error| NativeError::new(StageName::Link, error.to_string()))?;
    }
    // A launched executable binds the one run it was built for; a standalone one takes fresh
    // grants at each launch.
    let config = (!standalone).then(|| run.config());
    let mut config_bytes = Vec::with_capacity(56);
    for seed in config.map_or([0; 3], |c| [c.hash_seed(), c.random_seed(), c.test_seed()]) {
        config_bytes.extend(seed.to_le_bytes());
    }
    config_bytes.extend(config.map_or([0; 32], |c| c.run_identity()));
    let mut flags = 0;
    if program.foreign_unreplayable {
        flags |= hirsel_rt::program::UNREPLAYABLE;
    }
    if program.escaping.contains(entry) {
        flags |= hirsel_rt::program::ESCAPES;
    }
    if program.fuel {
        flags |= hirsel_rt::program::FUELED;
    }
    if standalone {
        flags |= hirsel_rt::program::STANDALONE;
    }
    let table = failure_table(program)?;
    let name = revision.entry().as_bytes();
    // The launcher object comes from the backend that emitted the program's objects, so a
    // Cranelift build never runs LLVM.
    let main = if cranelift_objects(objects) {
        use hirsel_cranelift::DescriptorField as F;
        let descriptor = [
            F::U32(hirsel_rt::ABI_VERSION),
            F::U32(flags),
            F::U32(u32::from(roots_mask)),
            F::U32(registry.external_count()),
            F::Entry,
            F::U64(result_size),
            F::Symbol(registry.symbol()),
            F::Bytes(table.clone()),
            F::U64(table.len() as u64),
            F::Bytes(name.to_vec()),
            F::U64(name.len() as u64),
            F::Inline(revision.digest().to_vec()),
            F::Inline(config_bytes),
        ];
        let words: Vec<u64> = roots.iter().map(|r| root_index(*r) as u64).collect();
        let report = hirsel_cranelift::ExitReport {
            result_index,
            offset: report_offset,
            root: report_root,
            nodes: report.nodes.clone(),
        };
        hirsel_cranelift::named_launch_object(
            main_symbol,
            target,
            &symbol,
            caller_site,
            &words,
            &report,
            &descriptor,
        )
        .map_err(|e| NativeError::new(StageName::Link, e.to_string()))?
    } else {
        let bytes = |data: &[u8]| -> String {
            data.iter()
                .map(|b| format!("i8 {b}"))
                .collect::<Vec<_>>()
                .join(", ")
        };
        let text: String = table.iter().map(|b| format!("\\{b:02X}")).collect();
        let program_ir = format!(
            "@hirsel.failure.table = private constant [{table_len} x i8] c\"{text}\"\n\
             @hirsel.entry.name = private constant [{name_len} x i8] [{name_bytes}]\n\
             @hirsel.program = private constant {{ i32, i32, i32, i32, ptr, i64, ptr, ptr, i64, ptr, i64, [32 x i8], [56 x i8] }} \
             {{ i32 {abi}, i32 {flags}, i32 {roots_mask}, i32 {layout_count}, ptr @hirsel.entry, i64 {result_size}, \
             ptr @\"{registry_symbol}\", ptr @hirsel.failure.table, i64 {table_len}, ptr @hirsel.entry.name, i64 {name_len}, \
             [32 x i8] [{digest}], [56 x i8] [{config}] }}\n\
             declare i32 @hirsel_rt_v2_main(ptr, i32, ptr)\n",
            table_len = table.len(),
            name_len = name.len(),
            name_bytes = bytes(name),
            abi = hirsel_rt::ABI_VERSION,
            layout_count = registry.external_count(),
            registry_symbol = registry.symbol(),
            digest = bytes(&revision.digest()),
            config = bytes(&config_bytes),
        );
        let shim = format!(
            "target triple = \"{}\"\n{registry_declaration}\n{declaration}\n{program_ir}declare ptr @signal(i32, ptr)\ndefine i32 @{main_symbol}(i32 %argc, ptr %argv) {{\n %old_sigpipe = call ptr @signal(i32 13, ptr inttoptr (i64 1 to ptr))\n %r = call i32 @hirsel_rt_v2_main(ptr @hirsel.program, i32 %argc, ptr %argv)\n ret i32 %r\n}}\n",
            target.triple
        );
        hirsel_llvm::compile_ir(&shim, target, OptLevel::None)
            .map_err(|e| NativeError::new(StageName::Link, e.to_string()))?
            .bytes
    };
    Ok(main)
}

/// What a link produces.
#[derive(Clone, Copy)]
enum Output<'a> {
    /// An executable run for the run the driver bound (`program`).
    Launched,
    /// An executable that is its own host (`hirsel build`).
    Standalone,
    /// The image `runner` loads (`native/runner.rs`), or, when the program needs an
    /// executable or the image is refused, a launched executable.
    Image(&'a super::runner::Runner),
}

fn link_launcher(
    program: &ReadyProgram,
    entry: &DefPath,
    objects: &[hirsel_lir::stage::ObjectArtifact],
    toolchain: &Toolchain,
    dir: &Path,
    mut output: Output<'_>,
    launchers: &[PathBuf],
) -> Result<Linked, NativeError> {
    let (symbol, _, _) = entry_result(program, entry)?;
    let target = &program.target;
    let library_symbols: std::collections::BTreeSet<String> = program
        .units
        .iter()
        .filter(|u| {
            u.unit().header.origin == hirsel_lir::ir::UnitOrigin::Library
                || program.inline_instances.contains(&u.unit().header.instance)
        })
        .map(|u| {
            hirsel_lir::emission::symbol_name(
                &u.unit().header.definition,
                Some(&u.unit().header.instance),
            )
        })
        .collect();
    // Link manifest: each definition instance has exactly one implementation
    // across the objects, the entry among them, and every import is another
    // object's export, a runtime entry or a toolchain routine.
    let mut exported = std::collections::BTreeSet::new();
    for export in objects.iter().flat_map(|o| &o.exports) {
        if !exported.insert(export.name.as_str()) && !library_symbols.contains(&export.name) {
            return Err(NativeError::new(
                StageName::Link,
                format!("two objects implement `{}`", export.name),
            ));
        }
    }
    if !exported.contains(symbol.as_str()) {
        return Err(NativeError::new(
            StageName::Link,
            "no object exports the launch entry",
        ));
    }
    for import in objects.iter().flat_map(|o| &o.imports) {
        let declared = matches!(
            import.kind,
            hirsel_lir::stage::ObjectSymbolKind::Runtime
                | hirsel_lir::stage::ObjectSymbolKind::Foreign
        ) || exported.contains(import.name.as_str())
            || hirsel_lir::emission::TOOLCHAIN_IMPORTS.contains(&import.name.as_str());
        if !declared {
            return Err(NativeError::new(
                StageName::Link,
                format!("`{}` is imported and no object implements it", import.name),
            ));
        }
    }
    let archive = toolchain.archive(&target.triple).ok_or_else(|| {
        NativeError::new(
            StageName::Link,
            format!(
                "no {} runtime archive for {}",
                toolchain.runtime_build.variant(),
                target.triple
            ),
        )
    })?;
    let executable = dir.join("program");
    let kit = super::kit::Kit::load(&toolchain.kit)?;
    let mut inputs: Vec<PathBuf> = launchers.to_vec();
    for (index, object) in objects.iter().enumerate() {
        inputs.push(write(dir, &format!("unit-{index}.o"), &object.bytes)?);
    }
    if let Output::Image(runner) = output {
        let image = match executable_reason(program, objects, toolchain) {
            Some(reason) => Err(reason),
            None => super::runner::link_image(toolchain, target, runner, &inputs, dir)?,
        };
        match image {
            Ok(image) => {
                return Ok(Linked {
                    path: image.path.clone(),
                    debug: None,
                    image: Some(image),
                });
            }
            Err(reason) => {
                super::runner::count_fallback(&reason);
                output = Output::Launched;
            }
        }
    }
    // The externs resolve against the root's `unsafe ffi` libraries (PROG-4).
    let foreign = foreign_inputs(&kit, &target.triple, &program.foreign_links)?;
    let debug = toolchain.debug_info;
    let (kind, runtime) = match output {
        Output::Image(_) => unreachable!("an image links above"),
        Output::Standalone => (
            super::kit::Kind::Executable { loader: None },
            runtime_archive(toolchain, &target.triple, archive, debug)?,
        ),
        Output::Launched => (
            super::kit::Kind::Executable {
                loader: toolchain.loader(target)?,
            },
            runtime_archive(toolchain, &target.triple, archive, debug)?,
        ),
    };
    let link = super::kit::Link {
        kit: &kit,
        triple: &target.triple,
        kind,
        objects: &inputs,
        runtime: Some(runtime),
        unwind: true,
        foreign: &foreign,
    };
    let (mut command, build_id) = link.command(&toolchain.lld, &executable)?;
    checked(StageName::Link, &mut command)?;
    let debug = debug
        .then(|| split_debug(toolchain, &executable, &build_id))
        .transpose()?;
    Ok(Linked {
        path: executable,
        debug,
        image: None,
    })
}

/// Why `program` runs as an executable rather than an image (link design §2): it has a
/// foreign-call surface (an extern import, a C callback or a foreign link input, even an
/// `unsafe ffi` with no library calling libc), or it runs under memcheck.
fn executable_reason(
    program: &ReadyProgram,
    objects: &[hirsel_lir::stage::ObjectArtifact],
    toolchain: &Toolchain,
) -> Option<String> {
    let foreign = objects
        .iter()
        .flat_map(|o| &o.imports)
        .any(|i| i.kind == hirsel_lir::stage::ObjectSymbolKind::Foreign);
    if foreign || !program.callbacks.is_empty() || !program.foreign_links.is_empty() {
        return Some("foreign: the program calls C".into());
    }
    if toolchain.runtime_build.memcheck || toolchain.uses_memcheck() {
        return Some("memcheck: memcheck runs an executable".into());
    }
    None
}

/// A linked file and, for a `debug_info` toolchain, its split debug information; or the
/// program's image.
pub(crate) struct Linked {
    pub path: PathBuf,
    pub debug: Option<PathBuf>,
    pub image: Option<super::runner::Image>,
}

impl Linked {
    fn runnable(self) -> Runnable {
        match self.image {
            Some(image) => Runnable::Image(image),
            None => Runnable::Executable(self.path),
        }
    }
}

/// The archive a link reads: the debug-stripped one, or the debug-bearing one.
pub(crate) fn runtime_archive<'a>(
    toolchain: &'a Toolchain,
    triple: &str,
    stripped: &'a Path,
    debug: bool,
) -> Result<&'a Path, NativeError> {
    if !debug {
        return Ok(stripped);
    }
    toolchain.debug_archive(triple).ok_or_else(|| {
        NativeError::new(
            StageName::Link,
            format!("no debug runtime archive for {triple}"),
        )
    })
}

/// The file name of an executable's split debug information.
pub(crate) fn debug_file_name(build_id: &[u8; 20]) -> String {
    let hex: String = build_id.iter().map(|b| format!("{b:02x}")).collect();
    format!("{hex}.debug")
}

/// Move `executable`'s debug information into `<build-id>.debug` beside it,
/// which its `.gnu_debuglink` names (with the file's CRC), as debuggers look it up.
pub(crate) fn split_debug(
    toolchain: &Toolchain,
    executable: &Path,
    build_id: &[u8; 20],
) -> Result<PathBuf, NativeError> {
    let objcopy = toolchain.lld.with_file_name("llvm-objcopy");
    let debug = executable.with_file_name(debug_file_name(build_id));
    let run = |args: &[&std::ffi::OsStr]| {
        let mut command = Command::new(&objcopy);
        command.env_clear().args(args).current_dir(
            executable.parent().ok_or_else(|| {
                NativeError::new(StageName::Link, "an executable has no directory")
            })?,
        );
        if let Some(tree) = objcopy.parent().and_then(Path::parent) {
            command.env("LD_LIBRARY_PATH", tree.join("lib"));
        }
        checked(StageName::Link, &mut command)
    };
    // objcopy runs in the file's directory, so it names both files by their names alone,
    // which also keeps the link to the file name, never this directory.
    let (file, debug_name) = (executable.file_name().unwrap(), debug.file_name().unwrap());
    run(&["--only-keep-debug".as_ref(), file, debug_name])?;
    let mut link = std::ffi::OsString::from("--add-gnu-debuglink=");
    link.push(debug_name);
    run(&["--strip-debug".as_ref(), &link, file])?;
    Ok(debug)
}

/// The files `links` name, in order: a library the kit provides (`c`, `m`,
/// `pthread`, ...) is its stub, any other library the first `lib<name>.so` or
/// `lib<name>.a` in the host's system library directories, as `ld -l` searches
/// them. Host libraries are the one link input outside the kit; they enter the
/// build ID by content.
fn foreign_inputs(
    kit: &super::kit::Kit,
    triple: &str,
    links: &[hirsel_engine::LinkInput],
) -> Result<Vec<PathBuf>, NativeError> {
    let multiarch = triple.replace("-unknown-", "-");
    let directories = [
        format!("/usr/local/lib/{multiarch}"),
        format!("/lib/{multiarch}"),
        format!("/usr/lib/{multiarch}"),
        "/usr/local/lib".into(),
        "/lib64".into(),
        "/usr/lib64".into(),
        "/lib".into(),
        "/usr/lib".into(),
    ];
    links
        .iter()
        .map(|link| match link {
            hirsel_engine::LinkInput::File(path) => Ok(path.clone()),
            hirsel_engine::LinkInput::Library(name) => kit
                .library(triple, name)
                .or_else(|| {
                    directories.iter().find_map(|directory| {
                        ["so", "a"].iter().find_map(|suffix| {
                            let path = Path::new(directory).join(format!("lib{name}.{suffix}"));
                            path.is_file().then_some(path)
                        })
                    })
                })
                .ok_or_else(|| {
                    NativeError::new(
                        StageName::Link,
                        format!("no foreign library `{name}` in the link kit or {directories:?}"),
                    )
                }),
        })
        .collect()
}

fn cranelift_objects(objects: &[hirsel_lir::stage::ObjectArtifact]) -> bool {
    !objects.is_empty()
        && objects.iter().all(|object| {
            object
                .reproducibility
                .toolchain
                .starts_with(hirsel_cranelift::TOOLCHAIN)
        })
}

/// Link one package test executable, or its image when the toolchain runs programs in
/// runners. Each child selects one adapter; the runtime starts fresh with Test as its only root
/// and its own fixed seed.
pub(crate) fn link_tests(
    program: &mut ReadyProgram,
    tests: &[crate::testing::Test],
    objects: &[hirsel_lir::stage::ObjectArtifact],
    toolchain: &Toolchain,
    dir: &Path,
) -> Result<Runnable, NativeError> {
    let mut declarations = String::new();
    let mut cases = String::new();
    let mut launchers = vec![dir.join("tests.o")];
    let mut entries = Vec::new();
    for (index, test) in tests.iter().enumerate() {
        program.launch.test_seed = test.seed;
        let name = format!("hirsel_test_{index}");
        entries.push(name.clone());
        let bytes = launcher_object(program, &test.entry, objects, dir, None, false, &name)?;
        launchers.push(write(dir, &format!("test-{index}.o"), &bytes)?);
        declarations.push_str(&format!("declare i32 @{name}(i32, ptr)\n"));
        // The test entry sees no process argument but its own name.
        cases.push_str(&format!(
            "case{index}:\n %r{index} = call i32 @{name}(i32 1, ptr %slot)\n ret i32 %r{index}\n"
        ));
    }
    // `main <index>` runs adapter `index`; any other argument list exits 3. The
    // index is decimal digits only, and parsing stops once it names no adapter.
    let switch: String = (0..tests.len())
        .map(|index| format!(" i64 {index}, label %case{index}"))
        .collect();
    let dispatch = format!(
        "target triple = \"{triple}\"\n{declarations}\
define i32 @main(i32 %argc, ptr %argv) {{\n\
entry:\n %two = icmp eq i32 %argc, 2\n br i1 %two, label %argument, label %refuse\n\
argument:\n %slot = getelementptr ptr, ptr %argv, i64 1\n %text = load ptr, ptr %slot\n\
 %first = load i8, ptr %text\n %empty = icmp eq i8 %first, 0\n br i1 %empty, label %refuse, label %digit\n\
digit:\n %at = phi i64 [0, %argument], [%next, %accumulate]\n %n = phi i64 [0, %argument], [%value, %accumulate]\n\
 %place = getelementptr i8, ptr %text, i64 %at\n %byte = load i8, ptr %place\n %end = icmp eq i8 %byte, 0\n\
 br i1 %end, label %dispatch, label %parse\n\
parse:\n %d = sub i8 %byte, 48\n %decimal = icmp ult i8 %d, 10\n br i1 %decimal, label %accumulate, label %refuse\n\
accumulate:\n %wide = zext i8 %d to i64\n %tens = mul i64 %n, 10\n %value = add i64 %tens, %wide\n\
 %next = add i64 %at, 1\n %named = icmp ult i64 %value, {count}\n br i1 %named, label %digit, label %refuse\n\
dispatch:\n switch i64 %n, label %refuse [{switch} ]\n{cases}\
refuse:\n ret i32 3\n}}\n",
        triple = program.target.triple,
        count = tests.len(),
    );
    let dispatcher = if cranelift_objects(objects) {
        let entries: Vec<_> = entries.iter().map(String::as_str).collect();
        hirsel_cranelift::test_dispatcher_object(&program.target, &entries)
            .map_err(|error| NativeError::new(StageName::Emission, error.to_string()))?
    } else {
        hirsel_llvm::compile_ir(&dispatch, &program.target, OptLevel::None)
            .map_err(|e| NativeError::new(StageName::Link, e.to_string()))?
            .bytes
    };
    write(dir, "tests.o", &dispatcher)?;
    let runner = match toolchain.execution {
        super::runner::Execution::Executable => None,
        _ => Some(super::runner::runner(toolchain, &program.target)?),
    };
    link_launcher(
        program,
        &tests[0].entry,
        objects,
        toolchain,
        dir,
        runner.as_ref().map_or(Output::Launched, Output::Image),
        &launchers,
    )
    .map(Linked::runnable)
}

/// The encoded failure table every source program's descriptor carries.
fn failure_table(program: &ReadyProgram) -> Result<Vec<u8>, NativeError> {
    let table = program.failure_table.as_ref().ok_or_else(|| {
        NativeError::new(StageName::Link, "a program links without its failure table")
    })?;
    Ok(table.encode())
}

/// What one launched process did, before interpretation.
#[derive(Clone, Debug)]
pub struct Launched {
    /// The selected archive promises a leak audit; an unavailable record must then refuse.
    pub audit_required: bool,
    pub exit: Option<i32>,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    /// The launcher's observation record (`hirsel_rt::launch`).
    pub record: String,
}

/// What a launch runs: a linked executable, or a program image in its runner.
#[derive(Clone, Debug)]
pub enum Runnable {
    Executable(PathBuf),
    Image(super::runner::Image),
}

/// Run a linked executable with an empty granted directory under `dir`.
/// `diagnostic` selects the runtime's poisoning and header checks.
pub fn launch(
    executable: &Path,
    target: &TargetSpec,
    toolchain: &Toolchain,
    dir: &Path,
    diagnostic: bool,
) -> Result<Launched, NativeError> {
    launch_with(
        executable,
        target,
        toolchain,
        dir,
        diagnostic,
        Tape::Live {
            record: None,
            revocations: None,
        },
    )
}

/// What a launch does with the Io completion tape.
#[derive(Clone, Copy, Debug)]
pub enum Tape<'a> {
    Live {
        record: Option<&'a Path>,
        revocations: Option<&'a Path>,
    },
    /// Simulated Io consuming exactly this recorded file.
    Replay(&'a Path),
}

/// `launch`, recording or replaying the Io completion tape. On x86-64 the
/// process runs under `toolchain.wrapper` when one is set.
pub fn launch_with(
    executable: &Path,
    target: &TargetSpec,
    toolchain: &Toolchain,
    dir: &Path,
    diagnostic: bool,
    tape: Tape<'_>,
) -> Result<Launched, NativeError> {
    launch_runnable(
        &Runnable::Executable(executable.to_path_buf()),
        target,
        toolchain,
        dir,
        diagnostic,
        tape,
    )
}

/// `launch_with` for an executable or an image.
pub fn launch_runnable(
    runnable: &Runnable,
    target: &TargetSpec,
    toolchain: &Toolchain,
    dir: &Path,
    diagnostic: bool,
    tape: Tape<'_>,
) -> Result<Launched, NativeError> {
    launch_impl(
        runnable,
        target,
        toolchain,
        dir,
        diagnostic,
        tape,
        None,
        &Default::default(),
    )
}

pub(crate) fn launch_fixture(
    runnable: &Runnable,
    target: &TargetSpec,
    toolchain: &Toolchain,
    dir: &Path,
    diagnostic: bool,
    tape: Tape<'_>,
    fixture: &super::Fixture,
) -> Result<Launched, NativeError> {
    launch_impl(
        runnable, target, toolchain, dir, diagnostic, tape, None, fixture,
    )
}

/// The command that runs `executable` for `target`: under qemu for AArch64,
/// else under the wrapper, and at the floor through the release's own loader.
fn launcher(
    executable: &Path,
    target: &TargetSpec,
    toolchain: &Toolchain,
) -> Result<Command, NativeError> {
    // At the floor, the release's loader runs the executable with the
    // release's libraries, so nothing installed on the host takes part.
    let floor = toolchain.glibc_floor.as_ref().map(|floor| {
        let libraries = floor.join(&target.triple);
        let loader = if aarch64(target) {
            "ld-linux-aarch64.so.1"
        } else {
            "ld-linux-x86-64.so.2"
        };
        [
            libraries.join(loader).into_os_string(),
            "--library-path".into(),
            libraries.into_os_string(),
        ]
    });
    Ok(if aarch64(target) {
        let mut command = Command::new(required_path(
            &toolchain.qemu_aarch64,
            "HIRSEL_QEMU_AARCH64",
        )?);
        match &floor {
            Some(floor) => command.args(floor),
            None => command.arg("-L").arg(required_path(
                &toolchain.aarch64_sysroot,
                "HIRSEL_AARCH64_SYSROOT",
            )?),
        };
        command.arg(executable);
        command
    } else if let Some((program, arguments)) = toolchain.wrapper.split_first() {
        let mut command = Command::new(program);
        command.args(arguments).arg(executable);
        command
    } else if let Some([loader, option, libraries]) = &floor {
        let mut command = Command::new(loader);
        command.arg(option).arg(libraries).arg(executable);
        command
    } else {
        Command::new(executable)
    })
}

/// The file under a launch's directory receiving its `--trace-roots` lines (S-14).
pub(crate) const ROOT_TRACE: &str = "roots";

/// Harness execution or a user launch with an explicit admitted run record.
fn launch_impl(
    runnable: &Runnable,
    target: &TargetSpec,
    toolchain: &Toolchain,
    dir: &Path,
    diagnostic: bool,
    tape: Tape<'_>,
    live: Option<LiveRun<'_>>,
    fixture: &super::Fixture,
) -> Result<Launched, NativeError> {
    let root = dir.join("root");
    let record = dir.join("observation");
    let roots = dir.join(ROOT_TRACE);
    let _ = std::fs::remove_file(&roots);
    // Where the executable reports a launch it refused, instead of standard error.
    let refusal = dir.join("refusal");
    // Where a runner reports a failure before the program's code ran.
    let tooling = dir.join("tooling");
    let _ = std::fs::remove_dir_all(&root);
    let _ = std::fs::remove_file(&record);
    let _ = std::fs::remove_file(&refusal);
    let _ = std::fs::remove_file(&tooling);
    std::fs::create_dir_all(&root)
        .map_err(|e| NativeError::new(StageName::Launch, format!("granted directory: {e}")))?;
    // The program's arguments after the one naming it, and its environment.
    let mut command = Command::new("program");
    command
        .env_clear()
        .env("HIRSEL_RT_ROOT", &root)
        .env("HIRSEL_RT_OBSERVATION", &record)
        .env("HIRSEL_RT_REFUSAL", &refusal);
    if let Some(ticks) = fixture.monotonic {
        command.env("HIRSEL_RT_VIRTUAL_CLOCK", ticks.to_string());
    }
    if let Some(net) = &fixture.net {
        command.env("HIRSEL_RT_NET", serde_json::to_string(net).unwrap());
    }
    if let Some(process) = &fixture.process {
        command.env("HIRSEL_RT_PROCESS", serde_json::to_string(process).unwrap());
    }
    if let Some(env) = &fixture.env {
        command.env("HIRSEL_RT_ENV", serde_json::to_string(env).unwrap());
    }
    if fixture.trace_roots {
        command.env("HIRSEL_RT_TRACE_ROOTS", &roots);
    }
    // A live launch selects record and replay by flag; harness launches by environment.
    match tape {
        Tape::Replay(path) => {
            if live.is_some() {
                command.arg("--hirsel-replay").arg(absolute(path)?);
            } else {
                command.env("HIRSEL_RT_REPLAY", path);
            }
        }
        Tape::Live {
            record,
            revocations,
        } => {
            if live.is_some() {
                if let Some(path) = record {
                    command.arg("--hirsel-record").arg(absolute(path)?);
                }
                if let Some(path) = revocations {
                    command.arg("--hirsel-revocations").arg(absolute(path)?);
                }
                command.arg("--hirsel-run").arg(dir.join("run.json"));
            } else if let Some(path) = record {
                command.env("HIRSEL_RT_TAPE", path);
            }
        }
    }
    if toolchain.uses_memcheck() {
        command
            .env(
                "VALGRIND_LIB",
                required_path(&toolchain.valgrind_lib, "VALGRIND_LIB")?,
            )
            .env(
                "LD_LIBRARY_PATH",
                required_path(&toolchain.valgrind_runtime, "HIRSEL_VALGRIND_RUNTIME")?,
            );
    }
    if toolchain.runtime_build.measurement {
        command.env("HIRSEL_RT_MEASUREMENT", dir.join("measurement"));
    }
    if diagnostic {
        command.env("HIRSEL_RT_DIAGNOSTIC", "1");
    }
    // A replay reads no host input.
    let stdin = live.as_ref().is_some_and(|live| {
        !matches!(tape, Tape::Replay(_))
            && live
                .run
                .record()
                .io
                .as_ref()
                .is_some_and(|io| io.stdin.is_some())
    });
    let output = match live {
        Some(live) => {
            let (mut process, _hold) = process(runnable, target, toolchain, &tooling, &command)?;
            let output = run_live(&mut process, stdin, live.inherit, live.stdout, live.stderr)?;
            if let Runnable::Image(image) = runnable {
                if let Some(error) = super::runner::tooling_failure(&tooling) {
                    return Err(error);
                }
                super::runner::count_image(image);
            }
            output
        }
        None => run_bounded(
            runnable,
            target,
            toolchain,
            &tooling,
            &command,
            Some(toolchain.launch_timeout),
        )
        .inspect_err(|error| {
            if error.message.contains("launch timeout") {
                match retain_timeout(runnable, dir, &command, &error.message) {
                    Ok(path) => {
                        eprintln!("native: timed-out launch retained at {}", path.display())
                    }
                    Err(error) => eprintln!("native: could not retain timed-out launch: {error}"),
                }
            }
        })?,
    };
    Ok(Launched {
        audit_required: toolchain.runtime_build.diagnostic || toolchain.runtime_build.measurement,
        exit: output.status.code(),
        stdout: output.stdout,
        stderr: output.stderr,
        record: std::fs::read_to_string(&record)
            .or_else(|_| std::fs::read_to_string(&refusal))
            .unwrap_or_default(),
    })
}

/// The process that runs `runnable` with `command`'s arguments and environment: the
/// executable, or a fresh runner process loading the image, each under its target's emulator
/// or loader. The hold keeps a cached image in place while the run lasts.
fn process(
    runnable: &Runnable,
    target: &TargetSpec,
    toolchain: &Toolchain,
    tooling: &Path,
    command: &Command,
) -> Result<(Command, Option<std::fs::File>), NativeError> {
    let (mut process, hold) = match runnable {
        Runnable::Executable(executable) => (launcher(executable, target, toolchain)?, None),
        Runnable::Image(image) => {
            let mut process = launcher(&image.runner.path, target, toolchain)?;
            process
                .arg("exec")
                .arg(tooling)
                .arg(&image.path)
                .arg(&image.path);
            (process, super::runner::hold(&image.path))
        }
    };
    process.env_clear().args(command.get_args());
    for (name, value) in command.get_envs() {
        if let Some(value) = value {
            process.env(name, value);
        }
    }
    Ok((process, hold))
}

/// Keep a timed-out launch's directory for diagnosis (`super::timeout`), with the image it
/// ran when it ran one, since an image lives in the cache rather than the launch directory.
fn retain_timeout(
    runnable: &Runnable,
    dir: &Path,
    command: &Command,
    reason: &str,
) -> std::io::Result<PathBuf> {
    let retained = super::timeout::retain(dir, command, reason)?;
    if let Runnable::Image(image) = runnable
        && let Some(name) = image.path.file_name()
    {
        std::fs::copy(&image.path, retained.join(name))?;
    }
    Ok(retained)
}

/// Run `runnable` with `command`'s arguments and environment in its own process group, with
/// no standard input and captured output, to completion or, past `timeout`, killed with its
/// group and failed. An image runs on its runner's fork server unless the startup audit, the
/// toolchain or a wrapper selects a fresh runner process; a failure before the program's code
/// ran (`tooling`) is a tool outcome.
pub(crate) fn run_bounded(
    runnable: &Runnable,
    target: &TargetSpec,
    toolchain: &Toolchain,
    tooling: &Path,
    command: &Command,
    timeout: Option<Duration>,
) -> Result<std::process::Output, NativeError> {
    let _ = std::fs::remove_file(tooling);
    let served = match runnable {
        Runnable::Image(image)
            if image.runner.mode == super::runner::Mode::Fork
                && toolchain.execution == super::runner::Execution::Runner
                && toolchain.wrapper.is_empty() =>
        {
            Some(image)
        }
        _ => None,
    };
    let output = match served {
        Some(image) => {
            let _hold = super::runner::hold(&image.path);
            let prefix = launcher(&image.runner.path, target, toolchain)?;
            let started = super::runner::start_served(&prefix, image, tooling, command, timeout)
                .map_err(|e| NativeError::new(StageName::Launch, format!("the runner: {e}")))?;
            finish(started, timeout)?
        }
        None => {
            use std::os::unix::process::CommandExt;
            use std::process::Stdio;
            let (mut process, _hold) = process(runnable, target, toolchain, tooling, command)?;
            let child = process
                .process_group(0)
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .map_err(|e| {
                    NativeError::new(
                        StageName::Launch,
                        format!("could not run {:?}: {e}", process.get_program()),
                    )
                })?;
            finish(super::runner::Started::spawned(child), timeout)?
        }
    };
    if let Runnable::Image(image) = runnable {
        if let Some(error) = super::runner::tooling_failure(tooling) {
            return Err(error);
        }
        super::runner::count_image(image);
    }
    Ok(output)
}

fn absolute(path: &Path) -> Result<PathBuf, NativeError> {
    std::path::absolute(path).map_err(|e| NativeError::new(StageName::Launch, e.to_string()))
}

pub(crate) struct LiveRun<'a> {
    pub run: &'a hirsel_run_config::ValidatedRun,
    pub inherit: bool,
    pub stdout: &'a mut dyn std::io::Write,
    pub stderr: &'a mut dyn std::io::Write,
}

/// A user's server runs until it exits or is interrupted; harness timeouts remain tooling.
pub(crate) fn launch_live(
    runnable: &Runnable,
    target: &TargetSpec,
    toolchain: &Toolchain,
    dir: &Path,
    tape: Tape<'_>,
    live: LiveRun<'_>,
) -> Result<Launched, NativeError> {
    launch_impl(
        runnable,
        target,
        toolchain,
        dir,
        false,
        tape,
        Some(live),
        &Default::default(),
    )
}
fn run_live(
    command: &mut Command,
    stdin_granted: bool,
    inherit: bool,
    stdout: &mut dyn std::io::Write,
    stderr: &mut dyn std::io::Write,
) -> Result<std::process::Output, NativeError> {
    use std::io::Read;
    use std::process::Stdio;
    let io = |e: std::io::Error| NativeError::new(StageName::Launch, e.to_string());
    if inherit {
        let status = command
            .stdin(if stdin_granted {
                Stdio::inherit()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .status()
            .map_err(io)?;
        return Ok(std::process::Output {
            status,
            stdout: vec![],
            stderr: vec![],
        });
    }
    let mut child = command
        .stdin(if stdin_granted {
            Stdio::inherit()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(io)?;
    let pipes: [Box<dyn Read + Send>; 2] = [
        Box::new(child.stdout.take().expect("stdout pipe")),
        Box::new(child.stderr.take().expect("stderr pipe")),
    ];
    // Drain both pipes concurrently. The caller's writers stay on this thread,
    // so a borrowed writer need not be Send and neither stream can fill its pipe.
    let (send, receive) = std::sync::mpsc::sync_channel(2);
    let readers = pipes
        .into_iter()
        .enumerate()
        .map(|(stream, mut pipe)| {
            let send = send.clone();
            std::thread::spawn(move || {
                let mut buffer = [0; 8192];
                loop {
                    let chunk = match pipe.read(&mut buffer) {
                        Ok(0) => break,
                        Ok(n) => Ok(buffer[..n].to_vec()),
                        Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                        Err(error) => Err(error),
                    };
                    let failed = chunk.is_err();
                    if send.send((stream, chunk)).is_err() || failed {
                        break;
                    }
                }
            })
        })
        .collect::<Vec<_>>();
    drop(send);
    let mut captured = [Vec::new(), Vec::new()];
    let mut result = Ok(());
    for (stream, chunk) in receive {
        let forwarded = match chunk {
            Ok(bytes) => {
                captured[stream].extend_from_slice(&bytes);
                if result.is_ok() {
                    if stream == 0 {
                        stdout.write_all(&bytes).and_then(|()| stdout.flush())
                    } else {
                        stderr.write_all(&bytes).and_then(|()| stderr.flush())
                    }
                } else {
                    Ok(())
                }
            }
            Err(error) => Err(error),
        };
        if result.is_ok()
            && let Err(error) = forwarded
        {
            result = Err(error);
            let _ = child.kill();
        }
    }
    for reader in readers {
        if reader.join().is_err() && result.is_ok() {
            result = Err(std::io::Error::other("native output reader panicked"));
            let _ = child.kill();
        }
    }
    let status = child.wait().map_err(io)?;
    result.map_err(io)?;
    let [stdout, stderr] = captured;
    Ok(std::process::Output {
        status,
        stdout,
        stderr,
    })
}

/// Interpret a launched process against the program it was linked from. A
/// crash, a refused launch, a malformed record and a block left live are tool
/// errors, never observations.
/// How a launched process's run ended, from its observation record.
#[derive(Debug)]
pub enum Observed {
    /// The run returned, failed or was Blocked.
    Ran(Observation, Vec<hirsel_run_config::Wait>),
    /// The launch was refused before entry: a `RunRefusal`'s JSON, or the launcher's reason.
    Refused(String),
    /// The run stopped: `Refused`, `Diverged` or `Environmental`.
    Stopped(hirsel_run_config::RunOutcome),
}

/// The observation of a run that returned, failed or was Blocked; a refused or stopped run is
/// a launch error here (see [`observe_run`]).
pub fn observe(
    launched: &Launched,
    program: &ReadyProgram,
    sites: &Sites<'_>,
) -> Result<Observation, NativeError> {
    match observe_run(launched, program, sites)? {
        Observed::Ran(observation, _) => Ok(observation),
        Observed::Refused(reason) => Err(NativeError::new(
            StageName::Launch,
            format!("launch refused: {reason}"),
        )),
        Observed::Stopped(outcome) => Err(NativeError::new(
            StageName::Launch,
            format!("the run stopped: {outcome:?}"),
        )),
    }
}

/// Read a launched process's observation record into how its run ended.
pub fn observe_run(
    launched: &Launched,
    program: &ReadyProgram,
    sites: &Sites<'_>,
) -> Result<Observed, NativeError> {
    let bad = |what: String| {
        NativeError::new(
            StageName::Launch,
            format!(
                "{what} (exit {:?}, stderr {:?})",
                launched.exit,
                String::from_utf8_lossy(&launched.stderr)
            ),
        )
    };
    let mut lines = launched.record.lines();
    let version = format!(
        "hirsel-observation {}",
        hirsel_rt::launch::OBSERVATION_VERSION
    );
    if lines.next() != Some(version.as_str()) {
        return Err(bad("the process left no observation record".into()));
    }
    let rules = failure_rules(program);
    let codes = site_codes(program);
    let mut status = None;
    let mut result = None;
    let mut failures = Vec::new();
    let mut audit = None;
    let mut blocked = false;
    let mut waits = None;
    let mut ended = false;
    let mut budget = None;
    let mut stopped = None;
    for line in lines {
        let words: Vec<&str> = line.split(' ').collect();
        let rest = |n: usize| words[n..].join(" ");
        match words.as_slice() {
            ["refused", ..] => {
                if launched.exit != Some(hirsel_run_config::exit::REFUSED) {
                    return Err(bad("a refused launch did not exit as refused".into()));
                }
                return Ok(Observed::Refused(rest(1)));
            }
            ["stopped", "refused", ..] => {
                stopped = Some(hirsel_run_config::RunOutcome::Refused(rest(2)))
            }
            ["stopped", "diverged", ..] => {
                stopped = Some(hirsel_run_config::RunOutcome::Diverged(rest(2)))
            }
            ["stopped", "environmental", "codec_limit"] => {
                stopped = Some(hirsel_run_config::RunOutcome::Environmental(
                    hirsel_run_config::Environmental::CodecLimit,
                ))
            }
            ["stopped", "environmental", "recording", ..] => {
                stopped = Some(hirsel_run_config::RunOutcome::Environmental(
                    hirsel_run_config::Environmental::Recording(rest(3)),
                ))
            }
            ["stopped", "environmental", "output", ..] => {
                stopped = Some(hirsel_run_config::RunOutcome::Environmental(
                    hirsel_run_config::Environmental::Output(rest(3)),
                ))
            }
            ["status", s] => status = s.parse::<u8>().ok(),
            ["blocked"] => blocked = true,
            ["waits", ..] => {
                if waits.is_some() {
                    return Err(bad("duplicate wait report".into()));
                }
                waits = Some(hirsel_run_config::Wait::decode(&rest(1)).map_err(bad)?);
            }
            ["ended"] => ended = true,
            ["budget_exceeded", kind] => {
                budget = Some(
                    hirsel_run_config::BudgetKind::parse(kind)
                        .ok_or_else(|| bad(format!("an unknown budget in `{line}`")))?,
                )
            }
            ["result", hex] => result = Some(*hex),
            ["failure", _kind, rule, site, operation] => {
                let rule = rule
                    .parse::<u32>()
                    .ok()
                    .and_then(
                        |r| match r.checked_sub(hirsel_core_contract::RUNTIME_RULE_BASE) {
                            Some(i) => hirsel_core_contract::RUNTIME_RULES
                                .get(i as usize)
                                .map(|r| r.to_string()),
                            None => rules.get(r.checked_sub(1)? as usize).cloned(),
                        },
                    )
                    .ok_or_else(|| bad(format!("an unknown failure rule code in `{line}`")))?;
                let (definition, range) = u64::from_str_radix(site, 16)
                    .ok()
                    .and_then(|code| codes.get(&code))
                    .ok_or_else(|| bad(format!("an unknown failure site code in `{line}`")))?;
                let mut failure = sites.failure(&rule, definition, *range)?;
                if operation != site && *operation != "0000000000000000" {
                    let (definition, range) = u64::from_str_radix(operation, 16)
                        .ok()
                        .and_then(|code| codes.get(&code))
                        .ok_or_else(|| {
                            bad(format!("an unknown operation site code in `{line}`"))
                        })?;
                    let operation = sites.failure(&rule, definition, *range)?;
                    failure.operation = Some(hirsel_interp::observation::SourceSite {
                        definition: operation.definition,
                        line: operation.line,
                        column: operation.column,
                    });
                }
                failures.push(failure);
            }
            ["audit", live_blocks, _bytes, _immortal, regions, retained] => {
                audit = Some((*live_blocks, *regions, *retained));
            }
            ["audit", "unavailable"] => audit = Some(("unavailable", "unavailable", "unavailable")),
            _ => return Err(bad(format!("a malformed observation line `{line}`"))),
        }
    }
    // A Blocked or stopped run ends without cleanup, so it has no exit audit.
    let unaudited = audit == Some(("unavailable", "unavailable", "unavailable"));
    if waits.is_some() && !blocked {
        return Err(bad("wait report on a run that is not Blocked".into()));
    }
    if let Some(outcome) = stopped {
        if blocked || status.is_some() || result.is_some() || !failures.is_empty() || !unaudited {
            return Err(bad("an inconsistent stopped observation record".into()));
        }
        if launched.exit != Some(outcome.exit_code()) {
            return Err(bad(format!(
                "a stopped process did not exit {}",
                outcome.exit_code()
            )));
        }
        return Ok(Observed::Stopped(outcome));
    }
    if let Some(kind) = budget {
        if blocked || status.is_some() || result.is_some() || !failures.is_empty() || !unaudited {
            return Err(bad("an inconsistent budget observation record".into()));
        }
        return verify_exit(
            launched,
            sites,
            Observation {
                exit_code: None,
                blocked: false,
                budget: Some(kind),
                output: launched.stdout.clone(),
                failures,
                roots: Vec::new(),
            },
            vec![],
        );
    }
    if blocked {
        if status.is_some() || result.is_some() || !failures.is_empty() || !unaudited {
            return Err(bad("an inconsistent Blocked observation record".into()));
        }
        return verify_exit(
            launched,
            sites,
            Observation {
                exit_code: None,
                blocked: true,
                budget: None,
                output: launched.stdout.clone(),
                failures,
                roots: Vec::new(),
            },
            waits.ok_or_else(|| bad("Blocked run has no wait report".into()))?,
        );
    }
    // A failure that ended the run where it stood (FAIL-3) is abandoned unaudited.
    if ended && (!unaudited || status != Some(1) || failures.is_empty()) {
        return Err(bad("an inconsistent ended observation record".into()));
    }
    if !ended && audit != Some(("0", "0", "0")) && (launched.audit_required || !unaudited) {
        return Err(bad(format!(
            "the runtime's exit audit is not clean: (live blocks, regions, retained) = {audit:?}"
        )));
    }
    let exit_code = match (status, result, failures.is_empty()) {
        (Some(0), Some(hex), true) => Some(i32::from(!hex.starts_with("00000000"))),
        (Some(1), None, false) => None,
        _ => return Err(bad("an inconsistent observation record".into())),
    };
    verify_exit(
        launched,
        sites,
        Observation {
            exit_code,
            blocked: false,
            budget: None,
            output: launched.stdout.clone(),
            failures,
            roots: Vec::new(),
        },
        vec![],
    )
}

fn verify_exit(
    launched: &Launched,
    sites: &Sites<'_>,
    observation: Observation,
    waits: Vec<hirsel_run_config::Wait>,
) -> Result<Observed, NativeError> {
    let expected = observation.outcome(sites, waits.clone())?.exit_code();
    if launched.exit != Some(expected) {
        return Err(NativeError::new(
            StageName::Launch,
            format!("the process did not exit {expected}: {:?}", launched.exit),
        ));
    }
    Ok(Observed::Ran(observation, waits))
}

impl Toolchain {
    fn uses_memcheck(&self) -> bool {
        self.valgrind.as_deref().is_some_and(|valgrind| {
            self.wrapper
                .first()
                .is_some_and(|wrapper| Path::new(wrapper) == valgrind)
        })
    }

    /// Link C (or other foreign) `objects` with the selected runtime archive into
    /// the executable `output`, by the same kit line and loader as a program.
    pub fn link_executable(
        &self,
        target: &TargetSpec,
        objects: &[PathBuf],
        output: &Path,
    ) -> Result<(), NativeError> {
        let archive = self.archive(&target.triple).ok_or_else(|| {
            NativeError::new(
                StageName::Link,
                format!("no runtime archive for {}", target.triple),
            )
        })?;
        let kit = super::kit::Kit::load(&self.kit)?;
        let link = super::kit::Link {
            kit: &kit,
            triple: &target.triple,
            kind: super::kit::Kind::Executable {
                loader: self.loader(target)?,
            },
            objects,
            runtime: Some(archive),
            unwind: true,
            foreign: &[],
        };
        checked(StageName::Link, &mut link.command(&self.lld, output)?.0)
    }

    /// The loader and library directory an executable for `target` loads with
    /// as this toolchain launches it: under memcheck, Valgrind's pinned loader
    /// and runtime libraries; otherwise the target's canonical interpreter (`None`).
    pub fn loader(&self, target: &TargetSpec) -> Result<Option<(PathBuf, PathBuf)>, NativeError> {
        if aarch64(target) || !self.uses_memcheck() {
            return Ok(None);
        }
        let runtime = required_path(&self.valgrind_runtime, "HIRSEL_VALGRIND_RUNTIME")?;
        Ok(Some((
            runtime.join("ld-linux-x86-64.so.2"),
            runtime.to_path_buf(),
        )))
    }

    /// This toolchain with x86-64 executables launched under Valgrind
    /// memcheck: an invalid read or write, a use of uninitialised memory in a
    /// branch or system call and a definite leak each fail the launch.
    /// Memcheck runs a program tens of times slower, so its launches get ten
    /// times the timeout.
    pub fn under_memcheck(mut self) -> Result<Self, NativeError> {
        self.launch_timeout = LAUNCH_TIMEOUT * 10;
        self.wrapper = [
            required_path(&self.valgrind, "HIRSEL_VALGRIND")?
                .to_str()
                .ok_or_else(|| NativeError::new(StageName::Link, "Valgrind path is not UTF-8"))?,
            "--quiet",
            "--error-exitcode=97",
            "--leak-check=full",
            "--errors-for-leak-kinds=definite",
        ]
        .map(String::from)
        .to_vec();
        Ok(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn concurrent_bounded_launches_close_their_pipes() {
        std::thread::scope(|scope| {
            for _ in 0..2 {
                scope.spawn(|| {
                    for _ in 0..128 {
                        let output = bounded(
                            Command::new("/bin/sh").args(["-c", "printf out; printf err >&2"]),
                            Duration::from_secs(30),
                        )
                        .unwrap();
                        assert!(output.status.success());
                        assert_eq!(output.stdout, b"out");
                        assert_eq!(output.stderr, b"err");
                    }
                });
            }
        });
    }

    #[test]
    fn a_launch_past_its_timeout_is_killed_with_its_whole_process_group() {
        // The shell forks the sleeping child, which holds the pipes open: the
        // whole group must be killed for the launch to end.
        let started = Instant::now();
        let error = bounded(
            Command::new("/bin/sh").args(["-c", "sleep 30; echo late"]),
            Duration::from_millis(200),
        )
        .unwrap_err();
        assert!(started.elapsed() < Duration::from_secs(10));
        assert_eq!(error.stage, StageName::Launch);
        assert!(
            error.message.contains("launch timeout"),
            "{}",
            error.message
        );
        let finished = bounded(
            Command::new("/bin/sh").args(["-c", "echo out; echo err >&2; exit 3"]),
            Duration::from_secs(30),
        )
        .unwrap();
        assert_eq!(finished.status.code(), Some(3));
        assert_eq!(finished.stdout, b"out\n");
        assert_eq!(finished.stderr, b"err\n");
    }

    #[test]
    fn floor_launches_run_through_the_release_loader_and_its_libraries_only() {
        let tools = Toolchain {
            runtime_build: RuntimeBuild::default(),
            clang: "/unused/clang".into(),
            aarch64_cc: None,
            lld: "/unused/ld.lld".into(),
            kit: "/unused/kit".into(),
            debug_info: false,
            glibc_floor: Some("/floor".into()),
            qemu_aarch64: Some("/tools/qemu".into()),
            aarch64_sysroot: Some("/unused/sysroot".into()),
            valgrind: None,
            valgrind_lib: None,
            valgrind_runtime: None,
            runtime: BTreeMap::new(),
            runtime_debug: BTreeMap::new(),
            builtins: BTreeMap::new(),
            wrapper: Vec::new(),
            launch_timeout: LAUNCH_TIMEOUT,
            cache: "/unused/cache".into(),
            execution: Default::default(),
        };
        let shape = |target: &TargetSpec| {
            let command = launcher(Path::new("/out/app"), target, &tools).unwrap();
            let mut words = vec![command.get_program().to_owned()];
            words.extend(command.get_args().map(ToOwned::to_owned));
            words
        };
        let x86 = "/floor/x86_64-unknown-linux-gnu";
        assert_eq!(
            shape(&TargetSpec::x86_64_unknown_linux_gnu()),
            [
                format!("{x86}/ld-linux-x86-64.so.2").as_str(),
                "--library-path",
                x86,
                "/out/app"
            ]
        );
        // The emulator runs the release's loader; the cross sysroot takes no part.
        let arm = "/floor/aarch64-unknown-linux-gnu";
        assert_eq!(
            shape(&TargetSpec::aarch64_unknown_linux_gnu()),
            [
                "/tools/qemu",
                format!("{arm}/ld-linux-aarch64.so.1").as_str(),
                "--library-path",
                arm,
                "/out/app"
            ]
        );
    }

    #[test]
    fn wrappers_keep_selected_memcheck_inputs_without_requiring_them_for_other_launches() {
        let dir = std::env::temp_dir().join(format!("hirsel-memcheck-env-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut tools = Toolchain {
            runtime_build: RuntimeBuild::default(),
            clang: "/unused/clang".into(),
            aarch64_cc: Some("/unused/gcc".into()),
            lld: "/unused/ld.lld".into(),
            kit: "/unused/kit".into(),
            debug_info: false,
            glibc_floor: None,
            qemu_aarch64: Some("/unused/qemu".into()),
            aarch64_sysroot: Some("/unused/sysroot".into()),
            valgrind: Some("/bin/sh".into()),
            valgrind_lib: Some("/inputs/valgrind-support".into()),
            valgrind_runtime: Some("/unused/libc".into()),
            runtime: BTreeMap::new(),
            runtime_debug: BTreeMap::new(),
            builtins: BTreeMap::new(),
            wrapper: vec![
                "/bin/sh".into(),
                "-c".into(),
                "printf '%s' \"$VALGRIND_LIB\"".into(),
            ],
            launch_timeout: LAUNCH_TIMEOUT,
            cache: "/unused/cache".into(),
            execution: Default::default(),
        };
        let launched = launch(
            Path::new("/unused/program"),
            &TargetSpec::x86_64_unknown_linux_gnu(),
            &tools,
            &dir,
            false,
        )
        .unwrap();
        assert_eq!(launched.exit, Some(0));
        assert_eq!(launched.stdout, b"/inputs/valgrind-support");
        // A different wrapper uses neither the Valgrind loader nor its environment,
        // even when the toolchain has a pinned Valgrind executable available.
        tools.valgrind = Some("/unused/valgrind".into());
        tools.valgrind_lib = None;
        tools.valgrind_runtime = None;
        assert!(
            tools
                .loader(&TargetSpec::x86_64_unknown_linux_gnu())
                .unwrap()
                .is_none()
        );
        let launched = launch(
            Path::new("/unused/program"),
            &TargetSpec::x86_64_unknown_linux_gnu(),
            &tools,
            &dir,
            false,
        )
        .unwrap();
        assert_eq!(launched.exit, Some(0));
        assert!(launched.stdout.is_empty());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn configured_tools_refuse_missing_or_path_searched_names() {
        assert!(configured_path("tool", None).is_err());
        assert!(configured_path("tool", Some("clang".into())).is_err());
        assert!(configured_path("tool", Some("relative/clang".into())).is_err());
        assert_eq!(
            configured_path("tool", Some("/inputs/clang".into())).unwrap(),
            Path::new("/inputs/clang")
        );
    }

    #[test]
    fn failed_tools_report_arguments_and_output_without_the_captured_environment() {
        let error = checked(
            StageName::Link,
            Command::new("/bin/sh")
                .args(["-c", "echo build-failed >&2; exit 7"])
                .env("HIRSEL_TEST_SECRET", "snapshot-secret-value"),
        )
        .unwrap_err();
        assert!(error.message.contains("build-failed"), "{error}");
        assert!(error.message.contains("exit status: 7"), "{error}");
        assert!(!error.message.contains("snapshot-secret-value"), "{error}");
        let error = tool(
            StageName::Link,
            Command::new("/missing/hirsel-test-tool")
                .env("HIRSEL_TEST_SECRET", "snapshot-secret-value"),
        )
        .unwrap_err();
        assert!(!error.message.contains("snapshot-secret-value"), "{error}");
    }

    #[test]
    fn unused_tool_inputs_are_not_read_and_missing_selected_inputs_are_errors() {
        let missing = "HIRSEL_TEST_UNCONFIGURED_TOOL_INPUT";
        assert_eq!(selected_path(missing, false).unwrap(), None);
        assert!(selected_path(missing, true).is_err());
        assert!(required_path(&None, missing).is_err());
    }
}

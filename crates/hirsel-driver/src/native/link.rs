//! P8: link the program's object with the runtime archive and a `main` that
//! hands the entry to `hirsel_rt_v2_launch`, then run it with explicit grants.
use super::emit::{Backend, check_runtime_calls, failure_rules};
use super::observe::{Observation, Sites, site_codes};
use super::stages::{NativeError, ReadyProgram, StageName};
use hirsel_lir::abi_types::{ParameterPassing, PhysicalType};
use hirsel_lir::emission::OptLevel;
use hirsel_lir::layout_types::{SizeSpec, TargetSpec};
use hirsel_syntax::{ContentHash, DefPath};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

/// The external tools and archives linking and launching use. Nothing here is
/// discovered ambiently at link time: a missing tool is a `NativeError`.
#[derive(Clone)]
pub struct Toolchain {
    /// The same admitted result provider used by native, object and runtime tasks.
    pub products: std::sync::Arc<dyn hirsel_engine::store::ResultStore>,
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
    /// Link the debug-bearing runtime archive and keep DWARF in the executable
    /// (`hirsel build --debug`), ready for gdb and lldb without post-link copies.
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

impl std::fmt::Debug for Toolchain {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Toolchain")
            .field("products", &self.products.name())
            .field("lld", &self.lld)
            .field("kit", &self.kit)
            .field("runtime", &self.runtime)
            .field("runtime_build", &self.runtime_build)
            .field("execution", &self.execution)
            .finish_non_exhaustive()
    }
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
    /// Compile ownership hooks and their bounded diagnostic buffer. Ordinary
    /// runtime products omit both the hooks and their private run state.
    pub ownership_trace: bool,
}
impl RuntimeBuild {
    pub(crate) fn features(self) -> String {
        [
            self.diagnostic.then_some("diagnostic"),
            self.system_allocator.then_some("system-allocator"),
            self.measurement.then_some("measurement"),
            self.memcheck.then_some("memcheck"),
            self.ownership_trace.then_some("ownership-trace"),
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
pub(super) fn bounded(
    command: &mut Command,
    timeout: Duration,
) -> Result<std::process::Output, NativeError> {
    bounded_input(command, timeout, false)
}
fn bounded_input(
    command: &mut Command,
    timeout: Duration,
    stdin_granted: bool,
) -> Result<std::process::Output, NativeError> {
    use std::os::unix::process::CommandExt;
    use std::process::Stdio;
    let child = command
        .process_group(0)
        .stdin(if stdin_granted {
            Stdio::inherit()
        } else {
            Stdio::null()
        })
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
    /// Resolve an ordinary host request using its already admitted provider.
    /// The runtime cache directory holds artifacts, never another store budget.
    pub(crate) fn for_host_request(
        target: &TargetSpec,
        store: std::sync::Arc<dyn hirsel_engine::store::ResultStore>,
    ) -> Result<Self, NativeError> {
        let cache = std::env::var_os("HIRSEL_NATIVE_CACHE")
            .map(PathBuf::from)
            .unwrap_or_else(super::runtime_cache::default_directory);
        Self::in_tree_with_products(&cache, &[target], &[RuntimeBuild::default()], None, store)
    }

    /// The toolchain of a source checkout: the pinned LLVM prefix
    /// (`LLVM_SYS_231_PREFIX`), explicit cross-tool inputs, and the runtime
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
        Self::in_tree_inputs(build_dir, targets, runtime_builds, None, None)
    }

    /// Construct a toolchain against a resident session's frozen Cargo inputs.
    pub fn in_tree_with_inputs(
        build_dir: &Path,
        targets: &[&TargetSpec],
        runtime_builds: &[RuntimeBuild],
        inputs: &super::RuntimeInputs,
    ) -> Result<Self, NativeError> {
        Self::in_tree_inputs(build_dir, targets, runtime_builds, Some(inputs), None)
    }

    /// Use the admitted native request's single provider for runtime products.
    pub fn in_tree_with_products(
        build_dir: &Path,
        targets: &[&TargetSpec],
        runtime_builds: &[RuntimeBuild],
        inputs: Option<&super::RuntimeInputs>,
        store: std::sync::Arc<dyn hirsel_engine::store::ResultStore>,
    ) -> Result<Self, NativeError> {
        Self::in_tree_inputs(build_dir, targets, runtime_builds, inputs, Some(store))
    }

    fn in_tree_inputs(
        build_dir: &Path,
        targets: &[&TargetSpec],
        runtime_builds: &[RuntimeBuild],
        inputs: Option<&super::RuntimeInputs>,
        store: Option<std::sync::Arc<dyn hirsel_engine::store::ResultStore>>,
    ) -> Result<Self, NativeError> {
        let _timing = hirsel_intern::timings::interval(hirsel_intern::timings::Interval::Link);
        let [runtime_build, ..] = *runtime_builds else {
            return Err(NativeError::new(
                StageName::Link,
                "a toolchain needs a runtime archive",
            ));
        };
        let prefix = environment_path("LLVM_SYS_231_PREFIX")?;
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
        let products = match store {
            Some(store) => store,
            None => {
                let cap = std::env::var("HIRSEL_MEMORY_LIMIT")
                    .ok()
                    .and_then(|text| hirsel_guard::parse_limit(text.as_bytes()))
                    .unwrap_or(hirsel_guard::DEFAULT_LIMIT);
                let bytes = (cap / 8).min(128 << 20);
                let budget = hirsel_engine::runtime::ProcessBudget::new(bytes);
                std::sync::Arc::new(hirsel_engine::store::RequestStore::new(
                    std::sync::Arc::new(hirsel_engine::store::NoneStore),
                    budget
                        .reserve(bytes)
                        .map_err(|e| NativeError::new(StageName::Link, e.to_string()))?,
                ))
            }
        };
        let store = products.as_ref();
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
                store,
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
            let stripped = super::runtime_cache::derived(
                store,
                build_dir,
                &archive,
                "stripped",
                runtime_derivation(&prefix, "llvm-objcopy")?,
                |scratch| {
                    let stripped = scratch.join("libhirsel_rt.a");
                    checked(
                        StageName::Link,
                        llvm_tool(&prefix, "llvm-objcopy")
                            .arg("--strip-debug")
                            .args([&archive, &stripped]),
                    )?;
                    Ok(stripped)
                },
            )?;
            if runtime_build == RuntimeBuild::default() || !builtins.contains_key(&target.triple) {
                let derived = super::runtime_cache::derived(
                    store,
                    build_dir,
                    &archive,
                    "builtins",
                    runtime_derivation(&prefix, "llvm-ar")?,
                    |scratch| compiler_builtins(&prefix, &stripped, scratch),
                )?;
                builtins.insert(target.triple.clone(), derived);
            }
            let key = (target.triple.clone(), runtime_build);
            runtime.insert(key.clone(), stripped);
            runtime_debug.insert(key, archive);
        }
        Ok(Self {
            products,
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

// Exact tool and shipped dynamic library contents enter derived runtime actions.
pub(super) fn runtime_derivation(prefix: &Path, tool: &str) -> Result<ContentHash, NativeError> {
    let io = |e| {
        NativeError::new(
            StageName::Link,
            format!("runtime transformation input: {e}"),
        )
    };
    let mut paths = vec![prefix.join("bin").join(tool)];
    for entry in std::fs::read_dir(prefix.join("lib")).map_err(io)? {
        let path = entry.map_err(io)?.path();
        if path.is_file()
            && path
                .file_name()
                .is_some_and(|name| name.to_string_lossy().contains(".so"))
        {
            paths.push(path);
        }
    }
    paths.sort();
    let mut parts = vec![crate::COMPILER_IDENTITY.to_vec()];
    for path in paths {
        parts.push(
            path.strip_prefix(prefix)
                .unwrap()
                .as_os_str()
                .as_encoded_bytes()
                .to_vec(),
        );
        parts.push(
            super::file_digest::file(&path)
                .map_err(io)?
                .content
                .0
                .to_vec(),
        );
    }
    Ok(ContentHash::of_parts(
        "hirsel.runtime-transformation.v1",
        parts.iter().map(Vec::as_slice),
    ))
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
    Call {
        roots: Vec<hirsel_run_config::Root>,
        decoder: String,
    },
}
impl LaunchKind {
    pub(crate) fn roots(&self) -> &[hirsel_run_config::Root] {
        match self {
            Self::Zero => &[],
            Self::Main(roots) | Self::Call { roots, .. } => roots,
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
pub(crate) fn launch_kind(
    types: &hirsel_types::TypeInterner,
    signature: &hirsel_types::Signature,
) -> Option<LaunchKind> {
    use hirsel_run_schema::LaunchKind as Kind;
    if hirsel_engine::entry_launch_kind(types, signature).ok()? == Kind::Test {
        return Some(LaunchKind::Test);
    }
    let roots = hirsel_engine::entry_roots(types, signature, Kind::Main).ok()?;
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
    if let Some(LaunchKind::Call { decoder, .. }) = program.launches.get(entry) {
        return Ok((decoder.clone(), 1, false));
    }
    let instance = program
        .functions
        .get(entry)
        .ok_or_else(|| refuse("no such non-generic function"))?;
    let unit = program
        .summaries
        .iter()
        .find(|s| s.definition == *entry && s.instance == *instance)
        .ok_or_else(|| refuse("no unit"))?;
    let abi = &program.table.fn_abis[unit.abi.0 as usize];
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
        .filter(|n| (1..=hirsel_rt::launch::RESULT_MAX).contains(n))
        .ok_or_else(|| refuse("a result that does not fit the inline entry return buffer"))?;
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

/// The objects of `partition`, one per group in plan order, emitted on independent
/// workers. Each template is owned by its assigned object, never chosen from its
/// callers. At O0 callers only import it; at LLVM O2/O3 reached templates and small
/// source units are optimizer-only `available_externally` copies. Their content is
/// therefore a dependency of the caller's object as well as of their own object.
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
/// callee bodies at any optimization level. The last object is the program's glue
/// (`emit::emit_glue`).
pub fn partition_backend_objects(
    program: &ReadyProgram,
    backend: Backend,
    opt: OptLevel,
    partition: Partition,
    registry: &hirsel_lir::descriptor_registry::DescriptorRegistry,
    measurement: bool,
    sources: Option<&hirsel_lir::profile::ProfileSources>,
) -> Result<Vec<hirsel_lir::stage::ObjectArtifact>, NativeError> {
    let jobs = match std::env::var("HIRSEL_JOBS") {
        Ok(text) => hirsel_intern::jobs::Jobs::new(text.parse().map_err(|_| {
            NativeError::new(StageName::Emission, "HIRSEL_JOBS must be a worker count")
        })?),
        Err(_) => hirsel_intern::jobs::Jobs::available(),
    };
    partition_backend_objects_with_jobs(
        program,
        backend,
        opt,
        partition,
        registry,
        measurement,
        sources,
        jobs,
    )
}

/// `partition_backend_objects` with an explicit worker count. Scheduling never changes
/// object contents or order; tests can compare worker counts without process globals.
pub fn partition_backend_objects_with_jobs(
    program: &ReadyProgram,
    backend: Backend,
    opt: OptLevel,
    partition: Partition,
    registry: &hirsel_lir::descriptor_registry::DescriptorRegistry,
    measurement: bool,
    sources: Option<&hirsel_lir::profile::ProfileSources>,
    jobs: hirsel_intern::jobs::Jobs,
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
    // Go's gc/compile.go:138 schedules independent backend work longest first.
    // jobs::map owns no backend handles and collects objects in plan order.
    hirsel_intern::jobs::map(
        jobs,
        groups,
        |group| {
            program
                .units
                .iter()
                .enumerate()
                .filter(|(i, _)| assignment[*i] == *group)
                .map(|(_, u)| u.unit().instructions.len() as u64)
                .sum()
        },
        |group| {
            let _timing = hirsel_intern::timings::phase(hirsel_intern::timings::Phase::Codegen);
            let own: Vec<bool> = (0..program.units.len())
                .map(|i| assignment[i] == group)
                .collect();
            let mut selected = own.clone();
            // Small callees, transitively through small callees.
            let mut copies = std::collections::BTreeSet::new();
            let mut pending: Vec<usize> = (0..own.len()).filter(|&i| own[i]).collect();
            while let Some(i) = pending.pop() {
                for &callee in &calls[i] {
                    if backend == Backend::Llvm
                        && opt != OptLevel::None
                        && (small[callee] || library[callee] || inline[callee])
                        && !std::mem::replace(&mut selected[callee], true)
                    {
                        copies.insert(program.units[callee].unit().header.instance);
                        pending.push(callee);
                    }
                }
            }
            // Known callable bodies, transitively, from every unit emitted.
            let mut pending: Vec<usize> = (0..selected.len()).filter(|&i| selected[i]).collect();
            while let Some(i) = pending.pop() {
                for &dependency in &references[i] {
                    if backend == Backend::Llvm
                        && opt != OptLevel::None
                        && (inline[dependency] || library[dependency])
                        && !std::mem::replace(&mut selected[dependency], true)
                    {
                        copies.insert(program.units[dependency].unit().header.instance);
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
                measurement,
                sources,
            )
        },
    )
    .into_iter()
    .collect::<Result<Vec<_>, _>>()
    .and_then(|mut objects| {
        objects.extend(super::ahead::objects(program, backend, opt, registry)?);
        objects.extend(super::continuation::objects(
            program,
            backend,
            opt,
            measurement,
        )?);
        objects.push(super::emit::emit_glue(program, backend, registry)?);
        if let Some(sources) = sources {
            objects.push(debug_artifact(program, &objects, sources)?);
        }
        Ok(objects)
    })
}

/// The link's debug object (`hirsel_cranelift::debug_object`): line tables for the
/// functions `objects` define, their intervals resolved in `sources`. A body an object
/// recorded intervals for but no longer defines (a template inlined everywhere) has none.
pub(super) fn debug_artifact(
    program: &ReadyProgram,
    objects: &[hirsel_lir::stage::ObjectArtifact],
    sources: &hirsel_lir::profile::ProfileSources,
) -> Result<hirsel_lir::stage::ObjectArtifact, NativeError> {
    use object::{Object as _, ObjectSymbol as _};
    let _timing = hirsel_intern::timings::phase(hirsel_intern::timings::Phase::DebugObject);
    let failed = |message: String| NativeError::new(StageName::Link, message);
    let mut functions = Vec::new();
    for artifact in objects.iter().filter(|o| !o.debug.is_empty()) {
        let file = object::File::parse(&*artifact.bytes)
            .map_err(|e| failed(format!("reading an object for its debug intervals: {e}")))?;
        let defined: std::collections::HashSet<&str> = file
            .symbols()
            .filter(|s| s.is_definition())
            .filter_map(|s| s.name().ok())
            .collect();
        functions.extend(
            artifact
                .debug
                .iter()
                .filter(|f| defined.contains(f.symbol.as_str())),
        );
    }
    let resolve = |item: &DefPath, selector: u32, offset: u32| {
        let source = sources.get(item)?;
        let (file, base) = source.source(selector)?;
        let location = file.location(base.checked_add(offset)?)?;
        Some(hirsel_cranelift::Resolved {
            path: file.logical_path().display().to_string(),
            line: location.line,
            column: location.column,
        })
    };
    let bytes = hirsel_cranelift::debug_object_sources(&functions, &resolve, &program.target)
        .map_err(|e| failed(format!("the debug object: {e}")))?;
    let template = objects.last().expect("the glue object");
    Ok(hirsel_lir::stage::ObjectArtifact {
        ahead: None,
        bytes,
        exports: Vec::new(),
        imports: Vec::new(),
        relocations: Vec::new(),
        debug: Vec::new(),
        ..template.clone()
    })
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

/// Link a test launch with the fixture's Args and virtual mounts bound into its run record.
/// Use `launch_fixture` with the same recipe for initial files and scripted readiness.
pub fn link_fixture_runnable(
    program: &ReadyProgram,
    entry: &DefPath,
    objects: &[hirsel_lir::stage::ObjectArtifact],
    toolchain: &Toolchain,
    dir: &Path,
    fixture: &super::Fixture,
) -> Result<Runnable, NativeError> {
    hirsel_run_schema::validate_file_faults(&fixture.file_faults)
        .map_err(|e| NativeError::new(StageName::Link, e))?;
    if !matches!(program.launches.get(entry), Some(LaunchKind::Main(_))) {
        return link_runnable(program, entry, objects, toolchain, dir, None);
    }
    let run = fixture_run_record(program, entry, fixture)?;
    link_runnable(program, entry, objects, toolchain, dir, Some(&run))
}

/// The exact fixture binding shared by native links and reference/checked tape producers.
/// Building this record performs no link or host operation.
pub fn fixture_run_record(
    program: &ReadyProgram,
    entry: &DefPath,
    fixture: &super::Fixture,
) -> Result<hirsel_run_config::ValidatedRun, NativeError> {
    use hirsel_run_schema::{IoGrants, Mount, ProviderContract as P};
    hirsel_run_schema::validate_file_faults(&fixture.file_faults)
        .map_err(|e| NativeError::new(StageName::Link, e))?;
    let revision = program
        .revisions
        .get(entry)
        .ok_or_else(|| NativeError::new(StageName::Link, "missing checked program revision"))?;
    let mut record =
        hirsel_run_config::RunConfig::for_program(revision, hirsel_run_schema::LaunchKind::Main);
    record.inputs = program.launch.clone();
    record.inputs.fault_seed = fixture.fault_seed;
    record.inputs.fixture_monotonic = fixture
        .process
        .as_ref()
        .filter(|g| g.timed.is_some())
        .and(fixture.monotonic);
    record.inputs.fixture_file_faults =
        (!fixture.file_faults.is_empty()).then(|| fixture.file_faults.clone());
    record.args = Some(fixture.args.clone());
    record.env = fixture.env.clone();
    record.io = Some(IoGrants {
        stdin: Some(P::StdinBytes.name().into()),
        stdout: (!fixture.deny_output).then(|| P::StdoutBytes.name().into()),
        stderr: (!fixture.deny_output).then(|| P::StderrBytes.name().into()),
        tasks: Some(P::TasksCooperative.name().into()),
        random: Some(P::RandomSplitMix64.name().into()),
        monotonic_clock: fixture
            .monotonic
            .map(|_| P::MonotonicNanoseconds.name().into()),
        dirs: fixture
            .mounts
            .iter()
            .map(|(path, access)| Mount {
                path: path.clone(),
                // Snapshot mounts have no host authority. A stable synthetic designator
                // keeps the run binding independent of backend and scratch directory.
                host: if path.is_empty() {
                    "/__hirsel_test_fixture__".into()
                } else {
                    format!("/__hirsel_test_fixture__/{path}")
                },
                access: *access,
            })
            .collect(),
        net: Some(fixture.net.clone().unwrap_or(hirsel_run_schema::NetGrant {
            provider: P::NetScript.name().into(),
            rules: vec![],
            peers: Some(vec![]),
            listeners: vec![],
        })),
        process: Some(
            fixture
                .process
                .clone()
                .unwrap_or(hirsel_run_schema::ProcessGrant {
                    timed: None,
                    provider: P::ProcessScript.name().into(),
                    rules: vec![],
                    runs: Some(vec![]),
                    children: None,
                }),
        ),
        signals: fixture.signals.clone(),
        runners: fixture.runners.clone(),
        telemetry: Some(hirsel_run_schema::TelemetryGrant {
            provider: P::TelemetryNull.name().into(),
            seed: Some(0),
            queue: None,
        }),
        ..Default::default()
    });
    record
        .validated()
        .map_err(|e| NativeError::new(StageName::Link, e.to_string()))
}

/// A standalone executable `dir/program`, including DWARF for a `debug_info` toolchain.
pub(crate) fn link_standalone(
    program: &ReadyProgram,
    entry: &DefPath,
    objects: &[hirsel_lir::stage::ObjectArtifact],
    toolchain: &Toolchain,
    dir: &Path,
) -> Result<Linked, NativeError> {
    link_objects_impl(program, entry, objects, toolchain, dir, None, true)
}

/// Link already emitted objects as a standalone executable, accepting fresh grants at
/// launch. Instrumented benchmark companions use the same launcher contract as `build`.
pub fn link_standalone_objects(
    program: &ReadyProgram,
    entry: &DefPath,
    objects: &[hirsel_lir::stage::ObjectArtifact],
    toolchain: &Toolchain,
    dir: &Path,
) -> Result<PathBuf, NativeError> {
    link_standalone(program, entry, objects, toolchain, dir).map(|linked| linked.path)
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
    launcher_object_with_seed(
        program,
        entry,
        objects,
        dir,
        requested,
        standalone,
        main_symbol,
        None,
    )
}

#[allow(clippy::too_many_arguments)]
fn launcher_object_with_seed(
    program: &ReadyProgram,
    entry: &DefPath,
    objects: &[hirsel_lir::stage::ObjectArtifact],
    dir: &Path,
    requested: Option<&hirsel_run_config::ValidatedRun>,
    standalone: bool,
    main_symbol: &str,
    test_seed: Option<u64>,
) -> Result<Vec<u8>, NativeError> {
    let mut launch_inputs = program.launch.clone();
    if let Some(seed) = test_seed {
        launch_inputs.test_seed = seed;
    }
    program.check_semantic_units(objects)?;
    super::ahead::check_objects(program, objects)?;
    let (symbol, result_size, caller_site) = entry_result(program, entry)?;
    let registry = hirsel_lir::descriptor_registry::DescriptorRegistry::new(
        &program.table,
        &program.finalizers,
    );
    let target = &program.target;
    let kind = &program.launches[entry];
    let roots = kind.roots();
    let call_entry = matches!(kind, LaunchKind::Call { .. });
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
    let callee_report = super::exit_report::Report {
        declarations: String::new(),
        nodes: vec![hirsel_cranelift::ReportNode::default()],
        offset: 0,
        root: 0,
        result: hirsel_lir::layout_types::LayoutId(0),
        return_kind: 1,
        success_offset: 0,
    };
    let report = if call_entry {
        &callee_report
    } else {
        program.exit_reports.get(entry).ok_or_else(|| {
            NativeError::new(StageName::Launch, "missing checked exit-report description")
        })?
    };
    let return_kind = report.return_kind;
    let success_offset = report.success_offset;
    let report_declarations = &report.declarations;
    let report_root = report.root;
    let report_offset = report.offset;
    let result_index = if call_entry {
        0
    } else {
        registry
            .index(report.result)
            .ok_or_else(|| NativeError::new(StageName::Launch, "missing entry-result descriptor"))?
    };
    let caller_parameters = if caller_site { ", i64, i64" } else { "" };
    let caller_arguments = if caller_site { ", i64 0, i64 0" } else { "" };
    let report_branch = if return_kind == 1 {
        " br label %done".to_owned()
    } else {
        " %tag = load i32, ptr %out, align 1\n %error = icmp eq i32 %tag, 1\n br i1 %error, label %report, label %done".to_owned()
    };
    let declaration = if call_entry {
        format!(
            r#"declare i8 @"{symbol}"(ptr)
define private i8 @hirsel.entry(ptr %ctx, ptr %dest, ptr %out, ptr %roots) {{
 %s = call i8 @"{symbol}"(ptr %ctx)
 store i8 0, ptr %out, align 1
 ret i8 %s
}}"#
        )
    } else {
        format!(
            r#"{report_declarations}
declare void @hirsel_rt_v2_exit_report(ptr, ptr, i32, i64, ptr)
declare i8 @hirsel_rt_v2_depth_enter(ptr)
declare void @hirsel_rt_v2_depth_leave(ptr)
declare i8 @"{symbol}"(ptr{caller_parameters}, ptr, ptr{parameters})
define private i8 @hirsel.entry(ptr %ctx, ptr %dest, ptr %out, ptr %roots) {{
 %admitted = call i8 @hirsel_rt_v2_depth_enter(ptr %ctx)
{loads} %s = call i8 @"{symbol}"(ptr %ctx{caller_arguments}, ptr %dest, ptr %out{arguments})
 %ok = icmp eq i8 %s, 0
 br i1 %ok, label %returned, label %done
returned:
{report_branch}
report:
 call void @hirsel_rt_v2_exit_report(ptr %ctx, ptr %out, i32 {result_index}, i64 {report_offset}, ptr @hirsel.report.node.{report_root})
 br label %done
done:
 call void @hirsel_rt_v2_depth_leave(ptr %ctx)
 ret i8 %s
}}"#
        )
    };
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
    let launch = if call_entry {
        hirsel_run_schema::LaunchKind::Call
    } else if *kind == LaunchKind::Test {
        hirsel_run_schema::LaunchKind::Test
    } else {
        hirsel_run_schema::LaunchKind::Main
    };
    let mut record = hirsel_run_config::RunConfig::for_program(revision, launch);
    record.inputs = launch_inputs;
    // The existing conformance launcher supplies this explicit fixture profile.
    // Standalone admission will consume the record at the C entry boundary.
    if matches!(kind, LaunchKind::Main(_)) && !standalone {
        use hirsel_run_schema::{Access, IoGrants, Mount, ProviderContract};
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
            hirsel_run_schema::snapshot_args([])
                .map_err(|error| NativeError::new(StageName::Launch, error.to_string()))?,
        );
    }
    let default = if requested.is_none() {
        Some(
            record
                .validated()
                .map_err(|error| NativeError::new(StageName::Launch, error.to_string()))?,
        )
    } else {
        None
    };
    let run = requested
        .or(default.as_ref())
        .expect("requested or default run");
    run.check_revision(revision, launch)
        .map_err(|e| NativeError::new(StageName::Launch, e.to_string()))?;
    if !standalone {
        std::fs::write(dir.join("run.json"), run.launch_bytes())
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
    // Existing unit summaries retain code identities even for cached units.
    // These diagnostic relocations do not extend the RUN-3 entries table.
    let mut value_codes = std::collections::BTreeSet::new();
    for unit in &program.summaries {
        value_codes.insert(hirsel_lir::emission::symbol_name(
            &unit.definition,
            Some(&unit.instance),
        ));
        for (path, instance) in &unit.glue.entries {
            value_codes.insert(hirsel_lir::emission::function_thunk_name(
                path,
                instance.as_ref(),
            ));
        }
    }
    let callee_facts = program
        .callee_facts
        .as_ref()
        .map(hirsel_rt::program::CalleeMetadata::json);
    let callee_identity = program.callee_facts.as_ref().map_or([0; 32], |facts| {
        hirsel_rt::program::callee_metadata_identity(&revision.digest(), facts)
    });
    let callee_codes: Vec<_> = program
        .entries
        .iter()
        .filter(|(_, path)| {
            program
                .callee_facts
                .as_ref()
                .is_some_and(|facts| facts.facts.contains_key(*path))
        })
        .map(|(thunk, path)| (thunk.clone(), path.as_bytes().to_vec()))
        .collect();
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
            F::Entries(
                program
                    .entries
                    .iter()
                    .map(|(thunk, path)| (thunk.clone(), path.as_bytes().to_vec()))
                    .collect(),
            ),
            F::U64(program.entries.len() as u64),
            F::U64(return_kind),
            F::U64(success_offset),
            F::Entries(
                value_codes
                    .iter()
                    .map(|name| (name.clone(), name.as_bytes().to_vec()))
                    .collect(),
            ),
            F::U64(value_codes.len() as u64),
            match &callee_facts {
                Some(text) => F::Bytes(text.as_bytes().to_vec()),
                None => F::U64(0),
            },
            F::U64(callee_facts.as_ref().map_or(0, |text| text.len() as u64)),
            if callee_codes.is_empty() {
                F::U64(0)
            } else {
                F::Entries(callee_codes.clone())
            },
            F::U64(callee_codes.len() as u64),
            F::Inline(callee_identity.to_vec()),
        ];
        let words: Vec<u64> = roots.iter().map(|r| root_index(*r) as u64).collect();
        let report = hirsel_cranelift::ExitReport {
            result_index,
            is_result: return_kind != 1,
            offset: report_offset,
            root: report_root,
            nodes: report.nodes.clone(),
        };
        let emit = if call_entry {
            hirsel_cranelift::named_callee_launch_object
        } else {
            hirsel_cranelift::named_launch_object
        };
        emit(
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
        // Each Entry row: its function-reference thunk and its definition's full path.
        let mut entry_ir = String::new();
        let mut rows = Vec::new();
        for (index, (thunk, path)) in program.entries.iter().enumerate() {
            entry_ir.push_str(&format!(
                "@hirsel.entry.path.{index} = private constant [{} x i8] [{}]\n\
                 declare void @\"{thunk}\"()\n",
                path.len(),
                bytes(path.as_bytes())
            ));
            rows.push(format!(
                "{{ ptr, ptr, i64 }} {{ ptr @\"{thunk}\", ptr @hirsel.entry.path.{index}, i64 {} }}",
                path.len()
            ));
        }
        entry_ir.push_str(&format!(
            "@hirsel.entries = private constant [{} x {{ ptr, ptr, i64 }}] [{}]\n",
            rows.len(),
            rows.join(", ")
        ));
        let mut code_ir = String::new();
        let mut code_rows = Vec::new();
        for (index, code) in value_codes.iter().enumerate() {
            code_ir.push_str(&format!(
                "@hirsel.value.code.{index} = private constant [{} x i8] [{}]\n",
                code.len(),
                bytes(code.as_bytes())
            ));
            let declaration = format!("@\"{code}\"(");
            if code != &symbol
                && !entry_ir.contains(&declaration)
                && !report_declarations.contains(&declaration)
            {
                code_ir.push_str(&format!("declare void @\"{code}\"()\n"));
            }
            code_rows.push(format!(
                "{{ ptr, ptr, i64 }} {{ ptr @\"{code}\", ptr @hirsel.value.code.{index}, i64 {} }}",
                code.len()
            ));
        }
        code_ir.push_str(&format!(
            "@hirsel.value.codes = private constant [{} x {{ ptr, ptr, i64 }}] [{}]\n",
            code_rows.len(),
            code_rows.join(", ")
        ));
        let mut callee_ir = String::new();
        let callee_facts_pointer = if let Some(text) = &callee_facts {
            callee_ir.push_str(&format!(
                "@hirsel.callee.facts = private constant [{} x i8] [{}]\n",
                text.len(),
                bytes(text.as_bytes())
            ));
            "@hirsel.callee.facts"
        } else {
            "null"
        };
        let callee_rows: Vec<_> = program.entries.iter().enumerate().filter(|(_, (_, path))|
            program.callee_facts.as_ref().is_some_and(|facts| facts.facts.contains_key(*path)))
            .map(|(index, (thunk, path))| format!(
                "{{ ptr, ptr, i64 }} {{ ptr @\"{thunk}\", ptr @hirsel.entry.path.{index}, i64 {} }}", path.len()))
            .collect();
        let callee_code_pointer = if callee_rows.is_empty() {
            "null"
        } else {
            callee_ir.push_str(&format!(
                "@hirsel.callee.codes = private constant [{} x {{ ptr, ptr, i64 }}] [{}]\n",
                callee_rows.len(),
                callee_rows.join(", ")
            ));
            "@hirsel.callee.codes"
        };
        let program_ir = format!(
            "@hirsel.failure.table = private constant [{table_len} x i8] c\"{text}\"\n\
             @hirsel.entry.name = private constant [{name_len} x i8] [{name_bytes}]\n\
             {entry_ir}{code_ir}{callee_ir}\
             @hirsel.program = private constant {{ i32, i32, i32, i32, ptr, i64, ptr, ptr, i64, ptr, i64, [32 x i8], [56 x i8], ptr, i64, i64, i64, ptr, i64, ptr, i64, ptr, i64, [32 x i8] }} \
             {{ i32 {abi}, i32 {flags}, i32 {roots_mask}, i32 {layout_count}, ptr @hirsel.entry, i64 {result_size}, \
             ptr @\"{registry_symbol}\", ptr @hirsel.failure.table, i64 {table_len}, ptr @hirsel.entry.name, i64 {name_len}, \
             [32 x i8] [{digest}], [56 x i8] [{config}], ptr @hirsel.entries, i64 {entry_count}, i64 {return_kind}, i64 {success_offset}, ptr @hirsel.value.codes, i64 {value_code_count}, ptr {callee_facts_pointer}, i64 {callee_facts_length}, ptr {callee_code_pointer}, i64 {callee_code_count}, [32 x i8] [{callee_digest}] }}\n\
             declare i32 @hirsel_rt_v2_main(ptr, i32, ptr)\n",
            table_len = table.len(),
            name_len = name.len(),
            name_bytes = bytes(name),
            abi = hirsel_rt::ABI_VERSION,
            layout_count = registry.external_count(),
            registry_symbol = registry.symbol(),
            digest = bytes(&revision.digest()),
            config = bytes(&config_bytes),
            entry_count = program.entries.len(),
            value_code_count = value_codes.len(),
            callee_facts_length = callee_facts.as_ref().map_or(0, |text| text.len()),
            callee_code_count = callee_codes.len(),
            callee_digest = bytes(&callee_identity),
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

/// Inputs selected and materialized by the caller, in their original link order.
/// No compiler driver, linker executable or ambient foreign-library search is used.
pub struct ExecutableLinkInputs<'a> {
    pub kit: &'a Path,
    /// The already selected debug or stripped runtime archive.
    pub runtime: &'a Path,
    pub foreign: &'a [PathBuf],
    /// None selects the kit's canonical target interpreter.
    pub loader: Option<(PathBuf, PathBuf)>,
}

/// A kit invocation ready for stored-tool capture. Arguments exclude `-o` and
/// its output; generated launcher, unit and run-record files live in `directory`.
pub struct ExecutableLinkPlan {
    pub directory: PathBuf,
    pub arguments: Vec<std::ffi::OsString>,
    pub build_id: [u8; 20],
    pub frozen_kit: Option<super::object_queries::capture::FrozenLinkKit>,
}

impl ExecutableLinkPlan {
    /// Capture with the independently selected frozen tool and caller's bounds.
    pub fn capture(
        &self,
        store: &dyn hirsel_engine::store::ResultStore,
        linker: super::object_queries::capture::FrozenLinker,
        limits: super::object_queries::capture::CaptureLimits,
    ) -> Result<super::object_queries::capture::CapturedLink, String> {
        let arguments = self
            .arguments
            .iter()
            .map(|arg| {
                arg.to_str()
                    .map(str::to_owned)
                    .ok_or_else(|| "non-UTF8 link argument".to_string())
            })
            .collect::<Result<Vec<_>, _>>()?;
        match self.frozen_kit {
            Some(kit) => kit.capture(store, linker, &self.directory, &arguments, limits),
            None => linker.capture(store, &self.directory, &arguments, limits),
        }
    }
}

/// Emit the existing launcher and plan an executable link without spawning a
/// linker. `standalone` accepts fresh launch grants; otherwise `requested` is
/// bound exactly as in `link_objects_with_run`. Object and launch manifests
/// undergo the same checks as the live linker path.
pub fn plan_executable_link(
    program: &ReadyProgram,
    entry: &DefPath,
    objects: &[hirsel_lir::stage::ObjectArtifact],
    directory: &Path,
    requested: Option<&hirsel_run_config::ValidatedRun>,
    standalone: bool,
    inputs: ExecutableLinkInputs<'_>,
) -> Result<ExecutableLinkPlan, NativeError> {
    let objects = executable_objects(program, entry, objects, directory, requested, standalone)?;
    plan_executable_inputs(&program.target, directory, &objects, inputs)
}

/// Emit the ordinary launcher and plan against an immutable selected platform.
/// Runtime/foreign virtual names and transitive reads stay bound to that owner.
pub fn plan_captured_executable_link(
    program: &ReadyProgram,
    entry: &DefPath,
    objects: &[hirsel_lir::stage::ObjectArtifact],
    directory: &Path,
    requested: Option<&hirsel_run_config::ValidatedRun>,
    standalone: bool,
    inputs: CapturedExecutableLinkInputs<'_>,
) -> Result<super::object_queries::platform::CapturedPlatformPlan, NativeError> {
    let objects = executable_objects(program, entry, objects, directory, requested, standalone)?;
    inputs
        .platform
        .plan_inputs(
            inputs.store,
            &program.target,
            directory,
            &objects,
            inputs.loader,
        )
        .map(|plan| plan.with_data_abi(program.data_abi.clone()))
        .map_err(|error| NativeError::new(StageName::Link, error))
}

pub struct CapturedExecutableLinkInputs<'a> {
    pub store: &'a dyn hirsel_engine::store::ResultStore,
    pub platform: &'a super::object_queries::platform::CapturedPlatform,
    pub loader: Option<(PathBuf, PathBuf)>,
}

fn executable_objects(
    program: &ReadyProgram,
    entry: &DefPath,
    objects: &[hirsel_lir::stage::ObjectArtifact],
    directory: &Path,
    requested: Option<&hirsel_run_config::ValidatedRun>,
    standalone: bool,
) -> Result<Vec<PathBuf>, NativeError> {
    let main = launcher_object(
        program, entry, objects, directory, requested, standalone, "main",
    )?;
    let main = write(directory, "main.o", &main)?;
    check_link_manifest(program, entry, objects)?;
    let mut inputs = write_link_objects(directory, objects, &[main])?;
    inputs.extend(data_abi_object(program, directory)?);
    Ok(inputs)
}

/// The published data ABI's identity symbols, which foreign code compiled
/// from a generated header links against (MEM-7, PROG-4).
fn data_abi_object(program: &ReadyProgram, dir: &Path) -> Result<Option<PathBuf>, NativeError> {
    match &program.data_abi {
        Some(abi) => Ok(Some(write(dir, "data-abi.o", &abi.object()?)?)),
        None => Ok(None),
    }
}

/// Fully selected kit input. The ordinary reference binds all actual kit leaves
/// before codegen, including transitive script and archive-member reads.
pub struct FrozenExecutableLinkInputs<'a> {
    pub store: &'a dyn hirsel_engine::store::ResultStore,
    pub kit: super::object_queries::capture::FrozenLinkKit,
    pub runtime: &'a Path,
    pub foreign: &'a [PathBuf],
    pub loader: Option<(PathBuf, PathBuf)>,
}

/// The same launcher/manifest algorithm over a kit captured before codegen.
/// Planning and final capture do not reopen the kit installation.
pub fn plan_frozen_executable_link(
    program: &ReadyProgram,
    entry: &DefPath,
    objects: &[hirsel_lir::stage::ObjectArtifact],
    directory: &Path,
    requested: Option<&hirsel_run_config::ValidatedRun>,
    standalone: bool,
    inputs: FrozenExecutableLinkInputs<'_>,
) -> Result<ExecutableLinkPlan, NativeError> {
    let objects = executable_objects(program, entry, objects, directory, requested, standalone)?;
    plan_frozen_executable_inputs(&program.target, directory, &objects, inputs)
}

pub fn plan_frozen_executable_inputs(
    target: &TargetSpec,
    directory: &Path,
    objects: &[PathBuf],
    inputs: FrozenExecutableLinkInputs<'_>,
) -> Result<ExecutableLinkPlan, NativeError> {
    let kit = inputs
        .kit
        .load(inputs.store, target)
        .map_err(|error| NativeError::new(StageName::Link, error))?;
    let mut plan = kit_plan(
        &kit,
        target,
        directory,
        objects,
        inputs.runtime,
        inputs.foreign,
        inputs.loader,
    )?;
    plan.frozen_kit = Some(inputs.kit);
    Ok(plan)
}

/// Plan the shared kit line for already emitted launcher/unit objects. This also
/// serves foreign executable producers that do not need a Hirsel launcher.
pub fn plan_executable_inputs(
    target: &TargetSpec,
    directory: &Path,
    objects: &[PathBuf],
    inputs: ExecutableLinkInputs<'_>,
) -> Result<ExecutableLinkPlan, NativeError> {
    let kit = super::kit::Kit::load(inputs.kit)?;
    kit_plan(
        &kit,
        target,
        directory,
        objects,
        inputs.runtime,
        inputs.foreign,
        inputs.loader,
    )
}

fn kit_plan(
    kit: &super::kit::Kit,
    target: &TargetSpec,
    directory: &Path,
    objects: &[PathBuf],
    runtime: &Path,
    foreign: &[PathBuf],
    loader: Option<(PathBuf, PathBuf)>,
) -> Result<ExecutableLinkPlan, NativeError> {
    let link = super::kit::Link {
        kit,
        triple: &target.triple,
        kind: super::kit::Kind::Executable { loader },
        objects,
        runtime: Some(runtime),
        unwind: true,
        foreign,
    };
    let (arguments, build_id) = link.invocation()?;
    Ok(ExecutableLinkPlan {
        directory: directory.to_path_buf(),
        arguments,
        build_id,
        frozen_kit: None,
    })
}

fn check_link_manifest(
    program: &ReadyProgram,
    entry: &DefPath,
    objects: &[hirsel_lir::stage::ObjectArtifact],
) -> Result<(), NativeError> {
    let (symbol, _, _) = entry_result(program, entry)?;
    // Link manifest: each definition instance has exactly one implementation
    // across the objects, the entry among them, and every import is another
    // object's export, a runtime entry or a toolchain routine.
    let mut exported = std::collections::BTreeSet::new();
    for export in objects.iter().flat_map(|o| &o.exports) {
        if !exported.insert(export.name.as_str()) {
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
    Ok(())
}

fn write_link_objects(
    dir: &Path,
    objects: &[hirsel_lir::stage::ObjectArtifact],
    launchers: &[PathBuf],
) -> Result<Vec<PathBuf>, NativeError> {
    let mut inputs = launchers.to_vec();
    for (index, object) in objects.iter().enumerate() {
        inputs.push(write(dir, &format!("unit-{index}.o"), &object.bytes)?);
    }
    Ok(inputs)
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
    check_link_manifest(program, entry, objects)?;
    let target = &program.target;
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
    let inputs = write_link_objects(dir, objects, launchers)?;
    if let Output::Image(runner) = output {
        let image = match executable_reason(program, objects, toolchain) {
            Some(reason) => Err(reason),
            None => super::runner::link_image(toolchain, target, runner, &inputs, dir)?,
        };
        match image {
            Ok(image) => {
                return Ok(Linked {
                    path: image.path.clone(),
                    image: Some(image),
                });
            }
            Err(reason) => {
                super::runner::count_fallback(&reason);
                output = Output::Launched;
            }
        }
    }
    // The externs resolve against the root's `unsafe ffi` libraries (PROG-4), and those
    // compiled from a generated header against its data ABI identities (MEM-7).
    let foreign = foreign_inputs(&kit, &target.triple, &program.foreign_links)?;
    let mut inputs = inputs;
    inputs.extend(data_abi_object(program, dir)?);
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
    let (mut command, _) = link.command(&toolchain.lld, &executable)?;
    checked(StageName::Link, &mut command).map_err(|error| {
        match program
            .data_abi
            .as_ref()
            .and_then(|abi| super::c_abi::refusal(abi, &error.message))
        {
            Some(refusal) => NativeError::new(StageName::Link, refusal),
            None => error,
        }
    })?;
    Ok(Linked {
        path: executable,
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

/// A linked file with embedded DWARF when requested, or the program's image.
pub(crate) struct Linked {
    pub path: PathBuf,
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

/// The files `links` name, in order: a library the kit provides (`c`, `m`,
/// `pthread`, ...) is its stub, any other library the first `lib<name>.so` or
/// `lib<name>.a` in the host's system library directories, as `ld -l` searches
/// them. Host libraries are the one link input outside the kit; they enter the
/// build ID by content.
pub(crate) fn selected_foreign_inputs(
    toolchain: &Toolchain,
    target: &TargetSpec,
    links: &[hirsel_engine::LinkInput],
) -> Result<Vec<PathBuf>, NativeError> {
    let kit = super::kit::Kit::load(&toolchain.kit)?;
    foreign_inputs(&kit, &target.triple, links)
}

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

/// Package tests use the same launch ABI and dispatcher as ordinary executable
/// planning, with an exact bound entry table instead of a reconstructed program.
pub(crate) fn plan_captured_test_link(
    program: &ReadyProgram,
    tests: &[(DefPath, u64)],
    objects: &[hirsel_lir::stage::ObjectArtifact],
    dir: &Path,
    inputs: CapturedExecutableLinkInputs<'_>,
) -> Result<super::object_queries::platform::CapturedPlatformPlan, NativeError> {
    if tests.is_empty() {
        return Err(NativeError::new(StageName::Link, "empty native test table"));
    }
    let mut declarations = String::new();
    let mut cases = String::new();
    let mut launchers = vec![dir.join("tests.o")];
    let mut entries = Vec::new();
    for (index, (entry, seed)) in tests.iter().enumerate() {
        let name = format!("hirsel_test_{index}");
        entries.push(name.clone());
        let bytes = launcher_object_with_seed(
            program,
            entry,
            objects,
            dir,
            None,
            false,
            &name,
            Some(*seed),
        )?;
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
    for (entry, _) in tests {
        check_link_manifest(program, entry, objects)?;
    }
    let mut objects = write_link_objects(dir, objects, &launchers)?;
    objects.extend(data_abi_object(program, dir)?);
    inputs
        .platform
        .plan_inputs(inputs.store, &program.target, dir, &objects, inputs.loader)
        .map(|plan| plan.with_data_abi(program.data_abi.clone()))
        .map_err(|error| NativeError::new(StageName::Link, error))
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
    /// The signal that terminated the process, which then has no exit status.
    pub signal: Option<i32>,
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
    Closed(ClosedExecutable),
}

/// The selected executable and its materialized closed runtime share one
/// launch lifetime; no loader or DSO path is rediscovered by process().
#[derive(Clone)]
pub struct ClosedExecutable {
    executable: PathBuf,
    runtime: std::sync::Arc<super::object_queries::runtime::RuntimeFiles>,
    emulator: Option<PathBuf>,
}
impl std::fmt::Debug for ClosedExecutable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ClosedExecutable")
            .field("executable", &self.executable)
            .field("emulator", &self.emulator)
            .finish_non_exhaustive()
    }
}
impl Runnable {
    /// The exact linked artifact consumed by this runnable. Image runners and
    /// closed runtime files remain owned by their respective launch lifetimes.
    pub fn artifact_path(&self) -> &Path {
        match self {
            Self::Executable(path) => path,
            Self::Image(image) => &image.path,
            Self::Closed(executable) => &executable.executable,
        }
    }

    pub(crate) fn closed(
        executable: PathBuf,
        runtime: super::object_queries::runtime::RuntimeFiles,
        emulator: Option<PathBuf>,
    ) -> Self {
        Self::Closed(ClosedExecutable {
            executable,
            runtime: std::sync::Arc::new(runtime),
            emulator,
        })
    }
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
        runnable, target, toolchain, dir, diagnostic, tape, None, None, None,
    )
}

/// Explicit diagnostic launch; the admitted program's environment and tape
/// remain unchanged. The selected runtime product owns observer admission.
pub(crate) fn launch_ownership(
    runnable: &Runnable,
    target: &TargetSpec,
    toolchain: &Toolchain,
    dir: &Path,
    destination: &Path,
    run: Option<&hirsel_run_config::ValidatedRun>,
    tape: Tape<'_>,
) -> Result<Launched, NativeError> {
    if !toolchain.runtime_build.ownership_trace {
        return Err(NativeError::new(
            StageName::Launch,
            "ownership launch requires the explicitly selected runtime product",
        ));
    }
    let mut stdout = std::io::sink();
    let mut stderr = std::io::sink();
    let live = run.map(|run| LiveRun {
        run,
        trace_events: None,
        inherit: false,
        stdout: &mut stdout,
        stderr: &mut stderr,
    });
    launch_impl(
        runnable,
        target,
        toolchain,
        dir,
        false,
        tape,
        live,
        None,
        Some(destination),
    )
}

/// Launch an explicitly selected test world; production launches never use this recipe.
pub fn launch_fixture(
    runnable: &Runnable,
    target: &TargetSpec,
    toolchain: &Toolchain,
    dir: &Path,
    diagnostic: bool,
    tape: Tape<'_>,
    fixture: &super::Fixture,
) -> Result<Launched, NativeError> {
    let run = hirsel_run_config::RunConfig::read_path(&absolute(&dir.join("run.json"))?)
        .map_err(|e| NativeError::new(StageName::Launch, e.to_string()))?;
    if run.record().launch == hirsel_run_schema::LaunchKind::Main && run.record().args.is_some() {
        let mounts = run
            .record()
            .io
            .iter()
            .flat_map(|io| &io.dirs)
            .map(|mount| (mount.path.clone(), mount.access))
            .collect();
        if run.record().args.as_deref().unwrap_or_default() != fixture.args
            || fixture.mounts != mounts
        {
            return Err(NativeError::new(
                StageName::Launch,
                "fixture Args and mounts must match link_fixture_runnable",
            ));
        }
    }
    if run.process_grant().is_some_and(|g| g.timed.is_some()) {
        let initial = run.record().inputs.fixture_monotonic;
        if (!matches!(tape, Tape::Replay(_)) || fixture.monotonic.is_some())
            && fixture.monotonic != initial
        {
            return Err(NativeError::new(
                StageName::Launch,
                "timed fixture initial clock differs from binding",
            ));
        }
        if (!matches!(tape, Tape::Replay(_)) || fixture.process.is_some())
            && fixture.process.as_ref() != run.process_grant()
        {
            return Err(NativeError::new(
                StageName::Launch,
                "timed fixture process script differs from binding",
            ));
        }
    }
    hirsel_run_schema::validate_file_faults(&fixture.file_faults)
        .map_err(|e| NativeError::new(StageName::Launch, e))?;
    // Replay consumes recorded inputs without a live script; an explicitly supplied
    // script must still match the one bound at link time.
    let bound_faults = run.record().inputs.fixture_file_faults.as_ref();
    if (!matches!(tape, Tape::Replay(_)) || !fixture.file_faults.is_empty())
        && bound_faults.cloned().unwrap_or_default() != fixture.file_faults
    {
        return Err(NativeError::new(
            StageName::Launch,
            "fixture file faults must match link_fixture_runnable",
        ));
    }
    if let Tape::Replay(path) = tape {
        let recording = hirsel_rt::host_launch::read_recording(path)
            .map_err(|e| NativeError::new(StageName::Launch, e.to_string()))?;
        if recording.binding.is_some() {
            recording
                .bound_run()
                .map_err(|e| NativeError::new(StageName::Launch, e.to_string()))?;
        }
    }
    let recipe = dir.join("fixture.json");
    std::fs::write(
        &recipe,
        serde_json::to_vec(&(&fixture.files, &fixture.mounts, &fixture.stdin)).unwrap(),
    )
    .map_err(|e| NativeError::new(StageName::Launch, e.to_string()))?;
    launch_impl(
        runnable,
        target,
        toolchain,
        dir,
        diagnostic,
        tape,
        None,
        Some(fixture),
        None,
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
    fixture: Option<&super::Fixture>,
    ownership: Option<&Path>,
) -> Result<Launched, NativeError> {
    let is_fixture = fixture.is_some();
    let default = super::Fixture::default();
    let fixture = fixture.unwrap_or(&default);
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
    if let Some(path) = ownership {
        command.env("HIRSEL_OWNERSHIP_TRACE", absolute(path)?);
    }
    if is_fixture {
        command.env(
            "HIRSEL_RT_FIXTURE_FILES",
            absolute(&dir.join("fixture.json"))?,
        );
        command.env("HIRSEL_RT_FIXTURE_RUN", absolute(&dir.join("run.json"))?);
    }
    if fixture.deny_output {
        command.env("HIRSEL_RT_DENY_OUTPUT", "1");
    }
    if !fixture.file_reads.is_empty() {
        command.env(
            "HIRSEL_RT_FILE_READS",
            serde_json::to_string(&fixture.file_reads).unwrap(),
        );
    }
    if let Some(ticks) = fixture.monotonic {
        command.env("HIRSEL_RT_VIRTUAL_CLOCK", ticks.to_string());
    }
    if let Some(net) = &fixture.net {
        command.env("HIRSEL_RT_NET", serde_json::to_string(net).unwrap());
    }
    if let Some(process) = &fixture.process {
        command.env("HIRSEL_RT_PROCESS", serde_json::to_string(process).unwrap());
    }
    if let Some(signals) = &fixture.signals {
        command.env("HIRSEL_RT_SIGNALS", serde_json::to_string(signals).unwrap());
    }
    if !fixture.runners.is_empty() {
        command.env(
            "HIRSEL_RT_RUNNERS",
            serde_json::to_string(&fixture.runners).unwrap(),
        );
        command.env(
            "HIRSEL_RT_CALLEES",
            hirsel_run_schema::callees_json(&fixture.callees),
        );
    }
    if let Some(seed) = fixture.fault_seed {
        command.env("HIRSEL_RT_FAULT_SEED", seed.to_string());
    }
    if let Some(env) = &fixture.env {
        command.env("HIRSEL_RT_ENV", serde_json::to_string(env).unwrap());
    }
    if fixture.migrate {
        command.env("HIRSEL_RT_MIGRATE", "1");
    }
    if fixture.trace_roots {
        // At a count bound of 3 every root past two holders lives in the runtime's overflow
        // table, so the trace checks exact counts through entry to and exit from it.
        command
            .env("HIRSEL_RT_TRACE_ROOTS", &roots)
            .env("HIRSEL_RT_COUNT_MAX", "3");
    }
    if let Some(path) = live.as_ref().and_then(|live| live.trace_events) {
        command.arg("--hirsel-trace-events").arg(absolute(path)?);
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
                    // Observer controls belong to the host launcher, outside the
                    // admitted program environment and all replay inputs.
                    for name in ["HIRSEL_TIMING", "HIRSEL_TIMING_CAPTURE_ID"] {
                        if let Some(value) = std::env::var_os(name) {
                            command.env(name, value);
                        }
                    }
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
    // F0's free-mode test channel (`hirsel_free::session::TestHook`) reaches the executable.
    if let Some(hook) = std::env::var_os(hirsel_free::session::TestHook::VAR) {
        command.env(hirsel_free::session::TestHook::VAR, hook);
    }
    if let Some(path) = std::env::var_os(hirsel_rt::free_replay::Report::VAR) {
        command.env(hirsel_rt::free_replay::Report::VAR, path);
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
            let observed = live
                .run
                .signals_grant()
                .filter(|grant| grant.script.is_none() && !matches!(tape, Tape::Replay(_)));
            let _deferred = DeferredSignals::to_child(&mut process, observed);
            let output = if ownership.is_some() {
                // Ownership diagnostics retain the existing bounded launch.
                // Grants select ordinary input and effects, never a wider cap.
                bounded_input(&mut process, toolchain.launch_timeout, stdin)?
            } else {
                run_live(&mut process, stdin, live.inherit, live.stdout, live.stderr)?
            };
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
        signal: std::os::unix::process::ExitStatusExt::signal(&output.status),
        stdout: output.stdout,
        stderr: output.stderr,
        // A late replay refusal can leave an existing, empty observation file.
        // Retain its exact diagnostic alongside any valid stdout prefix.
        record: std::fs::read_to_string(&record)
            .ok()
            .filter(|record| !record.is_empty())
            .or_else(|| std::fs::read_to_string(&refusal).ok())
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
        Runnable::Executable(executable) => {
            let mut process = launcher(executable, target, toolchain)?;
            process.env_clear();
            (process, None)
        }
        Runnable::Closed(executable) => {
            let selected = executable
                .runtime
                .command(executable.emulator.as_deref(), &executable.executable);
            let process = if !toolchain.wrapper.is_empty() && executable.emulator.is_none() {
                let mut wrapped = Command::new(&toolchain.wrapper[0]);
                wrapped
                    .env_clear()
                    .args(&toolchain.wrapper[1..])
                    .arg(selected.get_program())
                    .args(selected.get_args());
                for (name, value) in selected.get_envs() {
                    if let Some(value) = value {
                        wrapped.env(name, value);
                    }
                }
                wrapped
            } else {
                selected
            };
            (process, None)
        }
        Runnable::Image(image) => {
            let mut process = launcher(&image.runner.path, target, toolchain)?;
            process.env_clear();
            process
                .arg("exec")
                .arg(tooling)
                .arg(&image.path)
                .arg(&image.path);
            (process, super::runner::hold(&image.path))
        }
    };
    process.args(command.get_args());
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

/// While a live child that observes host signals (`signals.host@1`) runs, the launcher
/// leaves those signals to it: a terminal's Interrupt reaches the whole foreground group, and
/// the run's fate is the child's, not the launcher's. The child starts from the default
/// dispositions; the launcher's are restored when the child is done.
struct DeferredSignals(Vec<(libc::c_int, libc::sighandler_t)>);
impl DeferredSignals {
    fn to_child(command: &mut Command, grant: Option<&hirsel_run_schema::SignalsGrant>) -> Self {
        use std::os::unix::process::CommandExt;
        let Some(grant) = grant else {
            return Self(Vec::new());
        };
        let numbers: Vec<libc::c_int> = grant.kinds.iter().map(|kind| kind.number()).collect();
        let reset = numbers.clone();
        // SAFETY: signal(2) is async-signal-safe, and the closure allocates nothing.
        unsafe {
            command.pre_exec(move || {
                for number in &reset {
                    libc::signal(*number, libc::SIG_DFL);
                }
                Ok(())
            });
        }
        Self(
            numbers
                .into_iter()
                // SAFETY: replaces this process's disposition, which drop restores.
                .map(|number| (number, unsafe { libc::signal(number, libc::SIG_IGN) }))
                .collect(),
        )
    }
}
impl Drop for DeferredSignals {
    fn drop(&mut self) {
        for (number, previous) in self.0.drain(..).rev() {
            // SAFETY: restores the disposition `to_child` replaced.
            unsafe { libc::signal(number, previous) };
        }
    }
}

fn absolute(path: &Path) -> Result<PathBuf, NativeError> {
    std::path::absolute(path).map_err(|e| NativeError::new(StageName::Launch, e.to_string()))
}

pub(crate) struct LiveRun<'a> {
    pub run: &'a hirsel_run_config::ValidatedRun,
    pub trace_events: Option<&'a Path>,
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
        None,
        None,
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
    Ran(Observation, Vec<hirsel_run_schema::Wait>),
    /// The launch was refused before entry: a `RunRefusal`'s JSON, or the launcher's reason.
    Refused(String),
    /// The run stopped: `Refused`, `Diverged` or `Environmental`.
    Stopped(hirsel_run_schema::RunOutcome),
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
    observe_run_tables(
        launched,
        &failure_rules(program),
        &site_codes(program),
        sites,
    )
}

/// Decode a completed immutable native request using only its compact linked
/// failure tables and selected source anchors. This constructs no Value/Ready
/// body and applies the same audit, exit, refusal and malformed-record checks.
pub fn observe_request(
    launched: &Launched,
    metadata: &crate::requests::NativeObservation,
    sources: &hirsel_span::SourceMap,
) -> Result<Observed, NativeError> {
    let sites = Sites::from_anchors(
        sources,
        metadata
            .anchors
            .iter()
            .map(|(path, anchors)| (path, anchors.clone()))
            .collect(),
    );
    observe_run_tables(launched, &metadata.failure_rules, &metadata.sites, &sites)
}

fn observe_run_tables(
    launched: &Launched,
    rules: &[String],
    codes: &BTreeMap<u64, (DefPath, hirsel_lir::ir::SiteRange)>,
    sites: &Sites<'_>,
) -> Result<Observed, NativeError> {
    let bad = |what: String| {
        NativeError::new(
            StageName::Launch,
            format!(
                "{what} (exit {:?}, signal {:?}, stderr {:?})",
                launched.exit,
                launched.signal,
                String::from_utf8_lossy(&launched.stderr)
            ),
        )
    };
    // A host signal's default action ends the process before it writes a record.
    if launched.record.is_empty()
        && launched.exit.is_none()
        && let Some(kind) = hirsel_run_schema::SignalKind::ALL
            .into_iter()
            .find(|kind| launched.signal == Some(kind.number()))
    {
        return Ok(Observed::Ran(
            Observation {
                exit_code: None,
                blocked: false,
                budget: None,
                signaled: Some(kind),
                roots: Vec::new(),
                output: launched.stdout.clone(),
                failures: Vec::new(),
            },
            Vec::new(),
        ));
    }
    let mut lines = launched.record.lines();
    let version = format!(
        "hirsel-observation {}",
        hirsel_rt::launch::OBSERVATION_VERSION
    );
    if lines.next() != Some(version.as_str()) {
        return Err(bad("the process left no observation record".into()));
    }
    let mut status = None;
    let mut result = None;
    let mut reported_code = None;
    let mut failures = Vec::new();
    let mut audit = None;
    let mut blocked = false;
    let mut waits = None;
    let mut ended = false;
    let mut budget = None;
    let mut signaled = None;
    let mut stopped = None;
    let mut placement_seen = false;
    for line in lines {
        let words: Vec<&str> = line.split(' ').collect();
        let rest = |n: usize| words[n..].join(" ");
        match words.as_slice() {
            ["refused", ..] => {
                if launched.exit != Some(hirsel_run_schema::exit::REFUSED) {
                    return Err(bad("a refused launch did not exit as refused".into()));
                }
                return Ok(Observed::Refused(rest(1)));
            }
            ["stopped", "refused", ..] => {
                stopped = Some(hirsel_run_schema::RunOutcome::Refused(rest(2)))
            }
            ["stopped", "diverged", ..] => {
                stopped = Some(hirsel_run_schema::RunOutcome::Diverged(rest(2)))
            }
            ["stopped", "environmental", "codec_limit"] => {
                stopped = Some(hirsel_run_schema::RunOutcome::Environmental(
                    hirsel_run_schema::Environmental::CodecLimit,
                ))
            }
            ["stopped", "environmental", "recording", ..] => {
                stopped = Some(hirsel_run_schema::RunOutcome::Environmental(
                    hirsel_run_schema::Environmental::Recording(rest(3)),
                ))
            }
            ["stopped", "environmental", "output", ..] => {
                stopped = Some(hirsel_run_schema::RunOutcome::Environmental(
                    hirsel_run_schema::Environmental::Output(rest(3)),
                ))
            }
            ["stopped", "environmental", "stopped"] => {
                stopped = Some(hirsel_run_schema::RunOutcome::Environmental(
                    hirsel_run_schema::Environmental::Stopped,
                ))
            }
            ["placement", value] => {
                if std::mem::replace(&mut placement_seen, true) {
                    return Err(bad("duplicate placement report".into()));
                }
                hirsel_run_config::RecordedPlacement::from_json(&{
                    if value.len() % 2 != 0 || !value.bytes().all(|b| b.is_ascii_hexdigit()) {
                        return Err(bad("invalid placement hex".into()));
                    }
                    (0..value.len())
                        .step_by(2)
                        .map(|i| u8::from_str_radix(&value[i..i + 2], 16).unwrap())
                        .collect::<Vec<_>>()
                })
                .map_err(|e| bad(e.to_string()))?;
            }
            ["status", s] => status = s.parse::<u8>().ok(),
            ["blocked"] => blocked = true,
            ["waits", ..] => {
                if waits.is_some() {
                    return Err(bad("duplicate wait report".into()));
                }
                waits = Some(hirsel_run_schema::Wait::decode(&rest(1)).map_err(bad)?);
            }
            ["ended"] => ended = true,
            ["budget_exceeded", kind] => {
                budget = Some(
                    hirsel_run_schema::BudgetKind::parse(kind)
                        .ok_or_else(|| bad(format!("an unknown budget in `{line}`")))?,
                )
            }
            ["signaled", kind] => {
                signaled = Some(
                    hirsel_run_schema::SignalKind::parse(kind)
                        .ok_or_else(|| bad(format!("an unknown signal in `{line}`")))?,
                )
            }
            ["result", hex] => result = Some(*hex),
            ["exit_code", code] => {
                if reported_code.is_some() {
                    return Err(bad("duplicate exit code".into()));
                }
                reported_code = Some(i32::from(
                    code.parse::<u8>()
                        .map_err(|_| bad("invalid exit code".into()))?,
                ));
            }
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
                        file: operation.file,
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
                signaled: None,
                output: launched.stdout.clone(),
                failures,
                roots: Vec::new(),
            },
            vec![],
        );
    }
    if let Some(kind) = signaled {
        if blocked || status.is_some() || result.is_some() || !failures.is_empty() || !unaudited {
            return Err(bad("an inconsistent signaled observation record".into()));
        }
        // Terminated by the signal itself, never an exit status that spells it.
        if launched.signal != Some(kind.number()) || launched.exit.is_some() {
            return Err(bad(format!(
                "a signaled process was not ended by {}",
                kind.name()
            )));
        }
        return Ok(Observed::Ran(
            Observation {
                exit_code: None,
                blocked: false,
                budget: None,
                signaled: Some(kind),
                roots: Vec::new(),
                output: launched.stdout.clone(),
                failures,
            },
            Vec::new(),
        ));
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
                signaled: None,
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
        (Some(0), Some(hex), true) => {
            Some(reported_code.unwrap_or_else(|| i32::from(!hex.starts_with("00000000"))))
        }
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
            signaled: None,
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
    waits: Vec<hirsel_run_schema::Wait>,
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
            products: std::sync::Arc::new(hirsel_engine::store::NoneStore),
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
        let dir_scratch = tempfile::Builder::new()
            .prefix("hirsel-memcheck-env-")
            .tempdir()
            .unwrap();
        let dir = dir_scratch.path().to_path_buf();
        let mut tools = Toolchain {
            products: std::sync::Arc::new(hirsel_engine::store::NoneStore),
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

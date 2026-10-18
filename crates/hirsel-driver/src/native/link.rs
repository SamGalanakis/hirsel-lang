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
    /// The pinned clang, which drives the x86-64 link.
    pub clang: PathBuf,
    /// The AArch64 cross compiler driver.
    pub aarch64_cc: PathBuf,
    /// The user-mode emulator AArch64 executables run under, and its sysroot.
    pub qemu_aarch64: PathBuf,
    pub aarch64_sysroot: PathBuf,
    /// The pinned memcheck executable and its relocated support directory.
    pub valgrind: PathBuf,
    pub valgrind_lib: PathBuf,
    /// Matching libc/loader and debug symbols for memcheck's mandatory redirects.
    pub valgrind_runtime: PathBuf,
    /// `libhirsel_rt.a` per target triple and archive variant.
    pub runtime: BTreeMap<(String, RuntimeBuild), PathBuf>,
    /// A checking tool an x86-64 executable is launched under (Valgrind
    /// memcheck with a failing exit code), or empty.
    pub wrapper: Vec<String>,
    /// How long one launched process may run before it is killed and its
    /// launch fails.
    pub launch_timeout: Duration,
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
    command
        .output()
        .map_err(|e| NativeError::new(stage, format!("could not run {command:?}: {e}")))
}

/// Run `command` in its own process group to completion, or kill the group
/// once `timeout` has passed and fail.
fn bounded(command: &mut Command, timeout: Duration) -> Result<std::process::Output, NativeError> {
    use std::io::Read;
    use std::os::unix::process::CommandExt;
    use std::process::Stdio;
    let mut child = command
        .process_group(0)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| {
            NativeError::new(StageName::Launch, format!("could not run {command:?}: {e}"))
        })?;
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
    let stdout = drain(child.stdout.take().map(|p| Box::new(p) as _));
    let stderr = drain(child.stderr.take().map(|p| Box::new(p) as _));
    let deadline = Instant::now() + timeout;
    let timed_out = (0..2).any(|_| {
        ended
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            .is_err()
    });
    if timed_out {
        // SAFETY: kill(2) on the child's own process group.
        unsafe { libc::kill(-(child.id() as libc::pid_t), libc::SIGKILL) };
    }
    let status = child.wait().map_err(|e| {
        NativeError::new(StageName::Launch, format!("waiting for {command:?}: {e}"))
    })?;
    let stdout = stdout.join().unwrap_or_default();
    let stderr = stderr.join().unwrap_or_default();
    if timed_out {
        return Err(NativeError::new(
            StageName::Launch,
            format!(
                "the process ran past the {} s launch timeout and was killed (stderr {:?})",
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

fn checked(stage: StageName, command: &mut Command) -> Result<(), NativeError> {
    let output = tool(stage, command)?;
    if output.status.success() {
        return Ok(());
    }
    Err(NativeError::new(
        stage,
        format!(
            "{command:?} failed with {}: {}{}",
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
        let [runtime_build, ..] = *runtime_builds else {
            return Err(NativeError::new(
                StageName::Link,
                "a toolchain needs a runtime archive",
            ));
        };
        let prefix = environment_path("LLVM_SYS_211_PREFIX")?;
        let aarch64_cc = environment_path("HIRSEL_AARCH64_CC")?;
        // A build that makes the runtime archives itself (Buck2, tools/buck2/README.md)
        // names their directory: `<dir>/<triple>/<features or default>/libhirsel_rt.a`.
        // Cargo is then never run, and a missing archive is an error.
        let prebuilt = std::env::var_os("HIRSEL_RUNTIME_ARCHIVES").map(PathBuf::from);
        let mut runtime = BTreeMap::new();
        for (target, &runtime_build) in targets
            .iter()
            .flat_map(|t| runtime_builds.iter().map(move |b| (t, b)))
        {
            if let Some(dir) = &prebuilt {
                let archive = dir
                    .join(&target.triple)
                    .join(runtime_build.variant())
                    .join("libhirsel_rt.a");
                if !archive.is_file() {
                    return Err(NativeError::new(
                        StageName::Link,
                        format!("no runtime archive {}", archive.display()),
                    ));
                }
                runtime.insert((target.triple.clone(), runtime_build), archive);
                continue;
            }
            let archive = super::runtime_cache::archive(
                build_dir,
                target,
                &prefix,
                runtime_build,
                |build_dir| {
                    let mut command =
                        Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()));
                    command
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
                        command
                            .args(["--target", &target.triple])
                            .env("CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER", &aarch64_cc);
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
            runtime.insert((target.triple.clone(), runtime_build), archive);
        }
        Ok(Self {
            runtime_build,
            clang: prefix.join("bin/hirsel-clang"),
            aarch64_cc,
            qemu_aarch64: environment_path("HIRSEL_QEMU_AARCH64")?,
            aarch64_sysroot: environment_path("HIRSEL_AARCH64_SYSROOT")?,
            valgrind: environment_path("HIRSEL_VALGRIND")?,
            valgrind_lib: environment_path("VALGRIND_LIB")?,
            valgrind_runtime: environment_path("HIRSEL_VALGRIND_RUNTIME")?,
            runtime,
            wrapper: Vec::new(),
            launch_timeout: LAUNCH_TIMEOUT,
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

    /// The selected archive for `triple`.
    pub fn archive(&self, triple: &str) -> Option<&Path> {
        self.runtime
            .get(&(triple.to_string(), self.runtime_build))
            .map(PathBuf::as_path)
    }
}

/// Tools are explicit action inputs. Bare executable names would search libc's
/// default PATH after launch clears the environment, escaping the selected tree.
pub(super) fn environment_path(name: &str) -> Result<PathBuf, NativeError> {
    configured_path(name, std::env::var_os(name))
}

fn configured_path(name: &str, value: Option<std::ffi::OsString>) -> Result<PathBuf, NativeError> {
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
    if hirsel_engine::entry_roots(signature, Kind::Test).is_ok() {
        return Some(LaunchKind::Test);
    }
    let roots = hirsel_engine::entry_roots(signature, Kind::Main).ok()?;
    Some(if roots.is_empty() {
        LaunchKind::Zero
    } else {
        LaunchKind::Main(roots)
    })
}
fn entry_result(program: &ReadyProgram, entry: &DefPath) -> Result<(String, u64), NativeError> {
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
    sources: Option<&hirsel_llvm::ProfileSources>,
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
    sources: Option<&hirsel_llvm::ProfileSources>,
) -> Result<Vec<hirsel_lir::stage::ObjectArtifact>, NativeError> {
    use hirsel_lir::ir::{Callee, Inst, UnitOrigin};
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
    sources: Option<&hirsel_llvm::ProfileSources>,
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
    sources: Option<&hirsel_llvm::ProfileSources>,
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
    sources: Option<&hirsel_llvm::ProfileSources>,
) -> Result<Vec<hirsel_lir::stage::ObjectArtifact>, NativeError> {
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
}

pub(crate) fn link_standalone(
    program: &ReadyProgram,
    entry: &DefPath,
    objects: &[hirsel_lir::stage::ObjectArtifact],
    toolchain: &Toolchain,
    dir: &Path,
) -> Result<PathBuf, NativeError> {
    link_objects_impl(program, entry, objects, toolchain, dir, None, true)
}

fn link_objects_impl(
    program: &ReadyProgram,
    entry: &DefPath,
    objects: &[hirsel_lir::stage::ObjectArtifact],
    toolchain: &Toolchain,
    dir: &Path,
    requested: Option<&hirsel_run_config::ValidatedRun>,
    standalone: bool,
) -> Result<PathBuf, NativeError> {
    let (symbol, result_size) = entry_result(program, entry)?;
    let registry = hirsel_lir::descriptor_registry::DescriptorRegistry::new(
        &program.table,
        &program.finalizers,
    );
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
    let declaration = format!(
        r#"{report_declarations}
declare void @hirsel_rt_v2_exit_report(ptr, ptr, i32, i64, ptr)
declare i8 @"{symbol}"(ptr, ptr, ptr{parameters})
define private i8 @hirsel.entry(ptr %ctx, ptr %dest, ptr %out, ptr %roots) {{
{loads} %s = call i8 @"{symbol}"(ptr %ctx, ptr %dest, ptr %out{arguments})
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
    // hirsel-rt's UNREPLAYABLE bit beside the roots (PROG-4).
    let mask = kind.mask() | if program.foreign_unreplayable { 32 } else { 0 };
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
    let config = run.config();
    let identity = config
        .run_identity()
        .iter()
        .map(|b| format!("i8 {b}"))
        .collect::<Vec<_>>()
        .join(", ");
    let launch_config = format!(
        "@hirsel.launch.config = private constant {{i64, i64, i64, [32 x i8]}} {{i64 {}, i64 {}, i64 {}, [32 x i8] [{identity}]}}",
        config.hash_seed(),
        config.random_seed(),
        config.test_seed()
    );
    let (launcher, call, binding) = if standalone {
        let digest = revision
            .digest()
            .iter()
            .map(|b| format!("i8 {b}"))
            .collect::<Vec<_>>()
            .join(", ");
        let name = revision.entry().as_bytes();
        let name_bytes = name
            .iter()
            .map(|b| format!("i8 {b}"))
            .collect::<Vec<_>>()
            .join(", ");
        (
            "declare i32 @hirsel_rt_v2_launch_standalone(ptr, i64, ptr, i32, ptr, ptr, i64, i8)"
                .to_string(),
            format!(
                "call i32 @hirsel_rt_v2_launch_standalone(ptr @hirsel.entry, i64 {result_size}, ptr @\"{}\", i32 {}, ptr @hirsel.program, ptr @hirsel.entry.name, i64 {}, i8 {mask})",
                registry.symbol(),
                registry.external_count(),
                name.len()
            ),
            format!(
                "@hirsel.program = private constant [32 x i8] [{digest}]\n@hirsel.entry.name = private constant [{} x i8] [{name_bytes}]",
                name.len()
            ),
        )
    } else {
        (
            "declare i32 @hirsel_rt_v2_launch_roots(ptr, i64, ptr, i32, ptr, i8)".to_string(),
            format!(
                "call i32 @hirsel_rt_v2_launch_roots(ptr @hirsel.entry, i64 {result_size}, ptr @\"{}\", i32 {}, ptr @hirsel.launch.config, i8 {mask})",
                registry.symbol(),
                registry.external_count()
            ),
            launch_config,
        )
    };
    // A program that isolates registers its failure table before the launch.
    let table = failure_table(program)?;
    // The launcher object comes from the backend that emitted the program's objects, so a
    // Cranelift build never runs LLVM.
    let cranelift = !objects.is_empty()
        && objects.iter().all(|o| {
            o.reproducibility
                .toolchain
                .starts_with(hirsel_cranelift::TOOLCHAIN)
        });
    let main = if cranelift {
        use hirsel_cranelift::LaunchArgument as A;
        let registry_arguments = [
            A::Symbol(registry.symbol()),
            A::Int {
                bits: 32,
                value: u64::from(registry.external_count()),
            },
        ];
        let mut arguments = vec![
            A::Entry,
            A::Int {
                bits: 64,
                value: result_size,
            },
        ];
        arguments.extend(registry_arguments);
        let launcher = if standalone {
            let name = revision.entry().as_bytes();
            arguments.extend([
                A::Bytes(revision.digest().to_vec()),
                A::Bytes(name.to_vec()),
                A::Int {
                    bits: 64,
                    value: name.len() as u64,
                },
            ]);
            "hirsel_rt_v2_launch_standalone"
        } else {
            let mut bytes = Vec::with_capacity(56);
            for seed in [config.hash_seed(), config.random_seed(), config.test_seed()] {
                bytes.extend(seed.to_le_bytes());
            }
            bytes.extend(config.run_identity());
            arguments.push(A::Bytes(bytes));
            "hirsel_rt_v2_launch_roots"
        };
        arguments.push(A::Int {
            bits: 8,
            value: u64::from(mask),
        });
        let words: Vec<u64> = roots.iter().map(|r| root_index(*r) as u64).collect();
        let report = hirsel_cranelift::ExitReport {
            result_index,
            offset: report_offset,
            root: report_root,
            nodes: report.nodes.clone(),
        };

        let calls: Vec<_> = table
            .into_iter()
            .map(|bytes| {
                let length = bytes.len() as u64;
                (
                    "hirsel_rt_v2_failure_table",
                    vec![
                        A::Bytes(bytes),
                        A::Int {
                            bits: 64,
                            value: length,
                        },
                    ],
                )
            })
            .collect();
        hirsel_cranelift::launch_object(
            target, &symbol, &words, &report, &calls, launcher, &arguments,
        )
        .map_err(|e| NativeError::new(StageName::Link, e.to_string()))?
    } else {
        let (table, register) = failure_table_ir(table.as_deref());
        let shim = format!(
            "target triple = \"{}\"\n{registry_declaration}\n{binding}\n{declaration}\n{launcher}\n{table}declare ptr @signal(i32, ptr)\ndefine i32 @main() {{\n %old_sigpipe = call ptr @signal(i32 13, ptr inttoptr (i64 1 to ptr))\n{register} %r = {call}\n ret i32 %r\n}}\n",
            target.triple
        );
        hirsel_llvm::compile_ir(&shim, target, OptLevel::None)
            .map_err(|e| NativeError::new(StageName::Link, e.to_string()))?
            .bytes
    };
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
    let write = |name: &str, bytes: &[u8]| {
        let path = dir.join(name);
        std::fs::write(&path, bytes)
            .map(|()| path)
            .map_err(|e| NativeError::new(StageName::Link, format!("{name}: {e}")))
    };
    let executable = dir.join("program");
    let mut command = Command::new(if aarch64(target) {
        &toolchain.aarch64_cc
    } else {
        &toolchain.clang
    });
    if !standalone {
        command.args(toolchain.loader_flags(target));
    }
    command.arg(write("main.o", &main)?);
    for (index, object) in objects.iter().enumerate() {
        command.arg(write(&format!("unit-{index}.o"), &object.bytes)?);
    }
    command.arg(archive);
    // The externs resolve against the root's `unsafe ffi` libraries (PROG-4).
    for link in &program.foreign_links {
        match link {
            hirsel_engine::LinkInput::Library(name) => command.arg(format!("-l{name}")),
            hirsel_engine::LinkInput::File(path) => command.arg(path),
        };
    }
    if standalone {
        // Bundle compiler support and math objects as well as the Rust runtime.
        // On glibc Linux the remaining system interfaces are supplied by libc.
        command.args([
            "-static-libgcc",
            "-Wl,--as-needed",
            "-Wl,-Bstatic",
            "-lm",
            "-Wl,-Bdynamic",
            "-ldl",
            "-lpthread",
        ]);
    } else {
        command.args(["-lgcc_s", "-lutil", "-lrt", "-lpthread", "-lm", "-ldl"]);
    }
    command.arg("-o").arg(&executable);
    checked(StageName::Link, &mut command)?;
    Ok(executable)
}

/// The encoded failure table a program that isolates registers before its entry runs
/// (`hirsel_rt::isolate`); None for any other program.
fn failure_table(program: &ReadyProgram) -> Result<Option<Vec<u8>>, NativeError> {
    if !super::observe::isolates(program) {
        return Ok(None);
    }
    let table = program.failure_table.as_ref().ok_or_else(|| {
        NativeError::new(
            StageName::Link,
            "a program that isolates links without its failure table",
        )
    })?;
    Ok(Some(table.encode()))
}

/// The LLVM module text holding an encoded failure table and the call in `main` registering
/// it; both empty without one.
fn failure_table_ir(table: Option<&[u8]>) -> (String, String) {
    let Some(bytes) = table else {
        return (String::new(), String::new());
    };
    let text: String = bytes.iter().map(|b| format!("\\{b:02X}")).collect();
    (
        format!(
            "@hirsel.failure.table = private constant [{} x i8] c\"{text}\"\ndeclare void @hirsel_rt_v2_failure_table(ptr, i64)\n",
            bytes.len()
        ),
        format!(
            " call void @hirsel_rt_v2_failure_table(ptr @hirsel.failure.table, i64 {})\n",
            bytes.len()
        ),
    )
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
    launch_impl(executable, target, toolchain, dir, diagnostic, tape, None)
}

/// Harness execution or a user launch with an explicit admitted run record.
fn launch_impl(
    executable: &Path,
    target: &TargetSpec,
    toolchain: &Toolchain,
    dir: &Path,
    diagnostic: bool,
    tape: Tape<'_>,
    live: Option<LiveRun<'_>>,
) -> Result<Launched, NativeError> {
    let root = dir.join("root");
    let record = dir.join("observation");
    let _ = std::fs::remove_dir_all(&root);
    let _ = std::fs::remove_file(&record);
    std::fs::create_dir_all(&root)
        .map_err(|e| NativeError::new(StageName::Launch, format!("granted directory: {e}")))?;
    let mut command = if aarch64(target) {
        let mut command = Command::new(&toolchain.qemu_aarch64);
        command
            .arg("-L")
            .arg(&toolchain.aarch64_sysroot)
            .arg(executable);
        command
    } else if let Some((program, arguments)) = toolchain.wrapper.split_first() {
        let mut command = Command::new(program);
        command.args(arguments).arg(executable);
        command
    } else {
        Command::new(executable)
    };
    command
        .env_clear()
        .env("HIRSEL_RT_ROOT", &root)
        .env("HIRSEL_RT_OBSERVATION", &record);
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
                if live.as_ref().is_some_and(|live| live.run.is_some()) {
                    command.arg("--hirsel-run").arg(dir.join("run.json"));
                }
            } else if let Some(path) = record {
                command.env("HIRSEL_RT_TAPE", path);
            }
        }
    }
    if !toolchain.wrapper.is_empty() {
        command
            .env("VALGRIND_LIB", &toolchain.valgrind_lib)
            .env("LD_LIBRARY_PATH", &toolchain.valgrind_runtime);
    }
    if toolchain.runtime_build.measurement {
        command.env("HIRSEL_RT_MEASUREMENT", dir.join("measurement"));
    }
    if diagnostic {
        command.env("HIRSEL_RT_DIAGNOSTIC", "1");
    }
    let output = if let Some(live) = live {
        // A replay reads no host input.
        let stdin = !matches!(tape, Tape::Replay(_))
            && live
                .run
                .and_then(|run| run.record().io.as_ref())
                .is_some_and(|io| io.stdin.is_some());
        run_live(&mut command, stdin, live.inherit, live.stdout, live.stderr)?
    } else {
        bounded(&mut command, toolchain.launch_timeout)?
    };
    Ok(Launched {
        audit_required: toolchain.runtime_build.diagnostic || toolchain.runtime_build.measurement,
        exit: output.status.code(),
        stdout: output.stdout,
        stderr: output.stderr,
        record: std::fs::read_to_string(&record).unwrap_or_default(),
    })
}

fn absolute(path: &Path) -> Result<PathBuf, NativeError> {
    std::path::absolute(path).map_err(|e| NativeError::new(StageName::Launch, e.to_string()))
}

pub(crate) struct LiveRun<'a> {
    pub run: Option<&'a hirsel_run_config::ValidatedRun>,
    pub inherit: bool,
    pub stdout: &'a mut dyn std::io::Write,
    pub stderr: &'a mut dyn std::io::Write,
}

/// A user's server runs until it exits or is interrupted; harness timeouts remain tooling.
pub(crate) fn launch_live(
    executable: &Path,
    target: &TargetSpec,
    toolchain: &Toolchain,
    dir: &Path,
    tape: Tape<'_>,
    live: LiveRun<'_>,
) -> Result<Launched, NativeError> {
    launch_impl(executable, target, toolchain, dir, false, tape, Some(live))
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
pub fn observe(
    launched: &Launched,
    program: &ReadyProgram,
    sites: &Sites<'_>,
) -> Result<Observation, NativeError> {
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
    for line in lines {
        let words: Vec<&str> = line.split(' ').collect();
        match words.as_slice() {
            ["status", s] => status = s.parse::<u8>().ok(),
            ["result", hex] => result = Some(*hex),
            ["failure", _kind, rule, site] => {
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
                failures.push(sites.failure(&rule, definition, *range)?);
            }
            ["audit", live_blocks, _bytes, _immortal, regions, retained] => {
                audit = Some((*live_blocks, *regions, *retained));
            }
            ["audit", "unavailable"] => audit = Some(("unavailable", "unavailable", "unavailable")),
            _ => return Err(bad(format!("a malformed observation line `{line}`"))),
        }
    }
    if audit != Some(("0", "0", "0"))
        && (launched.audit_required || audit != Some(("unavailable", "unavailable", "unavailable")))
    {
        return Err(bad(format!(
            "the runtime's exit audit is not clean: (live blocks, regions, retained) = {audit:?}"
        )));
    }
    let exit_code = match (status, result, failures.is_empty()) {
        (Some(0), Some(hex), true) => Some(i32::from(!hex.starts_with("00000000"))),
        (Some(1), None, false) => None,
        _ => return Err(bad("an inconsistent observation record".into())),
    };
    let expected = exit_code.unwrap_or(hirsel_rt::launch::EXIT_FAILURE);
    if launched.exit != Some(expected) {
        return Err(bad(format!("the process did not exit {expected}")));
    }
    Ok(Observation {
        exit_code,
        output: launched.stdout.clone(),
        failures,
    })
}

impl Toolchain {
    /// This toolchain with x86-64 executables launched under Valgrind
    /// memcheck: an invalid read or write, a use of uninitialised memory in a
    /// branch or system call and a definite leak each fail the launch.
    /// Memcheck runs a program tens of times slower, so its launches get ten
    /// times the timeout.
    /// The link flags that make an executable for `target` load as this toolchain launches
    /// it: under the wrapper, Valgrind's pinned dynamic loader and runtime libraries.
    pub fn loader_flags(&self, target: &TargetSpec) -> Vec<std::ffi::OsString> {
        if aarch64(target) || self.wrapper.is_empty() {
            return Vec::new();
        }
        let runtime = &self.valgrind_runtime;
        [
            "-Xlinker".into(),
            "--dynamic-linker".into(),
            "-Xlinker".into(),
            runtime.join("ld-linux-x86-64.so.2").into(),
            "-Xlinker".into(),
            "-rpath".into(),
            "-Xlinker".into(),
            runtime.into(),
        ]
        .to_vec()
    }

    pub fn under_memcheck(mut self) -> Self {
        self.launch_timeout = LAUNCH_TIMEOUT * 10;
        self.wrapper = [
            self.valgrind.to_string_lossy().as_ref(),
            "--quiet",
            "--error-exitcode=97",
            "--leak-check=full",
            "--errors-for-leak-kinds=definite",
        ]
        .map(String::from)
        .to_vec();
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn memcheck_launch_keeps_its_input_library_directory_after_env_clear() {
        let dir = std::env::temp_dir().join(format!("hirsel-memcheck-env-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let tools = Toolchain {
            runtime_build: RuntimeBuild::default(),
            clang: "/unused/clang".into(),
            aarch64_cc: "/unused/gcc".into(),
            qemu_aarch64: "/unused/qemu".into(),
            aarch64_sysroot: "/unused/sysroot".into(),
            valgrind: "/bin/sh".into(),
            valgrind_lib: "/inputs/valgrind-support".into(),
            valgrind_runtime: "/unused/libc".into(),
            runtime: BTreeMap::new(),
            wrapper: vec![
                "/bin/sh".into(),
                "-c".into(),
                "printf '%s' \"$VALGRIND_LIB\"".into(),
            ],
            launch_timeout: LAUNCH_TIMEOUT,
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
}

//! P8: link the program's object with the runtime archive and a `main` that
//! hands the entry to `hirsel_rt_v2_launch`, then run it with explicit grants.
use super::emit::{check_runtime_calls, failure_rules};
use super::observe::{Observation, Sites, site_codes};
use super::stages::{NativeError, ReadyProgram, StageName};
use hirsel_lir::abi_types::{ParameterPassing, PhysicalType};
use hirsel_lir::layout_types::{SizeSpec, TargetSpec};
use hirsel_llvm::OptLevel;
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
    /// (`LLVM_SYS_211_PREFIX`), the installed cross tools, and the runtime
    /// archive of each listed target built with aborting panics into
    /// `build_dir` (as `crates/hirsel-rt/harness/run.sh` builds it).
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
        let prefix = std::env::var_os("LLVM_SYS_211_PREFIX")
            .map(PathBuf::from)
            .unwrap_or_else(|| "/usr/lib/llvm-21".into());
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
                        command.args(["--target", &target.triple]).env(
                            "CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER",
                            "aarch64-linux-gnu-gcc",
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
            runtime.insert((target.triple.clone(), runtime_build), archive);
        }
        Ok(Self {
            runtime_build,
            clang: prefix.join("bin/clang"),
            aarch64_cc: "aarch64-linux-gnu-gcc".into(),
            qemu_aarch64: on_path("qemu-aarch64-static"),
            aarch64_sysroot: "/usr/aarch64-linux-gnu".into(),
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

/// `program` as this process's PATH finds it. Launches clear the child's
/// environment, and a bare name would then be searched only in the libc
/// default path (/bin:/usr/bin), ignoring a user-local toolchain on PATH.
fn on_path(program: &str) -> PathBuf {
    on_path_in(program, std::env::var_os("PATH").as_deref())
}

fn on_path_in(program: &str, paths: Option<&std::ffi::OsStr>) -> PathBuf {
    paths
        .and_then(|paths| {
            std::env::split_paths(paths)
                .map(|dir| dir.join(program))
                .find(|path| path.is_file())
        })
        .unwrap_or_else(|| program.into())
}

fn aarch64(target: &TargetSpec) -> bool {
    target.triple.starts_with("aarch64")
}

/// The size of the entry's inline `Result`, after checking the launch
/// contract: `fn(rt: Io, args: Args) -> Result[(), E]` at the Hirsel ABI v1.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum LaunchKind {
    Zero,
    Main,
    Test,
}
pub(crate) fn launch_kind(signature: &hirsel_types::Signature) -> Option<LaunchKind> {
    use hirsel_types::{GenericArgument, SignatureKind, TypeKind};
    let SignatureKind::Function(f) = &signature.kind else {
        return None;
    };
    if signature.metadata.visibility != hirsel_syntax::Visibility::Public
        || !f.generics.is_empty()
        || f.incomplete
        || f.foreign.is_some()
    {
        return None;
    }
    let TypeKind::Named {
        definition,
        arguments,
    } = &signature.types.get(f.result)?.kind
    else {
        return None;
    };
    if definition.0.package.name != "core"
        || definition.0.package.version != "1"
        || !definition.0.modules.is_empty()
        || definition.0.name != "Result"
    {
        return None;
    }
    let [GenericArgument::Type(ok), GenericArgument::Type(_)] = arguments.as_ref() else {
        return None;
    };
    if !matches!(&signature.types.get(*ok)?.kind, TypeKind::Tuple(fields) if fields.is_empty()) {
        return None;
    }
    let names: Option<Vec<_>> = f
        .parameters
        .iter()
        .map(|p| match &signature.types.get(p.ty)?.kind {
            TypeKind::Named {
                definition,
                arguments,
            } if arguments.is_empty()
                && definition.0.package.name == "core"
                && definition.0.package.version == "1"
                && definition.0.modules.is_empty() =>
            {
                Some(definition.0.name.as_str())
            }
            _ => None,
        })
        .collect();
    match names?.as_slice() {
        [] => Some(LaunchKind::Zero),
        ["Io", "Args"] => Some(LaunchKind::Main),
        ["Test"] => Some(LaunchKind::Test),
        _ => None,
    }
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
    let parameter_count = match kind {
        LaunchKind::Zero => 0,
        LaunchKind::Main => 2,
        LaunchKind::Test => 1,
    };
    if abi.parameters.len() != parameter_count || !abi.parameters.iter().all(handle) {
        return Err(refuse("parameters other than (rt: Io, args: Args)"));
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
    Ok((hirsel_llvm::symbol_name(entry, Some(instance)), size))
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
            opt != OptLevel::None
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
    check_runtime_calls(program)?;
    let registry = hirsel_lir::descriptor_registry::DescriptorRegistry::new(
        &program.table,
        &program.finalizers,
    );
    partition_objects(program, opt, partition, &registry, measurement, sources)
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
            hirsel_llvm::symbol_name(&u.unit().header.definition, Some(&u.unit().header.instance))
        })
        .collect();
    let target = &program.target;
    let kind = program.launches[entry];
    let (declaration, launcher, call) = match kind {
        LaunchKind::Zero => (
            format!("declare i8 @\"{symbol}\"(ptr, ptr, ptr)"),
            "declare i32 @hirsel_rt_v2_launch_zero(ptr, i64, ptr, i32, ptr)".to_owned(),
            format!(
                "call i32 @hirsel_rt_v2_launch_zero(ptr @\"{symbol}\", i64 {result_size}, ptr @\"{}\", i32 {}, ptr @hirsel.launch.config)",
                registry.symbol(),
                registry.external_count()
            ),
        ),
        LaunchKind::Main => (
            format!("declare i8 @\"{symbol}\"(ptr, ptr, ptr, ptr, ptr)"),
            "declare i32 @hirsel_rt_v2_launch(ptr, i64, ptr, i32, ptr)".to_owned(),
            format!(
                "call i32 @hirsel_rt_v2_launch(ptr @\"{symbol}\", i64 {result_size}, ptr @\"{}\", i32 {}, ptr @hirsel.launch.config)",
                registry.symbol(),
                registry.external_count()
            ),
        ),
        LaunchKind::Test => (
            format!("declare i8 @\"{symbol}\"(ptr, ptr, ptr, ptr)"),
            "declare i32 @hirsel_rt_v2_launch_test(ptr, i64, ptr, i32, ptr)".to_owned(),
            format!(
                "call i32 @hirsel_rt_v2_launch_test(ptr @\"{symbol}\", i64 {result_size}, ptr @\"{}\", i32 {}, ptr @hirsel.launch.config)",
                registry.symbol(),
                registry.external_count()
            ),
        ),
    };
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
    let launch = if kind == LaunchKind::Test {
        hirsel_run_config::LaunchKind::Test
    } else {
        hirsel_run_config::LaunchKind::Main
    };
    let mut record = hirsel_run_config::RunConfig::for_program(revision, launch);
    record.inputs = program.launch.clone();
    // The existing conformance launcher supplies this explicit fixture profile.
    // Standalone admission will consume the record at the C entry boundary.
    if kind == LaunchKind::Main {
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
    let run = record
        .validated()
        .map_err(|error| NativeError::new(StageName::Launch, error.to_string()))?;
    std::fs::write(dir.join("run.json"), run.canonical())
        .map_err(|error| NativeError::new(StageName::Link, error.to_string()))?;
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
    let shim = format!(
        "target triple = \"{}\"\n{registry_declaration}\n{launch_config}\n{declaration}\n{launcher}\ndeclare ptr @signal(i32, ptr)\ndefine i32 @main() {{\n %old_sigpipe = call ptr @signal(i32 13, ptr inttoptr (i64 1 to ptr))\n %r = {call}\n ret i32 %r\n}}\n",
        target.triple
    );
    let main = hirsel_llvm::compile_ir(&shim, target, OptLevel::None)
        .map_err(|e| NativeError::new(StageName::Link, e.to_string()))?;
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
        let declared = matches!(import.kind, hirsel_lir::stage::ObjectSymbolKind::Runtime)
            || exported.contains(import.name.as_str())
            || hirsel_llvm::TOOLCHAIN_IMPORTS.contains(&import.name.as_str());
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
    command.arg(write("main.o", &main.bytes)?);
    for (index, object) in objects.iter().enumerate() {
        command.arg(write(&format!("unit-{index}.o"), &object.bytes)?);
    }
    command
        .arg(archive)
        .args([
            "-lgcc_s",
            "-lutil",
            "-lrt",
            "-lpthread",
            "-lm",
            "-ldl",
            "-o",
        ])
        .arg(&executable);
    checked(StageName::Link, &mut command)?;
    Ok(executable)
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
    launch_with(executable, target, toolchain, dir, diagnostic, Tape::None)
}

/// What a launch does with the Io completion tape.
#[derive(Clone, Copy, Debug)]
pub enum Tape<'a> {
    None,
    /// Real Io, its completions recorded to this file.
    Record(&'a Path),
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
    if toolchain.runtime_build.measurement {
        command.env("HIRSEL_RT_MEASUREMENT", dir.join("measurement"));
    }
    if diagnostic {
        command.env("HIRSEL_RT_DIAGNOSTIC", "1");
    }
    match tape {
        Tape::None => {}
        Tape::Record(path) => {
            command.env("HIRSEL_RT_TAPE", path);
        }
        Tape::Replay(path) => {
            command.env("HIRSEL_RT_REPLAY", path);
        }
    }
    let output = bounded(&mut command, toolchain.launch_timeout)?;
    Ok(Launched {
        audit_required: toolchain.runtime_build.diagnostic || toolchain.runtime_build.measurement,
        exit: output.status.code(),
        stdout: output.stdout,
        stderr: output.stderr,
        record: std::fs::read_to_string(&record).unwrap_or_default(),
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
                    .parse::<usize>()
                    .ok()
                    .and_then(|r| rules.get(r.checked_sub(1)?))
                    .ok_or_else(|| bad(format!("an unknown failure rule code in `{line}`")))?;
                let (definition, range) = u64::from_str_radix(site, 16)
                    .ok()
                    .and_then(|code| codes.get(&code))
                    .ok_or_else(|| bad(format!("an unknown failure site code in `{line}`")))?;
                failures.push(sites.failure(rule, definition, *range)?);
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
    pub fn under_memcheck(mut self) -> Self {
        self.launch_timeout = LAUNCH_TIMEOUT * 10;
        let valgrind = on_path("valgrind");
        self.wrapper = [
            valgrind.to_string_lossy().as_ref(),
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
    use std::os::unix::fs::PermissionsExt;

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
    fn path_resolution_selects_the_first_tool_before_clearing_the_environment() {
        let root = std::env::temp_dir().join(format!("hirsel-link-path-{}", std::process::id()));
        let first = root.join("first");
        let second = root.join("second");
        for dir in [&first, &second] {
            std::fs::create_dir_all(dir).unwrap();
            let tool = dir.join("tool");
            std::fs::write(&tool, "#!/bin/sh\nprintf '%s' chosen\n").unwrap();
            std::fs::set_permissions(&tool, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let paths = std::env::join_paths([&first, &second]).unwrap();
        let selected = on_path_in("tool", Some(&paths));
        assert_eq!(selected, first.join("tool"));
        let output = Command::new(selected).env_clear().output().unwrap();
        assert!(output.status.success());
        assert_eq!(output.stdout, b"chosen");
        assert_eq!(
            on_path_in("missing", Some(&paths)),
            PathBuf::from("missing")
        );
        assert_eq!(on_path_in("tool", None), PathBuf::from("tool"));
        std::fs::remove_dir_all(root).unwrap();
    }
}

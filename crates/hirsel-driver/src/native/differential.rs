//! The observation matrix: one program's reference observation
//! against the heap executor and native execution under every legal compiler
//! choice. A configuration names its target, placement, reuse, engine,
//! optimization level and build topology, so a report row is reproducible
//! from its name alone.
use super::emit::Backend;
use super::link::{
    Partition, RuntimeBuild, Toolchain, emit_backend_objects, link_fixture_runnable, observe,
};
use super::observe::{Observation, Sites};
use super::stages::{NativeError, Options, ReadyProgram, StageName, build};
use hirsel_engine::ValidatedNativeInput;
use hirsel_lir::emission::OptLevel;
use hirsel_lir::layout_types::TargetSpec;
use hirsel_syntax::DefPath;
use std::path::Path;

/// Bump when a configuration is added, removed or renamed: stored reports
/// and mismatch artifacts name the matrix they ran.
pub const MATRIX_VERSION: u32 = 4;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Engine {
    /// The checked heap executor; with `campaign`, also one audited run per
    /// failure-capable site with that site failed.
    Heap { campaign: bool },
    /// A linked executable on one runtime archive.
    Native {
        opt: OptLevel,
        partition: Partition,
        runtime: Runtime,
    },
    /// A linked executable whose objects and launcher Cranelift emitted (fast debug
    /// builds), held to the LLVM configurations' observations by the parity gate.
    Cranelift {
        partition: Partition,
        runtime: Runtime,
    },
}

/// The runtime archive a native configuration links, and how it launches.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Runtime {
    /// The diagnostic archive with poisoning, header checks and the exit
    /// leak audit on. Its compiled fast-path words stay zero, so every
    /// allocation, free and dup reaches a checked runtime entry.
    Diagnostic,
    /// The archive `hirsel run --native` ships: the inlined fast paths run.
    Default,
    /// The system-allocator archive: each fixed-size block is one libc
    /// allocation, so memcheck sees its use after free.
    System,
    /// The shipped size-class allocator built with Valgrind client requests:
    /// memcheck sees slab blocks, region reservations and arena blocks, and
    /// every allocation, free and arena bump reaches the runtime.
    Memcheck,
}

impl Runtime {
    pub fn name(self) -> &'static str {
        match self {
            Self::Diagnostic => "diagnostic",
            Self::Default => "default",
            Self::System => "system",
            Self::Memcheck => "memcheck",
        }
    }

    pub fn build(self) -> RuntimeBuild {
        RuntimeBuild {
            diagnostic: self == Self::Diagnostic,
            system_allocator: self == Self::System,
            measurement: false,
            memcheck: self == Self::Memcheck,
            ownership_trace: false,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Configuration {
    pub options: Options,
    pub engine: Engine,
}

impl Configuration {
    pub fn name(&self) -> String {
        let o = &self.options;
        let arch = o.target.triple.split('-').next().unwrap_or("?");
        let passes = format!(
            "{arch}/{}/{}/{}/{}",
            if o.regions { "regions" } else { "counted" },
            if o.elide { "elide" } else { "insert-only" },
            if o.reuse { "reuse" } else { "no-reuse" },
            if o.flat_walk {
                "flat-walk"
            } else {
                "no-flat-walk"
            },
        );
        let name = match self.engine {
            Engine::Heap { campaign } => {
                format!("{passes}/heap{}", if campaign { "+inject" } else { "" })
            }
            Engine::Native {
                opt,
                partition,
                runtime,
            } => format!(
                "{passes}/llvm-{}/{}/{}",
                match opt {
                    OptLevel::None => "O0",
                    OptLevel::Default => "O2",
                    OptLevel::Speed => "O3",
                },
                partition.name(),
                runtime.name(),
            ),
            Engine::Cranelift { partition, runtime } => {
                format!("{passes}/cranelift/{}/{}", partition.name(), runtime.name())
            }
        };
        let mut name = name;
        for (field, seed) in [
            ("hash", o.launch.hash_seed),
            ("random", o.launch.random_seed),
            ("test", o.launch.test_seed),
        ] {
            if seed != 0 {
                name.push_str(&format!("/{field}-{seed}"));
            }
        }
        name
    }
}

fn options(target: &TargetSpec, regions: bool, elide: bool, reuse: bool) -> Options {
    Options {
        validation: super::Validation::EveryStage,
        launch: Default::default(),
        target: target.clone(),
        regions,
        elide,
        reuse,
        flat_walk: true,
        scoped_specialization: true,
        discharge: Default::default(),
        objects: None,
    }
}

/// The per-push smoke matrix: the default pipeline on the heap executor with
/// fault injection, and one optimized x86-64 executable on the diagnostic
/// archive and one on the shipped archive.
pub fn smoke_matrix() -> Vec<Configuration> {
    let x86 = TargetSpec::x86_64_unknown_linux_gnu();
    let mut out = vec![Configuration {
        options: Options::new(x86.clone()),
        engine: Engine::Heap { campaign: true },
    }];
    for runtime in [Runtime::Diagnostic, Runtime::Default] {
        out.push(Configuration {
            options: Options::new(x86.clone()),
            engine: Engine::Native {
                opt: OptLevel::Speed,
                partition: Partition::Whole,
                runtime,
            },
        });
    }
    out
}

/// The full matrix: both executed targets; regions on and off; insertion
/// only, elision, and reuse; the heap executor on every pass configuration;
/// native code on the diagnostic archive at O0 and O3 as one object and one
/// object per unit; and, on the default passes at O3 as one object and one
/// object per unit, the shipped and system-allocator archives.
pub fn full_matrix() -> Vec<Configuration> {
    let mut out = Vec::new();
    for target in [
        TargetSpec::x86_64_unknown_linux_gnu(),
        TargetSpec::aarch64_unknown_linux_gnu(),
    ] {
        for regions in [true, false] {
            for (elide, reuse) in [(false, false), (true, false), (true, true)] {
                let options = options(&target, regions, elide, reuse);
                let default = regions && elide && reuse;
                out.push(Configuration {
                    options: options.clone(),
                    engine: Engine::Heap { campaign: default },
                });
                for opt in [OptLevel::None, OptLevel::Speed] {
                    for partition in [Partition::Whole, Partition::Singleton] {
                        // The off-default pass configurations run as one
                        // object; topology is varied on the default.
                        if partition == Partition::Singleton && !default {
                            continue;
                        }
                        out.push(Configuration {
                            options: options.clone(),
                            engine: Engine::Native {
                                opt,
                                partition,
                                runtime: Runtime::Diagnostic,
                            },
                        });
                    }
                }
                if default {
                    for runtime in [Runtime::Default, Runtime::System] {
                        for partition in [Partition::Whole, Partition::Singleton] {
                            out.push(Configuration {
                                options: options.clone(),
                                engine: Engine::Native {
                                    opt: OptLevel::Speed,
                                    partition,
                                    runtime,
                                },
                            });
                        }
                    }
                }
            }
        }
    }
    for target in [
        TargetSpec::x86_64_unknown_linux_gnu(),
        TargetSpec::aarch64_unknown_linux_gnu(),
    ] {
        let mut options = Options::new(target);
        options.flat_walk = false;
        out.push(Configuration {
            options,
            engine: Engine::Native {
                opt: OptLevel::Speed,
                partition: Partition::Whole,
                runtime: Runtime::Default,
            },
        });
    }
    out
}

/// The `--trace-roots` matrix (S-14): the default passes as one unoptimized x86-64 LLVM
/// executable and one Cranelift executable, both on the diagnostic archive, whose traces
/// must equal the interpreter's. Root releases are runtime calls placed by the passes, which
/// no LLVM optimization reorders, so the parity set's level checks them at its cost.
pub fn trace_roots_matrix() -> Vec<Configuration> {
    let x86 = TargetSpec::x86_64_unknown_linux_gnu();
    vec![
        Configuration {
            options: Options::new(x86.clone()),
            engine: Engine::Native {
                opt: OptLevel::None,
                partition: Partition::Whole,
                runtime: Runtime::Diagnostic,
            },
        },
        Configuration {
            options: Options::new(x86),
            engine: Engine::Cranelift {
                partition: Partition::Whole,
                runtime: Runtime::Diagnostic,
            },
        },
    ]
}

/// The `--migrate` matrix (design §9 W1): one optimized x86-64 LLVM executable and one
/// Cranelift executable, both on the optimized default archive, run with every suspension
/// migrating its task to another compute worker; their observations must equal the
/// interpreter's.
pub fn migrate_matrix() -> Vec<Configuration> {
    let x86 = TargetSpec::x86_64_unknown_linux_gnu();
    vec![
        Configuration {
            options: Options::new(x86.clone()),
            engine: Engine::Native {
                opt: OptLevel::Speed,
                partition: Partition::Whole,
                runtime: Runtime::Default,
            },
        },
        Configuration {
            options: Options::new(x86),
            engine: Engine::Cranelift {
                partition: Partition::Whole,
                runtime: Runtime::Default,
            },
        },
    ]
}

/// Every archive variant `matrix`'s native configurations link, the
/// diagnostic archive first (and alone when there are none).
pub fn runtime_builds(matrix: &[Configuration]) -> Vec<RuntimeBuild> {
    let mut builds = vec![Runtime::Diagnostic.build()];
    for configuration in matrix {
        if let Engine::Native { runtime, .. } | Engine::Cranelift { runtime, .. } =
            configuration.engine
            && !builds.contains(&runtime.build())
        {
            builds.push(runtime.build());
        }
    }
    builds
}

impl Toolchain {
    /// The in-tree toolchain for `matrix`: its targets, and every archive its
    /// native configurations link, with the diagnostic archive selected.
    pub fn for_matrix(build_dir: &Path, matrix: &[Configuration]) -> Result<Self, NativeError> {
        let mut targets: Vec<&TargetSpec> = Vec::new();
        for c in matrix {
            if !targets.contains(&&c.options.target) {
                targets.push(&c.options.target);
            }
        }
        Self::in_tree_with_runtimes(build_dir, &targets, &runtime_builds(matrix))
    }
}

/// The topology matrix (PROG-5): one object, one object per unit and
/// `seeds` seeded random partitions of the default pipeline on x86-64 at O3.
pub fn topology_matrix(seeds: u64) -> Vec<Configuration> {
    let options = Options::new(TargetSpec::x86_64_unknown_linux_gnu());
    [Partition::Whole, Partition::Singleton]
        .into_iter()
        .chain((1..=seeds).map(Partition::Random))
        .map(|partition| Configuration {
            options: options.clone(),
            engine: Engine::Native {
                opt: OptLevel::Speed,
                partition,
                runtime: Runtime::Diagnostic,
            },
        })
        .collect()
}

/// What one configuration did with one program.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Outcome {
    /// The engine's observation equals the reference's.
    Agree,
    /// The engine observed this instead.
    Mismatch(Observation),
    /// A tool outcome: no observation was produced.
    Unavailable(NativeError),
}

fn same_options(a: &Options, b: &Options) -> bool {
    a.launch == b.launch
        && a.target == b.target
        && a.regions == b.regions
        && a.elide == b.elide
        && a.reuse == b.reuse
        && a.flat_walk == b.flat_walk
        && a.scoped_specialization == b.scoped_specialization
}

/// Deterministic providers selected explicitly by a semantic fixture, never a host grant.
#[derive(Clone, Debug)]
pub struct Fixture {
    pub args: Vec<String>,
    /// Plain bytes followed by EOF, shared with SimulationConfig. No readiness script.
    pub stdin: Vec<u8>,
    pub files: std::collections::BTreeMap<String, Vec<u8>>,
    pub mounts: std::collections::BTreeMap<String, hirsel_run_schema::Access>,
    /// Simulation-only regular-file readiness, shared with SimulationConfig.
    pub file_reads: std::collections::BTreeMap<String, hirsel_run_schema::FileReadScript>,
    /// Ordered snapshot-only file inputs; byte counts are checked before consumption.
    pub file_faults: hirsel_run_schema::FileFaults,
    /// The negative output-grant fixture, shared by every engine.
    pub deny_output: bool,
    pub monotonic: Option<u64>,
    pub net: Option<hirsel_run_schema::NetGrant>,
    pub process: Option<hirsel_run_schema::ProcessGrant>,
    /// The `io.signals` grant, with its script under `signals.script@1`.
    pub signals: Option<hirsel_run_schema::SignalsGrant>,
    /// The root Env's snapshot: each granted name and its value, null when unset.
    pub env: Option<std::collections::BTreeMap<String, Option<String>>>,
    /// A corpus program's raised interpreter step budget (`// steps:`); H's
    /// instruction limit rises by the same factor. None keeps both defaults.
    pub steps: Option<u64>,
    /// The run's runners (`// runners:`) and the seed `sim@1` draws faults from
    /// (`// fault_seed:`).
    pub runners: Vec<hirsel_run_schema::RunnerEntry>,
    pub fault_seed: Option<u64>,
    /// What the launcher found about each callee entry of a run with runners, by full path
    /// (`hirsel_engine::callee_facts`).
    pub callees: std::collections::BTreeMap<String, hirsel_run_schema::CalleeFacts>,
    /// `--trace-roots` (S-14): native launches on the diagnostic archive write every live
    /// `Shared` root's count at every scheduling point into `Observation::roots`, to be
    /// compared with the interpreter's trace.
    pub trace_roots: bool,
    /// `--migrate` (design §9 W1): native launches run on several compute workers and move
    /// every task to another worker at every suspension (`HIRSEL_RT_MIGRATE`), and native
    /// resumable task bodies save their compact frame at every boundary.
    pub migrate: bool,
}

impl Default for Fixture {
    fn default() -> Self {
        Self {
            args: vec![],
            stdin: vec![],
            files: Default::default(),
            mounts: hirsel_engine::SimulationConfig::default().mounts,
            file_reads: Default::default(),
            file_faults: Default::default(),
            deny_output: false,
            monotonic: None,
            net: None,
            process: None,
            signals: None,
            env: None,
            steps: None,
            runners: vec![],
            fault_seed: None,
            callees: Default::default(),
            trace_roots: false,
            migrate: false,
        }
    }
}

impl From<&Fixture> for hirsel_engine::SimulationConfig {
    fn from(fixture: &Fixture) -> Self {
        let mut config = Self {
            args: fixture.args.clone(),
            stdin: fixture.stdin.clone(),
            files: fixture.files.clone(),
            mounts: fixture.mounts.clone(),
            file_reads: fixture.file_reads.clone(),
            file_faults: fixture.file_faults.clone(),
            net: fixture.net.clone(),
            process: fixture.process.clone(),
            signals: fixture.signals.clone(),
            env: fixture.env.clone().unwrap_or_default(),
            monotonic: fixture.monotonic.unwrap_or(0),
            runners: fixture.runners.clone(),
            fault_seed: fixture.fault_seed.unwrap_or(0),
            callees: std::sync::Arc::new(fixture.callees.clone()),
            trace_roots: fixture.trace_roots,
            ..Default::default()
        };
        if fixture.deny_output {
            config.projections.remove("stdout");
            config.projections.remove("stderr");
        }
        if fixture.monotonic.is_some() {
            config.grants.insert("MonotonicClock".into());
            config.projections.insert("monotonic_clock".into());
        }
        config
    }
}

/// Run `entry` under each configuration and compare with `reference`.
/// Configurations that share pass options share one Ready program, and native
/// configurations that differ only by runtime archive share its objects.
pub fn differential(
    validated: &ValidatedNativeInput,
    entry: &DefPath,
    reference: &Observation,
    configurations: &[Configuration],
    toolchain: &Toolchain,
    dir: &Path,
) -> Vec<(String, Outcome)> {
    differential_in(
        validated,
        entry,
        reference,
        configurations,
        toolchain,
        dir,
        &Fixture::default(),
    )
}

/// Run the matrix in an explicitly selected deterministic fixture world.
pub fn differential_in(
    validated: &ValidatedNativeInput,
    entry: &DefPath,
    reference: &Observation,
    configurations: &[Configuration],
    toolchain: &Toolchain,
    dir: &Path,
    fixture: &Fixture,
) -> Vec<(String, Outcome)> {
    let world = super::HeapWorld {
        stdin: fixture.stdin.clone(),
        args: fixture.args.clone(),
        files: fixture.files.clone(),
        mounts: fixture.mounts.clone(),
        file_reads: fixture.file_reads.clone(),
        file_faults: fixture.file_faults.clone(),
        deny_output: fixture.deny_output,
        monotonic: fixture.monotonic,
        process: fixture.process.clone(),
        net: fixture.net.clone(),
        signals: fixture.signals.clone(),
        env: fixture.env.clone(),
        steps: fixture.steps,
    };
    let sites = Sites::new(validated.snapshot());
    let mut built: Vec<(&Options, Result<ReadyProgram, NativeError>)> = Vec::new();
    type Objects = Result<Vec<hirsel_lir::stage::ObjectArtifact>, NativeError>;
    let mut emitted: Vec<((usize, Backend, OptLevel, Partition), Objects)> = Vec::new();
    let mut out = Vec::with_capacity(configurations.len());
    // Configurations that differ only by runtime archive share their objects.
    let mut native = |backend: Backend,
                      opt: OptLevel,
                      partition: Partition,
                      runtime: Runtime,
                      index: usize,
                      program: &ReadyProgram| {
        let toolchain = &toolchain.with_runtime(runtime.build());
        let key = (index, backend, opt, partition);
        let at = match emitted.iter().position(|(k, _)| *k == key) {
            Some(at) => at,
            None => {
                let measurement = toolchain.runtime_build.measurement;
                let objects =
                    emit_backend_objects(program, backend, opt, partition, measurement, None);
                emitted.push((key, objects));
                emitted.len() - 1
            }
        };
        let objects = emitted[at].1.as_ref().map_err(Clone::clone)?;
        let runnable = link_fixture_runnable(program, entry, objects, toolchain, dir, fixture)?;
        let target = &program.target;
        let diagnostic = runtime == Runtime::Diagnostic;
        let launched = super::link::launch_fixture(
            &runnable,
            target,
            toolchain,
            dir,
            diagnostic,
            super::link::Tape::Live {
                record: None,
                revocations: None,
            },
            fixture,
        )?;
        let mut observation = observe(&launched, program, &sites)?;
        if fixture.trace_roots {
            observation.roots = std::fs::read_to_string(dir.join(super::link::ROOT_TRACE))
                .unwrap_or_default()
                .lines()
                .map(str::to_string)
                .collect();
        }
        Ok(observation)
    };
    for configuration in configurations {
        let index = match built
            .iter()
            .position(|(o, _)| same_options(o, &configuration.options))
        {
            Some(index) => index,
            None => {
                built.push((
                    &configuration.options,
                    build(validated, &configuration.options),
                ));
                built.len() - 1
            }
        };
        let observed = built[index]
            .1
            .as_ref()
            .map_err(Clone::clone)
            .and_then(|program| match configuration.engine {
                Engine::Heap { campaign: true } => {
                    super::heap_campaign_in(program, entry, &sites, &world)
                        .map(|run| run.observation)
                }
                Engine::Heap { campaign: false } => {
                    super::run_heap_in(program, entry, &sites, &world).map(|run| run.observation)
                }
                Engine::Native {
                    opt,
                    partition,
                    runtime,
                } => native(Backend::Llvm, opt, partition, runtime, index, program),
                Engine::Cranelift { partition, runtime } => native(
                    Backend::Cranelift,
                    OptLevel::None,
                    partition,
                    runtime,
                    index,
                    program,
                ),
            });
        let outcome = match observed {
            Ok(observation) if observation == *reference => Outcome::Agree,
            Ok(observation) => Outcome::Mismatch(observation),
            Err(error) => Outcome::Unavailable(error),
        };
        out.push((configuration.name(), outcome));
    }
    out
}

/// The runner oracle (link design §2): under each native configuration of `configurations`,
/// the program's linked executable and its image in the runner, each launched once in the
/// fixture world, give byte-identical stdout, stderr, exit status and observation records.
/// With `tapes`, each path then records its Io completion tape and the other path replays it,
/// and both replays observe what the recordings did. A configuration whose program runs as an
/// executable anyway (a fallback, [`super::counts`]) compares the executable with itself.
pub fn runner_oracle_in(
    validated: &ValidatedNativeInput,
    entry: &DefPath,
    configurations: &[Configuration],
    toolchain: &Toolchain,
    dir: &Path,
    fixture: &Fixture,
    tapes: bool,
) -> Vec<(String, Outcome)> {
    let sites = Sites::new(validated.snapshot());
    let mut out = Vec::new();
    for configuration in configurations {
        let (backend, opt, partition, runtime) = match configuration.engine {
            Engine::Heap { .. } => continue,
            Engine::Native {
                opt,
                partition,
                runtime,
            } => (Backend::Llvm, opt, partition, runtime),
            Engine::Cranelift { partition, runtime } => {
                (Backend::Cranelift, OptLevel::None, partition, runtime)
            }
        };
        let toolchain = toolchain.with_runtime(runtime.build());
        let diagnostic = runtime == Runtime::Diagnostic;
        let run = || -> Result<(), NativeError> {
            let program = build(validated, &configuration.options)?;
            let target = &program.target;
            let objects = emit_backend_objects(
                &program,
                backend,
                opt,
                partition,
                toolchain.runtime_build.measurement,
                None,
            )?;
            let executable = Toolchain {
                execution: super::runner::Execution::Executable,
                ..toolchain.clone()
            };
            let paths = [
                link_fixture_runnable(&program, entry, &objects, &executable, dir, fixture)?,
                link_fixture_runnable(&program, entry, &objects, &toolchain, dir, fixture)?,
            ];
            let launch = |runnable: &super::link::Runnable, tape: super::link::Tape<'_>| {
                super::link::launch_fixture(
                    runnable, target, &toolchain, dir, diagnostic, tape, fixture,
                )
            };
            let live = super::link::Tape::Live {
                record: None,
                revocations: None,
            };
            let [linked, loaded] = [launch(&paths[0], live)?, launch(&paths[1], live)?];
            for (field, a, b) in [
                (
                    "exit status",
                    format!("{:?}", linked.exit),
                    format!("{:?}", loaded.exit),
                ),
                (
                    "stdout",
                    String::from_utf8_lossy(&linked.stdout).into_owned(),
                    String::from_utf8_lossy(&loaded.stdout).into_owned(),
                ),
                (
                    "stderr",
                    String::from_utf8_lossy(&linked.stderr).into_owned(),
                    String::from_utf8_lossy(&loaded.stderr).into_owned(),
                ),
                (
                    "observation record",
                    linked.record.clone(),
                    loaded.record.clone(),
                ),
            ] {
                if a != b {
                    return Err(NativeError::new(
                        StageName::Launch,
                        format!(
                            "runner oracle: the {field} differs: executable {a:?}, runner {b:?}"
                        ),
                    ));
                }
            }
            if !tapes {
                return Ok(());
            }
            let mut replays = Vec::new();
            for (recorder, replayer) in [(0, 1), (1, 0)] {
                let tape = dir.join(format!("oracle-tape-{recorder}"));
                let _ = std::fs::remove_file(&tape);
                let recorded = launch(
                    &paths[recorder],
                    super::link::Tape::Live {
                        record: Some(&tape),
                        revocations: None,
                    },
                )?;
                let mut recorded = observe(&recorded, &program, &sites)?;
                // A run with runners replays without its callees, printing only its own.
                if !fixture.runners.is_empty() {
                    recorded.output = recorded_prints(&tape_events(&tape)?);
                }
                let replayed = launch(&paths[replayer], super::link::Tape::Replay(&tape))?;
                let replayed = observe(&replayed, &program, &sites)?;
                if replayed != recorded {
                    return Err(NativeError::new(
                        StageName::Launch,
                        format!(
                            "runner oracle: a tape recorded by the {} replays differently",
                            ["executable", "runner"][recorder]
                        ),
                    ));
                }
                replays.push(replayed);
            }
            if replays[0] != replays[1] {
                return Err(NativeError::new(
                    StageName::Launch,
                    "runner oracle: the cross replays observe differently",
                ));
            }
            Ok(())
        };
        let outcome = match run() {
            Ok(()) => Outcome::Agree,
            Err(error) => Outcome::Unavailable(error),
        };
        out.push((format!("{}/runner-oracle", configuration.name()), outcome));
    }
    out
}

/// The events of the Io tape at `tape`.
fn tape_events(tape: &Path) -> Result<Vec<hirsel_rt::io::Event>, NativeError> {
    let text = hirsel_rt::launch::read_recording_text(tape)
        .map_err(|e| NativeError::new(StageName::Launch, format!("the Io tape: {e}")))?;
    hirsel_rt::launch::parse_recording(&text)
        .map(|record| record.events)
        .map_err(|e| NativeError::new(StageName::Launch, format!("the Io tape: {e}")))
}

/// What a tape's run printed, in order.
fn recorded_prints(events: &[hirsel_rt::io::Event]) -> Vec<u8> {
    let mut printed = Vec::new();
    for event in events {
        // Test log and snapshot records reach stdout through Test reporting, not Print;
        // their bytes are re-rendered in order. An invalid snapshot name records nothing.
        match &event.request {
            hirsel_rt::io::Request::Print { bytes, .. } => printed.extend_from_slice(bytes),
            hirsel_rt::io::Request::TestLog { text, .. } => {
                printed.extend(hirsel_rt::kernels::report::log(text))
            }
            hirsel_rt::io::Request::TestSnapshot { name, text, .. }
                if event.completion == hirsel_rt::io::Completion::Checked(true) =>
            {
                printed.extend(hirsel_rt::kernels::report::snapshot(name, text))
            }
            _ => {}
        }
    }
    printed
}

/// Real Io replay: run the program as an optimized x86-64
/// executable on real Io with its completion tape recorded, then run the same
/// executable on simulated Io consuming exactly that tape. Both runs must
/// observe what the reference does, and the tape's prints, in order, must be
/// the reference's output. A run with runners replays without its callees (RUN-11): its
/// replay observes the recorded run with only the tape's own prints. The first
/// disagreement is the outcome.
pub fn real_io_replay(
    validated: &ValidatedNativeInput,
    entry: &DefPath,
    reference: &Observation,
    toolchain: &Toolchain,
    dir: &Path,
) -> Outcome {
    real_io_replay_with_config(
        validated,
        entry,
        reference,
        toolchain,
        dir,
        Default::default(),
    )
}

pub fn real_io_replay_with_config(
    validated: &ValidatedNativeInput,
    entry: &DefPath,
    reference: &Observation,
    toolchain: &Toolchain,
    dir: &Path,
    launch_config: super::LaunchInputs,
) -> Outcome {
    real_io_replay_in(
        validated,
        entry,
        reference,
        toolchain,
        dir,
        launch_config,
        &Fixture::default(),
    )
}

/// Record and replay with the fixture's virtual clock and scripted providers.
pub fn real_io_replay_in(
    validated: &ValidatedNativeInput,
    entry: &DefPath,
    reference: &Observation,
    toolchain: &Toolchain,
    dir: &Path,
    launch_config: super::LaunchInputs,
    fixture: &Fixture,
) -> Outcome {
    // Replay checks the tape against the diagnostic archive's audited run.
    let toolchain = &toolchain.with_runtime(Runtime::Diagnostic.build());
    let run = || {
        let target = TargetSpec::x86_64_unknown_linux_gnu();
        let mut options = Options::new(target.clone());
        options.launch = launch_config;
        let program = build(validated, &options)?;
        let sites = Sites::new(validated.snapshot());
        let objects = emit_backend_objects(
            &program,
            Backend::Llvm,
            OptLevel::Speed,
            Partition::Whole,
            toolchain.runtime_build.measurement,
            None,
        )?;
        let runnable = link_fixture_runnable(&program, entry, &objects, toolchain, dir, fixture)?;
        let tape = dir.join("io-tape");
        let _ = std::fs::remove_file(&tape);
        let recorded = super::link::launch_fixture(
            &runnable,
            &target,
            toolchain,
            dir,
            true,
            super::link::Tape::Live {
                record: Some(&tape),
                revocations: None,
            },
            fixture,
        )?;
        let real = observe(&recorded, &program, &sites)?;
        if real != *reference {
            return Ok(real);
        }
        let text = hirsel_rt::launch::read_recording_text(&tape)
            .map_err(|e| NativeError::new(StageName::Launch, format!("the Io tape: {e}")))?;
        // A run a signal's default action ended leaves a recording with no trailer, as a
        // killed process does (determinism.md, The Signaled end): nothing to replay.
        if reference.signaled.is_some() {
            return match hirsel_rt::launch::parse_recording_prefix(&text) {
                Ok((_, false)) => Ok(real),
                Ok((_, true)) => Err(NativeError::new(
                    StageName::Launch,
                    "a signaled run's recording is complete",
                )),
                Err(error) => Err(NativeError::new(
                    StageName::Launch,
                    format!("the Io tape: {error}"),
                )),
            };
        }
        let events = hirsel_rt::launch::parse_recording(&text)
            .map(|record| record.events)
            .map_err(|e| NativeError::new(StageName::Launch, format!("the Io tape: {e}")))?;
        let printed = recorded_prints(&events);
        // A Test report (a failed check, a scripted peer's or child's Expect) is no Print:
        // replay re-derives it, and the replayed observation must still be the reference.
        let reported = events
            .iter()
            .any(|e| e.completion == hirsel_rt::io::Completion::Checked(false));
        // A run with runners replays without its callees (RUN-11): its tape holds only its
        // own prints, which its replay alone reproduces; each callee's are on its own tape.
        let alone = !fixture.runners.is_empty();
        if !alone
            && printed != reference.output
            && !(reported && reference.output.starts_with(&printed))
        {
            return Err(NativeError::new(
                StageName::Launch,
                "the recorded prints are not the reference output",
            ));
        }
        let replayed = super::link::launch_fixture(
            &runnable,
            &target,
            toolchain,
            dir,
            true,
            super::link::Tape::Replay(&tape),
            fixture,
        )?;
        let replayed = observe(&replayed, &program, &sites)?;
        if alone {
            let expected = Observation {
                output: printed,
                ..real.clone()
            };
            return Ok(if replayed == expected { real } else { replayed });
        }
        Ok(replayed)
    };
    match run() {
        Ok(observation) if observation == *reference => Outcome::Agree,
        Ok(observation) => Outcome::Mismatch(observation),
        Err(error) => Outcome::Unavailable(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ready_program_cache_distinguishes_every_launch_input() {
        let baseline = Options::new(TargetSpec::x86_64_unknown_linux_gnu());
        assert!(same_options(&baseline, &baseline.clone()));
        for field in 0..4 {
            let mut changed = baseline.clone();
            match field {
                0 => changed.launch.hash_seed = 1,
                1 => changed.launch.random_seed = 1,
                2 => changed.launch.test_seed = 1,
                _ => {
                    changed.launch.scheduler = Some(hirsel_run_schema::Scheduler {
                        algorithm: "unsupported".into(),
                        version: 1,
                        seed: 1,
                    })
                }
            }
            assert!(!same_options(&baseline, &changed));
        }
    }
}

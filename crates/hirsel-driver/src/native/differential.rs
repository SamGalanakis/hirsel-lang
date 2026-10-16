//! The observation matrix: one program's reference observation
//! against the heap executor and native execution under every legal compiler
//! choice. A configuration names its target, placement, reuse, engine,
//! optimization level and build topology, so a report row is reproducible
//! from its name alone.
use super::heap::{heap_campaign, run_heap};
use super::link::{
    Partition, RuntimeBuild, Toolchain, emit_objects, launch, link, link_objects, observe,
};
use super::observe::{Observation, Sites};
use super::stages::{NativeError, Options, ReadyProgram, StageName, build};
use hirsel_engine::ValidatedNativeInput;
use hirsel_lir::layout_types::TargetSpec;
use hirsel_llvm::OptLevel;
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
}

impl Runtime {
    pub fn name(self) -> &'static str {
        match self {
            Self::Diagnostic => "diagnostic",
            Self::Default => "default",
            Self::System => "system",
        }
    }

    pub fn build(self) -> RuntimeBuild {
        RuntimeBuild {
            diagnostic: self == Self::Diagnostic,
            system_allocator: self == Self::System,
            measurement: false,
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
                    OptLevel::Speed => "O3",
                },
                partition.name(),
                runtime.name(),
            ),
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
        launch: Default::default(),
        target: target.clone(),
        regions,
        elide,
        reuse,
        flat_walk: true,
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

/// Every archive variant `matrix`'s native configurations link, the
/// diagnostic archive first (and alone when there are none).
pub fn runtime_builds(matrix: &[Configuration]) -> Vec<RuntimeBuild> {
    let mut builds = vec![Runtime::Diagnostic.build()];
    for configuration in matrix {
        if let Engine::Native { runtime, .. } = configuration.engine
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
    let sites = Sites::new(validated.snapshot());
    let mut built: Vec<(&Options, Result<ReadyProgram, NativeError>)> = Vec::new();
    type Objects = Result<Vec<hirsel_lir::stage::ObjectArtifact>, NativeError>;
    let mut emitted: Vec<((usize, OptLevel, Partition), Objects)> = Vec::new();
    let mut out = Vec::with_capacity(configurations.len());
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
                    heap_campaign(program, entry, &sites).map(|run| run.observation)
                }
                Engine::Heap { campaign: false } => {
                    run_heap(program, entry, &sites).map(|run| run.observation)
                }
                Engine::Native {
                    opt,
                    partition,
                    runtime,
                } => {
                    let toolchain = &toolchain.with_runtime(runtime.build());
                    let key = (index, opt, partition);
                    let at = match emitted.iter().position(|(k, _)| *k == key) {
                        Some(at) => at,
                        None => {
                            let measurement = toolchain.runtime_build.measurement;
                            let objects = emit_objects(program, opt, partition, measurement, None);
                            emitted.push((key, objects));
                            emitted.len() - 1
                        }
                    };
                    let objects = emitted[at].1.as_ref().map_err(Clone::clone)?;
                    let executable = link_objects(program, entry, objects, toolchain, dir)?;
                    let target = &program.target;
                    let diagnostic = runtime == Runtime::Diagnostic;
                    let launched = launch(&executable, target, toolchain, dir, diagnostic)?;
                    observe(&launched, program, &sites)
                }
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

/// Real Io replay: run the program as an optimized x86-64
/// executable on real Io with its completion tape recorded, then run the same
/// executable on simulated Io consuming exactly that tape. Both runs must
/// observe what the reference does, and the tape's prints, in order, must be
/// the reference's output. The first disagreement is the outcome.
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
    // Replay checks the tape against the diagnostic archive's audited run.
    let toolchain = &toolchain.with_runtime(Runtime::Diagnostic.build());
    let run = || {
        let target = TargetSpec::x86_64_unknown_linux_gnu();
        let mut options = Options::new(target.clone());
        options.launch = launch_config;
        let program = build(validated, &options)?;
        let sites = Sites::new(validated.snapshot());
        let executable = link(
            &program,
            entry,
            OptLevel::Speed,
            Partition::Whole,
            toolchain,
            dir,
        )?;
        let tape = dir.join("io-tape");
        let _ = std::fs::remove_file(&tape);
        let recorded = super::link::launch_with(
            &executable,
            &target,
            toolchain,
            dir,
            true,
            super::link::Tape::Record(&tape),
        )?;
        let real = observe(&recorded, &program, &sites)?;
        if real != *reference {
            return Ok(real);
        }
        let text = std::fs::read_to_string(&tape)
            .map_err(|e| NativeError::new(StageName::Launch, format!("the Io tape: {e}")))?;
        let events = hirsel_rt::launch::parse_recording(&text)
            .map(|record| record.events)
            .map_err(|e| NativeError::new(StageName::Launch, format!("the Io tape: {e}")))?;
        let mut printed = Vec::new();
        for event in &events {
            if let hirsel_rt::io::Request::Print { bytes, .. } = &event.request {
                printed.extend_from_slice(bytes);
            }
        }
        if printed != reference.output {
            return Err(NativeError::new(
                StageName::Launch,
                "the recorded prints are not the reference output",
            ));
        }
        let replayed = super::link::launch_with(
            &executable,
            &target,
            toolchain,
            dir,
            true,
            super::link::Tape::Replay(&tape),
        )?;
        observe(&replayed, &program, &sites)
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
                    changed.launch.scheduler = Some(hirsel_run_config::Scheduler {
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

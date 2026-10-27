//! Definition-path dispatch; unknown ordinary definitions leave context unchanged.
use super::{
    Simulation, SimulationConfig, collection, float, integer,
    simulation::{self, Fault, FileError, Grant},
    string,
};
use crate::{
    Error, Failure, Integer, OperationCall, OperationContext, OperationResult, Operations, Value,
    invalid,
};
use hirsel_diag::RuleId;
use hirsel_run_schema::Access;
use hirsel_tir::RunConfiguration;
use hirsel_types::{BuiltinType, GenericArgument, Primitive, TypeKind};

pub struct Library {
    pub simulation: Simulation,
    pub(super) races: super::race::RaceTracker,
    pub entry_signature: Option<hirsel_types::SignatureRef>,
    pub exit_report: Option<crate::exit_report::Report>,
    pub(super) tasks: super::tasks::Tasks,
    pub(super) channels: super::channels::Channels,
    pub(super) synchronizers: super::synchronization::Synchronizers,
    pub(super) network_waits: std::collections::BTreeMap<u64, super::net::NetworkWait>,
    pub(super) file_waits: std::collections::BTreeMap<u64, Vec<Value>>,
    /// Parked `watcher.next` calls: watcher, Dir and recorded input.
    pub(super) watch_waits: std::collections::BTreeMap<u64, (u64, u64, Vec<u8>)>,
    pub(super) process_advance: Option<(u64, u64)>,
    pub(super) timers: super::timers::Timers,
    pub(super) signals: super::signals::Signals,
    pub(super) callbacks: std::collections::BTreeMap<u64, super::callbacks::Pending>,
    pub(super) next_callback: u64,
    pub(super) cpu_certified: bool,
    pub(super) cpu_waits: std::collections::BTreeMap<u64, super::cpu::Batch>,
    pub(super) cpu_active: std::collections::BTreeSet<u64>,
    pub(super) next_shared: u64,
    /// A scripted client's unmet Expect at the last handle drop (LANG-9).
    pub(super) drop_failure: Option<(String, String)>,
    pub(super) runners: super::runners::Runners,
    /// Free mode (`tasks.free@1`): the run's session and pending states.
    pub(super) free: Option<super::free::Free>,
}
impl Library {
    /// Internal provider seam shared by evaluator cursor pulls and completion-tape
    /// differential adapters. This is not a language operation or a public paging API.
    pub fn read_directory_page(
        &mut self,
        directory: &Value,
        p: &str,
        after: &str,
        recursive: bool,
        context: &mut OperationContext<'_>,
    ) -> Result<Result<Vec<hirsel_kernels::walk::Entry>, Value>, Error> {
        let (tree, root, dir) = self.simulation.directory(directory)?;
        let entries = if let Some(fault) = self
            .tasks
            .canceled()
            .map(Fault::Canceled)
            .or_else(|| self.simulation.fault("file.read_dir"))
        {
            Err(fault_result(fault))
        } else {
            self.simulation
                .walk_entries(tree, dir, &root, p, after, recursive)
                .map_err(file_error)
        };
        let output = match &entries {
            Ok(entries) => ok(collection::sequence(
                BuiltinType::List,
                entries.iter().map(|e| Value::Record {
                    definition: (&hirsel_syntax::DefPath::core("DirEntry")).into(),
                    fields: vec![Some(path(&e.path)), Some(variant(e.kind, vec![]))],
                }),
            )),
            Err(error) => error.clone(),
        };
        self.simulation.record(
            "Dir",
            "file.read_dir",
            encode_arguments(&[
                directory.clone(),
                path(p),
                Value::String(after.into()),
                Value::Bool(recursive),
            ]),
            encode_value(&output),
            context,
        )?;
        Ok(entries)
    }
    pub fn new(run: &RunConfiguration, config: SimulationConfig) -> Result<Self, Error> {
        let mut tasks = super::tasks::Tasks::default();
        if let Some(scheduler) = &config.scheduler {
            let algorithm = scheduler
                .code()
                .and_then(hirsel_kernels::schedule::Algorithm::from_code)
                .ok_or_else(|| invalid("unsupported scheduler"))?;
            tasks.schedule = hirsel_kernels::schedule::Schedule::new(algorithm, scheduler.seed);
        }
        tasks.root_priority = tasks.schedule.admit();
        Ok(Self {
            simulation: Simulation::new(run, config)?,
            races: Default::default(),
            entry_signature: None,
            exit_report: None,
            tasks,
            channels: Default::default(),
            synchronizers: Default::default(),
            network_waits: Default::default(),
            file_waits: Default::default(),
            watch_waits: Default::default(),
            process_advance: None,
            timers: Default::default(),
            signals: Default::default(),
            callbacks: Default::default(),
            next_callback: 0,
            cpu_active: Default::default(),
            cpu_waits: Default::default(),
            cpu_certified: false,
            next_shared: 0,
            drop_failure: None,
            runners: Default::default(),
            free: None,
        })
    }
    /// Whether the last choice among `count` runnable tasks is a scheduling input a recording
    /// keeps (`hirsel_kernels::schedule::Schedule::recorded`).
    pub fn schedule_recorded(&self, count: usize) -> bool {
        self.tasks.schedule.recorded(count)
    }
    /// `stdin.read` over the configured input; a host adapter resumes a parked read here.
    pub fn read_stdin(
        &mut self,
        a: &[Value],
        context: &mut OperationContext<'_>,
    ) -> Result<OperationResult, Error> {
        self.simulation.id(arg(a, 0)?, "Stdin")?;
        let max = int(arg(a, 2)?)?.bits() as u64;
        let mut buffer = arg(a, 1)?.clone();
        let Value::Sequence {
            kind: BuiltinType::List,
            elements,
            ..
        } = &mut buffer
        else {
            return Err(invalid("stdin.read expects List[u8]"));
        };
        let request = encode_arguments(&[a[0].clone(), a[2].clone()]);
        // A tape completion is the input provider during replay, including short reads.
        // The current Stdin handle was checked above; no configured bytes are consulted.
        let replay = self
            .simulation
            .replay_output("Stdin", "stdin.read", &request)?;
        let replaying = replay.is_some();
        let completion = match replay {
            Some(bytes) => decode_stdin_completion(bytes)?,
            None if max == 0 => Err((2, 0)),
            None => match self
                .tasks
                .canceled()
                .map(Fault::Canceled)
                .or_else(|| self.simulation.fault("stdin.read"))
            {
                Some(Fault::Canceled(cause)) => Err((1, cause)),
                Some(Fault::Fault { code, .. }) => Err((0, code)),
                Some(Fault::File { .. }) => return Err(invalid("file error supplied to stdin")),
                None => {
                    let position = self.simulation.stdin_position;
                    let remaining = &self.simulation.config.stdin[position..];
                    let len = remaining
                        .len()
                        .min(usize::try_from(max).unwrap_or(usize::MAX));
                    Ok(remaining[..len].to_vec())
                }
            },
        };
        let valid = match &completion {
            Ok(bytes) => max > 0 && bytes.len() as u64 <= max,
            Err((0, _)) => max > 0,
            Err((1, cause)) => max > 0 && *cause <= super::tasks::CANCEL_DEADLINE,
            Err((2, 0)) => max == 0,
            _ => false,
        };
        if !valid {
            return Err(invalid("invalid stdin tape completion"));
        }
        let recorded = encode_stdin_completion(&completion);
        let (value, delivered) = match completion {
            Ok(bytes) => (ok(u64_value(bytes.len() as u64)), bytes),
            Err((0, code)) => (error(variant(0, vec![u32_value(code)])), vec![]),
            Err((1, cause)) => (error(variant(1, vec![variant(cause, vec![])])), vec![]),
            Err((2, _)) => (error(variant(2, vec![])), vec![]),
            _ => unreachable!(),
        };
        self.simulation
            .record("Stdin", "stdin.read", request, recorded, context)?;
        if !replaying {
            self.simulation.stdin_position += delivered.len();
        }
        elements.extend(delivered.into_iter().map(|byte| {
            Some(Value::Integer(
                Integer::new(Primitive::U8, byte.into()).unwrap(),
            ))
        }));
        let mut result = OperationResult::returned(value);
        if let OperationResult::Complete { arguments, .. } = &mut result {
            arguments.push((1, buffer));
        }
        Ok(result)
    }
    fn capability(
        &mut self,
        name: &str,
        call: OperationCall<'_>,
        context: &mut OperationContext<'_>,
    ) -> Result<Option<OperationResult>, Error> {
        self.tasks.yield_now = true;
        if matches!(
            name,
            "file.read" | "file.seek" | "file.write" | "file.append" | "file.sync"
        ) {
            return self.file_stream_call(name, call, context).map(Some);
        }
        let a = &*call.arguments;
        if name == "random.bytes" {
            return self.random_bytes(a, context).map(Some);
        }
        let op = name.split_once('.').unwrap().1;
        let value = match name {
            "io.stdin" | "io.stderr" | "io.stdout" | "io.random" | "io.net" | "io.tasks" => {
                self.simulation.id(arg(a, 0)?, "Io")?;
                let kind = match op {
                    "stdin" => "Stdin",
                    "stderr" => "Stderr",
                    "stdout" => "Stdout",
                    "random" => "Random",
                    "net" => "Net",
                    _ => "Tasks",
                };
                if op == "clock" {
                    self.simulation.handle(kind, Grant::Root(kind.into()))
                } else if self.simulation.config.projections.contains(op) {
                    ok(if kind == "Net" {
                        self.simulation.net_root()
                    } else {
                        self.simulation.handle(kind, Grant::Root(kind.into()))
                    })
                } else {
                    error(variant(0, vec![]))
                }
            }
            "io.signals" => {
                self.simulation.id(arg(a, 0)?, "Io")?;
                self.project_signals()
            }
            "io.monotonic_clock" | "io.wall_clock" => {
                self.simulation.id(arg(a, 0)?, "Io")?;
                let kind = if op == "monotonic_clock" {
                    "MonotonicClock"
                } else {
                    "WallClock"
                };
                if self.simulation.config.grants.contains(kind) {
                    ok(self.simulation.handle(kind, Grant::Root(kind.into())))
                } else {
                    error(variant(0, vec![]))
                }
            }
            "clock.monotonic" | "clock.wall" => {
                let kind = if op == "monotonic" {
                    "MonotonicClock"
                } else {
                    "WallClock"
                };
                self.simulation.id(arg(a, 0)?, kind)?;
                match self
                    .simulation
                    .config
                    .faults
                    .get(name)
                    .and_then(|queue| queue.front())
                    .cloned()
                {
                    Some(Fault::Fault { code, partial: 0 }) if code <= 1 => {
                        error(variant(code, vec![]))
                    }
                    Some(_) => return Err(invalid("invalid nonblocking clock error")),
                    None if op == "monotonic" => {
                        let ticks = self.simulation.config.monotonic;
                        if self
                            .simulation
                            .last_monotonic
                            .is_some_and(|last| ticks < last)
                        {
                            return Err(invalid("regressing monotonic input"));
                        }
                        ok(Value::Handle {
                            kind: "Instant".into(),
                            id: ticks,
                        })
                    }
                    None => ok(Value::Integer(
                        Integer::new(Primitive::I64, self.simulation.config.wall as u64 as u128)
                            .unwrap(),
                    )),
                }
            }
            "io.dir" | "dir.narrow" | "dir.read_only" => {
                // A read-only Dir is the directory itself, its access lowered to Read.
                let requested = if name == "dir.read_only" {
                    ""
                } else {
                    path_text(arg(a, 1)?)?
                };
                let selected = if name == "io.dir" {
                    self.simulation.id(arg(a, 0)?, "Io")?;
                    if self.simulation.config.projections.contains("dir") {
                        self.simulation
                            .config
                            .mounts
                            .iter()
                            .find_map(|(selector, access)| {
                                let local = if selector.is_empty() {
                                    Some(requested)
                                } else if requested == selector {
                                    Some("")
                                } else {
                                    requested.strip_prefix(selector)?.strip_prefix('/')
                                }?;
                                Some((0, selector.clone(), 0, *access, local.to_owned()))
                            })
                    } else {
                        None
                    }
                } else {
                    let (tree, root, directory) = self.simulation.directory(arg(a, 0)?)?;
                    let Some(Grant::Dir { access, .. }) = self.simulation.grants.get(&directory)
                    else {
                        unreachable!()
                    };
                    let access = if name == "dir.read_only" {
                        Access::Read
                    } else {
                        *access
                    };
                    Some((tree, root, directory, access, requested.to_owned()))
                };
                if let Some((tree, root, directory, access, local)) = selected {
                    match self.simulation.fault(name) {
                        Some(f) => fault_result(f),
                        None => {
                            let resolved = if name == "io.dir" {
                                self.simulation.resolve_mount(&root, &local)
                            } else {
                                self.simulation
                                    .resolve_in(tree, directory, &root, &local, true)
                            };
                            match resolved {
                                Ok(physical)
                                    if self.simulation.trees[&tree].0.contains(&physical)
                                        || self
                                            .simulation
                                            .detached_directories
                                            .contains(&directory)
                                        || (name == "io.dir"
                                            && self.simulation.detached_mounts.contains(&root)) =>
                                {
                                    let logical =
                                        hirsel_run_schema::confined_relative(&root, &local)
                                            .unwrap();
                                    let handle = self.simulation.handle(
                                        "Dir",
                                        Grant::Dir {
                                            tree,
                                            root: logical,
                                            access,
                                        },
                                    );
                                    let Value::Handle { id, .. } = &handle else {
                                        unreachable!()
                                    };
                                    self.simulation.resolved_directories.insert(*id, physical);
                                    if self.simulation.detached_directories.contains(&directory)
                                        || (name == "io.dir"
                                            && self.simulation.detached_mounts.contains(&root))
                                    {
                                        self.simulation.detached_directories.insert(*id);
                                    }
                                    ok(handle)
                                }
                                Ok(physical) => file_error(
                                    if self.simulation.trees[&tree].1.contains_key(&physical) {
                                        FileError::NotDirectory
                                    } else {
                                        FileError::NotFound
                                    },
                                ),
                                Err(error) => file_error(error),
                            }
                        }
                    }
                } else {
                    error(variant(2, vec![]))
                }
            }
            "arg.len" => {
                self.simulation.id(arg(a, 0)?, "Args")?;
                u64_value(self.simulation.config.args.len() as u64)
            }
            "arg.text" | "arg.path" => {
                self.simulation.id(arg(a, 0)?, "Args")?;
                let i = index(arg(a, 1)?)?;
                self.simulation
                    .config
                    .args
                    .get(i)
                    .map(|s| {
                        ok(if name == "arg.path" {
                            path(s)
                        } else {
                            Value::String(s.clone())
                        })
                    })
                    .unwrap_or_else(|| error(variant(0, vec![])))
            }
            "env.get" => {
                let env = self.simulation.id(arg(a, 0)?, "Env")?;
                let name = text(arg(a, 1)?)?;
                match self
                    .simulation
                    .env_granted(env, name)
                    .then(|| self.simulation.config.env.get(name))
                    .flatten()
                {
                    None => error(variant(0, vec![])),
                    Some(None) => ok(none()),
                    Some(Some(value)) => ok(some(Value::String(value.clone()))),
                }
            }
            "env.narrow" => {
                let env = self.simulation.id(arg(a, 0)?, "Env")?;
                let names = collection::elements(arg(a, 1)?, BuiltinType::List)?
                    .iter()
                    .map(|v| text(v).map(str::to_owned))
                    .collect::<Result<std::collections::BTreeSet<_>, _>>()?;
                if names.iter().all(|n| self.simulation.env_granted(env, n)) {
                    ok(self.simulation.handle("Env", Grant::Env(names)))
                } else {
                    error(variant(0, vec![]))
                }
            }
            "stdin.read" => return self.read_stdin(a, context).map(Some),
            "err.print" | "out.print" | "err.write" | "out.write" => {
                self.simulation.id(
                    arg(a, 0)?,
                    if name.starts_with("err.") {
                        "Stderr"
                    } else {
                        "Stdout"
                    },
                )?;
                match self.free_print_begin(name, a, context)? {
                    None => {}
                    Some(super::free::FreePrint::Done(result)) => return Ok(Some(result)),
                    Some(super::free::FreePrint::Write(write)) => {
                        return self.free_print_end(write, name, a, context).map(Some);
                    }
                }
                let canceled = self.tasks.canceled();
                self.print(name, a, canceled, context)?
            }
            "test.check" | "test.check_case" => {
                self.simulation.id(arg(a, 0)?, "Test")?;
                let Value::Bool(condition) = arg(a, 1)? else {
                    return Err(invalid("test.check expects bool"));
                };
                if !condition {
                    if name == "test.check_case" {
                        let case = int(arg(a, 2)?)?;
                        if case.kind() != Primitive::U64 {
                            return Err(invalid("test.check_case expects u64"));
                        }
                        let case = case.bits() as u64;
                        let seed = self.simulation.config.test_seed;
                        context.stdout.extend_from_slice(
                            format!("test failure: seed={seed} case={case}\n").as_bytes(),
                        );
                    }
                    let failure = fail(&call, context, "LANG-9", "test.check(false)");
                    self.simulation.record(
                        "Test",
                        name,
                        encode_arguments(a),
                        b"failure:LANG-9".to_vec(),
                        context,
                    )?;
                    return Ok(Some(OperationResult::Failed(failure)));
                }
                Value::Unit
            }
            // Record the failure, then end the test along the
            // call's failure edge. It never completes (TIR v3 diverging call).
            "test.fail" => {
                self.simulation.id(arg(a, 0)?, "Test")?;
                let failure = fail(&call, context, "LANG-9", "test.fail");
                self.simulation.record(
                    "Test",
                    name,
                    encode_arguments(a),
                    b"failure:LANG-9".to_vec(),
                    context,
                )?;
                return Ok(Some(OperationResult::Failed(failure)));
            }
            "test.clock" => {
                self.simulation.id(arg(a, 0)?, "Test")?;
                let clock = self
                    .simulation
                    .handle("MonotonicClock", Grant::Root("MonotonicClock".into()));
                self.simulation
                    .test_clocks
                    .insert(self.simulation.id(&clock, "MonotonicClock")?);
                clock
            }
            "test.seed" => {
                self.simulation.id(arg(a, 0)?, "Test")?;
                u64_value(self.simulation.config.test_seed)
            }
            // The test's own tasks, under the run's scheduler; no host authority.
            "test.tasks" => {
                self.simulation.id(arg(a, 0)?, "Test")?;
                self.simulation.handle("Tasks", Grant::Root("Tasks".into()))
            }
            // Test report records (hirsel_kernels::report); an invalid snapshot name
            // fails LANG-9 before anything is reported.
            "test.log" | "test.expect_snapshot" => {
                self.simulation.id(arg(a, 0)?, "Test")?;
                let record = if name == "test.log" {
                    hirsel_kernels::report::log(text(arg(a, 1)?)?)
                } else {
                    let snapshot = text(arg(a, 1)?)?;
                    if !hirsel_kernels::report::snapshot_name(snapshot) {
                        let failure = fail(&call, context, "LANG-9", "test.expect_snapshot name");
                        self.simulation.record(
                            "Test",
                            name,
                            encode_arguments(a),
                            b"failure:LANG-9".to_vec(),
                            context,
                        )?;
                        return Ok(Some(OperationResult::Failed(failure)));
                    }
                    hirsel_kernels::report::snapshot(snapshot, text(arg(a, 2)?)?)
                };
                context.stdout.extend_from_slice(&record);
                Value::Unit
            }
            "test.crash" => {
                self.simulation.id(arg(a, 0)?, "Test")?;
                let (tree, _, _) = self.simulation.directory(arg(a, 1)?)?;
                let Value::Integer(seed) = arg(a, 2)? else {
                    return Err(invalid("test.crash seed"));
                };
                let seed = u64::try_from(seed.bits()).map_err(|_| invalid("test.crash seed"))?;
                if !self.simulation.durable.contains_key(&tree) {
                    let failure = fail(&call, context, "LANG-9", "test.crash");
                    self.simulation.record(
                        "Test",
                        name,
                        encode_arguments(a),
                        b"failure:LANG-9".to_vec(),
                        context,
                    )?;
                    return Ok(Some(OperationResult::Failed(failure)));
                }
                self.simulation.crash(tree, seed);
                Value::Unit
            }
            "test.dir" => {
                self.simulation.id(arg(a, 0)?, "Test")?;
                let entries = collection::elements(arg(a, 1)?, BuiltinType::List)?;
                let mut files = std::collections::BTreeMap::new();
                let mut dirs = std::collections::BTreeSet::from([String::new()]);
                for entry in entries {
                    let Value::Tuple(pair) = entry else {
                        return Err(invalid("invalid test.dir entry"));
                    };
                    if pair.len() != 2 {
                        return Err(invalid("invalid test.dir pair"));
                    }
                    let p = path_text(pair[0].as_ref().ok_or_else(|| invalid("dead path"))?)?;
                    let p = hirsel_run_schema::confined_relative("", p)
                        .ok_or_else(|| invalid("test.dir entry escapes root"))?;
                    let data =
                        bytes(pair[1].as_ref().ok_or_else(|| invalid("dead bytes"))?)?.to_vec();
                    simulation::parents(&p, &mut dirs);
                    files.insert(p, data);
                }
                if files.keys().any(|p| dirs.contains(p)) {
                    return Err(invalid("test.dir file/directory collision"));
                }
                let tree = self.simulation.trees.keys().next_back().unwrap() + 1;
                self.simulation.trees.insert(tree, (dirs, files));
                self.simulation.identify_tree(tree);
                self.simulation.durable_start(tree);
                self.simulation.handle(
                    "Dir",
                    Grant::Dir {
                        tree,
                        root: String::new(),
                        access: Access::ReadWrite,
                    },
                )
            }
            n if n.starts_with("file.") || n == "dir.remove_all" => self.file(name, &call)?,
            _ => return Ok(None),
        };
        let cap = if name.starts_with("io.") {
            "Io"
        } else if name == "clock.monotonic" {
            "MonotonicClock"
        } else if name == "clock.wall" {
            "WallClock"
        } else if name.starts_with("arg.") {
            "Args"
        } else if name.starts_with("test.") {
            "Test"
        } else if name.starts_with("env.") {
            "Env"
        } else if name.starts_with("err.") {
            "Stderr"
        } else if name.starts_with("out.") {
            "Stdout"
        } else {
            "Dir"
        };
        let record = self.simulation.record(
            cap,
            name,
            encode_arguments(a),
            encode_value(&value),
            context,
        );
        if let Err(error) = record {
            if matches!(name, "io.monotonic_clock" | "io.wall_clock")
                && let Value::Variant { variant: 0, fields } = &value
                && let Some(Some(Value::Handle { id, .. })) = fields.first()
            {
                self.simulation.grants.remove(id);
                self.simulation.next_id = *id;
            }
            return Err(error);
        }
        if matches!(name, "clock.monotonic" | "clock.wall") {
            self.simulation.fault(name);
        }
        if name == "clock.monotonic"
            && let Value::Variant { variant: 0, .. } = &value
        {
            self.simulation.last_monotonic = Some(self.simulation.config.monotonic);
        }
        let result = OperationResult::returned(value);
        Ok(Some(if matches!(name, "file.narrow" | "file.commit") {
            result.transferring([1])
        } else {
            result
        }))
    }
    fn file(&mut self, name: &str, call: &OperationCall<'_>) -> Result<Value, Error> {
        let a = &*call.arguments;
        let (tree, root, dir) = self.simulation.directory(arg(a, 0)?)?;
        let atomic = if matches!(name, "file.write_all" | "file.commit") {
            let id = self.simulation.id(arg(a, 1)?, "AtomicFile")?;
            let Some(
                grant @ Grant::Atomic {
                    tree: owner_tree,
                    dir: owner_dir,
                    ..
                },
            ) = self.simulation.grants.get(&id)
            else {
                return Err(invalid("wrong atomic grant"));
            };
            if *owner_tree != tree || *owner_dir != dir {
                return Err(invalid("wrong atomic directory"));
            }
            Some((id, grant.clone()))
        } else {
            None
        };
        if !self.simulation.writable(dir)
            && matches!(
                name,
                "file.create"
                    | "file.open_append"
                    | "file.write_bytes"
                    | "file.remove"
                    | "file.mkdir"
                    | "file.rename"
                    | "file.remove_dir"
                    | "file.set_permissions"
                    | "dir.remove_all"
                    | "file.create_atomic"
                    | "file.write_all"
                    | "file.commit"
                    | "file.put_new"
            )
        {
            return Ok(error(variant(2, vec![])));
        }
        if !matches!(
            name,
            "file.narrow" | "file.walk" | "file.write_all" | "file.commit"
        ) {
            let invalid_path = hirsel_run_schema::confined_relative(&root, path_text(arg(a, 1)?)?)
                .is_none()
                || (name == "file.rename"
                    && hirsel_run_schema::confined_relative(&root, path_text(arg(a, 2)?)?)
                        .is_none());
            let invalid_path = invalid_path
                || (name == "dir.remove_all"
                    && hirsel_run_schema::confined_relative(&root, path_text(arg(a, 1)?)?)
                        .as_deref()
                        == Some(&root))
                || (name == "file.set_permissions" && int(arg(a, 2)?)?.bits() > 0o777);
            if invalid_path {
                return Ok(file_fault(self.simulation.config.invalid_path_code));
            }
        }
        let canceled = (name != "file.narrow")
            .then(|| self.tasks.canceled())
            .flatten();
        let fault = match canceled {
            Some(cause) => Some(Fault::Canceled(cause)),
            None => self.simulation.file_fault(
                name,
                match name {
                    "file.write_bytes" | "file.write_all" | "file.put_new" => {
                        Some(bytes(arg(a, 2)?)?.len() as u64)
                    }
                    _ => None,
                },
            )?,
        };
        if let Some(f) = fault {
            // Cancellation has no effects. Other faults may commit a prefix.
            if let Fault::Fault { partial, .. } | Fault::File { partial, .. } = &f
                && *partial > 0
            {
                match name {
                    "dir.remove_all" => {
                        if let Ok(p) = self.simulation.resolve_in(
                            tree,
                            dir,
                            &root,
                            path_text(arg(a, 1)?)?,
                            false,
                        ) && hirsel_run_schema::confined_relative(&root, path_text(arg(a, 1)?)?)
                            .as_deref()
                            != Some(&root)
                        {
                            let _ = self.simulation.remove_tree(tree, &p, Some(*partial));
                        }
                    }
                    "file.write_bytes" => {
                        if let Ok(p) = self.simulation.resolve_in(
                            tree,
                            dir,
                            &root,
                            path_text(arg(a, 1)?)?,
                            false,
                        ) && self.simulation.trees[&tree].0.contains(parent(&p))
                            && !self.simulation.trees[&tree].0.contains(&p)
                        {
                            let b = bytes(arg(a, 2)?)?;
                            if b.is_empty() {
                                return Ok(fault_result(f));
                            }
                            if tree == 0 {
                                self.simulation.config.links.remove(&p);
                            }
                            let prefix = b[..(*partial).min(b.len())].to_vec();
                            self.simulation
                                .trees
                                .get_mut(&tree)
                                .unwrap()
                                .1
                                .insert(p.clone(), prefix.clone());
                            self.simulation.replace_identity(tree, &p);
                            let identity = self.simulation.file_identity(tree, &p).unwrap();
                            self.simulation.durable_write(tree, identity, 0, &prefix);
                        }
                    }
                    "file.write_all" => {
                        let id = self.simulation.id(arg(a, 1)?, "AtomicFile")?;
                        let Some(Grant::Atomic {
                            temp,
                            dir: d,
                            tree: t,
                            ..
                        }) = self.simulation.grants.get(&id)
                        else {
                            return Err(invalid("wrong atomic grant"));
                        };
                        if *d != dir || *t != tree {
                            return Err(invalid("wrong atomic directory"));
                        }
                        let temp = self
                            .simulation
                            .resolve_stored(tree, dir, temp, false)
                            .map_err(|_| invalid("invalid atomic path"))?;
                        let b = bytes(arg(a, 2)?)?;
                        let prefix = &b[..(*partial).min(b.len())];
                        let file = self
                            .simulation
                            .trees
                            .get_mut(&tree)
                            .unwrap()
                            .1
                            .get_mut(&temp)
                            .unwrap();
                        let start = file.len();
                        file.extend_from_slice(prefix);
                        self.simulation.refresh_file(tree, &temp);
                        let identity = self.simulation.file_identity(tree, &temp).unwrap();
                        self.simulation.durable_write(tree, identity, start, prefix);
                    }
                    _ => {}
                }
            }
            // Commit consumes its taken handle on every Result, even canceled.
            if name == "file.commit" {
                let id = self.simulation.id(arg(a, 1)?, "AtomicFile")?;
                if let Some(Grant::Atomic { temp, tree, .. }) = self.simulation.grants.remove(&id) {
                    let temp = self
                        .simulation
                        .resolve_stored(tree, dir, &temp, false)
                        .map_err(|_| invalid("invalid atomic path"))?;
                    self.simulation
                        .trees
                        .get_mut(&tree)
                        .unwrap()
                        .1
                        .remove(&temp);
                    self.simulation.unlink_identity(tree, &temp);
                }
            }
            if name == "file.narrow" {
                let id = self.simulation.id(arg(a, 1)?, "File")?;
                self.simulation.grants.remove(&id);
                return Ok(none());
            }
            return Ok(fault_result(f));
        }
        if name == "file.walk" {
            return Ok(
                match self.simulation.walk_entries(tree, dir, &root, "", "", true) {
                    Ok(entries) => ok(super::walk::iterator(a[0].clone(), "", entries, true)),
                    Err(error) => file_error(error),
                },
            );
        }
        if name == "file.narrow" {
            let id = self.simulation.id(arg(a, 1)?, "File")?;
            let Some(Grant::File {
                tree: t,
                path: p,
                identity,
                position,
                writable,
                ..
            }) = self.simulation.grants.remove(&id)
            else {
                return Err(invalid("wrong file grant"));
            };
            let fits = t == tree
                && !self.simulation.stale.contains(&id)
                && (root.is_empty() || p.starts_with(&format!("{root}/")))
                && self
                    .simulation
                    .resolve_stored(tree, dir, &p, true)
                    .ok()
                    .and_then(|p| self.simulation.file_identity(tree, &p))
                    == Some(identity);
            return Ok(if fits {
                some(self.simulation.handle(
                    "File",
                    Grant::File {
                        tree,
                        path: p,
                        dir,
                        identity,
                        position,
                        writable,
                    },
                ))
            } else {
                none()
            });
        }
        if matches!(name, "file.write_all" | "file.commit") {
            let (id, grant) = atomic.expect("atomic ownership checked before cancellation");
            let Grant::Atomic {
                temp,
                dst,
                identity,
                ..
            } = grant
            else {
                unreachable!()
            };
            if name == "file.commit" {
                self.simulation.grants.remove(&id);
            }
            if self.simulation.stale.contains(&id) {
                return Ok(file_fault(1));
            }
            let local = |p: &str| -> String {
                if root.is_empty() {
                    p.into()
                } else {
                    p.strip_prefix(&format!("{root}/")).unwrap().into()
                }
            };
            let temp = match self
                .simulation
                .resolve_in(tree, dir, &root, &local(&temp), false)
            {
                Ok(p) => p,
                Err(error) => return Ok(file_error(error)),
            };
            let dst = match self
                .simulation
                .resolve_in(tree, dir, &root, &local(&dst), false)
            {
                Ok(p) => p,
                Err(error) => return Ok(file_error(error)),
            };
            if name == "file.commit" && self.simulation.file_identity(tree, &temp) != Some(identity)
            {
                self.simulation
                    .trees
                    .get_mut(&tree)
                    .unwrap()
                    .1
                    .remove(&temp);
                self.simulation.unlink_identity(tree, &temp);
                if tree == 0 {
                    self.simulation.config.links.remove(&temp);
                }
                return Ok(file_fault(self.simulation.config.invalid_path_code));
            }
            if name == "file.commit" && tree == 0 {
                self.simulation.config.links.remove(&dst);
            }
            let files = &mut self.simulation.trees.get_mut(&tree).unwrap().1;
            if name == "file.write_all" {
                let data = bytes(arg(a, 2)?)?;
                let file = files
                    .get_mut(&temp)
                    .ok_or_else(|| invalid("dead atomic temporary"))?;
                let start = file.len();
                file.extend_from_slice(data);
                self.simulation.refresh_file(tree, &temp);
                self.simulation.durable_write(tree, identity, start, data);
            } else {
                let data = files
                    .remove(&temp)
                    .ok_or_else(|| invalid("dead atomic temporary"))?;
                files.insert(dst.clone(), data);
                self.simulation.publish_identity(tree, &temp, &dst);
            }
            return Ok(ok(Value::Unit));
        }
        let p = path_text(arg(a, 1)?)?;
        let Some(logical) = hirsel_run_schema::confined_relative(&root, p) else {
            return Ok(file_fault(self.simulation.config.invalid_path_code));
        };
        let follow = !matches!(
            name,
            "file.write_bytes"
                | "file.put_new"
                | "file.remove"
                | "file.metadata"
                | "file.mkdir"
                | "file.remove_dir"
                | "file.set_permissions"
                | "dir.remove_all"
                | "file.rename"
        );
        let p = match self.simulation.resolve_in(tree, dir, &root, p, follow) {
            Ok(p) => p,
            Err(error) => return Ok(file_error(error)),
        };
        if name == "file.read_dir" {
            let prefix = if root.is_empty() {
                logical.as_str()
            } else if logical == root {
                ""
            } else {
                logical.strip_prefix(&format!("{root}/")).unwrap()
            };
            return Ok(
                match self
                    .simulation
                    .walk_entries(tree, dir, &root, prefix, "", false)
                {
                    Ok(entries) => ok(super::walk::iterator(a[0].clone(), prefix, entries, false)),
                    Err(e) => file_error(e),
                },
            );
        }
        if matches!(
            name,
            "file.read_dir"
                | "file.metadata"
                | "file.mkdir"
                | "file.rename"
                | "file.remove_dir"
                | "file.set_permissions"
                | "dir.remove_all"
        ) {
            return self.file_lifecycle(name, a, (tree, dir, &root), &logical, &p);
        }
        if self.simulation.detached_directories.contains(&dir) {
            return Ok(file_error(FileError::IsDirectory));
        }
        match name {
            "file.open" => {
                if let Err(error) = self.simulation.file_bytes(tree, &p) {
                    Ok(file_error(error))
                } else {
                    Ok(ok(self.simulation.handle(
                        "File",
                        Grant::File {
                            tree,
                            position: 0,
                            writable: false,
                            path: logical,
                            dir,
                            identity: self
                                .simulation
                                .file_identity(tree, &p)
                                .expect("opened identity"),
                        },
                    )))
                }
            }
            "file.open_append" => {
                if let Err(error) = self.simulation.file_destination(tree, &p) {
                    return Ok(file_error(error));
                }
                if self.simulation.file_identity(tree, &p).is_none() {
                    self.simulation
                        .trees
                        .get_mut(&tree)
                        .unwrap()
                        .1
                        .insert(p.clone(), vec![]);
                    self.simulation.replace_identity(tree, &p);
                }
                let identity = self
                    .simulation
                    .file_identity(tree, &p)
                    .expect("append identity");
                Ok(ok(self.simulation.handle(
                    "File",
                    Grant::File {
                        tree,
                        path: logical,
                        dir,
                        identity,
                        position: 0,
                        writable: true,
                    },
                )))
            }
            "file.create" => {
                if let Err(error) = self.simulation.file_destination(tree, &p) {
                    return Ok(file_error(error));
                }
                self.simulation
                    .trees
                    .get_mut(&tree)
                    .unwrap()
                    .1
                    .insert(p.clone(), vec![]);
                let identity = if let Some(id) = self.simulation.file_identity(tree, &p) {
                    self.simulation.file_contents.insert(id, vec![]);
                    id
                } else {
                    self.simulation.replace_identity(tree, &p);
                    self.simulation.file_identity(tree, &p).unwrap()
                };
                self.simulation.sync_file(tree, identity);
                Ok(ok(self.simulation.handle(
                    "File",
                    Grant::File {
                        tree,
                        path: logical,
                        dir,
                        identity,
                        position: 0,
                        writable: true,
                    },
                )))
            }
            "file.watch" => {
                let (directories, files) = &self.simulation.trees[&tree];
                if !directories.contains(&p) && !files.contains_key(&p) {
                    return Ok(file_error(FileError::NotFound));
                }
                let Value::Bool(recursive) = arg(a, 2)? else {
                    return Err(invalid("invalid watch recursion"));
                };
                Ok(ok(self.simulation.handle(
                    "Watcher",
                    Grant::Watcher {
                        tree,
                        dir,
                        root: p,
                        recursive: *recursive,
                        queue: Default::default(),
                    },
                )))
            }
            "file.map" => match self.simulation.file_bytes(tree, &p).map(|b| b.len() as u64) {
                Err(error) => Ok(file_error(error)),
                Ok(length) => {
                    let identity = self
                        .simulation
                        .file_identity(tree, &p)
                        .expect("mapped identity");
                    Ok(ok(self.simulation.handle(
                        "Mapped",
                        Grant::Mapped {
                            tree,
                            dir,
                            identity,
                            length,
                            truncated: false,
                        },
                    )))
                }
            },
            "file.size" => Ok(self
                .simulation
                .file_bytes(tree, &p)
                .map(|b| ok(u64_value(b.len() as u64)))
                .unwrap_or_else(file_error)),
            "file.read_bytes" => Ok(self
                .simulation
                .file_bytes(tree, &p)
                .map(|b| ok(Value::Bytes(b.clone())))
                .unwrap_or_else(file_error)),
            "file.write_bytes" => {
                if let Err(error) = self.simulation.file_destination(tree, &p) {
                    return Ok(file_error(error));
                }
                let data = bytes(arg(a, 2)?)?.to_vec();
                if tree == 0 {
                    self.simulation.config.links.remove(&p);
                }
                self.simulation
                    .trees
                    .get_mut(&tree)
                    .unwrap()
                    .1
                    .insert(p.clone(), data.clone());
                self.simulation.replace_identity(tree, &p);
                let identity = self.simulation.file_identity(tree, &p).unwrap();
                self.simulation.durable_write(tree, identity, 0, &data);
                self.simulation.file_readiness.wrote(tree, &p);
                Ok(ok(Value::Unit))
            }
            "file.put_new" => {
                let data = bytes(arg(a, 2)?)?.to_vec();
                let durable = matches!(arg(a, 3)?, Value::Variant { variant: 0, .. });
                let (dirs, files) = &self.simulation.trees[&tree];
                if dirs.contains(&p)
                    || (tree == 0
                        && (self.simulation.config.links.contains_key(&p)
                            || (self.simulation.config.metadata.contains_key(&p)
                                && !files.contains_key(&p))))
                {
                    return Ok(file_error(FileError::AlreadyExists));
                }
                if let Err(error) = self.simulation.file_destination(tree, &p) {
                    return Ok(file_error(error));
                }
                if let Some(existing) = files.get(&p) {
                    if *existing != data {
                        return Ok(file_error(FileError::AlreadyExists));
                    }
                    if durable {
                        let identity = self.simulation.file_identity(tree, &p).unwrap();
                        self.simulation.durable_sync(tree, identity, &p);
                    }
                    return Ok(ok(Value::Unit));
                }
                self.simulation
                    .trees
                    .get_mut(&tree)
                    .unwrap()
                    .1
                    .insert(p.clone(), data.clone());
                self.simulation.replace_identity(tree, &p);
                let identity = self.simulation.file_identity(tree, &p).unwrap();
                if durable {
                    self.simulation.durable_sync(tree, identity, &p);
                } else {
                    self.simulation.durable_write(tree, identity, 0, &data);
                }
                Ok(ok(Value::Unit))
            }
            "file.remove" if tree == 0 && self.simulation.config.links.remove(&p).is_some() => {
                self.simulation.config.metadata.remove(&p);
                Ok(ok(Value::Unit))
            }
            "file.remove" => Ok(
                if self
                    .simulation
                    .trees
                    .get_mut(&tree)
                    .unwrap()
                    .1
                    .remove(&p)
                    .is_some()
                {
                    self.simulation.unlink_identity(tree, &p);
                    ok(Value::Unit)
                } else {
                    file_error(if self.simulation.trees[&tree].0.contains(&p) {
                        FileError::IsDirectory
                    } else {
                        FileError::NotFound
                    })
                },
            ),
            "file.create_atomic" => {
                if let Err(error) = self.simulation.file_destination(tree, &p) {
                    return Ok(file_error(error));
                }
                let mut serial = self.simulation.next_id;
                let temp = loop {
                    let candidate = format!("{}.hirsel-tmp-{serial}", p);
                    if !self.simulation.trees[&tree].1.contains_key(&candidate)
                        && !self.simulation.trees[&tree].0.contains(&candidate)
                        && !(tree == 0 && self.simulation.config.links.contains_key(&candidate))
                    {
                        break candidate;
                    }
                    serial += 1;
                };
                self.simulation
                    .trees
                    .get_mut(&tree)
                    .unwrap()
                    .1
                    .insert(temp.clone(), vec![]);
                self.simulation.replace_identity(tree, &temp);
                Ok(ok(self.simulation.handle(
                    "AtomicFile",
                    Grant::Atomic {
                        tree,
                        temp: format!("{logical}.hirsel-tmp-{serial}"),
                        dst: logical,
                        dir,
                        identity: self
                            .simulation
                            .file_identity(tree, &temp)
                            .expect("atomic identity"),
                    },
                )))
            }
            _ => Err(Error::Unavailable(format!("unimplemented {name}"))),
        }
    }
}
impl Library {
    /// A draw is nonblocking, so it is no cancellation point. A zero count returns
    /// empty Bytes without touching the provider.
    fn random_bytes(
        &mut self,
        a: &[Value],
        context: &mut OperationContext<'_>,
    ) -> Result<OperationResult, Error> {
        self.simulation.id(arg(a, 0)?, "Random")?;
        let Value::Integer(count) = arg(a, 1)? else {
            return Err(invalid("random.bytes count"));
        };
        let count = u64::try_from(count.bits()).map_err(|_| invalid("random.bytes count"))?;
        if count == 0 {
            // No draw consults the provider, so the operation does not yield.
            self.tasks.yield_now = false;
            return Ok(OperationResult::returned(ok(Value::Bytes(vec![]))));
        }
        let value = match self.simulation.random_bytes(count) {
            Ok(bytes) => ok(Value::Bytes(bytes)),
            Err(()) => error(variant(0, vec![])),
        };
        self.simulation.record(
            "Random",
            "random.bytes",
            count.to_le_bytes().to_vec(),
            encode_value(&value),
            context,
        )?;
        Ok(OperationResult::returned(value))
    }
    /// Cancellation of the currently entered task, suppressed during destruction.
    pub fn cancellation(&self) -> Option<u32> {
        self.tasks.canceled()
    }
    /// Counts the run's cancellations: a parked task's [`cancellation`](Self::cancellation)
    /// changes only when this does.
    pub fn cancellations(&self) -> u64 {
        self.tasks.cancellations
    }
    /// Stable spawn path for diagnostics of host-owned waits.
    pub fn task_id(&self) -> u64 {
        self.tasks.current
    }
    pub fn task_cancellation(&self, task: u64) -> Option<u32> {
        self.tasks.canceled_task(task)
    }
    pub fn task_path(&self) -> Vec<u32> {
        self.tasks.path()
    }
    /// A host-owned handle's drop raised a scripted child's LANG-9 failure.
    pub fn fail_drop(&mut self, kind: &str) {
        self.drop_failure = Some(("LANG-9".into(), kind.into()));
    }
}
impl Library {
    /// A text or byte write, with the cancellation its check observed.
    pub(super) fn print(
        &mut self,
        name: &str,
        a: &[Value],
        canceled: Option<u32>,
        context: &mut OperationContext<'_>,
    ) -> Result<Value, Error> {
        let bytes = output_bytes(name, arg(a, 1)?)?;
        let fault = canceled
            .map(Fault::Canceled)
            .or_else(|| self.simulation.fault(name));
        let output = if name.starts_with("err.") {
            &mut self.simulation.stderr
        } else {
            &mut *context.stdout
        };
        Ok(match fault {
            Some(Fault::Canceled(cause)) => error(variant(1, vec![variant(cause, vec![])])),
            Some(Fault::Fault { code, partial }) => {
                output.extend_from_slice(&bytes[..partial.min(bytes.len())]);
                error(variant(0, vec![u32_value(code)]))
            }
            Some(Fault::File { .. }) => {
                return Err(invalid("file error supplied to output"));
            }
            None => {
                output.extend_from_slice(bytes);
                ok(Value::Unit)
            }
        })
    }
    fn poll_inner(
        &mut self,
        token: u64,
        context: &mut OperationContext<'_>,
    ) -> Result<Option<OperationResult>, Error> {
        if self.cpu_waits.contains_key(&token) {
            return self.poll_cpu(token, context);
        }
        if self.synchronizers.takes.contains_key(&token) {
            return self.poll_take(token, context);
        }
        if self.network_waits.contains_key(&token) {
            return self.poll_network(token, context);
        }
        if let Some(mut arguments) = self.file_waits.remove(&token) {
            let saved = (!context.configuration.inputs.is_empty()).then(|| self.simulation.clone());
            match self.file_stream("file.read", &mut arguments, context) {
                Ok(Some(mut result)) => {
                    // Cancellation returns the buffer unchanged. A successful read
                    // already moved it into its writeback.
                    if let OperationResult::Complete {
                        arguments: updates, ..
                    } = &mut result
                        && !matches!(arguments[2], Value::Unit)
                    {
                        updates.push((2, std::mem::replace(&mut arguments[2], Value::Unit)));
                    }
                    return Ok(Some(result));
                }
                Ok(None) => {
                    self.file_waits.insert(token, arguments);
                    return Ok(None);
                }
                Err(error) => {
                    if let Some(saved) = saved {
                        self.simulation = saved;
                    }
                    self.file_waits.insert(token, arguments);
                    return Err(error);
                }
            }
        }
        if self.runners.waits_on(token) {
            return self.poll_runner(token, context);
        }
        if self.signals.contains(token) {
            let saved = (!context.configuration.inputs.is_empty()).then(|| self.simulation.clone());
            let result = self.poll_signal(token, context);
            if result.is_err()
                && let Some(saved) = saved
            {
                self.simulation = saved;
            }
            return result;
        }
        if self.watch_waits.contains_key(&token) {
            let saved = (!context.configuration.inputs.is_empty()).then(|| self.simulation.clone());
            let result = self.poll_watch(token, context);
            if result.is_err()
                && let Some(saved) = saved
            {
                self.simulation = saved;
            }
            return result;
        }
        if self.timers.contains(token) {
            let saved = (!context.configuration.inputs.is_empty()).then(|| self.simulation.clone());
            let result = self.poll_timer(token, context);
            if result.is_err()
                && let Some(saved) = saved
            {
                self.simulation = saved;
            }
            return result;
        }
        if self.channels.waits.contains_key(&token) {
            let saved = (!context.configuration.inputs.is_empty()).then(|| self.channels.clone());
            let result = self.poll_channel(token, context);
            if result.is_err()
                && let Some(saved) = saved
            {
                self.channels = saved;
            }
            result
        } else {
            self.poll_task(token, context)
        }
    }
}
impl Operations for Library {
    fn retained_bytes(&self) -> u64 {
        self.tasks
            .retained_bytes()
            .saturating_add(self.channels.retained_bytes())
            .saturating_add(
                self.cpu_waits
                    .values()
                    .map(super::cpu::Batch::bytes)
                    .sum::<u64>(),
            )
            .saturating_add(
                self.callbacks
                    .values()
                    .filter_map(|p| {
                        if let super::callbacks::Pending::Cpu(b) = p {
                            Some(b.bytes())
                        } else {
                            None
                        }
                    })
                    .sum::<u64>(),
            )
    }
    fn entry_returned(
        &mut self,
        value: &Value,
        runtime: &crate::aggregate::RuntimeTypes,
        context: &mut OperationContext<'_>,
    ) -> Result<bool, Error> {
        let Some(signature) = &self.entry_signature else {
            return Ok(false);
        };
        self.exit_report =
            crate::exit_report::returned_error(value, signature, runtime).map(|text| {
                crate::exit_report::Report {
                    text,
                    stdout_len: context.stdout.len(),
                    stderr_len: self.simulation.stderr.len(),
                }
            });
        if matches!(value, Value::Variant { variant: 1, .. }) && self.exit_report.is_none() {
            return Err(invalid("entry Error has no diagnostic payload view"));
        }
        Ok(true)
    }

    fn callback_finished(
        &mut self,
        token: u64,
        updates: &[(usize, Value)],
        abandoned: bool,
    ) -> Result<Vec<Value>, Error> {
        if abandoned
            && matches!(
                self.callbacks.get(&token),
                Some(super::callbacks::Pending::Cpu(_))
            )
        {
            let Some(super::callbacks::Pending::Cpu(batch)) = self.callbacks.remove(&token) else {
                unreachable!()
            };
            self.cpu_active.remove(&self.tasks.current);
            return Ok((*batch).release());
        }
        self.synchronization_writeback(token, updates, abandoned)
    }

    fn take_releases(&mut self) -> Vec<Value> {
        std::mem::take(&mut self.channels.released)
    }

    fn take_drop_failure(&mut self) -> Option<(String, String)> {
        self.drop_failure.take()
    }

    fn ownership_channel(&self, endpoint: u64) -> Option<u64> {
        self.channels.endpoint_channel(endpoint)
    }
    fn ownership_task_path(&self, id: u64) -> Option<Vec<u32>> {
        if id == 0 {
            Some(vec![])
        } else {
            self.tasks.child_path(id)
        }
    }
    fn retains_task_arguments(&self, token: u64) -> bool {
        matches!(
            self.callbacks.get(&token),
            Some(super::callbacks::Pending::Cpu(_))
        )
    }
    fn cpu_mapper_certified(&mut self, certified: bool) {
        self.cpu_certified = certified;
    }
    fn take_tasks(&mut self) -> Vec<crate::TaskStart> {
        self.tasks.take_starts()
    }
    fn enter_task(&mut self, id: u64, shielded: bool) {
        self.tasks.current = id;
        self.tasks.shielded = shielded;
    }
    /// The recorded resume in replay; otherwise the run's scheduler policy chooses, after
    /// lowering the priority of the task that ran last at a `pct@1` change point.
    fn choose_task(&mut self, runnable: &[u64]) -> Result<usize, Error> {
        if let Some(event) = self.simulation.next_replay() {
            if event.operation != "task.resume" || event.input.len() != 8 {
                return Err(invalid("missing task scheduling input"));
            }
            let id = u64::from_le_bytes(event.input.as_slice().try_into().unwrap());
            return runnable
                .iter()
                .position(|candidate| *candidate == id)
                .ok_or_else(|| invalid("scheduling input selects a blocked task"));
        }
        if let Some(priority) = self.tasks.schedule.point() {
            let ran = self.tasks.current;
            if let Some(slot) = self.tasks.priority(ran) {
                *slot = priority;
            }
        }
        let priorities: Vec<u64> = runnable
            .iter()
            .map(|&id| self.tasks.priority(id).map_or(0, |p| *p))
            .collect();
        Ok(self.tasks.schedule.choose(&priorities))
    }
    fn task_resumed(&mut self, id: u64, context: &mut OperationContext<'_>) -> Result<(), Error> {
        self.simulation.record(
            "Tasks",
            "task.resume",
            id.to_le_bytes().to_vec(),
            vec![],
            context,
        )
    }
    fn discard_task_result(&self, id: u64) -> bool {
        self.tasks.detached(id)
    }
    fn holds_result(&self, id: u64) -> bool {
        self.tasks.holds_result(id)
    }
    fn take_rests(&mut self, parent: u64) -> Vec<crate::TaskRest> {
        let rests = self.tasks.take_rests(parent);
        rests
            .into_iter()
            .filter(|rest| {
                let path = self.path_of(rest.child);
                self.free
                    .as_ref()
                    .is_none_or(|free| !free.session.at_cut(&path))
            })
            .collect()
    }
    fn frameless_task(&self, child: u64) -> bool {
        self.tasks.frameless.contains(&child)
    }
    fn rest_finished(
        &mut self,
        child: u64,
        failures: Vec<crate::observation::FailureRecord>,
    ) -> Result<(), Error> {
        // Result readiness and a returned rest's actual completion are
        // separate Free ops in both engines, even for a dropless rest.
        self.free_end(child)?;
        self.tasks.rest_finished(child, failures)
    }
    fn rests_returned(&mut self, parent: u64, result: &mut OperationResult) {
        self.tasks.rests_returned(parent, result)
    }
    fn rests_completed(
        &mut self,
        parent: u64,
        result: &mut OperationResult,
        context: &mut OperationContext<'_>,
    ) -> Result<(), Error> {
        self.free_rests_completed(parent, result, context)
    }
    fn task_finished(
        &mut self,
        id: u64,
        result: OperationResult,
        context: &mut OperationContext<'_>,
    ) -> Result<(), Error> {
        self.simulation.record(
            "Tasks",
            "task.complete",
            id.to_le_bytes().to_vec(),
            vec![u8::from(matches!(
                result,
                OperationResult::FailureChain { .. }
            ))],
            context,
        )?;
        self.free_end(id)?;
        self.cpu_active.remove(&id);
        self.tasks.finished(id, result)
    }
    fn defer_release(&mut self, root: &crate::value::SharedRoot) -> bool {
        self.free_defer_release(root)
    }
    fn root_finished(&mut self) -> Result<bool, Error> {
        self.free_end(0)?;
        Ok(self.free_replay())
    }
    fn wait_deadline(&self, token: u64) -> Option<u64> {
        self.timers.deadline(token)
    }
    fn simulation_point(&mut self, point: super::simulation::SchedulePoint) -> bool {
        let enabled = self.simulation.config.debug.enabled(point);
        self.tasks.yield_now |= enabled;
        enabled
    }
    fn shared_released(&mut self, root: u64, context: &mut OperationContext<'_>) {
        self.race_access("Shared", root, "shared.release", true, context);
        if self.simulation_point(super::simulation::SchedulePoint::RootRelease) {
            context.trace.push(crate::TraceEvent::SimulationPoint(
                super::simulation::SchedulePoint::RootRelease,
            ));
        }
    }
    fn take_yield(&mut self) -> bool {
        let point = std::mem::take(&mut self.tasks.yield_now);
        if point && let Some(trace) = &mut self.simulation.root_trace {
            trace.point(self.tasks.path());
        }
        point
    }
    fn expire_deadlines(&mut self, context: &mut OperationContext<'_>) -> Result<(), Error> {
        if let Some((from, to)) = self.process_advance.take() {
            self.simulation.record(
                "Proc",
                "process.script.timed@1.advance",
                [from.to_le_bytes(), to.to_le_bytes()].concat(),
                vec![],
                context,
            )?;
        }
        self.expire_due(context)?;
        if self
            .simulation
            .config
            .process
            .as_ref()
            .is_some_and(|g| g.timed.is_some())
        {
            self.claim_process_waits(context)?;
            if let Some((child, report)) = self.simulation.settle_children(&[]) {
                context.stdout.extend_from_slice(report.as_bytes());
                let (anchor, spawn_path) = self
                    .simulation
                    .process_sites
                    .get(&child)
                    .cloned()
                    .ok_or_else(|| invalid("timed child source missing"))?;
                let failure = crate::Failure {
                    rule: hirsel_diag::RuleId::new("LANG-9").unwrap(),
                    line: context.sources.location(anchor.site).map_or(0, |p| p.line),
                    site: anchor.site,
                    kind: "scripted child unmet".into(),
                };
                return Err(Error::ProviderFailed(Box::new(
                    crate::observation::FailureRecord {
                        failure,
                        anchor,
                        operation: None,
                        cause: None,
                        spawn_path,
                    },
                )));
            }
        }
        Ok(())
    }
    /// The outstanding event sources: the signal script's next arrival, then virtual time
    /// for pending timer waits.
    fn wait(&mut self) -> Result<bool, Error> {
        Ok(self.deliver_signal()? || self.advance_virtual_time())
    }
    fn take_calls(&mut self) -> Vec<crate::CalleeStart> {
        self.take_runner_starts()
    }
    fn take_cancels(&mut self) -> Vec<u64> {
        self.take_runner_cancels()
    }
    fn call_ended(
        &mut self,
        token: u64,
        end: crate::CalleeEnd,
        context: &mut OperationContext<'_>,
    ) -> Result<(), Error> {
        self.runner_ended(token, end, context)
    }
    fn cancel_run(&mut self) {
        // The provider cancels the callee run's root scope with cause `Parent` (RUN-9).
        self.tasks.cancel_root(1);
    }
    fn handles_failure(&self, token: u64) -> bool {
        self.tasks.pending.contains_key(&token)
            || self.synchronizers.pending.contains_key(&token)
            || matches!(
                self.callbacks.get(&token),
                Some(super::callbacks::Pending::Cpu(_))
            )
    }
    fn poll(
        &mut self,
        token: u64,
        context: &mut OperationContext<'_>,
    ) -> Result<Option<OperationResult>, Error> {
        if let Some(result) = self.free_poll(token, context)? {
            return Ok(result);
        }
        let result = self.poll_inner(token, context)?;
        self.free_polled(token, result, context)
    }

    fn resume(
        &mut self,
        token: u64,
        result: OperationResult,
        arguments: &mut [Value],
        context: &mut OperationContext<'_>,
    ) -> Result<OperationResult, Error> {
        if self.synchronizers.pending.contains_key(&token) {
            return self.resume_synchronization(token, result, context);
        }
        if let Some(pending) = self.tasks.pending.remove(&token) {
            self.resume_task(pending, result, context)
        } else {
            self.resume_callback(token, result, arguments, context)
        }
    }
    fn drop_handle(
        &mut self,
        kind: &str,
        id: u64,
        context: &mut OperationContext<'_>,
    ) -> Result<(), Error> {
        if kind == "Mutex" {
            if self.free_close("Mutex", id)? {
                return Ok(());
            }
            let released = self.drop_mutex(id);
            self.channels.released.extend(released);
            return Ok(());
        }
        if matches!(kind, "Sender" | "Receiver") {
            let kind = if kind == "Sender" {
                "Sender"
            } else {
                "Receiver"
            };
            if self.free_close(kind, id)? {
                return Ok(());
            }
            self.channels.close(kind, id)?;
            return Ok(());
        }
        if kind == "TaskSet" {
            self.tasks.drop_set(id);
            return Ok(());
        }
        if matches!(kind, "ReadHalf" | "WriteHalf") {
            if !matches!(
                self.simulation.grants.get(&id),
                Some(super::simulation::Grant::Half { .. })
            ) {
                return Ok(());
            }
            let report = self.simulation.close_half(id);
            if let Some(report) = &report {
                context.stdout.extend_from_slice(report.as_bytes());
                self.drop_failure = Some(("LANG-9".into(), "scripted net client unmet".into()));
            }
            self.simulation.record(
                "Net",
                "net.close",
                id.to_le_bytes().to_vec(),
                if report.is_some() {
                    b"failure:LANG-9".to_vec()
                } else {
                    vec![]
                },
                context,
            )?;
            return Ok(());
        }
        if kind == "Span" {
            return self.abandon(id, context);
        }
        if matches!(kind, "Stream" | "Listener" | "UdpSocket") {
            let open = if kind == "Stream" {
                self.simulation.net_streams.contains_key(&id)
            } else if kind == "UdpSocket" {
                self.simulation.udp_sockets.contains_key(&id)
            } else {
                self.simulation.net_listeners.contains_key(&id)
            };
            if open {
                let report = self.simulation.close_net(id);
                if let Some(report) = &report {
                    context.stdout.extend_from_slice(report.as_bytes());
                    self.drop_failure = Some(("LANG-9".into(), "scripted net client unmet".into()));
                }
                self.simulation.record(
                    "Net",
                    "net.close",
                    id.to_le_bytes().to_vec(),
                    if report.is_some() {
                        b"failure:LANG-9".to_vec()
                    } else {
                        vec![]
                    },
                    context,
                )?;
            }
            return Ok(());
        }
        if kind == "Child" || kind == "ChildGroup" {
            if !matches!(
                self.simulation.grants.get(&id),
                Some(super::simulation::Grant::Child { .. })
            ) {
                return Ok(());
            }
            let report = self.simulation.close_child(id);
            if let Some(report) = &report {
                context.stdout.extend_from_slice(report.as_bytes());
                self.drop_failure = Some(("LANG-9".into(), "scripted child unmet".into()));
            }
            self.simulation.record(
                "Proc",
                if kind == "Child" {
                    "child.close"
                } else {
                    "group.close"
                },
                id.to_le_bytes().to_vec(),
                if report.is_some() {
                    b"failure:LANG-9".to_vec()
                } else {
                    vec![]
                },
                context,
            )?;
            return Ok(());
        }
        if kind == "Mapped" || kind == "Watcher" {
            if self.simulation.grants.remove(&id).is_some() {
                self.simulation.record(
                    "Dir",
                    if kind == "Mapped" {
                        "mapped.close"
                    } else {
                        "watcher.close"
                    },
                    id.to_le_bytes().to_vec(),
                    vec![],
                    context,
                )?;
            }
            return Ok(());
        }
        if !matches!(kind, "File" | "AtomicFile") {
            return Ok(());
        }
        // Taken commit/narrow handles have already been consumed.
        let Some(grant) = self.simulation.grants.get(&id).cloned() else {
            return Ok(());
        };
        self.simulation.record(
            "Dir",
            if kind == "File" {
                "file.close"
            } else {
                "file.abort"
            },
            id.to_le_bytes().to_vec(),
            vec![],
            context,
        )?;
        self.simulation.grants.remove(&id);
        if let Grant::Atomic {
            tree, temp, dir, ..
        } = grant
        {
            let temp = self
                .simulation
                .resolve_stored(tree, dir, &temp, false)
                .map_err(|_| invalid("invalid atomic path"))?;
            self.simulation
                .trees
                .get_mut(&tree)
                .unwrap()
                .1
                .remove(&temp);
            self.simulation.unlink_identity(tree, &temp);
        }
        Ok(())
    }
    fn resolve(&self, path: &hirsel_syntax::DefPath) -> Option<crate::OperationId> {
        operation_ids()
            .get(&hirsel_syntax::DefId::get(path)?)
            .copied()
    }
    fn immediate(&self, operation: crate::OperationId) -> bool {
        self.free.is_none()
            && operation.name != "shared.thaw"
            && PRIMARY_HANDLERS
                .get(operation.index)
                .is_none_or(|entry| entry.2 == super::OperationDisposition::Primitive)
    }
    fn call(
        &mut self,
        call: OperationCall<'_>,
        context: &mut OperationContext<'_>,
    ) -> Result<Option<OperationResult>, Error> {
        match self.resolve(call.path) {
            Some(operation) => self.call_resolved(operation, call, context),
            None => Ok(None),
        }
    }
    fn call_resolved(
        &mut self,
        operation: crate::OperationId,
        call: OperationCall<'_>,
        context: &mut OperationContext<'_>,
    ) -> Result<Option<OperationResult>, Error> {
        let name = operation.name;
        if operation.index == PRIMARY_HANDLERS.len() {
            return super::dictionaries::call(name, call.arguments);
        }
        if operation.index == PRIMARY_HANDLERS.len() + 1 {
            return super::cursors::call(name, call.arguments);
        }
        let access = match name {
            "atomic.load" | "atomic.store" => match call.arguments.get(1) {
                Some(Value::Handle { kind, id }) if kind == "Atomic" => {
                    Some(("Atomic", *id, name == "atomic.store"))
                }
                _ => None,
            },
            "shared.thaw" => match call.arguments.first() {
                Some(Value::Shared(root)) => Some(("Shared", root.id(), false)),
                _ => None,
            },
            _ => None,
        };
        let result = (PRIMARY_HANDLERS[operation.index].1)(self, call, context, name)?;
        if result.is_some()
            && let Some((kind, id, write)) = access
        {
            self.race_access(kind, id, name, write, context);
        }
        Ok(result)
    }
}
type PrimaryHandler = fn(
    &mut Library,
    OperationCall<'_>,
    &mut OperationContext<'_>,
    &str,
) -> Result<Option<OperationResult>, Error>;

fn operation_ids() -> &'static hirsel_syntax::DefMap<crate::OperationId> {
    static IDS: std::sync::OnceLock<hirsel_syntax::DefMap<crate::OperationId>> =
        std::sync::OnceLock::new();
    IDS.get_or_init(|| {
        let mut ids = hirsel_syntax::DefMap::default();
        for (index, (names, _, _)) in PRIMARY_HANDLERS.iter().enumerate() {
            for &name in *names {
                ids.insert(
                    hirsel_syntax::DefId::intern(&hirsel_syntax::DefPath::core(name)),
                    crate::OperationId { index, name },
                );
            }
        }
        for &(name, _) in super::dictionaries::INTEGER_ORDERS {
            ids.insert(
                hirsel_syntax::DefId::intern(&hirsel_syntax::DefPath::core(name)),
                crate::OperationId {
                    index: PRIMARY_HANDLERS.len(),
                    name,
                },
            );
        }
        for name in super::cursors::operation_names() {
            let path = hirsel_syntax::DefId::intern(&hirsel_syntax::DefPath::core(&name));
            let name = Box::leak(name.into_boxed_str());
            ids.insert(
                path,
                crate::OperationId {
                    index: PRIMARY_HANDLERS.len() + 1,
                    name,
                },
            );
        }
        ids
    })
}

// The dispatch arms and exported path set are emitted from one declaration.
macro_rules! primary_operations {
    ($library:ident, $call:ident, $context:ident, $name:ident, $args:ident, $value:ident;
     $( $kind:ident: $( $path:literal )|+ => $handler:expr, )*
     ; finish $finish:expr) => {
        pub(super) const PRIMARY_OPERATIONS: &[(&str, super::OperationDisposition)] = &[
            $( $( ($path, super::OperationDisposition::$kind), )+ )*
        ];
        #[allow(unused_variables, unreachable_code)]
        const PRIMARY_HANDLERS: &[(&[&str], PrimaryHandler, super::OperationDisposition)] = &[
            $( (&[ $( $path ),+ ], |$library, $call, $context, $name| {
                let $args = &*$call.arguments;
                let $value = $handler;
                $finish
            }, super::OperationDisposition::$kind), )*
        ];
    };
}

primary_operations! { library, call, context, name, a, value;
    Primitive: "codec.encode" | "codec.decode" => {
        return super::codec::call(library, name, &call).map(Some);
    },
    Primitive: "type_shape.info" => super::shape::info(&call)?,
    // Typed generation: a generated body is its text; only the compiler reads it.
    Primitive: "codegen.body" => Value::String(text(arg(a, 0)?)?.to_owned()),
    Primitive: "$intrinsic.heap" => {
        if !a.is_empty() { return Err(invalid("heap operation arity")); }
        Value::Handle { kind: "$Heap".into(), id: 0 }
    },
    Primitive: "copy_to" => {
        match arg(a, 0)? {
            Value::Handle { kind, .. } if kind == "$Heap" || kind == crate::checked::ARENA_HANDLE => {},
            _ => return Err(invalid("copy_to destination is not an Arena")),
        }
        arg(a, 1)?.copied()
    },
    Primitive: "shared.freeze" | "shared.thaw" => {
        if name == "shared.thaw" {
            // Free mode: a thaw is an op, its outcome a recorded bit.
            match library.free_enter(hirsel_free::phase::Phase::Done, context)? {
                super::free::Enter::Off => {}
                super::free::Enter::Park(park) => return Ok(Some(park)),
                super::free::Enter::Run => {
                    let [Value::Shared(root)] = a else {
                        return Err(invalid("Shared representation"));
                    };
                    return library.free_thaw(root.clone(), context).map(Some);
                }
            }
        }
        let result = super::shared::call(name, a, &mut library.next_shared)?;
        if let (Some(trace), OperationResult::Complete { value: Value::Shared(root), .. }) =
            (&mut library.simulation.root_trace, &result)
            && name == "shared.freeze"
        {
            trace.frozen(root);
        }
        return Ok(Some(result));
    },
    Primitive: "shared.with" => { return library.shared_with(a).map(Some); },
    Capability: "mapped.with" => {
        // A rejected replay never publishes the view.
        let saved = (!context.configuration.inputs.is_empty()).then(|| library.simulation.clone());
        let result = library.mapped_with(a, context);
        if result.is_err() && let Some(saved) = saved { library.simulation = saved; }
        return result.map(Some);
    },
    Primitive: "slots.empty" | "slots.insert" | "slots.get" | "slots.remove" => {
        return super::slots::call(name, a).map(Some);
    },
    Capability: "io.signals" | "io.stdin" | "io.stderr" | "stdin.read" | "err.print" | "err.write" | "env.get" | "env.narrow" | "clock.monotonic" | "clock.wall" | "io.monotonic_clock" | "io.wall_clock" | "io.stdout" | "io.random" | "random.bytes" | "io.net" | "io.tasks" | "io.dir" | "dir.narrow" | "dir.read_only" | "arg.len" | "arg.text" | "arg.path" | "out.print" | "out.write" | "test.check" | "test.check_case" | "test.fail" | "test.seed" | "test.tasks" | "test.clock" | "test.log" | "test.expect_snapshot" | "test.dir" | "file.open" | "file.create" | "file.open_append" | "file.read" | "file.seek" | "file.write" | "file.append" | "file.narrow" | "file.walk" | "file.size" | "file.read_dir" | "file.metadata" | "file.mkdir" | "file.rename" | "file.remove_dir" | "file.set_permissions" | "dir.remove_all" | "file.read_bytes" | "file.write_bytes" | "file.remove" | "file.create_atomic" | "file.write_all" | "file.commit" | "file.sync" | "file.put_new" | "test.crash" | "file.map" | "file.watch" => {
            // A rejected replay never publishes effects or consumes grants/faults.
            if !context.configuration.inputs.is_empty() {
                let saved = library.simulation.clone();
                let out_len = context.stdout.len();
                let trace_len = context.trace.len();
                let result = library.capability(name, call, context);
                if result.is_err() {
                    library.simulation = saved;
                    context.stdout.truncate(out_len);
                    context.trace.truncate(trace_len);
                }
                return result;
            }
            return library.capability(name, call, context);
            },
    Capability: "cpu.map_scoped" => { return library.cpu_call(&call, context).map(Some); },
    Capability: "task.scope" | "task.scope_until" | "task.async" | "task.await" | "task.cancel" | "task.detach" | "task.set" | "task.add" | "task.await_any" | "task.await_at" | "task.cancel_all" | "task.detach_all" | "task.check_canceled" | "task.isolate" => { return library.task_call(name, &call, context).map(Some); },
    Capability: "channel.open" | "channel.send" | "channel.receive" | "sender.dup" => { let saved = (!context.configuration.inputs.is_empty()).then(|| library.channels.clone());
        let result = library.channel_call(name, &call, context);
        if result.is_err() && let Some(saved) = saved { library.channels = saved; }
        return result.map(Some); },
    Capability: "mutex.create" | "mutex.with" | "atomic.create" | "atomic.load" | "atomic.store" => { let saved = (!context.configuration.inputs.is_empty()).then(|| library.synchronizers.clone());
        let result = library.synchronization_call(name, &call, context);
        if result.is_err() && let Some(saved) = saved { library.synchronizers = saved; }
        return result.map(Some); },
    Primitive: "arena.seal" => { return super::sealed::seal(call).map(Some); },
    Primitive: "region.with" => { return library.region_with(call).map(Some); },
    Capability: "net.narrow" | "net.resolve" | "net.connect" | "net.connect_addr" | "net.read" | "net.read_into" | "net.write" | "net.write_all" | "net.shutdown" | "net.set_read_timeout" | "net.set_write_timeout" | "net.peer_addr" | "net.close" | "net.split" | "net.join" | "read_half.read" | "read_half.read_into" | "read_half.set_timeout" | "read_half.close" | "write_half.write" | "write_half.write_all" | "write_half.shutdown" | "write_half.set_timeout" | "write_half.close" | "tls.connect" | "tls.connect_as" | "tls.connect_with" | "tls.accept" | "tls.peer" | "tls.negotiated_protocol" | "tls.close" | "test.net" | "net.listen" | "net.unix_connect" | "net.unix_listen" | "net.accept" | "net.local_addr" | "net.udp_bind" | "udp.send_to" | "udp.recv_from" | "udp.close" => {
        return library.network_call(name, &call, context).map(Some);
    },
    // A rejected replay never publishes effects or consumes script entries; a scripted
    // child's read parks as a scripted peer's does.
    Capability: "io.process" | "proc.narrow" | "proc.run" | "proc.pipeline" | "test.process" | "proc.spawn" | "child.write" | "child.close_stdin" | "child.read_stdout" | "child.read_stderr" | "child.wait" | "child.kill" | "proc.spawn_pipeline" | "group.write" | "group.close_stdin" | "group.read_stdout" | "proc.wait_group" | "group.kill" => {
        return library.network_call(name, &call, context).map(Some);
    },
    Unavailable: "$intrinsic.CpuMapScoped" | "$intrinsic.TaskScopeEnter" | "$intrinsic.TaskScopeExit" | "$intrinsic.TaskSetHas" | "$intrinsic.TaskSetTake" | "$intrinsic.TaskScopeEnterUntil" | "$intrinsic.TaskScopeExitUntil" | "$intrinsic.TaskSpawn" | "$intrinsic.TaskFinish" | "$intrinsic.TaskJoin" | "$intrinsic.ChannelOpen" | "$intrinsic.ChannelSend" | "$intrinsic.ChannelReceive" | "$intrinsic.MutexCreate" | "$intrinsic.MutexTake" | "$intrinsic.MutexPut" | "$intrinsic.MutexKill" | "$intrinsic.MutexCause" | "$intrinsic.AtomicCreate" | "$intrinsic.AtomicLoad" | "$intrinsic.AtomicStore" | "$intrinsic.SharedThawTurn" | "$intrinsic.TaskCanceled" | "$intrinsic.IsolateBegin" | "$intrinsic.IsolateEnd" | "$intrinsic.IsolateCount" | "$intrinsic.IsolateKind" | "$intrinsic.IsolateRule" | "$intrinsic.IsolateDefinition" | "$intrinsic.IsolateLine" | "$intrinsic.IsolateColumn" | "$intrinsic.IsolateSpawn" | "$intrinsic.RunnerSend" | "$intrinsic.RunnerWait" | "$intrinsic.RunnerTake" | "$intrinsic.RunnerEntry" | "$intrinsic.RunnerArgument" | "$intrinsic.RunnerIo" | "$intrinsic.RunnerTasks" | "$intrinsic.RunnerReply" | "$intrinsic.RunnerGrants" => { return Err(Error::Unavailable(format!("{name} is an entry of the native task runtime's compiled bodies; the reference executes the task operations"))); },
    Capability: "io.telemetry" | "capture.begin" | "capture.context" | "capture.event" | "capture.finish" | "capture.log" | "capture.measure" => {
        // A rejected replay never publishes effects or delivers identities.
        let saved = (!context.configuration.inputs.is_empty()).then(|| library.simulation.clone());
        let result = library.telemetry_call(name, &call, context);
        if result.is_err() && let Some(saved) = saved { library.simulation = saved; }
        return result.map(Some);
    },
    Capability: "watcher.next" | "test.dir_event" => {
        // A rejected replay never publishes effects or consumes an event.
        let saved = (!context.configuration.inputs.is_empty()).then(|| library.simulation.clone());
        let result = library.watch_call(name, &call, context);
        if result.is_err() && let Some(saved) = saved { library.simulation = saved; }
        return result.map(Some);
    },
    Unavailable: "$intrinsic.MappedView" | "$intrinsic.MappedCheck" => { return Err(Error::Unavailable(format!("{name} is an entry of mapped.with's compiled body; the reference executes mapped.with"))); },
    Unavailable: "$intrinsic.NetReadInto" | "$intrinsic.ReadHalfReadInto" => { return Err(Error::Unavailable(format!("{name} is the native entry of a read_into's compiled body; the reference executes the read_into"))); },
    Primitive: "path.of" => path(text(arg(a, 0)?)?),
    Primitive: "runner.entry_path" => match arg(a, 0)? {
        Value::FunctionReference(entry) => Value::String(entry.definition.to_string()),
        _ => return Err(invalid("runner.entry_path expects an Entry")),
    },
    Capability: "io.runner" | "runner.grants" | "runner.narrow" | "runner.only" | "runner.compute" | "runner.call" | "runner.spawn" | "test.runner" => {
        return library.runner_call(name, &call, context).map(Some);
    },
    Capability: "signal.wait" => {
        // The initial attempt suspends once. A host poll completes the same
        // wait through wait_signal without yielding its resumed task again.
        library.tasks.yield_now = true;
        // A rejected replay never publishes effects, consumes arrivals or keeps a wait.
        let saved = (!context.configuration.inputs.is_empty()).then(|| (library.simulation.clone(), library.signals.clone()));
        let result = library.wait_signal(a, context);
        if result.is_err() && let Some((simulation, signals)) = saved { library.simulation = simulation; library.signals = signals; }
        return result.map(Some);
    },
    Capability: "clock.sleep_until" => {
        // A rejected replay never publishes effects, consumes faults or keeps a timer.
        let saved = (!context.configuration.inputs.is_empty()).then(|| (library.simulation.clone(), library.timers.clone()));
        let result = library.sleep_until(&call, context);
        if result.is_err() && let Some((simulation, timers)) = saved { library.simulation = simulation; library.timers = timers; }
        return result.map(Some);
    },
    Primitive: "instant.checked_add" => library.instant_checked_add(a)?,
    Primitive: "instant.ticks" => {
        match arg(a, 0)? {
            Value::Handle { kind, id } if kind == "Instant" => u64_value(*id),
            _ => return Err(invalid("invalid Instant representation")),
        }
    },
    Primitive: "int.widen" | "int.try_narrow" | "int.narrow" | "int.truncate" => {
                let dest = destination(&call)?;
                let operation = call.path.0.name.as_str();
                match integer::convert(operation, int(arg(a, 0)?)?, dest) {
                    Ok(v) => {
                        if operation == "try_narrow" {
                            v.map(|i| some(Value::Integer(i))).unwrap_or_else(none)
                        } else {
                            Value::Integer(v.unwrap())
                        }
                    }
                    Err(crate::scalar::ScalarError::InvalidOperation) => {
                        return Err(invalid("invalid integer conversion schema"));
                    }
                    Err(_) => {
                        return Ok(Some(OperationResult::Failed(fail(
                            &call,
                            context,
                            "SEM-1",
                            "unrepresentable conversion",
                        ))));
                    }
                }
            },
    Primitive: "int.wrapping_add" | "int.saturating_add" | "int.checked_add" | "int.unchecked_add"
        | "int.wrapping_sub" | "int.saturating_sub" | "int.checked_sub" | "int.wrapping_mul"
        | "int.checked_mul" => {
                let offset = usize::from(name == "int.unchecked_add");
                if offset == 1 {
                    library.simulation.id(arg(a, 0)?, "Unsafe")?;
                    library.simulation.record(
                        "Unsafe",
                        &name,
                        encode_arguments(a),
                        vec![],
                        context,
                    )?;
                }
                let v = integer::arith(
                    &call.path.0.name,
                    int(arg(a, offset)?)?,
                    int(arg(a, offset + 1)?)?,
                );
                let v = match v {
                    Err(crate::scalar::ScalarError::Overflow) if name == "int.unchecked_add" => {
                        return Ok(Some(OperationResult::UndefinedBehavior(crate::UndefinedBehavior {
                            kind: "undefined behaviour detected".into(),
                            operation: call.path.clone(), site: call.site,
                            line: context.sources.location(call.site).map(|site| site.line).unwrap_or(0),
                            contract: "mathematical sum must fit the destination integer type".into(),
                        })));
                    }
                    Err(_) => return Err(invalid("invalid integer schema")),
                    Ok(value) => value,
                };
                if matches!(name, "int.checked_add" | "int.checked_sub" | "int.checked_mul") {
                    v.map(|i| some(Value::Integer(i))).unwrap_or_else(none)
                } else {
                    Value::Integer(v.unwrap())
                }
            },
    Primitive: "int.leading_zeros" | "int.trailing_zeros" | "int.count_ones" | "int.swap_bytes"
        | "int.reverse_bits" => Value::Integer(
                integer::bits(&call.path.0.name, int(arg(a, 0)?)?)
                    .map_err(|_| invalid("invalid integer schema"))?,
            ),
    Primitive: "int.div_floor" | "int.mod" | "int.div_exact" => {
                match integer::divide(&call.path.0.name, int(arg(a, 0)?)?, int(arg(a, 1)?)?) {
                    Ok(i) => Value::Integer(i),
                    Err(_) => {
                        return Ok(Some(OperationResult::Failed(fail(
                            &call,
                            context,
                            "SEM-1",
                            "invalid integer division",
                        ))));
                    }
                }
            },
    Primitive: "f32.bits" | "f64.bits" => {
        float::bits(arg(a, 0)?).ok_or_else(|| invalid("expected float"))?
    },
    Primitive: "f32.from_bits" | "f64.from_bits" => {
        float::from_bits(int(arg(a, 0)?)?).ok_or_else(|| invalid("expected u32 or u64"))?
    },
    Primitive: "f64.from_f32" | "f32.from_f64" => {
        float::resize(arg(a, 0)?).ok_or_else(|| invalid("expected float"))?
    },
    Primitive: "char.to_u32" => match arg(a, 0)? {
        Value::Char(c) => u32_value((*c).into()),
        _ => return Err(invalid("expected char")),
    },
    Primitive: "char.from_u32" => {
        let n = u32::try_from(int(arg(a, 0)?)?.bits()).map_err(|_| invalid("expected u32"))?;
        char::from_u32(n).map_or_else(none, |c| some(Value::Char(c)))
    },
    Primitive: "float.min" | "float.max" | "float.min_number" | "float.max_number" => {
                float::minmax(&call.path.0.name, arg(a, 0)?.clone(), arg(a, 1)?.clone())
                    .ok_or_else(|| invalid("float type mismatch"))?
            },
    Primitive: "float.sqrt" | "float.floor" | "float.ceil" | "float.trunc" | "float.round"
        | "float.abs" => {
                float::unary(&call.path.0.name, arg(a, 0)?).ok_or_else(|| invalid("expected float"))?
            },
    Primitive: "float.mul_add" => {
                float::mul_add(arg(a, 0)?.clone(), arg(a, 1)?.clone(), arg(a, 2)?.clone())
                    .ok_or_else(|| invalid("float type mismatch"))?
            },
    Primitive: "float.from_int" => {
        float::from_int(int(arg(a, 0)?)?, destination(&call)?)
            .ok_or_else(|| invalid("expected float destination"))?
    },
    Primitive: "float.to_i32" => {
                match float::to_i32(arg(a, 0)?).ok_or_else(|| invalid("expected float"))? {
                    Ok(n) => ok(i32_value(n)),
                    Err(e) => error(variant(e, vec![])),
                }
            },
    Primitive: "string.byte_len" => u64_value(text(arg(a, 0)?)?.len() as u64),
    Primitive: "string.scalars" => collection::sequence(
                BuiltinType::Array,
                text(arg(a, 0)?)?.chars().map(Value::Char),
            ),
    Primitive: "string.positions" => {
                let s = text(arg(a, 0)?)?;
                collection::sequence(
                    BuiltinType::Array,
                    string::positions(s).map(|i| Value::Record {
                        definition: (&hirsel_syntax::DefPath::core("StrPos")).into(),
                        fields: vec![Some(Value::String(s.into())), Some(u64_value(i as u64))],
                    }),
                )
            },
    Primitive: "string.slice" => {
                let s = text(arg(a, 0)?)?;
                let start = position(arg(a, 1)?, s)?;
                let end = position(arg(a, 2)?, s)?;
                if let Some(v) = string::slice(s, start, end) {
                    Value::String(v.into())
                } else {
                    return Ok(Some(OperationResult::Failed(fail(
                        &call,
                        context,
                        "SEM-3",
                        "reversed or invalid string positions",
                    ))));
                }
            },
    Primitive: "bytes.json_number_end" => {
        let (result, _) = hirsel_kernels::numeric::json_number(bytes(arg(a, 0)?)?, int(arg(a, 1)?)?.bits() as u64);
        Value::Tuple(result.into_iter().map(|n| Some(u64_value(n))).collect())
    },
    Primitive: "bytes.integer" => {
        let Value::Bool(signed) = arg(a, 2)? else { return Err(invalid("integer signed flag")); };
        let word = |i| -> Result<u128, Error> { Ok(int(arg(a, i)?)?.bits()) };
        let result = hirsel_kernels::numeric::integer(bytes(arg(a, 0)?)?, word(1)? as u8, *signed,
            word(3)? << 64 | word(4)?, word(5)? << 64 | word(6)?);
        Value::Tuple(result.into_iter().map(|n| Some(u64_value(n))).collect())
    },
    Primitive: "bytes.decimal_word" => {
        let value = int(arg(a, 0)?)?.bits() as u64;
        let mut digits = [0; 20];
        let count = hirsel_kernels::numeric::decimal_word(value, &mut digits);
        let Value::Sequence { elements, .. } = arg_mut(call.arguments, 1)? else {
            return Err(invalid("decimal inline buffer"));
        };
        for (slot, byte) in elements.iter_mut().zip(&digits[..count as usize]) {
            *slot = Some(Value::Integer(Integer::new(Primitive::U8, u128::from(*byte)).unwrap()));
        }
        u64_value(count)
    },
    Primitive: "bytes.sha1" | "bytes.sha224" | "bytes.sha256" | "bytes.sha384" | "bytes.sha512" | "bytes.sha512_256" => {
        let algorithm = hirsel_kernels::sha::Algorithm::from_operation(&call.path.0.name).unwrap();
        if algorithm.block_size() == 64 && bytes(arg(a, 0)?)?.len() as u64 > u64::MAX / 8 {
            return Ok(Some(OperationResult::Failed(fail(&call, context, "SEM-1", "SHA input length overflow"))));
        }
        // eval charged steps and fuel before offering this borrowed-only operation.
        let digest = hirsel_kernels::sha::digest(algorithm, bytes(arg(a, 0)?)?, |_| Ok::<_, std::convert::Infallible>(())).unwrap();
        collection::sequence(BuiltinType::Inline, digest[..algorithm.output_size()].iter().map(|&n| Value::Integer(Integer::new(Primitive::U8, u128::from(n)).unwrap())))
    },
    Primitive: "bytes.find_any" => u64_value(hirsel_kernels::byte_scan::find_any(
        bytes(arg(a, 0)?)?, bytes(arg(a, 1)?)?, index(arg(a, 2)?)? as u64,
    )),
    Primitive: "bytes.base64_encode" | "bytes.base64_decode" => {
        use base64::Engine;
        let Value::Variant { variant: alphabet, .. } = arg(a, 1)? else {
            return Err(invalid("Base64Alphabet"));
        };
        let Value::Variant { variant: padding, .. } = arg(a, 2)? else {
            return Err(invalid("Base64Padding"));
        };
        let engine = match (alphabet, padding) {
            (0, 0) => base64::engine::general_purpose::STANDARD,
            (0, 1) => base64::engine::general_purpose::STANDARD_NO_PAD,
            (1, 0) => base64::engine::general_purpose::URL_SAFE,
            (1, 1) => base64::engine::general_purpose::URL_SAFE_NO_PAD,
            _ => return Err(invalid("Base64 configuration")),
        };
        let data = bytes(arg(a, 0)?)?;
        if call.path.is_core("bytes.base64_encode") {
            Value::Bytes(engine.encode(data).into_bytes())
        } else {
            engine.decode(data).ok().map_or_else(none, |data| some(Value::Bytes(data)))
        }
    },
    Primitive: "string.from_bytes" => match String::from_utf8(bytes(arg(a, 0)?)?.to_vec()) {
                Ok(s) => ok(Value::String(s)),
                Err(e) => error(Value::Bytes(e.into_bytes())),
            },
    Primitive: "list.len" | "array.len" => u64_value(collection::len(arg(a, 0)?)?),
    Primitive: "list.truncate" => {
        let len = index(arg(a, 1)?)?;
        let Value::Sequence { kind: BuiltinType::List, elements, .. } = arg(a, 0)? else {
            return Err(invalid("list.truncate requires List"));
        };
        if len >= elements.len() {
            Value::Unit
        } else {
            let mut kept = call.arguments[0].clone();
            let Value::Sequence { elements, .. } = &mut kept else { unreachable!() };
            // Complete.released is a cleanup stack, drained high to low even
            // when one release fails. Reverse the tail for ascending drops.
            let mut released = Vec::new();
            while elements.len() > len {
                released.push(elements.pop().flatten().ok_or_else(|| invalid("dead list element"))?);
            }
            return Ok(Some(OperationResult::Complete {
                value: Value::Unit, arguments: vec![(0, kept)],
                consumed: vec![], released,
            }));
        }
    },
    Primitive: "list.pop" | "list.remove" | "list.swap" | "list.reserve" => {
        if name == "list.reserve" {
            let [GenericArgument::Type(element)] = call.generics else {
                return Err(invalid("list.reserve type actual"));
            };
            let size = crate::bounded::operation_size(&call, *element)?;
            // Same logical count ceiling as the native library body: the
            // runtime's 2^62-1 extent limit with 64 bytes for metadata/alignment.
            let capacity = int(arg(call.arguments, 1)?)?.bits() as u64;
            if size != 0 && capacity > ((i64::MAX as u64 / 2) - 64) / size {
                return Ok(Some(OperationResult::Failed(fail(&call, context, "SEM-1", "list capacity overflow"))));
            }
        }
        let (list, rest) = call.arguments.split_first_mut().ok_or_else(|| invalid("missing builtin argument"))?;
        let a = &*rest;
        let Value::Sequence { kind: BuiltinType::List, elements, .. } = list else {
            return Err(invalid("list primitive requires List"));
        };
        let len = elements.len();
        let value = match name {
            "list.pop" => elements.pop().flatten().map_or_else(none, some),
            "list.remove" => {
                let i = index(arg(a, 0)?)?;
                if i >= len {
                    return Ok(Some(OperationResult::Failed(fail(&call, context, "MEM-14", "list index out of bounds"))));
                }
                elements.remove(i).ok_or_else(|| invalid("dead list element"))?
            }
            "list.swap" => {
                let i = index(arg(a, 0)?)?;
                let j = index(arg(a, 1)?)?;
                if i >= len || j >= len {
                    return Ok(Some(OperationResult::Failed(fail(&call, context, "MEM-14", "list index out of bounds"))));
                }
                elements.swap(i, j);
                Value::Unit
            }
            // Reference values have no observable capacity; do not allocate host
            // storage based on an unbounded language capacity request.
            "list.reserve" => { index(arg(a, 0)?)?; Value::Unit }
            _ => unreachable!(),
        };
        value
    },
    Primitive: "failure.check" => {
        let Value::Variant { variant, fields } = arg(a, 1)? else { return Err(invalid("CheckRule variant")); };
        if !fields.is_empty() || *variant > 2 { return Err(invalid("CheckRule ordinal")); }
        let Value::Bool(condition) = arg(a, 0)? else { return Err(invalid("check condition")); };
        if !condition {
            let rule = ["SEM-1", "SEM-3", "MEM-14"][*variant as usize];
            return Ok(Some(OperationResult::Failed(fail(&call, context, rule, "failed check"))));
        }
        Value::Unit
    },
    Primitive: "string.as_bytes" => arg(a, 0)?.byte_view(text(arg(a, 0)?)?.as_bytes().to_vec()),
    Primitive: "bytes.len" => u64_value(bytes(arg(a, 0)?)?.len() as u64),
    Primitive: "bytes.replace_range" => {
        let (list, rest) = call.arguments.split_first_mut().ok_or_else(|| invalid("missing replacement arguments"))?;
        let Value::Integer(start) = arg(rest, 0)? else { return Err(invalid("replacement start")); };
        let start = u64::try_from(start.bits()).map_err(|_| invalid("replacement start is not u64"))?;
        let data = bytes(arg(rest, 1)?)?;
        let Value::Sequence { kind: BuiltinType::List, elements, .. } = list else {
            return Err(invalid("replacement destination"));
        };
        let len = elements.len() as u64;
        if start > len || data.len() as u64 > len - start {
            return Ok(Some(OperationResult::Failed(fail(&call, context, "MEM-14", "invalid byte replacement interval"))));
        }
        for (slot, &byte) in elements[start as usize..].iter_mut().zip(data) {
            *slot = Some(Value::Integer(Integer::new(Primitive::U8, byte.into()).unwrap()));
        }
        Value::Unit
    },
    Primitive: "bytes.append" => {
        let (list, rest) = call.arguments.split_first_mut().ok_or_else(|| invalid("missing builtin argument"))?;
        let data = bytes(arg(rest, 0)?)?;
        if !data.is_empty() {
            let Value::Sequence { kind: BuiltinType::List, elements, .. } = list else {
                return Err(invalid("byte append destination"));
            };
            elements.extend(data.iter().map(|&b| Some(Value::Integer(Integer::new(Primitive::U8, b.into()).unwrap()))));
        }
        Value::Unit
    },
    Primitive: "bytes.append_from_history" => {
        let (list, rest) = call.arguments.split_first_mut().ok_or_else(|| invalid("missing history arguments"))?;
        let distance = index(arg(rest, 0)?)?;
        let count = index(arg(rest, 1)?)?;
        let Value::Sequence { kind: BuiltinType::List, elements, .. } = list else {
            return Err(invalid("history destination"));
        };
        if distance == 0 || distance > elements.len()
            || (count as u64) > ((1u64 << 62) - 17).saturating_sub(elements.len() as u64) {
            return Ok(Some(OperationResult::Failed(fail(&call, context, "SEM-3", "invalid byte history interval"))));
        }
        for _ in 0..count {
            let byte = elements[elements.len() - distance].clone();
            elements.push(byte);
        }
        Value::Unit
    },
    Primitive: "bytes.from_list" | "$intrinsic.TextFinish" => {
        let values = collection::elements(arg(a, 0)?, BuiltinType::List)?;
        let mut result = Vec::with_capacity(values.len());
        for v in values {
            let Value::Integer(i) = v else { return Err(invalid("byte list element")); };
            if i.kind() != Primitive::U8 { return Err(invalid("byte list element kind")); }
            result.push(i.bits() as u8);
        }
        return Ok(Some(OperationResult::Complete { value: if call.path.is_core("$intrinsic.TextFinish") {
            arg(a, 0)?.string_view(String::from_utf8(result).map_err(|_| invalid("compiler text invariant"))?)
        } else { arg(a, 0)?.byte_view(result) }, arguments: vec![], consumed: vec![0], released: vec![] }));
    },
    Primitive: "bytes.slice" => {
        let src = bytes(arg(a, 0)?)?;
        let start = index(arg(a, 1)?)?;
        let end = index(arg(a, 2)?)?;
        if start > end || end > src.len() {
            return Ok(Some(OperationResult::Failed(fail(&call, context, "SEM-3", "invalid byte interval"))));
        }
        arg(a, 0)?.byte_view(src[start..end].to_vec())
    },
    Primitive: "string.position" => {
        let s = text(arg(a, 0)?)?;
        let offset = index(arg(a, 1)?)?;
        if s.is_char_boundary(offset) {
            some(Value::Record { definition: (&hirsel_syntax::DefPath::core("StrPos")).into(), fields: vec![Some(arg(a, 0)?.string_view(s.into())), Some(u64_value(offset as u64))] })
        } else { none() }
    },
    Primitive: "string.byte_offset" => u64_value(position(arg(a, 1)?, text(arg(a, 0)?)?)? as u64),
    Primitive: "path.text" => Value::String(path_text(arg(a, 0)?)?.to_owned()),
    Primitive: "list.push" => {
                let item = take_arg(call.arguments, 1)?;
                collection::push(arg_mut(call.arguments, 0)?, item)?;
                return Ok(Some(OperationResult::Complete {
                    value: Value::Unit,
                    arguments: vec![],
                    consumed: vec![1],
                    released: vec![],
                }));
            },
    Primitive: "map.len" | "set.len" => u64_value(collection::len(arg(a, 0)?)?),
    Primitive: "map.remove" => {
        let (map, key) = call.arguments.split_first_mut().ok_or_else(|| invalid("missing builtin argument"))?;
        return Ok(Some(keyed(collection::map_remove(map, arg(key, 0)?), &call, context)?));
    },
    Primitive: "set.remove" => {
        let (set, key) = call.arguments.split_first_mut().ok_or_else(|| invalid("missing builtin argument"))?;
        return Ok(Some(keyed(collection::set_remove(set, arg(key, 0)?), &call, context)?));
    },
    Primitive: "map.empty" => collection::sequence(BuiltinType::Map, []),
    Primitive: "set.empty" => collection::sequence(BuiltinType::Set, []),
    Primitive: "map.update_value_with" => {
        return Ok(Some(keyed(library.map_update_value(a), &call, context)?));
    },
    Primitive: "map.upsert_with" | "map.update_with" => {
        return Ok(Some(keyed(library.map_edit(a, name == "map.upsert_with"), &call, context)?));
    },
    Primitive: "map.insert" => {
                let (key, item) = (take_arg(call.arguments, 1)?, take_arg(call.arguments, 2)?);
                return Ok(Some(keyed(
                    collection::insert(arg_mut(call.arguments, 0)?, key, item)
                        .map(|displaced| displaced.into_result(vec![1, 2])),
                    &call,
                    context,
                )?));
            },
    Primitive: "set.insert" => {
                let item = take_arg(call.arguments, 1)?;
                return Ok(Some(keyed(
                    collection::set_insert(arg_mut(call.arguments, 0)?, item)
                        .map(|displaced| displaced.into_result(vec![1])),
                    &call,
                    context,
                )?));
            },
    Primitive: "map.get" => {
                return Ok(Some(keyed(
                    collection::get(arg(a, 0)?, arg(a, 1)?)
                        .map(|v| OperationResult::returned(v.map_or_else(none, some))),
                    &call,
                    context,
                )?));
            },
    Primitive: "map.contains" | "set.contains" => {
                return Ok(Some(keyed(
                    collection::contains(arg(a, 0)?, arg(a, 1)?)
                        .map(|b| OperationResult::returned(Value::Bool(b))),
                    &call,
                    context,
                )?));
            },
    Primitive: "iter.from" => {
                return Ok(Some(
                    OperationResult::returned(super::callbacks::iterator(
                        arg(a, 0)?.clone(),
                        arg(a, 1)?.clone(),
                    ))
                    .transferring([0, 1]),
                ));
            },
    Primitive: "iter.next" => {
        let mut it = arg(a, 0)?.clone();
        let saved = (!context.configuration.inputs.is_empty()).then(|| library.simulation.clone());
        let trace_len = context.trace.len();
        let walked = super::walk::pull(&mut it, |directory, p, after, recursive| library.read_directory_page(directory, p, after, recursive, context));
        let walked = match walked {
            Ok(walked) => walked,
            Err(error) => {
                if let Some(saved) = saved { library.simulation = saved; }
                context.trace.truncate(trace_len);
                return Err(error);
            }
        };
        return library.next_iterator(it, walked).map(Some);
    },
    Primitive: "simd.splat" => {
                let n = call
                    .generics
                    .iter()
                    .find_map(|g| {
                        if let GenericArgument::Size(hirsel_types::Size::Constant(n)) = g {
                            Some(*n)
                        } else {
                            None
                        }
                    })
                    .ok_or_else(|| invalid("missing SIMD width"))?;
                if n == 0 {
                    return Err(invalid("SIMD width must be positive"));
                }
                let n = usize::try_from(n).map_err(|_| invalid("SIMD width"))?;
                if !matches!(
                    arg(a, 0)?,
                    Value::Bool(_) | Value::Integer(_) | Value::F32(_) | Value::F64(_)
                ) {
                    return Err(invalid("SIMD splat requires scalar"));
                }
                collection::sequence(BuiltinType::Simd, vec![arg(a, 0)?.clone(); n])
            },
    Primitive: "simd.add" => {
                let (x, y) = (
                    collection::elements(arg(a, 0)?, BuiltinType::Simd)?,
                    collection::elements(arg(a, 1)?, BuiltinType::Simd)?,
                );
                if x.len() != y.len() || x.is_empty() {
                    return Err(invalid("SIMD lane mismatch"));
                }
                let mut out = vec![];
                for (a, b) in x.into_iter().zip(y) {
                    int(&a)?;
                    int(&b)?;
                    match crate::scalar::binary(hirsel_syntax::BinaryOp::Add, a, b) {
                        Ok(v) => out.push(v),
                        Err(_) => {
                            return Ok(Some(OperationResult::Failed(fail(
                                &call,
                                context,
                                "SEM-1",
                                "SIMD lane overflow",
                            ))));
                        }
                    }
                }
                collection::sequence(BuiltinType::Simd, out)
            },
    Primitive: "sorted_map.len" => super::callbacks::sorted_len(a)?,
    Primitive: "sorted_map.get" => { return Ok(Some(library.sorted_access(a, false)?)); },
    Primitive: "sorted_map.remove" => { return Ok(Some(library.sorted_access(a, true)?)); },
    Primitive: "sorted_map.empty" => super::callbacks::sorted(arg(a, 0)?.clone(), vec![]),
    Primitive: "sorted_map.insert" => return library.sorted_insert(a).map(Some),
    ; finish Ok(Some(OperationResult::returned(value)))
}

pub(super) fn parent(path: &str) -> &str {
    path.rsplit_once('/').map_or("", |(p, _)| p)
}
pub(super) fn arg(a: &[Value], i: usize) -> Result<&Value, Error> {
    a.get(i).ok_or_else(|| invalid("missing builtin argument"))
}
/// A `mut` argument, written in place.
fn arg_mut(a: &mut [Value], i: usize) -> Result<&mut Value, Error> {
    a.get_mut(i)
        .ok_or_else(|| invalid("missing builtin argument"))
}
/// A taken argument the completion transfers.
fn take_arg(a: &mut [Value], i: usize) -> Result<Value, Error> {
    Ok(std::mem::replace(arg_mut(a, i)?, Value::Unit))
}
pub(super) fn text(v: &Value) -> Result<&str, Error> {
    if let Value::String(s) | Value::ArenaString { text: s, .. } = v {
        Ok(s)
    } else {
        Err(invalid("expected String"))
    }
}
pub(super) fn bytes(v: &Value) -> Result<&[u8], Error> {
    if let Value::Bytes(s) | Value::ArenaBytes { bytes: s, .. } = v {
        Ok(s)
    } else {
        Err(invalid("expected Bytes"))
    }
}
pub(super) fn int(v: &Value) -> Result<Integer, Error> {
    if let Value::Integer(n) = v {
        Ok(*n)
    } else {
        Err(invalid("expected integer"))
    }
}
pub(super) fn index(v: &Value) -> Result<usize, Error> {
    let n = int(v)?;
    if n.kind() != Primitive::U64 {
        return Err(invalid("index requires u64"));
    }
    usize::try_from(n.bits()).map_err(|_| invalid("index too wide"))
}
pub fn path(s: &str) -> Value {
    Value::Record {
        definition: (&hirsel_syntax::DefPath::core("Path")).into(),
        fields: vec![Some(Value::String(s.into()))],
    }
}
pub(super) fn path_text(v: &Value) -> Result<&str, Error> {
    if let Value::Record { definition, fields } = v
        && definition.is_core("Path")
        && fields.len() == 1
    {
        text(fields[0].as_ref().ok_or_else(|| invalid("dead Path"))?)
    } else {
        Err(invalid("expected nominal Path"))
    }
}
fn position(v: &Value, s: &str) -> Result<usize, Error> {
    if let Value::Record { definition, fields } = v
        && definition.is_core("StrPos")
        && fields.len() == 2
        && fields[0].as_ref() == Some(&Value::String(s.into()))
    {
        index(fields[1].as_ref().ok_or_else(|| invalid("dead StrPos"))?)
    } else {
        Err(invalid("StrPos source mismatch"))
    }
}
fn destination(call: &OperationCall<'_>) -> Result<Primitive, Error> {
    let mut t = call
        .types
        .get(call.result_type)
        .ok_or_else(|| invalid("missing result type"))?;
    if let TypeKind::Named {
        definition,
        arguments,
    } = &t.kind
        && definition.path().0.name == "Option"
        && definition.path().is_core("Option")
    {
        if let GenericArgument::Type(id) = arguments[0] {
            t = call
                .types
                .get(id)
                .ok_or_else(|| invalid("missing conversion destination"))?;
        }
    }
    if let TypeKind::Primitive(p) = t.kind {
        Ok(p)
    } else {
        Err(invalid("result must be primitive"))
    }
}
pub fn variant(variant: u32, fields: Vec<Value>) -> Value {
    Value::Variant {
        variant,
        fields: fields.into_iter().map(Some).collect(),
    }
}
pub fn ok(v: Value) -> Value {
    variant(0, vec![v])
}
pub fn error(v: Value) -> Value {
    variant(1, vec![v])
}
pub fn some(v: Value) -> Value {
    variant(1, vec![v])
}
pub fn none() -> Value {
    variant(0, vec![])
}
pub fn u64_value(n: u64) -> Value {
    Value::Integer(Integer::new(Primitive::U64, n as u128).unwrap())
}
pub fn u32_value(n: u32) -> Value {
    Value::Integer(Integer::new(Primitive::U32, n as u128).unwrap())
}
pub fn i32_value(n: i32) -> Value {
    Value::Integer(Integer::new(Primitive::I32, n as u32 as u128).unwrap())
}
pub(super) fn file_fault(code: u32) -> Value {
    error(variant(0, vec![u32_value(code)]))
}
pub(super) fn file_error(f: FileError) -> Value {
    let ordinal = match f {
        FileError::Fault(code) => return file_fault(code),
        FileError::NotFound => 3,
        FileError::PermissionDenied => 4,
        FileError::AlreadyExists => 5,
        FileError::IsDirectory => 6,
        FileError::NotDirectory => 7,
        FileError::DirectoryNotEmpty => 8,
        FileError::CrossDevice => 9,
    };
    error(variant(ordinal, vec![]))
}
pub(super) fn fault_result(f: Fault) -> Value {
    match f {
        Fault::Canceled(c) => error(variant(1, vec![variant(c, vec![])])),
        Fault::Fault { code, .. } => file_fault(code),
        Fault::File { error, .. } => file_error(error),
    }
}
/// A key past the structural depth limit fails the keyed operation (MEM-3).
fn keyed(
    result: Result<OperationResult, collection::KeyedError>,
    call: &OperationCall<'_>,
    context: &OperationContext<'_>,
) -> Result<OperationResult, Error> {
    match result {
        Ok(result) => Ok(result),
        Err(collection::KeyedError::Depth) => Ok(OperationResult::Failed(fail(
            call, context, "MEM-3", "Depth",
        ))),
        Err(collection::KeyedError::Invalid(error)) => Err(error),
    }
}
pub(super) fn fail(
    call: &OperationCall<'_>,
    context: &OperationContext<'_>,
    rule: &str,
    kind: &str,
) -> Failure {
    Failure {
        rule: RuleId::new(rule).unwrap(),
        line: context.sources.location(call.site).map_or(0, |p| p.line),
        site: call.site,
        kind: kind.into(),
    }
}
pub(super) fn encode_arguments(arguments: &[Value]) -> Vec<u8> {
    let mut out = vec![];
    for arg in arguments {
        let bytes = encode_value(arg);
        out.extend_from_slice(&(bytes.len() as u64).to_le_bytes());
        out.extend_from_slice(&bytes);
    }
    out
}
pub(super) fn encode_value(value: &Value) -> Vec<u8> {
    fn atom(out: &mut Vec<u8>, bytes: &[u8]) {
        out.extend_from_slice(&(bytes.len() as u64).to_le_bytes());
        out.extend_from_slice(bytes);
    }
    fn path(out: &mut Vec<u8>, definition: &hirsel_syntax::DefPath) {
        atom(out, definition.0.package.name.as_bytes());
        atom(out, definition.0.package.version.as_bytes());
        out.extend_from_slice(&(definition.0.modules.len() as u64).to_le_bytes());
        for module in &definition.0.modules {
            atom(out, module.as_bytes());
        }
        atom(out, definition.0.name.as_bytes());
    }
    fn fields<'a>(out: &mut Vec<u8>, work: &mut Vec<&'a Value>, items: &'a [Option<Value>]) {
        out.extend_from_slice(&(items.len() as u64).to_le_bytes());
        out.extend(items.iter().map(|v| u8::from(v.is_some())));
        work.extend(items.iter().rev().flatten());
    }
    fn kind_tag(kind: BuiltinType) -> u8 {
        use BuiltinType::*;
        match kind {
            Array => 0,
            List => 1,
            Map => 2,
            Set => 3,
            SortedMap => 4,
            Inline => 5,
            Simd => 6,
            Packed => 7,
            Shared => 8,
            Slots => 9,
            SlotId => 10,
            Region => 11,
            Iter => 12,
            Task => 13,
            Sender => 14,
            Receiver => 15,
            Mutex => 16,
            Atomic => 17,
            Ptr => 18,
            FnPtr => 19,
            TypeShape => 20,
        }
    }
    fn size_bindings(out: &mut Vec<u8>, sizes: &[Option<u64>]) {
        out.extend_from_slice(&(sizes.len() as u64).to_le_bytes());
        for size in sizes {
            out.push(u8::from(size.is_some()));
            if let Some(size) = size {
                out.extend_from_slice(&size.to_le_bytes());
            }
        }
    }
    let mut out = vec![];
    let mut work = vec![value];
    while let Some(value) = work.pop() {
        match value {
            Value::Unit => out.push(0),
            Value::Bool(b) => out.extend_from_slice(&[1, u8::from(*b)]),
            Value::Integer(n) => {
                out.push(2);
                out.extend_from_slice(&[n.width() as u8, u8::from(n.is_signed())]);
                out.extend_from_slice(&n.bits().to_le_bytes());
            }
            Value::String(s) | Value::ArenaString { text: s, .. } => {
                out.push(3);
                atom(&mut out, s.as_bytes());
            }
            Value::Bytes(b) | Value::ArenaBytes { bytes: b, .. } => {
                out.push(4);
                atom(&mut out, b);
            }
            Value::Handle { kind, id } => {
                out.push(5);
                atom(&mut out, kind.as_bytes());
                out.extend_from_slice(&id.to_le_bytes());
            }
            Value::Mutex(root) => {
                out.push(5);
                atom(&mut out, b"Mutex");
                out.extend_from_slice(&root.id().to_le_bytes());
            }
            Value::Shared(root) => {
                out.push(20);
                out.extend_from_slice(&root.id().to_le_bytes());
            }
            Value::Variant {
                variant,
                fields: items,
            } => {
                out.push(6);
                out.extend_from_slice(&variant.to_le_bytes());
                fields(&mut out, &mut work, items);
            }
            Value::Record {
                definition,
                fields: items,
            } => {
                out.push(7);
                path(&mut out, definition.path());
                fields(&mut out, &mut work, items);
            }
            Value::Sequence { kind, elements, .. } => {
                out.extend_from_slice(&[8, kind_tag(*kind)]);
                fields(&mut out, &mut work, elements);
            }
            Value::Map { entries: items, .. } | Value::Set { entries: items, .. } => {
                out.push(if matches!(value, Value::Map { .. }) {
                    16
                } else {
                    17
                });
                out.extend_from_slice(&(items.live_len() as u64).to_le_bytes());
                out.extend(items.iter().flatten().map(|_| 1u8));
                work.extend(items.iter().rev().flatten());
            }
            Value::ResourceVariant {
                sizes,
                definition,
                variant,
                fields: items,
            } => {
                out.push(18);
                size_bindings(&mut out, sizes);
                path(&mut out, definition.path());
                out.extend_from_slice(&variant.to_le_bytes());
                fields(&mut out, &mut work, items);
            }
            Value::Tuple(items) => {
                out.push(9);
                fields(&mut out, &mut work, items);
            }
            Value::Char(c) => {
                out.push(21);
                out.extend_from_slice(&u32::from(*c).to_le_bytes());
            }
            Value::F32(b) => {
                out.push(10);
                out.extend_from_slice(&b.to_le_bytes());
            }
            Value::F64(b) => {
                out.push(11);
                out.extend_from_slice(&b.to_le_bytes());
            }
            Value::Box(items) | Value::ArenaBox { fields: items, .. } => {
                out.push(12);
                fields(&mut out, &mut work, items);
            }
            Value::Distinct {
                definition,
                fields: items,
            } => {
                out.push(13);
                path(&mut out, definition.path());
                fields(&mut out, &mut work, items);
            }
            Value::Resource {
                sizes,
                definition,
                fields: items,
            } => {
                out.push(14);
                size_bindings(&mut out, sizes);
                path(&mut out, definition.path());
                fields(&mut out, &mut work, items);
            }
            Value::FunctionReference(reference) => {
                out.push(19);
                path(&mut out, &reference.definition);
                out.extend_from_slice(&(reference.arguments.len() as u64).to_le_bytes());
                for argument in &reference.arguments {
                    match hirsel_types::encoding::canonical_argument_encoding(
                        &reference.types,
                        argument,
                    ) {
                        Ok(bytes) => {
                            out.push(1);
                            atom(&mut out, &bytes);
                        }
                        Err(_) => out.push(0), // Same explicit non-comparable class as observations.
                    }
                }
            }
            Value::Closure {
                definition,
                body,
                captures,
                environment,
                ..
            } => {
                let crate::value::Environment {
                    owned_captures,
                    sizes,
                    ..
                } = &**environment;
                out.push(15);
                path(&mut out, definition.path());
                out.extend_from_slice(&body.0.to_le_bytes());
                out.extend_from_slice(&(sizes.len() as u64).to_le_bytes());
                for ((scope, parameter), size) in sizes {
                    use hirsel_types::NameSite;
                    let (tag, ordinal): (u8, u32) = match scope.site {
                        NameSite::Expression(id) => (0, id.0),
                        NameSite::Pattern(id) => (1, id.0),
                        NameSite::Type(id) => (2, id.0),
                        NameSite::Statement(id) => (3, id.0),
                        NameSite::Item => (4, 0),
                    };
                    path(&mut out, scope.owner.def.path());
                    out.extend_from_slice(&scope.owner.occurrence.to_le_bytes());
                    out.push(tag);
                    out.extend_from_slice(&ordinal.to_le_bytes());
                    out.extend_from_slice(&parameter.0.to_le_bytes());
                    out.extend_from_slice(&size.to_le_bytes());
                }
                atom(
                    &mut out,
                    &owned_captures
                        .iter()
                        .map(|b| u8::from(*b))
                        .collect::<Vec<_>>(),
                );
                fields(&mut out, &mut work, captures);
            }
        }
    }
    out
}

// Stdin completions are independently framed input bytes, rather than the caller's
// mutable buffer or returned count. Error tags are the core InputError ordinals.
fn encode_stdin_completion(value: &Result<Vec<u8>, (u32, u32)>) -> Vec<u8> {
    match value {
        Ok(bytes) => {
            let mut output = vec![0];
            output.extend_from_slice(&(bytes.len() as u64).to_le_bytes());
            output.extend_from_slice(bytes);
            output
        }
        Err((tag, payload)) => {
            let mut output = vec![1];
            output.extend_from_slice(&tag.to_le_bytes());
            output.extend_from_slice(&payload.to_le_bytes());
            output
        }
    }
}
fn decode_stdin_completion(bytes: &[u8]) -> Result<Result<Vec<u8>, (u32, u32)>, Error> {
    if bytes.len() < 9 {
        return Err(invalid("truncated stdin tape completion"));
    }
    match bytes[0] {
        0 if u64::from_le_bytes(bytes[1..9].try_into().unwrap()) == (bytes.len() - 9) as u64 => {
            Ok(Ok(bytes[9..].to_vec()))
        }
        1 if bytes.len() == 9 => Ok(Err((
            u32::from_le_bytes(bytes[1..5].try_into().unwrap()),
            u32::from_le_bytes(bytes[5..9].try_into().unwrap()),
        ))),
        _ => Err(invalid("invalid stdin tape completion framing")),
    }
}

/// Print is a write of the String's UTF-8 bytes; byte writes preserve every byte.
pub(super) fn output_bytes<'a>(name: &str, value: &'a Value) -> Result<&'a [u8], Error> {
    if name.ends_with(".print") {
        text(value).map(str::as_bytes)
    } else {
        bytes(value)
    }
}

/// Exact deterministic work preflight, shared by reference and checked H.
pub(crate) fn numeric_work(
    path: &hirsel_syntax::DefPath,
    arguments: &[Value],
) -> Result<Option<u64>, Error> {
    if !path.0.package.package_is("core", "1") || path.0.modules != ["bytes"] {
        return Ok(None);
    }
    let cost = match path.0.name.as_str() {
        "json_number_end" => {
            hirsel_kernels::numeric::json_number(
                bytes(arg(arguments, 0)?)?,
                int(arg(arguments, 1)?)?.bits() as u64,
            )
            .1
        }
        "integer" => 1 + bytes(arg(arguments, 0)?)?.len() as u64,
        "decimal_word" => 9,
        _ => return Ok(None),
    };
    Ok(Some(cost))
}

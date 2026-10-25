//! MEM-6 histories derived from memory.md, not allocator sizes or runtime results.
use hirsel_driver::{compiler_adapters, native};
use hirsel_engine::*;
use hirsel_interp::observation::Effect;

const CORPUS: &str =
    include_str!("../../../../conformance/MEM/MEM-6-arena-effect-charge-17-run.hir");

#[derive(Clone, Copy, Debug)]
struct Budget {
    length: u64,
    shared: bool,
    max: u64,
    after: bool,
}
impl Budget {
    fn before_read(self) -> u64 {
        self.length + if self.shared { self.length } else { 0 } + self.max
    }
    fn read(self, cap: u64) -> bool {
        cap >= self.before_read()
    }
    fn fits(self, cap: u64) -> bool {
        cap >= self.before_read() + u64::from(self.after)
    }
}

fn vectors() -> Vec<(Budget, u64)> {
    let mut out = vec![];
    for length in [0, 2] {
        for shared in [false, true] {
            for max in [0, 1, 4] {
                for after in [false, true] {
                    let b = Budget {
                        length,
                        shared,
                        max,
                        after,
                    };
                    let required = b.before_read() + u64::from(after);
                    let mut caps = vec![0, required.saturating_sub(1), required, required + 1];
                    caps.sort_unstable();
                    caps.dedup();
                    out.extend(caps.into_iter().map(|cap| (b, cap)));
                }
            }
        }
    }
    out
}

fn file_matrix() -> (String, Vec<u64>) {
    let mut source = CORPUS[..CORPUS.find("pub fn exercise").unwrap()].to_owned();
    source.push_str("pub fn exercise(t: Test) -> Result[(), FileError] {\n");
    let mut requests = vec![];
    for data in ["", "AB"] {
        source.push_str(&format!(
            "  let d{} = test.dir(t, [(path.of(\"input\"), b\"{data}\")])\n",
            data.len()
        ));
        for (b, cap) in vectors() {
            let n = b.max.min(data.len() as u64);
            let consumed = if b.read(cap) { n } else { 0 };
            let rest = &data[consumed as usize..];
            source.push_str(&format!(
                "  probe(t, d{}, {cap}, {}, {}, {}, {}, {}, {n}, b\"{rest}\")?\n",
                data.len(),
                b.length,
                b.shared,
                b.max,
                b.after,
                b.fits(cap),
            ));
            if b.read(cap) {
                requests.push(b.max);
            }
            if b.fits(cap) {
                requests.push(8);
            }
        }
    }
    source.push_str("  bounds(t, d2)?\n  Ok(())\n}\n");
    // The bounds failure issued no read.
    (source, requests)
}

fn engine(source: &str) -> QueryEngine {
    let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
    let file = engine
        .sources
        .add("arena-effects.hir", source.to_owned())
        .unwrap();
    let checked = check_progress(&mut engine, file, &default_context()).unwrap();
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
    engine
}

fn events(
    observation: &hirsel_interp::observation::Observable,
) -> Vec<hirsel_tir::CapabilityEvent> {
    observation
        .effects
        .iter()
        .filter_map(|e| match &e.effect {
            Effect::CapabilityConsumed(input) => Some(input.clone()),
            _ => None,
        })
        .collect()
}

// The tape's documented value encoding: the final argument is a u64 atom
// (tag, width, signed, little-endian u128). No production decoder computes an oracle.
fn max_argument(event: &hirsel_tir::CapabilityEvent) -> u64 {
    let atom = &event.input[event.input.len() - 19..];
    assert_eq!(&atom[..3], &[2, 64, 0]);
    u128::from_le_bytes(atom[3..].try_into().unwrap())
        .try_into()
        .unwrap()
}

#[test]
fn file_histories_and_offline_replay() {
    with_compiler_stack(|| {
        let (source, requests) = file_matrix();
        let mut engine = engine(&source);
        let mut entry = entry_path(&default_context());
        entry.0.name = "exercise".into();
        let run = RunConfiguration {
            entry: entry.clone(),
            hash_seed: 0,
            inputs: vec![],
        };
        let observed =
            run_observed_snapshot(&mut engine, &run, SimulationConfig::default()).unwrap();
        let observation = observed.observation.as_ref().unwrap();
        let reference = native::Observation::of_reference(observation);
        assert_eq!(reference.exit_code, Some(0), "{:?}", observed.outcome);
        assert!(reference.output.is_empty(), "{:?}", reference.output);
        assert!(reference.failures.is_empty());
        let tape = events(observation);
        assert_eq!(
            tape.iter()
                .filter(|e| e.operation == "file.read")
                .map(max_argument)
                .collect::<Vec<_>>(),
            requests
        );
        let replay = run_observed_snapshot(
            &mut engine,
            &RunConfiguration {
                inputs: tape,
                ..run
            },
            SimulationConfig::default(),
        )
        .unwrap();
        assert_eq!(replay.observation, observed.observation);

        let input = engine.export_native_input().unwrap();
        let sites = native::Sites::new(&input);
        let validated = engine.import_native_input(input.clone()).unwrap();
        let target = native::TargetSpec::x86_64_unknown_linux_gnu();
        let ready = native::build(&validated, &native::Options::new(target.clone())).unwrap();
        let heap = native::run_heap(&ready, &entry, &sites).unwrap();
        assert_eq!(heap.observation, reference);
        assert_eq!(heap.counters.live_heap_bytes, 0);
        let root = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("arena-effect-charge-17");
        let tools =
            native::Toolchain::in_tree_diagnostic(&root.join("runtime"), &[&target]).unwrap();
        for backend in [native::Backend::Llvm, native::Backend::Cranelift] {
            let dir = root.join(format!("{backend:?}"));
            std::fs::create_dir_all(&dir).unwrap();
            let objects = native::emit_backend_objects(
                &ready,
                backend,
                native::OptLevel::Speed,
                native::Partition::Whole,
                false,
                None,
            )
            .unwrap();
            let exe = native::link_objects(&ready, &entry, &objects, &tools, &dir).unwrap();
            let tape = dir.join("io-tape");
            let _ = std::fs::remove_file(&tape);
            let live = native::launch_with(
                &exe,
                &target,
                &tools,
                &dir,
                true,
                native::Tape::Live {
                    record: Some(&tape),
                    revocations: None,
                },
            )
            .unwrap();
            assert_eq!(native::observe(&live, &ready, &sites).unwrap(), reference);
            let record = hirsel_rt::launch::parse_recording(
                &hirsel_rt::launch::read_recording_text(&tape).unwrap(),
            )
            .unwrap();
            let actual: Vec<_> = record
                .events
                .iter()
                .filter_map(|e| match e.request {
                    hirsel_rt::io::Request::FileRead { max, .. } => Some(max),
                    _ => None,
                })
                .collect();
            assert_eq!(actual, requests, "{backend:?}");
            let replay = native::launch_with(
                &exe,
                &target,
                &tools,
                &dir,
                true,
                native::Tape::Replay(&tape),
            )
            .unwrap();
            assert_eq!(native::observe(&replay, &ready, &sites).unwrap(), reference);
        }
    });
}

const STDIN: &str = r#"

pub fn main(rt: Io, args: Args) -> Result[(), GrantError] {
  let stdin = io.stdin(rt)?
  let stdout = io.stdout(rt)?
  let length = if arg.text(args, 0) == Ok("two") { 2u64 } else { 0u64 }
  let shared = arg.text(args, 1) == Ok("shared")
  let max = if arg.text(args, 2) == Ok("four") { 4u64 }
            else if arg.text(args, 2) == Ok("one") { 1u64 } else { 0u64 }
  let after = arg.text(args, 3) == Ok("push")
  let cap = arg.len(args) - 4u64
  let status = arena.bounded(cap, fn(a: Arena) -> u64 {
    let mut buffer: List[u8] in a = []
    let mut i = 0u64
    while i < length { list.push(mut buffer, 11u8); i = i + 1 }
    if shared {
      let alias = copy buffer
      let got = stdin.read(stdin, mut buffer, max)
      failure.check(if max == 0 { got == Error(InvalidInput()) }
                    else { match got { Ok(n) => list.len(buffer) == length + n, Error(_) => false } }, Bounds())
      failure.check(list.len(alias) == length, Bounds())
      if after { list.push(mut buffer, 22u8) }
    } else {
      let got = stdin.read(stdin, mut buffer, max)
      failure.check(if max == 0 { got == Error(InvalidInput()) }
                    else { match got { Ok(n) => list.len(buffer) == length + n, Error(_) => false } }, Bounds())
      if after { list.push(mut buffer, 22u8) }
    }
    list.len(buffer)
  })
  match status {
    Ok(n) => { let _ = out.print(stdout, "ok:"); let _ = out.print(stdout, match n { 0 => "0", 1 => "1", 2 => "2", 3 => "3", 4 => "4", 5 => "5", _ => "bad" }); let _ = out.print(stdout, ";") },
    Error(_) => { let _ = out.print(stdout, "space;") },
  }
  let mut tail: List[u8] = []
  let _ = stdin.read(stdin, mut tail, 8u64)
  let _ = out.write(stdout, bytes.from_list(take tail))
  Ok(())
}
"#;

#[test]
fn stdin_histories_and_offline_replay() {
    with_compiler_stack(|| {
        let mut engine = engine(STDIN);
        let run = RunConfiguration {
            entry: entry_path(&default_context()),
            hash_seed: 0,
            inputs: vec![],
        };
        let mut cases = vec![];
        for data in [b"".as_slice(), b"AB".as_slice()] {
            for (b, cap) in vectors() {
                let mut args = vec![
                    if b.length == 2 { "two" } else { "empty" }.to_owned(),
                    if b.shared { "shared" } else { "unshared" }.to_owned(),
                    match b.max {
                        4 => "four",
                        1 => "one",
                        _ => "zero",
                    }
                    .to_owned(),
                    if b.after { "push" } else { "none" }.to_owned(),
                ];
                args.extend((0..cap).map(|_| "cap".to_owned()));
                let config = SimulationConfig {
                    args,
                    stdin: data.to_vec(),
                    ..Default::default()
                };
                let observed = run_observed_snapshot(&mut engine, &run, config.clone()).unwrap();
                let observation = observed.observation.as_ref().unwrap();
                assert_eq!(
                    observation.exit_code,
                    Some(0),
                    "{b:?} cap={cap}: {:?}",
                    observed.outcome
                );
                let mut expected = vec![];
                let n = b.max.min(data.len() as u64);
                expected.extend(
                    if b.fits(cap) {
                        format!("ok:{};", b.length + n + u64::from(b.after))
                    } else {
                        "space;".into()
                    }
                    .bytes(),
                );
                expected.extend_from_slice(&data[if b.read(cap) { n as usize } else { 0 }..]);
                assert_eq!(observation.output, expected, "{b:?} cap={cap}");
                let tape = events(observation);
                let requests: Vec<_> = tape
                    .iter()
                    .filter(|e| e.operation == "stdin.read")
                    .map(max_argument)
                    .collect();
                let expected_requests = if b.read(cap) { vec![b.max, 8] } else { vec![8] };
                assert_eq!(requests, expected_requests, "{b:?} cap={cap}");
                let replay_config = SimulationConfig {
                    stdin: vec![],
                    ..config.clone()
                };
                let replay = run_observed_snapshot(
                    &mut engine,
                    &RunConfiguration {
                        inputs: tape,
                        ..run.clone()
                    },
                    replay_config,
                )
                .unwrap();
                assert_eq!(replay.observation, observed.observation);
                cases.push((
                    data.to_vec(),
                    config.args,
                    native::Observation::of_reference(observation),
                    expected_requests,
                ));
            }
        }
        let input = engine.export_native_input().unwrap();
        let sites = native::Sites::new(&input);
        let validated = engine.import_native_input(input.clone()).unwrap();
        let target = native::TargetSpec::x86_64_unknown_linux_gnu();
        let ready = native::build(&validated, &native::Options::new(target.clone())).unwrap();
        // The production H fixture has EOF-only stdin. Supply a small
        // explicit scripted provider through H's existing Operations seam.
        let heap = ready.heap_program();
        let heap_config = hirsel_heapcheck::Config {
            context_handles: vec![(1, "Io".into()), (0, "Args".into())],
            ..Default::default()
        };
        for (data, args, expected, requests) in &cases {
            let mut script = StdinScript {
                data: data.clone(),
                args: args.clone(),
                position: 0,
                requests: vec![],
            };
            let execution = hirsel_heapcheck::run_ready(
                &heap,
                &run.entry,
                &[
                    hirsel_heapcheck::Arg::Handle(1),
                    hirsel_heapcheck::Arg::Handle(0),
                ],
                &mut script,
                &heap_config,
            );
            let audit = execution.result.unwrap();
            assert!(audit.observation.returned.is_some());
            assert!(audit.observation.failures.is_empty());
            assert!(!audit.observation.blocked);
            assert_eq!(execution.output, expected.output);
            assert_eq!(&script.requests, requests);
            assert_eq!(execution.counters.live_heap_bytes, 0);
        }
        let root =
            std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("arena-effect-charge-17-stdin");
        let tools =
            native::Toolchain::in_tree_diagnostic(&root.join("runtime"), &[&target]).unwrap();
        for backend in [native::Backend::Llvm, native::Backend::Cranelift] {
            let dir = root.join(format!("{backend:?}"));
            std::fs::create_dir_all(dir.join("root")).unwrap();
            let objects = native::emit_backend_objects(
                &ready,
                backend,
                native::OptLevel::Speed,
                native::Partition::Whole,
                false,
                None,
            )
            .unwrap();
            let exe = native::link_standalone_objects(&ready, &run.entry, &objects, &tools, &dir)
                .unwrap();
            let grants = dir.join("grants.json");
            std::fs::write(&grants, r#"{"io":{"stdin":"stdin.bytes@1","stdout":"stdout.bytes@1","tasks":"tasks.cooperative@1","dirs":[]},"args":[]}"#).unwrap();
            for (index, (data, args, expected, requests)) in cases.iter().enumerate() {
                let tape = dir.join(format!("stdin-{index}.tape"));
                let record = dir.join("observation");
                for replay in [false, true] {
                    use std::io::Write;
                    use std::process::{Command, Stdio};
                    let _ = std::fs::remove_file(&record);
                    let mut command = Command::new(&exe);
                    command
                        .env_clear()
                        .env("HIRSEL_RT_ROOT", dir.join("root"))
                        .env("HIRSEL_RT_DIAGNOSTIC", "1")
                        .env("HIRSEL_RT_OBSERVATION", &record)
                        .stdout(Stdio::piped())
                        .stderr(Stdio::piped());
                    if replay {
                        command
                            .arg("--hirsel-replay")
                            .arg(&tape)
                            .stdin(Stdio::null());
                    } else {
                        let _ = std::fs::remove_file(&tape);
                        command
                            .arg("--hirsel-record")
                            .arg(&tape)
                            .arg("--hirsel-grants")
                            .arg(&grants)
                            .args(args)
                            .stdin(Stdio::piped());
                    }
                    let mut child = command.spawn().unwrap();
                    if let Some(mut stdin) = child.stdin.take() {
                        stdin.write_all(data).unwrap();
                    }
                    let output = child.wait_with_output().unwrap();
                    let launched = native::Launched {
                        audit_required: true,
                        exit: output.status.code(),
                        signal: None,
                        stdout: output.stdout,
                        stderr: output.stderr,
                        record: std::fs::read_to_string(&record).unwrap(),
                    };
                    assert_eq!(
                        native::observe(&launched, &ready, &sites).unwrap(),
                        *expected,
                        "{backend:?} case={index} replay={replay}"
                    );
                }
                let recording = hirsel_rt::launch::parse_recording(
                    &hirsel_rt::launch::read_recording_text(&tape).unwrap(),
                )
                .unwrap();
                let actual: Vec<_> = recording
                    .events
                    .iter()
                    .filter(|e| !matches!(e.completion, hirsel_rt::io::Completion::Blocked { .. }))
                    .filter_map(|e| match e.request {
                        hirsel_rt::io::Request::ReadStdin { max, .. } => Some(max),
                        _ => None,
                    })
                    .collect();
                assert_eq!(&actual, requests, "{backend:?} case={index}");
            }
        }
    });
}

/// Port of arena_lists.rs's descriptor/setup and share/detach controls. This
/// adds the reserve history and inspects the retained alias before arena exit;
/// the source-language tests separately check effects and input cursors.
#[test]
fn reserve_retains_aliases_on_refusal_and_releases_the_arena() {
    use hirsel_rt::context::{
        hirsel_rt_v2_ctx_create, hirsel_rt_v2_ctx_destroy, hirsel_rt_v2_task_exit,
    };
    use hirsel_rt::region::*;
    use hirsel_rt::*;
    use std::{mem::MaybeUninit, ptr};
    let layout = Layout {
        magic: LAYOUT_MAGIC,
        version: LAYOUT_VERSION,
        size: 16,
        align: 8,
        finalizer: None,
        steps: ptr::null(),
        step_count: 0,
        field_count: 0,
        flags: LAYOUT_DYNAMIC | LAYOUT_LOGICAL_PER_ELEMENT | LAYOUT_SEQUENCE,
        logical: 1,
        logical_count: ptr::null(),
        scan: ptr::null(),
        scan_words: u64::MAX,
    };
    let layouts = [&layout as *const Layout];
    for (b, cap) in vectors().into_iter().filter(|(b, _)| !b.after) {
        // Initial construction can itself exhaust. Source tests cover that
        // unwind; here a live source is required to inspect the alias.
        if cap < b.length {
            continue;
        }
        let config = Config {
            layouts: layouts.as_ptr(),
            layout_count: 1,
            ..Config::new()
        };
        let mut ctx = ptr::null_mut();
        // SAFETY: each context/arena/descriptor outlives its blocks. Payload
        // writes stay within the room returned by arena_reserve. Every arena
        // exits before task audit and context destruction.
        unsafe {
            assert_eq!(hirsel_rt_v2_ctx_create(&config, &mut ctx), 0);
            let mut arena = MaybeUninit::<Region>::uninit();
            hirsel_rt_v2_arena_enter(ctx, arena.as_mut_ptr());
            hirsel_rt_v2_arena_bound(ctx, arena.as_mut_ptr(), cap);
            // User descriptors follow the runtime's two built-in descriptors.
            let mut block =
                hirsel_rt_v2_arena_alloc(ctx, arena.as_mut_ptr(), 2, 16, 8, ALLOC_TRACKED);
            let payload = hirsel_rt::alloc::hirsel_rt_v2_payload(block);
            payload.cast::<u64>().write(0);
            payload.add(8).cast::<u64>().write(0);
            assert_eq!(
                hirsel_rt_v2_arena_reserve(ctx, block, b.length, 1, 1, &mut block),
                0
            );
            let payload = hirsel_rt::alloc::hirsel_rt_v2_payload(block);
            payload.cast::<u64>().write(b.length);
            for i in 0..b.length {
                payload.add(16 + i as usize).write(11);
            }
            let original = block;
            if b.shared {
                hirsel_rt::count::hirsel_rt_v2_dup(ctx, block);
            }
            let before = (*arena.as_ptr()).remaining;
            let status = hirsel_rt_v2_arena_reserve(ctx, block, b.max, 1, 1, &mut block);
            assert_eq!(status == 0, b.read(cap), "{b:?} cap={cap}");
            let charge = b.max + if b.shared { b.length } else { 0 };
            assert_eq!(
                (*arena.as_ptr()).remaining,
                if status == 0 { before - charge } else { before }
            );
            if status != 0 {
                assert_eq!(block, original);
            } else if b.shared {
                assert_ne!(block, original);
                assert_eq!((*block).count, COUNT_ARENA_UNIQUE);
            }
            let retained = if b.shared || status != 0 {
                original
            } else {
                block
            };
            let alias = hirsel_rt::alloc::hirsel_rt_v2_payload(retained);
            assert_eq!(alias.cast::<u64>().read(), b.length);
            for i in 0..b.length {
                assert_eq!(alias.add(16 + i as usize).read(), 11);
            }
            assert_eq!(hirsel_rt_v2_region_exit(ctx, arena.as_mut_ptr(), 0, 0), 0);
            let mut audit = Audit::default();
            hirsel_rt_v2_task_exit(ctx, &mut audit);
            assert_eq!(
                (
                    audit.live_blocks,
                    audit.active_regions,
                    audit.retained_bytes
                ),
                (0, 0, 0)
            );
            hirsel_rt_v2_ctx_destroy(ctx);
        }
    }
}

/// The script owns only bytes/argv. H still owns and audits every buffer,
/// copy, charge, failure edge and cleanup. Structure follows native/heap.rs's
/// existing EOF provider, extended with a cursor and exact request history.
struct StdinScript {
    data: Vec<u8>,
    args: Vec<String>,
    position: usize,
    requests: Vec<u64>,
}
impl hirsel_heapcheck::Operations for StdinScript {
    fn call(
        &mut self,
        op: &hirsel_core_contract::OperationId,
        args: &[hirsel_heapcheck::OpValue],
        output: &mut Vec<u8>,
    ) -> hirsel_heapcheck::OpOutcome {
        use hirsel_heapcheck::{OpOutcome as O, OpValue as V};
        use hirsel_lir::layout_types::ScalarKind;
        let ok = |value| V::Variant {
            tag: 0,
            value: Box::new(value),
        };
        let error = |tag| V::Variant {
            tag: 1,
            value: Box::new(V::Variant {
                tag,
                value: Box::new(V::Unit),
            }),
        };
        let number = |n: usize| V::Bits {
            scalar: ScalarKind::U64,
            bits: n as u128,
        };
        let path = &op.definition.0;
        assert!(path.package.name_is("core"));
        let value = match (
            path.modules.first().map(String::as_str),
            path.name.as_str(),
            args,
        ) {
            (Some("io"), "stdin", [V::Handle(1)]) => ok(V::Handle(2)),
            (Some("io"), "stdout", [V::Handle(1)]) => ok(V::Handle(3)),
            (Some("arg"), "len", [V::Handle(0)]) => number(self.args.len()),
            (Some("arg"), "text", [V::Handle(0), V::Bits { bits, .. }]) => {
                self.args.get(*bits as usize).map_or_else(
                    || error(0),
                    |arg| ok(V::ByteSequence(arg.as_bytes().to_vec())),
                )
            }
            (Some("out"), "print" | "write", [V::Handle(3), V::Bytes(bytes)]) => {
                output.extend_from_slice(bytes);
                ok(V::Unit)
            }
            (
                Some("stdin"),
                "read",
                [
                    V::Handle(2),
                    V::Elements {
                        scalar: ScalarKind::U8,
                        bits,
                    },
                    V::Bits { bits: max, .. },
                ],
            ) => {
                self.requests.push(*max as u64);
                if *max == 0 {
                    return O::Value(error(2));
                }
                let n = (*max as usize).min(self.data.len() - self.position);
                let mut appended = bits.clone();
                appended.extend(
                    self.data[self.position..self.position + n]
                        .iter()
                        .map(|b| u128::from(*b)),
                );
                self.position += n;
                return O::Updated {
                    value: ok(number(n)),
                    arguments: vec![(
                        1,
                        V::Elements {
                            scalar: ScalarKind::U8,
                            bits: appended,
                        },
                    )],
                };
            }
            _ => panic!("unexpected scripted operation {op:?}: {args:?}"),
        };
        O::Value(value)
    }
}

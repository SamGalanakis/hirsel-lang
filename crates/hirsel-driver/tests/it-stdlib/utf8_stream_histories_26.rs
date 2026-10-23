//! Bounded decoder histories, independent of utf8.hir's scalar scanner.
//! Rust f6c1107f8be13e48465e12c0fd26e6a39fe18e4f validations.rs:146-219
//! supplies valid_up_to/error_len; Go ebe2feefc400f813aefdc8a036e66d2619da6649
//! utf8_test.go:31-107 supplies scalar identities (existing pinned table).
//! New coverage: per-action state, copied pending prefixes, divergent terminals.
//! Existing vector/overflow/ownership controls remain in utf8_stream_5.
use hirsel_driver::*;
use hirsel_engine::*;
use std::fmt::Write;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug)]
struct Model {
    lossy: bool,
    consumed: usize,
    pending: Vec<u8>,
    status: &'static str,
}

impl Model {
    fn new(lossy: bool) -> Self {
        Self {
            lossy,
            consumed: 0,
            pending: vec![],
            status: "Active",
        }
    }

    fn feed(&mut self, chunk: &[u8], eof: bool) -> Result<String, String> {
        match self.status {
            "Finished" => return Err("AlreadyFinished()".into()),
            "Failed" => return Err("DecoderFailed()".into()),
            _ => {}
        }
        let base = self.consumed - self.pending.len();
        let mut input = std::mem::take(&mut self.pending);
        input.extend_from_slice(chunk);
        self.consumed += chunk.len();
        let mut cursor = 0;
        let mut output = String::new();
        loop {
            match std::str::from_utf8(&input[cursor..]) {
                Ok(valid) => {
                    output.push_str(valid);
                    break;
                }
                Err(error) => {
                    let valid_end = cursor + error.valid_up_to();
                    output.push_str(std::str::from_utf8(&input[cursor..valid_end]).unwrap());
                    match error.error_len() {
                        None if !eof => {
                            self.pending.extend_from_slice(&input[valid_end..]);
                            break;
                        }
                        span if !self.lossy => {
                            self.status = "Failed";
                            if let Some(span) = span {
                                self.consumed = base + valid_end + span;
                            }
                            return Err(format!(
                                "{}Sequence({})",
                                if span.is_some() {
                                    "Invalid"
                                } else {
                                    "Incomplete"
                                },
                                base + valid_end
                            ));
                        }
                        span => {
                            output.push('\u{fffd}');
                            cursor = valid_end + span.unwrap_or(input.len() - valid_end);
                        }
                    }
                }
            }
        }
        assert!(self.pending.len() <= 3);
        assert!(std::str::from_utf8(output.as_bytes()).is_ok());
        if eof {
            self.status = "Finished";
        }
        Ok(output)
    }
}

#[derive(Clone, Debug)]
enum Action {
    Feed(usize, Vec<u8>, bool),
    Copy,
}

#[derive(Debug)]
struct History {
    id: String,
    lossy: bool,
    actions: Vec<Action>,
}

fn feed(branch: usize, bytes: &[u8], eof: bool) -> Action {
    Action::Feed(branch, bytes.to_vec(), eof)
}

fn table() -> Vec<(String, Vec<u8>)> {
    include_str!("../fixtures/utf8-stream-5-go-vectors.tsv")
        .lines()
        .filter(|line| !line.starts_with('#'))
        .map(|line| {
            let (id, encoded) = line.split_once('\t').unwrap();
            let bytes: Vec<_> = encoded
                .split("\\x")
                .skip(1)
                .map(|hex| u8::from_str_radix(hex, 16).unwrap())
                .collect();
            if id != "invalid" {
                let scalar = u32::from_str_radix(id.trim_start_matches("0x"), 16).unwrap();
                if let Some(c) = char::from_u32(scalar) {
                    assert_eq!(c.to_string().as_bytes(), bytes);
                } else {
                    assert!(std::str::from_utf8(&bytes).is_err());
                }
            } else {
                assert!(std::str::from_utf8(&bytes).is_err());
            }
            (id.to_owned(), bytes)
        })
        .collect()
}

fn histories() -> Vec<History> {
    let table = table();
    assert_eq!(table.len(), 71);
    // Stable IDs from the pinned table, not newly named byte vectors.
    let scalars = [
        ["0x0000", "0x0001", "0x007e", "0x007f"],
        ["0x0080", "0x00bf", "0x0100", "0x07ff"],
        ["0x0800", "0xd7ff", "0xe000", "0xffff"],
        ["0x10000", "0x40000", "0x10fffe", "0x10ffff"],
    ];
    let mut templates = Vec::new();
    for (pending, ids) in scalars.iter().enumerate() {
        for (variant, &id) in ids.iter().enumerate() {
            let bytes = &table.iter().find(|(key, _)| key == id).unwrap().1;
            let mut prefix = vec![b'A'];
            prefix.extend(&bytes[..pending]);
            // One copy completes. Its sibling fails at EOF or rejects ASCII,
            // exercising maximal invalid spans and different final offsets.
            let alternate: &[u8] = if variant % 2 == 0 { b"" } else { b"Z" };
            templates.push((
                format!("go:{id}/copy-p{pending}/v{variant}"),
                vec![
                    feed(0, &prefix, false),
                    Action::Copy,
                    feed(0, &bytes[pending..], true),
                    feed(1, alternate, true),
                    feed(0, b"!", false),
                    feed(1, b"", true),
                ],
            ));
        }
    }
    // Rust allo tests str.rs:881-900: retain the original source line IDs.
    // Each already-covered vector gains per-feed offsets and terminal controls.
    for (line, bytes) in [
        (881, vec![0xc0, 0x80]),
        (883, vec![0xe0, 0x80, 0x80]),
        (885, vec![0xe0, 0x81, 0x81]),
        (886, vec![0xf0, 0x82, 0x82, 0xac]),
        (887, vec![0xf4, 0x90, 0x80, 0x80]),
        (890, vec![0xed, 0xa0, 0x80]),
        (891, vec![0xed, 0xbf, 0xbf]),
        (899, vec![0xf0, 0x90, 0x80, 0x80]),
    ] {
        let cut = bytes.len() - 1;
        templates.push((
            format!("rust:str.rs:{line}/cut{cut}"),
            vec![
                feed(0, b"A", false),
                feed(0, &bytes[..cut], false),
                feed(0, b"", false),
                feed(0, &bytes[cut..], true),
                feed(0, b"", true),
                feed(0, b"!", false),
            ],
        ));
    }
    // 24 bounded valid strings assembled from pinned Go scalars. The seed and
    // lane suffix are reproduction metadata, not an exhaustive proof.
    let mut seed = 20_261_026u64 ^ 0x7574_6638_6869_7374;
    let valid: Vec<_> = table
        .iter()
        .filter(|(_, b)| std::str::from_utf8(b).is_ok())
        .collect();
    for index in 0..24 {
        let mut bytes = Vec::new();
        for _ in 0..(2 + index % 3) {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            bytes.extend(&valid[seed as usize % valid.len()].1);
        }
        assert!(bytes.len() <= 16);
        let cut = (seed as usize) % (bytes.len() + 1);
        let end = if index % 3 == 0 {
            bytes.len().saturating_sub(1)
        } else {
            bytes.len()
        };
        let cut = cut.min(end);
        templates.push((
            format!("generated:{index:02}/cut{cut}/end{end}"),
            vec![
                feed(0, &bytes[..cut], false),
                feed(0, b"", false),
                feed(0, &bytes[cut..end], true),
                feed(0, b"", true),
                feed(0, b"!", false),
                feed(0, b"", false),
            ],
        ));
    }
    assert_eq!(templates.len(), 48);
    let mut histories = Vec::new();
    for (id, actions) in templates {
        assert!(actions.len() <= 6);
        let bytes: usize = actions
            .iter()
            .map(|a| match a {
                Action::Feed(_, b, _) => b.len(),
                Action::Copy => 0,
            })
            .sum();
        assert!(bytes <= 16, "{id}: {bytes}");
        for lossy in [false, true] {
            histories.push(History {
                id: format!("{id}/{}", if lossy { "Lossy" } else { "Strict" }),
                lossy,
                actions: actions.clone(),
            });
        }
    }
    assert_eq!(histories.len(), 96);
    histories
}

fn literal(bytes: &[u8], text: bool) -> String {
    if text {
        let text = std::str::from_utf8(bytes).unwrap();
        let escaped: String = text
            .chars()
            .map(|c| format!("\\u{{{:x}}}", u32::from(c)))
            .collect();
        format!("\"{escaped}\"")
    } else {
        let escaped: String = bytes.iter().map(|b| format!("\\x{b:02x}")).collect();
        format!("b\"{escaped}\"")
    }
}

fn state(source: &mut String, branch: usize, model: &Model) {
    for (name, expected) in [
        ("offset", model.consumed.to_string()),
        ("pending_bytes", model.pending.len().to_string()),
        ("status", format!("{}()", model.status)),
    ] {
        writeln!(
            source,
            "failure.check(std@1/utf8.{name}(d{branch}) == {expected}, TextInvariant())"
        )
        .unwrap();
    }
}

fn program(batch: usize) -> String {
    let histories = histories();
    let mut source = String::from("pub fn main() -> Result[(), ()] {\n");
    for index in 0..8 {
        writeln!(source, "history{index}()\n").unwrap();
    }
    source += "Ok(())\n}\n";
    for (index, history) in histories[batch * 8..batch * 8 + 8].iter().enumerate() {
        writeln!(source, "// {}\nfn history{index}() -> () {{", history.id).unwrap();
        writeln!(
            source,
            "let mut d0 = std@1/utf8.decoder({}())",
            if history.lossy { "Lossy" } else { "Strict" }
        )
        .unwrap();
        let mut models = vec![Model::new(history.lossy)];
        state(&mut source, 0, &models[0]);
        for (action_index, action) in history.actions.iter().enumerate() {
            writeln!(source, "// action {action_index}: {action:?}").unwrap();
            match action {
                Action::Copy => {
                    source += "let mut d1 = copy d0\n";
                    models.push(models[0].clone());
                }
                Action::Feed(branch, bytes, eof) => {
                    let expected = match models[*branch].feed(bytes, *eof) {
                        Ok(text) => format!("Ok({})", literal(text.as_bytes(), true)),
                        Err(error) => format!("Error({error})"),
                    };
                    writeln!(source, "failure.check(std@1/utf8.feed(mut d{branch}, {}, {eof}) == {expected}, TextInvariant())", literal(bytes, false)).unwrap();
                }
            }
            // Observe the untouched sibling too; aliasing copies cannot hide
            // behind a correct active branch's returned text.
            for (branch, model) in models.iter().enumerate() {
                state(&mut source, branch, model);
            }
        }
        source += "}\n";
    }
    source
}

// Runtime adapters: every engine compares to host-generated assertions. Native
// observations are constructed here, never derived from interpreter output.
fn execute(batch: usize, engine: &str) {
    with_compiler_stack(|| {
        let source = program(batch);
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/std_source/main.hir");
        for h in &histories()[batch * 8..batch * 8 + 8] {
            eprintln!("selected started {engine} {}", h.id);
        }
        let mut loaded = load_program_source(&path, Some(&source)).unwrap();
        loaded.check_for_execution().unwrap_or_else(|e| {
            panic!(
                "batch {batch}: {}",
                format!("{e:?}").chars().take(2000).collect::<String>()
            )
        });
        let config = RunConfiguration {
            entry: loaded.entry.clone(),
            hash_seed: 0,
            inputs: vec![],
        };
        let expected = native::Observation {
            exit_code: Some(0),
            blocked: false,
            budget: None,
            signaled: None,
            output: vec![],
            failures: vec![],
            roots: vec![],
        };
        if engine == "interpreter" {
            loaded.engine.adapters.interpreter = Some(Box::new(InterpreterAdapter {
                limits: hirsel_interp::Limits {
                    steps: Some(1_000_000),
                    call_depth: 256,
                },
                ..Default::default()
            }));
            let run =
                run_observed_snapshot(&mut loaded.engine, &config, SimulationConfig::default())
                    .unwrap();
            assert!(
                matches!(
                    run.outcome,
                    Ok(RunOutcome::Returned(hirsel_run_schema::Returned::Ok))
                ),
                "{:?}",
                run.outcome
            );
            assert_eq!(
                native::Observation::of_reference(run.observation.as_ref().unwrap()),
                expected
            );
        } else {
            let input = loaded.engine.export_native_input().unwrap();
            let validated = loaded.engine.import_native_input(input).unwrap();
            let options = native::Options::new(native::TargetSpec::x86_64_unknown_linux_gnu());
            if engine == "heap" {
                let ready = native::build(&validated, &options).unwrap();
                let world = native::HeapWorld {
                    steps: Some(1_000_000),
                    ..Default::default()
                };
                let run = native::run_heap_in(
                    &ready,
                    &loaded.entry,
                    &native::Sites::new(validated.snapshot()),
                    &world,
                )
                .unwrap();
                assert_eq!(run.observation, expected);
                assert_eq!(run.counters.live_heap_bytes, 0);
            } else {
                let configuration = native::Configuration {
                    options,
                    engine: if engine == "llvm" {
                        native::Engine::Native {
                            opt: native::OptLevel::Speed,
                            partition: native::Partition::Whole,
                            runtime: native::Runtime::Diagnostic,
                        }
                    } else {
                        assert_eq!(engine, "cranelift");
                        native::Engine::Cranelift {
                            partition: native::Partition::Whole,
                            runtime: native::Runtime::Diagnostic,
                        }
                    },
                };
                let dir = tempfile::Builder::new()
                    .prefix("utf8-stream-histories-26-")
                    .tempdir_in(PathBuf::from(env!("CARGO_TARGET_TMPDIR")))
                    .unwrap();
                let configs = [configuration];
                let tools =
                    native::Toolchain::for_matrix(&dir.path().join("runtime"), &configs).unwrap();
                let mut count = 0;
                for (config, outcome) in native::differential(
                    &validated,
                    &loaded.entry,
                    &expected,
                    &configs,
                    &tools,
                    dir.path(),
                ) {
                    assert!(
                        matches!(outcome, native::Outcome::Agree),
                        "{config}: {outcome:?}"
                    );
                    count += 1;
                }
                assert_eq!(count, 1);
            }
        }
        for h in &histories()[batch * 8..batch * 8 + 8] {
            eprintln!("completed pass {engine} {}", h.id);
        }
    });
}

macro_rules! partition {
    ($batch:literal, $interp:ident, $heap:ident, $llvm:ident, $cranelift:ident) => {
        #[test]
        fn $interp() {
            execute($batch, "interpreter");
        }
        #[test]
        fn $heap() {
            execute($batch, "heap");
        }
        #[test]
        fn $llvm() {
            execute($batch, "llvm");
        }
        #[test]
        fn $cranelift() {
            execute($batch, "cranelift");
        }
    };
}
partition!(0, p00_interpreter, p00_heap, p00_llvm, p00_cranelift);
partition!(1, p01_interpreter, p01_heap, p01_llvm, p01_cranelift);
partition!(2, p02_interpreter, p02_heap, p02_llvm, p02_cranelift);
partition!(3, p03_interpreter, p03_heap, p03_llvm, p03_cranelift);
partition!(4, p04_interpreter, p04_heap, p04_llvm, p04_cranelift);
partition!(5, p05_interpreter, p05_heap, p05_llvm, p05_cranelift);
partition!(6, p06_interpreter, p06_heap, p06_llvm, p06_cranelift);
partition!(7, p07_interpreter, p07_heap, p07_llvm, p07_cranelift);
partition!(8, p08_interpreter, p08_heap, p08_llvm, p08_cranelift);
partition!(9, p09_interpreter, p09_heap, p09_llvm, p09_cranelift);
partition!(10, p10_interpreter, p10_heap, p10_llvm, p10_cranelift);
partition!(11, p11_interpreter, p11_heap, p11_llvm, p11_cranelift);

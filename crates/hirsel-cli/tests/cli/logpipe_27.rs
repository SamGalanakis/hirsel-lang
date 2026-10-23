//! Three real stages on the reference interpreter, checked H and both native adapters.
use hirsel_driver::native::{
    Configuration, Engine, Fixture, Observation, OptLevel, Options, Outcome, Partition, Runtime,
    TargetSpec, Toolchain, differential_in,
};
use serde_json::Value;
use std::{collections::BTreeMap, fs, path::PathBuf};

const VECTORS: &str = include_str!("../support/logpipe_27/vectors.json");
const APP: &str = include_str!("../../../../examples/logpipe/main.hir");
const DRIVER: &str = include_str!("../support/logpipe_27/driver.hir");

fn input(v: &Value) -> Vec<u8> {
    if v["generated"] == "output-cap" {
        return format!("hit{}\n", "a".repeat(4092)).repeat(64).into_bytes();
    }
    if let Some(n) = v["repeat"].as_u64() {
        return vec![b'a'; n as usize];
    }
    if let Some(hex) = v["hex"].as_str() {
        return hex
            .as_bytes()
            .chunks_exact(2)
            .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
            .collect();
    }
    v["input"].as_str().unwrap().as_bytes().to_vec()
}

// Independent sequential byte model. Literal small-case expectations are also
// checked, so agreement between engines alone cannot bless a changed policy.
fn sequential(data: &[u8], stop: u64) -> (Vec<u8>, &'static str) {
    if stop == 0 {
        return (vec![], "");
    }
    if data.len() > 262144 {
        return (vec![], "InputCap");
    }
    let mut output = vec![];
    let mut matches = 0;
    for (index, raw) in data.split_inclusive(|&b| b == b'\n').enumerate() {
        let row = raw
            .strip_suffix(b"\n")
            .map_or(raw, |s| s.strip_suffix(b"\r").unwrap_or(s));
        if row.len() > 4096 {
            return (output, "LineCap");
        }
        let Ok(text) = std::str::from_utf8(row) else {
            return (output, "Utf8");
        };
        if text.contains("hit") {
            let line = format!("{}: {text}\n", index + 1);
            if output.len() + line.len() > 262144 {
                return (output, "OutputCap");
            }
            output.extend(line.bytes());
            matches += 1;
            if matches == stop {
                break;
            }
        }
    }
    (output, "")
}

// FIFO reserve/disarm/drain vectors: Tokio ce685ee tests/sync_mpsc.rs:48-109.
// The independent ledger retains the reservation across transfer and sink hold.
fn check_ledger(trace: &str, v: &Value) {
    let mut held = BTreeMap::new();
    let mut joined = false;
    let mut pulls = 0;
    for line in trace.lines() {
        assert!(!joined, "event after joined: {line}");
        let fields: Vec<_> = line.split_whitespace().collect();
        if fields[0] == "J" {
            held.clear();
            joined = true;
            continue;
        }
        let token: u64 = fields[1].parse().unwrap();
        assert!(token < v["credits"].as_u64().unwrap());
        match fields[0] {
            "R" => assert_eq!(held.insert(token, "reserved"), None),
            "P" => {
                assert_eq!(held.insert(token, "pulled"), Some("reserved"));
                pulls += 1;
                assert_eq!(fields[2].parse::<u64>().unwrap(), pulls);
            }
            "T" => assert_eq!(held.insert(token, "transferred"), Some("pulled")),
            "C" => assert_eq!(held.remove(&token), Some("transferred")),
            "D" => assert_eq!(held.remove(&token), Some("pulled")),
            "E" => assert_eq!(held.remove(&token), Some("reserved")),
            other => panic!("unexpected ledger event {other}"),
        }
        assert!(held.len() as u64 <= v["credits"].as_u64().unwrap());
    }
    if !matches!(v["error"].as_str().unwrap(), "InputCap" | "Invalid") {
        assert!(joined, "missing cancel/join receipt: {}", v["id"]);
    }
    assert!(held.is_empty());
    if v["id"] == "stop-before-pull" {
        assert_eq!(pulls, 0);
    }
    if v["id"] == "cancel-wait-credit" {
        assert_eq!(pulls, 1);
        assert!(
            !trace.contains("C "),
            "credit released during sink callback"
        );
    }
    if v["id"] == "cancel-wait-data" {
        assert_eq!(pulls, 0);
    }
}

fn verify(output: &[u8], vectors: &[Value]) {
    let mut text = std::str::from_utf8(output).unwrap();
    for v in vectors {
        let header = format!("CASE {}\n", v["id"].as_str().unwrap());
        text = text
            .strip_prefix(&header)
            .unwrap_or_else(|| panic!("missing {header}: {text}"));
        let (rows, rest) = text.split_once("STATUS ").unwrap();
        let actual: Vec<u8> = if v["audit"] == false {
            rows.as_bytes().to_vec()
        } else {
            rows.lines()
                .flat_map(|line| serde_json::from_str::<String>(line).unwrap().into_bytes())
                .collect()
        };
        let (status, rest) = rest.split_once("\nTRACE\n").unwrap();
        let (trace, rest) = rest.split_once("END\n").unwrap();
        let (model, error) = sequential(&input(v), v["stop"].as_u64().unwrap());
        let exceptional = v["sink_failure"] == true
            || v["deadline"].as_u64().unwrap() != 0
            || v["error"] == "Invalid";
        let expected = if exceptional { vec![] } else { model };
        assert_eq!(actual, expected, "{}", v["id"]);
        if v["generated"] != "output-cap" {
            assert_eq!(
                actual,
                v["expected"].as_str().unwrap().as_bytes(),
                "literal {}",
                v["id"]
            );
        }
        if !exceptional {
            assert_eq!(error, v["error"].as_str().unwrap());
        }
        assert_eq!(
            status,
            if v["error"] == "" {
                "ok"
            } else {
                v["error"].as_str().unwrap()
            }
        );
        if v["audit"] != false {
            check_ledger(trace, v);
        }
        text = rest;
    }
    assert!(text.is_empty(), "extra output: {text}");
}

fn partition(ids: &[&str]) {
    hirsel_engine::with_compiler_stack(|| {
        let evidence = std::env::var_os("LOGPIPE_27_EVIDENCE").map(PathBuf::from);
        let scratch = tempfile::Builder::new()
            .prefix("logpipe-27-")
            .tempdir_in(evidence.as_deref().unwrap_or(&std::env::temp_dir()))
            .unwrap();
        let root = scratch.keep();
        eprintln!("LOGPIPE_27_RETAINED {}", root.display());
        let all: Vec<Value> = serde_json::from_str(VECTORS).unwrap();
        let selected: Vec<_> = all
            .into_iter()
            .filter(|v| ids.contains(&v["id"].as_str().unwrap()))
            .collect();
        assert_eq!(selected.len(), ids.len());
        let mut driver = format!("{DRIVER}\npub fn workflow(rt: Io) -> Result[(), AppError] {{\n");
        for v in &selected {
            let bytes = input(v);
            let literal = format!(
                "b\"{}\"",
                bytes
                    .iter()
                    .map(|b| format!("\\x{b:02x}"))
                    .collect::<String>()
            );
            driver.push_str(&format!(
                "run_case(rt, {}, {literal}, {}u64, {}u64, {}u64, {}u64, {}u64, {}, {})?\n",
                v["id"],
                v["credits"],
                v["stop"],
                v["source_delay"],
                v["sink_delay"],
                v["deadline"],
                v["sink_failure"] == true,
                v["audit"] != false
            ));
        }
        driver.push_str("Ok(())\n}\n");
        fs::write(root.join("main.hir"), APP).unwrap();
        fs::write(root.join("driver.hir"), driver).unwrap();
        let manifest = serde_json::json!({"schema":1,"package":{"name":"logpipe","version":"1"},
            "sources":[{"path":"main.hir","module":[]},{"path":"driver.hir","module":[]}],
            "entry":{"module":[],"name":"workflow"},"dependencies":[{"manifest":PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../stdlib/std@1/hirsel.json").canonicalize().unwrap(),"package":{"name":"std","version":"1"},"expose":[]}]});
        fs::write(root.join("hirsel.json"), manifest.to_string()).unwrap();
        fs::write(
            root.join("selected.json"),
            serde_json::to_vec_pretty(&selected).unwrap(),
        )
        .unwrap();
        let mut loaded = hirsel_driver::load_program(&root.join("hirsel.json"))
            .unwrap_or_else(|e| panic!("{e}"));
        loaded
            .check_for_execution()
            .unwrap_or_else(|e| panic!("{e}"));
        let large = ids.contains(&"output-cap");
        if large {
            // A bounded Rust harness fixture, outside the 1M-step corpus.
            loaded.engine.adapters.interpreter =
                Some(Box::new(hirsel_engine::InterpreterAdapter {
                    limits: hirsel_interp::Limits {
                        steps: Some(40_000_000),
                        ..Default::default()
                    },
                    ..Default::default()
                }));
        }
        let config = hirsel_engine::RunConfiguration {
            entry: loaded.entry.clone(),
            hash_seed: 0,
            inputs: vec![],
        };
        let mut world = hirsel_engine::SimulationConfig::default();
        world.projections.insert("monotonic_clock".into());
        world.grants.insert("MonotonicClock".into());
        let mut reference = None;
        let native_only = std::env::var_os("LOGPIPE_27_NATIVE_REFERENCE").map(PathBuf::from);
        for checked in [false, true].into_iter().filter(|_| native_only.is_none()) {
            eprintln!(
                "LOGPIPE_27_STARTED {ids:?} {}",
                if checked { "checked-h" } else { "interpreter" }
            );
            let run = if checked {
                hirsel_engine::run_observed_checked(&mut loaded.engine, &config, world.clone())
            } else {
                hirsel_engine::run_observed_snapshot(&mut loaded.engine, &config, world.clone())
            }
            .unwrap();
            fs::write(
                root.join(if checked {
                    "checked-h.txt"
                } else {
                    "interpreter.txt"
                }),
                format!("{run:?}"),
            )
            .unwrap();
            let observation = Observation::of_reference(
                run.observation
                    .as_ref()
                    .unwrap_or_else(|| panic!("missing observation: {:?}", run.outcome)),
            );
            assert_eq!(
                observation.exit_code,
                Some(0),
                "{:?}: {observation:?}",
                run.outcome
            );
            assert!(observation.failures.is_empty(), "{observation:?}");
            verify(&observation.output, &selected);
            if let Some(before) = &reference {
                assert_eq!(before, &observation);
            } else {
                reference = Some(observation);
            }
            eprintln!(
                "LOGPIPE_27_COMPLETED {ids:?} {}",
                if checked { "checked-h" } else { "interpreter" }
            );
        }
        if let Some(path) = native_only {
            let output = fs::read(path).unwrap();
            verify(&output, &selected);
            reference = Some(Observation {
                exit_code: Some(0),
                blocked: false,
                budget: None,
                signaled: None,
                output,
                failures: vec![],
                roots: vec![],
            });
        } else {
            fs::write(
                root.join("reference.json"),
                &reference.as_ref().unwrap().output,
            )
            .unwrap();
        }
        let matrix: Vec<_> = [
            Engine::Native {
                opt: OptLevel::Speed,
                partition: Partition::Whole,
                runtime: Runtime::Diagnostic,
            },
            Engine::Cranelift {
                partition: Partition::Whole,
                runtime: Runtime::Diagnostic,
            },
        ]
        .into_iter()
        .map(|engine| Configuration {
            options: Options::new(TargetSpec::x86_64_unknown_linux_gnu()),
            engine,
        })
        .collect();
        let toolchain = Toolchain::for_matrix(&root.join("runtime"), &matrix).unwrap();
        fs::create_dir(root.join("native")).unwrap();
        let input = loaded.engine.export_native_input().unwrap();
        let validated = loaded.engine.import_native_input(input).unwrap();
        let results = differential_in(
            &validated,
            &loaded.entry,
            reference.as_ref().unwrap(),
            &matrix,
            &toolchain,
            &root.join("native"),
            &Fixture {
                monotonic: Some(0),
                steps: large.then_some(40_000_000),
                ..Fixture::default()
            },
        );
        let count = results.len();
        for (engine, outcome) in results {
            eprintln!("LOGPIPE_27_COMPLETED {ids:?} {engine}: {outcome:?}");
            assert!(matches!(outcome, Outcome::Agree), "{engine}: {outcome:?}");
        }
        assert_eq!(count, 2);
        if evidence.is_none() {
            fs::remove_dir_all(root).unwrap();
        }
    });
}

#[test]
fn logpipe_27_text() {
    partition(&[
        "empty",
        "one-line",
        "final-without-newline",
        "crlf",
        "multibyte",
        "query-miss",
    ]);
}
#[test]
fn logpipe_27_credit() {
    partition(&[
        "slow-sink-1",
        "slow-sink-2",
        "slow-sink-4",
        "stop-before-pull",
        "stop-after-match",
    ]);
}
#[test]
fn logpipe_27_failure_cancel() {
    partition(&[
        "parser-failure",
        "sink-failure",
        "cancel-wait-credit",
        "cancel-wait-data",
        "credits-zero",
        "credits-nine",
    ]);
}
#[test]
fn logpipe_27_line_limits() {
    partition(&["long-line", "line-boundary"]);
}
#[test]
fn logpipe_27_input_cap() {
    partition(&["input-cap"]);
}
#[test]
fn logpipe_27_output_cap() {
    partition(&["output-cap"]);
}

#[test]
fn logpipe_27_shipped_cli() {
    let scratch = tempfile::tempdir().unwrap();
    let root = scratch.path();
    fs::write(root.join("main.hir"), APP).unwrap();
    fs::write(
        root.join("sample.log"),
        include_bytes!("../../../../examples/logpipe/fixtures/sample.log"),
    )
    .unwrap();
    let mut manifest: Value =
        serde_json::from_str(include_str!("../../../../examples/logpipe/hirsel.json")).unwrap();
    manifest["dependencies"][0]["manifest"] = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../stdlib/std@1/hirsel.json")
        .canonicalize()
        .unwrap()
        .to_str()
        .unwrap()
        .into();
    let selection = root.join("hirsel.json");
    fs::write(&selection, manifest.to_string()).unwrap();
    let mut grants: Value =
        serde_json::from_str(include_str!("../../../../examples/logpipe/grants.json")).unwrap();
    grants["io"]["dirs"][0]["host"] = root.to_str().unwrap().into();
    for (args, expected, status) in [
        (
            vec!["sample.log", "ERROR", "2"],
            include_bytes!("../../../../examples/logpipe/fixtures/sample.expected").as_slice(),
            0,
        ),
        (vec!["absent.log", "ERROR", "1", "0"], b"".as_slice(), 0),
        (vec!["sample.log", "ERROR", "0"], b"".as_slice(), 1),
        (
            vec!["sample.log", "ERROR", "2", "1"],
            b"2: ERROR first\n".as_slice(),
            0,
        ),
    ] {
        let result = std::process::Command::new(env!("CARGO_BIN_EXE_hirsel"))
            .args(["run", "--max-steps", "1000000"])
            .arg(&selection)
            .arg("--grants")
            .arg(grants.to_string())
            .arg("--")
            .args(args)
            .output()
            .unwrap();
        assert_eq!(result.status.code(), Some(status), "{result:?}");
        assert_eq!(result.stdout, expected, "{result:?}");
        if status == 0 {
            assert!(result.stderr.is_empty(), "{result:?}");
        } else {
            assert_eq!(result.stderr, b"error: Invalid\n");
        }
    }
}

//! Pinned Jiff answers; every row executes on reference, checked H, LLVM and Cranelift.
use crate::source_std::load_program_source;
use hirsel_driver::native;
use hirsel_engine::*;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{fmt::Write, path::Path};

#[allow(dead_code)]
#[path = "../support/history.rs"]
mod history;

const VECTORS: &str = include_str!("../fixtures/time-zone-upstream-25/vectors.json");
const HELPERS: &str = r#"
fn dt(y: i32, m: u8, d: u8, h: u8, minute: u8, s: u8, n: u32) -> Option[time.DateTime] {
  let day = match date.new(y,m,d) { Some(v) => v, None() => return None() }
  let clock = match time_of_day.new(h,minute,s,n) { Some(v) => v, None() => return None() }
  Some(time.DateTime(date:day,time:clock))
}
fn show_dt(value: Result[time.DateTime,time.TimeError]) -> String {
  match value {
    Error(OutOfRange()) => "OutOfRange",
    Ok(v) => f"{date.year(v.date)} {date.month(v.date)} {date.day(v.date)} {time_of_day.hour(v.time)} {time_of_day.minute(v.time)} {time_of_day.second(v.time)} {time_of_day.nanosecond(v.time)}",
  }
}
fn stamp(value: Result[time.Timestamp,time.ZoneError]) -> String {
  match value { Ok(v) => f"{v.unix_nanoseconds}", Error(Gap()) => "Gap", Error(Fold()) => "Fold" }
}
"#;

fn sha(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn bytes(data: &[u8]) -> String {
    let mut out = String::from("b\"");
    for byte in data {
        write!(out, "\\x{byte:02x}").unwrap();
    }
    out.push('"');
    out
}

fn args(value: &Value) -> String {
    value
        .as_array()
        .unwrap()
        .iter()
        .map(Value::to_string)
        .collect::<Vec<_>>()
        .join(",")
}

fn fixture_zone(zone: &str) -> &'static [u8] {
    match zone {
        "new_york" => include_bytes!("../fixtures/time-zone-upstream-25/new_york.tzif"),
        "dublin" => include_bytes!("../fixtures/time-zone-upstream-25/dublin.tzif"),
        "tasmania" => include_bytes!("../fixtures/time-zone-upstream-25/tasmania.tzif"),
        "minimal" => include_bytes!("../fixtures/time-zone-upstream-25/minimal.tzif"),
        "sitka" => include_bytes!("../fixtures/time-zone-upstream-25/sitka.tzif"),
        _ => panic!("unknown zone {zone}"),
    }
}

fn statement(row: &Value) -> String {
    let id = row["id"].as_str().unwrap();
    let input = &row["input"];
    match row["operation"].as_str().unwrap() {
        "instant" => format!(
            "let ts = time.Timestamp(unix_nanoseconds:{}i128)\nlet _ = out.print(stdout, f\"{id} {{time_zone.offset(zone,ts)}} {{show_dt(time_zone.to_datetime(zone,ts))}}\\n\")\n",
            input.as_str().unwrap()
        ),
        "civil" => format!(
            "let wall = match dt({}) {{ Some(v) => v, None() => {{ failure.check(false,Arithmetic())\nreturn Ok(()) }} }}\nlet _ = out.print(stdout, f\"{id} {{stamp(time_zone.to_timestamp(zone,wall,Earlier()))}} {{stamp(time_zone.to_timestamp(zone,wall,Later()))}} {{stamp(time_zone.to_timestamp(zone,wall,Reject()))}}\\n\")\n",
            args(input)
        ),
        "date" => format!(
            "let valid = match date.new({}) {{ Some(_) => true, None() => false }}\nlet _ = out.print(stdout,f\"{id} {{valid}}\\n\")\n",
            args(input)
        ),
        "clock" => format!(
            "let valid = match time_of_day.new({}) {{ Some(_) => true, None() => false }}\nlet _ = out.print(stdout,f\"{id} {{valid}}\\n\")\n",
            args(input)
        ),
        "tzif" => {
            let data: Vec<_> = input["bytes"]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| u8::try_from(v.as_u64().unwrap()).unwrap())
                .collect();
            format!(
                "let parsed = match time_zone.from_tzif({}) {{ Ok(_) => \"Ok\", Error(Invalid()) => \"Invalid\", Error(OutOfRange()) => \"OutOfRange\" }}\nlet _ = out.print(stdout,f\"{id} {{parsed}}\\n\")\n",
                bytes(&data)
            )
        }
        op => panic!("unknown operation {op}"),
    }
}

fn partition(zone: &str, operation: &str, part: usize) {
    let vectors: Value = serde_json::from_str(VECTORS).unwrap();
    let all = vectors["rows"].as_array().unwrap();
    assert_eq!(all.len(), 160);
    assert_eq!(
        all.iter()
            .map(|r| r["id"].as_str().unwrap())
            .collect::<std::collections::BTreeSet<_>>()
            .len(),
        all.len()
    );
    let rows: Vec<_> = vectors["rows"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|r| r["zone"] == zone && (operation == "controls" || r["operation"] == operation))
        .skip(part * 10)
        .take(10)
        .collect();
    assert!(!rows.is_empty());
    let mut source = String::from(HELPERS);
    source
        .push_str("pub fn main(rt: Io) -> Result[(), GrantError] {\nlet stdout = io.stdout(rt)?\n");
    if zone == "utc" {
        source.push_str("let zone = time_zone.utc()\n");
    } else if zone != "controls" {
        let data = fixture_zone(zone);
        assert_eq!(
            sha(data),
            vectors["zones"][zone]["sha256"].as_str().unwrap()
        );
        writeln!(source,"let zone = match time_zone.from_tzif({}) {{ Ok(v) => v, Error(_) => {{ failure.check(false,Arithmetic())\ntime_zone.utc() }} }}",bytes(data)).unwrap();
    }
    let mut expected = Vec::new();
    for row in &rows {
        let zone_hash = vectors["zones"][zone]["sha256"].as_str();
        let input = serde_json::json!({"zone_sha256":zone_hash,"operation":row["operation"],"input":row["input"]});
        assert_eq!(
            sha(&serde_json::to_vec(&input).unwrap()),
            row["input_sha256"].as_str().unwrap()
        );
        let output = row["expected_output"].as_str().unwrap();
        assert_eq!(
            sha(output.as_bytes()),
            row["expected_sha256"].as_str().unwrap()
        );
        writeln!(source, "{{\n{}\n}}", statement(row)).unwrap();
        expected.extend_from_slice(output.as_bytes());
    }
    source.push_str("Ok(())\n}\n");
    let ids: Vec<_> = rows.iter().map(|r| r["id"].as_str().unwrap()).collect();
    let name = format!("{zone}-{operation}-{part}");
    let dir = tempfile::Builder::new()
        .prefix("time-zone-upstream-25-")
        .tempdir()
        .unwrap();
    with_compiler_stack(|| {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/std_numeric.hir");
        let mut loaded = load_program_source(&path, Some(&source))
            .unwrap_or_else(|e| panic!("SourceRejection {ids:?}: {e}"));
        loaded
            .check_for_execution()
            .unwrap_or_else(|e| panic!("SourceRejection {ids:?}: {e}\n{source}"));
        let input = loaded
            .engine
            .export_native_input()
            .unwrap_or_else(|e| panic!("V-export {ids:?}: {e:?}"));
        let validated = loaded
            .engine
            .import_native_input(input)
            .unwrap_or_else(|e| panic!("VRejection {ids:?}: {e:?}"));
        // This bounded Rust harness lifts only the corpus interpreter step budget.
        loaded.engine.adapters.interpreter = Some(Box::new(InterpreterAdapter {
            limits: hirsel_interp::Limits {
                steps: None,
                call_depth: 256,
                source: None,
            },
            ..Default::default()
        }));
        let run = run_observed_snapshot(
            &mut loaded.engine,
            &RunConfiguration {
                entry: loaded.entry.clone(),
                hash_seed: 0,
                inputs: vec![],
            },
            SimulationConfig::default(),
        )
        .unwrap();
        let observation = native::Observation::of_reference(
            run.observation.as_ref().expect("reference observation"),
        );
        let target = native::TargetSpec::x86_64_unknown_linux_gnu();
        let matrix = [
            native::Engine::Heap { campaign: false },
            native::Engine::Native {
                opt: native::OptLevel::None,
                partition: native::Partition::Whole,
                runtime: native::Runtime::Diagnostic,
            },
            native::Engine::Cranelift {
                partition: native::Partition::Whole,
                runtime: native::Runtime::Diagnostic,
            },
        ]
        .into_iter()
        .map(|engine| native::Configuration {
            engine,
            options: native::Options::new(target.clone()),
        })
        .collect();
        let runner = history::Runner::with_matrix(dir.path(), matrix)
            .expect("required four-engine toolchain");
        let report = runner.compare(&validated, &loaded.entry, &observation, &name);
        assert_eq!(report.cells.len(), 4, "missing required engine: {report:?}");
        let mut failed = Vec::new();
        for cell in &report.cells {
            let pass = cell.category == history::Category::Observation
                && cell.observation.as_ref().is_some_and(|o| {
                    o.output == expected
                        && o.exit_code == Some(0)
                        && o.failures.is_empty()
                        && !o.blocked
                        && o.budget.is_none()
                        && o.signaled.is_none()
                });
            eprintln!(
                "TZ25 {}",
                serde_json::json!({"partition":name,"engine":cell.engine,"selected":rows.len(),"completed":if cell.category == history::Category::Observation { rows.len() } else {0},"passed":pass,"category":format!("{:?}",cell.category),"rows":rows.iter().map(|r| { let line=r["expected_output"].as_str().unwrap(); let actual=cell.observation.as_ref().and_then(|o|std::str::from_utf8(&o.output).ok()).and_then(|text|text.split_inclusive('\n').find(|line|line.starts_with(&format!("{} ",r["id"].as_str().unwrap())))); serde_json::json!({"id":r["id"],"completed":actual.is_some(),"passed":actual==Some(line),"actual":actual}) }).collect::<Vec<_>>(),"ids":ids,"input_sha256":rows.iter().map(|r|r["input_sha256"].as_str().unwrap()).collect::<Vec<_>>(),"expected_sha256":sha(&expected),"source_sha256":sha(source.as_bytes())})
            );
            if !pass {
                failed.push(format!("{} {:?} {:?}", cell.engine, cell.category, cell));
            }
        }
        assert!(
            failed.is_empty(),
            "{name} {ids:?}: {failed:?}\nexpected {}\nsource {source}",
            String::from_utf8_lossy(&expected)
        );
    });
}

macro_rules! partitions {
    ($zone:literal,$op:literal; $($name:ident:$part:literal),+ $(,)?) => {$(#[test] fn $name() { partition($zone,$op,$part); })+};
}
partitions!("new_york","civil"; new_york_civil_0:0,new_york_civil_1:1,new_york_civil_2:2);
partitions!("new_york","instant"; new_york_instant_0:0,new_york_instant_1:1);
partitions!("dublin","civil"; dublin_civil_0:0,dublin_civil_1:1);
partitions!("dublin","instant"; dublin_instant_0:0,dublin_instant_1:1);
partitions!("tasmania","civil"; tasmania_civil_0:0,tasmania_civil_1:1);
partitions!("tasmania","instant"; tasmania_instant_0:0,tasmania_instant_1:1);
partitions!("sitka","civil"; sitka_civil_0:0,sitka_civil_1:1);
partitions!("sitka","instant"; sitka_instant_0:0,sitka_instant_1:1);
partitions!("utc","civil"; utc_civil:0);
partitions!("controls","controls"; controls_0:0,controls_1:1);

partitions!("minimal","instant"; minimal_instant:0);
